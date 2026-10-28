//! Colliders fitted to a model's meshes ([`FitCollider`], part of the physics contract), for
//! scenery and props built from art that has no collision shape of its own: buildings, piers,
//! boats. Like Unreal's collision presets (simple box, Auto Convex Collision, complex as
//! simple) and Unity's `MeshCollider` (exact, or `convex`), a [`Fit`] picks the shape:
//!
//! * [`Fit::Box`]: one box around the whole model. Cheapest.
//! * [`Fit::Hull`]: one convex hull (hollows and doorways filled in).
//! * [`Fit::Parts`]: convex pieces (V-HACD), so hollows, arches and hulls stay open. Works on
//!   moving bodies.
//! * [`Fit::Mesh`]: the exact triangles. Only for things that never move: a dynamic body gets
//!   [`Fit::Parts`] instead (Rapier, like PhysX and Unity, collides moving triangle meshes
//!   poorly).
//!
//! Put a [`FitCollider`] on a model's root and physics gives it a `Collider` once its meshes
//! have loaded (a glTF scene's appear frames after it is spawned). Meshes named for collision
//! (Unreal's `UBX_` / `USP_` / `UCP_` / `UCX_` prefixes, Godot's `-colonly` / `-convcolonly`
//! suffixes) win over the fit and are hidden. Or call [`fit_points`] on triangles you have.
//! The editor's model import fits colliders the same way (games/editor's `collision.rs`, which
//! this follows).

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::math::{DAffine3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRef, MeshRenderer};
use engine_world::physics::{BodyKind, Collider, ColliderShape, Fit, FitCollider, RigidBody};
use rapier3d::parry::math::Vector;
use rapier3d::parry::transformation::vhacd::{VHACD, VHACDParameters};

/// Most points one hull keeps (Unreal's Max Hull Verts goes up to 32).
pub const MAX_HULL_POINTS: usize = 32;
/// Most pieces [`Fit::Parts`] makes (Unreal's Auto Convex Collision defaults to 4 to 32).
pub const MAX_PARTS: u32 = 24;

/// What a mesh named for collision stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    Box,
    Sphere,
    Capsule,
    Convex,
    Mesh,
}

/// A node name's collision meaning ("UCX_Hull_00", "Pier-colonly"). A primitive's
/// "name/1" counts as its node's.
fn named(name: &str) -> Option<Named> {
    let name = name.split('/').next().unwrap_or(name);
    let upper = name.to_ascii_uppercase();
    for (prefix, kind) in [
        ("UBX_", Named::Box),
        ("USP_", Named::Sphere),
        ("UCP_", Named::Capsule),
        ("UCX_", Named::Convex),
    ] {
        if upper.starts_with(prefix) {
            return Some(kind);
        }
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("-convcolonly") || lower.ends_with("_convcolonly") {
        Some(Named::Convex)
    } else if lower.ends_with("-colonly") || lower.ends_with("_colonly") {
        Some(Named::Mesh)
    } else {
        None
    }
}

/// One mesh of a model, in the root's space.
struct Part {
    named: Option<Named>,
    entity: Entity,
    /// Which mesh and where (for [`Fitted`]).
    key: PartKey,
    points: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    /// The mesh's own bounds and its placement in the root's space (for a named box, sphere
    /// or capsule).
    local: (Vec3, Vec3),
    to_root: DAffine3,
}

/// The meshes under `root` in its space, or `None` while one of them is still loading.
fn parts(world: &World, root: Entity) -> Option<Vec<Part>> {
    let meshes = world.get_resource::<Assets<Mesh>>()?;
    let mut out = Vec::new();
    let mut stack: Vec<(Entity, DAffine3, Option<Named>)> = vec![(root, DAffine3::IDENTITY, None)];
    while let Some((e, to_root, parent_named)) = stack.pop() {
        let named = world
            .get::<Name>(e)
            .and_then(|n| named(n.as_str()))
            .or(parent_named);
        if let Some(children) = world.get::<Children>(e) {
            for child in children.iter() {
                let local = world
                    .get::<Transform>(child)
                    .map_or(DAffine3::IDENTITY, Transform::to_affine);
                stack.push((child, to_root * local, named));
            }
        }
        let Some(renderer) = world.get::<MeshRenderer>(e) else {
            continue;
        };
        let handle = match &renderer.mesh {
            MeshRef::Handle(h) => Some(*h),
            MeshRef::Asset(key) => meshes.loaded_by_key(&engine_assets::normalize_path(key)),
            MeshRef::Shape(_) => continue,
        };
        let handle = handle?;
        let mesh = meshes.get(handle)?;
        let aabb = mesh.aabb();
        out.push(Part {
            named,
            entity: e,
            key: (handle, to_root.to_cols_array().map(f64::to_bits)),
            points: mesh
                .vertices
                .iter()
                .map(|v| {
                    to_root
                        .transform_point3(Vec3::from(v.position).as_dvec3())
                        .as_vec3()
                })
                .collect(),
            triangles: mesh.indices.as_chunks::<3>().0.to_vec(),
            local: (aabb.min, aabb.max),
            to_root,
        });
    }
    Some(out)
}

/// At most `max` of `points`, spread out (farthest point sampling from the one farthest from
/// the middle), so a reduced hull keeps its corners.
fn spread(points: &[Vec3], max: usize) -> Vec<Vec3> {
    if points.len() <= max {
        return points.to_vec();
    }
    let middle = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let first = points
        .iter()
        .copied()
        .max_by(|a, b| {
            a.distance_squared(middle)
                .total_cmp(&b.distance_squared(middle))
        })
        .unwrap_or_default();
    let mut chosen = vec![first];
    let mut nearest: Vec<f32> = points.iter().map(|p| p.distance_squared(first)).collect();
    while chosen.len() < max {
        let (i, d) = nearest
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or_default();
        if d <= 0.0 {
            break;
        }
        let p = points[i];
        chosen.push(p);
        for (n, q) in nearest.iter_mut().zip(points) {
            *n = n.min(q.distance_squared(p));
        }
    }
    chosen
}

fn to_parry(points: &[Vec3]) -> Vec<Vector> {
    points.iter().map(|p| Vector::new(p.x, p.y, p.z)).collect()
}

fn from_parry(points: &[Vector]) -> Vec<Vec3> {
    points.iter().map(|p| Vec3::new(p.x, p.y, p.z)).collect()
}

/// The convex hull of `points`, at most [`MAX_HULL_POINTS`] corners.
fn hull(points: &[Vec3]) -> Option<ColliderShape> {
    if points.len() < 4 {
        return None;
    }
    let (corners, _) = rapier3d::parry::transformation::try_convex_hull(&to_parry(points)).ok()?;
    let corners = from_parry(&corners);
    (corners.len() >= 4).then(|| ColliderShape::ConvexHull {
        points: spread(&corners, MAX_HULL_POINTS),
    })
}

/// Convex pieces of a triangle soup (V-HACD).
fn convex_parts(points: &[Vec3], triangles: &[[u32; 3]]) -> Vec<ColliderShape> {
    if triangles.is_empty() {
        return Vec::new();
    }
    let params = VHACDParameters {
        resolution: 64,
        max_convex_hulls: MAX_PARTS,
        ..Default::default()
    };
    VHACD::decompose(&params, &to_parry(points), triangles, false)
        .compute_convex_hulls(4)
        .into_iter()
        .filter_map(|(corners, _)| hull(&from_parry(&corners)))
        .collect()
}

/// The collider `fit` wraps around `points` and `triangles` (one mesh, or several merged into
/// one soup). `None` when there is nothing to wrap.
pub fn fit_points(points: &[Vec3], triangles: &[[u32; 3]], fit: Fit) -> Option<ColliderShape> {
    match fit {
        Fit::Box => {
            let (low, high) = points.iter().fold(
                (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                |(lo, hi), p| (lo.min(*p), hi.max(*p)),
            );
            (low.x <= high.x).then(|| {
                ColliderShape::Compound(vec![(
                    (low + high) * 0.5,
                    Quat::IDENTITY,
                    ColliderShape::Cuboid {
                        half_extents: ((high - low) * 0.5).max(Vec3::splat(0.005)),
                    },
                )])
            })
        }
        Fit::Hull => hull(points),
        Fit::Parts => {
            let pieces = convex_parts(points, triangles);
            (!pieces.is_empty()).then(|| {
                ColliderShape::Compound(
                    pieces
                        .into_iter()
                        .map(|s| (Vec3::ZERO, Quat::IDENTITY, s))
                        .collect(),
                )
            })
        }
        Fit::Mesh => (!triangles.is_empty()).then(|| ColliderShape::TriMesh {
            vertices: points.to_vec(),
            indices: triangles.to_vec(),
        }),
    }
}

/// The shape a mesh named for collision stands for, in the root's space.
fn named_shape(part: &Part, kind: Named) -> Option<(Vec3, Quat, ColliderShape)> {
    let (scale, rotation, _) = part.to_root.to_scale_rotation_translation();
    let (scale, rotation) = (scale.as_vec3(), rotation.as_quat());
    let (low, high) = part.local;
    let half = ((high - low) * 0.5 * scale).abs().max(Vec3::splat(0.005));
    let center = part
        .to_root
        .transform_point3(((low + high) * 0.5).as_dvec3())
        .as_vec3();
    let shape = match kind {
        Named::Box => ColliderShape::Cuboid { half_extents: half },
        Named::Sphere => ColliderShape::Sphere {
            radius: half.max_element(),
        },
        Named::Capsule => {
            let radius = half.x.max(half.z);
            ColliderShape::Capsule {
                radius,
                half_height: (half.y - radius).max(0.0),
            }
        }
        Named::Convex => return hull(&part.points).map(|s| (Vec3::ZERO, Quat::IDENTITY, s)),
        Named::Mesh => {
            return fit_points(&part.points, &part.triangles, Fit::Mesh)
                .map(|s| (Vec3::ZERO, Quat::IDENTITY, s));
        }
    };
    Some((center, rotation, shape))
}

/// The collider `fit` wraps around the meshes under `root`, in its space, and the meshes that
/// only stand for collision (to hide). `None` while a mesh is still loading; `Some((None, _))`
/// when there is nothing to wrap.
#[allow(clippy::type_complexity)]
pub fn fit_model(
    world: &World,
    root: Entity,
    fit: Fit,
) -> Option<(Option<ColliderShape>, Vec<Entity>)> {
    let parts = parts(world, root)?;
    Some(fit_parts(&parts, fit))
}

#[allow(clippy::type_complexity)]
fn fit_parts(parts: &[Part], fit: Fit) -> (Option<ColliderShape>, Vec<Entity>) {
    let named: Vec<(Vec3, Quat, ColliderShape)> = parts
        .iter()
        .filter_map(|p| named_shape(p, p.named?))
        .collect();
    let hidden: Vec<Entity> = parts
        .iter()
        .filter(|p| p.named.is_some())
        .map(|p| p.entity)
        .collect();
    if !named.is_empty() {
        return (Some(ColliderShape::Compound(named)), hidden);
    }
    let mut points = Vec::new();
    let mut triangles = Vec::new();
    for part in parts {
        let base = points.len() as u32;
        points.extend_from_slice(&part.points);
        triangles.extend(part.triangles.iter().map(|t| t.map(|i| i + base)));
    }
    (fit_points(&points, &triangles, fit), hidden)
}

/// Fitted shapes by (meshes and where they sit, fit), so a village of one house model fits it
/// once (V-HACD takes a moment on a big model).
#[derive(Resource, Default)]
pub(crate) struct Fitted(HashMap<(Vec<PartKey>, Fit), Option<ColliderShape>>);

/// A mesh and its placement in the model, bit for bit.
type PartKey = (Handle<Mesh>, [u64; 12]);

/// Give each [`FitCollider`] whose meshes have loaded its collider (`Update`).
pub(crate) fn fit_colliders(world: &mut World) {
    let todo: Vec<(Entity, FitCollider)> = world
        .query_filtered::<(Entity, &FitCollider), Without<Collider>>()
        .iter(world)
        .map(|(e, f)| (e, *f))
        .collect();
    for (e, f) in todo {
        let dynamic = world
            .get::<RigidBody>(e)
            .is_some_and(|rb| rb.kind == BodyKind::Dynamic);
        // Moving triangle meshes collide poorly: a dynamic body gets convex parts.
        let fit = if dynamic && f.fit == Fit::Mesh {
            Fit::Parts
        } else {
            f.fit
        };
        let Some(parts) = parts(world, e) else {
            continue;
        };
        let key = (parts.iter().map(|p| p.key).collect::<Vec<_>>(), fit);
        let cached = world
            .get_resource::<Fitted>()
            .and_then(|f| f.0.get(&key).cloned());
        let (shape, hidden) = match cached {
            Some(shape) => {
                let hidden = parts
                    .iter()
                    .filter(|p| p.named.is_some())
                    .map(|p| p.entity)
                    .collect();
                (shape, hidden)
            }
            None => {
                let (shape, hidden) = fit_parts(&parts, fit);
                world
                    .get_resource_or_init::<Fitted>()
                    .0
                    .insert(key, shape.clone());
                (shape, hidden)
            }
        };
        for h in hidden {
            if let Some(mut r) = world.get_mut::<MeshRenderer>(h) {
                r.visible = false;
                r.cast_shadows = false;
            }
        }
        let Ok(mut ent) = world.get_entity_mut(e) else {
            continue;
        };
        ent.remove::<FitCollider>();
        let Some(shape) = shape else {
            continue;
        };
        if !ent.contains::<RigidBody>() {
            ent.insert(RigidBody::fixed());
        }
        ent.insert(Collider {
            shape,
            surface: f.surface,
            density: f.density,
            ..Default::default()
        });
    }
}

/// Any [`FitCollider`] waiting (so [`fit_colliders`], which takes the whole world, runs only
/// then).
pub(crate) fn fits_waiting(fits: Query<(), (With<FitCollider>, Without<Collider>)>) -> bool {
    !fits.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let points: Vec<Vec3> = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { -1.0 } else { 1.0 },
                    if i & 2 == 0 { -1.0 } else { 1.0 },
                    if i & 4 == 0 { -1.0 } else { 1.0 },
                )
            })
            .collect();
        let triangles = vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ];
        (points, triangles)
    }

    #[test]
    fn every_fit_wraps_a_cube() {
        let (points, triangles) = cube();
        for fit in [Fit::Box, Fit::Hull, Fit::Parts, Fit::Mesh] {
            let shape = fit_points(&points, &triangles, fit)
                .unwrap_or_else(|| panic!("{fit:?} wrapped nothing"));
            let ok = match (&shape, fit) {
                (ColliderShape::Compound(p), Fit::Box) => {
                    matches!(p[0].2, ColliderShape::Cuboid { half_extents } if (half_extents - Vec3::ONE).length() < 1e-5)
                }
                (ColliderShape::ConvexHull { points }, Fit::Hull) => points.len() == 8,
                (ColliderShape::Compound(p), Fit::Parts) => !p.is_empty(),
                (ColliderShape::TriMesh { indices, .. }, Fit::Mesh) => indices.len() == 12,
                _ => false,
            };
            assert!(ok, "{fit:?} gave {shape:?}");
        }
        assert!(fit_points(&[], &[], Fit::Mesh).is_none());
    }

    #[test]
    fn collision_names_are_read() {
        assert_eq!(named("UCX_Pier_00"), Some(Named::Convex));
        assert_eq!(named("ubx_wall/1"), Some(Named::Box));
        assert_eq!(named("Hull-convcolonly"), Some(Named::Convex));
        assert_eq!(named("Pier-colonly"), Some(Named::Mesh));
        assert_eq!(named("Pier"), None);
    }
}
