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
use crate::felling::{FallingTree, LogPose};
use crate::plant::{EcologySettings, FelledLog, Plant, PlantStage};
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

/// What each part should draw.
pub fn desired_parts(
    plant: &Plant,
    sp: &PlantSpecies,
    vis: &SpeciesVisual,
    distance: f32,
    settings: &EcologySettings,
) -> [Part; 3] {
    let v = &vis.variants[plant.genes.variant as usize % VARIANTS];
    let tint = tint_of(plant);
    let pair = |m: &Option<Handle<Mesh>>, mat: Handle<StandardMaterial>| m.map(|m| (m, mat));
    let height = plant.height(sp).max(0.1);
    if is_small(sp)
        && distance > settings.small_plant_distance_m * (0.6 + 0.4 * (height / 1.0).min(2.5))
    {
        return [None, None, None];
    }
    match plant.stage {
        PlantStage::Alive => {
            let sapling = v.sapling_wood.is_some() && plant.size < 0.22;
            let lod = distance > settings.tree_lod_distance_m * (0.5 + plant.size);
            let leaves = if vis.glow.is_empty() {
                vis.leaves[tint]
            } else {
                vis.glow[tint % vis.glow.len()][glow_level(plant)]
            };
            if sapling {
                [
                    None,
                    pair(&v.sapling_wood, vis.bark),
                    pair(&v.sapling_foliage, leaves),
                ]
            } else if lod && v.wood_lod.is_some() {
                [
                    pair(&v.base, vis.bark_rigid),
                    pair(&v.wood_lod, vis.bark),
                    pair(&v.foliage_lod, leaves),
                ]
            } else {
                [
                    pair(&v.base, vis.bark_rigid),
                    pair(&v.wood, vis.bark),
                    pair(&v.foliage, leaves),
                ]
            }
        }
        PlantStage::Stump { .. } => [pair(&v.base, vis.bark_rigid), None, None],
        PlantStage::Dead { years } => {
            let wood = if plant.size < 0.22 {
                &v.sapling_wood
            } else if years > 1.5 && v.log.is_some() {
                // Twigs drop off a dead tree within a couple of years.
                &v.log
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
                } else if v.log.is_some() {
                    &v.log
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

/// Keep every plant's parts in sync with its stage, size and distance.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_plant_visuals(
    mut commands: Commands,
    mut frame: Local<u32>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    settings: Res<EcologySettings>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut plants: Query<(Entity, Ref<Plant>, &mut Transform, Option<&mut PlantVisual>)>,
    mut parts: Query<&mut MeshRenderer, With<PlantPart>>,
) {
    *frame = frame.wrapping_add(1);
    let full_pass = *frame % 8 == 1 || visuals.is_changed() || settings.is_changed();
    let cam = camera_position(&cams);
    for (entity, plant, mut transform, visual) in &mut plants {
        let fresh = visual.is_none();
        if !full_pass && !fresh && !plant.is_changed() {
            continue;
        }
        let Some(vis) = visuals.get(plant.species) else {
            continue;
        };
        let sp = library.get(plant.species);
        if plant.is_changed() || fresh {
            let (rotation, scale) = plant_pose(&plant);
            if transform.rotation != rotation || transform.scale != scale {
                transform.rotation = rotation;
                transform.scale = scale;
            }
        }
        let distance = cam
            .map(|c| c.distance(transform.translation) as f32)
            .unwrap_or(0.0);
        let want = desired_parts(&plant, sp, vis, distance, &settings);
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
/// brown leaves for a year, then bare wood that greys as it rots.
#[allow(clippy::too_many_arguments)]
pub fn desired_log_parts(
    vis: &SpeciesVisual,
    pose: &LogPose,
    size: f32,
    years: f32,
    rot: f32,
    distance: f32,
    settings: &EcologySettings,
) -> [Part; 2] {
    let v = &vis.variants[pose.genes.variant as usize % VARIANTS];
    let tint = (((pose.genes.hue + 1.0) * 0.5 * TINTS as f32) as usize).min(TINTS - 1);
    let lod = distance > settings.tree_lod_distance_m * (0.5 + size) && v.wood_lod.is_some();
    let (wood, foliage) = if size < 0.22 && v.sapling_wood.is_some() {
        (v.sapling_wood, v.sapling_foliage)
    } else if (years >= 1.2 || pose.charred) && v.log.is_some() {
        // The twigs have rotted (or burnt) off.
        (v.log, None)
    } else if lod {
        (v.wood_lod, v.foliage_lod)
    } else {
        (v.wood, v.foliage)
    };
    let bark = if pose.charred {
        vis.burnt
    } else if rot > 0.45 {
        vis.dead_bark
    } else {
        vis.bark_rigid
    };
    let leaves = if pose.charred || years >= 1.2 {
        None
    } else if years < 0.15 {
        Some(vis.leaves_still[tint])
    } else {
        Some(vis.dead_leaves_still)
    };
    [wood.map(|m| (m, bark)), foliage.zip(leaves)]
}

/// Keep the parts of falling trees and logs in sync with their age and distance.
#[allow(clippy::type_complexity)]
pub fn sync_log_visuals(
    mut commands: Commands,
    mut frame: Local<u32>,
    visuals: Res<FoliageVisuals>,
    settings: Res<EcologySettings>,
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
    let full_pass = *frame % 16 == 3 || visuals.is_changed();
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
        let want = desired_log_parts(vis, pose, size, years, rot, distance, &settings);
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
