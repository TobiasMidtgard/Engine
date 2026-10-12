//! Mesh building blocks for props. All sizes in meters; UVs are in meters too (one texture
//! repeat per meter at `uv_scale` 1), so materials tile at the same scale on every prop.

use engine_core::math::{Vec2, Vec3};
use engine_render::{Mesh, Vertex};

/// A surface of revolution around +Y: `profile` points are (radius, height), bottom to top.
/// Normals are smooth around the axis and follow the profile's slope.
pub fn lathe(profile: &[Vec2], segments: u32) -> Mesh {
    let segments = segments.max(3);
    let mut vertices = Vec::with_capacity(profile.len() * (segments as usize + 1));
    let mut indices = Vec::new();
    // Distance along the profile, for V.
    let mut along = vec![0.0f32; profile.len()];
    for i in 1..profile.len() {
        along[i] = along[i - 1] + profile[i].distance(profile[i - 1]);
    }
    for (i, p) in profile.iter().enumerate() {
        let prev = profile[i.saturating_sub(1)];
        let next = profile[(i + 1).min(profile.len() - 1)];
        let d = (next - prev).normalize_or(Vec2::Y);
        // Outward normal in the profile plane: rotate the tangent by -90°.
        let n2 = Vec2::new(d.y, -d.x);
        for s in 0..=segments {
            let a = s as f32 / segments as f32 * std::f32::consts::TAU;
            let (sin, cos) = a.sin_cos();
            let position = Vec3::new(p.x * cos, p.y, p.x * sin);
            let normal = Vec3::new(n2.x * cos, n2.y, n2.x * sin).normalize_or(Vec3::Y);
            let uv = Vec2::new(a * p.x.max(0.05), along[i]);
            vertices.push(Vertex::new(position, normal, uv));
        }
    }
    let ring = segments + 1;
    for i in 0..profile.len() as u32 - 1 {
        for s in 0..segments {
            let a = i * ring + s;
            let b = a + ring;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Mesh::new(vertices, indices)
}
