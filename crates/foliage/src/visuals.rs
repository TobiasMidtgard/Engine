//! Drawing plants: each plant entity gets three child parts (stump/base, wood, foliage) whose
//! meshes and materials follow its life stage, size and distance to the camera.
//!
//! - Saplings use a slender young mesh; grown trees the full mesh; far trees a cheaper one.
//! - Small plants (grass, flowers, ferns, mushrooms) disappear beyond a distance.
//! - Stumps show only the base, dead trees stand as grey snags, burnt ones as charcoal.
//! - Glowing species pick a brighter material the healthier and better fed they are, breathe
//!   slowly, and light their surroundings through a pool of point lights (one per cluster).

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Camera, Mesh, MeshRenderer, PointLight, StandardMaterial};

use crate::assets::{FoliageVisuals, GLOW_LEVELS, SpeciesVisual, TINTS, VARIANTS};
use crate::ecology::InRegion;
use crate::felling::{FallingTree, LOG_BARE_YEARS, LOG_GREEN_YEARS, LogPose};
use crate::grass::RegionGrass;
use crate::meshgen::tree::LEAF_STEPS;
use crate::plant::{EcologySettings, FelledLog, Plant, PlantStage};
use crate::season::{FoliageSeason, Leafing, shift_days};
use crate::species::{Layer, PlantForm, PlantLibrary, PlantSpecies};

/// The drawn parts of a plant (child entities) and what they currently show.
#[derive(Component, Debug, Clone)]
pub struct PlantVisual {
    pub parts: [Entity; 3],
    shown: [Option<(Handle<Mesh>, Handle<StandardMaterial>)>; 3],
}

/// Marks the child entities that draw a plant.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlantPart;

type Part = Option<(Handle<Mesh>, Handle<StandardMaterial>)>;

fn tint_of(plant: &Plant) -> usize {
    (((plant.genes.hue + 1.0) * 0.5 * TINTS as f32) as usize).min(TINTS - 1)
}

/// Brightness level of a glowing plant: health and how well fed it is.
pub fn glow_level(plant: &Plant) -> usize {
    let b = plant.health * 0.55 + plant.vigor * 0.45;
    ((b * GLOW_LEVELS as f32) as usize).min(GLOW_LEVELS - 1)
}

fn is_small(sp: &PlantSpecies) -> bool {
    matches!(
        sp.form,
        PlantForm::Grass | PlantForm::Flower | PlantForm::Fern | PlantForm::Mushroom
    )
}

/// The crown to draw for a share of leaves (`amount`, 0..1): the full crown, the nearest of
/// its thinned copies (`thin`, one per [`LEAF_STEPS`] step), or none once it is all but bare.
fn leafy(full: Option<Handle<Mesh>>, thin: &[Handle<Mesh>], amount: f32) -> Option<Handle<Mesh>> {
    let (mut shown, mut share) = (full, 1.0);
    for (&step, mesh) in LEAF_STEPS.iter().zip(thin) {
        if amount >= 0.5 * (share + step) {
            return shown;
        }
        (shown, share) = (Some(*mesh), step);
    }
    if amount >= 0.5 * share { shown } else { None }
}

/// What each part should draw on `day` of the year ([`FoliageSeason`]).
pub fn desired_parts(
    plant: &Plant,
    sp: &PlantSpecies,
    vis: &SpeciesVisual,
    distance: f32,
    settings: &EcologySettings,
    day: f32,
) -> [Part; 3] {
    let v = &vis.variants[plant.genes.variant as usize % VARIANTS];
    let tint = tint_of(plant);
    let pair = |m: &Option<Handle<Mesh>>, mat: Handle<StandardMaterial>| m.map(|m| (m, mat));
    let height = plant.height(sp).max(0.1);
    let leafing = sp.seasons.map_or(Leafing::SUMMER, |s| {
        s.leafing(day, shift_days(&plant.genes))
    });
    if is_small(sp)
        && (distance > settings.small_plant_distance_m * (0.6 + 0.4 * (height / 1.0).min(2.5))
            || !leafing.shows(&plant.genes))
    {
        return [None, None, None];
    }
    match plant.stage {
        PlantStage::Alive => {
            let sapling = v.sapling_wood.is_some() && plant.size < 0.22;
            let lod = if is_small(sp) {
                distance > settings.small_plant_lod_distance_m * (0.6 + 0.4 * height.min(2.5))
            } else {
                distance > settings.tree_lod_distance_m * (0.5 + plant.size)
            };
            let leaves = if vis.glow.is_empty() {
                vis.leaves[tint]
            } else {
                vis.glow[tint % vis.glow.len()][glow_level(plant)]
            };
            let amount = if is_small(sp) { 1.0 } else { leafing.amount };
            if sapling {
                let crown = leafy(v.sapling_foliage, &v.sapling_foliage_thin, amount);
                [None, pair(&v.sapling_wood, vis.bark), pair(&crown, leaves)]
            } else if lod && (v.wood_lod.is_some() || v.foliage_lod.is_some()) {
                let or = |a: &Option<Handle<Mesh>>, b: &Option<Handle<Mesh>>| a.or(*b);
                let crown = match v.foliage_lod {
                    Some(_) => leafy(v.foliage_lod, &v.foliage_lod_thin, amount),
                    None => leafy(v.foliage, &v.foliage_thin, amount),
                };
                [
                    pair(&v.base, vis.bark_rigid),
                    pair(&or(&v.wood_lod, &v.wood), vis.bark),
                    pair(&crown, leaves),
                ]
            } else {
                [
                    pair(&v.base, vis.bark_rigid),
                    pair(&v.wood, vis.bark),
                    pair(&leafy(v.foliage, &v.foliage_thin, amount), leaves),
                ]
            }
        }
        PlantStage::Stump { .. } => [pair(&v.base, vis.bark_rigid), None, None],
        PlantStage::Dead { years } => {
            let wood = if plant.size < 0.22 {
                &v.sapling_wood
            } else if years > 1.5 && v.snag.is_some() {
                // Twigs drop off a dead tree within a couple of years.
                &v.snag
            } else {
                &v.wood
            };
            if sp.is_woody() {
                [
                    pair(&v.base, vis.dead_bark),
                    pair(wood, vis.dead_bark),
                    if years < 1.0 {
                        pair(&v.foliage, vis.dead_leaves)
                    } else {
                        None
                    },
                ]
            } else {
                [None, None, pair(&v.foliage, vis.dead_leaves)]
            }
        }
        PlantStage::Burnt { .. } => {
            if sp.is_woody() {
                let wood = if plant.size < 0.22 {
                    &v.sapling_wood
                } else if v.snag.is_some() {
                    &v.snag
                } else {
                    &v.wood
                };
                [pair(&v.base, vis.burnt), pair(wood, vis.burnt), None]
            } else {
                [None, None, None]
            }
        }
    }
}

/// Turns a plant's pose ([`plant_pose`]) sits on: small plants lean with the slope they grow
/// on (trees grow straight up), and a world shift that turns the world turns this too.
/// Missing means none.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PlantBase(pub Quat);

/// How far small plants lean toward the slope's normal (1 = square to the ground).
const SLOPE_FOLLOW: f32 = 0.7;

/// The lean of a small plant on ground with normal `normal`.
pub fn slope_tilt(normal: Vec3) -> Quat {
    let n = Vec3::Y.lerp(normal.normalize_or(Vec3::Y), SLOPE_FOLLOW);
    Quat::from_rotation_arc(Vec3::Y, n.normalize_or(Vec3::Y))
}

/// Rotation and scale of a plant from its genes and size.
pub fn plant_pose(plant: &Plant) -> (Quat, Vec3) {
    let g = &plant.genes;
    let yaw = (g.lean_dir * 7.31 + g.variant as f32 * 0.77).rem_euclid(TAU);
    let axis = Vec3::new(g.lean_dir.cos(), 0.0, g.lean_dir.sin());
    let rotation = Quat::from_axis_angle(axis, g.lean) * Quat::from_rotation_y(yaw);
    let s = plant.size.max(0.02);
    (rotation, Vec3::new(s * g.crown, s * g.height, s * g.crown))
}

/// The active camera's world position.
pub fn camera_position(world_cams: &Query<(&Camera, &GlobalTransform)>) -> Option<DVec3> {
    world_cams
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
}

/// Keep every plant's parts in sync with its stage, size, distance and the season.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_plant_visuals(
    mut commands: Commands,
    mut frame: Local<u32>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    settings: Res<EcologySettings>,
    season: Res<FoliageSeason>,
    ground: Option<Res<engine_world::Ground>>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut plants: Query<(
        Entity,
        Ref<Plant>,
        &mut Transform,
        Option<&mut PlantVisual>,
        Option<&PlantBase>,
        Option<&InRegion>,
    )>,
    mut parts: Query<&mut MeshRenderer, With<PlantPart>>,
    meadows: Query<(), With<RegionGrass>>,
) {
    *frame = frame.wrapping_add(1);
    let full_pass =
        *frame % 8 == 1 || visuals.is_changed() || settings.is_changed() || season.is_changed();
    let cam = camera_position(&cams);
    for (entity, plant, mut transform, visual, base, region) in &mut plants {
        let fresh = visual.is_none();
        if !full_pass && !fresh && !plant.is_changed() {
            continue;
        }
        let Some(vis) = visuals.get(plant.species) else {
            continue;
        };
        let sp = library.get(plant.species);
        if plant.is_changed() || fresh {
            let base = match base {
                Some(b) => b.0,
                // Small plants lean with the slope, worked out once.
                None if is_small(sp) => {
                    let normal = ground
                        .as_ref()
                        .and_then(|g| g.sample(transform.translation + DVec3::Y))
                        .map_or(Vec3::Y, |g| g.normal);
                    let tilt = slope_tilt(normal);
                    commands.entity(entity).insert(PlantBase(tilt));
                    tilt
                }
                None => Quat::IDENTITY,
            };
            let (rotation, scale) = plant_pose(&plant);
            let rotation = base * rotation;
            if transform.rotation != rotation || transform.scale != scale {
                transform.rotation = rotation;
                transform.scale = scale;
            }
        }
        let distance = cam
            .map(|c| c.distance(transform.translation) as f32)
            .unwrap_or(0.0);
        // Grass under a region's GPU meadow is drawn as blades there.
        let meadow = sp.form == PlantForm::Grass
            && region
                .and_then(|r| r.0)
                .is_some_and(|r| meadows.contains(r));
        let want = if meadow {
            [None, None, None]
        } else {
            desired_parts(&plant, sp, vis, distance, &settings, season.day_of_year)
        };
        let shadows = sp.layer != Layer::Ground && sp.form != PlantForm::Mushroom;
        match visual {
            Some(mut visual) => {
                if visual.shown == want {
                    continue;
                }
                for (i, part) in want.iter().enumerate() {
                    if visual.shown[i] == *part {
                        continue;
                    }
                    if let Ok(mut r) = parts.get_mut(visual.parts[i]) {
                        apply(&mut r, *part);
                    }
                }
                visual.shown = want;
            }
            None => {
                let parts = std::array::from_fn(|i| {
                    let mut r = MeshRenderer::new(
                        engine_render::Shape::Cuboid { size: [0.01; 3] },
                        StandardMaterial::default(),
                    );
                    r.cast_shadows = shadows;
                    apply(&mut r, want[i]);
                    commands
                        .spawn((Transform::default(), ChildOf(entity), r, PlantPart))
                        .id()
                });
                commands
                    .entity(entity)
                    .insert(PlantVisual { parts, shown: want });
            }
        }
    }
}

fn apply(r: &mut MeshRenderer, part: Part) {
    match part {
        Some((mesh, material)) => {
            r.mesh = engine_render::MeshRef::Handle(mesh);
            r.material = engine_render::MaterialRef::Handle(material);
            r.visible = true;
        }
        None => r.visible = false,
    }
}

/// The drawn parts of a falling tree or a log (wood, foliage) and what they show.
#[derive(Component, Debug, Clone)]
pub struct LogVisual {
    pub parts: [Entity; 2],
    shown: [Part; 2],
}

/// What a falling tree or log draws: fresh logs keep their green crown for a few weeks, then
/// brown leaves for a year, then bare wood that greys as it rots. A deciduous tree that came
/// down out of leaf (`day` of the year) lies bare.
#[allow(clippy::too_many_arguments)]
pub fn desired_log_parts(
    vis: &SpeciesVisual,
    pose: &LogPose,
    size: f32,
    years: f32,
    rot: f32,
    distance: f32,
    settings: &EcologySettings,
    day: f32,
) -> [Part; 2] {
    let v = &vis.variants[pose.genes.variant as usize % VARIANTS];
    let tint = (((pose.genes.hue + 1.0) * 0.5 * TINTS as f32) as usize).min(TINTS - 1);
    let lod = distance > settings.tree_lod_distance_m * (0.5 + size) && v.wood_lod.is_some();
    let amount = vis
        .built_from
        .seasons
        .map_or(1.0, |s| s.leafing(day, shift_days(&pose.genes)).amount);
    let (wood, foliage) = if size < 0.22 && v.sapling_wood.is_some() {
        (
            v.sapling_wood,
            leafy(v.sapling_foliage, &v.sapling_foliage_thin, amount),
        )
    } else if (years >= LOG_BARE_YEARS || pose.charred) && v.log.is_some() {
        // The twigs have rotted (or burnt) off.
        (v.log, None)
    } else if lod {
        (
            v.wood_lod,
            leafy(v.foliage_lod, &v.foliage_lod_thin, amount),
        )
    } else {
        (v.wood, leafy(v.foliage, &v.foliage_thin, amount))
    };
    let bark = if pose.charred {
        vis.burnt
    } else if rot > 0.45 {
        vis.dead_bark
    } else {
        vis.bark_rigid
    };
    let leaves = if pose.charred || years >= LOG_BARE_YEARS {
        None
    } else if years < LOG_GREEN_YEARS {
        Some(vis.leaves_still[tint])
    } else {
        Some(vis.dead_leaves_still)
    };
    [wood.map(|m| (m, bark)), foliage.zip(leaves)]
}

/// Keep the parts of falling trees and logs in sync with their age, distance and the season.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_log_visuals(
    mut commands: Commands,
    mut frame: Local<u32>,
    visuals: Res<FoliageVisuals>,
    settings: Res<EcologySettings>,
    season: Res<FoliageSeason>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut logs: Query<
        (
            Entity,
            &Transform,
            &LogPose,
            Option<&FelledLog>,
            Option<&FallingTree>,
            Option<&mut LogVisual>,
        ),
        Or<(With<FelledLog>, With<FallingTree>)>,
    >,
    mut parts: Query<&mut MeshRenderer, With<PlantPart>>,
) {
    *frame = frame.wrapping_add(1);
    let full_pass = *frame % 16 == 3 || visuals.is_changed() || season.is_changed();
    let cam = camera_position(&cams);
    for (entity, transform, pose, log, falling, visual) in &mut logs {
        if !full_pass && visual.is_some() {
            continue;
        }
        let (species, size, years, rot) = match (log, falling) {
            (_, Some(f)) => (f.species, f.size, f.years_dead, 0.0),
            (Some(l), None) => (l.species, l.size, l.years, l.years / l.rot_years.max(0.1)),
            _ => continue,
        };
        let Some(vis) = visuals.get(species) else {
            continue;
        };
        let distance = cam
            .map(|c| c.distance(transform.translation) as f32)
            .unwrap_or(0.0);
        let day = season.day_of_year;
        let want = desired_log_parts(vis, pose, size, years, rot, distance, &settings, day);
        match visual {
            Some(mut visual) => {
                for (i, part) in want.iter().enumerate() {
                    if visual.shown[i] != *part
                        && let Ok(mut r) = parts.get_mut(visual.parts[i])
                    {
                        apply(&mut r, *part);
                    }
                }
                visual.shown = want;
            }
            None => {
                let parts = std::array::from_fn(|i| {
                    let mut r = MeshRenderer::new(
                        engine_render::Shape::Cuboid { size: [0.01; 3] },
                        StandardMaterial::default(),
                    );
                    apply(&mut r, want[i]);
                    commands
                        .spawn((Transform::default(), ChildOf(entity), r, PlantPart))
                        .id()
                });
                commands
                    .entity(entity)
                    .insert(LogVisual { parts, shown: want });
            }
        }
    }
}

/// A point light cast by a cluster of glowing plants.
#[derive(Component, Debug, Clone, Copy)]
pub struct GlowLight {
    /// Lumens before the slow pulse.
    pub base_lm: f32,
    pub pulse_secs: f32,
    pub phase: f32,
}

/// Pool of glow lights.
#[derive(Resource, Default)]
pub struct GlowLights {
    pub lights: Vec<Entity>,
    timer: f32,
    last_camera: Option<DVec3>,
}

impl GlowLights {
    /// Follow a world re-anchoring ([`crate::shift`]): the lights themselves are root entities
    /// and move with the world; where they were gathered moves here.
    pub fn follow(&mut self, s: &engine_world::WorldShift) {
        self.last_camera = self.last_camera.map(|c| s.point(c));
    }
}

/// Most glow lights at once.
pub const MAX_GLOW_LIGHTS: usize = 40;

/// Gather glowing plants near the camera into clusters and light each with one point light.
#[allow(clippy::type_complexity)]
pub fn update_glow_lights(
    mut commands: Commands,
    time: Res<Time>,
    library: Res<PlantLibrary>,
    mut pool: ResMut<GlowLights>,
    cams: Query<(&Camera, &GlobalTransform)>,
    plants: Query<(&Plant, &GlobalTransform)>,
    mut lights: Query<(&mut PointLight, &mut Transform, &mut GlowLight)>,
) {
    let Some(cam) = camera_position(&cams) else {
        return;
    };
    pool.timer -= time.real_delta_secs();
    let moved = pool.last_camera.is_none_or(|c| c.distance(cam) > 15.0);
    if pool.timer > 0.0 && !moved {
        return;
    }
    pool.timer = 0.5;
    pool.last_camera = Some(cam);
    const CELL: f64 = 3.5;
    // cell → (lumens, weighted position, weighted color, pulse)
    let mut cells: HashMap<(i64, i64, i64), (f32, DVec3, Vec3, f32)> = HashMap::new();
    for (plant, gt) in &plants {
        let sp = library.get(plant.species);
        let Some(glow) = sp.glow else { continue };
        if !plant.stage.is_alive() {
            continue;
        }
        let p = gt.translation();
        if p.distance(cam) > 90.0 {
            continue;
        }
        let level = (glow_level(plant) as f32 + 0.5) / GLOW_LEVELS as f32;
        let lm = glow.lumens * plant.size * plant.size.sqrt() * level;
        if lm <= 0.01 {
            continue;
        }
        let key = (
            (p.x / CELL).floor() as i64,
            (p.y / CELL).floor() as i64,
            (p.z / CELL).floor() as i64,
        );
        let top = p + DVec3::Y * (plant.height(sp) as f64 * 0.8 + 0.15);
        let e = cells
            .entry(key)
            .or_insert((0.0, DVec3::ZERO, Vec3::ZERO, glow.pulse_secs));
        e.0 += lm;
        e.1 += top * lm as f64;
        e.2 += Vec3::from(glow.color) * lm;
    }
    let mut clusters: Vec<_> = cells.into_values().filter(|c| c.0 > 0.5).collect();
    clusters.sort_by(|a, b| b.0.total_cmp(&a.0));
    clusters.truncate(MAX_GLOW_LIGHTS);
    while pool.lights.len() < clusters.len() {
        let e = commands
            .spawn((
                Transform::default(),
                PointLight {
                    intensity_lm: 0.0,
                    range: 8.0,
                    radius: 0.3,
                    color: [1.0; 3],
                },
                GlowLight {
                    base_lm: 0.0,
                    pulse_secs: 7.0,
                    phase: 0.0,
                },
            ))
            .id();
        pool.lights.push(e);
    }
    for (i, &e) in pool.lights.iter().enumerate() {
        let Ok((mut light, mut t, mut g)) = lights.get_mut(e) else {
            continue;
        };
        match clusters.get(i) {
            Some(&(lm, wp, wc, pulse)) => {
                t.translation = wp / lm as f64;
                let c = wc / lm;
                light.color = (c / c.max_element().max(1e-3)).to_array();
                g.base_lm = lm;
                g.pulse_secs = pulse;
                g.phase = (t.translation.x * 0.37 + t.translation.z * 0.61) as f32;
                light.range = (4.0 + lm.sqrt() * 1.2).min(16.0);
            }
            None => {
                g.base_lm = 0.0;
                light.intensity_lm = 0.0;
            }
        }
    }
}

/// Slow breathing of glowing materials and their lights.
pub fn pulse_glow(
    time: Res<Time>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut lights: Query<(&mut PointLight, &GlowLight)>,
) {
    let t = time.elapsed_secs();
    let breath = |pulse: f32, phase: f32| {
        if pulse <= 0.0 {
            1.0
        } else {
            0.84 + 0.16 * (t * std::f32::consts::TAU / pulse + phase).sin()
        }
    };
    for (i, sp) in library.species.iter().enumerate() {
        let Some(glow) = sp.glow else { continue };
        let Some(Some(vis)) = visuals.species.get(i) else {
            continue;
        };
        for (tint, levels) in vis.glow.iter().enumerate() {
            let k = breath(glow.pulse_secs, tint as f32 * 1.9);
            for (level, h) in levels.iter().enumerate() {
                if let Some(m) = materials.get_mut(*h) {
                    m.emissive_strength = glow.nits * (level as f32 + 0.5) / GLOW_LEVELS as f32 * k;
                }
            }
        }
    }
    for (mut light, g) in &mut lights {
        light.intensity_lm = g.base_lm * breath(g.pulse_secs, g.phase);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crowns_thin_step_by_step_as_leaves_fall() {
        let h = |i: u32| Handle::<Mesh>::from_index(i);
        let (full, thin) = (Some(h(0)), [h(1), h(2), h(3)]);
        assert_eq!(leafy(full, &thin, 1.0), full);
        assert_eq!(leafy(full, &thin, 0.85), full);
        assert_eq!(leafy(full, &thin, 0.55), Some(h(1)));
        assert_eq!(leafy(full, &thin, 0.25), Some(h(2)));
        assert_eq!(leafy(full, &thin, 0.08), Some(h(3)));
        assert_eq!(leafy(full, &thin, 0.02), None);
        // No thinned copies: full crown or nothing.
        assert_eq!(leafy(full, &[], 0.6), full);
        assert_eq!(leafy(full, &[], 0.3), None);
    }

    #[test]
    fn small_plants_lean_partly_with_the_slope() {
        assert_eq!(slope_tilt(Vec3::Y), Quat::IDENTITY);
        // A 30° slope facing +X: the plant leans toward +X, but less than the slope.
        let normal = Vec3::new(30f32.to_radians().sin(), 30f32.to_radians().cos(), 0.0);
        let up = slope_tilt(normal) * Vec3::Y;
        let lean = up.angle_between(Vec3::Y).to_degrees();
        assert!(up.x > 0.0 && (15.0..25.0).contains(&lean), "{up} {lean}");
    }
}
