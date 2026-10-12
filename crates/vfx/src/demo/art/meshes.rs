//! Mesh building blocks for props. All sizes in meters; UVs are in meters too (one texture
//! repeat per meter at `uv_scale` 1), so materials tile at the same scale on every prop.
//!
//! The workhorse is [`grid`]: a sheet of points (rows stacked along a profile, columns around
//! it) turned into triangles with smooth, area-weighted normals. Repeating a row or a column
//! makes a hard crease there. On top of it:
//!
//! - [`revolve`] spins a [`Profile`] around +Y (round plinths, basins, bowls, moldings).
//! - [`sweep`] runs a profile around a convex [`Outline`] (square pedestals with moldings,
//!   beveled blocks, flagstones, tapered shafts).
//! - [`slab`] and [`block`] are beveled prisms: every edge rounded so it catches light.
//! - [`tube`] sweeps a circle along a path (iron bars, bands, branches, rope).
//! - [`warp`] and [`noise_warp`] bend meshes while keeping normals right; [`rock`] makes
//!   weathered boulders.
//! - Vertex paint ([`paint`], [`grime`], [`cavity`], [`mottle`]) bakes dirt and wear into
//!   vertex colors, which multiply the material.
//!
//! More specialized pieces live in submodules: stone ([`stone`]: paving, arches, glyphs),
//! nature ([`nature`]: rocks, leaves, moss) and metal ([`metal`]: rivets, bands, chains).

pub mod metal;
pub mod nature;
pub mod stone;

use std::collections::HashMap;

use engine_core::math::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4, noise};
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

// ------------------------------------------------------------------------------------------
// Grid surfaces

/// Triangulate a sheet of points: `rows[v][u]`, every row the same length. Front faces point
/// along `d_v × d_u` (for rows climbing a profile and columns running counter-clockwise seen
/// from above, that is outward). With `wrap`, each row closes back on its first point.
///
/// Normals are smooth (area-weighted); a repeated row or column makes a hard crease, and a
/// row collapsed to one point (a pole) gets one shared normal. Degenerate triangles are
/// dropped. UVs are meters along the rows (U) and along the columns (V).
pub fn grid(rows: &[Vec<Vec3>], wrap: bool) -> Mesh {
    if rows.len() < 2 || rows[0].len() < 2 {
        return Mesh::default();
    }
    let rows = &support_poles(rows);
    let nr = rows.len();
    let n0 = rows[0].len();
    let nc = if wrap { n0 + 1 } else { n0 };
    let mut pos = Vec::with_capacity(nr * nc);
    for row in rows {
        debug_assert_eq!(
            row.len(),
            n0,
            "every grid row needs the same number of points"
        );
        pos.extend_from_slice(&row[..n0]);
        if wrap {
            pos.push(row[0]);
        }
    }
    let mut uv = vec![Vec2::ZERO; nr * nc];
    for r in 0..nr {
        let mut acc = 0.0;
        for c in 1..nc {
            acc += pos[r * nc + c].distance(pos[r * nc + c - 1]);
            uv[r * nc + c].x = acc;
        }
    }
    for c in 0..nc {
        let mut acc = 0.0;
        for r in 1..nr {
            acc += pos[r * nc + c].distance(pos[(r - 1) * nc + c]);
            uv[r * nc + c].y = acc;
        }
    }
    let mut normals = vec![Vec3::ZERO; nr * nc];
    let mut indices = Vec::with_capacity((nr - 1) * (nc - 1) * 6);
    for r in 0..nr - 1 {
        for c in 0..nc - 1 {
            let a = r * nc + c;
            let b = a + nc;
            for tri in [[a, b, a + 1], [a + 1, b, b + 1]] {
                let n = (pos[tri[1]] - pos[tri[0]]).cross(pos[tri[2]] - pos[tri[0]]);
                if n.length_squared() < 1e-18 {
                    continue;
                }
                for &i in &tri {
                    normals[i] += n;
                    indices.push(i as u32);
                }
            }
        }
    }
    if wrap {
        for r in 0..nr {
            let (a, b) = (r * nc, r * nc + nc - 1);
            let s = normals[a] + normals[b];
            normals[a] = s;
            normals[b] = s;
        }
    }
    for r in 0..nr {
        if is_pole(&pos[r * nc..(r + 1) * nc]) {
            let s: Vec3 = normals[r * nc..(r + 1) * nc].iter().copied().sum();
            normals[r * nc..(r + 1) * nc].fill(s);
        }
    }
    let vertices = pos
        .iter()
        .zip(&normals)
        .zip(&uv)
        .map(|((p, n), t)| Vertex::new(*p, n.normalize_or(Vec3::Y), *t))
        .collect();
    Mesh::new(vertices, indices)
}

fn is_pole(row: &[Vec3]) -> bool {
    row.iter().all(|p| p.distance_squared(row[0]) < 1e-12)
}

/// Caps close on a pole: a fan of long thin triangles from the rim to the center. Its rim
/// vertices carry the edge's normal and curvature, which a fan smears across the whole cap
/// in a star. A support ring just inside the rim keeps that to a narrow band.
fn support_poles(rows: &[Vec<Vec3>]) -> Vec<Vec<Vec3>> {
    let mut out: Vec<Vec<Vec3>> = Vec::with_capacity(rows.len() + 2);
    let ring = |rim: &[Vec3], pole: &[Vec3]| -> Vec<Vec3> {
        rim.iter()
            .zip(pole)
            .map(|(r, p)| {
                let d = r.distance(*p);
                if d < 1e-6 {
                    return *r;
                }
                let inset = (0.01 + 0.03 * d).min(0.25 * d);
                r.lerp(*p, inset / d)
            })
            .collect()
    };
    for (i, row) in rows.iter().enumerate() {
        if i > 0 && is_pole(row) && !is_pole(&rows[i - 1]) {
            out.push(ring(&rows[i - 1], row));
        }
        out.push(row.clone());
        if i + 1 < rows.len() && is_pole(row) && !is_pole(&rows[i + 1]) {
            out.push(ring(&rows[i + 1], row));
        }
    }
    out
}

/// One point of a [`Profile`]: `o` is the outward offset (the radius for [`revolve`]), `y`
/// the height and `s` the scale of the outline it is offset from (for [`sweep`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub o: f32,
    pub y: f32,
    pub s: f32,
}

/// A cross-section walked bottom to top with the solid on the left: going up the outside,
/// inward across the top. Built with [`Profile::to`] (smooth), [`Profile::corner`] (hard
/// crease) and [`Profile::arc`].
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub pts: Vec<Pt>,
    scale: Option<f32>,
}

impl Profile {
    pub fn new() -> Self {
        Self::default()
    }

    /// Outline scale for the points added after this (1 by default). Tapers sweeps; a scale
    /// of 0 with offset 0 is the outline's center (a cap's pole).
    pub fn scale(mut self, s: f32) -> Self {
        self.scale = Some(s);
        self
    }

    fn push(&mut self, o: f32, y: f32) {
        let p = Pt {
            o,
            y,
            s: self.scale.unwrap_or(1.0),
        };
        let n = self.pts.len();
        // A third copy of the same point adds nothing.
        if n >= 2 && self.pts[n - 1] == p && self.pts[n - 2] == p {
            return;
        }
        self.pts.push(p);
    }

    /// A smooth point.
    pub fn to(mut self, o: f32, y: f32) -> Self {
        self.push(o, y);
        self
    }

    /// A hard crease.
    pub fn corner(mut self, o: f32, y: f32) -> Self {
        self.push(o, y);
        self.push(o, y);
        self
    }

    /// Smooth points along an arc around (`co`, `cy`) of radius `r`, from angle `a0` to `a1`
    /// in degrees (0 = outward, 90 = up), in `segs` segments, both ends included.
    pub fn arc(mut self, co: f32, cy: f32, r: f32, a0: f32, a1: f32, segs: u32) -> Self {
        let segs = segs.max(1);
        for i in 0..=segs {
            let a = (a0 + (a1 - a0) * i as f32 / segs as f32).to_radians();
            self.push(co + r * a.cos(), cy + r * a.sin());
        }
        self
    }

    /// The highest point.
    pub fn top(&self) -> f32 {
        self.pts.iter().map(|p| p.y).fold(f32::MIN, f32::max)
    }
}

/// Revolve `profile` around +Y in `segments` steps (offsets are radii).
pub fn revolve(profile: &Profile, segments: u32) -> Mesh {
    let segments = segments.max(3);
    let rows: Vec<Vec<Vec3>> = profile
        .pts
        .iter()
        .map(|p| {
            (0..segments)
                .map(|i| {
                    let a = i as f32 / segments as f32 * std::f32::consts::TAU;
                    Vec3::new(p.o * a.cos(), p.y, p.o * a.sin())
                })
                .collect()
        })
        .collect();
    grid(&rows, true)
}

/// A height field over the box `min..max` on the XZ plane, `cell` meters per quad, facing
/// up: `height(x, z)` gives Y. UVs are X and Z in meters.
pub fn heightfield(min: Vec2, max: Vec2, cell: f32, height: impl Fn(Vec2) -> f32) -> Mesh {
    let nx = ((max.x - min.x) / cell).ceil().max(1.0) as usize;
    let nz = ((max.y - min.y) / cell).ceil().max(1.0) as usize;
    let rows: Vec<Vec<Vec3>> = (0..=nz)
        .map(|j| {
            let z = min.y + (max.y - min.y) * j as f32 / nz as f32;
            (0..=nx)
                .map(|i| {
                    let x = min.x + (max.x - min.x) * i as f32 / nx as f32;
                    Vec3::new(x, height(Vec2::new(x, z)), z)
                })
                .collect()
        })
        .collect();
    let mut m = grid(&rows, false);
    for v in &mut m.vertices {
        v.uv = [v.position[0], v.position[2]];
    }
    m
}

/// Revolve `profile` around +Y from angle `a0` to `a1` (radians, a1 > a0) in `segments`
/// steps, leaving the ends open (an arch molding, a basin section).
pub fn revolve_part(profile: &Profile, a0: f32, a1: f32, segments: u32) -> Mesh {
    let segments = segments.max(1);
    let rows: Vec<Vec<Vec3>> = profile
        .pts
        .iter()
        .map(|p| {
            (0..=segments)
                .map(|i| {
                    let a = a0 + (a1 - a0) * i as f32 / segments as f32;
                    Vec3::new(p.o * a.cos(), p.y, p.o * a.sin())
                })
                .collect()
        })
        .collect();
    grid(&rows, false)
}

/// A closed polygon on the XZ plane around the origin, counter-clockwise by angle
/// (`atan2(z, x)` increasing), with a rounding per corner (0 = sharp miter, n = an arc of n
/// segments whose radius is the profile's offset).
#[derive(Debug, Clone)]
pub struct Outline {
    pub pts: Vec<Vec2>,
    pub round: Vec<u32>,
    /// Longest straight piece along an edge (0 = no extra points), for meshes that bend later.
    pub max_edge: f32,
}

fn perp(d: Vec2) -> Vec2 {
    Vec2::new(d.y, -d.x).normalize_or(Vec2::X)
}

impl Outline {
    /// A polygon with sharp corners. Points may come in either order.
    pub fn new(mut pts: Vec<Vec2>) -> Self {
        let n = pts.len();
        let area: f32 = (0..n)
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                a.x * b.y - b.x * a.y
            })
            .sum();
        if area < 0.0 {
            pts.reverse();
        }
        Self {
            round: vec![0; n],
            pts,
            max_edge: 0.0,
        }
    }

    /// A rectangle `2·hx` by `2·hz`.
    pub fn rect(hx: f32, hz: f32) -> Self {
        Self::new(vec![
            Vec2::new(hx, hz),
            Vec2::new(-hx, hz),
            Vec2::new(-hx, -hz),
            Vec2::new(hx, -hz),
        ])
    }

    /// A regular polygon of `n` corners on a circle of `radius`, rotated by `phase` radians.
    pub fn regular(n: u32, radius: f32, phase: f32) -> Self {
        Self::new(
            (0..n)
                .map(|i| {
                    let a = phase + i as f32 / n as f32 * std::f32::consts::TAU;
                    Vec2::new(a.cos(), a.sin()) * radius
                })
                .collect(),
        )
    }

    /// Round every corner with `segs` arc segments.
    pub fn rounded(mut self, segs: u32) -> Self {
        self.round.fill(segs);
        self
    }

    /// Add points along edges so no piece is longer than `max_edge`.
    pub fn detail(mut self, max_edge: f32) -> Self {
        self.max_edge = max_edge;
        self
    }

    /// The polygon moved inward by `d` (edges parallel; for convex outlines).
    pub fn inset(&self, d: f32) -> Outline {
        let n = self.pts.len();
        let pts = (0..n)
            .map(|i| {
                let prev = self.pts[(i + n - 1) % n];
                let next = self.pts[(i + 1) % n];
                let c = self.pts[i];
                let n_in = perp(c - prev);
                let n_out = perp(next - c);
                let miter = (n_in + n_out) / (1.0 + n_in.dot(n_out)).max(0.2);
                c - miter * d
            })
            .collect();
        Outline {
            pts,
            round: self.round.clone(),
            max_edge: self.max_edge,
        }
    }

    /// The ring of points for profile point `p` (one per column, same count for every `p`).
    pub fn ring(&self, p: Pt) -> Vec<Vec3> {
        let n = self.pts.len();
        let mut ring = Vec::new();
        let at = |q: Vec2| Vec3::new(q.x, p.y, q.y);
        for i in 0..n {
            let prev = self.pts[(i + n - 1) % n];
            let c = self.pts[i];
            let next = self.pts[(i + 1) % n];
            let n_in = perp(c - prev);
            let n_out = perp(next - c);
            let k = self.round[i];
            if k == 0 {
                let miter = (n_in + n_out) / (1.0 + n_in.dot(n_out)).max(0.2);
                let q = at(c * p.s + miter * p.o);
                ring.push(q);
                ring.push(q);
            } else {
                let a0 = n_in.y.atan2(n_in.x);
                let mut da = n_out.y.atan2(n_out.x) - a0;
                while da > std::f32::consts::PI {
                    da -= std::f32::consts::TAU;
                }
                while da < -std::f32::consts::PI {
                    da += std::f32::consts::TAU;
                }
                for j in 0..=k {
                    let a = a0 + da * j as f32 / k as f32;
                    ring.push(at(c * p.s + Vec2::new(a.cos(), a.sin()) * p.o));
                }
            }
            if self.max_edge > 0.0 {
                let len = c.distance(next);
                let pieces = (len / self.max_edge).ceil().max(1.0) as u32;
                for j in 1..pieces {
                    let t = j as f32 / pieces as f32;
                    ring.push(at(c.lerp(next, t) * p.s + n_out * p.o));
                }
            }
        }
        ring
    }
}

/// Run `profile` around `outline`: each profile point becomes a ring offset `o` outward from
/// the outline scaled by `s`, at height `y`.
pub fn sweep(outline: &Outline, profile: &Profile) -> Mesh {
    let rows: Vec<Vec<Vec3>> = profile.pts.iter().map(|p| outline.ring(*p)).collect();
    grid(&rows, true)
}

/// A beveled prism: a convex `outline` (outer size) extruded from `y0` to `y1` with every edge
/// rounded by `bevel` in `segs` steps.
#[derive(Debug, Clone)]
pub struct Slab {
    pub outline: Outline,
    pub y0: f32,
    pub y1: f32,
    pub bevel: f32,
    pub segs: u32,
    /// Outline scale at the top (1 = straight sides).
    pub taper: f32,
    /// Close the bottom (off for things sunk into the ground).
    pub bottom: bool,
    /// Extra rows up the sides and rings across the top, for meshes that bend later.
    pub detail: u32,
}

impl Slab {
    pub fn new(outline: Outline, y0: f32, y1: f32, bevel: f32) -> Self {
        Self {
            outline,
            y0,
            y1,
            bevel,
            segs: 2,
            taper: 1.0,
            bottom: true,
            detail: 0,
        }
    }

    pub fn segs(mut self, segs: u32) -> Self {
        self.segs = segs;
        self
    }

    pub fn taper(mut self, taper: f32) -> Self {
        self.taper = taper;
        self
    }

    pub fn open_bottom(mut self) -> Self {
        self.bottom = false;
        self
    }

    pub fn detail(mut self, detail: u32) -> Self {
        self.detail = detail;
        self
    }

    pub fn mesh(&self) -> Mesh {
        // Build around the outline's middle (caps close on it and tapers shrink toward it),
        // then move back.
        let n = self.outline.pts.len().max(1) as f32;
        let mid = self.outline.pts.iter().copied().sum::<Vec2>() / n;
        if mid.length_squared() > 1e-10 {
            let mut centered = self.clone();
            for p in &mut centered.outline.pts {
                *p -= mid;
            }
            return moved(
                centered.mesh(),
                Vec3::new(mid.x, 0.0, mid.y),
                Quat::IDENTITY,
            );
        }
        let r = self.bevel.min((self.y1 - self.y0) * 0.5).max(0.0);
        let segs = self.segs.max(1);
        let core = self.outline.inset(r).rounded(segs);
        let (y0, y1) = (self.y0, self.y1);
        let h = (y1 - y0).max(1e-6);
        let scale = |y: f32| 1.0 + (self.taper - 1.0) * ((y - y0) / h);
        let mut prof = Profile::new();
        let add = |prof: Profile, o: f32, y: f32| prof.scale(scale(y)).to(o, y);
        if self.bottom {
            for i in 0..=self.detail {
                let s = i as f32 / (self.detail + 1) as f32;
                prof = prof.scale(s * scale(y0)).to(0.0, y0);
            }
            for i in 0..=segs {
                let a = (-90.0 + 90.0 * i as f32 / segs as f32).to_radians();
                prof = add(prof, r * a.cos(), y0 + r + r * a.sin());
            }
        } else {
            prof = add(prof, r, y0);
        }
        for i in 1..=self.detail {
            let y = (y0 + r) + (y1 - y0 - 2.0 * r) * i as f32 / (self.detail + 1) as f32;
            prof = add(prof, r, y);
        }
        for i in 0..=segs {
            let a = (90.0 * i as f32 / segs as f32).to_radians();
            prof = add(prof, r * a.cos(), y1 - r + r * a.sin());
        }
        for i in (0..=self.detail).rev() {
            let s = i as f32 / (self.detail + 1) as f32;
            prof = prof.scale(s * scale(y1)).to(0.0, y1);
        }
        sweep(&core, &prof)
    }
}

/// Shorthand for a beveled prism from `outline`, `y0` to `y1`.
pub fn slab(outline: Outline, y0: f32, y1: f32, bevel: f32) -> Mesh {
    Slab::new(outline, y0, y1, bevel).mesh()
}

/// A box centered on the origin with every edge rounded by `bevel`.
pub fn block(size: Vec3, bevel: f32) -> Mesh {
    let h = size * 0.5;
    slab(Outline::rect(h.x, h.z), -h.y, h.y, bevel)
}

/// A circle swept along `path` (open), `radius(t)` for t in 0..1 along it, `sides` around.
/// With `caps` both ends are closed.
pub fn tube(path: &[Vec3], radius: impl Fn(f32) -> f32, sides: u32, caps: bool) -> Mesh {
    let n = path.len();
    if n < 2 {
        return Mesh::default();
    }
    let sides = sides.max(3);
    let tangent = |i: usize| {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        (b - a).normalize_or(Vec3::Y)
    };
    // Parallel-transported frame, so the tube does not twist.
    let t0 = tangent(0);
    let mut normal = t0.any_orthonormal_vector();
    let mut rows = Vec::with_capacity(n + 2);
    let mut lengths = vec![0.0f32; n];
    for i in 1..n {
        lengths[i] = lengths[i - 1] + path[i].distance(path[i - 1]);
    }
    let total = lengths[n - 1].max(1e-6);
    let mut prev_t = t0;
    let ring = |c: Vec3, t: Vec3, nrm: Vec3, r: f32| -> Vec<Vec3> {
        let b = t.cross(nrm);
        (0..sides)
            .map(|k| {
                let a = k as f32 / sides as f32 * std::f32::consts::TAU;
                c + (b * a.cos() + nrm * a.sin()) * r
            })
            .collect()
    };
    if caps {
        rows.push(vec![path[0]; sides as usize]);
    }
    for i in 0..n {
        let t = tangent(i);
        let q = Quat::from_rotation_arc(prev_t, t);
        normal = (q * normal).normalize_or(t.any_orthonormal_vector());
        prev_t = t;
        let r = radius(lengths[i] / total);
        rows.push(ring(path[i], t, normal, r));
    }
    if caps {
        rows.push(vec![path[n - 1]; sides as usize]);
    }
    // Caps need a hard crease at the rim: repeat the first and last rings.
    if caps {
        let first = rows[1].clone();
        let last = rows[rows.len() - 2].clone();
        rows.insert(1, first);
        let at = rows.len() - 1;
        rows.insert(at, last);
    }
    grid(&rows, true)
}

/// A closed loop of tube (a ring, a chain link): `path` goes once around without repeating
/// its first point; the cross-section is an ellipse `radius` (x: across the loop's plane
/// normal, y: along it).
pub fn tube_loop(path: &[Vec3], radius: Vec2, sides: u32) -> Mesh {
    let n = path.len();
    if n < 3 {
        return Mesh::default();
    }
    let sides = sides.max(3);
    // The loop's plane normal (Newell's method) keeps every ring's frame consistent.
    let mut up = Vec3::ZERO;
    for i in 0..n {
        let (a, b) = (path[i], path[(i + 1) % n]);
        up += Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    let up = up.normalize_or(Vec3::Y);
    let rows: Vec<Vec<Vec3>> = (0..=n)
        .map(|i| {
            let c = path[i % n];
            let t = (path[(i + 1) % n] - path[(i + n - 1) % n]).normalize_or(Vec3::X);
            let side = t.cross(up).normalize_or(Vec3::X);
            let nrm = side.cross(t);
            (0..sides)
                .map(|k| {
                    let a = k as f32 / sides as f32 * std::f32::consts::TAU;
                    c + side * (a.cos() * radius.x) + nrm * (a.sin() * radius.y)
                })
                .collect()
        })
        .collect();
    let mut m = grid(&rows, true);
    // Weld the seam where the loop closes.
    let nc = sides as usize + 1;
    let last = n * nc;
    for c in 0..nc {
        let s = Vec3::from(m.vertices[c].normal) + Vec3::from(m.vertices[last + c].normal);
        let s = s.normalize_or(Vec3::Y).to_array();
        m.vertices[c].normal = s;
        m.vertices[last + c].normal = s;
    }
    m
}

/// Unit icosphere: 20 · 4^`subdiv` triangles, shared vertices.
pub fn icosphere(subdiv: u32) -> Mesh {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let mut pos: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from_array(*p).normalize())
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdiv {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        let mut midpoint = |a: u32, b: u32, pos: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *mid.entry(key).or_insert_with(|| {
                pos.push(((pos[a as usize] + pos[b as usize]) * 0.5).normalize());
                pos.len() as u32 - 1
            })
        };
        for [a, b, c] in tris {
            let ab = midpoint(a, b, &mut pos);
            let bc = midpoint(b, c, &mut pos);
            let ca = midpoint(c, a, &mut pos);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    let vertices = pos
        .iter()
        .map(|p| Vertex::new(*p, *p, Vec2::ZERO))
        .collect();
    // The table above winds counter-clockwise seen from outside.
    let indices = tris.iter().flat_map(|t| *t).collect();
    Mesh::new(vertices, indices)
}

// ------------------------------------------------------------------------------------------
// Transforms and deformation

/// `mesh` moved by translation `t`, rotation `r` and scale `s` (scale first).
pub fn placed(mut mesh: Mesh, t: Vec3, r: Quat, s: Vec3) -> Mesh {
    mesh.transform(&Mat4::from_scale_rotation_translation(s, r, t));
    mesh
}

/// `mesh` moved by `t` and turned by `r`.
pub fn moved(mesh: Mesh, t: Vec3, r: Quat) -> Mesh {
    placed(mesh, t, r, Vec3::ONE)
}

/// Bend every vertex through `f`, carrying normals along with the local Jacobian (so creases
/// stay crisp and smooth parts stay smooth, unlike recomputing normals).
pub fn warp(mesh: &mut Mesh, f: impl Fn(Vec3) -> Vec3) {
    let h = 1e-3;
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        let dx = (f(p + Vec3::X * h) - f(p - Vec3::X * h)) / (2.0 * h);
        let dy = (f(p + Vec3::Y * h) - f(p - Vec3::Y * h)) / (2.0 * h);
        let dz = (f(p + Vec3::Z * h) - f(p - Vec3::Z * h)) / (2.0 * h);
        let j = Mat3::from_cols(dx, dy, dz);
        let nn = if j.determinant().abs() > 1e-8 {
            (j.inverse().transpose() * n).normalize_or(n)
        } else {
            n
        };
        v.position = f(p).to_array();
        v.normal = nn.to_array();
    }
}

/// A smooth 3D vector noise field, about `[-1, 1]` per axis.
pub fn noise3(p: Vec3, freq: f32, seed: u64, octaves: u32) -> Vec3 {
    let q = p.as_dvec3() * freq as f64;
    Vec3::new(
        noise::fbm3(q.x, q.y, q.z, seed, octaves, 2.0, 0.5),
        noise::fbm3(
            q.x + 31.7,
            q.y,
            q.z,
            seed.wrapping_add(1),
            octaves,
            2.0,
            0.5,
        ),
        noise::fbm3(
            q.x,
            q.y - 17.3,
            q.z,
            seed.wrapping_add(2),
            octaves,
            2.0,
            0.5,
        ),
    )
}

/// Scalar fbm noise at `p`, about `[-1, 1]`.
pub fn noise1(p: Vec3, freq: f32, seed: u64, octaves: u32) -> f32 {
    let q = p.as_dvec3() * freq as f64;
    noise::fbm3(q.x, q.y, q.z, seed, octaves, 2.0, 0.5)
}

/// Wobble a mesh by `amp` meters of smooth noise at `freq` cycles per meter: hand-cut stone,
/// a sagging plank, a lumpy rock.
pub fn noise_warp(mesh: &mut Mesh, amp: f32, freq: f32, seed: u64) {
    warp(mesh, |p| p + noise3(p, freq, seed, 2) * amp);
}

/// Hard normals (every triangle flat).
pub fn faceted(mut mesh: Mesh) -> Mesh {
    mesh.compute_flat_normals();
    mesh
}

/// Normals smooth across gentle bends and hard across edges sharper than `angle` degrees
/// (fracture faces of a rock, chipped stone). Every triangle gets its own three vertices.
pub fn auto_smooth(mesh: &Mesh, angle: f32) -> Mesh {
    let key = |p: Vec3| {
        let q = (p * 4000.0).round();
        (q.x as i64, q.y as i64, q.z as i64)
    };
    let tris = mesh.indices.as_chunks::<3>().0;
    let pos = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
    // Area-weighted face normals, and the faces around each position.
    let faces: Vec<Vec3> = tris
        .iter()
        .map(|t| (pos(t[1]) - pos(t[0])).cross(pos(t[2]) - pos(t[0])))
        .collect();
    let mut around: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    for (f, t) in tris.iter().enumerate() {
        for &i in t {
            around.entry(key(pos(i))).or_default().push(f);
        }
    }
    let cos = angle.to_radians().cos();
    let mut out = Mesh::default();
    for (f, t) in tris.iter().enumerate() {
        let own = faces[f].normalize_or(Vec3::Y);
        for &i in t {
            let n: Vec3 = around[&key(pos(i))]
                .iter()
                .filter(|&&g| faces[g].normalize_or(Vec3::Y).dot(own) >= cos)
                .map(|&g| faces[g])
                .sum();
            let mut v = mesh.vertices[i as usize];
            v.normal = n.normalize_or(own).to_array();
            out.indices.push(out.vertices.len() as u32);
            out.vertices.push(v);
        }
    }
    out
}

/// Append a copy with reversed winding and normals, so thin things (leaves, blades) show from
/// both sides on single-sided materials.
pub fn two_sided(mut mesh: Mesh) -> Mesh {
    let n = mesh.vertices.len() as u32;
    let mut back: Vec<Vertex> = mesh.vertices.clone();
    for v in &mut back {
        v.normal = (-Vec3::from(v.normal)).to_array();
    }
    mesh.vertices.extend(back);
    let flipped: Vec<u32> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|t| [t[0] + n, t[2] + n, t[1] + n])
        .collect();
    mesh.indices.extend(flipped);
    mesh
}

/// Append a copy with reversed winding but the same normals, so both faces of a thin blade
/// are lit alike (grass).
pub fn double_faced(mut mesh: Mesh) -> Mesh {
    let n = mesh.vertices.len() as u32;
    let back = mesh.vertices.clone();
    mesh.vertices.extend(back);
    let flipped: Vec<u32> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|t| [t[0] + n, t[2] + n, t[1] + n])
        .collect();
    mesh.indices.extend(flipped);
    mesh
}

/// Merge several meshes into one.
pub fn merged(meshes: impl IntoIterator<Item = Mesh>) -> Mesh {
    let mut out = Mesh::default();
    for m in meshes {
        out.merge(&m);
    }
    out
}

// ------------------------------------------------------------------------------------------
// UVs

/// Box-project UVs in meters: each vertex takes the two axes across its dominant normal
/// axis, shifted by `offset` (vary it per piece so neighbors don't share texture).
pub fn uv_box(mesh: &mut Mesh, offset: Vec2) {
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal).abs();
        let uv = if n.y >= n.x && n.y >= n.z {
            Vec2::new(p.x, p.z)
        } else if n.x >= n.z {
            Vec2::new(p.z, -p.y)
        } else {
            Vec2::new(p.x, -p.y)
        };
        v.uv = (uv + offset).to_array();
    }
}

/// Shift and rotate a mesh's UVs (radians), so copies of a texture don't line up.
pub fn uv_shift(mesh: &mut Mesh, offset: Vec2, angle: f32) {
    let (s, c) = angle.sin_cos();
    for v in &mut mesh.vertices {
        let u = Vec2::from(v.uv);
        v.uv = (Vec2::new(u.x * c - u.y * s, u.x * s + u.y * c) + offset).to_array();
    }
}

// ------------------------------------------------------------------------------------------
// Vertex paint (multiplies the material's base color)

/// Multiply each vertex color by `f(position, normal)`, an RGB factor.
pub fn paint(mesh: &mut Mesh, f: impl Fn(Vec3, Vec3) -> Vec3) {
    for v in &mut mesh.vertices {
        let k = f(Vec3::from(v.position), Vec3::from(v.normal));
        v.color[0] *= k.x;
        v.color[1] *= k.y;
        v.color[2] *= k.z;
    }
}

/// Multiply every vertex color by `rgb`.
pub fn tint(mesh: &mut Mesh, rgb: Vec3) {
    paint(mesh, |_, _| rgb);
}

/// Contact grime: darken toward `dark` within `height` meters above `ground`.
pub fn grime(mesh: &mut Mesh, ground: f32, height: f32, dark: f32) {
    paint(mesh, |p, _| {
        let t = engine_core::math::smoothstep(ground, ground + height.max(1e-4), p.y);
        Vec3::splat(dark + (1.0 - dark) * t)
    });
}

/// Brightness mottling: `1 ± amp` of noise at `freq` cycles per meter, warm/cool shifted.
pub fn mottle(mesh: &mut Mesh, freq: f32, amp: f32, seed: u64) {
    paint(mesh, |p, _| {
        let n = noise1(p, freq, seed, 3);
        let h = noise1(p, freq * 0.5, seed.wrapping_add(9), 2);
        Vec3::new(1.0 + 0.04 * h, 1.0, 1.0 - 0.04 * h) * (1.0 + amp * n)
    });
}

/// Bake curvature: concave spots darken by up to `dark`, convex edges lighten by up to
/// `light` (worn edges catch dust and wear; cavities hold dirt). `scale` sets how sharp a
/// bend counts as full strength.
pub fn cavity(mesh: &mut Mesh, dark: f32, light: f32, scale: f32) {
    let key = |p: Vec3| {
        let q = (p * 2000.0).round();
        (q.x as i64, q.y as i64, q.z as i64)
    };
    let mut ids: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut weld = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let n = ids.len();
        weld.push(*ids.entry(key(Vec3::from(v.position))).or_insert(n));
    }
    let count = ids.len();
    let mut pos = vec![Vec3::ZERO; count];
    let mut nrm = vec![Vec3::ZERO; count];
    for (v, &w) in mesh.vertices.iter().zip(&weld) {
        pos[w] = Vec3::from(v.position);
        nrm[w] += Vec3::from(v.normal);
    }
    let mut sum = vec![Vec3::ZERO; count];
    let mut deg = vec![0.0f32; count];
    let mut len = vec![0.0f32; count];
    for t in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (a, b) = (weld[a as usize], weld[b as usize]);
            if a == b {
                continue;
            }
            let d = pos[a].distance(pos[b]);
            sum[a] += pos[b];
            sum[b] += pos[a];
            deg[a] += 1.0;
            deg[b] += 1.0;
            len[a] += d;
            len[b] += d;
        }
    }
    let curv: Vec<f32> = (0..count)
        .map(|i| {
            if deg[i] == 0.0 {
                return 0.0;
            }
            let avg = sum[i] / deg[i];
            let edge = (len[i] / deg[i]).max(1e-5);
            nrm[i].normalize_or(Vec3::Y).dot(pos[i] - avg) / edge * scale
        })
        .collect();
    for (v, &w) in mesh.vertices.iter_mut().zip(&weld) {
        let c = curv[w].clamp(-1.0, 1.0);
        let k = if c < 0.0 {
            1.0 + c * dark
        } else {
            1.0 + c * light
        };
        for ch in &mut v.color[..3] {
            *ch *= k;
        }
    }
}

/// Set every vertex color to `rgb` (alpha 1).
pub fn colored(mut mesh: Mesh, rgb: Vec3) -> Mesh {
    for v in &mut mesh.vertices {
        v.color = Vec4::new(rgb.x, rgb.y, rgb.z, 1.0).to_array();
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every triangle's winding agrees with its vertex normals, and nothing is NaN.
    pub(crate) fn assert_sane(m: &Mesh, what: &str) {
        assert!(!m.indices.is_empty(), "{what}: empty");
        let mut bad = 0;
        for v in &m.vertices {
            assert!(
                v.position.iter().chain(&v.normal).all(|x| x.is_finite()),
                "{what}: NaN"
            );
        }
        for t in m.indices.as_chunks::<3>().0 {
            let p: Vec<Vec3> = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].position))
                .collect();
            let n: Vec3 = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                .sum();
            let g = (p[1] - p[0]).cross(p[2] - p[0]);
            if g.length_squared() > 1e-14 && g.dot(n) < 0.0 {
                bad += 1;
            }
        }
        let share = bad as f32 / m.triangle_count() as f32;
        assert!(
            share < 0.01,
            "{what}: {bad} triangles face against their normals"
        );
    }

    #[test]
    fn building_blocks_face_outward() {
        let prof = Profile::new()
            .to(0.0, 0.0)
            .corner(1.0, 0.0)
            .arc(0.9, 0.5, 0.1, 0.0, 90.0, 4)
            .corner(0.5, 1.0)
            .to(0.0, 1.0);
        assert_sane(&revolve(&prof, 24), "revolve");
        assert_sane(&block(Vec3::new(1.0, 0.5, 0.7), 0.05), "block");
        let s = Slab::new(Outline::regular(5, 0.4, 0.3), 0.0, 0.2, 0.03)
            .taper(0.8)
            .detail(2)
            .mesh();
        assert_sane(&s, "slab");
        let path: Vec<Vec3> = (0..10)
            .map(|i| Vec3::new(i as f32 * 0.1, (i as f32 * 0.5).sin(), 0.0))
            .collect();
        assert_sane(&tube(&path, |t| 0.05 * (1.0 - t * 0.5), 8, true), "tube");
        let mut ico = icosphere(2);
        ico.compute_normals();
        assert_sane(&ico, "icosphere");
        assert!(
            ico.vertices
                .iter()
                .all(|v| Vec3::from(v.normal).dot(Vec3::from(v.position)) > 0.9),
            "icosphere faces inward"
        );
        let r = nature::rock(nature::RockShape::new(Vec3::new(1.0, 0.6, 0.8), 3));
        let c = r.aabb().center();
        let outward = r
            .vertices
            .iter()
            .filter(|v| Vec3::from(v.normal).dot(Vec3::from(v.position) - c) > 0.0)
            .count();
        assert!(outward * 10 > r.vertices.len() * 9, "rock faces inward");
        let mut w = block(Vec3::ONE, 0.1);
        noise_warp(&mut w, 0.02, 3.0, 1);
        assert_sane(&w, "warped block");
    }

    #[test]
    fn block_has_its_size() {
        let b = block(Vec3::new(2.0, 1.0, 0.5), 0.1).aabb();
        assert!(b.max.abs_diff_eq(Vec3::new(1.0, 0.5, 0.25), 1e-4), "{b:?}");
        assert!(
            b.min.abs_diff_eq(Vec3::new(-1.0, -0.5, -0.25), 1e-4),
            "{b:?}"
        );
    }
}
