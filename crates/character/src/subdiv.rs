//! Catmull-Clark subdivision of quad meshes with any number of per-vertex attributes.
//!
//! The base mesh is a few thousand quads on the head; two levels of subdivision turn its
//! polygons into the smooth limit surface a close-up needs (the same smoothing MakeHuman and
//! sculpting tools apply). Every attribute (position, rest position, region masks) is
//! subdivided with the same weights, so masks stay smooth and patterns stay stuck to the skin.
//! Open edges (the neckline of a bust) use the boundary rules, so they stay put instead of
//! shrinking.

use std::collections::HashMap;

/// A quad mesh with `stride` floats per vertex (the first three are the position).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadMesh {
    pub quads: Vec<[u32; 4]>,
    pub attrs: Vec<f32>,
    pub stride: usize,
}

impl QuadMesh {
    pub fn vertex_count(&self) -> usize {
        self.attrs.len().checked_div(self.stride).unwrap_or(0)
    }

    pub fn vertex(&self, i: usize) -> &[f32] {
        &self.attrs[i * self.stride..(i + 1) * self.stride]
    }

    /// One level of Catmull-Clark subdivision: four quads for every quad.
    pub fn subdivide(&self) -> QuadMesh {
        let s = self.stride;
        let nv = self.vertex_count();
        let nf = self.quads.len();
        // Edges with the faces on either side.
        let mut edge_index: HashMap<(u32, u32), u32> = HashMap::with_capacity(nf * 2);
        let mut edges: Vec<([u32; 2], [u32; 2], u8)> = Vec::with_capacity(nf * 2);
        let mut face_edges: Vec<[u32; 4]> = Vec::with_capacity(nf);
        for (fi, q) in self.quads.iter().enumerate() {
            let mut fe = [0u32; 4];
            for k in 0..4 {
                let (a, b) = (q[k], q[(k + 1) % 4]);
                let key = (a.min(b), a.max(b));
                let e = *edge_index.entry(key).or_insert_with(|| {
                    edges.push(([key.0, key.1], [u32::MAX; 2], 0));
                    (edges.len() - 1) as u32
                });
                let entry = &mut edges[e as usize];
                if entry.2 < 2 {
                    entry.1[entry.2 as usize] = fi as u32;
                }
                entry.2 = entry.2.saturating_add(1);
                fe[k] = e;
            }
            face_edges.push(fe);
        }
        let ne = edges.len();
        let mut out = vec![0.0f32; (nv + ne + nf) * s];
        // Face points.
        let fbase = nv + ne;
        for (fi, q) in self.quads.iter().enumerate() {
            let dst = (fbase + fi) * s;
            for &v in q {
                let src = self.vertex(v as usize);
                for c in 0..s {
                    out[dst + c] += src[c] * 0.25;
                }
            }
        }
        // Edge points; boundary edges (and their vertices) are marked.
        let mut boundary_vertex = vec![false; nv];
        let mut vertex_faces = vec![0u32; nv];
        let mut vertex_edges = vec![0u32; nv];
        let mut boundary_neighbors: Vec<[u32; 2]> = vec![[u32::MAX; 2]; nv];
        let mut boundary_count = vec![0u8; nv];
        for (ei, (vs, fs, count)) in edges.iter().enumerate() {
            let dst = (nv + ei) * s;
            let (a, b) = (self.vertex(vs[0] as usize), self.vertex(vs[1] as usize));
            if *count == 2 {
                let (fa, fb) = (fs[0] as usize, fs[1] as usize);
                for c in 0..s {
                    let f = out[(fbase + fa) * s + c] + out[(fbase + fb) * s + c];
                    out[dst + c] = (a[c] + b[c] + f) * 0.25;
                }
            } else {
                for c in 0..s {
                    out[dst + c] = (a[c] + b[c]) * 0.5;
                }
                for (v, other) in [(vs[0], vs[1]), (vs[1], vs[0])] {
                    let v = v as usize;
                    boundary_vertex[v] = true;
                    if (boundary_count[v] as usize) < 2 {
                        boundary_neighbors[v][boundary_count[v] as usize] = other;
                    }
                    boundary_count[v] = boundary_count[v].saturating_add(1);
                }
            }
            for v in vs {
                vertex_edges[*v as usize] += 1;
            }
        }
        // Sums for interior vertex points: face points (Q) and edge midpoints (R).
        let mut q_sum = vec![0.0f32; nv * s];
        let mut r_sum = vec![0.0f32; nv * s];
        for (fi, q) in self.quads.iter().enumerate() {
            for &v in q {
                let v = v as usize;
                vertex_faces[v] += 1;
                for c in 0..s {
                    q_sum[v * s + c] += out[(fbase + fi) * s + c];
                }
            }
        }
        for (vs, _, _) in &edges {
            let (a, b) = (vs[0] as usize, vs[1] as usize);
            for c in 0..s {
                let mid = (self.attrs[a * s + c] + self.attrs[b * s + c]) * 0.5;
                r_sum[a * s + c] += mid;
                r_sum[b * s + c] += mid;
            }
        }
        for v in 0..nv {
            let src = self.vertex(v);
            let dst = v * s;
            if boundary_vertex[v] {
                if boundary_count[v] == 2 {
                    let [a, b] = boundary_neighbors[v];
                    let (a, b) = (self.vertex(a as usize), self.vertex(b as usize));
                    for c in 0..s {
                        out[dst + c] = (a[c] + 6.0 * src[c] + b[c]) / 8.0;
                    }
                } else {
                    out[dst..dst + s].copy_from_slice(src);
                }
                continue;
            }
            let n = vertex_faces[v] as f32;
            if n < 3.0 || vertex_edges[v] as f32 != n {
                out[dst..dst + s].copy_from_slice(src);
                continue;
            }
            for c in 0..s {
                let q = q_sum[dst + c] / n;
                let r = r_sum[dst + c] / n;
                out[dst + c] = (q + 2.0 * r + (n - 3.0) * src[c]) / n;
            }
        }
        let mut quads = Vec::with_capacity(nf * 4);
        for (fi, q) in self.quads.iter().enumerate() {
            let fe = face_edges[fi];
            let f = (fbase + fi) as u32;
            for k in 0..4 {
                let prev = (k + 3) % 4;
                quads.push([q[k], (nv as u32) + fe[k], f, (nv as u32) + fe[prev]]);
            }
        }
        QuadMesh {
            quads,
            attrs: out,
            stride: s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube of six quads, outward winding.
    fn cube() -> QuadMesh {
        let p = [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        QuadMesh {
            quads: vec![
                [0, 3, 2, 1],
                [4, 5, 6, 7],
                [0, 1, 5, 4],
                [2, 3, 7, 6],
                [1, 2, 6, 5],
                [0, 4, 7, 3],
            ],
            attrs: p.iter().flatten().copied().collect(),
            stride: 3,
        }
    }

    #[test]
    fn a_cube_rounds_toward_a_sphere() {
        let mut m = cube();
        for _ in 0..3 {
            m = m.subdivide();
        }
        assert_eq!(m.quads.len(), 6 * 64);
        let radii: Vec<f32> = (0..m.vertex_count())
            .map(|i| {
                let v = m.vertex(i);
                (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
            })
            .collect();
        let lo = radii.iter().copied().fold(f32::MAX, f32::min);
        let hi = radii.iter().copied().fold(0.0, f32::max);
        // Converges to a rounded cube well inside the original corners (radius 1.73).
        assert!(hi < 1.0 && lo > 0.5 && hi / lo < 1.35, "{lo} .. {hi}");
    }

    #[test]
    fn open_edges_stay_in_their_plane() {
        // A 2x2 grid in the XY plane: its border must stay at z = 0 and inside the square.
        let mut attrs = Vec::new();
        for y in 0..3 {
            for x in 0..3 {
                attrs.extend([x as f32, y as f32, 0.0, (x + y) as f32]);
            }
        }
        let m = QuadMesh {
            quads: vec![[0, 1, 4, 3], [1, 2, 5, 4], [3, 4, 7, 6], [4, 5, 8, 7]],
            attrs,
            stride: 4,
        };
        let d = m.subdivide().subdivide();
        for i in 0..d.vertex_count() {
            let v = d.vertex(i);
            assert!(v[2].abs() < 1e-6);
            assert!((-1e-6..=2.0 + 1e-6).contains(&v[0]));
            // A linear attribute stays linear in the interior and on the border.
            assert!((v[3] - (v[0] + v[1])).abs() < 1e-4, "{v:?}");
        }
    }
}
