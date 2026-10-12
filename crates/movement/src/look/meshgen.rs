//! Procedural mesh building for the demo's characters, props and architecture.
//!
//! [`MeshBuilder`] collects vertices and triangles (counter-clockwise front faces, +Y up,
//! meters). Primitives come out with normals and UVs; combine them with
//! [`MeshBuilder::append`] under a transform and finish with [`MeshBuilder::build`].
//!
//! - [`MeshBuilder::rounded_box`]: a box with smoothly rounded edges (armor plates, panels).
//! - [`MeshBuilder::chamfer_box`]: a box with flat 45° bevels (concrete, machined steel).
//! - [`MeshBuilder::loft`]: superellipse cross-sections along a path (limbs, torsos, helmets,
//!   blades): exponent 2 is an ellipse, higher is squarer.
//! - [`MeshBuilder::lathe`]: a profile spun around +Y (lanterns, posts, pommels).
//! - [`MeshBuilder::extrude`]: a 2D outline pushed along +Z with optional bevel (trims,
//!   sword guards, signs).
//! - [`MeshBuilder::tube`]: a round pipe along a path (railings, cables).
//!
//! UVs are in meters unless a primitive says otherwise, so a tiling material's `uv_scale` is
//! "repeats per meter". [`MeshBuilder::planar_uvs`] re-projects UVs per face (box mapping)
//! for anything built by hand. Vertex colors multiply the material's base color; use
//! [`MeshBuilder::shade`] for baked cavity darkening or color variation.

use engine_core::math::{Mat3, Mat4, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

/// One cross-section of a [`MeshBuilder::loft`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    /// Center of the section.
    pub center: Vec3,
    /// Half extents along the section's local X (`right`) and Y (`up`).
    pub half: Vec2,
    /// Superellipse exponent: 2 = ellipse, 4 = rounded rectangle, 8+ = nearly a box.
    pub exponent: f32,
    /// Local X axis of the section (perpendicular to the path); `Vec3::ZERO` = automatic.
    pub right: Vec3,
    /// Offset of the section's shape center from `center` in local (right, up) coordinates
    /// (for asymmetric shapes like a chest that bulges forward).
    pub offset: Vec2,
}

impl Section {
    /// An elliptical section.
    pub fn ellipse(center: Vec3, half: Vec2) -> Self {
        Self {
            center,
            half,
            exponent: 2.0,
            right: Vec3::ZERO,
            offset: Vec2::ZERO,
        }
    }

    /// A rounded-rectangle section (exponent 4).
    pub fn rounded(center: Vec3, half: Vec2) -> Self {
        Self {
            exponent: 4.0,
            ..Self::ellipse(center, half)
        }
    }

    pub fn with_exponent(mut self, exponent: f32) -> Self {
        self.exponent = exponent;
        self
    }

    pub fn with_right(mut self, right: Vec3) -> Self {
        self.right = right;
        self
    }

    pub fn with_offset(mut self, offset: Vec2) -> Self {
        self.offset = offset;
        self
    }
}

/// How a loft's ends are closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// Left open.
    None,
    /// A flat fan.
    Flat,
    /// Rounded inward (a dome that shrinks the last section to a point over a few rings).
    Round,
}

/// Collects vertices and triangles; see the module docs.
#[derive(Debug, Clone, Default)]
pub struct MeshBuilder {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Adds a vertex and returns its index.
    pub fn vertex(&mut self, position: Vec3, normal: Vec3, uv: Vec2) -> u32 {
        self.vertices.push(Vertex {
            position: position.to_array(),
            normal: normal.normalize_or(Vec3::Y).to_array(),
            uv: uv.to_array(),
            color: WHITE,
        });
        (self.vertices.len() - 1) as u32
    }

    /// Adds a triangle (counter-clockwise seen from the front).
    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    /// Adds a quad `a b c d` (counter-clockwise seen from the front).
    pub fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// A flat quad with its own vertices; the normal follows the winding.
    pub fn flat_quad(&mut self, p: [Vec3; 4], uv: [Vec2; 4]) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let i: Vec<u32> = (0..4).map(|k| self.vertex(p[k], n, uv[k])).collect();
        self.quad(i[0], i[1], i[2], i[3]);
    }

    /// Appends `other` moved by `transform` (normals follow, also under non-uniform scale).
    pub fn append(&mut self, other: &MeshBuilder, transform: Mat4) -> &mut Self {
        let base = self.vertices.len() as u32;
        let normal_m = Mat3::from_mat4(transform).inverse().transpose();
        let flip = Mat3::from_mat4(transform).determinant() < 0.0;
        for v in &other.vertices {
            let p = transform.transform_point3(Vec3::from(v.position));
            let n = (normal_m * Vec3::from(v.normal)).normalize_or(Vec3::Y);
            self.vertices.push(Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                ..*v
            });
        }
        for t in other.indices.as_chunks::<3>().0 {
            if flip {
                self.indices
                    .extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
            } else {
                self.indices
                    .extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
            }
        }
        self
    }

    /// `self` moved by `transform` (a copy).
    pub fn transformed(&self, transform: Mat4) -> Self {
        let mut m = MeshBuilder::new();
        m.append(self, transform);
        m
    }

    /// Multiplies every vertex color by `rgba` (linear).
    pub fn tint(&mut self, rgba: Vec4) -> &mut Self {
        for v in &mut self.vertices {
            v.color = (Vec4::from(v.color) * rgba).to_array();
        }
        self
    }

    /// Multiplies each vertex color's RGB by `f(position, normal)` (baked cavity, dirt,
    /// gradients).
    pub fn shade(&mut self, f: impl Fn(Vec3, Vec3) -> f32) -> &mut Self {
        for v in &mut self.vertices {
            let k = f(Vec3::from(v.position), Vec3::from(v.normal));
            v.color[0] *= k;
            v.color[1] *= k;
            v.color[2] *= k;
        }
        self
    }

    /// Re-projects UVs per vertex from the dominant normal axis (box mapping), `scale` UV
    /// units per meter. Works best on flat-shaded meshes.
    pub fn planar_uvs(&mut self, scale: f32) -> &mut Self {
        for v in &mut self.vertices {
            let p = Vec3::from(v.position) * scale;
            let n = Vec3::from(v.normal).abs();
            v.uv = if n.x >= n.y && n.x >= n.z {
                [p.z, p.y]
            } else if n.y >= n.z {
                [p.x, p.z]
            } else {
                [p.x, p.y]
            };
        }
        self
    }

    /// Recomputes smooth normals by averaging face normals per vertex (shared vertices only).
    pub fn smooth_normals(&mut self) -> &mut Self {
        let mut acc = vec![Vec3::ZERO; self.vertices.len()];
        for t in self.indices.as_chunks::<3>().0 {
            let [a, b, c] =
                [t[0], t[1], t[2]].map(|i| Vec3::from(self.vertices[i as usize].position));
            let n = (b - a).cross(c - a); // area-weighted
            for &i in t {
                acc[i as usize] += n;
            }
        }
        for (v, n) in self.vertices.iter_mut().zip(acc) {
            v.normal = n.normalize_or(Vec3::Y).to_array();
        }
        self
    }

    /// Splits every triangle into its own vertices with the face normal (hard edges).
    pub fn flat_shaded(&self) -> Self {
        let mut m = MeshBuilder::new();
        for t in self.indices.as_chunks::<3>().0 {
            let v = [t[0], t[1], t[2]].map(|i| self.vertices[i as usize]);
            let p = v.map(|v| Vec3::from(v.position));
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
            let base = m.vertices.len() as u32;
            for vv in v {
                m.vertices.push(Vertex {
                    normal: n.to_array(),
                    ..vv
                });
            }
            m.tri(base, base + 1, base + 2);
        }
        m
    }

    /// Axis-aligned bounds (min, max).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for v in &self.vertices {
            lo = lo.min(Vec3::from(v.position));
            hi = hi.max(Vec3::from(v.position));
        }
        (lo, hi)
    }

    pub fn build(self) -> Mesh {
        Mesh::new(self.vertices, self.indices)
    }

    // ---------------------------------------------------------------- primitives

    /// A box of full `size` centered on the origin whose edges and corners are rounded with
    /// `radius` (clamped to half the smallest side), `segments` steps per rounded edge.
    /// Smooth normals; UVs are box-mapped in meters.
    pub fn rounded_box(size: Vec3, radius: f32, segments: u32) -> Self {
        let half = size * 0.5;
        let r = radius.clamp(0.0, half.min_element());
        let inner = half - Vec3::splat(r);
        let n = segments.max(1);
        // Sample positions along one axis: dense over the rounded ends (even angle steps),
        // one span over the flat middle. Neighboring faces sample their shared edge the same
        // way, so the seams match exactly.
        let samples = |h: f32, i: f32| -> Vec<f32> {
            let mut out = Vec::new();
            for k in (0..=n).rev() {
                let t = (k as f32 / n as f32 * std::f32::consts::FRAC_PI_4).tan();
                out.push(-(i + (h - i) * t));
            }
            for k in 0..=n {
                let t = (k as f32 / n as f32 * std::f32::consts::FRAC_PI_4).tan();
                out.push(i + (h - i) * t);
            }
            out
        };
        let faces: [(Vec3, Vec3, Vec3); 6] = [
            (Vec3::X, Vec3::NEG_Z, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        ];
        let mut m = MeshBuilder::new();
        for (normal, u_axis, v_axis) in faces {
            let ext = |a: Vec3| a.abs().dot(half);
            let inn = |a: Vec3| a.abs().dot(inner);
            let us = samples(ext(u_axis), inn(u_axis));
            let vs = samples(ext(v_axis), inn(v_axis));
            let base = m.vertices.len() as u32;
            for &b in &vs {
                for &a in &us {
                    // Point on the sharp box, pulled onto the rounded surface.
                    let q = normal * ext(normal) + u_axis * a + v_axis * b;
                    let core = q.clamp(-inner, inner);
                    let dir = (q - core).normalize_or(normal);
                    let p = core + dir * r;
                    m.vertex(p, dir, Vec2::new(p.dot(u_axis), p.dot(v_axis)));
                }
            }
            let row = us.len() as u32;
            for iv in 0..vs.len() as u32 - 1 {
                for iu in 0..row - 1 {
                    let a = base + iv * row + iu;
                    m.quad(a, a + 1, a + row + 1, a + row);
                }
            }
        }
        m.remove_degenerate();
        m
    }

    /// A box of full `size` centered on the origin with flat 45° bevels of width `bevel` on
    /// every edge (hard edges, catches highlights like machined or cast parts). UVs are
    /// box-mapped in meters.
    pub fn chamfer_box(size: Vec3, bevel: f32) -> Self {
        let mut m = Self::rounded_box(size, bevel, 1).flat_shaded();
        m.planar_uvs(1.0);
        m
    }

    /// Superellipse cross-sections along a path, `ring` vertices around each section, with
    /// the chosen caps. The path runs from the first section to the last; each section's
    /// plane is perpendicular to the path (averaged at joints). V runs along the path in
    /// meters, U around the section (0..1 × perimeter in meters). Smooth normals.
    pub fn loft(sections: &[Section], ring: u32, start: Cap, end: Cap) -> Self {
        let mut m = MeshBuilder::new();
        let ring = ring.max(3) as usize;
        if sections.len() < 2 {
            return m;
        }
        // Expand round caps into extra sections.
        let mut secs = sections.to_vec();
        let dome = |s: Section, dir: Vec3| -> Vec<Section> {
            (1..=4)
                .map(|k| {
                    let a = k as f32 / 4.0 * std::f32::consts::FRAC_PI_2;
                    let reach = s.half.min_element().min(s.half.max_element() * 0.6);
                    Section {
                        center: s.center + dir * reach * a.sin(),
                        half: s.half * a.cos().max(0.02),
                        ..s
                    }
                })
                .collect()
        };
        if end == Cap::Round {
            let n = secs.len();
            let dir = (secs[n - 1].center - secs[n - 2].center).normalize_or(Vec3::Y);
            let extra = dome(secs[n - 1], dir);
            secs.extend(extra);
        }
        if start == Cap::Round {
            let dir = (secs[0].center - secs[1].center).normalize_or(Vec3::NEG_Y);
            let mut extra = dome(secs[0], dir);
            extra.reverse();
            extra.extend(secs);
            secs = extra;
        }
        let n = secs.len();
        // Frames.
        let tangents: Vec<Vec3> = (0..n)
            .map(|i| {
                let a = secs[i.saturating_sub(1)].center;
                let b = secs[(i + 1).min(n - 1)].center;
                (b - a).normalize_or(Vec3::Y)
            })
            .collect();
        let mut prev_right = {
            let t = tangents[0];
            let guess = if secs[0].right != Vec3::ZERO {
                secs[0].right
            } else if t.dot(Vec3::X).abs() < 0.9 {
                Vec3::X
            } else {
                Vec3::Z
            };
            (guess - t * guess.dot(t)).normalize_or(Vec3::X)
        };
        let mut v_dist = 0.0;
        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            let t = tangents[i];
            let guess = if secs[i].right != Vec3::ZERO {
                secs[i].right
            } else {
                prev_right
            };
            let right = (guess - t * guess.dot(t)).normalize_or(prev_right);
            let up = t.cross(right).normalize_or(Vec3::Y);
            prev_right = right;
            if i > 0 {
                v_dist += secs[i].center.distance(secs[i - 1].center);
            }
            let s = secs[i];
            let base = m.vertices.len() as u32;
            for k in 0..=ring {
                let a = k as f32 / ring as f32 * std::f32::consts::TAU;
                let (sn, cs) = a.sin_cos();
                let e = 2.0 / s.exponent.max(0.5);
                let x = cs.signum() * cs.abs().powf(e) * s.half.x + s.offset.x;
                let y = sn.signum() * sn.abs().powf(e) * s.half.y + s.offset.y;
                let p = s.center + right * x + up * y;
                let perim = (s.half.x + s.half.y) * std::f32::consts::PI;
                m.vertex(
                    p,
                    right * x + up * y,
                    Vec2::new(k as f32 / ring as f32 * perim, v_dist),
                );
            }
            rows.push(base);
        }
        let row = (ring + 1) as u32;
        for i in 0..n - 1 {
            for k in 0..ring as u32 {
                let a = rows[i] + k;
                let b = rows[i + 1] + k;
                m.quad(a, a + 1, b + 1, b);
            }
        }
        m.smooth_normals();
        // Seam: average the duplicated first/last ring vertices' normals.
        for &r in &rows {
            let a = r as usize;
            let b = (r + row - 1) as usize;
            let nrm = (Vec3::from(m.vertices[a].normal) + Vec3::from(m.vertices[b].normal))
                .normalize_or(Vec3::Y)
                .to_array();
            m.vertices[a].normal = nrm;
            m.vertices[b].normal = nrm;
        }
        let cap = |m: &mut MeshBuilder, r: u32, t: Vec3, center: Vec3, flip: bool| {
            let c = m.vertex(center, t, Vec2::ZERO);
            let first = m.vertices.len() as u32;
            for k in 0..ring as u32 {
                let p = Vec3::from(m.vertices[(r + k) as usize].position);
                m.vertex(p, t, (p - center).truncate());
            }
            for k in 0..ring as u32 {
                let a = first + k;
                let b = first + (k + 1) % ring as u32;
                if flip {
                    m.tri(c, b, a);
                } else {
                    m.tri(c, a, b);
                }
            }
        };
        if start == Cap::Flat {
            cap(&mut m, rows[0], -tangents[0], secs[0].center, true);
        }
        if end == Cap::Flat {
            cap(
                &mut m,
                rows[n - 1],
                tangents[n - 1],
                secs[n - 1].center,
                false,
            );
        }
        m.remove_degenerate();
        m
    }

    /// Spins `profile` points (x = radius, y = height) around +Y in `segments` steps.
    /// Smooth normals; U around (meters of circumference at radius 1), V along the profile.
    pub fn lathe(profile: &[Vec2], segments: u32) -> Self {
        let sections: Vec<Section> = profile
            .iter()
            .map(|p| {
                Section::ellipse(Vec3::new(0.0, p.y, 0.0), Vec2::splat(p.x.max(1e-4)))
                    .with_right(Vec3::X)
            })
            .collect();
        Self::loft(&sections, segments, Cap::Flat, Cap::Flat)
    }

    /// Pushes a closed 2D `outline` (counter-clockwise in the XY plane) from z = 0 to
    /// z = `depth`, with a flat chamfer of `bevel` on both faces. Hard edges; front and back
    /// faces are triangulated as a fan around the centroid, so keep outlines star-shaped
    /// around their centroid. UVs in meters.
    pub fn extrude(outline: &[Vec2], depth: f32, bevel: f32) -> Self {
        let mut m = MeshBuilder::new();
        let n = outline.len();
        if n < 3 {
            return m;
        }
        let centroid = outline.iter().copied().sum::<Vec2>() / n as f32;
        let b = bevel.clamp(0.0, depth * 0.45);
        // Inset outline for the bevel.
        let inset: Vec<Vec2> = (0..n)
            .map(|i| {
                let p = outline[i];
                let prev = outline[(i + n - 1) % n];
                let next = outline[(i + 1) % n];
                let e1 = (p - prev).normalize_or(Vec2::X);
                let e2 = (next - p).normalize_or(Vec2::X);
                let n1 = Vec2::new(e1.y, -e1.x);
                let n2 = Vec2::new(e2.y, -e2.x);
                let bis = (n1 + n2).normalize_or(n1);
                let k = bis.dot(n1).max(0.3);
                p - bis * (b / k)
            })
            .collect();
        let rings: Vec<(f32, Vec<Vec2>)> = if b > 0.0 {
            vec![
                (0.0, inset.clone()),
                (b, outline.to_vec()),
                (depth - b, outline.to_vec()),
                (depth, inset),
            ]
        } else {
            vec![(0.0, outline.to_vec()), (depth, outline.to_vec())]
        };
        for w in rings.windows(2) {
            let (z0, a) = (w[0].0, &w[0].1);
            let (z1, c) = (w[1].0, &w[1].1);
            for i in 0..n {
                let j = (i + 1) % n;
                let p = [
                    a[i].extend(z0),
                    a[j].extend(z0),
                    c[j].extend(z1),
                    c[i].extend(z1),
                ];
                let along = (outline[j] - outline[i]).length();
                m.flat_quad(
                    [p[0], p[1], p[2], p[3]],
                    [
                        Vec2::new(0.0, z0),
                        Vec2::new(along, z0),
                        Vec2::new(along, z1),
                        Vec2::new(0.0, z1),
                    ],
                );
            }
        }
        // Faces (fans).
        let (front, back) = (&rings[0].1, &rings[rings.len() - 1].1);
        let c0 = m.vertex(centroid.extend(0.0), Vec3::NEG_Z, centroid);
        let first0 = m.vertices.len() as u32;
        for p in front {
            m.vertex(p.extend(0.0), Vec3::NEG_Z, *p);
        }
        for i in 0..n as u32 {
            m.tri(c0, first0 + (i + 1) % n as u32, first0 + i);
        }
        let c1 = m.vertex(centroid.extend(depth), Vec3::Z, centroid);
        let first1 = m.vertices.len() as u32;
        for p in back {
            m.vertex(p.extend(depth), Vec3::Z, *p);
        }
        for i in 0..n as u32 {
            m.tri(c1, first1 + i, first1 + (i + 1) % n as u32);
        }
        // Make side windings face outward whatever the outline's orientation.
        let area: f32 = (0..n)
            .map(|i| outline[i].perp_dot(outline[(i + 1) % n]))
            .sum();
        if area < 0.0 {
            m.flip();
        }
        m
    }

    /// A round tube of `radius` along `path` (smooth), `segments` around, flat end caps.
    pub fn tube(path: &[Vec3], radius: f32, segments: u32) -> Self {
        let sections: Vec<Section> = path
            .iter()
            .map(|&p| Section::ellipse(p, Vec2::splat(radius)))
            .collect();
        Self::loft(&sections, segments, Cap::Flat, Cap::Flat)
    }

    /// Reverses every triangle's winding and normal.
    pub fn flip(&mut self) -> &mut Self {
        for t in self.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        for v in &mut self.vertices {
            v.normal = (-Vec3::from(v.normal)).to_array();
        }
        self
    }

    /// Drops zero-area triangles (left by collapsed rounding rows and domes).
    pub fn remove_degenerate(&mut self) -> &mut Self {
        let verts = &self.vertices;
        let mut out = Vec::with_capacity(self.indices.len());
        for t in self.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(verts[i as usize].position));
            if (b - a).cross(c - a).length_squared() > 1e-14 {
                out.extend_from_slice(t);
            }
        }
        self.indices = out;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(m: &MeshBuilder) -> bool {
        // Every edge (by position) is shared by an even number of triangles.
        use std::collections::HashMap;
        let key = |v: &Vertex| {
            let q = |x: f32| (x * 1e4).round() as i64;
            (q(v.position[0]), q(v.position[1]), q(v.position[2]))
        };
        type Key = (i64, i64, i64);
        let mut edges: HashMap<(Key, Key), i32> = HashMap::new();
        for t in m.indices.as_chunks::<3>().0 {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (ka, kb) = (key(&m.vertices[a as usize]), key(&m.vertices[b as usize]));
                let k = if ka < kb { (ka, kb) } else { (kb, ka) };
                *edges.entry(k).or_default() += 1;
            }
        }
        edges.values().all(|c| c % 2 == 0)
    }

    fn outward(m: &MeshBuilder) -> bool {
        // Signed volume is positive for outward-facing triangles.
        let mut vol = 0.0;
        for t in m.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].position));
            vol += a.dot(b.cross(c)) / 6.0;
        }
        vol > 0.0
    }

    #[test]
    fn rounded_box_is_closed_with_the_right_size() {
        let m = MeshBuilder::rounded_box(Vec3::new(2.0, 1.0, 0.5), 0.1, 3);
        assert!(closed(&m));
        assert!(outward(&m));
        let (lo, hi) = m.bounds();
        assert!((hi - lo - Vec3::new(2.0, 1.0, 0.5)).abs().max_element() < 1e-4);
        for v in &m.vertices {
            assert!((Vec3::from(v.normal).length() - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn chamfer_box_is_closed() {
        let m = MeshBuilder::chamfer_box(Vec3::new(1.0, 2.0, 3.0), 0.05);
        assert!(closed(&m));
        assert!(outward(&m));
    }

    #[test]
    fn lofts_and_lathes_face_outward() {
        let m = MeshBuilder::loft(
            &[
                Section::ellipse(Vec3::ZERO, Vec2::new(0.2, 0.1)),
                Section::rounded(Vec3::new(0.0, 0.5, 0.0), Vec2::new(0.15, 0.1)),
                Section::ellipse(Vec3::new(0.1, 1.0, 0.0), Vec2::new(0.1, 0.1)),
            ],
            16,
            Cap::Flat,
            Cap::Round,
        );
        assert!(outward(&m), "loft");
        let l = MeshBuilder::lathe(
            &[
                Vec2::new(0.3, 0.0),
                Vec2::new(0.35, 0.5),
                Vec2::new(0.1, 1.0),
            ],
            24,
        );
        assert!(outward(&l), "lathe");
    }

    #[test]
    fn extrusions_face_outward_either_winding() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let a = MeshBuilder::extrude(&square, 0.2, 0.02);
        assert!(outward(&a));
        let mut rev = square;
        rev.reverse();
        let b = MeshBuilder::extrude(&rev, 0.2, 0.0);
        assert!(outward(&b));
    }

    #[test]
    fn append_flips_mirrored_parts() {
        let part = MeshBuilder::chamfer_box(Vec3::ONE, 0.1);
        let mut m = MeshBuilder::new();
        m.append(&part, Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        assert!(outward(&m));
    }
}
