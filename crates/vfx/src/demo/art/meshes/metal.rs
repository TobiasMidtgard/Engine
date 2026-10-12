//! Forged metal: rivets, bands, square bars, scrolls and chain links.

use engine_core::math::{Quat, Vec2, Vec3};
use engine_render::Mesh;

use super::{Profile, grid, moved, revolve, tube_loop};

/// A domed rivet head of `radius`, sitting on y = 0 and facing +Y.
pub fn rivet(radius: f32) -> Mesh {
    let r = radius;
    let prof = Profile::new()
        .corner(r * 1.05, -r * 0.2)
        .to(r, 0.0)
        .arc(0.0, 0.0, r, 0.0, 90.0, 3)
        .to(0.0, r);
    let mut m = revolve(&prof, 8);
    for v in &mut m.vertices {
        v.position[1] *= 0.6;
    }
    m
}

/// An iron band hugging a cylinder of `radius` (around +Y, centered on y = 0): `width` tall,
/// `thickness` proud, with softened edges.
pub fn band(radius: f32, width: f32, thickness: f32, segments: u32) -> Mesh {
    let (r, w, t) = (radius, width * 0.5, thickness);
    let e = (t * 0.35).min(w * 0.3);
    let prof = Profile::new()
        .corner(r - 0.002, -w)
        .to(r + t - e, -w)
        .arc(r + t - e, -w + e, e, -90.0, 0.0, 2)
        .to(r + t, w - e)
        .arc(r + t - e, w - e, e, 0.0, 90.0, 2)
        .corner(r - 0.002, w);
    revolve(&prof, segments)
}

/// A square bar `half` wide along `path`, twisted `turns` times along its length, with hard
/// edges (forged iron).
pub fn bar(path: &[Vec3], half: f32, turns: f32) -> Mesh {
    let n = path.len();
    if n < 2 {
        return Mesh::default();
    }
    let mut total = 0.0;
    let mut along = vec![0.0f32];
    for i in 1..n {
        total += path[i].distance(path[i - 1]);
        along.push(total);
    }
    let tangent =
        |i: usize| (path[(i + 1).min(n - 1)] - path[i.saturating_sub(1)]).normalize_or(Vec3::Y);
    let mut normal = tangent(0).any_orthonormal_vector();
    let mut prev = tangent(0);
    let mut rows = Vec::with_capacity(n + 4);
    let mut rings = Vec::with_capacity(n);
    for i in 0..n {
        let t = tangent(i);
        normal =
            (Quat::from_rotation_arc(prev, t) * normal).normalize_or(t.any_orthonormal_vector());
        prev = t;
        let twist = turns * std::f32::consts::TAU * along[i] / total.max(1e-6);
        let nrm = Quat::from_axis_angle(t, twist) * normal;
        let side = t.cross(nrm);
        let mut ring = Vec::with_capacity(8);
        for k in 0..4 {
            let a = std::f32::consts::FRAC_PI_4 + k as f32 * std::f32::consts::FRAC_PI_2;
            let p = path[i] + (side * a.cos() + nrm * a.sin()) * half * std::f32::consts::SQRT_2;
            ring.push(p);
            ring.push(p);
        }
        // Start mid-face so the seam is smooth.
        ring.rotate_left(1);
        rings.push(ring);
    }
    // End caps: poles with a crease at the rim.
    rows.push(vec![path[0]; 8]);
    rows.push(rings[0].clone());
    rows.extend(rings.iter().cloned());
    rows.push(rings[n - 1].clone());
    rows.push(vec![path[n - 1]; 8]);
    grid(&rows, true)
}

/// A flat spiral (wrought-iron scroll) in the XY plane: from radius `r0` winding inward
/// `turns` times to `r1`, starting at angle `a0` (radians). Returns the path.
pub fn scroll_path(r0: f32, r1: f32, turns: f32, a0: f32, steps: u32) -> Vec<Vec3> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let a = a0 + t * turns * std::f32::consts::TAU;
            let r = r0 + (r1 - r0) * t;
            Vec3::new(a.cos() * r, a.sin() * r, 0.0)
        })
        .collect()
}

/// One oval chain link `length` long (along X) and `width` wide, of wire radius `wire`, in
/// the XY plane, centered on the origin.
pub fn chain_link(length: f32, width: f32, wire: f32) -> Mesh {
    let half_straight = ((length - width) * 0.5).max(0.0);
    let r = (width - wire * 2.0) * 0.5;
    let mut path = Vec::new();
    for end in [1.0f32, -1.0] {
        for k in 0..=6 {
            let a = -std::f32::consts::FRAC_PI_2 + k as f32 / 6.0 * std::f32::consts::PI;
            let a = if end > 0.0 {
                a
            } else {
                a + std::f32::consts::PI
            };
            path.push(Vec3::new(
                end * half_straight + a.cos() * r,
                a.sin() * r,
                0.0,
            ));
        }
    }
    tube_loop(&path, Vec2::splat(wire), 6)
}

/// A hanging chain of `links` links from `top` straight down, each link turned 90° from the
/// last.
pub fn chain(top: Vec3, links: u32, length: f32, width: f32, wire: f32) -> Mesh {
    let mut m = Mesh::default();
    let pitch = length - wire * 2.2;
    for i in 0..links {
        let c = top - Vec3::Y * (length * 0.5 + i as f32 * pitch);
        let rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)
            * Quat::from_rotation_x(if i % 2 == 0 {
                0.0
            } else {
                std::f32::consts::FRAC_PI_2
            });
        m.merge(&moved(chain_link(length, width, wire), c, rot));
    }
    m
}
