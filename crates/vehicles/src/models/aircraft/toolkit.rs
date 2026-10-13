//! Tools the aircraft models are built with: hash noise, smooth curves, mesh building (grids,
//! lathes, tubes, rounded boxes, airfoils), texture baking (rasterize a part's UVs into a
//! position/normal buffer, paint every texel with a function of where it is on the part,
//! derive the normal map from painted heights, pad the islands), a stroke font for markings,
//! and the asset bundle a model registers.
//!
//! Everything is plain CPU code without dependencies beyond the engine crates, fast enough to
//! run at startup (the bakes split rows over the available cores).

use std::f32::consts::{PI, TAU};

use engine_assets::Assets;
use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::World;
use engine_render::{Image, Mesh, StandardMaterial, Vertex};

// ---------------------------------------------------------------- noise

/// 32-bit integer hash of a lattice point.
#[inline]
pub fn hash(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// Uniform 0..1 from a lattice point.
#[inline]
pub fn rand(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    (hash(x, y, z, seed) >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise in 0..1.
pub fn noise2(p: Vec2, seed: u32) -> f32 {
    let (fx, fy) = (p.x.floor(), p.y.floor());
    let (x, y) = (fx as i32, fy as i32);
    let (tx, ty) = (fade(p.x - fx), fade(p.y - fy));
    let a = rand(x, y, 0, seed);
    let b = rand(x + 1, y, 0, seed);
    let c = rand(x, y + 1, 0, seed);
    let d = rand(x + 1, y + 1, 0, seed);
    let ab = a + (b - a) * tx;
    let cd = c + (d - c) * tx;
    ab + (cd - ab) * ty
}

/// Smooth 3D value noise in 0..1.
pub fn noise3(p: Vec3, seed: u32) -> f32 {
    let f = p.floor();
    let (x, y, z) = (f.x as i32, f.y as i32, f.z as i32);
    let t = p - f;
    let (tx, ty, tz) = (fade(t.x), fade(t.y), fade(t.z));
    let l = |dx: i32, dy: i32, dz: i32| rand(x + dx, y + dy, z + dz, seed);
    let x00 = l(0, 0, 0) + (l(1, 0, 0) - l(0, 0, 0)) * tx;
    let x10 = l(0, 1, 0) + (l(1, 1, 0) - l(0, 1, 0)) * tx;
    let x01 = l(0, 0, 1) + (l(1, 0, 1) - l(0, 0, 1)) * tx;
    let x11 = l(0, 1, 1) + (l(1, 1, 1) - l(0, 1, 1)) * tx;
    let y0 = x00 + (x10 - x00) * ty;
    let y1 = x01 + (x11 - x01) * ty;
    y0 + (y1 - y0) * tz
}

/// Fractal 3D value noise (`octaves` doublings), 0..1.
pub fn fbm3(p: Vec3, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 0.5, 0.0, p);
    for o in 0..octaves {
        sum += noise3(q, seed.wrapping_add(o * 7919)) * amp;
        norm += amp;
        amp *= 0.5;
        q *= 2.03;
    }
    sum / norm
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Coverage of a line of half-width `half` at distance `d`, antialiased over `aa` meters.
#[inline]
pub fn line(d: f32, half: f32, aa: f32) -> f32 {
    (0.5 - (d.abs() - half) / aa.max(1e-6)).clamp(0.0, 1.0)
}

/// sRGB hex to linear RGB.
pub fn hex(rgb: u32) -> Vec3 {
    Vec3::from(engine_render::material::srgb_hex_to_linear(rgb))
}

// ---------------------------------------------------------------- curves

/// A smooth monotone curve through points (Fritsch–Carlson cubic Hermite): no overshoot,
/// so profiles drawn with it never bulge past their control points.
#[derive(Debug, Clone)]
pub struct Curve {
    xs: Vec<f32>,
    ys: Vec<f32>,
    ms: Vec<f32>,
}

impl Curve {
    pub fn new(points: &[(f32, f32)]) -> Self {
        let n = points.len();
        let xs: Vec<f32> = points.iter().map(|p| p.0).collect();
        let ys: Vec<f32> = points.iter().map(|p| p.1).collect();
        let mut d = vec![0.0; n.saturating_sub(1)];
        for i in 0..n.saturating_sub(1) {
            d[i] = (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]).max(1e-6);
        }
        let mut ms = vec![0.0; n];
        if n >= 2 {
            ms[0] = d[0];
            ms[n - 1] = d[n - 2];
            for i in 1..n - 1 {
                ms[i] = if d[i - 1] * d[i] <= 0.0 {
                    0.0
                } else {
                    // Harmonic mean weighted by interval lengths.
                    let (h0, h1) = (xs[i] - xs[i - 1], xs[i + 1] - xs[i]);
                    let w1 = 2.0 * h1 + h0;
                    let w2 = h1 + 2.0 * h0;
                    (w1 + w2) / (w1 / d[i - 1] + w2 / d[i])
                };
            }
        }
        Self { xs, ys, ms }
    }

    pub fn at(&self, x: f32) -> f32 {
        let n = self.xs.len();
        if n == 0 {
            return 0.0;
        }
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = self.xs.partition_point(|&v| v <= x).clamp(1, n - 1) - 1;
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * self.ys[i] + h10 * h * self.ms[i] + h01 * self.ys[i + 1] + h11 * h * self.ms[i + 1]
    }
}

/// Superellipse quarter: point at angle `t` (0..π/2) with exponent `p` (2 = ellipse).
#[inline]
pub fn superellipse(t: f32, p: f32) -> Vec2 {
    let (s, c) = t.sin_cos();
    let e = 2.0 / p.max(0.5);
    Vec2::new(c.abs().powf(e) * c.signum(), s.abs().powf(e) * s.signum())
}

// ---------------------------------------------------------------- meshes

/// A grid surface: `rows[i][j]` (rows along v, columns along u). Faces point along
/// du × dv; `flip` turns them around. `uv(i, j)` gives texture coordinates (`j` runs to
/// `cols` inclusive when `wrap_u` closes the rows into rings, for the seam's duplicate).
pub fn grid(
    rows: &[Vec<Vec3>],
    wrap_u: bool,
    flip: bool,
    uv: impl Fn(usize, usize) -> Vec2,
) -> Mesh {
    let n = rows.len();
    let m = rows.first().map_or(0, |r| r.len());
    if n < 2 || m < 2 {
        return Mesh::default();
    }
    let at = |i: usize, j: isize| -> Vec3 {
        let j = if wrap_u {
            j.rem_euclid(m as isize) as usize
        } else {
            j.clamp(0, m as isize - 1) as usize
        };
        rows[i][j]
    };
    // Grid normals from central differences (one-sided at open borders).
    let mut normals = vec![Vec3::ZERO; n * m];
    for i in 0..n {
        for j in 0..m {
            let ji = j as isize;
            let du = at(i, ji + 1) - at(i, ji - 1);
            let dv = rows[(i + 1).min(n - 1)][j] - rows[i.saturating_sub(1)][j];
            let nrm = du.cross(dv);
            normals[i * m + j] = if nrm.length_squared() > 1e-14 {
                nrm.normalize()
            } else {
                Vec3::ZERO
            };
        }
    }
    // Poles (a row collapsed to a point): use the neighboring row's mean normal.
    for i in 0..n {
        if normals[i * m..(i + 1) * m].iter().all(|v| *v == Vec3::ZERO) {
            let k = if i + 1 < n {
                i + 1
            } else {
                i.saturating_sub(1)
            };
            let mean: Vec3 = normals[k * m..(k + 1) * m].iter().copied().sum();
            let mean = mean.normalize_or_zero();
            for v in &mut normals[i * m..(i + 1) * m] {
                *v = mean;
            }
        }
        for j in 0..m {
            if normals[i * m + j] == Vec3::ZERO {
                // A degenerate point inside a row: borrow from a neighbor.
                let k = if i + 1 < n {
                    i + 1
                } else {
                    i.saturating_sub(1)
                };
                normals[i * m + j] = normals[k * m + j];
            }
        }
    }
    let cols = if wrap_u { m + 1 } else { m };
    let mut vertices = Vec::with_capacity(n * cols);
    for i in 0..n {
        for j in 0..cols {
            let jj = j % m;
            let mut nrm = normals[i * m + jj];
            if flip {
                nrm = -nrm;
            }
            vertices.push(Vertex::new(rows[i][jj], nrm, uv(i, j)));
        }
    }
    let mut indices = Vec::with_capacity((n - 1) * (cols - 1) * 6);
    for i in 0..n - 1 {
        for j in 0..cols - 1 {
            let a = (i * cols + j) as u32;
            let b = a + 1;
            let d = a + cols as u32;
            let c = d + 1;
            if flip {
                indices.extend_from_slice(&[a, c, b, a, d, c]);
            } else {
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    Mesh::new(vertices, indices)
}

/// Fill a closed outline (a convex-ish polygon) with a fan from its centroid, facing along
/// `normal`.
pub fn fan(outline: &[Vec3], normal: Vec3, uv: impl Fn(Vec3) -> Vec2) -> Mesh {
    let n = outline.len();
    if n < 3 {
        return Mesh::default();
    }
    let c = outline.iter().copied().sum::<Vec3>() / n as f32;
    let mut vertices = vec![Vertex::new(c, normal, uv(c))];
    for p in outline {
        vertices.push(Vertex::new(*p, normal, uv(*p)));
    }
    let mut indices = Vec::new();
    for k in 0..n as u32 {
        let (a, b) = (1 + k, 1 + (k + 1) % n as u32);
        let face =
            (outline[(k as usize + 1) % n] - outline[k as usize]).cross(c - outline[k as usize]);
        if face.dot(normal) >= 0.0 {
            indices.extend_from_slice(&[0, b, a]);
        } else {
            indices.extend_from_slice(&[0, a, b]);
        }
    }
    // Make the winding agree with `normal` (CCW seen from the normal side).
    let mut m = Mesh::new(vertices, indices);
    fix_winding(&mut m, normal);
    m
}

/// Turn every triangle so its geometric normal agrees with `normal`.
fn fix_winding(m: &mut Mesh, normal: Vec3) {
    for t in m.indices.as_chunks_mut::<3>().0 {
        let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
        let g = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        if g.dot(normal) < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// A surface of revolution about +Z (from `origin`): `profile` is (radius, z) from one end
/// to the other. Faces point outward when z increases along the profile with the radius
/// on the outside; `inward` flips them (nozzle interiors). U runs around, V along the
/// profile's length.
pub fn lathe(profile: &[(f32, f32)], segments: usize, inward: bool) -> Mesh {
    let rows: Vec<Vec<Vec3>> = profile
        .iter()
        .map(|&(r, z)| {
            (0..segments)
                .map(|j| {
                    let a = j as f32 / segments as f32 * TAU;
                    Vec3::new(a.cos() * r, a.sin() * r, z)
                })
                .collect()
        })
        .collect();
    let mut len = vec![0.0f32; profile.len()];
    for i in 1..profile.len() {
        let d = Vec2::new(
            profile[i].0 - profile[i - 1].0,
            profile[i].1 - profile[i - 1].1,
        );
        len[i] = len[i - 1] + d.length();
    }
    let total = len.last().copied().unwrap_or(1.0).max(1e-6);
    // du × dv points outward for a CCW ring (seen from +Z) when z grows along the profile.
    grid(&rows, true, inward, |i, j| {
        Vec2::new(j as f32 / segments as f32, len[i] / total)
    })
}

/// A tube along `path` with `radius(t)` (t = 0..1 along the path), `sides` around, using
/// parallel-transported frames. Ends are capped with `cap`.
pub fn tube(path: &[Vec3], radius: impl Fn(f32) -> f32, sides: usize, cap: bool) -> Mesh {
    let n = path.len();
    if n < 2 {
        return Mesh::default();
    }
    let tangent = |i: usize| -> Vec3 {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        (b - a).normalize_or(Vec3::Z)
    };
    let t0 = tangent(0);
    let mut normal = t0.any_orthonormal_vector();
    let mut len = vec![0.0f32; n];
    for i in 1..n {
        len[i] = len[i - 1] + (path[i] - path[i - 1]).length();
    }
    let total = len[n - 1].max(1e-6);
    let mut rows = Vec::with_capacity(n);
    let mut prev_t = t0;
    for (i, p) in path.iter().enumerate() {
        let t = tangent(i);
        // Parallel transport: rotate the normal by the change in tangent.
        let q = Quat::from_rotation_arc(prev_t, t);
        normal = (q * normal - t * (q * normal).dot(t)).normalize_or(t.any_orthonormal_vector());
        prev_t = t;
        let binormal = t.cross(normal);
        let r = radius(len[i] / total);
        rows.push(
            (0..sides)
                .map(|j| {
                    let a = j as f32 / sides as f32 * TAU;
                    *p + (normal * a.cos() + binormal * a.sin()) * r
                })
                .collect::<Vec<_>>(),
        );
    }
    let mut mesh = grid(&rows, true, false, |i, j| {
        Vec2::new(j as f32 / sides as f32, len[i] / total)
    });
    orient_out_from(&mut mesh, path);
    if cap {
        for (i, dir) in [(0usize, -t0), (n - 1, tangent(n - 1))] {
            let ring = &rows[i];
            mesh.merge(&fan(ring, dir, |_| Vec2::splat(0.5)));
        }
    }
    mesh
}

/// Flip a tube's faces if they point toward its center line.
fn orient_out_from(mesh: &mut Mesh, path: &[Vec3]) {
    let mut score = 0.0;
    for v in mesh.vertices.iter().step_by(7) {
        let p = Vec3::from(v.position);
        let nearest = path
            .iter()
            .min_by(|a, b| {
                (**a - p)
                    .length_squared()
                    .total_cmp(&(**b - p).length_squared())
            })
            .copied()
            .unwrap_or(Vec3::ZERO);
        score += Vec3::from(v.normal).dot(p - nearest);
    }
    if score < 0.0 {
        flip_faces(mesh);
    }
}

/// Reverse winding and normals.
pub fn flip_faces(mesh: &mut Mesh) {
    for t in mesh.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    for v in &mut mesh.vertices {
        v.normal = (-Vec3::from(v.normal)).to_array();
    }
}

/// A box with rounded edges (`radius`), centered on the origin, `seg` divisions per face
/// side (denser toward the edges). UVs are box-projected in meters.
pub fn rounded_box(half: Vec3, radius: f32, seg: usize) -> Mesh {
    let r = radius.min(half.min_element() * 0.999).max(0.0);
    let inner = half - Vec3::splat(r);
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut mesh = Mesh::default();
    let s = seg.max(2);
    for (n, u, v) in faces {
        // Parameter spacing that spends half the samples on the rounded rim.
        let param = |k: usize| -> f32 {
            let t = k as f32 / s as f32 * 2.0 - 1.0;
            t.signum() * t.abs().powf(0.6)
        };
        let rows: Vec<Vec<Vec3>> = (0..=s)
            .map(|i| {
                (0..=s)
                    .map(|j| {
                        let q = n * half + u * param(j) * half + v * param(i) * half;
                        let q = q * (n.abs() + (u.abs() + v.abs())) * 1.0;
                        let c = q.clamp(-inner, inner);
                        let d = q - c;
                        if d.length_squared() < 1e-12 {
                            c + n * r
                        } else {
                            c + d.normalize() * r
                        }
                    })
                    .collect()
            })
            .collect();
        let mut face = grid(&rows, false, false, |i, j| {
            let p = rows[i][j];
            Vec2::new(p.dot(u), -p.dot(v))
        });
        // The grid's du × dv must point along n.
        let g = Vec3::from(face.vertices[face.vertices.len() / 2].normal);
        if g.dot(n) < 0.0 {
            flip_faces(&mut face);
        }
        mesh.merge(&face);
    }
    mesh
}

/// Transform a mesh (positions and normals) and return it.
pub fn transformed(mut mesh: Mesh, m: Mat4) -> Mesh {
    mesh.transform(&m);
    if m.determinant() < 0.0 {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    mesh
}

/// Mirror across x = 0, keeping faces outward.
pub fn mirrored_x(mesh: &Mesh) -> Mesh {
    transformed(mesh.clone(), Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)))
}

/// Multiply every vertex color by `f(position, normal)`.
pub fn tint(mesh: &mut Mesh, f: impl Fn(Vec3, Vec3) -> Vec4) {
    for v in &mut mesh.vertices {
        let c = Vec4::from(v.color) * f(Vec3::from(v.position), Vec3::from(v.normal));
        v.color = c.to_array();
    }
}

/// Map every vertex UV through `f`.
pub fn remap_uv(mesh: &mut Mesh, f: impl Fn(Vec2) -> Vec2) {
    for v in &mut mesh.vertices {
        v.uv = f(Vec2::from(v.uv)).to_array();
    }
}

/// Merge several meshes.
pub fn merged(parts: impl IntoIterator<Item = Mesh>) -> Mesh {
    let mut out = Mesh::default();
    for p in parts {
        out.merge(&p);
    }
    out
}

/// A NACA 4-digit section (unit chord) sampled at `n` cosine-spaced stations: returns
/// (x, upper y, lower y) from the leading edge (x = 0) to the trailing edge (x = 1). The
/// trailing edge keeps a small thickness (`te`, fraction of chord) like a real part.
pub fn naca(
    thickness: f32,
    camber: f32,
    camber_pos: f32,
    n: usize,
    te: f32,
) -> Vec<(f32, f32, f32)> {
    (0..n)
        .map(|i| {
            let x = 0.5 * (1.0 - (PI * i as f32 / (n - 1) as f32).cos());
            naca_at(x, thickness, camber, camber_pos, te)
        })
        .collect()
}

/// One NACA 4-digit station at chord fraction `x`: (x, upper y, lower y), thickness measured
/// perpendicular to the chord (simplified: added vertically to the camber line).
pub fn naca_at(x: f32, thickness: f32, camber: f32, camber_pos: f32, te: f32) -> (f32, f32, f32) {
    let x = x.clamp(0.0, 1.0);
    let yt = 5.0
        * thickness
        * (0.2969 * x.sqrt() - 0.1260 * x - 0.3516 * x * x + 0.2843 * x.powi(3)
            - 0.1015 * x.powi(4))
        + 0.5 * te * x;
    let yc = if camber <= 0.0 {
        0.0
    } else if x < camber_pos {
        camber / camber_pos.powi(2) * (2.0 * camber_pos * x - x * x)
    } else {
        camber / (1.0 - camber_pos).powi(2)
            * (1.0 - 2.0 * camber_pos + 2.0 * camber_pos * x - x * x)
    };
    (x, yc + yt, yc - yt)
}

// ---------------------------------------------------------------- texture baking

/// Where every texel of a texture lies on its parts: position, normal and part id (0 = no
/// part), rasterized from the parts' UVs.
pub struct Atlas {
    pub w: usize,
    pub h: usize,
    pub pos: Vec<Vec3>,
    pub nrm: Vec<Vec3>,
    pub part: Vec<u8>,
    /// Texels claimed by two different parts (islands overlapping: a layout bug).
    pub overlaps: usize,
}

impl Atlas {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            pos: vec![Vec3::ZERO; w * h],
            nrm: vec![Vec3::ZERO; w * h],
            part: vec![0; w * h],
            overlaps: 0,
        }
    }

    /// Rasterize `mesh`'s triangles (UVs in 0..1) as part `part` (1..255).
    pub fn raster(&mut self, mesh: &Mesh, part: u8) {
        self.raster_with(mesh, |_| part);
    }

    /// Rasterize `mesh` with each triangle's part chosen from its UV centroid (0 skips it).
    pub fn raster_with(&mut self, mesh: &Mesh, part_of: impl Fn(Vec2) -> u8) {
        let (wf, hf) = (self.w as f32, self.h as f32);
        let w = self.w;
        // Triangles in texel space with their bounding rows.
        struct Tri {
            part: u8,
            q: [Vec2; 3],
            p: [Vec3; 3],
            n: [Vec3; 3],
            area: f32,
            min: Vec2,
            max: Vec2,
        }
        let tris: Vec<Tri> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter_map(|t| {
                let v = t.map(|i| &mesh.vertices[i as usize]);
                let centroid = v.iter().map(|v| Vec2::from(v.uv)).sum::<Vec2>() / 3.0;
                let part = part_of(centroid);
                if part == 0 {
                    return None;
                }
                let q = v.map(|v| Vec2::new(v.uv[0] * wf - 0.5, v.uv[1] * hf - 0.5));
                let area = (q[1] - q[0]).perp_dot(q[2] - q[0]);
                let min = q[0].min(q[1]).min(q[2]).floor().max(Vec2::ZERO);
                let max = q[0]
                    .max(q[1])
                    .max(q[2])
                    .ceil()
                    .min(Vec2::new(wf - 1.0, hf - 1.0));
                (area.abs() >= 1e-9 && min.x <= max.x && min.y <= max.y).then(|| Tri {
                    part,
                    q,
                    p: v.map(|v| Vec3::from(v.position)),
                    n: v.map(|v| Vec3::from(v.normal)),
                    area,
                    min,
                    max,
                })
            })
            .collect();
        // Rows are split over the cores; every band walks all triangles.
        let band = self.h.div_ceil(threads()).max(1);
        let tris = &tris;
        let overlaps: usize = std::thread::scope(|s| {
            let jobs: Vec<_> = self
                .pos
                .chunks_mut(band * w)
                .zip(self.nrm.chunks_mut(band * w))
                .zip(self.part.chunks_mut(band * w))
                .enumerate()
                .map(|(k, ((pos, nrm), parts))| {
                    s.spawn(move || {
                        let y0 = k * band;
                        let y1 = y0 + parts.len() / w;
                        let mut overlaps = 0;
                        for t in tris {
                            let ya = (t.min.y as usize).max(y0);
                            let yb = (t.max.y as usize + 1).min(y1);
                            let eps = -1e-4 * t.area.abs();
                            let (q, area) = (t.q, t.area);
                            for y in ya..yb {
                                for x in t.min.x as usize..=t.max.x as usize {
                                    let sp = Vec2::new(x as f32, y as f32);
                                    let w0 = (q[1] - sp).perp_dot(q[2] - sp) / area;
                                    let w1 = (q[2] - sp).perp_dot(q[0] - sp) / area;
                                    let w2 = 1.0 - w0 - w1;
                                    let a = area.abs();
                                    if w0 * a < eps || w1 * a < eps || w2 * a < eps {
                                        continue;
                                    }
                                    let i = (y - y0) * w + x;
                                    let part = t.part;
                                    if parts[i] != 0 && parts[i] != part {
                                        overlaps += 1;
                                    }
                                    pos[i] = t.p[0] * w0 + t.p[1] * w1 + t.p[2] * w2;
                                    nrm[i] = (t.n[0] * w0 + t.n[1] * w1 + t.n[2] * w2)
                                        .normalize_or(Vec3::Y);
                                    parts[i] = part;
                                }
                            }
                        }
                        overlaps
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().unwrap_or(0)).sum()
        });
        self.overlaps += overlaps;
    }
}

/// A texel being painted.
#[derive(Debug, Clone, Copy)]
pub struct Texel {
    /// Point on the part (the part's own frame, meters).
    pub p: Vec3,
    /// Surface normal there.
    pub n: Vec3,
    pub part: u8,
    pub uv: Vec2,
    /// Texel size on the surface, meters (antialiasing width).
    pub size: f32,
}

/// What a paint function returns for one texel.
#[derive(Debug, Clone, Copy)]
pub struct Paint {
    /// Linear albedo.
    pub color: Vec3,
    /// Height above the surface, meters (panel gaps negative, rivet heads positive).
    pub height: f32,
    pub rough: f32,
    pub metal: f32,
    /// Ambient occlusion, 0..1.
    pub ao: f32,
    /// Emission mask, 0..1 (only baked when asked).
    pub glow: f32,
}

impl Paint {
    pub fn new(color: Vec3, rough: f32) -> Self {
        Self {
            color,
            height: 0.0,
            rough,
            metal: 0.0,
            ao: 1.0,
            glow: 0.0,
        }
    }
}

/// The textures of one baked material.
pub struct Baked {
    /// sRGB albedo.
    pub color: Image,
    /// Tangent-space normal (+Y up).
    pub normal: Image,
    /// R = occlusion, G = roughness, B = metallic.
    pub orm: Image,
    /// sRGB emission mask (when asked).
    pub glow: Option<Image>,
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8)
}

/// Run `f(row, out_row)` for every row of a `w`-wide buffer on all cores.
pub fn par_rows<T: Send>(data: &mut [T], w: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len() / w.max(1);
    let per = rows.div_ceil(threads()).max(1);
    let f = &f;
    std::thread::scope(|s| {
        for (k, chunk) in data.chunks_mut(per * w).enumerate() {
            s.spawn(move || {
                for (r, row) in chunk.chunks_mut(w).enumerate() {
                    f(k * per + r, row);
                }
            });
        }
    });
}

/// Linear 0..1 to sRGB bytes through a table (the exact curve costs a `powf` per channel,
/// which dominates baking).
fn srgb_table() -> &'static [u8; 4096] {
    static TABLE: std::sync::OnceLock<[u8; 4096]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 4095.0;
            let s = if c <= 0.003_130_8 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0 + 0.5) as u8
        })
    })
}

#[inline]
fn to_srgb8(c: f32) -> u8 {
    srgb_table()[(c.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize]
}

#[inline]
fn to_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Paint every covered texel of `atlas` with `paint` and bake albedo, normal (from the
/// painted heights, scaled by `bump`), occlusion/roughness/metallic and optionally an
/// emission mask. Islands are padded a few texels so mipmaps don't bleed.
pub fn bake(atlas: &Atlas, glow: bool, bump: f32, paint: impl Fn(&Texel) -> Paint + Sync) -> Baked {
    let (w, h) = (atlas.w, atlas.h);
    let mut color = vec![[0u8; 4]; w * h];
    let mut orm = vec![[255u8, 200, 0, 255]; w * h];
    let mut height = vec![0.0f32; w * h];
    let mut glow_mask = if glow {
        vec![[0u8, 0, 0, 255]; w * h]
    } else {
        Vec::new()
    };
    // Texel sizes along x and y, found while painting and reused for the normals.
    let mut sizes = vec![[0.0f32; 2]; w * h];
    // Texel size on the surface from neighbor positions.
    let size_at = |x: usize, y: usize| -> (f32, f32) {
        let i = y * w + x;
        let part = atlas.part[i];
        let pick = |a: Option<usize>, b: Option<usize>| -> f32 {
            let ok = |k: Option<usize>| k.filter(|&k| atlas.part[k] == part);
            match (ok(a), ok(b)) {
                (Some(a), Some(b)) => (atlas.pos[a] - atlas.pos[b]).length() * 0.5,
                (Some(a), None) => (atlas.pos[a] - atlas.pos[i]).length(),
                (None, Some(b)) => (atlas.pos[b] - atlas.pos[i]).length(),
                (None, None) => 0.0,
            }
        };
        let lx = pick((x > 0).then(|| i - 1), (x + 1 < w).then_some(i + 1));
        let ly = pick((y > 0).then(|| i - w), (y + 1 < h).then_some(i + w));
        (lx, ly)
    };
    {
        // Paint.
        type Row<'a> = (
            &'a mut [[u8; 4]],
            &'a mut [[u8; 4]],
            &'a mut [f32],
            &'a mut [[f32; 2]],
            &'a mut [[u8; 4]],
        );
        let mut rows: Vec<Row> = Vec::with_capacity(h);
        let mut glow_rows = glow_mask.chunks_mut(w);
        for (((c, o), hgt), sz) in color
            .chunks_mut(w)
            .zip(orm.chunks_mut(w))
            .zip(height.chunks_mut(w))
            .zip(sizes.chunks_mut(w))
        {
            rows.push((c, o, hgt, sz, glow_rows.next().unwrap_or_default()));
        }
        let paint = &paint;
        let size_at = &size_at;
        par_rows(&mut rows, 1, |y, row| {
            let (c, o, hgt, sz, g) = &mut row[0];
            for x in 0..w {
                let i = y * w + x;
                let part = atlas.part[i];
                if part == 0 {
                    continue;
                }
                let (lx, ly) = size_at(x, y);
                sz[x] = [lx, ly];
                let t = Texel {
                    p: atlas.pos[i],
                    n: atlas.nrm[i],
                    part,
                    uv: Vec2::new((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32),
                    size: lx.max(ly).max(1e-5),
                };
                let p = paint(&t);
                c[x] = [
                    to_srgb8(p.color.x),
                    to_srgb8(p.color.y),
                    to_srgb8(p.color.z),
                    255,
                ];
                o[x] = [to_u8(p.ao), to_u8(p.rough), to_u8(p.metal), 255];
                hgt[x] = p.height;
                if !g.is_empty() {
                    let v = to_srgb8(p.glow);
                    g[x] = [v, v, v, 255];
                }
            }
        });
    }
    // Normals from heights, in the shader's derivative-built tangent frame: per texture
    // unit gradients over the shorter of the two texel metrics (see the module docs).
    let mut normal = vec![[128u8, 128, 255, 255]; w * h];
    par_rows(&mut normal, w, |y, row| {
        for (x, out) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let part = atlas.part[i];
            if part == 0 {
                continue;
            }
            let same = |k: usize| atlas.part[k] == part;
            let hx = |k: Option<usize>| k.filter(|&k| same(k)).map(|k| height[k]);
            let l = hx((x > 0).then(|| i - 1));
            let r = hx((x + 1 < w).then_some(i + 1));
            let u = hx((y > 0).then(|| i - w));
            let d = hx((y + 1 < h).then_some(i + w));
            let c = height[i];
            let dhdx = match (l, r) {
                (Some(l), Some(r)) => (r - l) * 0.5,
                (Some(l), None) => c - l,
                (None, Some(r)) => r - c,
                _ => 0.0,
            };
            let dhdy = match (u, d) {
                (Some(u), Some(d)) => (d - u) * 0.5,
                (Some(u), None) => c - u,
                (None, Some(d)) => d - c,
                _ => 0.0,
            };
            let [lx, ly] = sizes[i];
            if lx <= 0.0 || ly <= 0.0 {
                continue;
            }
            // Meters per texture unit along u and v.
            let (lu, lv) = (lx * w as f32, ly * h as f32);
            let m = lu.min(lv);
            let tx = -(dhdx * w as f32) / m * bump;
            let ty = (dhdy * h as f32) / m * bump;
            let n = Vec3::new(tx, ty, 1.0).normalize();
            *out = [
                to_u8(n.x * 0.5 + 0.5),
                to_u8(n.y * 0.5 + 0.5),
                to_u8(n.z * 0.5 + 0.5),
                255,
            ];
        }
    });
    // Pad islands: each pass finds the empty texels next to filled ones (in parallel), then
    // copies their neighbors' values.
    let mut filled: Vec<bool> = atlas.part.iter().map(|&p| p != 0).collect();
    for _ in 0..4 {
        let rows = h.div_ceil(threads()).max(1);
        let snapshot = &filled;
        let found: Vec<(usize, usize)> = std::thread::scope(|s| {
            let jobs: Vec<_> = (0..h)
                .step_by(rows)
                .map(|y0| {
                    s.spawn(move || {
                        let mut out = Vec::new();
                        for y in y0..(y0 + rows).min(h) {
                            for x in 0..w {
                                let i = y * w + x;
                                if snapshot[i] {
                                    continue;
                                }
                                let src = if x > 0 && snapshot[i - 1] {
                                    i - 1
                                } else if x + 1 < w && snapshot[i + 1] {
                                    i + 1
                                } else if y > 0 && snapshot[i - w] {
                                    i - w
                                } else if y + 1 < h && snapshot[i + w] {
                                    i + w
                                } else {
                                    continue;
                                };
                                out.push((i, src));
                            }
                        }
                        out
                    })
                })
                .collect();
            jobs.into_iter()
                .flat_map(|j| j.join().unwrap_or_default())
                .collect()
        });
        for (i, src) in found {
            color[i] = color[src];
            orm[i] = orm[src];
            normal[i] = normal[src];
            if glow {
                glow_mask[i] = glow_mask[src];
            }
            filled[i] = true;
        }
    }
    let (wu, hu) = (w as u32, h as u32);
    Baked {
        color: Image::new(wu, hu, color.into_flattened(), true),
        normal: Image::new(wu, hu, normal.into_flattened(), false),
        orm: Image::new(wu, hu, orm.into_flattened(), false),
        glow: glow.then(|| Image::new(wu, hu, glow_mask.into_flattened(), true)),
    }
}

/// A texture painted directly in UV space (`f(u, v) -> rgba`, linear for data, sRGB-encoded
/// by the caller for colors) on all cores.
pub fn image_fn(w: usize, h: usize, srgb: bool, f: impl Fn(f32, f32) -> [u8; 4] + Sync) -> Image {
    let mut px = vec![[0u8; 4]; w * h];
    par_rows(&mut px, w, |y, row| {
        for (x, p) in row.iter_mut().enumerate() {
            *p = f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        }
    });
    Image::new(w as u32, h as u32, px.into_iter().flatten().collect(), srgb)
}

/// Encode a linear color as sRGB bytes.
pub fn srgb(c: Vec3) -> [u8; 3] {
    [to_srgb8(c.x), to_srgb8(c.y), to_srgb8(c.z)]
}

/// A normal map from a height function in UV space (`height(u, v)` in meters on a texture
/// that spans `meters_u` × `meters_v`).
pub fn normal_image(
    w: usize,
    h: usize,
    meters_u: f32,
    meters_v: f32,
    height: impl Fn(f32, f32) -> f32 + Sync,
) -> Image {
    let (du, dv) = (1.0 / w as f32, 1.0 / h as f32);
    let m = meters_u.min(meters_v);
    image_fn(w, h, false, |u, v| {
        let gx = (height(u + du, v) - height(u - du, v)) / (2.0 * du);
        let gy = (height(u, v + dv) - height(u, v - dv)) / (2.0 * dv);
        let n = Vec3::new(-gx / m, gy / m, 1.0).normalize();
        [
            to_u8(n.x * 0.5 + 0.5),
            to_u8(n.y * 0.5 + 0.5),
            to_u8(n.z * 0.5 + 0.5),
            255,
        ]
    })
}

// ---------------------------------------------------------------- stroke font

/// Strokes of each glyph on a 4 × 6 grid (y up), as polylines separated by `;`.
fn glyph_strokes(c: char) -> &'static str {
    match c {
        'A' => "0,0 0,4.4 1.4,6 2.6,6 4,4.4 4,0;0,2.8 4,2.8",
        'B' => "0,0 0,6 3,6 4,5 4,4 3,3.1 0,3.1;3,3.1 4,2.1 4,1 3,0 0,0",
        'C' => "4,4.9 3,6 1,6 0,4.9 0,1.1 1,0 3,0 4,1.1",
        'D' => "0,0 0,6 2.6,6 4,4.6 4,1.4 2.6,0 0,0",
        'E' => "4,6 0,6 0,0 4,0;0,3.1 3,3.1",
        'F' => "4,6 0,6 0,0;0,3.1 3,3.1",
        'G' => "4,4.9 3,6 1,6 0,4.9 0,1.1 1,0 3,0 4,1.1 4,2.8 2.2,2.8",
        'H' => "0,0 0,6;4,0 4,6;0,3.1 4,3.1",
        'I' => "1,0 3,0;2,0 2,6;1,6 3,6",
        'J' => "0,1.1 1,0 3,0 4,1.1 4,6",
        'K' => "0,0 0,6;4,6 0,2;1.4,3.4 4,0",
        'L' => "0,6 0,0 4,0",
        'M' => "0,0 0,6 2,2.8 4,6 4,0",
        'N' => "0,0 0,6 4,0 4,6",
        'O' | '0' => "1,0 0,1.1 0,4.9 1,6 3,6 4,4.9 4,1.1 3,0 1,0",
        'P' => "0,0 0,6 3,6 4,5 4,4 3,3 0,3",
        'Q' => "1,0 0,1.1 0,4.9 1,6 3,6 4,4.9 4,1.1 3,0 1,0;2.6,1.4 4,-0.4",
        'R' => "0,0 0,6 3,6 4,5 4,4 3,3 0,3;2,3 4,0",
        'S' => "4,4.9 3,6 1,6 0,4.9 0,4.1 1,3.1 3,3.1 4,2.1 4,1.1 3,0 1,0 0,1.1",
        'T' => "0,6 4,6;2,6 2,0",
        'U' => "0,6 0,1.1 1,0 3,0 4,1.1 4,6",
        'V' => "0,6 2,0 4,6",
        'W' => "0,6 0.9,0 2,4 3.1,0 4,6",
        'X' => "0,0 4,6;0,6 4,0",
        'Y' => "0,6 2,3 4,6;2,3 2,0",
        'Z' => "0,6 4,6 0,0 4,0",
        '1' => "0.8,4.8 2.2,6 2.2,0;0.8,0 3.6,0",
        '2' => "0,4.9 1,6 3,6 4,4.9 4,4 0,0 4,0",
        '3' => "0,4.9 1,6 3,6 4,4.9 4,4.1 3,3.1 1.6,3.1;3,3.1 4,2.1 4,1.1 3,0 1,0 0,1.1",
        '4' => "3,0 3,6 0,1.8 4,1.8",
        '5' => "4,6 0,6 0,3.4 3,3.4 4,2.4 4,1.1 3,0 1,0 0,1.1",
        '6' => "4,4.9 3,6 1,6 0,4.9 0,1.1 1,0 3,0 4,1.1 4,2.1 3,3.1 0,3.1",
        '7' => "0,6 4,6 1.4,0",
        '8' => {
            "1,3.1 0,4.1 0,4.9 1,6 3,6 4,4.9 4,4.1 3,3.1 1,3.1 0,2.1 0,1.1 1,0 3,0 4,1.1 4,2.1 3,3.1"
        }
        '9' => "0,1.1 1,0 3,0 4,1.1 4,4.9 3,6 1,6 0,4.9 0,4.1 1,3.1 4,3.1",
        '-' => "0.8,3 3.2,3",
        '.' => "2,0 2,0.05",
        '/' => "0,0 4,6",
        '>' => "0,6 4,3 0,0",
        '<' => "4,6 0,3 4,0",
        '!' => "2,6 2,1.8;2,0 2,0.05",
        _ => "",
    }
}

/// Text rendered as distances to stroke center lines: glyphs 4 units wide and 6 tall,
/// advancing `advance` units per character.
pub struct StrokeText {
    segments: Vec<Vec<(Vec2, Vec2)>>,
    pub advance: f32,
}

impl StrokeText {
    pub fn new(text: &str, advance: f32) -> Self {
        let segments = text
            .chars()
            .map(|c| {
                let mut segs = Vec::new();
                for stroke in glyph_strokes(c.to_ascii_uppercase()).split(';') {
                    let pts: Vec<Vec2> = stroke
                        .split_whitespace()
                        .filter_map(|p| {
                            let (a, b) = p.split_once(',')?;
                            Some(Vec2::new(a.parse().ok()?, b.parse().ok()?))
                        })
                        .collect();
                    for k in 1..pts.len() {
                        segs.push((pts[k - 1], pts[k]));
                    }
                }
                segs
            })
            .collect();
        Self { segments, advance }
    }

    /// Width of the whole string in glyph units.
    pub fn width(&self) -> f32 {
        (self.segments.len() as f32 - 1.0).max(0.0) * self.advance + 4.0
    }

    /// Distance from `p` (glyph units, origin at the first glyph's bottom left) to the nearest
    /// stroke.
    pub fn distance(&self, p: Vec2) -> f32 {
        if p.y < -2.0 || p.y > 8.0 || p.x < -2.0 || p.x > self.width() + 2.0 {
            return 1e9;
        }
        let k = (p.x / self.advance).floor() as isize;
        let mut best = 1e9f32;
        for c in [k - 1, k, k + 1] {
            if c < 0 || c as usize >= self.segments.len() {
                continue;
            }
            let o = Vec2::new(c as f32 * self.advance, 0.0);
            for &(a, b) in &self.segments[c as usize] {
                best = best.min(segment_distance(p - o, a, b));
            }
        }
        best
    }
}

/// Distance from `p` to the segment `a`–`b`.
pub fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Signed distance to a rounded rectangle centered at the origin (negative inside).
pub fn rounded_rect(p: Vec2, half: Vec2, r: f32) -> f32 {
    let q = p.abs() - half + Vec2::splat(r);
    q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - r
}

// ---------------------------------------------------------------- assets

/// One material of a model: values plus the keys of its textures.
pub struct MaterialDef {
    pub key: String,
    pub material: StandardMaterial,
    pub color: Option<String>,
    pub normal: Option<String>,
    /// Occlusion (R), roughness (G) and metallic (B).
    pub orm: Option<String>,
    pub glow: Option<String>,
}

impl MaterialDef {
    pub fn plain(key: &str, material: StandardMaterial) -> Self {
        Self {
            key: key.into(),
            material,
            color: None,
            normal: None,
            orm: None,
            glow: None,
        }
    }

    /// A material whose textures are `<tex>/color`, `<tex>/normal` and `<tex>/orm`.
    pub fn baked(key: &str, material: StandardMaterial, tex: &str) -> Self {
        Self {
            key: key.into(),
            material,
            color: Some(format!("{tex}/color")),
            normal: Some(format!("{tex}/normal")),
            orm: Some(format!("{tex}/orm")),
            glow: None,
        }
    }
}

/// Everything a model registers: meshes, images and materials, each under a `vehicles/…`
/// key.
#[derive(Default)]
pub struct ModelAssets {
    pub meshes: Vec<(String, Mesh)>,
    pub images: Vec<(String, Image)>,
    pub materials: Vec<MaterialDef>,
}

impl ModelAssets {
    pub fn mesh(&mut self, key: &str, mesh: Mesh) {
        self.meshes.push((key.into(), mesh));
    }

    pub fn image(&mut self, key: &str, image: Image) {
        self.images.push((key.into(), image));
    }

    /// Add a baked set as `<key>/color`, `<key>/normal`, `<key>/orm` (and `<key>/glow`).
    pub fn baked(&mut self, key: &str, b: Baked) {
        self.image(&format!("{key}/color"), b.color);
        self.image(&format!("{key}/normal"), b.normal);
        self.image(&format!("{key}/orm"), b.orm);
        if let Some(g) = b.glow {
            self.image(&format!("{key}/glow"), g);
        }
    }

    pub fn material(&mut self, def: MaterialDef) {
        self.materials.push(def);
    }

    /// Triangles over all meshes.
    pub fn triangles(&self) -> usize {
        self.meshes.iter().map(|(_, m)| m.triangle_count()).sum()
    }

    /// Insert everything into the world's asset stores (each key once).
    pub fn install(self, world: &mut World) {
        let mut handles = std::collections::HashMap::new();
        if let Some(mut images) = world.get_resource_mut::<Assets<Image>>() {
            for (key, image) in self.images {
                let h = images.get_or_insert_keyed(&key, || image);
                handles.insert(key, h);
            }
        }
        if let Some(mut materials) = world.get_resource_mut::<Assets<StandardMaterial>>() {
            for def in self.materials {
                let mut m = def.material;
                let tex = |k: &Option<String>| k.as_ref().and_then(|k| handles.get(k).copied());
                m.base_color_texture = tex(&def.color);
                m.normal_texture = tex(&def.normal);
                m.metallic_roughness_texture = tex(&def.orm);
                m.occlusion_texture = tex(&def.orm);
                m.emissive_texture = tex(&def.glow);
                materials.get_or_insert_keyed(&def.key, || m);
            }
        }
        if let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() {
            for (key, mesh) in self.meshes {
                meshes.get_or_insert_keyed(&key, || mesh);
            }
        }
    }
}

/// Build and install a model's assets the first time it is spawned in a world that draws
/// (one with mesh, image and material stores). `guard` is a mesh key the model registers.
pub fn ensure(world: &mut World, guard: &str, build: impl FnOnce() -> ModelAssets) {
    let draws = world.contains_resource::<Assets<Mesh>>()
        && world.contains_resource::<Assets<Image>>()
        && world.contains_resource::<Assets<StandardMaterial>>();
    if !draws || world.resource::<Assets<Mesh>>().by_key(guard).is_some() {
        return;
    }
    build().install(world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_passes_through_points_without_overshoot() {
        let c = Curve::new(&[(0.0, 0.0), (1.0, 1.0), (2.0, 1.0), (3.0, 0.0)]);
        assert!((c.at(1.0) - 1.0).abs() < 1e-5);
        for i in 0..=30 {
            let v = c.at(i as f32 * 0.1);
            assert!((-1e-4..=1.0001).contains(&v), "{v}");
        }
    }

    #[test]
    fn lathe_and_tube_face_outward() {
        let l = lathe(&[(0.0, 0.0), (0.5, 0.1), (0.5, 1.0), (0.0, 1.1)], 16, false);
        for v in &l.vertices {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            let radial = Vec3::new(p.x, p.y, 0.0);
            if radial.length() > 0.4 && p.z > 0.2 && p.z < 0.9 {
                assert!(n.dot(radial) > 0.0);
            }
        }
        let t = tube(
            &[Vec3::ZERO, Vec3::Z, Vec3::new(1.0, 0.0, 2.0)],
            |_| 0.1,
            8,
            true,
        );
        assert!(t.triangle_count() > 30);
    }

    #[test]
    fn text_distance_finds_strokes() {
        let t = StrokeText::new("H1", 5.5);
        assert!(t.distance(Vec2::new(0.0, 3.0)) < 1e-4);
        assert!(t.distance(Vec2::new(2.0, 5.0)) > 1.0);
        assert!(t.distance(Vec2::new(5.5 + 2.2, 3.0)) < 1e-4);
    }

    #[test]
    fn baking_paints_covered_texels_and_tilts_normals_on_slopes() {
        // A unit quad with UVs = xy; a height ramp along x must tilt normals toward -u.
        let mut m = Mesh::new(
            vec![
                Vertex::new(Vec3::new(0.0, 0.0, 0.0), Vec3::Z, Vec2::new(0.0, 0.0)),
                Vertex::new(Vec3::new(1.0, 0.0, 0.0), Vec3::Z, Vec2::new(1.0, 0.0)),
                Vertex::new(Vec3::new(1.0, 1.0, 0.0), Vec3::Z, Vec2::new(1.0, 1.0)),
                Vertex::new(Vec3::new(0.0, 1.0, 0.0), Vec3::Z, Vec2::new(0.0, 1.0)),
            ],
            vec![0, 1, 2, 0, 2, 3],
        );
        m.compute_normals();
        let mut atlas = Atlas::new(32, 32);
        atlas.raster(&m, 1);
        assert!(atlas.part.iter().all(|&p| p == 1));
        let b = bake(&atlas, false, 1.0, |t| {
            let mut p = Paint::new(Vec3::ONE, 0.5);
            p.height = t.p.x * 0.1;
            p
        });
        let px = &b.normal.data[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 4];
        assert!(px[0] < 128, "{px:?}");
        assert!((px[1] as i32 - 128).abs() < 3, "{px:?}");
    }
}
