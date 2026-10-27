//! Colliders fitted to a placed model's shape.
//!
//! A model's import settings choose how Make solid and Make physical wrap it (Unreal's
//! collision presets and Auto Convex Collision, Godot's import physics shapes): one box, one
//! convex hull, convex parts (the mesh split into convex pieces by V-HACD, so a well, an arch
//! or a cup stays open), or the exact triangles for things that never move. Collision meshes
//! modelled in the DCC tool win over all of these: Unreal's `UBX_` / `USP_` / `UCP_` / `UCX_`
//! prefixes (box, sphere, capsule, convex) and Godot's `-colonly` / `-convcolonly` suffixes.
//! Those meshes are hidden in the placed copy and only give the collider its shape.
//!
//! Hulls keep at most [`MAX_HULL_POINTS`] points, so a scene file stays small. The selection's
//! colliders are drawn as green wireframes in the viewport (Unity's collider gizmos).

use std::collections::HashMap;

use engine::debug::DebugDraw;
use engine::prelude::*;
use engine::render::{Mesh, MeshRef, MeshRenderer};
use engine::world::physics::{Collider, ColliderShape};
use rapier3d::parry::math::Vector;
use rapier3d::parry::transformation::vhacd::{VHACD, VHACDParameters};

/// The collider choices of a model's import settings, with what each does.
pub const KINDS: &[(&str, &str)] = &[
    ("box", "One box around the whole model. Cheapest"),
    (
        "convex",
        "One convex hull: a tight wrap, but hollows and holes are filled in",
    ),
    (
        "convex parts",
        "Split into convex pieces (V-HACD), so hollows, arches and holes stay open. Works on \
         moving bodies",
    ),
    (
        "mesh",
        "The exact triangles. Only for things that don't move (Make physical uses convex \
         parts instead)",
    ),
    ("none", "No collider"),
];

/// Most points one hull keeps (Unreal's Max Hull Verts goes up to 32).
pub const MAX_HULL_POINTS: usize = 32;
/// Most pieces convex parts makes.
const MAX_PARTS: u32 = 24;

/// The colour colliders are drawn in (Unity's collider green).
const COLOR: [f32; 4] = [0.45, 1.0, 0.45, 0.9];

/// What a mesh named for collision stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    Box,
    Sphere,
    Capsule,
    Convex,
    Mesh,
}

/// A node name's collision meaning: Unreal's `UBX_` / `USP_` / `UCP_` / `UCX_` prefixes
/// ("UCX_Well_00"), Godot's `-convcolonly` and `-colonly` suffixes. A primitive's
/// "name/1" counts as its node's.
pub fn named(name: &str) -> Option<Named> {
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

/// One mesh of a placed model.
struct Part {
    named: Option<Named>,
    /// Points in the model root's space.
    points: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    /// The mesh's own bounds and the node's placement in the root's space, for the box,
    /// sphere and capsule a named mesh stands for.
    local: (Vec3, Vec3),
    to_root: DAffine3,
}

/// The meshes under `root`, in its space.
fn parts(world: &World, root: Entity) -> Vec<Part> {
    let Some(meshes) = world.get_resource::<Assets<Mesh>>() else {
        return Vec::new();
    };
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
            MeshRef::Asset(key) => meshes.loaded_by_key(&engine::assets::normalize_path(key)),
            MeshRef::Shape(_) => None,
        };
        let Some(mesh) = handle.and_then(|h| meshes.get(h)) else {
            continue;
        };
        let aabb = mesh.aabb();
        out.push(Part {
            named,
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
    out
}

/// At most `max` of `points`, spread out (farthest point sampling from the one farthest from
/// the middle), so a reduced hull keeps the corners.
pub fn spread(points: &[Vec3], max: usize) -> Vec<Vec3> {
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

/// The convex hull's corners of `points`, at most [`MAX_HULL_POINTS`].
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
    let vhacd = VHACD::decompose(&params, &to_parry(points), triangles, false);
    vhacd
        .compute_convex_hulls(4)
        .into_iter()
        .filter_map(|(corners, _)| hull(&from_parry(&corners)))
        .collect()
}

/// The shape a mesh named for collision stands for, in the root's space.
fn named_shape(part: &Part, kind: Named) -> Option<(Vec3, Quat, ColliderShape)> {
    let (scale, rotation, _) = part.to_root.to_scale_rotation_translation();
    let scale = scale.as_vec3();
    let rotation = rotation.as_quat();
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
            return (!part.triangles.is_empty()).then(|| {
                (
                    Vec3::ZERO,
                    Quat::IDENTITY,
                    ColliderShape::TriMesh {
                        vertices: part.points.clone(),
                        indices: part.triangles.clone(),
                    },
                )
            });
        }
    };
    Some((center, rotation, shape))
}

/// Every part's points and triangles in one soup.
fn merged<'a>(parts: impl Iterator<Item = &'a Part>) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut points = Vec::new();
    let mut triangles = Vec::new();
    for part in parts {
        let base = points.len() as u32;
        points.extend_from_slice(&part.points);
        triangles.extend(part.triangles.iter().map(|t| t.map(|i| i + base)));
    }
    (points, triangles)
}

/// Fitted shapes by (model key, kind, import scale): V-HACD takes a moment on big models.
#[derive(Resource, Default)]
pub struct Fitted(HashMap<(String, String, u64), Option<ColliderShape>>);

impl Fitted {
    /// Forget `key`'s shapes (its file changed).
    pub fn forget(&mut self, key: &str) {
        self.0.retain(|(k, _, _), _| k != key);
    }
}

/// The collider `kind` fits around placed model `root` (of file `key`), in the root's space.
/// `None` for "box" and "none" without collision meshes, and when there is nothing to fit:
/// the caller falls back to a box.
pub fn fit(world: &mut World, root: Entity, key: &str, kind: &str) -> Option<ColliderShape> {
    let scale = crate::models::import_scale(world, key);
    let id = (key.to_string(), kind.to_string(), scale.to_bits());
    if let Some(shape) = world.get_resource::<Fitted>().and_then(|f| f.0.get(&id)) {
        return shape.clone();
    }
    let parts = parts(world, root);
    let named: Vec<(Vec3, Quat, ColliderShape)> = parts
        .iter()
        .filter_map(|p| named_shape(p, p.named?))
        .collect();
    let render = || parts.iter().filter(|p| p.named.is_none());
    let shape = if kind == "none" {
        None
    } else if !named.is_empty() {
        Some(ColliderShape::Compound(named))
    } else {
        match kind {
            "convex" => hull(&merged(render()).0),
            "convex parts" => {
                let (points, triangles) = merged(render());
                let pieces = convex_parts(&points, &triangles);
                (!pieces.is_empty()).then(|| {
                    ColliderShape::Compound(
                        pieces
                            .into_iter()
                            .map(|s| (Vec3::ZERO, Quat::IDENTITY, s))
                            .collect(),
                    )
                })
            }
            "mesh" => {
                let (vertices, triangles) = merged(render());
                (!triangles.is_empty()).then_some(ColliderShape::TriMesh {
                    vertices,
                    indices: triangles,
                })
            }
            _ => None,
        }
    };
    world
        .get_resource_or_init::<Fitted>()
        .0
        .insert(id, shape.clone());
    shape
}

/// Hide the meshes of a freshly spawned model that only stand for collision.
pub fn hide_collision_meshes(world: &mut World, root: Entity) {
    let mut stack = vec![(root, false)];
    while let Some((e, hidden)) = stack.pop() {
        let hidden = hidden
            || world
                .get::<Name>(e)
                .is_some_and(|n| named(n.as_str()).is_some());
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter().map(|c| (c, hidden)));
        }
        if hidden && let Some(mut r) = world.get_mut::<MeshRenderer>(e) {
            r.visible = false;
            r.cast_shadows = false;
        }
    }
}

/// How many convex pieces and points a shape has, for a note: "12 convex parts".
pub fn describe(shape: &ColliderShape) -> String {
    match shape {
        ColliderShape::Compound(parts) if parts.len() == 1 => describe(&parts[0].2),
        ColliderShape::Compound(parts) => {
            let hulls = parts
                .iter()
                .filter(|(_, _, s)| matches!(s, ColliderShape::ConvexHull { .. }))
                .count();
            if hulls == parts.len() {
                format!("{hulls} convex parts")
            } else {
                format!("{} shapes", parts.len())
            }
        }
        ColliderShape::ConvexHull { points } => format!("a convex hull of {} points", points.len()),
        ColliderShape::TriMesh { indices, .. } => format!("{} triangles", indices.len()),
        ColliderShape::Cuboid { .. } => "a box".into(),
        _ => "a shape".into(),
    }
}

/// Draw `entity`'s collider as a wireframe.
pub fn draw(world: &mut World, entity: Entity) {
    let (Some(collider), Some(global)) = (
        world.get::<Collider>(entity).cloned(),
        world.get::<GlobalTransform>(entity).copied(),
    ) else {
        return;
    };
    let mut lines = Vec::new();
    outline(&collider.shape, DAffine3::IDENTITY, &mut lines);
    let mut debug = world.resource_mut::<DebugDraw>();
    for (a, b) in lines.into_iter().take(40_000) {
        debug.line(
            global.0.transform_point3(a),
            global.0.transform_point3(b),
            COLOR,
        );
    }
}

/// The edges of `shape` placed by `at`.
fn outline(shape: &ColliderShape, at: DAffine3, lines: &mut Vec<(DVec3, DVec3)>) {
    let p = |v: Vec3| at.transform_point3(v.as_dvec3());
    match shape {
        ColliderShape::Cuboid { half_extents: h } => {
            let corner = |i: usize| {
                Vec3::new(
                    if i & 1 == 0 { -h.x } else { h.x },
                    if i & 2 == 0 { -h.y } else { h.y },
                    if i & 4 == 0 { -h.z } else { h.z },
                )
            };
            for i in 0..8 {
                for bit in [1, 2, 4] {
                    if i & bit == 0 {
                        lines.push((p(corner(i)), p(corner(i | bit))));
                    }
                }
            }
        }
        ColliderShape::Sphere { radius } => ring_set(*radius, 0.0, at, lines),
        ColliderShape::Capsule {
            radius,
            half_height,
        }
        | ColliderShape::Cylinder {
            radius,
            half_height,
        } => {
            ring_set(*radius, *half_height, at, lines);
            for k in 0..4 {
                let a = std::f32::consts::FRAC_PI_2 * k as f32;
                let side = Vec3::new(a.cos(), 0.0, a.sin()) * *radius;
                lines.push((
                    p(side + Vec3::Y * *half_height),
                    p(side - Vec3::Y * *half_height),
                ));
            }
        }
        ColliderShape::ConvexHull { points } => {
            if let Ok((corners, faces)) =
                rapier3d::parry::transformation::try_convex_hull(&to_parry(points))
            {
                let corners = from_parry(&corners);
                triangle_edges(&corners, &faces, at, lines);
            }
        }
        ColliderShape::TriMesh { vertices, indices } => {
            triangle_edges(vertices, indices, at, lines);
        }
        ColliderShape::Compound(parts) => {
            for (offset, rotation, shape) in parts {
                let local =
                    DAffine3::from_rotation_translation(rotation.as_dquat(), offset.as_dvec3());
                outline(shape, at * local, lines);
            }
        }
        ColliderShape::Heightfield { .. } => {}
    }
}

/// Three rings (and caps `half_height` up and down for capsules and cylinders).
fn ring_set(radius: f32, half_height: f32, at: DAffine3, lines: &mut Vec<(DVec3, DVec3)>) {
    const N: usize = 24;
    let p = |v: Vec3| at.transform_point3(v.as_dvec3());
    for k in 0..N {
        let (a, b) = (
            std::f32::consts::TAU * k as f32 / N as f32,
            std::f32::consts::TAU * (k + 1) as f32 / N as f32,
        );
        let (ca, sa, cb, sb) = (a.cos(), a.sin(), b.cos(), b.sin());
        for y in [-half_height, half_height] {
            lines.push((
                p(Vec3::new(ca * radius, y, sa * radius)),
                p(Vec3::new(cb * radius, y, sb * radius)),
            ));
        }
        if half_height == 0.0 {
            lines.push((
                p(Vec3::new(ca * radius, sa * radius, 0.0)),
                p(Vec3::new(cb * radius, sb * radius, 0.0)),
            ));
            lines.push((
                p(Vec3::new(0.0, ca * radius, sa * radius)),
                p(Vec3::new(0.0, cb * radius, sb * radius)),
            ));
        }
    }
}

/// Each edge of a triangle list once.
fn triangle_edges(
    points: &[Vec3],
    triangles: &[[u32; 3]],
    at: DAffine3,
    lines: &mut Vec<(DVec3, DVec3)>,
) {
    let mut seen = std::collections::HashSet::new();
    for t in triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if seen.insert((a.min(b), a.max(b)))
                && let (Some(pa), Some(pb)) = (points.get(a as usize), points.get(b as usize))
            {
                lines.push((
                    at.transform_point3(pa.as_dvec3()),
                    at.transform_point3(pb.as_dvec3()),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_names_from_unreal_and_godot() {
        assert_eq!(named("UCX_Well_00"), Some(Named::Convex));
        assert_eq!(named("ubx_Crate"), Some(Named::Box));
        assert_eq!(named("USP_Ball_01/0"), Some(Named::Sphere));
        assert_eq!(named("UCP_Pillar"), Some(Named::Capsule));
        assert_eq!(named("Rock-colonly"), Some(Named::Mesh));
        assert_eq!(named("Rock-convcolonly"), Some(Named::Convex));
        assert_eq!(named("Wall"), None);
        assert_eq!(named("UCXWall"), None);
    }

    #[test]
    fn spread_keeps_the_corners() {
        // A cube's corners plus many points inside: the eight corners survive.
        let mut points: Vec<Vec3> = (0..8)
            .map(|i| Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32))
            .collect();
        for i in 0..200 {
            let t = i as f32 / 200.0;
            points.push(Vec3::new(0.2 + 0.6 * t, 0.5, 0.3 + 0.2 * t));
        }
        let kept = spread(&points, 8);
        assert_eq!(kept.len(), 8);
        for corner in &points[..8] {
            assert!(kept.contains(corner), "{corner} was dropped");
        }
    }

    /// An open ring (a well's wall): one hull fills the middle, convex parts leave it open.
    #[test]
    fn convex_parts_keep_a_ring_open() {
        let n = 24;
        let mut points = Vec::new();
        let mut triangles = Vec::new();
        for k in 0..n {
            let a = std::f32::consts::TAU * k as f32 / n as f32;
            let (c, s) = (a.cos(), a.sin());
            // Outer and inner wall, bottom and top.
            for (r, y) in [(1.0, 0.0), (1.0, 1.0), (0.8, 0.0), (0.8, 1.0)] {
                points.push(Vec3::new(c * r, y, s * r));
            }
        }
        let v = |k: usize, i: u32| (k % n * 4) as u32 + i;
        for k in 0..n {
            for (a, b) in [(0, 1), (3, 2), (1, 3), (2, 0)] {
                let (p, q, r, s) = (v(k, a), v(k, b), v(k + 1, a), v(k + 1, b));
                triangles.push([p, r, q]);
                triangles.push([q, r, s]);
            }
        }
        let one = hull(&points).unwrap();
        let ColliderShape::ConvexHull { points: corners } = &one else {
            panic!("{one:?}")
        };
        assert!(corners.len() <= MAX_HULL_POINTS);
        let pieces = convex_parts(&points, &triangles);
        assert!(pieces.len() >= 4, "{} pieces", pieces.len());
        // No piece reaches the middle.
        for piece in &pieces {
            let ColliderShape::ConvexHull { points } = piece else {
                panic!()
            };
            let middle = points.iter().copied().sum::<Vec3>() / points.len() as f32;
            assert!(Vec2::new(middle.x, middle.z).length() > 0.5, "{middle}");
        }
    }
}
