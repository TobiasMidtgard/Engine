//! Setting the stage: the planet's ground, its air, and the woods (the sea is in `sea`).

use std::collections::HashMap;
use std::sync::Arc;

use engine::prelude::*;
use engine_foliage::{PlantLibrary, SceneryTree};
use engine_terrain::{HeightOverlay, PlanetTerrain};

use crate::build::Forest;
use crate::camera::StrategyCam;
use crate::globe::{self, Biome, Frame};
use crate::grid;
use crate::island;

/// How thickly to plant the woods (1 = a tree about every 8 m in a wood).
#[derive(Resource, Clone, Copy)]
pub struct WoodsDensity(pub f32);

/// The middle of the side of the planet the lands lie on (planet-local unit direction): the
/// hand-made ground is laid over the half of the globe around it.
fn lands_middle() -> DVec3 {
    globe::regions()
        .iter()
        .map(|r| r.anchor())
        .sum::<DVec3>()
        .normalize()
}

/// The planet's air: Earth's, scaled to a small planet so the sky is as blue from the ground
/// and the horizon as hazy, with a thin bright rim from space.
fn air(frame: &Frame) -> engine::render::Atmosphere {
    let r = globe::RADIUS_M;
    let k = (6_360_000.0 / r * 0.1) as f32;
    let mut a = engine::render::Atmosphere {
        planet_center: frame.center(),
        planet_radius: r,
        ..Default::default()
    };
    a.thickness /= k;
    a.rayleigh_scale_height /= k;
    a.mie_scale_height /= k;
    for c in a
        .rayleigh_scattering
        .iter_mut()
        .chain(a.mie_scattering.iter_mut())
        .chain(a.mie_absorption.iter_mut())
        .chain(a.ozone_absorption.iter_mut())
    {
        *c *= k;
    }
    a
}

/// Spawn the planet's ground and set the sky.
pub fn spawn_world(world: &mut World) {
    let r = globe::RADIUS_M;
    let mut terrain = PlanetTerrain::earthlike(7).with_radius(r);
    // The planet's own ground is all sea bed; every land is the game's.
    terrain.shape.ocean_fraction = 1.0;
    terrain.shape.rivers = false;
    terrain.shape.mountain_coverage = 0.0;
    terrain.shape.ocean_depth_m = -island::OCEAN_M;
    terrain.shape.plateau_height_m = 0.0;
    terrain.shape.hill_height_m = 0.0;
    terrain.shape.downs_height_m = 0.0;
    terrain.shape.detail_height_m = 0.0;
    terrain.shape.continent_scale_m = r * 0.5;
    // The sea is the game's own (`sea`), and no ice sheet hides the north.
    terrain.ocean_surface = false;
    terrain.shape.climate.pole_c = 6.0;
    terrain.colliders = false;
    terrain.landmarks = Default::default();
    // The game's ground over the half of the planet the lands lie on, read straight off the
    // sphere (the overlay's flat mode hands each direction's own point to the function).
    let to_cap = DQuat::from_rotation_arc(lands_middle(), DVec3::Y);
    let from_cap = to_cap.inverse();
    let dir = move |x: f64, z: f64| from_cap * DVec3::new(x, r, z).normalize();
    terrain.shape.overlay = Some(HeightOverlay {
        height: Arc::new(move |x, z| island::ground(dir(x, z))),
        radius_m: r * 72f64.to_radians().tan(),
        blend_m: r * 0.3,
        rotation: to_cap,
        translation: DVec3::ZERO,
        flat: true,
        y_range: island::HEIGHT_RANGE_M,
        paint: Some(Arc::new(move |x, z| island::ground_paint(dir(x, z)))),
    });
    let frame = Frame::default();
    world.insert_resource(frame);
    world.spawn((
        Name::new("Archipelago planet"),
        Transform::from_translation(frame.pos).with_rotation(frame.rot.as_quat()),
        terrain,
        globe::Planet,
    ));
    world.insert_resource(air(&frame));
    if let Some(mut sky) = world.get_resource_mut::<engine_weather::Sky>() {
        sky.hour = 15.5;
        sky.day_of_year = 160.0;
        sky.hours_per_second = 0.0;
    }
    if let Some(mut w) = world.get_resource_mut::<engine_weather::Weather>() {
        w.preset = engine_weather::WeatherPreset::Clear;
        w.transition_s = 0.0;
        w.auto = false;
        w.wind_from_deg = 240.0;
    }
}

/// Keep the air round the planet as the world moves under the camera.
pub fn follow_air(
    frame: Res<Frame>,
    cams: Query<&StrategyCam>,
    mut air: ResMut<engine::render::Atmosphere>,
    mut settings: ResMut<engine::render::RenderSettings>,
) {
    if air.planet_center != frame.center() {
        air.planet_center = frame.center();
    }
    // From high up the haze table's depth slices show as pale rings on the sea (this small
    // planet's air is thin and thins fast): there every pixel marches the air itself.
    let high = cams.iter().next().is_some_and(|c| c.dist > 2_500.0);
    if settings.aerial_table == high {
        settings.aerial_table = !high;
    }
}

// ------------------------------------------------------------------------------ the woods

/// Side of a woods tile, meters.
const WOODS_TILE_M: f64 = 128.0;
/// Within this distance of where the camera looks every tree stands; out to `THIN_M` one in
/// four, and beyond that one in sixteen (the dark floor of the woods carries the rest). The
/// distances count from further out the higher the camera is.
const DENSE_M: f64 = 600.0;
const THIN_M: f64 = 1_400.0;
/// Beyond this one in sixty-four, and beyond `FEW_M` one in 256, drawn larger.
const SPARSE_M: f64 = 3_000.0;
const FEW_M: f64 = 5_000.0;
/// Above this camera distance trees are too small to see and none are kept.
const NO_TREES_ABOVE_M: f32 = 12_000.0;

/// The woods standing now: trees are planted around where the camera looks and taken up
/// again far from it (four continents hold a few million).
#[derive(Resource, Default)]
pub struct Woods {
    tiles: HashMap<IVec2, Stand>,
    /// The first planting is done at once; later ones a little a frame.
    started: bool,
}

struct Stand {
    /// 0 every tree, 1 one in four, 2 one in sixteen.
    thin: u8,
    trees: Vec<(IVec2, Entity)>,
}

fn hash(x: i64, z: i64, k: u64) -> f64 {
    engine::core::math::hash01(x, z, 0, k) as f64
}

/// One tree to plant: map point, species, seed, size, turn.
type Seedling = (DVec3, &'static str, u64, f32, f32);

/// The tree species at a spot: by the region's climate, the height and the coast.
fn species(isle: &island::Island, h: f64, i: i64, j: i64) -> &'static str {
    let pick = hash(i, j, 5);
    match isle.biome() {
        Biome::Temperate => {
            // Conifers on the hills and the north island, broadleaf on low ground.
            let conifer = (h / 40.0
                + hash(i, j, 4) * 0.6
                + if isle.name == "Northmoor" { 0.35 } else { 0.0 })
            .clamp(0.0, 1.0);
            if conifer > 0.7 {
                if pick > 0.5 { "spruce" } else { "pine" }
            } else if hash(i, j, 6) > 0.6 {
                "birch"
            } else {
                "oak"
            }
        }
        Biome::Tropical => {
            // Palms along the shore, the tall forest behind.
            if h < 6.0 + 6.0 * hash(i, j, 4) {
                if pick > 0.15 {
                    "coconut palm"
                } else {
                    "tropical canopy"
                }
            } else if pick > 0.45 {
                "tropical canopy"
            } else {
                "meranti"
            }
        }
        Biome::Arid => {
            if h < 5.0 && pick > 0.6 {
                "coconut palm"
            } else if h > 70.0 || pick < 0.2 {
                "juniper"
            } else {
                "umbrella thorn"
            }
        }
        Biome::Arctic => {
            if pick > 0.45 {
                "black spruce"
            } else if pick > 0.25 {
                "spruce"
            } else if pick > 0.12 {
                "larch"
            } else {
                "mountain birch"
            }
        }
    }
}

/// The trees of one woods tile. A thinned tile keeps one tree in four or in sixteen, the
/// largest ones.
fn seedlings(tile: IVec2, thin: u8, spacing: f64) -> Vec<Seedling> {
    let (x0, z0) = (tile.x as f64 * WOODS_TILE_M, tile.y as f64 * WOODS_TILE_M);
    let mid = DVec2::new(x0 + WOODS_TILE_M * 0.5, z0 + WOODS_TILE_M * 0.5);
    // Only tiles on or beside land.
    let near_land = island::islands()
        .iter()
        .any(|isle| (mid - isle.center).length() < isle.radius * 1.5 + WOODS_TILE_M);
    let mut out = Vec::new();
    if !near_land {
        return out;
    }
    let (i0, j0) = ((x0 / spacing).ceil() as i64, (z0 / spacing).ceil() as i64);
    let (i1, j1) = (
        ((x0 + WOODS_TILE_M) / spacing).ceil() as i64,
        ((z0 + WOODS_TILE_M) / spacing).ceil() as i64,
    );
    for j in j0..j1 {
        for i in i0..i1 {
            // (By lot, not every so-manieth: rows of trees would show from above. A tree
            // kept at one thinning is kept at every lighter one.)
            if thin > 0 && hash(i, j, 9) >= 0.25f64.powi(thin as i32) {
                continue;
            }
            let x = (i as f64 + (hash(i, j, 1) - 0.5) * 0.9) * spacing;
            let z = (j as f64 + (hash(i, j, 2) - 0.5) * 0.9) * spacing;
            let w = island::woods(x, z);
            if w <= 0.02 || hash(i, j, 3) > w {
                continue;
            }
            if island::slope(x, z, 3.0) > 0.9 {
                continue;
            }
            let Some(isle) = island::island_at(x, z).map(|k| &island::islands()[k]) else {
                continue;
            };
            let h = island::height(x, z);
            let mut size = 0.65 + 0.5 * hash(i, j, 7) as f32;
            if thin > 0 {
                size = (size * (1.0 + 0.2 * thin as f32)).min(1.3);
            }
            out.push((
                DVec3::new(x, h - 0.1, z),
                species(isle, h, i, j),
                (i as u64) << 20 ^ (j as u64 & 0xF_FFFF) ^ isle.seed << 40,
                size,
                hash(i, j, 8) as f32,
            ));
        }
    }
    out
}

/// Plant the woods around where the camera looks and take up the ones far from it.
pub fn tend_woods(world: &mut World) {
    let Some(library) = world.get_resource::<PlantLibrary>().cloned() else {
        return;
    };
    let mut q = world.query::<&StrategyCam>();
    let Some((focus, dist)) = q.iter(world).next().map(|c| (c.goal_focus, c.goal_dist)) else {
        return;
    };
    let density = world
        .get_resource::<WoodsDensity>()
        .map_or(1.0, |d| d.0)
        .max(0.05);
    let spacing = 8.0 / (density as f64).sqrt();
    let reach = if dist > NO_TREES_ABOVE_M {
        0.0
    } else {
        (dist as f64 * 1.3 + 500.0).clamp(700.0, 7_000.0)
    };
    // From high up no tree is near: the bands count from further out.
    let lift = (dist as f64 - 500.0).max(0.0) * 0.5;
    let mut woods = world.remove_resource::<Woods>().unwrap_or_default();
    let at = DVec2::new(focus.x, focus.z);
    let mid = |t: IVec2| {
        DVec2::new(
            (t.x as f64 + 0.5) * WOODS_TILE_M,
            (t.y as f64 + 0.5) * WOODS_TILE_M,
        )
    };
    // Take up what is far away (well beyond where planting starts, so nothing flickers).
    let far: Vec<IVec2> = woods
        .tiles
        .keys()
        .copied()
        .filter(|t| (mid(*t) - at).length() > reach * 1.25 + WOODS_TILE_M)
        .collect();
    for t in far {
        take_up(world, &mut woods, t);
    }
    // What should stand, nearest first.
    let n = (reach / WOODS_TILE_M).ceil() as i32;
    let c = IVec2::new(
        (at.x / WOODS_TILE_M).floor() as i32,
        (at.y / WOODS_TILE_M).floor() as i32,
    );
    let mut want: Vec<(f64, IVec2, u8)> = Vec::new();
    if reach > 0.0 {
        for dz in -n..=n {
            for dx in -n..=n {
                let t = c + IVec2::new(dx, dz);
                let d = (mid(t) - at).length();
                if d > reach {
                    continue;
                }
                // Thin out a little further than where thicker planting starts, for the
                // same reason.
                let had = woods.tiles.get(&t).map(|s| s.thin);
                let band =
                    |edge: f64, inner: bool| d + lift < if inner { edge * 1.2 } else { edge };
                let thin = if band(DENSE_M, had == Some(0)) {
                    0
                } else if band(THIN_M, had.is_some_and(|h| h <= 1)) {
                    1
                } else if band(SPARSE_M, had.is_some_and(|h| h <= 2)) {
                    2
                } else if band(FEW_M, had.is_some_and(|h| h <= 3)) {
                    3
                } else {
                    4
                };
                if had != Some(thin) {
                    want.push((d, t, thin));
                }
            }
        }
    }
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    let unhurried = woods.started && !world.resource::<EngineConfig>().headless;
    let started = std::time::Instant::now();
    let frame = *world.resource::<Frame>();
    for (_, t, thin) in want {
        if unhurried && started.elapsed().as_secs_f64() > 0.004 {
            break;
        }
        take_up(world, &mut woods, t);
        let mut trees = Vec::new();
        for (p, name, seed, size, yaw) in seedlings(t, thin, spacing) {
            let cell = grid::cell_of(p.x, p.z);
            if world.resource::<Forest>().cleared.contains(&cell) {
                continue;
            }
            let Some(mut tree) = SceneryTree::new(&library, name, seed) else {
                continue;
            };
            tree.size = size;
            let mut at = frame.stand(p, yaw * std::f32::consts::TAU);
            if thin >= 3 {
                // The few that stand for a whole wood from far above.
                at.scale = Vec3::splat(if thin == 3 { 1.7 } else { 2.3 });
            }
            let e = world.spawn((at, tree)).id();
            world
                .resource_mut::<Forest>()
                .by_cell
                .entry(cell)
                .or_default()
                .push(e);
            trees.push((cell, e));
        }
        woods.tiles.insert(t, Stand { thin, trees });
    }
    woods.started = true;
    world.insert_resource(woods);
}

/// Remove a tile's trees.
fn take_up(world: &mut World, woods: &mut Woods, tile: IVec2) {
    let Some(stand) = woods.tiles.remove(&tile) else {
        return;
    };
    for (cell, e) in stand.trees {
        let mut forest = world.resource_mut::<Forest>();
        let mut standing = false;
        if let Some(v) = forest.by_cell.get_mut(&cell) {
            if let Some(k) = v.iter().position(|x| *x == e) {
                v.swap_remove(k);
                standing = true;
            }
            if v.is_empty() {
                forest.by_cell.remove(&cell);
            }
        }
        // Trees a building cleared are gone already.
        if standing {
            world.despawn(e);
        }
    }
}
