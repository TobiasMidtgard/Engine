//! Boulders for the paddock's spider garden: flat-topped slabs of rock whose tops match the
//! walkable boxes they stand for ([`IkSurface`](crate::ik::IkSurface) boxes), with chipped
//! corners, fractured top edges and a bulging foot sunk into the ground. Meshed from signed
//! distances like the bodies and drawn with the renderer's rock material (triplanar, so they
//! need no UVs).

use engine_core::prelude::*;
use engine_render::{Mesh, Vertex};

use super::bone;
use super::sdf::{Body, Paint, Shape};
use crate::skeleton::Skeleton;

/// A deterministic value in -1..1 for `seed` and `k`.
fn jitter(seed: u32, k: u32) -> f32 {
    let mut h = seed.wrapping_mul(0x9e37_79b9) ^ k.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// The rock's shape around its center: `half` is the walkable box's half extents (the top is
/// at `half.y`, the base at `-half.y`).
pub fn body(half: Vec3, seed: u32) -> Body {
    let stone = Paint::hard([1.0; 3]);
    let dark = Paint::hard([0.78, 0.76, 0.73]);
    let j = |k: u32| jitter(seed, k);
    let round = (half.y * 0.45).min(0.07);
    let mut b = Body::default();
    // The slab itself, its top flush with the box top.
    b.part(
        "root",
        Shape::round_box(
            Vec3::new(0.0, -0.03, 0.0),
            Vec3::new(half.x * 0.96, half.y + 0.03, half.z * 0.95),
            round,
        ),
        stone,
        0.02,
    );
    // A foot that bulges out and sinks into the ground on each side.
    for k in 0..4u32 {
        let a = (k as f32 + 0.5 + 0.3 * j(k)) * std::f32::consts::FRAC_PI_2;
        let at = Vec3::new(
            a.cos() * half.x * 0.62,
            -half.y * 0.85,
            a.sin() * half.z * 0.62,
        );
        b.part(
            "root",
            Shape::ellipsoid(
                at,
                Vec3::new(
                    half.x * (0.42 + 0.1 * j(k + 10)),
                    half.y * 0.7 + 0.04,
                    half.z * (0.45 + 0.1 * j(k + 20)),
                ),
            )
            .rotated(Quat::from_rotation_y(0.6 * j(k + 30))),
            dark,
            0.08,
        );
    }
    // Chipped vertical corners: a plane sliced off each at an uneven angle and depth (a cube
    // with one face on the plane carves everything beyond it).
    for k in 0..4u32 {
        let (sx, sz) = (
            [1.0, -1.0, -1.0, 1.0][k as usize],
            [1.0, 1.0, -1.0, -1.0][k as usize],
        );
        let yaw = (45.0 + 18.0 * j(k + 40)).to_radians();
        let corner = Vec3::new(sx * half.x, 0.0, sz * half.z);
        let out = Vec3::new(sx * yaw.sin(), 0.0, sz * yaw.cos());
        let depth = 0.08 + 0.06 * (j(k + 50) + 1.0);
        b.cut(
            "root",
            Shape::round_box(
                corner + out * (0.5 - depth),
                Vec3::new(0.5, half.y + 0.5, 0.5),
                0.0,
            )
            .rotated(Quat::from_rotation_y(out.x.atan2(out.z))),
            dark,
            0.015,
        );
    }
    // Fractured top edges: a bevel along part of each side, tilted 30-50 degrees.
    for (k, n) in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z]
        .into_iter()
        .enumerate()
    {
        let k = k as u32;
        let along_x = n.z != 0.0;
        let (reach, run) = if along_x {
            (half.z, half.x)
        } else {
            (half.x, half.z)
        };
        let tilt = (40.0 + 10.0 * j(k + 60)).to_radians();
        let normal = (n * tilt.cos() + Vec3::Y * tilt.sin()).normalize();
        let edge_axis = if along_x { Vec3::X } else { Vec3::Z };
        let bite = 0.035 + 0.025 * (j(k + 70) + 1.0);
        let center = n * reach
            + Vec3::Y * half.y
            + edge_axis * (run * 0.5 * j(k + 80))
            + normal * (0.3 - bite);
        let length = run * (0.5 + 0.25 * (j(k + 90) + 1.0));
        let size = if along_x {
            Vec3::new(length, 0.3, 0.3)
        } else {
            Vec3::new(0.3, 0.3, length)
        };
        b.cut(
            "root",
            Shape::round_box(center, size, 0.0).rotated(Quat::from_rotation_arc(Vec3::Y, normal)),
            dark,
            0.012,
        );
    }
    b
}

/// The rock as a static mesh, its origin at the walkable box's center.
pub fn mesh(half: Vec3, seed: u32) -> Mesh {
    let skeleton = Skeleton::new("rock", vec![bone("root", None, Vec3::ZERO, Quat::IDENTITY)])
        .expect("rock skeleton");
    let skinned = body(half, seed).build(&skeleton, 0.018, 0.02);
    let vertices = skinned
        .vertices
        .iter()
        .map(|v| {
            // Darker low down, where soil and damp cling to it.
            let low = ((v.position.y + half.y) / (half.y * 1.2)).clamp(0.0, 1.0);
            let shade = 0.72 + 0.28 * low;
            Vertex::new(v.position, v.normal, v.uv).with_color(Vec4::new(
                v.color.x * shade,
                v.color.y * shade,
                v.color.z * shade,
                1.0,
            ))
        })
        .collect();
    Mesh::new(vertices, skinned.indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boulder_is_walkable_where_its_box_is() {
        let half = Vec3::new(0.7, 0.28, 0.55);
        let rock = body(half, 3);
        // Flat on top over most of the box, and solid down to the ground.
        for (x, z) in [(0.0, 0.0), (0.3, 0.2), (-0.3, -0.2), (0.4, -0.25)] {
            let top = Vec3::new(x, half.y, z);
            assert!(rock.distance(top).abs() < 0.02, "top at ({x}, {z})");
            assert!(rock.distance(Vec3::new(x, -half.y + 0.05, z)) < 0.0);
        }
        let m = mesh(half, 3);
        assert!(
            m.indices.len() / 3 > 2000,
            "{} triangles",
            m.indices.len() / 3
        );
        let aabb = m.aabb();
        assert!(aabb.max.y < half.y + 0.01, "top at {}", aabb.max.y);
        assert!(aabb.min.y < -half.y, "sunk into the ground: {}", aabb.min.y);
        // Different seeds give different rocks.
        assert_ne!(mesh(half, 4).vertices.len(), m.vertices.len());
    }
}
