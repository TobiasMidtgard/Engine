//! Colliders shaped from what is drawn: a convex hull round a prop's own vertices, as Unreal's
//! auto convex collision and Unity's convex `MeshCollider` make one from the render mesh
//! (simple collision, never the render triangles themselves). Playtest 3 item 13 and Physics'
//! collider audit (engine-design/collider-audit/README.md) found the plaza's stalls, crates,
//! benches, barrels, lamps and the maypole walk-through.
//!
//! The hull keeps only the farthest vertex along each of [`DIRECTIONS`] directions spread over
//! the sphere, so a box keeps its 8 corners and a barrel a few dozen points: the physics builds
//! it every time the collider streams in near the action ([`crate::scenery::NearSolid`]), so it
//! is kept small, as Unreal caps a hull's vertices (`MaxHullVerts`).

use engine::prelude::*;
use engine::world::physics::ColliderShape;

/// Directions the hull's points are picked along (a Fibonacci sphere).
const DIRECTIONS: usize = 64;

/// The extreme points of `points` along [`DIRECTIONS`] directions: the corners of their convex
/// hull, at most that many.
pub fn hull_points(points: &[Vec3]) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = Vec::new();
    if points.is_empty() {
        return out;
    }
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    for i in 0..DIRECTIONS {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / DIRECTIONS as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let a = golden * i as f32;
        let d = Vec3::new(r * a.cos(), y, r * a.sin());
        let best = points
            .iter()
            .copied()
            .max_by(|p, q| p.dot(d).total_cmp(&q.dot(d)))
            .expect("not empty");
        if !out.contains(&best) {
            out.push(best);
        }
    }
    out
}

/// A convex hull round `meshes` (in their shared frame, each vertex scaled by `scale`), or
/// none when they are flat or empty.
pub fn hull_of<'a>(
    meshes: impl IntoIterator<Item = &'a Mesh>,
    scale: Vec3,
) -> Option<ColliderShape> {
    let points: Vec<Vec3> = meshes
        .into_iter()
        .flat_map(|m| {
            m.vertices
                .iter()
                .map(move |v| Vec3::from(v.position) * scale)
        })
        .collect();
    hull(&points)
}

/// A convex hull round `points`, or none when they don't span a volume.
pub fn hull(points: &[Vec3]) -> Option<ColliderShape> {
    let pts = hull_points(points);
    let (lo, hi) = pts.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), &p| (lo.min(p), hi.max(p)),
    );
    if pts.len() < 4 || (hi - lo).min_element() < 0.01 {
        return None;
    }
    Some(ColliderShape::ConvexHull { points: pts.into() })
}

/// A ring wall: `count` boxes round the Y axis between radii `inner` and `outer`, from `y0` up
/// to `y1` (a basin's rim, which a hull would fill in).
pub fn ring(
    inner: f32,
    outer: f32,
    y0: f32,
    y1: f32,
    count: usize,
) -> Vec<(Vec3, Quat, ColliderShape)> {
    let mid = (inner + outer) * 0.5;
    // Long enough that neighbors overlap at the outer face.
    let half_len = outer * (std::f32::consts::PI / count as f32).tan() + 0.02;
    (0..count)
        .map(|k| {
            let a = k as f32 / count as f32 * std::f32::consts::TAU;
            (
                Vec3::new(a.cos() * mid, (y0 + y1) * 0.5, a.sin() * mid),
                Quat::from_rotation_y(-a),
                ColliderShape::Cuboid {
                    half_extents: Vec3::new((outer - inner) * 0.5, (y1 - y0) * 0.5, half_len),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_keeps_its_corners_and_a_flat_sheet_gets_no_hull() {
        let mut corners = Vec::new();
        for i in 0..8 {
            corners.push(Vec3::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { 0.0 } else { 2.0 },
                if i & 4 == 0 { -0.5 } else { 0.5 },
            ));
        }
        // Points inside never make it.
        let mut points = corners.clone();
        points.push(Vec3::new(0.1, 1.0, 0.0));
        let hull = hull_points(&points);
        assert_eq!(hull.len(), 8);
        assert!(corners.iter().all(|c| hull.contains(c)));
        let flat: Vec<Vec3> = corners.iter().map(|c| Vec3::new(c.x, 0.0, c.z)).collect();
        assert!(super::hull(&flat).is_none());
    }

    #[test]
    fn ring_boxes_close_the_circle() {
        let r = ring(2.0, 2.4, 0.0, 0.8, 16);
        assert_eq!(r.len(), 16);
        for (at, rot, shape) in &r {
            let ColliderShape::Cuboid { half_extents } = shape else {
                panic!("box");
            };
            assert!((Vec2::new(at.x, at.z).length() - 2.2).abs() < 1e-4);
            assert!((at.y - 0.4).abs() < 1e-6);
            // The box's thickness (local X) points out from the middle.
            let out = Vec3::new(at.x, 0.0, at.z).normalize();
            assert!((*rot * Vec3::X).dot(out) > 0.9999);
            // Each box spans more than its share of the outer circle.
            assert!(half_extents.z * 2.0 * 16.0 > std::f32::consts::TAU * 2.4);
        }
    }
}
