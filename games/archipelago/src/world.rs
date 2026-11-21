//! Setting the stage: the planet's ground, its air, and the woods (the sea is in `sea`).

use std::collections::HashMap;
use std::sync::Arc;

use engine::prelude::*;
use engine_foliage::lod_fade::LodFades;
use engine_foliage::{PlantLibrary, RetireTree, SceneryTree, StreamedTree};
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
/// How far the woods are thinned by distance from where the camera looks: (distance m, k),
/// where a tree stands while its lot is under `0.25^k` (k 0 every tree, 1 one in four, 2 one in
/// sixteen, and so on: the dark floor of the woods carries the rest). Between the marks it
/// thins smoothly, a quarter step at a time, so a wood loses or gains a few trees at once
/// rather than a whole tile of them (Brightsund note 12, tree pop-in). The distances count
/// from further out the higher the camera is.
const THINNING: [(f64, f32); 5] = [
    (450.0, 0.0),
    (950.0, 1.0),
    (2_050.0, 2.0),
    (3_950.0, 3.0),
    (5_950.0, 4.0),
];
/// A stand is thinned or thickened in steps of this (a quarter step keeps 71% of the trees or
/// adds 41%), and only once the wanted thinning has moved this much past the half step (no
/// flicker on the edge).
const THIN_STEP: f32 = 0.25;
const THIN_HYSTERESIS: f32 = 0.05;
/// Above this camera distance trees are too small to see and none are kept.
const NO_TREES_ABOVE_M: f32 = 12_000.0;

/// The thinning wanted at a distance (meters, from where the camera looks plus its lift).
fn thinning(d: f64) -> f32 {
    let (d0, _) = THINNING[0];
    if d <= d0 {
        return 0.0;
    }
    for w in THINNING.windows(2) {
        let ((a, ka), (b, kb)) = (w[0], w[1]);
        if d < b {
            return ka + (kb - ka) * ((d - a) / (b - a)) as f32;
        }
    }
    THINNING[THINNING.len() - 1].1
}

/// The scale the trees of a stand thinned by `k` are drawn at: the few standing for a whole
/// wood from far above are drawn larger, growing with the thinning (no jump).
fn far_scale(k: f32) -> f32 {
    1.0 + 0.7 * (k - 2.0).clamp(0.0, 1.0) + 0.6 * (k - 3.0).clamp(0.0, 1.0)
}

/// The woods standing now: trees are planted around where the camera looks and taken up
/// again far from it (four continents hold a few million).
#[derive(Resource, Default)]
pub struct Woods {
    tiles: HashMap<IVec2, Stand>,
    /// The first planting is done at once; later ones a little a frame.
    started: bool,
}

struct Stand {
    /// How far it is thinned (see [`THINNING`]), a whole number of [`THIN_STEP`]s.
    k: f32,
    /// Its trees: cell, entity, lot.
    trees: Vec<(IVec2, Entity, f64)>,
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

/// The trees of one woods tile whose lot lies in `lots` (a stand thinned by k keeps the lots
/// under `0.25^k`: the largest trees, so a tree kept at one thinning is kept at every lighter
/// one, and the size of each never changes with the thinning).
fn seedlings(tile: IVec2, lots: std::ops::Range<f64>, spacing: f64) -> Vec<(Seedling, f64)> {
    let (x0, z0) = (tile.x as f64 * WOODS_TILE_M, tile.y as f64 * WOODS_TILE_M);
    let mid = DVec2::new(x0 + WOODS_TILE_M * 0.5, z0 + WOODS_TILE_M * 0.5);
    // Only tiles on or beside land.
    let near_land = island::islands()
        .iter()
        .any(|isle| (mid - isle.center).length() < isle.radius * 1.5 + WOODS_TILE_M);
    let mut out = Vec::new();
    if !near_land || lots.is_empty() {
        return out;
    }
    let (i0, j0) = ((x0 / spacing).ceil() as i64, (z0 / spacing).ceil() as i64);
    let (i1, j1) = (
        ((x0 + WOODS_TILE_M) / spacing).ceil() as i64,
        ((z0 + WOODS_TILE_M) / spacing).ceil() as i64,
    );
    for j in j0..j1 {
        for i in i0..i1 {
            // (By lot, not every so-manieth: rows of trees would show from above.)
            let lot = hash(i, j, 9);
            if !lots.contains(&lot) {
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
            // The trees kept furthest out are the big ones of the wood (a fifth larger for
            // each thinning they outlast).
            let outlasts = (-lot.max(1e-9).log(4.0)).floor().clamp(0.0, 4.0) as f32;
            let size = ((0.65 + 0.5 * hash(i, j, 7) as f32) * (1.0 + 0.2 * outlasts)).min(1.3);
            out.push((
                (
                    DVec3::new(x, h - 0.1, z),
                    species(isle, h, i, j),
                    (i as u64) << 20 ^ (j as u64 & 0xF_FFFF) ^ isle.seed << 40,
                    size,
                    hash(i, j, 8) as f32,
                ),
                lot,
            ));
        }
    }
    out
}

/// The lots a stand thinned by `k` keeps.
fn kept(k: f32) -> f64 {
    0.25f64.powf(k as f64)
}

/// Plant the woods around where the camera looks and take up the ones far from it. A stand
/// that thins or thickens as the view moves takes up or plants only the trees that change,
/// and they dither out and in (the engine's `StreamedTree` and `RetireTree`).
pub fn tend_woods(world: &mut World) {
    let Some(library) = world.get_resource::<PlantLibrary>().cloned() else {
        return;
    };
    let mut q = world.query::<&StrategyCam>();
    // Plant where the view is headed, but thin by the nearer of where it is and where it is
    // going (zooming out, the woods thin as the camera climbs, not before).
    let Some((focus, dist)) = q
        .iter(world)
        .next()
        .map(|c| (c.goal_focus, c.dist.min(c.goal_dist)))
    else {
        return;
    };
    // A smooth zoom or pan high over the land moves the camera further in a frame than the
    // engine's cut distance: only a jump counts as a cut (then trees appear at once).
    if let Some(mut fades) = world.get_resource_mut::<LodFades>() {
        let cut = (dist as f64 * 0.12).max(engine_foliage::lod_fade::JUMP_M);
        if fades.cut_m != cut {
            fades.cut_m = cut;
        }
    }
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
    // From high up no tree is near: the thinning counts from further out.
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
        if let Some(stand) = woods.tiles.remove(&t) {
            retire(world, stand.trees);
        }
    }
    // What should change, nearest first.
    let n = (reach / WOODS_TILE_M).ceil() as i32;
    let c = IVec2::new(
        (at.x / WOODS_TILE_M).floor() as i32,
        (at.y / WOODS_TILE_M).floor() as i32,
    );
    let mut want: Vec<(f64, IVec2, f32)> = Vec::new();
    if reach > 0.0 {
        for dz in -n..=n {
            for dx in -n..=n {
                let t = c + IVec2::new(dx, dz);
                let d = (mid(t) - at).length();
                if d > reach {
                    continue;
                }
                let k = thinning(d + lift);
                let step = (k / THIN_STEP).round() * THIN_STEP;
                match woods.tiles.get(&t) {
                    Some(s) if s.k == step => {}
                    Some(s) if (k - s.k).abs() < THIN_STEP * 0.5 + THIN_HYSTERESIS => {}
                    _ => want.push((d, t, step)),
                }
            }
        }
    }
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    let unhurried = woods.started && !world.resource::<EngineConfig>().headless;
    let started = std::time::Instant::now();
    let frame = *world.resource::<Frame>();
    for (_, t, k) in want {
        if unhurried && started.elapsed().as_secs_f64() > 0.004 {
            break;
        }
        let mut stand = woods.tiles.remove(&t).unwrap_or(Stand {
            k: f32::INFINITY,
            trees: Vec::new(),
        });
        // Thinner: the trees past the new thinning go.
        let keep = kept(k);
        let (stay, gone): (Vec<_>, Vec<_>) = stand.trees.into_iter().partition(|x| x.2 < keep);
        retire(world, gone);
        stand.trees = stay;
        // Thicker: plant the lots between the old thinning and the new.
        let had = if stand.k.is_finite() {
            kept(stand.k)
        } else {
            0.0
        };
        let scale = far_scale(k);
        for ((p, name, seed, size, yaw), lot) in seedlings(t, had..keep, spacing) {
            let cell = grid::cell_of(p.x, p.z);
            if world.resource::<Forest>().cleared.contains(&cell) {
                continue;
            }
            let Some(mut tree) = SceneryTree::new(&library, name, seed) else {
                continue;
            };
            tree.size = size;
            let at = frame
                .stand(p, yaw * std::f32::consts::TAU)
                .with_scale(Vec3::splat(scale));
            let e = world.spawn((at, tree, StreamedTree)).id();
            world
                .resource_mut::<Forest>()
                .by_cell
                .entry(cell)
                .or_default()
                .push(e);
            stand.trees.push((cell, e, lot));
        }
        // The ones that stay grow or shrink to the new thinning's scale.
        if stand.k.is_finite() && far_scale(stand.k) != scale {
            for (_, e, _) in &stand.trees {
                if let Some(mut tf) = world.get_mut::<Transform>(*e) {
                    tf.scale = Vec3::splat(scale);
                }
            }
        }
        stand.k = k;
        woods.tiles.insert(t, stand);
    }
    woods.started = true;
    world.insert_resource(woods);
}

/// Take trees away (they dither out), unless a building cleared them already.
fn retire(world: &mut World, trees: Vec<(IVec2, Entity, f64)>) {
    for (cell, e, _) in trees {
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
        if standing && let Ok(mut tree) = world.get_entity_mut(e) {
            tree.insert(RetireTree::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The woods thin smoothly with distance (no step a whole tile of trees would jump at),
    /// every tree near the view, one in 256 far out; the far trees grow without a jump.
    #[test]
    fn the_woods_thin_without_steps() {
        assert_eq!(thinning(0.0), 0.0);
        assert_eq!(thinning(20_000.0), 4.0);
        let mut last = 0.0;
        for i in 1..8_000 {
            let k = thinning(i as f64);
            assert!(k >= last && k - last < 0.01, "jump at {i} m: {last} to {k}");
            let (a, b) = (far_scale(last), far_scale(k));
            assert!(b >= a && b - a < 0.01, "scale jumps at {i} m");
            last = k;
        }
        // A tree kept at a heavier thinning is kept at every lighter one.
        assert!(kept(1.0) > kept(1.25) && kept(1.25) > kept(2.0));
    }
}
