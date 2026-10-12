//! Small helpers for building destruction meshes (walls, chunks, voxels, rebar).

use engine_core::math::{Vec2, Vec3};
use engine_render::{Mesh, Vertex};

/// Accumulates vertices and triangles. Vertex colors carry the material colors, so one
/// material per surface type is enough and everything batches.
#[derive(Debug, Default, Clone)]
pub struct MeshBuilder {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn vertex(&mut self, position: Vec3, normal: Vec3, color: [f32; 3]) -> u32 {
        let i = self.vertices.len() as u32;
        let uv = box_uv(position, normal);
        self.vertices.push(Vertex {
            position: position.to_array(),
            normal: normal.to_array(),
            uv: uv.to_array(),
            color: [color[0], color[1], color[2], 1.0],
        });
        i
    }

    pub fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    /// Flat-shaded quad `a b c d`, wound so its front faces `outward`.
    pub fn quad_facing(&mut self, p: [Vec3; 4], outward: Vec3, color: [f32; 3]) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]) + (p[2] - p[0]).cross(p[3] - p[0]);
        if n.length_squared() < 1e-14 {
            return;
        }
        let (p, n) = if n.dot(outward) < 0.0 {
            ([p[3], p[2], p[1], p[0]], -n)
        } else {
            (p, n)
        };
        let n = n.normalize();
        let base = self.vertices.len() as u32;
        for v in p {
            self.vertex(v, n, color);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Flat convex polygon (counter-clockwise around `normal`), fan-triangulated.
    pub fn polygon(&mut self, points: &[Vec3], normal: Vec3, color: [f32; 3]) {
        if points.len() < 3 {
            return;
        }
        let base = self.vertices.len() as u32;
        for &p in points {
            self.vertex(p, normal, color);
        }
        for i in 1..points.len() as u32 - 1 {
            self.triangle(base, base + i, base + i + 1);
        }
    }

    /// Thin box from `a` to `b` (rebar, splinters), `radius` half-width.
    pub fn rod(&mut self, a: Vec3, b: Vec3, radius: f32, color: [f32; 3]) {
        let axis = b - a;
        let len = axis.length();
        if len < 1e-5 {
            return;
        }
        let d = axis / len;
        let up = if d.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = d.cross(up).normalize() * radius;
        let v = d.cross(u).normalize() * radius;
        let corners = [u + v, u - v, -u - v, -u + v];
        for k in 0..4 {
            let c0 = corners[k];
            let c1 = corners[(k + 1) % 4];
            let out = (c0 + c1).normalize();
            self.quad_facing([a + c0, a + c1, b + c1, b + c0], out, color);
        }
    }

    /// Recompute smooth normals for the vertices in `range` from the triangles that use them
    /// (area weighted). Vertices outside the range keep their normals.
    pub fn smooth_normals(
        &mut self,
        vertex_range: std::ops::Range<usize>,
        tri_range: std::ops::Range<usize>,
    ) {
        let mut acc = vec![Vec3::ZERO; vertex_range.len()];
        for tri in self.indices[tri_range].as_chunks::<3>().0 {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let pa = Vec3::from(self.vertices[a].position);
            let pb = Vec3::from(self.vertices[b].position);
            let pc = Vec3::from(self.vertices[c].position);
            let n = (pb - pa).cross(pc - pa);
            for i in [a, b, c] {
                if vertex_range.contains(&i) {
                    acc[i - vertex_range.start] += n;
                }
            }
        }
        for (k, n) in acc.into_iter().enumerate() {
            let v = &mut self.vertices[vertex_range.start + k];
            let fallback = Vec3::from(v.normal);
            v.normal = n.normalize_or(fallback).to_array();
        }
    }

    /// Append `other`, offsetting its indices.
    pub fn append(&mut self, other: &MeshBuilder) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    /// Move every vertex by `offset` (recenter a chunk on its center of mass).
    pub fn translate(&mut self, offset: Vec3) {
        for v in &mut self.vertices {
            v.position = (Vec3::from(v.position) + offset).to_array();
        }
    }

    pub fn build(self) -> Mesh {
        Mesh::new(self.vertices, self.indices)
    }
}

/// Texture coordinates in meters, projected along the normal's main axis (box mapping), with
/// +v down so an image's top is up: a wall's front (+Z) gets `(x, -y)`, the ground `(x, z)`.
/// Materials tile with `uv_scale = 1 / tile size`.
pub fn box_uv(p: Vec3, n: Vec3) -> Vec2 {
    let a = n.abs();
    if a.y >= a.x && a.y >= a.z {
        Vec2::new(p.x, if n.y >= 0.0 { p.z } else { -p.z })
    } else if a.x >= a.z {
        Vec2::new(if n.x >= 0.0 { -p.z } else { p.z }, -p.y)
    } else {
        Vec2::new(if n.z >= 0.0 { p.x } else { -p.x }, -p.y)
    }
}

/// Deterministic hash of integer coordinates to `0..1`.
pub fn hash3(x: i64, y: i64, z: i64, seed: u64) -> f32 {
    engine_core::math::hash01(x, y, z, seed)
}

/// Multiply a color by `k`.
pub fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

/// Linear blend between two colors.
pub fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
