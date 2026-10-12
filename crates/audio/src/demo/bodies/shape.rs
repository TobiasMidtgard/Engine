//! Procedural meshes for the walkers: lofts (cross-sections along a spine), sweeps along
//! curves and parametric surfaces, all with smooth normals welded across seams and poles.
//!
//! A loft is a list of [`Key`] cross-sections: a centre, radii either side of two in-plane
//! axes and a superellipse exponent. The axes are `X` (projected off the spine) and
//! `q = tangent × X`, so `+q` points to the front on an upward spine (a torso), to the back on
//! a downward one (a leg), and down on one heading −Z (a dog's body, built tail to head).

use engine_core::math::{Quat, TAU, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

/// One cross-section of a loft.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// Centre, meters.
    pub c: Vec3,
    /// Radius toward +X and −X (in-plane side axis), meters.
    pub x: [f32; 2],
    /// Radius toward +q and −q, meters.
    pub q: [f32; 2],
    /// Superellipse exponent on the +q and −q halves: 2 is an ellipse, 3–5 a rounded box.
    pub p: [f32; 2],
}

/// A symmetric elliptical cross-section.
pub fn key(c: Vec3, rx: f32, rq: f32) -> Key {
    Key {
        c,
        x: [rx, rx],
        q: [rq, rq],
        p: [2.0, 2.0],
    }
}

impl Key {
    /// Different radii toward +q and −q.
    pub fn q2(mut self, pos: f32, neg: f32) -> Self {
        self.q = [pos, neg];
        self
    }

    /// Superellipse exponent.
    pub fn pow(mut self, p: f32) -> Self {
        self.p = [p, p];
        self
    }

    /// Superellipse exponents on the +q and −q halves.
    pub fn pow2(mut self, pos: f32, neg: f32) -> Self {
        self.p = [pos, neg];
        self
    }

    /// Catmull-Rom through `b` and `c` (radii clamped so overshoot never turns them inside
    /// out).
    fn spline(a: Key, b: Key, c: Key, d: Key, t: f32) -> Key {
        let cr = |a: f32, b: f32, c: f32, d: f32| {
            let t2 = t * t;
            let t3 = t2 * t;
            0.5 * (2.0 * b
                + (-a + c) * t
                + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
                + (-a + 3.0 * b - 3.0 * c + d) * t3)
        };
        let crv = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            Vec3::new(
                cr(a.x, b.x, c.x, d.x),
                cr(a.y, b.y, c.y, d.y),
                cr(a.z, b.z, c.z, d.z),
            )
        };
        let r = |i: usize, f: fn(&Key) -> [f32; 2]| {
            let lo = f(&b)[i].min(f(&c)[i]) * 0.85;
            cr(f(&a)[i], f(&b)[i], f(&c)[i], f(&d)[i]).max(lo).max(0.0)
        };
        Key {
            c: crv(a.c, b.c, c.c, d.c),
            x: [r(0, |k| k.x), r(1, |k| k.x)],
            q: [r(0, |k| k.q), r(1, |k| k.q)],
            p: [lerp(b.p[0], c.p[0], t), lerp(b.p[1], c.p[1], t)],
        }
    }

    /// Point at angle `a` (radians, 0 = +X, π/2 = +q) given the ring's axes.
    fn point(&self, ax: Vec3, aq: Vec3, a: f32) -> Vec3 {
        let (s, c) = a.sin_cos();
        let e = 2.0 / if s >= 0.0 { self.p[0] } else { self.p[1] }.max(0.5);
        let fx = c.signum() * c.abs().powf(e);
        let fq = s.signum() * s.abs().powf(e);
        let rx = if c >= 0.0 { self.x[0] } else { self.x[1] };
        let rq = if s >= 0.0 { self.q[0] } else { self.q[1] };
        self.c + ax * (fx * rx) + aq * (fq * rq)
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A Gaussian bump: 1 at the centre, falling off with `size`.
pub fn bump(d: Vec3, size: Vec3) -> f32 {
    let v = d / size;
    (-v.length_squared()).exp()
}

/// How the cross-section axes turn along the spine.
#[derive(Clone, Copy, Debug)]
pub enum Frames {
    /// `X` is this vector with the tangent removed (bilaterally symmetric bodies).
    Side(Vec3),
    /// Parallel transport starting from this side vector (curves that turn sideways).
    Transport(Vec3),
}

/// Cross-sections lofted into a tube, optionally closed with domes.
#[derive(Clone, Debug)]
pub struct Loft {
    pub keys: Vec<Key>,
    /// Vertices around each ring.
    pub around: usize,
    /// Largest distance between rings, meters.
    pub spacing: f32,
    /// Dome length closing the first and last ring (0 leaves it open), meters.
    pub caps: [f32; 2],
    pub frames: Frames,
}

impl Loft {
    pub fn new(keys: Vec<Key>, around: usize, spacing: f32) -> Self {
        Self {
            keys,
            around,
            spacing,
            caps: [0.0, 0.0],
            frames: Frames::Side(Vec3::X),
        }
    }

    pub fn caps(mut self, start: f32, end: f32) -> Self {
        self.caps = [start, end];
        self
    }

    pub fn frames(mut self, frames: Frames) -> Self {
        self.frames = frames;
        self
    }

    /// The cross-section `s` (0..=1 over the keys) along the loft, as the mesh samples it.
    pub fn at(&self, s: f32) -> Key {
        let n = self.keys.len();
        let x = s.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (x.floor() as usize).min(n - 2);
        Key::spline(
            self.key(i as isize - 1),
            self.keys[i],
            self.keys[i + 1],
            self.key(i as isize + 2),
            x - i as f32,
        )
    }

    /// The cross-section whose centre is at height `y` (for a loft rising in Y).
    pub fn at_y(&self, y: f32) -> Key {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if self.at(mid).c.y < y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        self.at(0.5 * (lo + hi))
    }

    /// Point on the ring `key` at angle `a` (0 = +X, π/2 = +q), with this loft's side axis.
    pub fn ring_point(&self, key: &Key, tangent: Vec3, a: f32) -> Vec3 {
        let side = match self.frames {
            Frames::Side(s) | Frames::Transport(s) => s,
        };
        let x = (side - tangent * tangent.dot(side)).normalize_or(Vec3::X);
        key.point(x, tangent.cross(x), a)
    }

    /// Key `i`, extended past either end by reflecting the centres.
    fn key(&self, i: isize) -> Key {
        let k = &self.keys;
        let n = k.len();
        if i < 0 {
            let mut e = k[0];
            e.c = k[0].c * 2.0 - k[1].c;
            e
        } else if i as usize >= n {
            let mut e = k[n - 1];
            e.c = k[n - 1].c * 2.0 - k[n - 2].c;
            e
        } else {
            k[i as usize]
        }
    }

    /// Sample the loft into a grid of rings.
    pub fn grid(&self) -> Grid {
        let k = &self.keys;
        assert!(k.len() >= 2, "a loft needs two keys");
        let n = k.len();
        let get = |i: isize| self.key(i);
        // Sample the spline.
        let mut rings: Vec<(Key, f32)> = Vec::new();
        for i in 0..n - 1 {
            let (a, b) = (k[i], k[i + 1]);
            let span =
                a.c.distance(b.c)
                    .max((a.x[0] - b.x[0]).abs())
                    .max((a.x[1] - b.x[1]).abs())
                    .max((a.q[0] - b.q[0]).abs())
                    .max((a.q[1] - b.q[1]).abs());
            let steps = ((span / self.spacing).ceil() as usize).max(1);
            for s in 0..steps {
                let t = s as f32 / steps as f32;
                let key = Key::spline(get(i as isize - 1), a, b, get(i as isize + 2), t);
                rings.push((key, (i as f32 + t) / (n - 1) as f32));
            }
        }
        rings.push((k[n - 1], 1.0));

        // Tangents and frames.
        let m = rings.len();
        let tangent = |i: usize| -> Vec3 {
            let a = rings[i.saturating_sub(1)].0.c;
            let b = rings[(i + 1).min(m - 1)].0.c;
            (b - a).normalize_or(Vec3::Y)
        };
        let mut frames: Vec<(Vec3, Vec3, Vec3)> = Vec::with_capacity(m);
        for i in 0..m {
            let t = tangent(i);
            let x = match self.frames {
                Frames::Side(side) => {
                    let x = side - t * t.dot(side);
                    if x.length_squared() < 1e-8 {
                        frames.last().map_or(Vec3::X, |f| f.1)
                    } else {
                        x.normalize()
                    }
                }
                Frames::Transport(side) => match frames.last() {
                    None => (side - t * t.dot(side)).normalize_or(Vec3::X),
                    Some(&(pt, px, _)) => {
                        let x = Quat::from_rotation_arc(pt, t) * px;
                        (x - t * t.dot(x)).normalize_or(px)
                    }
                },
            };
            frames.push((t, x, t.cross(x)));
        }

        let cols = self.around;
        let mut grid = Grid {
            rows: 0,
            cols,
            pts: Vec::new(),
            centers: Vec::new(),
            s: Vec::new(),
        };
        let ring = |key: &Key, ax: Vec3, aq: Vec3| -> Vec<Vec3> {
            (0..cols)
                .map(|j| key.point(ax, aq, j as f32 / cols as f32 * TAU))
                .collect()
        };
        let cap_rows = |key: &Key, ax: Vec3, aq: Vec3, dir: Vec3, len: f32| {
            let base = ring(key, ax, aq);
            let steps = 7;
            (1..=steps)
                .map(|i| {
                    let phi = i as f32 / steps as f32 * std::f32::consts::FRAC_PI_2;
                    let (s, c) = phi.sin_cos();
                    let c = if i == steps { 0.0 } else { c };
                    let center = key.c + dir * (s * len);
                    let pts: Vec<Vec3> = base.iter().map(|p| center + (*p - key.c) * c).collect();
                    (center, pts)
                })
                .collect::<Vec<_>>()
        };
        if self.caps[0] > 0.0 {
            let (t, ax, aq) = frames[0];
            let rows = cap_rows(&rings[0].0, ax, aq, -t, self.caps[0]);
            for (c, pts) in rows.into_iter().rev() {
                grid.push(c, pts, 0.0);
            }
        }
        for (i, (key, s)) in rings.iter().enumerate() {
            let (_, ax, aq) = frames[i];
            grid.push(key.c, ring(key, ax, aq), *s);
        }
        if self.caps[1] > 0.0 {
            let (t, ax, aq) = frames[m - 1];
            for (c, pts) in cap_rows(&rings[m - 1].0, ax, aq, t, self.caps[1]) {
                grid.push(c, pts, 1.0);
            }
        }
        grid
    }

    pub fn mesh(&self, paint: impl Fn(&Paint) -> Vec4) -> Mesh {
        self.grid().mesh(paint)
    }
}

/// What a paint closure knows about a vertex.
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub p: Vec3,
    pub n: Vec3,
    /// Along the loft, 0..1 (key index / (keys - 1)); caps read 0 or 1.
    pub s: f32,
}

/// Plain white vertex colour.
pub fn white(_: &Paint) -> Vec4 {
    Vec4::ONE
}

/// Rows of points around a spine; turned into a mesh by [`Grid::mesh`].
#[derive(Clone, Debug, Default)]
pub struct Grid {
    pub rows: usize,
    pub cols: usize,
    pub pts: Vec<Vec3>,
    pub centers: Vec<Vec3>,
    pub s: Vec<f32>,
}

impl Grid {
    fn push(&mut self, center: Vec3, pts: Vec<Vec3>, s: f32) {
        debug_assert_eq!(pts.len(), self.cols);
        self.pts.extend(pts);
        self.centers.push(center);
        self.s.push(s);
        self.rows += 1;
    }

    /// A surface from a function of (along 0..=1, around 0..1).
    pub fn surface(rows: usize, cols: usize, f: impl Fn(f32, f32) -> Vec3) -> Self {
        let mut g = Grid {
            rows: 0,
            cols,
            ..Default::default()
        };
        for r in 0..rows {
            let s = r as f32 / (rows - 1) as f32;
            let pts: Vec<Vec3> = (0..cols).map(|j| f(s, j as f32 / cols as f32)).collect();
            let c = pts.iter().copied().sum::<Vec3>() / cols as f32;
            g.push(c, pts, s);
        }
        g
    }

    /// Move every point: `f(point, ring centre, s, u)`.
    pub fn deform(mut self, f: impl Fn(Vec3, Vec3, f32, f32) -> Vec3) -> Self {
        for r in 0..self.rows {
            for j in 0..self.cols {
                let i = r * self.cols + j;
                self.pts[i] = f(
                    self.pts[i],
                    self.centers[r],
                    self.s[r],
                    j as f32 / self.cols as f32,
                );
            }
        }
        self
    }

    /// Triangulate with smooth normals (welded across the seam and at poles), texture
    /// coordinates in meters (around, along) and vertex colours from `paint`.
    pub fn mesh(&self, paint: impl Fn(&Paint) -> Vec4) -> Mesh {
        let (rows, cols) = (self.rows, self.cols);
        let at = |r: usize, j: usize| self.pts[r * cols + j % cols];
        let pole: Vec<bool> = (0..rows)
            .map(|r| (0..cols).all(|j| at(r, j).distance_squared(at(r, 0)) < 1e-12))
            .collect();
        // Outward winding: positive signed volume about the centroid.
        let o = self.pts.iter().copied().sum::<Vec3>() / self.pts.len().max(1) as f32;
        let mut vol = 0.0;
        for r in 0..rows - 1 {
            for j in 0..cols {
                let (a, b, c, d) = (at(r, j), at(r, j + 1), at(r + 1, j + 1), at(r + 1, j));
                vol += (a - o).dot((b - o).cross(c - o)) + (a - o).dot((c - o).cross(d - o));
            }
        }
        let flip = vol < 0.0;
        let slot = |r: usize, j: usize| {
            if pole[r] {
                r * cols
            } else {
                r * cols + j % cols
            }
        };
        let mut acc = vec![Vec3::ZERO; rows * cols];
        let mut tris: Vec<[(usize, usize); 3]> = Vec::with_capacity(rows * cols * 2);
        for r in 0..rows - 1 {
            for j in 0..cols {
                for tri in [
                    [(r, j), (r, j + 1), (r + 1, j + 1)],
                    [(r, j), (r + 1, j + 1), (r + 1, j)],
                ] {
                    let tri = if flip { [tri[0], tri[2], tri[1]] } else { tri };
                    let [a, b, c] = tri.map(|(r, j)| at(r, j));
                    let n = (b - a).cross(c - a);
                    if n.length_squared() < 1e-16 {
                        continue;
                    }
                    for (r, j) in tri {
                        acc[slot(r, j)] += n;
                    }
                    tris.push(tri);
                }
            }
        }
        // Texture coordinates in meters.
        let mut perim = 0.0;
        let mut count = 0;
        for (r, &at_pole) in pole.iter().enumerate() {
            if !at_pole {
                perim += (0..cols)
                    .map(|j| at(r, j).distance(at(r, j + 1)))
                    .sum::<f32>();
                count += 1;
            }
        }
        let perim = perim / count.max(1) as f32;
        let mut along = vec![0.0; rows];
        for r in 1..rows {
            along[r] = along[r - 1] + self.centers[r].distance(self.centers[r - 1]);
        }
        let stride = cols + 1;
        let mut vertices = Vec::with_capacity(rows * stride);
        for r in 0..rows {
            for j in 0..=cols {
                let p = at(r, j);
                let n = acc[slot(r, j)].normalize_or(Vec3::Y);
                let u = j as f32 / cols as f32;
                let color = paint(&Paint { p, n, s: self.s[r] });
                vertices.push(Vertex::new(p, n, Vec2::new(u * perim, along[r])).with_color(color));
            }
        }
        let indices = tris
            .iter()
            .flat_map(|tri| tri.map(|(r, j)| (r * stride + j) as u32))
            .collect();
        Mesh::new(vertices, indices)
    }
}

/// An open sheet (a hair card): `f(s, t)` over `rows`×`cols` samples of s, t in 0..=1, with
/// smooth normals, texture coordinates `uv(s, t)` and vertex colours `color(s, t)`.
pub fn sheet(
    rows: usize,
    cols: usize,
    f: impl Fn(f32, f32) -> Vec3,
    uv: impl Fn(f32, f32) -> Vec2,
    color: impl Fn(f32, f32) -> Vec4,
) -> Mesh {
    let at = |r: usize, c: usize| (r as f32 / (rows - 1) as f32, c as f32 / (cols - 1) as f32);
    let pts: Vec<Vec3> = (0..rows * cols)
        .map(|i| {
            let (s, t) = at(i / cols, i % cols);
            f(s, t)
        })
        .collect();
    let mut acc = vec![Vec3::ZERO; rows * cols];
    let mut indices = Vec::with_capacity((rows - 1) * (cols - 1) * 6);
    for r in 0..rows - 1 {
        for c in 0..cols - 1 {
            let (a, b, d, e) = (
                r * cols + c,
                r * cols + c + 1,
                (r + 1) * cols + c,
                (r + 1) * cols + c + 1,
            );
            for tri in [[a, d, e], [a, e, b]] {
                let n = (pts[tri[1]] - pts[tri[0]]).cross(pts[tri[2]] - pts[tri[0]]);
                if n.length_squared() < 1e-16 {
                    continue;
                }
                for i in tri {
                    acc[i] += n;
                }
                indices.extend(tri.map(|i| i as u32));
            }
        }
    }
    let vertices = (0..rows * cols)
        .map(|i| {
            let (s, t) = at(i / cols, i % cols);
            Vertex::new(pts[i], acc[i].normalize_or(Vec3::Y), uv(s, t)).with_color(color(s, t))
        })
        .collect();
    Mesh::new(vertices, indices)
}

/// Mirror a mesh across X (a left part from a right one), keeping the winding outward.
pub fn mirror_x(mut m: Mesh) -> Mesh {
    for v in &mut m.vertices {
        v.position[0] = -v.position[0];
        v.normal[0] = -v.normal[0];
    }
    for tri in m.indices.as_chunks_mut::<3>().0 {
        tri.swap(1, 2);
    }
    m
}

/// Merge several meshes into one.
pub fn merged(parts: impl IntoIterator<Item = Mesh>) -> Mesh {
    let mut out = Mesh::default();
    for p in parts {
        out.merge(&p);
    }
    out
}

/// Move and rotate a mesh.
pub fn placed(mut m: Mesh, rot: Quat, at: Vec3) -> Mesh {
    m.transform(&engine_core::math::Mat4::from_rotation_translation(rot, at));
    m
}

/// An ellipsoid (centre, radii) as a mesh with `cols` around.
pub fn ellipsoid(c: Vec3, r: Vec3, cols: usize, paint: impl Fn(&Paint) -> Vec4) -> Mesh {
    Grid::surface(cols / 2 + 1, cols, |s, u| {
        let phi = s * std::f32::consts::PI;
        let a = u * TAU;
        c + Vec3::new(
            phi.sin() * a.cos() * r.x,
            -phi.cos() * r.y,
            phi.sin() * a.sin() * r.z,
        )
    })
    .mesh(paint)
}

/// Fur shells over `m`: `layers` copies pushed out along the normals to `f(p, n).0` meters and
/// combed sideways by `f(p, n).1` per meter of length (so strands lie down rather than stand
/// up), darker deeper in. A layer at height `t` (0..1 of the fur's length) gets vertex alpha
/// `cutoff / t`, so with a material alpha cutoff of `cutoff` it keeps only the texels whose
/// alpha (a strand height field, 0..1) reaches it: strands taper to their tips.
pub fn shells(m: &Mesh, layers: usize, cutoff: f32, f: impl Fn(Vec3, Vec3) -> (f32, Vec3)) -> Mesh {
    let fur: Vec<(f32, Vec3)> = m
        .vertices
        .iter()
        .map(|v| f(Vec3::from(v.position), Vec3::from(v.normal)))
        .collect();
    let hairy = |i: u32| fur[i as usize].0 > 1e-4;
    let tris: Vec<u32> = m
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| t.iter().any(|&i| hairy(i)))
        .flatten()
        .copied()
        .collect();
    let mut out = Mesh::default();
    for k in 1..=layers {
        let t = k as f32 / layers as f32;
        let vertices = m
            .vertices
            .iter()
            .zip(&fur)
            .map(|(v, &(len, comb))| {
                let mut v = *v;
                let n = Vec3::from(v.normal);
                let p = Vec3::from(v.position) + n * (len * t) + comb * (len * t * t);
                v.position = p.to_array();
                let shade = lerp(0.55, 1.0, t.sqrt());
                v.color = [
                    v.color[0] * shade,
                    v.color[1] * shade,
                    v.color[2] * shade,
                    if len > 1e-4 { cutoff / t } else { 0.0 },
                ];
                v
            })
            .collect();
        out.merge(&Mesh::new(vertices, tris.clone()));
    }
    out
}

/// Linear RGB(A) from an sRGB hex colour.
pub fn rgb(hex: u32) -> Vec4 {
    let c = engine_render::material::srgb_hex_to_linear(hex);
    Vec4::new(c[0], c[1], c[2], 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outward(m: &Mesh, center: Vec3) -> usize {
        let mut bad = 0;
        for tri in m.indices.as_chunks::<3>().0 {
            let p: Vec<Vec3> = tri
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].position))
                .collect();
            let n: Vec3 = tri
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                .sum();
            let g = (p[1] - p[0]).cross(p[2] - p[0]);
            let mid = (p[0] + p[1] + p[2]) / 3.0;
            if g.dot(n) < 0.0 || g.dot(mid - center) < 0.0 {
                bad += 1;
            }
        }
        bad
    }

    #[test]
    fn lofts_wind_outward_whichever_way_they_run() {
        for dir in [Vec3::Y, Vec3::NEG_Y, Vec3::NEG_Z, Vec3::Z] {
            let keys = vec![
                key(Vec3::ZERO, 0.1, 0.08),
                key(dir * 0.2, 0.12, 0.1).q2(0.12, 0.08),
                key(dir * 0.4, 0.06, 0.06),
            ];
            let m = Loft::new(keys, 24, 0.02).caps(0.05, 0.05).mesh(white);
            assert!(m.triangle_count() > 300);
            assert_eq!(outward(&m, dir * 0.2), 0, "{dir}");
            let mirrored = mirror_x(m);
            assert_eq!(outward(&mirrored, dir * 0.2), 0, "mirrored {dir}");
        }
        let e = ellipsoid(Vec3::ONE, Vec3::new(0.1, 0.2, 0.3), 32, white);
        assert_eq!(outward(&e, Vec3::ONE), 0);
    }

    #[test]
    fn caps_close_the_tube_smoothly() {
        let m = Loft::new(
            vec![key(Vec3::ZERO, 0.1, 0.1), key(Vec3::Y * 0.3, 0.1, 0.1)],
            32,
            0.02,
        )
        .caps(0.1, 0.1)
        .mesh(white);
        // The poles' normals point along the spine.
        let top = m
            .vertices
            .iter()
            .max_by(|a, b| a.position[1].total_cmp(&b.position[1]))
            .unwrap();
        assert!(Vec3::from(top.normal).dot(Vec3::Y) > 0.99);
        let b = m.aabb();
        assert!((b.max.y - 0.4).abs() < 1e-4 && (b.min.y + 0.1).abs() < 1e-4);
    }
}
