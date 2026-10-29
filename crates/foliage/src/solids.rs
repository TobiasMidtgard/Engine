//! Trees are solid where their wood is thick. A tree's collision is a compound of capsules over
//! its trunk and its thickest limbs ([`crate::meshgen::tree::collision_capsules`], the way
//! SpeedTree's collision objects are made), posed like the drawn tree (its lean and size), so
//! a walker stops at the bark, a car hits the trunk it sees and a grapple or a low flight can
//! catch a limb. The leaves stay soft, as in most open-world games.
//!
//! Scenery trees (the hand-placed valley's and the near woods') get theirs as a child entity
//! while a focus point is within [`EcologySettings::tree_solid_distance_m`], and lose it again
//! past a margin: a valley holds tens of thousands of trees, the physics world only the few
//! hundred round the action. Ecology plants use the same shapes ([`crate::felling`]).
//! Research: engine-design/foliage-research.md, section 7.

use crate::assets::{FoliageVisuals, VARIANTS};
use crate::meshgen::tree::CollisionCapsule;
use crate::plant::{EcologySettings, Plant};
use crate::scenery_tree::SceneryTree;
use crate::species::PlantLibrary;
use crate::visuals::{camera_position, plant_pose};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::physics::{Collider, ColliderShape};
use engine_world::{FocusPoints, SurfaceMaterials};

/// Below this size a plant draws as a sapling (and collides like one).
const SAPLING_SIZE: f32 = 0.22;
/// Smaller plants have no collision at all.
const MIN_SOLID_SIZE: f32 = 0.15;
/// A tree's solid goes this far past the distance it came at, meters (no churn on the edge).
const SOLID_MARGIN_M: f64 = 20.0;

/// The child entity holding a scenery tree's collider.
#[derive(Component, Debug, Clone, Copy)]
pub struct TreeSolid;

/// A scenery tree's solid.
#[derive(Component, Debug, Clone, Copy)]
pub struct HasSolid(pub Entity);

/// A plant's collision shape, posed (lean and size) as its parts are drawn, in the plant's own
/// frame: `None` for plants too small or without wood to collide with, or before the species'
/// visuals are built.
pub fn plant_shape(
    plant: &Plant,
    visuals: &FoliageVisuals,
    library: &PlantLibrary,
) -> Option<ColliderShape> {
    if plant.size < MIN_SOLID_SIZE || !library.get(plant.species).is_tree() {
        return None;
    }
    let vis = visuals.get(plant.species)?;
    let v = &vis.variants[plant.genes.variant as usize % VARIANTS];
    let capsules = if plant.size < SAPLING_SIZE && v.sapling_wood.is_some() {
        &v.sapling_collision
    } else {
        &v.collision
    };
    let (rotation, scale) = plant_pose(plant);
    posed_shape(capsules, rotation, scale)
}

/// Capsules in model space, scaled and turned into a compound collider.
pub fn posed_shape(
    capsules: &[CollisionCapsule],
    rotation: Quat,
    scale: Vec3,
) -> Option<ColliderShape> {
    // Wood thickens with the crown's width (the mesh is scaled so), its length with height.
    let girth = (scale.x + scale.z) * 0.5;
    let parts: Vec<_> = capsules
        .iter()
        .map(|c| {
            let (a, b) = (rotation * (c.a * scale), rotation * (c.b * scale));
            let radius = (c.radius * girth).max(0.02);
            let axis = b - a;
            let len = axis.length();
            let turn = Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Y));
            (
                (a + b) * 0.5,
                turn,
                ColliderShape::Capsule {
                    radius,
                    half_height: len * 0.5,
                },
            )
        })
        .collect();
    (!parts.is_empty()).then_some(ColliderShape::Compound(parts))
}

/// A collider for a tree's wood.
pub fn wood_collider(shape: ColliderShape, surfaces: Option<&SurfaceMaterials>) -> Collider {
    Collider {
        shape,
        surface: surfaces.map_or_else(Default::default, |s| s.id_or_default("wood")),
        density: 700.0,
        ..Default::default()
    }
}

/// Give scenery trees near a focus point their solid, rebuild it when a tree changes (grown,
/// a new species), and take it from those far off, twice a second.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn stream_scenery_tree_solids(
    mut commands: Commands,
    mut timer: Local<f32>,
    time: Res<Time>,
    settings: Res<EcologySettings>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    surfaces: Option<Res<SurfaceMaterials>>,
    focus: Option<Res<FocusPoints>>,
    cams: Query<(&Camera, &GlobalTransform)>,
    trees: Query<(
        Entity,
        Ref<SceneryTree>,
        &GlobalTransform,
        Option<&HasSolid>,
    )>,
    mut solids: Query<&mut Collider, With<TreeSolid>>,
) {
    *timer -= time.real_delta_secs();
    let tick = *timer <= 0.0;
    if tick {
        *timer = 0.5;
    }
    let focus = focus.filter(|f| !f.points.is_empty());
    let camera = camera_position(&cams);
    let reach = settings.tree_solid_distance_m as f64;
    for (e, tree, global, has) in &trees {
        // Changed trees at once (a solid matching the tree it was), the rest on the tick.
        if !tick && !(tree.is_changed() && has.is_some()) {
            continue;
        }
        let p = global.translation();
        let d = match (&focus, camera) {
            (Some(f), _) => f.distance(p),
            (None, Some(c)) => c.distance(p),
            (None, None) => continue,
        };
        match has {
            Some(h) if d > reach + SOLID_MARGIN_M => {
                if let Ok(mut s) = commands.get_entity(h.0) {
                    s.despawn();
                }
                commands.entity(e).remove::<HasSolid>();
            }
            Some(h) if tree.is_changed() => {
                let shape = plant_shape(&tree.plant(), &visuals, &library);
                match (shape, solids.get_mut(h.0)) {
                    (Some(shape), Ok(mut c)) => {
                        if c.shape != shape {
                            c.shape = shape;
                        }
                    }
                    _ => {
                        if let Ok(mut s) = commands.get_entity(h.0) {
                            s.despawn();
                        }
                        commands.entity(e).remove::<HasSolid>();
                    }
                }
            }
            None if d < reach => {
                let Some(shape) = plant_shape(&tree.plant(), &visuals, &library) else {
                    continue;
                };
                let solid = commands
                    .spawn((
                        Transform::default(),
                        ChildOf(e),
                        wood_collider(shape, surfaces.as_deref()),
                        TreeSolid,
                    ))
                    .id();
                commands.entity(e).insert(HasSolid(solid));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FoliagePlugin;
    use engine_assets::Assets;
    use engine_render::Mesh;

    #[test]
    fn scenery_trees_near_a_focus_are_solid_up_their_trunk() {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<engine_render::StandardMaterial>>()
            .init_resource::<Assets<engine_render::Image>>()
            .add_plugins(FoliagePlugin);
        app.world.insert_resource(crate::FoliageLoading::in_frame());
        let all = PlantLibrary::default();
        let oak = all.get(all.id("oak").unwrap()).clone();
        app.world
            .insert_resource(PlantLibrary { species: vec![oak] });
        let library = app.world.resource::<PlantLibrary>().clone();
        let tree = SceneryTree::new(&library, "oak", 3).unwrap();
        let at = |x: f64| Transform::from_translation(DVec3::new(x, 0.0, 0.0));
        let near = app.world.spawn((at(10.0), tree)).id();
        let far = app.world.spawn((at(400.0), tree)).id();
        let focus = app
            .world
            .spawn((
                Transform::from_translation(DVec3::ZERO),
                engine_world::SimulationFocus::default(),
            ))
            .id();
        app.update_n(40);
        let solid = app.world.get::<HasSolid>(near).expect("near tree solid").0;
        assert!(app.world.get::<HasSolid>(far).is_none());
        let Some(ColliderShape::Compound(parts)) =
            app.world.get::<Collider>(solid).map(|c| c.shape.clone())
        else {
            panic!("no compound on the solid");
        };
        assert!(parts.len() >= 4, "{} capsules", parts.len());
        // Solid well above the showcase's old 3.5 m trunk stub.
        let top = parts
            .iter()
            .map(|(p, r, s)| match s {
                ColliderShape::Capsule { half_height, .. } => {
                    p.y + (*r * Vec3::Y).y.abs() * half_height
                }
                _ => p.y,
            })
            .fold(0.0f32, f32::max);
        assert!(top > 6.0, "solid up to {top} m");
        // The bole at chest height is inside a capsule (the tree stands at x = 10).
        let inside = |q: Vec3| {
            parts.iter().any(|(p, r, s)| match s {
                ColliderShape::Capsule {
                    radius,
                    half_height,
                } => {
                    let axis = *r * Vec3::Y * *half_height;
                    let (a, b) = (*p - axis, *p + axis);
                    let t =
                        ((q - a).dot(b - a) / (b - a).length_squared().max(1e-6)).clamp(0.0, 1.0);
                    q.distance(a + (b - a) * t) <= *radius
                }
                _ => false,
            })
        };
        assert!(inside(Vec3::new(0.0, 1.3, 0.0)), "walked through the trunk");
        assert!(
            !inside(Vec3::new(3.0, 1.3, 0.0)),
            "solid air beside the trunk"
        );
        // The focus walks off: the solid goes with it.
        app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::new(-500.0, 0.0, 0.0);
        app.update_n(40);
        assert!(app.world.get::<HasSolid>(near).is_none());
        assert!(app.world.get_entity(solid).is_err());
    }
}
