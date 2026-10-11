//! CPU-side meshes and procedural shapes.

use std::f32::consts::{PI, TAU};

use bytemuck::{Pod, Zeroable};
use engine_assets::Asset;
use engine_core::math::{Aabb, Vec2, Vec3, Vec4};
use serde::{Deserialize, Serialize};

/// The engine's standard vertex: 48 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGBA vertex color (multiplied with the material's base color).
    pub color: [f32; 4],
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self {
            position: position.to_array(),
            normal: normal.to_array(),
            uv: uv.to_array(),
            color: [1.0; 4],
        }
    }

    pub fn with_color(mut self, color: Vec4) -> Self {
        self.color = color.to_array();
        self
    }

    pub const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Triangle mesh (indexed, counter-clockwise front faces).
#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Asset for Mesh {}

impl Mesh {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::from_points(self.vertices.iter().map(|v| Vec3::from(v.position)))
    }

    /// Recompute smooth normals from triangles.
    pub fn compute_normals(&mut self) {
        let mut normals = vec![Vec3::ZERO; self.vertices.len()];
        for tri in self.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let pa = Vec3::from(self.vertices[a].position);
            let pb = Vec3::from(self.vertices[b].position);
            let pc = Vec3::from(self.vertices[c].position);
            let n = (pb - pa).cross(pc - pa);
            normals[a] += n;
            normals[b] += n;
            normals[c] += n;
        }
        for (v, n) in self.vertices.iter_mut().zip(normals) {
            v.normal = n.normalize_or(Vec3::Y).to_array();
        }
    }

    /// Paint every vertex with one color.
    pub fn with_vertex_color(mut self, color: Vec4) -> Self {
        for v in &mut self.vertices {
            v.color = color.to_array();
        }
        self
    }

    /// Append another mesh (merging draw calls).
    pub fn merge(&mut self, other: &Mesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    /// Transform every vertex in place.
    pub fn transform(&mut self, m: &engine_core::math::Mat4) {
        let nm = engine_core::math::Mat3::from_mat4(*m).inverse().transpose();
        for v in &mut self.vertices {
            v.position = m.transform_point3(Vec3::from(v.position)).to_array();
            v.normal = (nm * Vec3::from(v.normal)).normalize_or(Vec3::Y).to_array();
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Serializable description of a procedural mesh. Scenes and AI agents use these so they
/// never need to ship mesh data: `{"Cuboid": {"size": [1, 2, 1]}}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Cuboid { size: [f32; 3] },
    Sphere { radius: f32, segments: u32 },
    Plane { size: [f32; 2], subdivisions: u32 },
    Capsule { radius: f32, height: f32, segments: u32 },
    Cylinder { radius: f32, height: f32, segments: u32 },
    Cone { radius: f32, height: f32, segments: u32 },
    Torus { radius: f32, tube: f32, segments: u32 },
    /// Single quad facing +Z.
    Quad { size: [f32; 2] },
}

impl Shape {
    /// Stable key for sharing identical shapes.
    pub fn key(&self) -> String {
        format!("shape:{self:?}")
    }

    pub fn mesh(&self) -> Mesh {
        match *self {
            Shape::Cuboid { size } => cuboid(Vec3::from(size)),
            Shape::Sphere { radius, segments } => uv_sphere(radius, segments.max(3)),
            Shape::Plane { size, subdivisions } => plane(Vec2::from(size), subdivisions),
            Shape::Capsule {
                radius,
                height,
                segments,
            } => capsule(radius, height, segments.max(3)),
            Shape::Cylinder {
                radius,
                height,
                segments,
            } => cylinder(radius, radius, height, segments.max(3)),
            Shape::Cone {
                radius,
                height,
                segments,
            } => cylinder(radius, 0.0, height, segments.max(3)),
            Shape::Torus {
                radius,
                tube,
                segments,
            } => torus(radius, tube, segments.max(3)),
            Shape::Quad { size } => quad(Vec2::from(size)),
        }
    }
}

/// Box centered on the origin.
pub fn cuboid(size: Vec3) -> Mesh {
    let h = size * 0.5;
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut m = Mesh::default();
    for (n, u, v) in faces {
        let base = m.vertices.len() as u32;
        let center = n * n.abs().dot(h);
        let ue = u * u.abs().dot(h);
        let ve = v * v.abs().dot(h);
        for (du, dv, uv) in [
            (-1.0, -1.0, Vec2::new(0.0, 1.0)),
            (1.0, -1.0, Vec2::new(1.0, 1.0)),
            (1.0, 1.0, Vec2::new(1.0, 0.0)),
            (-1.0, 1.0, Vec2::new(0.0, 0.0)),
        ] {
            m.vertices
                .push(Vertex::new(center + ue * du + ve * dv, n, uv));
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    m
}

/// Flat plane on XZ facing +Y, centered on the origin.
pub fn plane(size: Vec2, subdivisions: u32) -> Mesh {
    let n = subdivisions + 1;
    let mut m = Mesh::default();
    for z in 0..=n {
        for x in 0..=n {
            let u = x as f32 / n as f32;
            let v = z as f32 / n as f32;
            let p = Vec3::new((u - 0.5) * size.x, 0.0, (v - 0.5) * size.y);
            m.vertices.push(Vertex::new(p, Vec3::Y, Vec2::new(u, v) * size));
        }
    }
    let row = n + 1;
    for z in 0..n {
        for x in 0..n {
            let i = z * row + x;
            m.indices
                .extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    m
}

/// Quad in the XY plane facing +Z.
pub fn quad(size: Vec2) -> Mesh {
    let h = size * 0.5;
    let v = |x: f32, y: f32, u: f32, w: f32| Vertex::new(Vec3::new(x, y, 0.0), Vec3::Z, Vec2::new(u, w));
    Mesh::new(
        vec![
            v(-h.x, -h.y, 0.0, 1.0),
            v(h.x, -h.y, 1.0, 1.0),
            v(h.x, h.y, 1.0, 0.0),
            v(-h.x, h.y, 0.0, 0.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
}

pub fn uv_sphere(radius: f32, segments: u32) -> Mesh {
    let rings = segments / 2 + 1;
    let mut m = Mesh::default();
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let phi = v * PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * TAU;
            let n = Vec3::new(phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin());
            m.vertices.push(Vertex::new(n * radius, n, Vec2::new(u, v)));
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let i = r * row + s;
            m.indices
                .extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    m
}

/// Cylinder (or cone when `top_radius` is 0) along +Y, centered on the origin, with caps.
pub fn cylinder(bottom_radius: f32, top_radius: f32, height: f32, segments: u32) -> Mesh {
    let mut m = Mesh::default();
    let hh = height * 0.5;
    let slope = (bottom_radius - top_radius) / height.max(1e-6);
    for s in 0..=segments {
        let u = s as f32 / segments as f32;
        let a = u * TAU;
        let (sin, cos) = a.sin_cos();
        let n = Vec3::new(cos, slope, -sin).normalize();
        m.vertices.push(Vertex::new(
            Vec3::new(cos * bottom_radius, -hh, -sin * bottom_radius),
            n,
            Vec2::new(u, 1.0),
        ));
        m.vertices.push(Vertex::new(
            Vec3::new(cos * top_radius, hh, -sin * top_radius),
            n,
            Vec2::new(u, 0.0),
        ));
    }
    for s in 0..segments {
        let i = s * 2;
        m.indices
            .extend_from_slice(&[i, i + 2, i + 1, i + 1, i + 2, i + 3]);
    }
    for (y, r, n) in [(-hh, bottom_radius, Vec3::NEG_Y), (hh, top_radius, Vec3::Y)] {
        if r <= 0.0 {
            continue;
        }
        let center = m.vertices.len() as u32;
        m.vertices
            .push(Vertex::new(Vec3::new(0.0, y, 0.0), n, Vec2::splat(0.5)));
        for s in 0..=segments {
            let a = s as f32 / segments as f32 * TAU;
            let (sin, cos) = a.sin_cos();
            m.vertices.push(Vertex::new(
                Vec3::new(cos * r, y, -sin * r),
                n,
                Vec2::new(cos * 0.5 + 0.5, sin * 0.5 + 0.5),
            ));
        }
        for s in 0..segments {
            let a = center + 1 + s;
            if n.y > 0.0 {
                m.indices.extend_from_slice(&[center, a, a + 1]);
            } else {
                m.indices.extend_from_slice(&[center, a + 1, a]);
            }
        }
    }
    m
}

/// Capsule along +Y: `height` is the total height including the hemispheres.
pub fn capsule(radius: f32, height: f32, segments: u32) -> Mesh {
    let half_cyl = (height * 0.5 - radius).max(0.0);
    let rings = (segments / 2).max(2);
    let mut m = Mesh::default();
    // Latitude rings from top pole to bottom pole; the equator ring is duplicated to stretch.
    let mut lats: Vec<(f32, f32)> = Vec::new(); // (phi, y offset)
    for r in 0..=rings {
        let phi = r as f32 / rings as f32 * (PI * 0.5);
        lats.push((phi, half_cyl));
    }
    for r in 0..=rings {
        let phi = PI * 0.5 + r as f32 / rings as f32 * (PI * 0.5);
        lats.push((phi, -half_cyl));
    }
    for (li, (phi, yoff)) in lats.iter().enumerate() {
        let v = li as f32 / (lats.len() - 1) as f32;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * TAU;
            let n = Vec3::new(phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin());
            m.vertices.push(Vertex::new(
                n * radius + Vec3::new(0.0, *yoff, 0.0),
                n,
                Vec2::new(u, v),
            ));
        }
    }
    let row = segments + 1;
    for r in 0..(lats.len() as u32 - 1) {
        for s in 0..segments {
            let i = r * row + s;
            m.indices
                .extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    m
}

pub fn torus(radius: f32, tube: f32, segments: u32) -> Mesh {
    let sides = (segments / 2).max(3);
    let mut m = Mesh::default();
    for i in 0..=segments {
        let u = i as f32 / segments as f32;
        let a = u * TAU;
        let center = Vec3::new(a.cos() * radius, 0.0, -a.sin() * radius);
        for j in 0..=sides {
            let v = j as f32 / sides as f32;
            let b = v * TAU;
            let n = (center.normalize() * b.cos() + Vec3::Y * b.sin()).normalize();
            m.vertices
                .push(Vertex::new(center + n * tube, n, Vec2::new(u, v)));
        }
    }
    let row = sides + 1;
    for i in 0..segments {
        for j in 0..sides {
            let k = i * row + j;
            m.indices
                .extend_from_slice(&[k, k + row, k + 1, k + 1, k + row, k + row + 1]);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_winding(m: &Mesh) {
        // Every triangle's geometric normal should agree with its vertex normals.
        let mut bad = 0;
        for tri in m.indices.chunks_exact(3) {
            let p: Vec<Vec3> = tri.iter().map(|&i| Vec3::from(m.vertices[i as usize].position)).collect();
            let n: Vec3 = tri.iter().map(|&i| Vec3::from(m.vertices[i as usize].normal)).sum();
            let g = (p[1] - p[0]).cross(p[2] - p[0]);
            if g.length_squared() > 1e-12 && g.dot(n) < 0.0 {
                bad += 1;
            }
        }
        assert_eq!(bad, 0, "{bad} triangles wound clockwise");
    }

    #[test]
    fn shapes_are_counter_clockwise() {
        for shape in [
            Shape::Cuboid { size: [1.0, 2.0, 3.0] },
            Shape::Sphere { radius: 1.0, segments: 16 },
            Shape::Plane { size: [4.0, 4.0], subdivisions: 2 },
            Shape::Capsule { radius: 0.4, height: 1.8, segments: 12 },
            Shape::Cylinder { radius: 0.5, height: 2.0, segments: 12 },
            Shape::Cone { radius: 0.5, height: 1.0, segments: 12 },
            Shape::Torus { radius: 1.0, tube: 0.25, segments: 16 },
            Shape::Quad { size: [1.0, 1.0] },
        ] {
            let m = shape.mesh();
            assert!(!m.indices.is_empty(), "{shape:?}");
            check_winding(&m);
        }
    }

    #[test]
    fn cuboid_bounds() {
        let b = cuboid(Vec3::new(2.0, 4.0, 6.0)).aabb();
        assert!(b.max.abs_diff_eq(Vec3::new(1.0, 2.0, 3.0), 1e-6));
    }
}
