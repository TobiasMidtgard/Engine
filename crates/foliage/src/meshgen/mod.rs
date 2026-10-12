//! Procedural plant meshes and textures.
//!
//! Every plant is built from a few primitives: tapered tubes along curved paths (trunks,
//! branches, stems), two-sided leaf cards (alpha-masked sprays of leaves or needles), blades
//! (grass) and lathed caps (mushrooms). Colors are baked into vertex colors (bark, cut wood,
//! ambient occlusion inside a crown) so one material per part covers every plant of a species.
//!
//! Leaf cards use "spherical" normals: each card is shaded as if it were part of the crown's
//! rounded surface, which gives soft, volumetric canopies instead of a mess of flat planes.

pub mod small;
pub mod textures;
pub mod tree;

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

/// Collects vertices and triangles.
#[derive(Default)]
pub struct MeshBuilder {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2, color: Vec4) -> u32 {
        self.vertices
            .push(Vertex::new(p, n.normalize_or(Vec3::Y), uv).with_color(color));
        self.vertices.len() as u32 - 1
    }

    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// A tube along `path` with a radius per point. `color(i, around)` gives the vertex color of
    /// ring `i` (around = 0..1). Caps close the start and/or end.
    #[allow(clippy::too_many_arguments)]
    pub fn tube(
        &mut self,
        path: &[Vec3],
        radius: &[f32],
        sides: u32,
        v_scale: f32,
        color: &dyn Fn(usize, f32) -> Vec4,
        cap_start: Option<&dyn Fn(f32) -> Vec4>,
        cap_end: Option<&dyn Fn(f32) -> Vec4>,
    ) {
        if path.len() < 2 {
            return;
        }
        let sides = sides.max(3);
        let frames = frames(path);
        let mut v = 0.0f32;
        let first = self.vertices.len() as u32;
        for (i, &p) in path.iter().enumerate() {
            if i > 0 {
                v += path[i].distance(path[i - 1]) * v_scale;
            }
            let (_, n, b) = frames[i];
            for k in 0..=sides {
                let a = k as f32 / sides as f32 * std::f32::consts::TAU;
                let dir = n * a.cos() + b * a.sin();
                self.vertex(
                    p + dir * radius[i],
                    dir,
                    Vec2::new(k as f32 / sides as f32, v),
                    color(i, k as f32 / sides as f32),
                );
            }
        }
        let ring = sides + 1;
        for i in 0..path.len() as u32 - 1 {
            for k in 0..sides {
                let a = first + i * ring + k;
                let b = a + 1;
                let c = a + ring + 1;
                let d = a + ring;
                self.tri(a, b, c);
                self.tri(a, c, d);
            }
        }
        if let Some(cap) = cap_start {
            let (t, n, b) = frames[0];
            self.disc(path[0], -t, n, b, radius[0], sides, cap, true);
        }
        if let Some(cap) = cap_end {
            let last = path.len() - 1;
            let (t, n, b) = frames[last];
            self.disc(path[last], t, n, b, radius[last], sides, cap, false);
        }
    }

    /// A flat disc made of concentric rings (so a cut trunk can show growth rings).
    /// `color(r)` gives the color at relative radius r (0 center .. 1 rim).
    #[allow(clippy::too_many_arguments)]
    pub fn disc(
        &mut self,
        center: Vec3,
        normal: Vec3,
        n: Vec3,
        b: Vec3,
        radius: f32,
        sides: u32,
        color: &dyn Fn(f32) -> Vec4,
        flip: bool,
    ) {
        const RINGS: [f32; 6] = [0.0, 0.18, 0.4, 0.62, 0.86, 1.0];
        let c = self.vertex(center, normal, Vec2::splat(0.5), color(0.0));
        let mut prev: Vec<u32> = Vec::new();
        for (ri, &r) in RINGS.iter().enumerate().skip(1) {
            let ring: Vec<u32> = (0..sides)
                .map(|k| {
                    let a = k as f32 / sides as f32 * std::f32::consts::TAU;
                    let dir = n * a.cos() + b * a.sin();
                    // Slightly uneven rings look sawn rather than machined.
                    let wobble = 1.0 + 0.04 * ((a * 3.0 + ri as f32).sin());
                    self.vertex(
                        center + dir * radius * r * wobble.min(1.0 / r.max(0.01)),
                        normal,
                        Vec2::new(0.5 + 0.5 * a.cos() * r, 0.5 + 0.5 * a.sin() * r),
                        color(r),
                    )
                })
                .collect();
            for k in 0..sides as usize {
                let k1 = (k + 1) % sides as usize;
                if prev.is_empty() {
                    if flip {
                        self.tri(c, ring[k1], ring[k]);
                    } else {
                        self.tri(c, ring[k], ring[k1]);
                    }
                } else if flip {
                    self.tri(prev[k], ring[k1], ring[k]);
                    self.tri(prev[k], prev[k1], ring[k1]);
                } else {
                    self.tri(prev[k], ring[k], ring[k1]);
                    self.tri(prev[k], ring[k1], prev[k1]);
                }
            }
            prev = ring;
        }
    }

    /// A two-sided card: `corners` counter-clockwise seen from the front. Both faces use
    /// `normal` (the crown's normal, not the card's), so cards shade like a rounded crown.
    pub fn card(&mut self, corners: [Vec3; 4], uvs: [Vec2; 4], normal: Vec3, colors: [Vec4; 4]) {
        let base = self.vertices.len() as u32;
        for i in 0..4 {
            self.vertex(corners[i], normal, uvs[i], colors[i]);
        }
        self.tri(base, base + 1, base + 2);
        self.tri(base, base + 2, base + 3);
        self.tri(base, base + 2, base + 1);
        self.tri(base, base + 3, base + 2);
    }

    /// A two-sided strip along a path (fern fronds, grass blades, leaves): `left`/`right`
    /// edge points per row, `normals` per row, `v` per row.
    pub fn strip(
        &mut self,
        left: &[Vec3],
        right: &[Vec3],
        normals: &[Vec3],
        v: &[f32],
        colors: &[Vec4],
    ) {
        let base = self.vertices.len() as u32;
        for i in 0..left.len() {
            self.vertex(left[i], normals[i], Vec2::new(0.0, v[i]), colors[i]);
            self.vertex(right[i], normals[i], Vec2::new(1.0, v[i]), colors[i]);
        }
        for i in 0..left.len() as u32 - 1 {
            let a = base + i * 2;
            let (b, c, d) = (a + 1, a + 3, a + 2);
            // Front and back.
            self.tri(a, b, c);
            self.tri(a, c, d);
            self.tri(a, c, b);
            self.tri(a, d, c);
        }
    }

    pub fn finish(self) -> Mesh {
        Mesh::new(self.vertices, self.indices)
    }
}

/// Parallel-transport frames (tangent, normal, binormal) along a path: tubes don't twist.
pub fn frames(path: &[Vec3]) -> Vec<(Vec3, Vec3, Vec3)> {
    let n = path.len();
    let tangent = |i: usize| {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        (b - a).normalize_or(Vec3::Y)
    };
    let mut out = Vec::with_capacity(n);
    let t0 = tangent(0);
    let mut normal = t0.any_orthonormal_vector();
    for i in 0..n {
        let t = tangent(i);
        normal = (normal - t * normal.dot(t)).normalize_or(t.any_orthonormal_vector());
        let b = t.cross(normal);
        out.push((t, normal, b));
    }
    out
}

/// Quadratic Bézier point.
pub fn bezier(a: Vec3, b: Vec3, c: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    a * (u * u) + b * (2.0 * u * t) + c * (t * t)
}

/// Color as a vertex color.
pub fn rgb(c: [f32; 3], scale: f32) -> Vec4 {
    Vec4::new(c[0] * scale, c[1] * scale, c[2] * scale, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Front faces wind counter-clockwise: each triangle's geometric normal agrees with the
    /// vertex normals (for tubes) on the outside.
    #[test]
    fn tube_faces_outward() {
        let mut b = MeshBuilder::new();
        let path = [
            Vec3::ZERO,
            Vec3::new(0.1, 1.0, 0.0),
            Vec3::new(0.3, 2.0, 0.1),
        ];
        b.tube(
            &path,
            &[0.3, 0.2, 0.1],
            8,
            1.0,
            &|_, _| Vec4::ONE,
            Some(&|_| Vec4::ONE),
            Some(&|_| Vec4::ONE),
        );
        let m = b.finish();
        for tri in m.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
            let pa = Vec3::from(a.position);
            let geo = (Vec3::from(b.position) - pa).cross(Vec3::from(c.position) - pa);
            if geo.length() < 1e-8 {
                continue;
            }
            let n = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            assert!(geo.dot(n) > 0.0, "inward-facing triangle {tri:?}");
        }
    }

    #[test]
    fn frames_stay_orthonormal() {
        let path: Vec<Vec3> = (0..10)
            .map(|i| {
                Vec3::new(
                    (i as f32 * 0.7).sin(),
                    i as f32 * 0.5,
                    (i as f32 * 0.3).cos(),
                )
            })
            .collect();
        for (t, n, b) in frames(&path) {
            assert!(t.dot(n).abs() < 1e-4 && t.dot(b).abs() < 1e-4 && n.dot(b).abs() < 1e-4);
            assert!((n.length() - 1.0).abs() < 1e-4);
        }
    }
}
