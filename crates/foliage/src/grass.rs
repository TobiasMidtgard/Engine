//! Meadow grass on the GPU: every ecology region where grass grows gets one
//! `engine_render::GrassField` over its rectangle, so the ground layer is millions of blades
//! that sway, part around characters (`GrassBender`) and thin out with distance, instead of
//! one mesh per tuft.
//!
//! The ecology stays the source of truth. Each living grass plant in the region spreads its
//! cover over the field's density map (a healthy, grown tuft covers about a square meter),
//! so grass is thick in the clear-cut and the burn scar, thin under the canopy and absent
//! where nothing lets it grow: keep-clear circles (the trail, the pond), logs, rocks, water.
//! Dryness follows the soil's water and the grass's own health, so a drought or a fire
//! browns it, and the season ([`crate::season::grass_dryness`]) turns it to straw in winter.
//! The maps are rebuilt every few seconds; heights come from `Ground` once and again after
//! `TerrainChanged`. Grass plants under a field draw nothing themselves.

use engine_core::math::{noise::fbm2, smoothstep};
use engine_core::prelude::*;
use engine_render::{GrassField, GrassStyle};
use engine_world::Ground;

use crate::ecology::{InRegion, RegionState};
use crate::felling::{LOG_BARE_YEARS, LogPlaced};
use crate::plant::{EcologyRegion, EcologySettings, FelledLog, Plant};
use crate::scenery::Rock;
use crate::season::{FoliageSeason, grass_dryness};
use crate::species::{PlantForm, PlantLibrary};

/// Spacing of the field's map grid, meters.
pub const GRASS_CELL_M: f32 = 1.0;
/// Most grid points along one side of a field's maps.
const MAX_RESOLUTION: u32 = 512;
/// Seconds (real time) between map rebuilds.
const REBUILD_SECS: f32 = 3.0;

/// On a region: the entity drawing its GPU grass.
#[derive(Component, Debug, Clone, Copy)]
pub struct RegionGrass(pub Entity);

/// On a grass field entity: the region it belongs to.
#[derive(Component, Debug, Clone, Copy)]
pub struct GrassOf(pub Entity);

/// The look of a region's blades: its grass species' colors and height.
pub fn region_style(library: &PlantLibrary, region: &EcologyRegion) -> Option<GrassStyle> {
    let grass = region
        .species
        .iter()
        .filter_map(|n| library.id(n))
        .map(|id| library.get(id))
        .find(|sp| sp.form == PlantForm::Grass)?;
    let lush = grass.leaf_color.map(|c| c * 0.62);
    let straw = [0.42, 0.34, 0.16];
    let dry = std::array::from_fn(|i| grass.accent_color[i] * 0.35 + straw[i] * 0.65);
    Some(GrassStyle {
        blades_per_m2: 420.0,
        height: grass.max_height_m * 0.7,
        height_variation: 0.45,
        width: 0.011,
        bend: 0.4,
        lush_color: lush,
        dry_color: dry,
        dryness: 0.2,
        wind_response: 1.0,
        draw_distance: 65.0,
        shadow_distance: 18.0,
    })
}

/// Grid points along x and z for a region of `half_extents`.
fn resolution(half_extents: [f32; 2]) -> [u32; 2] {
    half_extents.map(|h| ((h * 2.0 / GRASS_CELL_M).ceil() as u32 + 1).clamp(2, MAX_RESOLUTION))
}

/// Something lying on the ground that grass doesn't grow under: a circle, or a segment
/// (a log) with a radius. Positions in meters from the region's center along world x/z.
#[derive(Debug, Clone, Copy)]
pub struct Cover {
    pub a: Vec2,
    pub b: Vec2,
    pub radius: f32,
}

/// Density (0..1) and dryness (0..1) maps for a region from its grass plants, soil and what
/// covers the ground. `plants` are (position from the region's center, cover radius,
/// vigor 0..1, health 0..1) of living grass.
#[allow(clippy::too_many_arguments)]
pub fn grass_maps(
    region: &EcologyRegion,
    state: &RegionState,
    resolution: [u32; 2],
    plants: &[(Vec2, f32, f32, f32)],
    covers: &[Cover],
) -> (Vec<f32>, Vec<f32>) {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let n = (rx * rz) as usize;
    let mut cover = vec![0.0f32; n];
    let mut sick = vec![0.0f32; n];
    let mut weight = vec![0.0f32; n];
    let index = |p: Vec2| -> (i64, i64) {
        (
            ((p.x - corner.x) / step.x).round() as i64,
            ((p.y - corner.y) / step.y).round() as i64,
        )
    };
    for &(p, r, vigor, health) in plants {
        let reach = (r * 1.4).max(step.x.max(step.y));
        let (ci, cj) = index(p);
        let (si, sj) = (
            (reach / step.x).ceil() as i64,
            (reach / step.y).ceil() as i64,
        );
        for j in (cj - sj).max(0)..=(cj + sj).min(rz as i64 - 1) {
            for i in (ci - si).max(0)..=(ci + si).min(rx as i64 - 1) {
                let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
                let d = q.distance(p) / reach;
                if d >= 1.0 {
                    continue;
                }
                let f = (1.0 - d * d) * vigor;
                let k = (j as u32 * rx + i as u32) as usize;
                cover[k] += f;
                sick[k] += f * (1.0 - health);
                weight[k] += f;
            }
        }
    }
    let center = state.center;
    let mut density = Vec::with_capacity(n);
    let mut dryness = Vec::with_capacity(n);
    for j in 0..rz {
        for i in 0..rx {
            let k = (j * rx + i) as usize;
            let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
            let world = center + DVec3::new(q.x as f64, 0.0, q.y as f64);
            let soil = &state.sim.soil;
            let blocked = !soil.contains(world) || soil.blocked[soil.index(world)];
            let clear = region
                .keep_clear
                .iter()
                .any(|c| q.distance(Vec2::new(c[0], c[1])) < c[2]);
            let covered = covers.iter().any(|c| {
                let ab = c.b - c.a;
                let t = ((q - c.a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                q.distance(c.a + ab * t) < c.radius
            });
            // Meadows are patchy even where the tufts are thick: dense swards, thin rises and
            // bare gaps a few meters across, so the ground shows through here and there.
            let (x, z) = (q.x as f64, q.y as f64);
            let patch = fbm2(x * 0.15, z * 0.15, region.seed ^ 0x6a55, 3, 2.0, 0.5);
            let gaps = fbm2(x * 0.45, z * 0.45, region.seed ^ 0x9a9, 2, 2.0, 0.5);
            let d = if blocked || clear || covered {
                0.0
            } else {
                // Tufts overlap: 1.2 of cover is a closed sward.
                let sward = (cover[k] / 1.2).min(1.0).powf(0.8);
                let d = sward
                    * (0.3 + 0.75 * smoothstep(-0.6, 0.35, patch))
                    * (1.0 - 0.6 * smoothstep(0.15, 0.45, gaps));
                if d < 0.06 { 0.0 } else { d.min(1.0) }
            };
            density.push(d);
            let water = if soil.contains(world) {
                soil.sample(world)[0]
            } else {
                0.5
            };
            let thirst = ((0.45 - water) / 0.35).clamp(0.0, 1.0);
            let ill = if weight[k] > 0.0 {
                sick[k] / weight[k]
            } else {
                0.0
            };
            // Drier, paler grass on the thin patches.
            let parched = 0.12 * (1.0 - smoothstep(-0.6, 0.35, patch));
            dryness.push((0.1 + parched + 0.6 * thirst + 0.8 * ill).min(1.0));
        }
    }
    (density, dryness)
}

/// Spawn, update and remove each region's grass field.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_region_grass(
    mut commands: Commands,
    mut timer: Local<f32>,
    time: Res<Time>,
    settings: Res<EcologySettings>,
    season: Res<FoliageSeason>,
    library: Res<PlantLibrary>,
    ground: Option<Res<Ground>>,
    mut terrain: MessageReader<engine_world::TerrainChanged>,
    regions: Query<(Entity, &EcologyRegion, &RegionState, Option<&RegionGrass>)>,
    mut fields: Query<(&mut GrassField, &mut Transform, &GrassOf)>,
    plants: Query<(&Plant, &Transform, &InRegion), Without<GrassField>>,
    logs: Query<(&FelledLog, &Transform), (With<LogPlaced>, Without<GrassField>)>,
    rocks: Query<&Transform, (With<Rock>, Without<GrassField>)>,
    orphans: Query<(Entity, &GrassOf)>,
) {
    let relief = terrain.read().count() > 0;
    *timer -= time.real_delta_secs();
    if *timer > 0.0 && !relief && !settings.is_changed() && !season.is_changed() {
        return;
    }
    *timer = REBUILD_SECS;
    // Fields whose region is gone.
    for (e, of) in &orphans {
        if !regions.contains(of.0) {
            commands.entity(e).despawn();
        }
    }
    for (region_e, region, state, linked) in &regions {
        let style = region_style(&library, region).filter(|_| settings.gpu_grass);
        let Some(style) = style else {
            if let Some(g) = linked {
                commands.entity(g.0).despawn();
                commands.entity(region_e).remove::<RegionGrass>();
            }
            continue;
        };
        let res = resolution(region.half_extents);
        let center = state.center;
        let local = |t: &Transform| {
            Vec2::new(
                (t.translation.x - center.x) as f32,
                (t.translation.z - center.z) as f32,
            )
        };
        let inside = |p: Vec2| {
            p.x.abs() <= region.half_extents[0] + 1.0 && p.y.abs() <= region.half_extents[1] + 1.0
        };
        let grass: Vec<_> = plants
            .iter()
            .filter(|(p, _, r)| r.0 == Some(region_e) && p.stage.is_alive())
            .filter(|(p, _, _)| library.get(p.species).form == PlantForm::Grass)
            .map(|(p, t, _)| {
                let sp = library.get(p.species);
                (
                    local(t),
                    p.crown_radius(sp),
                    (0.35 + 0.65 * p.size).min(1.0),
                    p.health,
                )
            })
            .collect();
        // A log's transform sits at its butt (the model's base); the trunk runs up its axis.
        let mut covers: Vec<Cover> = logs
            .iter()
            .filter_map(|(log, t)| {
                let sp = library.get(log.species);
                let a = local(t);
                // Bare old logs have lost their thin tops (`meshgen::tree`).
                let reach = if log.years >= LOG_BARE_YEARS {
                    0.62
                } else {
                    0.85
                };
                let axis = t.rotation * Vec3::Y * (sp.max_height_m * t.scale.y * reach);
                let r = (sp.trunk_radius_m * log.size * 1.6).max(0.15);
                inside(a).then_some(Cover {
                    a,
                    b: a + Vec2::new(axis.x, axis.z),
                    radius: r,
                })
            })
            .collect();
        covers.extend(rocks.iter().filter_map(|t| {
            let p = local(t);
            let r = 0.45 * t.scale.x.max(t.scale.z);
            (inside(p) && r > 0.08).then_some(Cover {
                a: p,
                b: p,
                radius: r,
            })
        }));
        let (density, mut dryness) = grass_maps(region, state, res, &grass, &covers);
        // Autumn browns the meadow and winter leaves it straw, wherever it grows.
        let season_dry = grass_dryness(season.day_of_year);
        for d in &mut dryness {
            *d += (1.0 - *d) * season_dry;
        }
        let field_at = Transform::from_translation(center);
        match linked.and_then(|g| fields.get_mut(g.0).ok()) {
            Some((mut field, mut t, _)) => {
                if relief || field.heights.is_empty() || field.resolution != res {
                    field.heights = heights(ground.as_deref(), center, region, res);
                }
                field.size = [region.half_extents[0] * 2.0, region.half_extents[1] * 2.0];
                field.resolution = res;
                field.density = density;
                field.dryness = dryness;
                field.style = style;
                if t.translation != field_at.translation {
                    *t = field_at;
                }
            }
            None => {
                let field = GrassField {
                    size: [region.half_extents[0] * 2.0, region.half_extents[1] * 2.0],
                    resolution: res,
                    heights: heights(ground.as_deref(), center, region, res),
                    density,
                    dryness,
                    style,
                };
                let e = commands.spawn((field_at, field, GrassOf(region_e))).id();
                commands.entity(region_e).insert(RegionGrass(e));
            }
        }
    }
}

/// Ground heights over a region's grid, meters above its center.
fn heights(
    ground: Option<&Ground>,
    center: DVec3,
    region: &EcologyRegion,
    res: [u32; 2],
) -> Vec<f32> {
    let [rx, rz] = res;
    let size = [region.half_extents[0] * 2.0, region.half_extents[1] * 2.0];
    let mut out = Vec::with_capacity((rx * rz) as usize);
    for j in 0..rz {
        for i in 0..rx {
            let x = (i as f32 / (rx - 1) as f32 - 0.5) * size[0];
            let z = (j as f32 / (rz - 1) as f32 - 0.5) * size[1];
            let p = center + DVec3::new(x as f64, 2.0, z as f64);
            let h = ground
                .and_then(|g| g.height(p))
                .map_or(0.0, |h| (h - center.y) as f32);
            out.push(h);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plant::{Genes, RegionHistory};
    use engine_core::time::TimeSource;

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app
    }

    #[test]
    fn grass_grows_where_the_ecology_puts_it() {
        let mut app = app();
        let region = app
            .world
            .spawn((
                Transform::from_xyz(50.0, 0.0, 0.0),
                EcologyRegion {
                    half_extents: [20.0, 20.0],
                    species: vec!["grass".into(), "oak".into()],
                    keep_clear: vec![[10.0, 0.0, 3.0]],
                    history: RegionHistory {
                        grow_years: 0.0,
                        initial_plants: [0, 0, 0],
                        ..Default::default()
                    },
                    seed_rain: 0.0,
                    ..Default::default()
                },
            ))
            .id();
        let lib = PlantLibrary::default();
        let grass = lib.id("grass").unwrap();
        // A lawn on the -x half, none on the +x half.
        for i in 0..20 {
            for j in 0..40 {
                app.world.spawn((
                    Transform::from_xyz(50.0 - 19.0 + i as f64 * 0.9, 0.0, -19.0 + j as f64 * 0.95),
                    Plant::new(grass, 1.0, Genes::default()),
                ));
            }
        }
        // The maps are rebuilt every few seconds; give the plants time to join the region.
        app.update_n(200);
        let field_e = app
            .world
            .get::<RegionGrass>(region)
            .expect("a grass field")
            .0;
        let field = app.world.get::<GrassField>(field_e).unwrap();
        let [rx, rz] = field.resolution;
        assert_eq!(field.density.len(), (rx * rz) as usize);
        let at = |x: f32, z: f32| {
            let i = ((x / field.size[0] + 0.5) * (rx - 1) as f32).round() as u32;
            let j = ((z / field.size[1] + 0.5) * (rz - 1) as f32).round() as u32;
            field.density[(j * rx + i) as usize]
        };
        // The lawn is thick on the whole but patchy, with the odd gap.
        let lawn: Vec<f32> = (0..15)
            .flat_map(|i| (0..30).map(move |j| (-17.0 + i as f32, -15.0 + j as f32)))
            .map(|(x, z)| at(x, z))
            .collect();
        let mean = lawn.iter().sum::<f32>() / lawn.len() as f32;
        assert!(mean > 0.55, "lawn {mean}");
        assert!(lawn.iter().any(|&d| d > 0.9), "no dense sward");
        assert!(lawn.iter().any(|&d| d < 0.5), "no thin patch");
        assert!(at(14.0, 5.0) < 0.05, "bare {}", at(14.0, 5.0));
        assert_eq!(at(10.0, 0.0), 0.0, "keep-clear");

        // A log lying in the lawn along +z: no grass along its length, grass behind its butt.
        let oak = lib.id("oak").unwrap();
        app.world.spawn((
            Transform::from_xyz(50.0 - 10.0, 0.0, -10.0),
            FelledLog {
                species: oak,
                size: 0.8,
                ..Default::default()
            },
            crate::felling::LogPose {
                yaw: std::f32::consts::FRAC_PI_2,
                ..Default::default()
            },
        ));
        app.update_n(200);
        // It rests a little to the side of where it fell, its butt off the stump.
        let butt = app
            .world
            .query_filtered::<&Transform, With<FelledLog>>()
            .single(&app.world)
            .unwrap()
            .translation;
        let (x, z) = ((butt.x - 50.0) as f32, butt.z as f32);
        let field = app.world.get::<GrassField>(field_e).unwrap();
        let at = |x: f32, z: f32| {
            let i = ((x / field.size[0] + 0.5) * (rx - 1) as f32).round() as u32;
            let j = ((z / field.size[1] + 0.5) * (rz - 1) as f32).round() as u32;
            field.density[(j * rx + i) as usize]
        };
        let length = lib.get(oak).max_height_m * 0.8;
        for s in [0.1, 0.4, 0.7] {
            assert_eq!(at(x, z + length * s), 0.0, "under the log at {s}");
        }
        assert!(
            (1..4).any(|k| at(x, z - k as f32) > 0.3),
            "no grass behind the butt"
        );

        // Turned off: the field goes, and the tufts draw again.
        app.world.resource_mut::<EcologySettings>().gpu_grass = false;
        app.update_n(2);
        assert!(app.world.get::<RegionGrass>(region).is_none());
        assert!(app.world.get_entity(field_e).is_err());
    }

    #[test]
    fn caves_without_grass_get_no_field() {
        let lib = PlantLibrary::default();
        assert!(region_style(&lib, &crate::demo::cave_region()).is_none());
        assert!(region_style(&lib, &crate::demo::forest_region()).is_some());
    }
}
