//! Clicking things in the scene view: a ray through the cursor tested against every drawn
//! mesh (bounding box first, then its triangles; animated characters in their current pose),
//! plus screen-space icons for things without
//! a mesh (lights, cameras, empties, module demos).

use engine::prelude::*;
use engine::render::{Mesh, ResolvedMesh};
use engine_anim::skin::{SkinPose, SkinnedMesh, SkinnedMeshRenderer, skin_on_cpu};

use crate::scene_ops::EditorOnly;

/// The nearest mesh the ray hits: entity and distance in meters.
pub fn pick_mesh(world: &mut World, origin: DVec3, dir: Vec3) -> Option<(Entity, f64)> {
    pick_mesh_except(world, origin, dir, &|_| false)
}

/// [`pick_mesh`], skipping entities `skip` names.
pub fn pick_mesh_except(
    world: &mut World,
    origin: DVec3,
    dir: Vec3,
    skip: &dyn Fn(Entity) -> bool,
) -> Option<(Entity, f64)> {
    let mut q = world.query_filtered::<(Entity, &GlobalTransform, &ResolvedMesh, &MeshRenderer), Without<EditorOnly>>();
    let meshes = world.get_resource::<Assets<Mesh>>()?;
    let mut best: Option<(Entity, f64)> = None;
    for (entity, global, resolved, renderer) in q.iter(world) {
        if !renderer.visible || skip(entity) {
            continue;
        }
        let Some(mesh) = meshes.get(resolved.mesh) else {
            continue;
        };
        if let Some(d) = ray_mesh(global, mesh, origin, dir)
            && best.is_none_or(|(_, b)| d < b)
        {
            best = Some((entity, d));
        }
    }
    // Rigs: skinned on the CPU into the pose they're drawn in.
    let mut q = world.query_filtered::<(
        Entity,
        &GlobalTransform,
        &SkinnedMeshRenderer,
        Option<&SkinPose>,
    ), Without<EditorOnly>>();
    let skinned = world.get_resource::<Assets<SkinnedMesh>>()?;
    for (entity, global, renderer, pose) in q.iter(world) {
        if !renderer.visible || skip(entity) {
            continue;
        }
        let Some(mesh) = renderer.mesh.and_then(|h| skinned.get(h)) else {
            continue;
        };
        let points: Vec<Vec3> = match pose {
            Some(pose) if !pose.joints.is_empty() => skin_on_cpu(mesh, pose).0,
            _ => mesh.vertices.iter().map(|v| v.position).collect(),
        };
        let (min, max) = points.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        let point = |i: u32| points[i as usize];
        if let Some(d) = ray_triangles(global, &point, &mesh.indices, (min, max), origin, dir)
            && best.is_none_or(|(_, b)| d < b)
        {
            best = Some((entity, d));
        }
    }
    best
}

/// The nearest surface the ray meets, mesh or ground, within `max` meters.
pub fn pick_surface(
    world: &mut World,
    origin: DVec3,
    dir: Vec3,
    max: f64,
    skip: &dyn Fn(Entity) -> bool,
) -> Option<DVec3> {
    let mesh = pick_mesh_except(world, origin, dir, skip).map(|(_, d)| d);
    let ground = world
        .get_resource::<engine::world::Ground>()
        .and_then(|g| crate::landscape::ground_hit(g, origin, dir, max))
        .map(|p| (p - origin).length());
    let d = match (mesh, ground) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => a.or(b)?,
    };
    (d <= max).then(|| origin + dir.as_dvec3() * d)
}

/// [`pick_surface`] (any distance) and the surface's normal there, from two neighbouring rays;
/// the normal faces the ray.
pub fn surface_and_normal(
    world: &mut World,
    origin: DVec3,
    dir: Vec3,
    skip: &dyn Fn(Entity) -> bool,
) -> Option<(DVec3, Vec3)> {
    let hit = pick_surface(world, origin, dir, 10_000.0, skip)?;
    let distance = (hit - origin).length().max(0.01);
    let step = (0.03 / distance) as f32;
    let side = dir.any_orthonormal_vector();
    let up = dir.cross(side);
    let a = pick_surface(
        world,
        origin,
        (dir + side * step).normalize(),
        10_000.0,
        skip,
    );
    let b = pick_surface(world, origin, (dir + up * step).normalize(), 10_000.0, skip);
    let normal = match (a, b) {
        (Some(a), Some(b)) => (a - hit).cross(b - hit).as_vec3().normalize_or_zero(),
        _ => Vec3::Y,
    };
    let normal = if normal == Vec3::ZERO {
        Vec3::Y
    } else if normal.dot(dir) > 0.0 {
        -normal
    } else {
        normal
    };
    Some((hit, normal))
}

/// Distance along a world ray to the first triangle of `mesh` placed at `global`.
pub fn ray_mesh(global: &GlobalTransform, mesh: &Mesh, origin: DVec3, dir: Vec3) -> Option<f64> {
    let aabb = mesh.aabb();
    let point = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
    ray_triangles(
        global,
        &point,
        &mesh.indices,
        (aabb.min, aabb.max),
        origin,
        dir,
    )
}

/// Distance along a world ray to the first of the triangles `indices` makes of `point`s (local
/// space, inside `bounds`), placed at `global`.
fn ray_triangles(
    global: &GlobalTransform,
    point: &dyn Fn(u32) -> Vec3,
    indices: &[u32],
    bounds: (Vec3, Vec3),
    origin: DVec3,
    dir: Vec3,
) -> Option<f64> {
    let inv = global.inverse();
    let o = inv.transform_point3(origin);
    let d = inv.transform_vector3(dir.as_dvec3());
    ray_box(o, d, bounds.0.as_dvec3(), bounds.1.as_dvec3())?;
    // Triangles in local space, relative to the ray origin for precision.
    let (o32, d32) = (DVec3::ZERO, d.as_vec3());
    let mut best = f32::INFINITY;
    for tri in indices.as_chunks::<3>().0 {
        let p = |i: u32| (point(i).as_dvec3() - o).as_vec3();
        if let Some(t) = ray_triangle(o32.as_vec3(), d32, p(tri[0]), p(tri[1]), p(tri[2]))
            && t < best
        {
            best = t;
        }
    }
    best.is_finite().then(|| {
        let hit_local = o + d * best as f64;
        (global.transform_point(hit_local) - origin).length()
    })
}

/// Slab test; the entry distance (in units of `d`) when the ray meets the box.
pub fn ray_box(o: DVec3, d: DVec3, min: DVec3, max: DVec3) -> Option<f64> {
    let inv = DVec3::ONE / d;
    let t1 = (min - o) * inv;
    let t2 = (max - o) * inv;
    let near = t1.min(t2).max_element();
    let far = t1.max(t2).min_element();
    (far >= near.max(0.0)).then_some(near.max(0.0))
}

/// Möller–Trumbore, both sides.
pub fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_the_front_of_a_cube() {
        let mesh = Shape::Cuboid { size: [2.0; 3] }.mesh();
        let global = GlobalTransform::from(Transform::from_xyz(0.0, 0.0, -10.0));
        let d = ray_mesh(&global, &mesh, DVec3::ZERO, -Vec3::Z).unwrap();
        assert!((d - 9.0).abs() < 1e-4, "{d}");
        assert!(ray_mesh(&global, &mesh, DVec3::ZERO, Vec3::Z).is_none());
        assert!(ray_mesh(&global, &mesh, DVec3::new(5.0, 0.0, 0.0), -Vec3::Z).is_none());
    }

    #[test]
    fn scaled_and_far_from_the_origin() {
        let mesh = Shape::Sphere {
            radius: 1.0,
            segments: 32,
        }
        .mesh();
        let far = DVec3::new(3.0e7, 0.0, 0.0);
        let global =
            GlobalTransform::from(Transform::from_translation(far).with_uniform_scale(3.0));
        let d = ray_mesh(&global, &mesh, far + DVec3::new(0.0, 0.0, 20.0), -Vec3::Z).unwrap();
        assert!((d - 17.0).abs() < 0.05, "{d}");
    }
}
