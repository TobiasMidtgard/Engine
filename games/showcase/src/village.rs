//! The village's buildings and street furniture, built from code as real geometry: half-timbered
//! houses (stone plinths laid stone by stone, chamfered timbers with jetties and joist heads,
//! plaster infill with grime, stepped tile courses with ridge caps, rafter tails under the eaves,
//! brick chimneys, framed and glazed windows with sills, shutters and flower boxes, plank doors
//! with iron hinges and stone steps), wrought-iron street lamps, benches, barrels, wood piles
//! and a hand cart. Materials come from `texgen.rs`.
//!
//! A house archetype ([`HouseSpec`]) is built twice: a near mesh with full detail and a far mesh
//! of a few hundred triangles that the scenery swaps in at a distance. Each is one mesh per
//! material ([`Part`]), shared by every instance of the archetype.
//!
//! Build frame: Y up, the ridge runs along Z, the ground under the house's center is y = 0.
//! Houses whose door is on a long side are built with the door on +X and turned at the end, so
//! every finished house has its door facing +Z.

use crate::texgen::{GLASS_CELLS, TILE_EXPOSURE, TILE_ROWS, TILE_WIDTH, TILES_ACROSS};
use engine::core::math::hash01;
use engine::core::math::noise::perlin2;
use engine::prelude::*;
use engine::render::Vertex;
use std::f32::consts::{FRAC_PI_2, PI};

// =============================================================================== mesh building

/// An axis-aligned rectangle in wall coordinates (u along the wall, y up).
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn new(u0: f32, y0: f32, u1: f32, y1: f32) -> Self {
        Self {
            min: Vec2::new(u0.min(u1), y0.min(y1)),
            max: Vec2::new(u0.max(u1), y0.max(y1)),
        }
    }
    fn contains(&self, p: Vec2) -> bool {
        p.x > self.min.x && p.x < self.max.x && p.y > self.min.y && p.y < self.max.y
    }
    fn poly(&self) -> [Vec2; 4] {
        [
            self.min,
            Vec2::new(self.max.x, self.min.y),
            self.max,
            Vec2::new(self.min.x, self.max.y),
        ]
    }
    fn w(&self) -> f32 {
        self.max.x - self.min.x
    }
    fn h(&self) -> f32 {
        self.max.y - self.min.y
    }
    fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }
    fn shrink(&self, d: f32) -> Rect {
        Rect::new(
            self.min.x + d,
            self.min.y + d,
            self.max.x - d,
            self.max.y - d,
        )
    }
}

/// A vertical wall plane: `at(p, d)` is the point at wall coordinates `p` (u along `u`, y is
/// world height) and `d` meters out along the outward normal `n`.
#[derive(Clone, Copy, Debug)]
struct Wall {
    o: Vec3,
    u: Vec3,
    n: Vec3,
    len: f32,
    /// Horizontal timbers run through the corners (front and back walls) instead of stopping
    /// at the crossing timber (side walls).
    through: bool,
}

impl Wall {
    fn at(&self, p: Vec2, d: f32) -> Vec3 {
        self.o + self.u * p.x + Vec3::Y * p.y + self.n * d
    }
    fn dir(&self, v: Vec2) -> Vec3 {
        self.u * v.x + Vec3::Y * v.y
    }
}

fn area(poly: &[Vec2]) -> f32 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Keep the part of a convex polygon where `n·p >= d`.
fn clip(poly: &[Vec2], n: Vec2, d: f32) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (n.dot(a) - d, n.dot(b) - d);
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out.dedup_by(|a, b| a.distance_squared(*b) < 1e-8);
    if out.len() > 1 && out[0].distance_squared(out[out.len() - 1]) < 1e-8 {
        out.pop();
    }
    out
}

/// Clip a convex polygon to the inside of a convex outline (counter-clockwise).
fn clip_to(poly: &[Vec2], outline: &[Vec2]) -> Vec<Vec2> {
    let mut out = poly.to_vec();
    for i in 0..outline.len() {
        if out.len() < 3 {
            break;
        }
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        let e = b - a;
        let n = Vec2::new(-e.y, e.x);
        out = clip(&out, n, n.dot(a));
    }
    out
}

/// A band of `width` around the segment a→b, as a polygon (to clip with [`clip_to`]).
fn band(a: Vec2, b: Vec2, width: f32) -> [Vec2; 4] {
    let d = (b - a).normalize_or(Vec2::X);
    let n = Vec2::new(-d.y, d.x) * width * 0.5;
    // Extended past both ends so clipping cuts the ends to the members they meet.
    let (a, b) = (a - d * 2.0, b + d * 2.0);
    [a - n, b - n, b + n, a + n]
}

/// Move every edge of a convex polygon inward by `c`.
fn inset(poly: &[Vec2], c: f32) -> Vec<Vec2> {
    let ccw = area(poly) > 0.0;
    let n = poly.len();
    let inward = |e: Vec2| {
        let e = e.normalize_or_zero();
        if ccw {
            Vec2::new(-e.y, e.x)
        } else {
            Vec2::new(e.y, -e.x)
        }
    };
    (0..n)
        .map(|i| {
            let p = poly[i];
            let n1 = inward(p - poly[(i + n - 1) % n]);
            let n2 = inward(poly[(i + 1) % n] - p);
            let d = 1.0 + n1.dot(n2);
            if d > 0.25 {
                p + (n1 + n2) * (c / d)
            } else {
                p + (n1 + n2).normalize_or_zero() * c * 2.0
            }
        })
        .collect()
}

fn hash3(p: Vec3, seed: u64) -> f32 {
    hash01(
        (p.x * 997.0) as i64,
        (p.y * 991.0) as i64,
        (p.z * 983.0) as i64,
        seed,
    )
}

/// A mesh under construction. With `far` set, the same calls build the cheap far version:
/// no chamfers, only the front faces of wall timbers, coarse plaster.
#[derive(Default)]
pub struct Mb {
    pub mesh: Mesh,
    pub far: bool,
}

impl Mb {
    pub fn new(far: bool) -> Self {
        Self {
            mesh: Mesh::default(),
            far,
        }
    }

    fn mark(&self) -> usize {
        self.mesh.vertices.len()
    }

    /// Recolor every vertex added since `start`: `f(position, color) -> color`.
    fn shade_since(&mut self, start: usize, f: impl Fn(Vec3, Vec3) -> Vec3) {
        for v in &mut self.mesh.vertices[start..] {
            let c = f(
                Vec3::from(v.position),
                Vec3::new(v.color[0], v.color[1], v.color[2]),
            );
            v.color = [c.x, c.y, c.z, 1.0];
        }
    }

    /// Polygon (convex, or fan-able from its first point), flat shaded, front toward `facing`.
    fn poly(&mut self, pts: &[Vec3], uvs: &[Vec2], cols: &[Vec3], facing: Vec3) {
        if pts.len() < 3 {
            return;
        }
        let mut n = Vec3::ZERO;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            n += Vec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        if n.length_squared() < 1e-14 {
            return;
        }
        let flip = n.dot(facing) < 0.0;
        let n = if flip { -n } else { n }.normalize();
        let b = self.mesh.vertices.len() as u32;
        for (i, p) in pts.iter().enumerate() {
            let c = cols[i.min(cols.len() - 1)];
            self.mesh
                .vertices
                .push(Vertex::new(*p, n, uvs[i]).with_color(c.extend(1.0)));
        }
        for i in 1..pts.len() as u32 - 1 {
            if flip {
                self.mesh.indices.extend_from_slice(&[b, b + i + 1, b + i]);
            } else {
                self.mesh.indices.extend_from_slice(&[b, b + i, b + i + 1]);
            }
        }
    }

    /// Polygon with texture coordinates projected onto it in meters: U along `grain` (along
    /// `alt` when the face looks down the grain).
    pub(crate) fn face(&mut self, pts: &[Vec3], facing: Vec3, grain: Vec3, alt: Vec3, col: Vec3) {
        let f = facing.normalize_or(Vec3::Y);
        let mut t = grain - f * grain.dot(f);
        if t.length_squared() < 0.04 {
            t = alt - f * alt.dot(f);
        }
        let t = t.normalize_or(f.any_orthonormal_vector());
        let bt = f.cross(t);
        let uvs: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.dot(t), p.dot(bt))).collect();
        self.poly(pts, &uvs, &[col], f);
    }

    /// Box with all edges chamfered by `c` (a plain box when `c` is 0 or in far mode). `ax` are
    /// orthonormal axes, `ax[0]` along the grain, `h` the half extents along them. Bit `2a` of
    /// `skip` leaves out the face on the negative side of axis `a`, bit `2a+1` the positive.
    #[allow(clippy::too_many_arguments)]
    fn cbox(&mut self, center: Vec3, ax: [Vec3; 3], h: Vec3, c: f32, col: Vec3, skip: u8) {
        self.cbox_rough(center, ax, h, c, col, skip, 0.0, 0);
    }

    /// [`Mb::cbox`] whose corners are displaced up to `rough` meters (rough-hewn stones).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cbox_rough(
        &mut self,
        center: Vec3,
        ax: [Vec3; 3],
        h: Vec3,
        c: f32,
        col: Vec3,
        skip: u8,
        rough: f32,
        seed: u64,
    ) {
        let c = if self.far {
            0.0
        } else {
            c.min(h.min_element() * 0.45).max(0.0)
        };
        let pt = |i: usize, s: [f32; 3]| -> Vec3 {
            let mut p = center;
            for m in 0..3 {
                let e = if m == i { h[m] } else { h[m] - c };
                p += ax[m] * e * s[m];
            }
            if rough > 0.0 {
                let k = (i * 8
                    + (s[0] > 0.0) as usize
                    + 2 * (s[1] > 0.0) as usize
                    + 4 * (s[2] > 0.0) as usize) as u64;
                let r = |j: u64| hash01(k as i64, j as i64, 0, seed) * 2.0 - 1.0;
                // Mostly in and out of the face, a little sideways.
                p += ax[i] * r(1) * rough * s[i].signum() * 0.8
                    + (ax[0] * r(2) + ax[1] * r(3) + ax[2] * r(4)) * rough * 0.4;
            }
            p
        };
        let skipped = |a: usize, s: f32| skip & (1 << (2 * a + (s > 0.0) as usize)) != 0;
        let quad_signs = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        for i in 0..3 {
            for si in [-1.0f32, 1.0] {
                if skipped(i, si) {
                    continue;
                }
                let (j, k) = ((i + 1) % 3, (i + 2) % 3);
                let pts: Vec<Vec3> = quad_signs
                    .iter()
                    .map(|&(sj, sk)| {
                        let mut s = [0.0; 3];
                        s[i] = si;
                        s[j] = sj;
                        s[k] = sk;
                        pt(i, s)
                    })
                    .collect();
                self.face(&pts, ax[i] * si, ax[0], ax[1], col);
            }
        }
        if c <= 0.0 {
            return;
        }
        for k in 0..3 {
            let (i, j) = ((k + 1) % 3, (k + 2) % 3);
            for si in [-1.0f32, 1.0] {
                for sj in [-1.0f32, 1.0] {
                    if skipped(i, si) || skipped(j, sj) {
                        continue;
                    }
                    let s = |sk: f32| {
                        let mut s = [0.0; 3];
                        s[i] = si;
                        s[j] = sj;
                        s[k] = sk;
                        s
                    };
                    let pts = [pt(i, s(-1.0)), pt(i, s(1.0)), pt(j, s(1.0)), pt(j, s(-1.0))];
                    self.face(&pts, ax[i] * si + ax[j] * sj, ax[0], ax[1], col);
                }
            }
        }
        for s0 in [-1.0f32, 1.0] {
            for s1 in [-1.0f32, 1.0] {
                for s2 in [-1.0f32, 1.0] {
                    if skipped(0, s0) || skipped(1, s1) || skipped(2, s2) {
                        continue;
                    }
                    let s = [s0, s1, s2];
                    let pts = [pt(0, s), pt(1, s), pt(2, s)];
                    self.face(
                        &pts,
                        ax[0] * s0 + ax[1] * s1 + ax[2] * s2,
                        ax[0],
                        ax[1],
                        col,
                    );
                }
            }
        }
    }

    /// Axis-aligned chamfered box from `min` to `max`, grain along `grain_axis` (0 = X, 1 = Y,
    /// 2 = Z).
    fn abox(&mut self, min: Vec3, max: Vec3, c: f32, grain_axis: usize, col: Vec3) {
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        let ax = [
            axes[grain_axis],
            axes[(grain_axis + 1) % 3],
            axes[(grain_axis + 2) % 3],
        ];
        let h = (max - min) * 0.5;
        let hv = Vec3::new(
            h[grain_axis],
            h[(grain_axis + 1) % 3],
            h[(grain_axis + 2) % 3],
        );
        self.cbox((min + max) * 0.5, ax, hv, c, col, 0);
    }

    /// A wall timber: the convex polygon `poly` (wall coordinates) pushed out of the wall from
    /// depth `back` to `front`, its front edges chamfered by `c`, grain along `grain`. The back
    /// face is left out (it is inside the wall). In far mode only the front face is built.
    #[allow(clippy::too_many_arguments)]
    fn member(
        &mut self,
        w: &Wall,
        poly: &[Vec2],
        back: f32,
        front: f32,
        c: f32,
        grain: Vec2,
        col: Vec3,
    ) {
        if poly.len() < 3 || area(poly).abs() < 1e-5 {
            return;
        }
        let g = w.dir(grain);
        if self.far {
            let pts: Vec<Vec3> = poly.iter().map(|p| w.at(*p, front)).collect();
            self.face(&pts, w.n, g, w.u, col);
            return;
        }
        let ccw = area(poly) > 0.0;
        let c = c.min(front - back - 0.002).max(0.0);
        let inner = inset(poly, c);
        let pts: Vec<Vec3> = inner.iter().map(|p| w.at(*p, front)).collect();
        self.face(&pts, w.n, g, w.u, col);
        let n = poly.len();
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if a.distance_squared(b) < 1e-8 {
                continue;
            }
            let e = (b - a).normalize();
            let out2 = if ccw {
                Vec2::new(e.y, -e.x)
            } else {
                Vec2::new(-e.y, e.x)
            };
            let out = w.dir(out2);
            if c > 0.0 {
                let pts = [
                    w.at(a, front - c),
                    w.at(b, front - c),
                    w.at(inner[(i + 1) % n], front),
                    w.at(inner[i], front),
                ];
                self.face(&pts, out + w.n, g, w.n, col);
            }
            let pts = [
                w.at(a, back),
                w.at(b, back),
                w.at(b, front - c),
                w.at(a, front - c),
            ];
            self.face(&pts, out, g, w.n, col);
        }
    }

    /// Plaster over the convex `outline` (wall coordinates, counter-clockwise) minus
    /// rectangular `holes`, on a grid fine enough for the grime in the vertex colors:
    /// `shade(p)` multiplies `col` at wall point `p`.
    fn plaster(
        &mut self,
        w: &Wall,
        outline: &[Vec2],
        holes: &[Rect],
        shade: &dyn Fn(Vec2) -> f32,
        col: Vec3,
    ) {
        let step = if self.far { 2.5 } else { 0.42 };
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in outline {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let mut us = vec![lo.x, hi.x];
        let mut ys = vec![lo.y, hi.y];
        for h in holes {
            us.extend([h.min.x, h.max.x]);
            ys.extend([h.min.y, h.max.y]);
            if !self.far {
                us.extend([h.min.x + 0.15, h.max.x - 0.15]);
            }
        }
        let mut t = lo.x + step;
        while t < hi.x {
            us.push(t);
            t += step;
        }
        let mut t = lo.y + step;
        while t < hi.y {
            ys.push(t);
            t += step;
        }
        for v in [&mut us, &mut ys] {
            v.sort_by(f32::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() < 0.02);
        }
        let uv_off = Vec2::new(w.o.x * 0.37 + w.o.z * 0.61, 0.0);
        for j in 0..ys.len() - 1 {
            for i in 0..us.len() - 1 {
                let cell = Rect::new(us[i], ys[j], us[i + 1], ys[j + 1]);
                if cell.w() < 1e-3 || cell.h() < 1e-3 {
                    continue;
                }
                let c = cell.center();
                if holes.iter().any(|h| h.contains(c)) {
                    continue;
                }
                let poly = clip_to(&cell.poly(), outline);
                if poly.len() < 3 {
                    continue;
                }
                let pts: Vec<Vec3> = poly.iter().map(|p| w.at(*p, 0.0)).collect();
                let uvs: Vec<Vec2> = poly.iter().map(|p| *p + uv_off).collect();
                let cols: Vec<Vec3> = poly.iter().map(|p| col * shade(*p)).collect();
                self.poly(&pts, &uvs, &cols, w.n);
            }
        }
    }

    /// Surface of revolution around the vertical axis through `base`. `profile` holds
    /// (radius, height) points from bottom to top; creases sharper than ~35° stay sharp.
    /// `phase` turns the first vertex (radians).
    fn lathe(&mut self, base: Vec3, profile: &[Vec2], sides: u32, col: Vec3, phase: f32) {
        self.lathe_axis(base, Vec3::Y, profile, sides, col, phase);
    }

    /// [`Mb::lathe`] around an arbitrary `axis`.
    fn lathe_axis(
        &mut self,
        base: Vec3,
        axis: Vec3,
        profile: &[Vec2],
        sides: u32,
        col: Vec3,
        phase: f32,
    ) {
        let sides = if self.far && sides > 6 {
            (sides / 2).max(6)
        } else {
            sides
        };
        let axis = axis.normalize();
        let e1 = axis.any_orthonormal_vector();
        let e2 = axis.cross(e1);
        if sides <= 6 {
            // Few sides: a faceted solid (square lantern parts), flat shaded.
            let ring = |p: Vec2, i: u32| {
                let a = phase + i as f32 / sides as f32 * std::f32::consts::TAU;
                base + (e1 * a.cos() + e2 * a.sin()) * p.x + axis * p.y
            };
            for w in profile.windows(2) {
                for i in 0..sides {
                    let pts = [
                        ring(w[0], i),
                        ring(w[0], i + 1),
                        ring(w[1], i + 1),
                        ring(w[1], i),
                    ];
                    let a = phase + (i as f32 + 0.5) / sides as f32 * std::f32::consts::TAU;
                    let radial = e1 * a.cos() + e2 * a.sin();
                    let d = (w[1] - w[0]).normalize_or(Vec2::Y);
                    let facing = radial * d.y - axis * d.x;
                    self.face(&pts, facing, axis, radial, col);
                }
            }
            return;
        }
        let seg_n: Vec<Vec2> = profile
            .windows(2)
            .map(|w| {
                let d = (w[1] - w[0]).normalize_or(Vec2::Y);
                Vec2::new(d.y, -d.x)
            })
            .collect();
        let mut v_len = 0.0;
        for k in 0..profile.len() - 1 {
            let (p0, p1) = (profile[k], profile[k + 1]);
            let len = p0.distance(p1);
            if len < 1e-5 {
                continue;
            }
            let smooth_with = |other: Option<&Vec2>| match other {
                Some(o) if o.dot(seg_n[k]) > 0.82 => (*o + seg_n[k]).normalize(),
                _ => seg_n[k],
            };
            let n0 = smooth_with(if k > 0 { seg_n.get(k - 1) } else { None });
            let n1 = smooth_with(seg_n.get(k + 1));
            let b = self.mesh.vertices.len() as u32;
            for i in 0..=sides {
                let a = phase + i as f32 / sides as f32 * std::f32::consts::TAU;
                let radial = e1 * a.cos() + e2 * a.sin();
                for (p, n, v) in [(p0, n0, v_len), (p1, n1, v_len + len)] {
                    let pos = base + radial * p.x + axis * p.y;
                    let nrm = (radial * n.x + axis * n.y).normalize_or(axis);
                    let uv = Vec2::new(a * p.x.max(0.02), v);
                    self.mesh
                        .vertices
                        .push(Vertex::new(pos, nrm, uv).with_color(col.extend(1.0)));
                }
            }
            v_len += len;
            // Orientation from the first quad against the intended normal.
            let vtx = |i: u32| Vec3::from(self.mesh.vertices[(b + i) as usize].position);
            let mut flip = false;
            for i in 0..sides {
                let (a0, a1, c0) = (vtx(i * 2), vtx(i * 2 + 2), vtx(i * 2 + 1));
                let tri_n = (a1 - a0).cross(c0 - a0);
                if tri_n.length_squared() > 1e-12 {
                    let want = Vec3::from(self.mesh.vertices[(b + i * 2) as usize].normal);
                    flip = tri_n.dot(want) < 0.0;
                    break;
                }
            }
            for i in 0..sides {
                let k0 = b + i * 2;
                let (a0, c0, a1, c1) = (k0, k0 + 1, k0 + 2, k0 + 3);
                if flip {
                    self.mesh
                        .indices
                        .extend_from_slice(&[a0, c0, a1, a1, c0, c1]);
                } else {
                    self.mesh
                        .indices
                        .extend_from_slice(&[a0, a1, c0, a1, c1, c0]);
                }
            }
        }
    }

    /// A round bar along `path` with radius `r(t)` (t from 0 to 1 along the path), smooth
    /// normals, optional end caps.
    fn sweep(&mut self, path: &[Vec3], r: &dyn Fn(f32) -> f32, sides: u32, col: Vec3, caps: bool) {
        if path.len() < 2 {
            return;
        }
        let sides = if self.far { (sides / 2).max(3) } else { sides };
        let n = path.len();
        let tangent = |i: usize| {
            let a = path[i.saturating_sub(1)];
            let b = path[(i + 1).min(n - 1)];
            (b - a).normalize_or(Vec3::Y)
        };
        let mut t_prev = tangent(0);
        let mut nrm = t_prev.any_orthonormal_vector();
        let mut lens = vec![0.0f32; n];
        for i in 1..n {
            lens[i] = lens[i - 1] + path[i].distance(path[i - 1]);
        }
        let total = lens[n - 1].max(1e-5);
        let b = self.mesh.vertices.len() as u32;
        let mut rings = Vec::with_capacity(n);
        for i in 0..n {
            let t = tangent(i);
            nrm = Quat::from_rotation_arc(t_prev, t) * nrm;
            nrm = (nrm - t * nrm.dot(t)).normalize_or(t.any_orthonormal_vector());
            t_prev = t;
            let bn = t.cross(nrm);
            let rad = r(lens[i] / total);
            rings.push((path[i], t, nrm, bn, rad));
            for s in 0..=sides {
                let a = s as f32 / sides as f32 * std::f32::consts::TAU;
                let dir = nrm * a.cos() + bn * a.sin();
                self.mesh.vertices.push(
                    Vertex::new(path[i] + dir * rad, dir, Vec2::new(lens[i], a * rad))
                        .with_color(col.extend(1.0)),
                );
            }
        }
        let row = sides + 1;
        for i in 0..n as u32 - 1 {
            for s in 0..sides {
                let a = b + i * row + s;
                let c = a + row;
                self.mesh
                    .indices
                    .extend_from_slice(&[a, a + 1, c, a + 1, c + 1, c]);
            }
        }
        // Fix the winding if the first triangle faces inward.
        let p = |k: u32| Vec3::from(self.mesh.vertices[k as usize].position);
        let first = self.mesh.indices.len() - (n - 1) * sides as usize * 6;
        let [i0, i1, i2] = [
            self.mesh.indices[first],
            self.mesh.indices[first + 1],
            self.mesh.indices[first + 2],
        ];
        let tri_n = (p(i1) - p(i0)).cross(p(i2) - p(i0));
        if tri_n.dot(Vec3::from(self.mesh.vertices[i0 as usize].normal)) < 0.0 {
            for tri in self.mesh.indices[first..].as_chunks_mut::<3>().0 {
                tri.swap(1, 2);
            }
        }
        if caps {
            for (k, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
                let (c, t, nr, bn, rad) = rings[k];
                if rad <= 1e-4 {
                    continue;
                }
                let pts: Vec<Vec3> = (0..sides)
                    .map(|s| {
                        let a = s as f32 / sides as f32 * std::f32::consts::TAU;
                        c + (nr * a.cos() + bn * a.sin()) * rad
                    })
                    .collect();
                self.face(&pts, t * sign, nr, bn, col);
            }
        }
    }

    /// A small sphere.
    fn ball(&mut self, c: Vec3, r: f32, sides: u32, col: Vec3) {
        let rings = (sides / 2).max(3);
        let profile: Vec<Vec2> = (0..=rings)
            .map(|i| {
                let a = -FRAC_PI_2 + PI * i as f32 / rings as f32;
                Vec2::new(a.cos() * r, a.sin() * r)
            })
            .collect();
        self.lathe(c, &profile, sides, col, 0.0);
    }

    /// A lumpy blob (plants), smooth normals; small ones are 20 triangles.
    fn blob(&mut self, c: Vec3, r: Vec3, col: Vec3, seed: u64) {
        let fine = !self.far && r.max_element() > 0.06;
        let (verts, idx) = crate::meshgen::icosphere(if fine { 1 } else { 0 });
        let b = self.mesh.vertices.len() as u32;
        for d in &verts {
            let k = 1.0 + 0.25 * (hash3(*d * 3.0, seed) - 0.5);
            let p = c + *d * r * k;
            let shade = 0.65 + 0.35 * (d.y * 0.5 + 0.5);
            self.mesh.vertices.push(
                Vertex::new(p, *d, Vec2::new(d.x, d.z)).with_color((col * shade).extend(1.0)),
            );
        }
        self.mesh.indices.extend(idx.iter().map(|i| i + b));
    }
}

// =============================================================================== house parts

/// The materials a house is made of, one mesh each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Plaster,
    Timber,
    Paint,
    Stone,
    Brick,
    Roof,
    Glass,
    Iron,
    Plants,
}

impl Part {
    pub const ALL: [Part; 9] = [
        Part::Plaster,
        Part::Timber,
        Part::Paint,
        Part::Stone,
        Part::Brick,
        Part::Roof,
        Part::Glass,
        Part::Iron,
        Part::Plants,
    ];
}

struct Parts {
    m: [Mb; 9],
}

impl Parts {
    fn new(far: bool) -> Self {
        Self {
            m: std::array::from_fn(|_| Mb::new(far)),
        }
    }
    fn get(&mut self, p: Part) -> &mut Mb {
        &mut self.m[p as usize]
    }
    fn far(&self) -> bool {
        self.m[0].far
    }
}

/// Where a prop can go next to a house (house-local, before any mirroring).
#[derive(Clone, Copy, Debug)]
pub struct PropSpot {
    pub kind: PropKind,
    pub pos: Vec3,
    /// Direction the prop faces (its front, +Z of the prop), in the XZ plane.
    pub facing: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropKind {
    /// Against a wall beside the door.
    Bench,
    /// At a corner.
    Barrels,
    /// Along a side wall under the eaves.
    WoodPile,
}

/// A finished house: one mesh per part, its footprint and spots for props.
pub struct HouseMeshes {
    pub parts: Vec<(Part, Mesh)>,
    /// Footprint half extents (x, z) including the roof overhang.
    pub half: Vec2,
    pub props: Vec<PropSpot>,
}

impl HouseMeshes {
    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|(_, m)| m.triangle_count()).sum()
    }
}

// =============================================================================== house specs

/// A house archetype.
#[derive(Clone, Debug)]
pub struct HouseSpec {
    pub seed: u64,
    /// Ground floor across the ridge (X), meters.
    pub width: f32,
    /// Ground floor along the ridge (Z), meters.
    pub length: f32,
    pub floors: u32,
    /// How far each upper floor overhangs the one below on its jettied sides, meters.
    pub jetty: f32,
    /// Jettied sides: +Z, +X, -Z, -X (build frame).
    pub jetty_sides: [bool; 4],
    pub pitch_deg: f32,
    /// Roof overhang at the eaves and at the gables (verge), meters.
    pub eaves: f32,
    pub verge: f32,
    /// Height of the stone plinth above the ground, meters.
    pub plinth: f32,
    /// Door on a long side (eaves to the street) instead of the gable.
    pub eaves_front: bool,
    pub shutters: bool,
    /// Share of windows with a flower box.
    pub flower_boxes: f32,
    /// St Andrew's crosses in the panels under the windows.
    pub crosses: bool,
    /// Chimney position along the ridge, -1..1.
    pub chimney_at: f32,
    pub window_paint: Vec3,
    pub shutter_paint: Vec3,
    pub door_paint: Vec3,
    pub stone_sills: bool,
    /// Dormers on the street side of the roof (eaves-front houses).
    pub dormers: u32,
}

/// The village's house archetypes.
pub fn archetypes() -> Vec<HouseSpec> {
    let green = Vec3::new(0.07, 0.17, 0.09);
    let blue = Vec3::new(0.13, 0.2, 0.27);
    let red = Vec3::new(0.34, 0.06, 0.04);
    let cream = Vec3::new(0.86, 0.8, 0.64);
    let white = Vec3::new(0.85, 0.85, 0.82);
    let brown = Vec3::new(0.2, 0.12, 0.07);
    let base = HouseSpec {
        seed: 0,
        width: 7.0,
        length: 9.5,
        floors: 2,
        jetty: 0.0,
        jetty_sides: [false; 4],
        pitch_deg: 52.0,
        eaves: 0.6,
        verge: 0.38,
        plinth: 0.55,
        eaves_front: false,
        shutters: false,
        flower_boxes: 0.4,
        crosses: false,
        chimney_at: 0.0,
        window_paint: white,
        shutter_paint: green,
        door_paint: brown,
        stone_sills: false,
        dormers: 0,
    };
    vec![
        // A one-floor cottage, gable to the street, green shutters.
        HouseSpec {
            seed: 11,
            width: 6.6,
            length: 8.8,
            floors: 1,
            pitch_deg: 53.0,
            shutters: true,
            flower_boxes: 0.7,
            chimney_at: -0.35,
            door_paint: green,
            ..base.clone()
        },
        // Two floors jettied front and back, crosses under the windows, slate-friendly.
        HouseSpec {
            seed: 22,
            width: 7.4,
            length: 10.0,
            floors: 2,
            jetty: 0.4,
            jetty_sides: [true, false, true, false],
            pitch_deg: 55.0,
            crosses: true,
            flower_boxes: 0.45,
            chimney_at: 0.25,
            window_paint: cream,
            door_paint: red,
            stone_sills: true,
            ..base.clone()
        },
        // A tall town house: three floors, jettied on three sides.
        HouseSpec {
            seed: 33,
            width: 7.0,
            length: 9.6,
            floors: 3,
            jetty: 0.32,
            jetty_sides: [true, true, false, true],
            pitch_deg: 57.0,
            eaves: 0.5,
            plinth: 0.5,
            crosses: true,
            flower_boxes: 0.3,
            chimney_at: -0.45,
            door_paint: blue,
            stone_sills: true,
            ..base.clone()
        },
        // A long house with its eaves to the street, jettied front, dormers.
        HouseSpec {
            seed: 44,
            width: 8.0,
            length: 12.4,
            floors: 2,
            jetty: 0.38,
            jetty_sides: [false, true, false, false],
            pitch_deg: 48.0,
            eaves: 0.7,
            eaves_front: true,
            shutters: true,
            flower_boxes: 0.5,
            chimney_at: 0.4,
            shutter_paint: red,
            door_paint: red,
            dormers: 2,
            ..base.clone()
        },
        // A small eaves-front cottage with blue shutters.
        HouseSpec {
            seed: 55,
            width: 6.4,
            length: 9.8,
            floors: 1,
            pitch_deg: 50.0,
            plinth: 0.62,
            eaves_front: true,
            shutters: true,
            flower_boxes: 0.6,
            chimney_at: -0.2,
            shutter_paint: blue,
            door_paint: blue,
            dormers: 1,
            ..base.clone()
        },
        // Two plain floors, gable front, brown shutters.
        HouseSpec {
            seed: 66,
            width: 7.8,
            length: 9.4,
            floors: 2,
            pitch_deg: 50.0,
            shutters: true,
            flower_boxes: 0.55,
            chimney_at: 0.1,
            window_paint: cream,
            shutter_paint: brown,
            door_paint: green,
            stone_sills: true,
            ..base
        },
    ]
}

// =============================================================================== the builder

/// Timber faces stand this far proud of the plaster.
const F: f32 = 0.035;
/// Timbers reach this deep behind the plaster (window and door reveals).
const DEEP: f32 = 0.14;
const SILL_H: f32 = 0.22;
const PLATE_H: f32 = 0.18;
const POST_W: f32 = 0.18;
const CORNER_W: f32 = 0.24;
const RAIL_H: f32 = 0.14;
const BRACE_W: f32 = 0.16;
const JOIST_H: f32 = 0.2;
const CHAMFER: f32 = 0.018;
/// How far beyond the plaster the plinth's stones stand.
const PLINTH_OUT: f32 = F + 0.05;
/// Ground-floor and upper-floor heights (sill bottom to plate top).
const FLOOR_H: [f32; 2] = [2.75, 2.55];
const SIDES: [usize; 4] = [0, 1, 2, 3];
const FRONT: usize = 0;
const RIGHT: usize = 1;
const BACK: usize = 2;

#[derive(Clone, Copy, Debug)]
struct Floor {
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
    y0: f32,
    h: f32,
}

impl Floor {
    fn wall(&self, side: usize) -> Wall {
        let (x0, x1, z0, z1) = (self.x0, self.x1, self.z0, self.z1);
        match side {
            0 => Wall {
                o: Vec3::new(x0, 0.0, z1),
                u: Vec3::X,
                n: Vec3::Z,
                len: x1 - x0,
                through: true,
            },
            1 => Wall {
                o: Vec3::new(x1, 0.0, z1),
                u: -Vec3::Z,
                n: Vec3::X,
                len: z1 - z0,
                through: false,
            },
            2 => Wall {
                o: Vec3::new(x1, 0.0, z0),
                u: -Vec3::X,
                n: -Vec3::Z,
                len: x1 - x0,
                through: true,
            },
            _ => Wall {
                o: Vec3::new(x0, 0.0, z0),
                u: Vec3::Z,
                n: -Vec3::X,
                len: z1 - z0,
                through: false,
            },
        }
    }
    fn sill_top(&self) -> f32 {
        self.y0 + SILL_H
    }
    fn plate_bottom(&self) -> f32 {
        self.y0 + self.h - PLATE_H
    }
    fn top(&self) -> f32 {
        self.y0 + self.h
    }
}

/// The roof's cross-section (build frame, ridge along Z): x is measured from the ridge line.
#[derive(Clone, Copy, Debug)]
struct RoofShape {
    xc: f32,
    hw: f32,
    z0: f32,
    z1: f32,
    y_top: f32,
    tan: f32,
    /// Rafter depth (vertical part under the deck).
    rafter: f32,
    /// Outer eaves edge and the kink where the shallower eaves part starts (x from ridge).
    x_kink: f32,
    x_eaves: f32,
    tan_kick: f32,
}

/// Deck boards + battens: the tiles' base is this far above the rafters' top.
const DECK: f32 = 0.06;

impl RoofShape {
    /// Height of the rafters' top along the main pitch (no eaves kick), `x` from the ridge.
    fn main(&self, x: f32) -> f32 {
        self.y_top
            + self.rafter * (1.0 + self.tan * self.tan).sqrt()
            + (self.hw + F - x.abs()) * self.tan
    }

    /// Height of the rafters' top (the deck's underside) at `x` from the ridge.
    fn deck(&self, x: f32) -> f32 {
        let x = x.abs();
        if x <= self.x_kink {
            self.main(x)
        } else {
            self.main(self.x_kink) - (x - self.x_kink) * self.tan_kick
        }
    }
    fn ridge_y(&self) -> f32 {
        self.deck(0.0)
    }
    /// The deck's underside as a polyline from the eaves edge to the ridge for one side.
    fn profile(&self) -> [Vec2; 3] {
        [
            Vec2::new(self.x_eaves, self.deck(self.x_eaves)),
            Vec2::new(self.x_kink, self.deck(self.x_kink)),
            Vec2::new(0.0, self.deck(0.0)),
        ]
    }
}

/// Upward normal of a roof line direction.
fn up_normal(dir: Vec2) -> Vec2 {
    let n = Vec2::new(-dir.y, dir.x);
    if n.y < 0.0 { -n } else { n }
}

/// A roof polyline (eaves to ridge) offset by `d` along its upward normals (mitered).
fn offset_line(line: &[Vec2], d: f32) -> Vec<Vec2> {
    let n = line.len();
    let seg = |k: usize| up_normal((line[k + 1] - line[k]).normalize_or(Vec2::X));
    (0..n)
        .map(|i| {
            let nm = if i == 0 {
                seg(0)
            } else if i == n - 1 {
                seg(n - 2)
            } else {
                let (a, b) = (seg(i - 1), seg(i));
                (a + b) / (1.0 + a.dot(b))
            };
            line[i] + nm * d
        })
        .collect()
}

/// Sample a polyline at arc length `s`: point and unit direction.
fn along(line: &[Vec2], s: f32) -> (Vec2, Vec2) {
    let mut acc = 0.0;
    let last = line.len() - 2;
    for (k, w) in line.windows(2).enumerate() {
        let len = w[0].distance(w[1]).max(1e-6);
        if s <= acc + len || k == last {
            let t = ((s - acc) / len).clamp(0.0, 1.0);
            return (w[0].lerp(w[1], t), (w[1] - w[0]) / len);
        }
        acc += len;
    }
    unreachable!("a roof line has at least two points")
}

fn line_len(line: &[Vec2]) -> f32 {
    line.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Build a house's near (`far = false`) or far mesh set.
pub fn house(spec: &HouseSpec, far: bool) -> HouseMeshes {
    let mut rng = Rng::new(spec.seed);
    let mut p = Parts::new(far);
    let (hw, hl) = (spec.width * 0.5, spec.length * 0.5);

    // Floors, each jettied out over the one below on the jettied sides.
    let mut floors = Vec::new();
    let mut fl = Floor {
        x0: -hw,
        x1: hw,
        z0: -hl,
        z1: hl,
        y0: spec.plinth,
        h: FLOOR_H[0],
    };
    for f in 0..spec.floors {
        if f > 0 {
            let j = spec.jetty;
            fl.z1 += if spec.jetty_sides[0] { j } else { 0.0 };
            fl.x1 += if spec.jetty_sides[1] { j } else { 0.0 };
            fl.z0 -= if spec.jetty_sides[2] { j } else { 0.0 };
            fl.x0 -= if spec.jetty_sides[3] { j } else { 0.0 };
            fl.y0 += fl.h + JOIST_H;
            fl.h = FLOOR_H[1];
        }
        floors.push(fl);
    }
    let door_side = if spec.eaves_front { RIGHT } else { FRONT };
    let top = *floors.last().unwrap();
    let tan = spec.pitch_deg.to_radians().tan();
    let thw = (top.x1 - top.x0) * 0.5;
    let roof = RoofShape {
        xc: (top.x0 + top.x1) * 0.5,
        hw: thw,
        z0: top.z0 - spec.verge,
        z1: top.z1 + spec.verge,
        y_top: top.top(),
        tan,
        rafter: 0.16,
        x_kink: thw + F + 0.12,
        x_eaves: thw + F + spec.eaves,
        tan_kick: (spec.pitch_deg.to_radians() - 0.3).max(0.4).tan(),
    };

    plinth(&mut p, &floors[0], spec, &mut rng);
    let mut props = Vec::new();
    for (fi, fl) in floors.iter().enumerate() {
        corner_posts(&mut p, fl, &mut rng, spec);
        for side in SIDES {
            floor_wall(&mut p, spec, fl, fi, side, door_side, &mut rng, &mut props);
        }
        if fi + 1 < floors.len() {
            joists(&mut p, fl, &floors[fi + 1], spec, &mut rng);
        }
    }
    for side in [FRONT, BACK] {
        gable(&mut p, spec, &top, side, &roof, &mut rng);
    }
    for side in [RIGHT, 3] {
        // Bird boards close the gap between the wall plate and the roof deck.
        let w = top.wall(side);
        let col = timber_col(&mut rng);
        let y1 = roof.deck(roof.hw) + 0.02;
        p.get(Part::Timber).member(
            &w,
            &Rect::new(-F, top.top() - 0.01, w.len + F, y1).poly(),
            -0.12,
            F - 0.008,
            0.008,
            Vec2::X,
            col,
        );
    }
    roof_build(&mut p, spec, &roof, &top, &mut rng);
    chimney(&mut p, spec, &roof, &mut rng);
    if spec.dormers > 0 {
        dormers(&mut p, spec, &roof, &top, &mut rng);
    }

    // Footprint half extents, and the spots for props.
    let mut half = Vec2::new(
        (roof.xc.abs() + roof.x_eaves).max(hw + spec.jetty * 2.0) + 0.3,
        (roof.z1).max(-roof.z0) + 0.2,
    );
    let mut parts: Vec<(Part, Mesh)> = Part::ALL
        .iter()
        .zip(p.m)
        .filter(|(_, mb)| !mb.mesh.indices.is_empty())
        .map(|(part, mb)| (*part, mb.mesh))
        .collect();
    if spec.eaves_front {
        let turn = Mat4::from_rotation_y(-FRAC_PI_2);
        for (_, m) in &mut parts {
            m.transform(&turn);
        }
        for s in &mut props {
            s.pos = turn.transform_point3(s.pos);
            s.facing = turn.transform_vector3(s.facing);
        }
        half = Vec2::new(half.y, half.x);
    }
    HouseMeshes { parts, half, props }
}

fn timber_col(rng: &mut Rng) -> Vec3 {
    Vec3::splat(rng.range(0.8, 1.12))
}

/// The stone plinth: rows of single rough stones in front of a mortar core, corners bonded
/// (each row's corner stone belongs to alternating walls).
fn plinth(p: &mut Parts, fl: &Floor, spec: &HouseSpec, rng: &mut Rng) {
    let top = spec.plinth;
    let bottom = -0.75;
    let depth = 0.24;
    let gap = 0.014;
    let far = p.far();
    // Mortar core behind the stones.
    for side in SIDES {
        let w = fl.wall(side);
        let ext = PLINTH_OUT - 0.025;
        let pts = [
            w.at(Vec2::new(-ext, -1.1), ext),
            w.at(Vec2::new(w.len + ext, -1.1), ext),
            w.at(Vec2::new(w.len + ext, top), ext),
            w.at(Vec2::new(-ext, top), ext),
        ];
        let col = Vec3::new(0.5, 0.48, 0.45);
        if far {
            // Far away the plinth is one stone-textured band.
            let pts = [
                w.at(Vec2::new(-PLINTH_OUT, -1.1), PLINTH_OUT),
                w.at(Vec2::new(w.len + PLINTH_OUT, -1.1), PLINTH_OUT),
                w.at(Vec2::new(w.len + PLINTH_OUT, top), PLINTH_OUT),
                w.at(Vec2::new(-PLINTH_OUT, top), PLINTH_OUT),
            ];
            p.get(Part::Stone)
                .face(&pts, w.n, w.u, Vec3::Y, Vec3::new(0.62, 0.6, 0.56));
            let lid = [
                w.at(Vec2::new(-PLINTH_OUT, top), PLINTH_OUT),
                w.at(Vec2::new(w.len + PLINTH_OUT, top), PLINTH_OUT),
                w.at(Vec2::new(w.len + PLINTH_OUT, top), -0.1),
                w.at(Vec2::new(-PLINTH_OUT, top), -0.1),
            ];
            p.get(Part::Stone)
                .face(&lid, Vec3::Y, w.u, w.n, Vec3::new(0.62, 0.6, 0.56));
            continue;
        }
        p.get(Part::Plaster).face(&pts, w.n, w.u, Vec3::Y, col);
    }
    if far {
        return;
    }
    let tints = [
        Vec3::new(0.62, 0.6, 0.56),
        Vec3::new(0.7, 0.64, 0.54),
        Vec3::new(0.55, 0.55, 0.55),
        Vec3::new(0.66, 0.58, 0.5),
        Vec3::new(0.5, 0.49, 0.47),
    ];
    // Row heights shared by all four walls so the corners bond.
    let mut rows = Vec::new();
    let mut y = bottom;
    while y < top - 0.05 {
        let mut h = rng.range(0.19, 0.31);
        if top - (y + h) < 0.14 {
            h = top - y;
        }
        rows.push((y, h));
        y += h;
    }
    let stone = p.get(Part::Stone);
    let mut seed = spec.seed * 1000;
    for side in SIDES {
        let w = fl.wall(side);
        for (r, &(y0, h)) in rows.iter().enumerate() {
            let owns = (r + side) % 2 == 0;
            let (start, end) = if owns {
                (-PLINTH_OUT, w.len + PLINTH_OUT)
            } else {
                (-PLINTH_OUT + depth + gap, w.len + PLINTH_OUT - depth - gap)
            };
            let mut u = start;
            while u < end - 0.05 {
                let mut len = rng.range(0.3, 0.68);
                if end - (u + len) < 0.22 {
                    len = end - u;
                }
                let out = rng.range(-0.008, 0.01);
                let d = depth + out;
                let c = Vec2::new(u + len * 0.5, y0 + h * 0.5);
                let center = w.at(c, PLINTH_OUT + out - d * 0.5);
                let tilt = Quat::from_axis_angle(w.n, rng.range(-0.025, 0.025))
                    * Quat::from_axis_angle(Vec3::Y, rng.range(-0.02, 0.02));
                let ax = [tilt * w.u, tilt * Vec3::Y, tilt * w.n];
                let tint =
                    tints[rng.range_i32(0, tints.len() as i32) as usize] * rng.range(0.82, 1.12);
                // Damp and dirt toward the ground.
                let tint = tint * (0.78 + 0.22 * engine::core::math::smoothstep(-0.2, 0.35, c.y));
                seed += 1;
                stone.cbox_rough(
                    center,
                    ax,
                    Vec3::new(len * 0.5 - gap * 0.5, h * 0.5 - gap * 0.5, d * 0.5),
                    rng.range(0.018, 0.035),
                    tint,
                    1 << 4, // the back face is inside the core
                    0.009,
                    seed,
                );
                u += len;
            }
        }
    }
}

/// The square posts at a floor's four corners.
fn corner_posts(p: &mut Parts, fl: &Floor, rng: &mut Rng, _spec: &HouseSpec) {
    for (x, z, sx, sz) in [
        (fl.x1, fl.z1, 1.0, 1.0),
        (fl.x1, fl.z0, 1.0, -1.0),
        (fl.x0, fl.z0, -1.0, -1.0),
        (fl.x0, fl.z1, -1.0, 1.0),
    ] {
        let outer = Vec3::new(x + sx * F, 0.0, z + sz * F);
        let inner = outer - Vec3::new(sx * CORNER_W, 0.0, sz * CORNER_W);
        let min = Vec3::new(outer.x.min(inner.x), fl.sill_top(), outer.z.min(inner.z));
        let max = Vec3::new(
            outer.x.max(inner.x),
            fl.plate_bottom(),
            outer.z.max(inner.z),
        );
        let col = timber_col(rng);
        p.get(Part::Timber).abox(min, max, CHAMFER, 1, col);
    }
}

/// What fills one bay of a timber wall.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Bay {
    /// A full-height brace rising toward `corner_left` (true) or the right.
    Brace(bool),
    Window,
    Door,
    Plain,
}

/// Lay out one wall of one floor: sill, plate, posts, rails, braces, windows, door, plaster.
#[allow(clippy::too_many_arguments)]
fn floor_wall(
    p: &mut Parts,
    spec: &HouseSpec,
    fl: &Floor,
    fi: usize,
    side: usize,
    door_side: usize,
    rng: &mut Rng,
    props: &mut Vec<PropSpot>,
) {
    let w = fl.wall(side);
    let far = p.far();
    let ext = if w.through { F + 0.09 } else { -DEEP };
    let (y_sill, y_plate) = (fl.sill_top(), fl.plate_bottom());
    let tcol = |rng: &mut Rng| timber_col(rng);
    // Sill and plate.
    let c = tcol(rng);
    p.get(Part::Timber).member(
        &w,
        &Rect::new(-ext, fl.y0, w.len + ext, y_sill).poly(),
        -DEEP,
        F + 0.005,
        CHAMFER,
        Vec2::X,
        c * if fi == 0 { 0.85 } else { 1.0 },
    );
    let c = tcol(rng);
    p.get(Part::Timber).member(
        &w,
        &Rect::new(-ext, y_plate, w.len + ext, fl.top()).poly(),
        -DEEP,
        F,
        CHAMFER,
        Vec2::X,
        c,
    );

    // Bays between the corner posts.
    let (ui0, ui1) = (CORNER_W - F, w.len + F - CORNER_W);
    let li = ui1 - ui0;
    let n = ((li / 1.25).round() as usize).max(1);
    let bw = li / n as f32;
    let is_door_wall = fi == 0 && side == door_side;
    let door_bay = n / 2;
    let mut bays = vec![Bay::Plain; n];
    for (k, bay) in bays.iter_mut().enumerate() {
        let corner = n >= 4 && (k == 0 || k == n - 1);
        *bay = if is_door_wall && k == door_bay {
            Bay::Door
        } else if corner {
            Bay::Brace(k == 0)
        } else if (n >= 4 && k % 2 == 1) || (n < 4 && k == n / 2) {
            let keep = match side {
                s if s == door_side => 1.0,
                BACK if !spec.eaves_front => 0.55,
                3 if spec.eaves_front => 0.55,
                _ => 0.8,
            };
            if hash01(k as i64, side as i64, fi as i64, spec.seed) < keep {
                Bay::Window
            } else {
                Bay::Plain
            }
        } else {
            Bay::Plain
        };
    }
    // Windows right beside the door keep their shutters off it.
    let beside_door = |a: f32, b: f32| {
        is_door_wall && {
            let (da, db) = (ui0 + door_bay as f32 * bw, ui0 + (door_bay + 1) as f32 * bw);
            (a - db).abs() < bw * 0.6 || (b - da).abs() < bw * 0.6
        }
    };
    let clear = |k: usize| {
        let a = ui0 + k as f32 * bw + if k > 0 { POST_W * 0.5 } else { 0.0 };
        let b = ui0 + (k + 1) as f32 * bw - if k + 1 < n { POST_W * 0.5 } else { 0.0 };
        (a, b)
    };
    // Window and rail heights.
    let (win_y0, win_h) = if fi == 0 {
        (y_sill + 0.8, 1.2)
    } else {
        (y_sill + 0.72, 1.15)
    };
    let win_y1 = win_y0 + win_h;
    let rails = [(win_y0 - RAIL_H, win_y0), (win_y1, win_y1 + RAIL_H)];
    let door_top = y_sill + 2.1;

    // Posts between bays.
    for k in 1..n {
        let u = ui0 + k as f32 * bw;
        let c = tcol(rng);
        p.get(Part::Timber).member(
            &w,
            &Rect::new(u - POST_W * 0.5, y_sill, u + POST_W * 0.5, y_plate).poly(),
            -DEEP,
            F,
            CHAMFER,
            Vec2::Y,
            c,
        );
    }
    let mut holes = Vec::new();
    let mut windows = Vec::new();
    for (k, bay) in bays.iter().enumerate() {
        let (a, b) = clear(k);
        let panel = Rect::new(a, y_sill, b, y_plate);
        match *bay {
            Bay::Window => {
                let r = Rect::new(a, win_y0, b, win_y1);
                holes.push(r);
                windows.push(r);
                for (y0, y1) in rails {
                    let c = tcol(rng);
                    p.get(Part::Timber).member(
                        &w,
                        &Rect::new(a, y0, b, y1).poly(),
                        -DEEP,
                        F - 0.004,
                        CHAMFER,
                        Vec2::X,
                        c,
                    );
                }
                if spec.crosses && !far {
                    // St Andrew's cross in the panel under the window.
                    let cross = Rect::new(a, y_sill, b, rails[0].0);
                    for (i, (s, e)) in [
                        (cross.min, cross.max),
                        (
                            Vec2::new(cross.min.x, cross.max.y),
                            Vec2::new(cross.max.x, cross.min.y),
                        ),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let poly = clip_to(&band(s, e, BRACE_W * 0.75), &cross.poly());
                        let dir = (e - s).normalize();
                        let c = tcol(rng);
                        p.get(Part::Timber).member(
                            &w,
                            &poly,
                            -DEEP,
                            F - 0.006 - i as f32 * 0.004,
                            CHAMFER,
                            dir,
                            c,
                        );
                    }
                }
            }
            Bay::Door => {
                let r = Rect::new(a, y_sill, b, door_top);
                holes.push(r);
                let c = tcol(rng);
                p.get(Part::Timber).member(
                    &w,
                    &Rect::new(a, door_top, b, door_top + 0.2).poly(),
                    -DEEP,
                    F + 0.01,
                    CHAMFER,
                    Vec2::X,
                    c,
                );
                door(p, &w, r, spec, rng, fl);
                // A bench beside the door.
                let bench_u = if k + 1 < n { b + 1.35 } else { a - 1.35 };
                if bench_u > 0.9 && bench_u < w.len - 0.9 {
                    props.push(PropSpot {
                        kind: PropKind::Bench,
                        pos: w.at(Vec2::new(bench_u, 0.0), PLINTH_OUT + 0.3),
                        facing: w.n,
                    });
                }
            }
            Bay::Brace(to_left) => {
                // The brace rises from the sill at the bay's inner side to the post at the corner.
                let (s, e) = if to_left {
                    (Vec2::new(b, y_sill), Vec2::new(a, y_plate))
                } else {
                    (Vec2::new(a, y_sill), Vec2::new(b, y_plate))
                };
                let brace = clip_to(&band(s, e, BRACE_W), &panel.poly());
                let c = tcol(rng);
                p.get(Part::Timber).member(
                    &w,
                    &brace,
                    -DEEP,
                    F - 0.003,
                    CHAMFER,
                    (e - s).normalize(),
                    c,
                );
                // Rails on both sides of the brace, cut to it.
                let d = (e - s).normalize();
                let nrm = Vec2::new(-d.y, d.x);
                let off = BRACE_W * 0.5;
                for (y0, y1) in rails {
                    let rail = Rect::new(a, y0, b, y1).poly();
                    for sgn in [1.0f32, -1.0] {
                        let piece = clip(&rail, nrm * sgn, nrm.dot(s) * sgn + off);
                        let c = tcol(rng);
                        p.get(Part::Timber).member(
                            &w,
                            &piece,
                            -DEEP,
                            F - 0.004,
                            CHAMFER,
                            Vec2::X,
                            c,
                        );
                    }
                }
            }
            Bay::Plain => {
                for (y0, y1) in rails {
                    let c = tcol(rng);
                    p.get(Part::Timber).member(
                        &w,
                        &Rect::new(a, y0, b, y1).poly(),
                        -DEEP,
                        F - 0.004,
                        CHAMFER,
                        Vec2::X,
                        c,
                    );
                }
            }
        }
    }
    // Plaster with grime: splash above the sill, darker under the plate, streaks under the
    // windows, blotches.
    let seed = spec.seed as u32 + side as u32 * 7 + fi as u32 * 31;
    let win = windows.clone();
    let shade = move |q: Vec2| {
        let mut s = 1.0
            + 0.08 * perlin2(q.x as f64 * 0.7, q.y as f64 * 0.7, seed as u64)
            + 0.05 * perlin2(q.x as f64 * 2.3, q.y as f64 * 1.6, seed as u64 + 1);
        if fi == 0 {
            // Rain splash and damp rising from the plinth, ragged at the top.
            let top = y_sill + 0.8 + 0.25 * perlin2(q.x as f64 * 1.3, 0.5, seed as u64 + 2);
            s *= 1.0 - 0.3 * engine::core::math::smoothstep(top, y_sill, q.y);
        } else {
            s *= 1.0 - 0.07 * engine::core::math::smoothstep(y_sill + 0.3, y_sill, q.y);
        }
        s *= 1.0 - 0.08 * engine::core::math::smoothstep(y_plate - 0.5, y_plate, q.y);
        for r in &win {
            if q.x > r.min.x + 0.1 && q.x < r.max.x - 0.1 && q.y < r.min.y {
                s *= 1.0 - 0.13 * (1.0 - (r.min.y - q.y) / 1.1).clamp(0.0, 1.0);
            }
        }
        s
    };
    p.get(Part::Plaster).plaster(
        &w,
        &Rect::new(0.0, fl.y0, w.len, fl.top()).poly(),
        &holes,
        &shade,
        Vec3::ONE,
    );
    let n_win = windows.len();
    for (i, r) in windows.into_iter().enumerate() {
        let flower = hash01(i as i64, side as i64, fi as i64 + 50, spec.seed) < spec.flower_boxes
            && (fi > 0 || side == door_side || n_win < 3);
        let shutters = spec.shutters && !beside_door(r.min.x, r.max.x);
        window(p, &w, r, spec, rng, shutters, flower);
    }
    // Wood piles and barrels by the side walls of the ground floor.
    if fi == 0 && side != door_side {
        if side == RIGHT || side == 3 {
            props.push(PropSpot {
                kind: PropKind::WoodPile,
                pos: w.at(Vec2::new(w.len * 0.5, 0.0), PLINTH_OUT + 0.35),
                facing: w.n,
            });
        }
        props.push(PropSpot {
            kind: PropKind::Barrels,
            pos: w.at(Vec2::new(0.7, 0.0), PLINTH_OUT + 0.45),
            facing: w.n,
        });
    }
}

/// Joist heads, filler boards and the soffit where `upper` overhangs `lower`; a timber band
/// on the other sides.
fn joists(p: &mut Parts, lower: &Floor, upper: &Floor, spec: &HouseSpec, rng: &mut Rng) {
    let y0 = lower.top();
    let y1 = upper.y0;
    for side in SIDES {
        let lw = lower.wall(side);
        let uw = upper.wall(side);
        // How far the upper wall stands out of the lower wall on this side.
        let out = (uw.o - lw.o).dot(lw.n);
        if out < 0.05 || !spec.jetty_sides[side] {
            // A band of timber hides the joists' ends.
            let ext = if uw.through { F + 0.09 } else { -DEEP };
            let c = timber_col(rng);
            let u_off = (uw.o - lw.o).dot(lw.u);
            let (ua, ub) = (u_off.min(0.0) - ext, (u_off + uw.len).max(lw.len) + ext);
            p.get(Part::Timber).member(
                &lw,
                &Rect::new(ua, y0, ub, y1).poly(),
                -DEEP,
                F - 0.01,
                CHAMFER,
                Vec2::X,
                c,
            );
            continue;
        }
        // Joist heads stick out under the upper floor's sill.
        let u_shift = (uw.o - lw.o).dot(lw.u);
        let span = (uw.len).max(lw.len);
        let count = ((span - 0.6) / 0.75).round().max(2.0) as usize;
        let tip = out + F + 0.08;
        let far = p.far();
        for k in 0..=count {
            let u = u_shift + 0.3 + k as f32 / count as f32 * (span - 0.6);
            let c = timber_col(rng) * 0.95;
            // Side profile: rounded-off lower end.
            let prof = [
                Vec2::new(-0.2, y0),
                Vec2::new(tip - 0.09, y0),
                Vec2::new(tip - 0.03, y0 + 0.03),
                Vec2::new(tip, y0 + 0.09),
                Vec2::new(tip, y1),
                Vec2::new(-0.2, y1),
            ];
            for sgn in [1.0f32, -1.0] {
                let frame = Wall {
                    o: lw.o + lw.u * u,
                    u: lw.n,
                    n: lw.u * sgn,
                    len: 0.0,
                    through: false,
                };
                if far && sgn < 0.0 {
                    continue;
                }
                p.get(Part::Timber)
                    .member(&frame, &prof, 0.0, 0.075, 0.012, Vec2::X, c);
            }
        }
        // Filler boards between the heads, and the soffit above them.
        let u_a = u_shift - F;
        let u_b = u_shift + uw.len + F;
        let c = timber_col(rng) * 0.9;
        let fill = Wall {
            o: lw.o + lw.n * out,
            ..lw
        };
        p.get(Part::Timber).member(
            &fill,
            &Rect::new(u_a, y0 + 0.04, u_b, y1).poly(),
            -0.04,
            F - 0.02,
            0.006,
            Vec2::X,
            c,
        );
        let pts = [
            lw.at(Vec2::new(u_a, y1 - 0.001), 0.0),
            lw.at(Vec2::new(u_b, y1 - 0.001), 0.0),
            lw.at(Vec2::new(u_b, y1 - 0.001), out),
            lw.at(Vec2::new(u_a, y1 - 0.001), out),
        ];
        p.get(Part::Timber)
            .face(&pts, -Vec3::Y, lw.u, lw.n, Vec3::splat(0.7));
    }
}

/// A gable wall above the top floor: sill, posts cut by the roof line, collar rail, slope
/// timbers, attic windows, plaster.
fn gable(
    p: &mut Parts,
    spec: &HouseSpec,
    top: &Floor,
    side: usize,
    roof: &RoofShape,
    rng: &mut Rng,
) {
    let w = top.wall(side);
    let y0 = top.top();
    let l = w.len;
    let line = |u: f32| roof.deck(u - l * 0.5);
    let apex = line(l * 0.5);
    let outline = [
        Vec2::new(0.0, y0),
        Vec2::new(l, y0),
        Vec2::new(l, line(l)),
        Vec2::new(l * 0.5, apex),
        Vec2::new(0.0, line(0.0)),
    ];
    // Members stay below the slope timbers.
    let t = 0.2 * (1.0 + roof.tan * roof.tan).sqrt();
    let main = |u: f32| roof.main(u - l * 0.5);
    let inner = [
        Vec2::new(-1.0, y0),
        Vec2::new(l + 1.0, y0),
        Vec2::new(l + 1.0, main(l + 1.0) - t),
        Vec2::new(l * 0.5, apex - t),
        Vec2::new(-1.0, main(-1.0) - t),
    ];
    let gsill = 0.16;
    let c = timber_col(rng);
    p.get(Part::Timber).member(
        &w,
        &Rect::new(-F, y0, l + F, y0 + gsill).poly(),
        -DEEP,
        F,
        CHAMFER,
        Vec2::X,
        c,
    );
    // Slope timbers along the roof line.
    for (i, (a, b)) in [
        (
            Vec2::new(-F - 0.02, line(-F - 0.02)),
            Vec2::new(l * 0.5, apex),
        ),
        (
            Vec2::new(l + F + 0.02, line(l + F + 0.02)),
            Vec2::new(l * 0.5, apex),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let d = (b - a).normalize();
        let nrm = up_normal(d);
        let poly = [a, b, b - nrm * 0.2, a - nrm * 0.2];
        let poly = clip(&poly, Vec2::Y, y0 + gsill);
        let c = timber_col(rng);
        p.get(Part::Timber)
            .member(&w, &poly, -DEEP, F - i as f32 * 0.003, CHAMFER, d, c);
    }
    // Posts in line with the floor below.
    let (ui0, ui1) = (CORNER_W - F, l + F - CORNER_W);
    let n = (((ui1 - ui0) / 1.25).round() as usize).max(1);
    let bw = (ui1 - ui0) / n as f32;
    let collar = y0 + 2.25;
    let has_collar = apex - collar > 1.2;
    let mut holes = Vec::new();
    let mut posts = Vec::new();
    for k in 0..=n {
        let u = ui0 + k as f32 * bw;
        let u = if k == 0 {
            u - POST_W * 0.5 + 0.02
        } else if k == n {
            u + POST_W * 0.5 - 0.02
        } else {
            u
        };
        posts.push(u);
        let poly = clip_to(
            &Rect::new(u - POST_W * 0.5, y0 + gsill, u + POST_W * 0.5, apex).poly(),
            &inner,
        );
        let c = timber_col(rng);
        p.get(Part::Timber)
            .member(&w, &poly, -DEEP, F, CHAMFER, Vec2::Y, c);
    }
    if has_collar {
        let poly = clip_to(&Rect::new(0.0, collar, l, collar + 0.16).poly(), &inner);
        let c = timber_col(rng);
        p.get(Part::Timber)
            .member(&w, &poly, -DEEP, F - 0.003, CHAMFER, Vec2::X, c);
    }
    // Attic windows in the bays that fit under the roof.
    let (wy0, wy1) = (y0 + gsill + 0.75, y0 + gsill + 1.75);
    let mut windows = Vec::new();
    for k in 0..n {
        let (a, b) = (posts[k] + POST_W * 0.5, posts[k + 1] - POST_W * 0.5);
        let fits = |u: f32| line(u) - t - 0.2 > wy1 + RAIL_H;
        let central = (k as f32 + 0.5 - n as f32 * 0.5).abs() < 1.6;
        if !(fits(a) && fits(b)) || !central || (k % 2 == 0 && n > 3) {
            // A rail at window height keeps the frame pattern going.
            let poly = clip_to(&Rect::new(a, wy0 - RAIL_H, b, wy0).poly(), &inner);
            let c = timber_col(rng);
            p.get(Part::Timber)
                .member(&w, &poly, -DEEP, F - 0.004, CHAMFER, Vec2::X, c);
            continue;
        }
        let r = Rect::new(a, wy0, b, wy1);
        for (ya, yb) in [(wy0 - RAIL_H, wy0), (wy1, wy1 + RAIL_H)] {
            let c = timber_col(rng);
            p.get(Part::Timber).member(
                &w,
                &Rect::new(a, ya, b, yb).poly(),
                -DEEP,
                F - 0.004,
                CHAMFER,
                Vec2::X,
                c,
            );
        }
        holes.push(r);
        windows.push(r);
    }
    // A small round-headed vent near the apex.
    if has_collar && apex - (collar + 0.16) > 0.9 && !p.far() {
        let cu = l * 0.5;
        let r = Rect::new(cu - 0.22, collar + 0.3, cu + 0.22, collar + 0.82);
        if line(r.min.x) - t > r.max.y + 0.05 {
            holes.push(r);
            windows.push(r);
        }
    }
    let seed = spec.seed + side as u64 * 3;
    let shade = move |q: Vec2| 1.0 + 0.07 * perlin2(q.x as f64 * 0.7, q.y as f64 * 0.7, seed);
    p.get(Part::Plaster)
        .plaster(&w, &outline, &holes, &shade, Vec3::ONE);
    for r in windows {
        window(p, &w, r, spec, rng, false, false);
    }
}

/// A window in the opening `r` (between timbers): frame, two casements with glazing bars,
/// glass, an outside sill, optional shutters and a flower box.
fn window(
    p: &mut Parts,
    w: &Wall,
    r: Rect,
    spec: &HouseSpec,
    rng: &mut Rng,
    shutters: bool,
    flower_box: bool,
) {
    let far = p.far();
    let paint = spec.window_paint;
    let fw = 0.06;
    let pm = p.get(Part::Paint);
    // Frame.
    for q in [
        Rect::new(r.min.x, r.min.y, r.min.x + fw, r.max.y),
        Rect::new(r.max.x - fw, r.min.y, r.max.x, r.max.y),
        Rect::new(r.min.x + fw, r.max.y - fw, r.max.x - fw, r.max.y),
        Rect::new(r.min.x + fw, r.min.y, r.max.x - fw, r.min.y + fw),
    ] {
        pm.member(w, &q.poly(), -0.13, -0.05, 0.008, Vec2::X, paint);
    }
    let inner = r.shrink(fw);
    // Glass, one cell of the glow atlas per window.
    let cell = Vec2::new(
        rng.range_i32(0, GLASS_CELLS as i32) as f32,
        rng.range_i32(0, GLASS_CELLS as i32) as f32,
    );
    let gd = -0.09;
    let pts = [
        w.at(inner.min, gd),
        w.at(Vec2::new(inner.max.x, inner.min.y), gd),
        w.at(inner.max, gd),
        w.at(Vec2::new(inner.min.x, inner.max.y), gd),
    ];
    let cuv =
        |a: f32, b: f32| (cell + Vec2::new(0.06 + 0.88 * a, 0.06 + 0.88 * b)) / GLASS_CELLS as f32;
    let uvs = [cuv(0.0, 1.0), cuv(1.0, 1.0), cuv(1.0, 0.0), cuv(0.0, 0.0)];
    p.get(Part::Glass).poly(&pts, &uvs, &[Vec3::ONE], w.n);
    if !far {
        // Casements: a mullion, leaf frames and glazing bars.
        let pm = p.get(Part::Paint);
        let two = inner.w() > 0.6;
        let leaves = if two {
            let mid = inner.center().x;
            pm.member(
                w,
                &Rect::new(mid - 0.025, inner.min.y, mid + 0.025, inner.max.y).poly(),
                -0.12,
                -0.055,
                0.006,
                Vec2::Y,
                paint,
            );
            vec![
                Rect::new(inner.min.x, inner.min.y, mid - 0.025, inner.max.y),
                Rect::new(mid + 0.025, inner.min.y, inner.max.x, inner.max.y),
            ]
        } else {
            vec![inner]
        };
        let lf = 0.042;
        for leaf in leaves {
            for q in [
                Rect::new(leaf.min.x, leaf.min.y, leaf.min.x + lf, leaf.max.y),
                Rect::new(leaf.max.x - lf, leaf.min.y, leaf.max.x, leaf.max.y),
                Rect::new(
                    leaf.min.x + lf,
                    leaf.max.y - lf,
                    leaf.max.x - lf,
                    leaf.max.y,
                ),
                Rect::new(
                    leaf.min.x + lf,
                    leaf.min.y,
                    leaf.max.x - lf,
                    leaf.min.y + lf * 1.4,
                ),
            ] {
                pm.member(w, &q.poly(), -0.12, -0.062, 0.006, Vec2::X, paint * 0.97);
            }
            let panes = if leaf.h() > 0.9 { 3 } else { 2 };
            for i in 1..panes {
                let y = leaf.min.y + leaf.h() * i as f32 / panes as f32;
                pm.member(
                    w,
                    &Rect::new(leaf.min.x + lf, y - 0.012, leaf.max.x - lf, y + 0.012).poly(),
                    -0.11,
                    -0.07,
                    0.004,
                    Vec2::X,
                    paint,
                );
            }
        }
    }
    // Outside sill: a sloped board (or a stone).
    let (sm, sc, thick) = if spec.stone_sills {
        (Part::Stone, Vec3::new(0.62, 0.6, 0.56), 0.07)
    } else {
        (Part::Timber, timber_col(rng), 0.045)
    };
    let tilt = Quat::from_axis_angle(w.u, 0.1);
    let depth_out = F + 0.06;
    let center = w.at(
        Vec2::new(r.center().x, r.min.y - thick * 0.5 + 0.01),
        (depth_out - 0.12) * 0.5,
    );
    p.get(sm).cbox(
        center,
        [w.u, tilt * Vec3::Y, tilt * w.n],
        Vec3::new(r.w() * 0.5 + 0.06, thick * 0.5, (depth_out + 0.12) * 0.5),
        0.01,
        sc,
        0,
    );
    if shutters {
        shutter_pair(p, w, r, spec, rng);
    }
    if flower_box {
        window_box(p, w, r, spec, rng);
    }
}

/// Board-and-batten shutters folded back against the wall on both sides of `r`.
fn shutter_pair(p: &mut Parts, w: &Wall, r: Rect, spec: &HouseSpec, rng: &mut Rng) {
    let far = p.far();
    let leaf_w = r.w() * 0.5 + 0.03;
    let y0 = r.min.y - 0.03;
    let y1 = r.max.y + 0.03;
    let d0 = F + 0.006;
    for side in [-1.0f32, 1.0] {
        let (u0, u1) = if side < 0.0 {
            (r.min.x - 0.04 - leaf_w, r.min.x - 0.04)
        } else {
            (r.max.x + 0.04, r.max.x + 0.04 + leaf_w)
        };
        let pm = p.get(Part::Paint);
        if far {
            pm.member(
                w,
                &Rect::new(u0, y0, u1, y1).poly(),
                d0,
                d0 + 0.03,
                0.0,
                Vec2::Y,
                spec.shutter_paint,
            );
            continue;
        }
        let boards = 3;
        let bw = (u1 - u0) / boards as f32;
        for b in 0..boards {
            let c = spec.shutter_paint * rng.range(0.88, 1.1);
            let a = u0 + b as f32 * bw + 0.003;
            pm.member(
                w,
                &Rect::new(a, y0, a + bw - 0.006, y1).poly(),
                d0,
                d0 + 0.026 + rng.range(-0.002, 0.002),
                0.004,
                Vec2::Y,
                c,
            );
        }
        // Battens and a diagonal brace (the inside of the shutter faces out when open).
        let bh = 0.09;
        let mut battens = Vec::new();
        for y in [y0 + 0.12, y1 - 0.12 - bh] {
            battens.push(Rect::new(u0 + 0.03, y, u1 - 0.03, y + bh));
        }
        for bt in &battens {
            pm.member(
                w,
                &bt.poly(),
                d0 + 0.02,
                d0 + 0.05,
                0.006,
                Vec2::X,
                spec.shutter_paint * 0.95,
            );
        }
        let (lo, hi) = (battens[0], battens[1]);
        let (s, e) = if side < 0.0 {
            (
                Vec2::new(u1 - 0.06, lo.max.y),
                Vec2::new(u0 + 0.06, hi.min.y),
            )
        } else {
            (
                Vec2::new(u0 + 0.06, lo.max.y),
                Vec2::new(u1 - 0.06, hi.min.y),
            )
        };
        let poly = clip_to(
            &band(s, e, 0.08),
            &Rect::new(u0, lo.max.y, u1, hi.min.y).poly(),
        );
        pm.member(
            w,
            &poly,
            d0 + 0.02,
            d0 + 0.045,
            0.005,
            (e - s).normalize(),
            spec.shutter_paint * 0.93,
        );
        // Strap hinges toward the window.
        let im = p.get(Part::Iron);
        for bt in &battens {
            let y = bt.center().y;
            let (ha, hb) = if side < 0.0 {
                (u1 - 0.32, u1 + 0.02)
            } else {
                (u0 - 0.02, u0 + 0.32)
            };
            im.member(
                w,
                &Rect::new(ha, y - 0.018, hb, y + 0.018).poly(),
                d0 + 0.05,
                d0 + 0.056,
                0.0,
                Vec2::X,
                Vec3::ONE,
            );
        }
    }
}

/// A painted flower box on iron brackets under the window, full of plants.
fn window_box(p: &mut Parts, w: &Wall, r: Rect, spec: &HouseSpec, rng: &mut Rng) {
    let far = p.far();
    let (u0, u1) = (r.min.x - 0.02, r.max.x + 0.02);
    let (y0, y1) = (r.min.y - 0.27, r.min.y - 0.08);
    let (d0, d1) = (F + 0.01, F + 0.22);
    let col = if spec.shutters {
        spec.shutter_paint
    } else {
        spec.door_paint
    } * 1.1;
    let center = w.at(Vec2::new((u0 + u1) * 0.5, (y0 + y1) * 0.5), (d0 + d1) * 0.5);
    p.get(Part::Paint).cbox(
        center,
        [w.u, Vec3::Y, w.n],
        Vec3::new((u1 - u0) * 0.5, (y1 - y0) * 0.5, (d1 - d0) * 0.5),
        0.01,
        col,
        0,
    );
    if far {
        return;
    }
    // Brackets.
    for u in [u0 + 0.12, u1 - 0.12] {
        let a = w.at(Vec2::new(u, y0 - 0.16), F);
        let b = w.at(Vec2::new(u, y0), d1 - 0.04);
        p.get(Part::Iron)
            .sweep(&[a, b], &|_| 0.008, 5, Vec3::ONE, false);
    }
    // Plants: leafy mounds with flowers on top, a few trailing down the front.
    let leaf = Vec3::new(0.05, 0.12, 0.03);
    let flower = [
        Vec3::new(0.62, 0.03, 0.03),
        Vec3::new(0.75, 0.22, 0.32),
        Vec3::new(0.8, 0.78, 0.72),
        Vec3::new(0.35, 0.09, 0.5),
    ][rng.range_i32(0, 4) as usize];
    let pl = p.get(Part::Plants);
    let n = ((u1 - u0) / 0.16) as usize;
    for i in 0..n {
        let u = u0 + 0.07 + (i as f32 + rng.range(-0.3, 0.3)) / n as f32 * (u1 - u0 - 0.14);
        let d = rng.range(d0 + 0.06, d1 - 0.05);
        let s = rng.range(0.07, 0.11);
        let c = w.at(Vec2::new(u, y1 + s * 0.5), d);
        pl.blob(
            c,
            Vec3::new(s, s * 0.8, s),
            leaf * rng.range(0.8, 1.2),
            rng.next_u64(),
        );
        for _ in 0..3 {
            let o = Vec3::new(
                rng.range(-1.0, 1.0),
                rng.range(0.3, 1.0),
                rng.range(-1.0, 1.0),
            )
            .normalize()
                * s
                * 0.9;
            pl.blob(
                c + o,
                Vec3::splat(rng.range(0.022, 0.034)),
                flower * rng.range(0.85, 1.15),
                rng.next_u64(),
            );
        }
        if rng.chance(0.4) {
            let top = w.at(Vec2::new(u, y1), d1);
            pl.blob(
                top + Vec3::Y * -0.12 + w.n * 0.03,
                Vec3::new(0.05, 0.14, 0.04),
                leaf * 0.9,
                rng.next_u64(),
            );
        }
    }
}

/// A plank door with strap hinges, a ring pull and stone steps up to it.
fn door(p: &mut Parts, w: &Wall, r: Rect, spec: &HouseSpec, rng: &mut Rng, fl: &Floor) {
    let far = p.far();
    let pm = p.get(Part::Paint);
    let (d0, d1) = (-0.11, -0.065);
    if far {
        pm.member(w, &r.poly(), d0, d1, 0.0, Vec2::Y, spec.door_paint);
    } else {
        let boards = ((r.w() / 0.17).round() as usize).max(3);
        let bw = r.w() / boards as f32;
        for b in 0..boards {
            let u = r.min.x + b as f32 * bw;
            let c = spec.door_paint * rng.range(0.85, 1.12);
            pm.member(
                w,
                &Rect::new(u + 0.003, r.min.y + 0.01, u + bw - 0.003, r.max.y).poly(),
                d0,
                d1 + rng.range(-0.003, 0.003),
                0.005,
                Vec2::Y,
                c,
            );
        }
        // Iron: two strap hinges with round ends, a ring pull on a plate, a keyhole plate.
        let im = p.get(Part::Iron);
        for y in [r.min.y + 0.35, r.max.y - 0.35] {
            im.member(
                w,
                &Rect::new(r.min.x + 0.02, y - 0.025, r.min.x + r.w() * 0.72, y + 0.025).poly(),
                d1,
                d1 + 0.007,
                0.0,
                Vec2::X,
                Vec3::ONE,
            );
            let end = w.at(Vec2::new(r.min.x + r.w() * 0.72, y), d1);
            im.lathe_axis(
                end,
                w.n,
                &[
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.04, 0.0),
                    Vec2::new(0.04, 0.006),
                    Vec2::new(0.0, 0.009),
                ],
                10,
                Vec3::ONE,
                0.0,
            );
        }
        let ring_c = w.at(Vec2::new(r.max.x - 0.16, r.min.y + 1.0), d1 + 0.012);
        im.lathe_axis(
            ring_c + Vec3::Y * 0.05,
            w.n,
            &[
                Vec2::new(0.0, -0.012),
                Vec2::new(0.035, -0.012),
                Vec2::new(0.035, -0.006),
                Vec2::new(0.0, 0.0),
            ],
            10,
            Vec3::ONE,
            0.0,
        );
        let ring: Vec<Vec3> = (0..=16)
            .map(|i| {
                let a = i as f32 / 16.0 * std::f32::consts::TAU;
                ring_c + (w.u * a.sin() + Vec3::Y * a.cos()) * 0.05
            })
            .collect();
        im.sweep(&ring, &|_| 0.007, 6, Vec3::ONE, false);
    }
    // Steps: rough stone blocks from the threshold down to the ground.
    let rise = fl.sill_top();
    let n = ((rise / 0.19).ceil() as usize).clamp(1, 4);
    let step_h = rise / n as f32;
    let sm = p.get(Part::Stone);
    for i in 0..n {
        let top = rise - i as f32 * step_h;
        let depth = 0.34 * (i + 1) as f32;
        let wide = r.w() * 0.5 + 0.25 + i as f32 * 0.06;
        let c = Vec3::new(0.62, 0.6, 0.56) * rng.range(0.85, 1.05);
        let center = w.at(
            Vec2::new(r.center().x, (top + -0.4) * 0.5),
            PLINTH_OUT + depth * 0.5 - 0.06,
        );
        sm.cbox_rough(
            center,
            [w.u, Vec3::Y, w.n],
            Vec3::new(wide, (top + 0.4) * 0.5, depth * 0.5 + 0.06),
            0.03,
            c,
            0,
            0.008,
            spec.seed * 77 + i as u64,
        );
    }
}

/// Tile courses, ridge caps, deck, rafter tails and barge boards.
fn roof_build(p: &mut Parts, spec: &HouseSpec, roof: &RoofShape, top: &Floor, rng: &mut Rng) {
    let far = p.far();
    let prof = roof.profile();
    // The tiles' base line (deck + battens above the rafters).
    let base = offset_line(&prof, DECK);
    let total = line_len(&base);
    let courses = ((total - 0.06) / TILE_EXPOSURE).floor().max(1.0) as usize;
    let e = (total - 0.06) / courses as f32;
    let (z0, z1) = (roof.z0, roof.z1);
    let xc = roof.xc;
    let lip = 0.024;
    let over = 0.09;
    let up_n = up_normal;
    for sgn in [1.0f32, -1.0] {
        let to3 = |q: Vec2, z: f32| Vec3::new(xc + sgn * q.x, q.y, z);
        let n3 = |n: Vec2| Vec3::new(sgn * n.x, n.y, 0.0);
        let rm = p.get(Part::Roof);
        if far {
            // Two textured slabs per side.
            for k in 0..base.len() - 1 {
                let (a, b) = (base[k], base[k + 1]);
                let s0: f32 = base[..=k].windows(2).map(|w| w[0].distance(w[1])).sum();
                let s1 = s0 + a.distance(b);
                let n = up_n((b - a).normalize());
                let (a, b) = (a + n * lip * 0.5, b + n * lip * 0.5);
                let v = |s: f32| -s / (TILE_EXPOSURE * TILE_ROWS as f32);
                let u = |z: f32| (z - z0) / (TILE_WIDTH * TILES_ACROSS as f32);
                let pts = [to3(a, z0), to3(a, z1), to3(b, z1), to3(b, z0)];
                let uvs = [
                    Vec2::new(u(z0), v(s0)),
                    Vec2::new(u(z1), v(s0)),
                    Vec2::new(u(z1), v(s1)),
                    Vec2::new(u(z0), v(s1)),
                ];
                rm.poly(&pts, &uvs, &[Vec3::ONE], n3(n));
            }
        } else {
            for k in 0..courses {
                let s = k as f32 * e;
                let row = (TILE_ROWS as i32 * 1000 - k as i32).rem_euclid(TILE_ROWS as i32);
                let shift: f32 = if row % 2 == 1 { 0.5 } else { 0.0 };
                let (pa, da) = along(&base, s);
                let (pb, db) = along(&base, (s + e + over).min(total));
                let na = up_n(da);
                let nb = up_n(db);
                let below = if k == 0 {
                    -0.07
                } else {
                    lip * over / (e + over)
                };
                // Tiles across the roof, each with its own small tilt and offset.
                let first = (-shift).floor() as i32;
                let last = ((z1 - z0) / TILE_WIDTH - shift).ceil() as i32;
                for i in first..last {
                    let za = (z0 + (i as f32 + shift) * TILE_WIDTH).max(z0);
                    let zb = (z0 + (i as f32 + 1.0 + shift) * TILE_WIDTH).min(z1);
                    if zb - za < 0.01 {
                        continue;
                    }
                    let h = hash01(i as i64, k as i64, sgn as i64, spec.seed);
                    let h2 = hash01(i as i64, k as i64, 7, spec.seed);
                    let ja = lip + (h - 0.5) * 0.008;
                    let jb = lip + (h2 - 0.5) * 0.008;
                    let ds = (hash01(i as i64, k as i64, 9, spec.seed) - 0.5) * 0.012;
                    let la = pa + da * ds + na * ja;
                    let lb = pa + da * ds + na * jb;
                    let lo_a = pa + da * ds + na * below;
                    let ta = pb + nb * 0.0;
                    let u = |z: f32| (z - z0) / (TILE_WIDTH * TILES_ACROSS as f32);
                    let v_lip = (row + 1) as f32 / TILE_ROWS as f32 - 0.002;
                    let v_top = (row as f32 + 1.0 - (e + over) / e) / TILE_ROWS as f32;
                    let shade = 0.9 + 0.2 * hash01(i as i64, k as i64, 3, spec.seed);
                    let col = Vec3::splat(shade);
                    let top = [to3(la, za), to3(lb, zb), to3(ta, zb), to3(ta, za)];
                    let uvs = [
                        Vec2::new(u(za), v_lip),
                        Vec2::new(u(zb), v_lip),
                        Vec2::new(u(zb), v_top),
                        Vec2::new(u(za), v_top),
                    ];
                    rm.poly(&top, &uvs, &[col], n3(na));
                    let lip_pts = [to3(lo_a, za), to3(lo_a, zb), to3(lb, zb), to3(la, za)];
                    let uvl = [
                        Vec2::new(u(za), v_lip),
                        Vec2::new(u(zb), v_lip),
                        Vec2::new(u(zb), v_lip - 0.01),
                        Vec2::new(u(za), v_lip - 0.01),
                    ];
                    rm.poly(&lip_pts, &uvl, &[col * 0.8], n3(-da));
                    // Course ends at the verges.
                    for (z, l, end_n) in [(za, la, -1.0f32), (zb, lb, 1.0)] {
                        if (z - z0).abs() < 1e-3 || (z - z1).abs() < 1e-3 {
                            let pts = [to3(lo_a, z), to3(l, z), to3(ta, z)];
                            let uvs = [
                                Vec2::new(u(z), v_lip),
                                Vec2::new(u(z) + 0.01, v_lip),
                                Vec2::new(u(z), v_top),
                            ];
                            rm.poly(&pts, &uvs, &[col * 0.7], Vec3::Z * end_n);
                        }
                    }
                }
            }
        }
        // Deck underside (soffit boards) and its front edge at the eaves.
        let tm = p.get(Part::Timber);
        for k in 0..prof.len() - 1 {
            let (a, b) = (prof[k], prof[k + 1]);
            let pts = [to3(a, z0), to3(a, z1), to3(b, z1), to3(b, z0)];
            tm.face(
                &pts,
                -n3(up_n((b - a).normalize())),
                Vec3::Z,
                Vec3::X,
                Vec3::splat(0.75),
            );
        }
        let (a, b) = (prof[0], base[0]);
        let lo = a - Vec2::Y * 0.02;
        let pts = [to3(lo, z0), to3(lo, z1), to3(b, z1), to3(b, z0)];
        tm.face(
            &pts,
            n3(Vec2::new(1.0, 0.0)),
            Vec3::Z,
            Vec3::Y,
            Vec3::splat(0.6),
        );
        // Rafter tails under the eaves.
        let kick = (prof[0] - prof[1]).normalize();
        let tail_n = up_n(kick);
        let count = ((top.z1 - top.z0 - 0.4) / 0.85).round().max(2.0) as usize;
        for i in 0..=count {
            let z = top.z0 + 0.2 + i as f32 / count as f32 * (top.z1 - top.z0 - 0.4);
            let start = prof[1] - kick * 0.25;
            let end = prof[0] - kick * 0.03;
            let mid = (start + end) * 0.5 - tail_n * 0.08;
            let len = start.distance(end) * 0.5;
            let c = timber_col(rng) * 0.9;
            tm.cbox(
                to3(mid, z),
                [n3(kick), n3(tail_n), Vec3::Z],
                Vec3::new(len, 0.08, 0.055),
                0.01,
                c,
                1 << 3,
            );
        }
        // Verge rafters and barge boards along both gables.
        for (z, out) in [(z0, -1.0f32), (z1, 1.0)] {
            for k in 0..prof.len() - 1 {
                let (a, b) = (prof[k], prof[k + 1]);
                let d = (b - a).normalize();
                let n = up_n(d);
                let c = timber_col(rng);
                // Barge board: covers deck and batten edges.
                let board = Wall {
                    o: Vec3::new(xc, 0.0, z),
                    u: Vec3::X * sgn,
                    n: Vec3::Z * out,
                    len: 0.0,
                    through: false,
                };
                let ext = if k == 0 { 0.03 } else { 0.0 };
                let a2 = a - d * ext;
                let b2 = if k == prof.len() - 2 { b + d * 0.03 } else { b };
                let poly = [
                    Vec2::new(a2.x, a2.y) - n * 0.16,
                    Vec2::new(b2.x, b2.y) - n * 0.16,
                    Vec2::new(b2.x, b2.y) + n * (DECK + 0.012),
                    Vec2::new(a2.x, a2.y) + n * (DECK + 0.012),
                ];
                tm.member(&board, &poly, -0.035, 0.0, 0.008, d, c);
            }
        }
    }
    // Ridge caps: half-round tiles, each overlapping the next, on a mortar bed.
    let ridge = Vec2::new(0.0, roof.ridge_y())
        + Vec2::Y * (DECK + 0.02) * (1.0 + roof.tan * roof.tan).sqrt();
    let rm = p.get(Part::Roof);
    let cap_len = 0.4;
    let n_caps = ((z1 - z0) / (cap_len - 0.04)).ceil() as usize;
    let tile_uv = |a: f32, b: f32| {
        Vec2::new(
            (0.15 + 0.7 * a) / TILES_ACROSS as f32,
            (0.15 + 0.6 * b) / TILE_ROWS as f32,
        )
    };
    if far {
        let r = 0.13;
        let pts = [
            Vec3::new(xc + r, ridge.y - 0.05, z0),
            Vec3::new(xc + r, ridge.y - 0.05, z1),
            Vec3::new(xc, ridge.y + r, z1),
            Vec3::new(xc, ridge.y + r, z0),
        ];
        let uvs = [
            tile_uv(0.0, 0.0),
            tile_uv(1.0, 0.0),
            tile_uv(1.0, 1.0),
            tile_uv(0.0, 1.0),
        ];
        rm.poly(&pts, &uvs, &[Vec3::splat(0.8)], Vec3::new(1.0, 1.0, 0.0));
        let pts = [
            Vec3::new(xc - r, ridge.y - 0.05, z1),
            Vec3::new(xc - r, ridge.y - 0.05, z0),
            Vec3::new(xc, ridge.y + r, z0),
            Vec3::new(xc, ridge.y + r, z1),
        ];
        rm.poly(&pts, &uvs, &[Vec3::splat(0.8)], Vec3::new(-1.0, 1.0, 0.0));
    } else {
        let segs = 9;
        for c in 0..n_caps {
            let za = z0 + c as f32 * (cap_len - 0.04) - 0.02;
            let zb = (za + cap_len).min(z1 + 0.03);
            let (ra, rb) = (0.135, 0.12);
            let shade = 0.8 + 0.15 * hash01(c as i64, 0, 5, spec.seed);
            let b = rm.mesh.vertices.len() as u32;
            for s in 0..=segs {
                let a = PI * s as f32 / segs as f32;
                let dir = Vec3::new(a.cos(), a.sin(), 0.0);
                for (z, r, t) in [(za, ra, 0.0), (zb, rb, 1.0)] {
                    let pos = Vec3::new(xc, ridge.y - 0.04, z) + dir * r;
                    rm.mesh.vertices.push(
                        Vertex::new(pos, dir, tile_uv(s as f32 / segs as f32, t))
                            .with_color(Vec3::splat(shade).extend(1.0)),
                    );
                }
            }
            for s in 0..segs {
                let k = b + s * 2;
                // Outward faces: winding checked against the radial normal.
                rm.mesh
                    .indices
                    .extend_from_slice(&[k, k + 2, k + 1, k + 1, k + 2, k + 3]);
            }
            // The cap's visible rim at its wide end.
            let rim: Vec<Vec3> = (0..=segs)
                .map(|s| {
                    let a = PI * s as f32 / segs as f32;
                    Vec3::new(xc, ridge.y - 0.04, za) + Vec3::new(a.cos(), a.sin(), 0.0) * ra
                })
                .collect();
            for s in 0..segs as usize {
                let (o0, o1) = (rim[s], rim[s + 1]);
                let c0 = Vec3::new(xc, ridge.y - 0.04, za);
                let i0 = c0 + (o0 - c0) * 0.86;
                let i1 = c0 + (o1 - c0) * 0.86;
                let uvs = [
                    tile_uv(0.0, 0.0),
                    tile_uv(0.1, 0.0),
                    tile_uv(0.1, 0.1),
                    tile_uv(0.0, 0.1),
                ];
                rm.poly(
                    &[o0, o1, i1, i0],
                    &uvs,
                    &[Vec3::splat(shade * 0.75)],
                    -Vec3::Z,
                );
            }
        }
        // Mortar beds along both sides.
        let pm = p.get(Part::Plaster);
        for sgn in [1.0f32, -1.0] {
            let x = xc + sgn * 0.125;
            let pts = [
                Vec3::new(x, ridge.y - 0.04, z0),
                Vec3::new(x, ridge.y - 0.04, z1),
                Vec3::new(x + sgn * 0.035, ridge.y - 0.1, z1),
                Vec3::new(x + sgn * 0.035, ridge.y - 0.1, z0),
            ];
            pm.face(
                &pts,
                Vec3::new(sgn, 0.6, 0.0),
                Vec3::Z,
                Vec3::Y,
                Vec3::splat(0.55),
            );
        }
    }
}

/// A brick chimney through the roof with a corbelled top, a stone cap and clay pots.
fn chimney(p: &mut Parts, spec: &HouseSpec, roof: &RoofShape, rng: &mut Rng) {
    let far = p.far();
    let z = (roof.z0 + roof.z1) * 0.5 + spec.chimney_at * (roof.z1 - roof.z0) * 0.3;
    let x = roof.xc + if spec.chimney_at > 0.0 { 0.35 } else { -0.3 };
    let (hx, hz) = (0.33, 0.27);
    let top = roof.ridge_y() + 1.1;
    let bottom = roof.y_top;
    let bm = p.get(Part::Brick);
    let start = bm.mark();
    // Axes X, Z, Y: the bricks' courses stay horizontal on every face.
    bm.cbox(
        Vec3::new(x, (bottom + top) * 0.5, z),
        [Vec3::X, Vec3::Z, Vec3::Y],
        Vec3::new(hx, hz, (top - bottom) * 0.5),
        0.01,
        Vec3::ONE,
        0,
    );
    // Soot toward the top.
    bm.shade_since(start, |q, c| {
        c * (1.0 - 0.45 * engine::core::math::smoothstep(top - 0.9, top, q.y))
    });
    for (dy, grow) in [(0.0, 0.04), (0.08, 0.07)] {
        bm.cbox(
            Vec3::new(x, top - 0.2 + dy, z),
            [Vec3::X, Vec3::Z, Vec3::Y],
            Vec3::new(hx + grow, hz + grow, 0.04),
            0.008,
            Vec3::splat(0.55),
            0,
        );
    }
    p.get(Part::Stone).abox(
        Vec3::new(x - hx - 0.1, top - 0.08, z - hz - 0.1),
        Vec3::new(x + hx + 0.1, top, z + hz + 0.1),
        0.012,
        0,
        Vec3::new(0.5, 0.49, 0.46),
    );
    if far {
        return;
    }
    let pots = if rng.chance(0.5) { 2 } else { 1 };
    for i in 0..pots {
        let px = if pots == 2 {
            x + (i as f32 - 0.5) * 0.3
        } else {
            x
        };
        let col = Vec3::new(0.62, 0.3, 0.18) * rng.range(0.8, 1.0);
        p.get(Part::Stone).lathe(
            Vec3::new(px, top, z),
            &[
                Vec2::new(0.11, 0.0),
                Vec2::new(0.1, 0.05),
                Vec2::new(0.085, 0.28),
                Vec2::new(0.1, 0.3),
                Vec2::new(0.1, 0.34),
                Vec2::new(0.075, 0.34),
                Vec2::new(0.07, 0.2),
            ],
            12,
            col,
            0.0,
        );
    }
}

/// Gabled dormers on the +X slope (the street side of eaves-front houses).
fn dormers(p: &mut Parts, spec: &HouseSpec, roof: &RoofShape, top: &Floor, rng: &mut Rng) {
    let n = spec.dormers as usize;
    let len = top.z1 - top.z0;
    let dw = 1.5;
    let x_front = roof.hw - 0.6;
    let y_floor = roof.deck(x_front) + DECK + 0.03;
    let wall_h = 1.25;
    let dpitch = 0.7f32;
    for i in 0..n {
        let zc = top.z0 + len * (i as f32 + 1.0) / (n as f32 + 1.0);
        let zc = if n == 1 { zc + len * 0.12 } else { zc };
        let (za, zb) = (zc - dw * 0.5, zc + dw * 0.5);
        let x = roof.xc + x_front;
        // Front wall frame: u along -Z (so it reads left to right from the street), n = +X.
        let w = Wall {
            o: Vec3::new(x, 0.0, zb),
            u: -Vec3::Z,
            n: Vec3::X,
            len: dw,
            through: true,
        };
        let y0 = y_floor - 0.25;
        let y1 = y_floor + wall_h;
        let apex = y1 + dw * 0.5 * dpitch;
        // Corner posts, sill, plate.
        for u in [0.0, dw] {
            let c = timber_col(rng);
            p.get(Part::Timber).member(
                &w,
                &Rect::new(u - 0.08, y0, u + 0.08, y1).poly(),
                -DEEP,
                F,
                CHAMFER,
                Vec2::Y,
                c,
            );
        }
        let c = timber_col(rng);
        p.get(Part::Timber).member(
            &w,
            &Rect::new(-0.12, y1 - 0.14, dw + 0.12, y1).poly(),
            -DEEP,
            F + 0.004,
            CHAMFER,
            Vec2::X,
            c,
        );
        let r = Rect::new(0.12, y_floor + 0.25, dw - 0.12, y1 - 0.18);
        let c = timber_col(rng);
        p.get(Part::Timber).member(
            &w,
            &Rect::new(0.08, r.min.y - 0.12, dw - 0.08, r.min.y).poly(),
            -DEEP,
            F - 0.003,
            CHAMFER,
            Vec2::X,
            c,
        );
        let outline = [
            Vec2::new(0.0, y0),
            Vec2::new(dw, y0),
            Vec2::new(dw, y1),
            Vec2::new(dw * 0.5, apex),
            Vec2::new(0.0, y1),
        ];
        p.get(Part::Plaster)
            .plaster(&w, &outline, &[r], &|_| 1.0, Vec3::ONE);
        window(p, &w, r, spec, rng, false, false);
        // Cheeks: plaster triangles from the front wall back to where they meet the roof.
        let depth_back = (apex - roof.deck(x_front)) / roof.tan + 0.3;
        for (z, s) in [(za, -1.0f32), (zb, 1.0)] {
            let back_x = x - depth_back;
            let pts = [
                Vec3::new(x, y0, z),
                Vec3::new(x, y1, z),
                Vec3::new(back_x, y1, z),
                Vec3::new(back_x, y0, z),
            ];
            let uvs: Vec<Vec2> = pts.iter().map(|q| Vec2::new(q.x, q.y)).collect();
            p.get(Part::Plaster)
                .poly(&pts, &uvs, &[Vec3::splat(0.93)], Vec3::Z * s);
        }
        // Gable roof of the dormer: two tiled slabs from the front back into the main roof.
        let over = 0.25;
        for s in [-1.0f32, 1.0] {
            let rm = p.get(Part::Roof);
            let eave = Vec3::new(0.0, y1 - over * dpitch, zc + s * (dw * 0.5 + over));
            let ridge_p = Vec3::new(0.0, apex + 0.06, zc);
            let x0 = x + 0.3;
            let x1 = x - depth_back - 0.3;
            let rows = ((ridge_p - eave).length() / TILE_EXPOSURE).max(1.0);
            let pts = [
                Vec3::new(x0, eave.y + 0.08, eave.z),
                Vec3::new(x1, eave.y + 0.08, eave.z),
                Vec3::new(x1, ridge_p.y + 0.08, zc),
                Vec3::new(x0, ridge_p.y + 0.08, zc),
            ];
            let u = |q: f32| q / (TILE_WIDTH * TILES_ACROSS as f32);
            let v1 = rows / TILE_ROWS as f32;
            let uvs = [
                Vec2::new(u(x0), v1),
                Vec2::new(u(x1), v1),
                Vec2::new(u(x1), 0.0),
                Vec2::new(u(x0), 0.0),
            ];
            rm.poly(&pts, &uvs, &[Vec3::ONE], Vec3::new(0.0, 1.0, s));
            // Underside and the front barge board.
            let under = [
                Vec3::new(x0, eave.y, eave.z),
                Vec3::new(x1, eave.y, eave.z),
                Vec3::new(x1, ridge_p.y, zc),
                Vec3::new(x0, ridge_p.y, zc),
            ];
            p.get(Part::Timber).face(
                &under,
                Vec3::new(0.0, -1.0, -s),
                Vec3::X,
                Vec3::Z,
                Vec3::splat(0.7),
            );
            let board = [
                Vec3::new(x0, eave.y - 0.1, eave.z),
                Vec3::new(x0, ridge_p.y - 0.1, zc),
                Vec3::new(x0, ridge_p.y + 0.1, zc),
                Vec3::new(x0, eave.y + 0.1, eave.z),
            ];
            let c = timber_col(rng);
            p.get(Part::Timber)
                .face(&board, Vec3::X, Vec3::Y, Vec3::Z, c);
            let rm = p.get(Part::Roof);
            let edge = [
                Vec3::new(x0, eave.y, eave.z),
                Vec3::new(x1, eave.y, eave.z),
                Vec3::new(x1, eave.y + 0.1, eave.z),
                Vec3::new(x0, eave.y + 0.1, eave.z),
            ];
            let e_uv = [
                Vec2::new(0.0, 0.24),
                Vec2::new(0.4, 0.24),
                Vec2::new(0.4, 0.23),
                Vec2::new(0.0, 0.23),
            ];
            rm.poly(&edge, &e_uv, &[Vec3::splat(0.7)], Vec3::Z * s);
        }
    }
}

// =============================================================================== street lamp

/// A cast and wrought-iron street lamp, 4.5 m tall: fluted base, rings, a ladder bar, scroll
/// brackets and a tapered four-sided lantern with a pyramid roof and finial. Returns the iron
/// and the glowing glass meshes; the light sits 3.9 m up.
pub fn lamp_post(far: bool) -> (Mesh, Mesh) {
    const SQ2: f32 = std::f32::consts::SQRT_2;
    let mut iron = Mb::new(far);
    let mut glass = Mb::new(far);
    let c = Vec3::ONE;
    let v = Vec2::new;
    // Post: one turned profile from the plinth to the lantern platform.
    iron.lathe(
        Vec3::ZERO,
        &[
            v(0.0, 0.0),
            v(0.2, 0.0),
            v(0.2, 0.06),
            v(0.17, 0.08),
            v(0.17, 0.12),
            v(0.14, 0.16),
            v(0.13, 0.3),
            v(0.1, 0.42),
            v(0.08, 0.6),
            v(0.095, 0.62),
            v(0.095, 0.68),
            v(0.07, 0.72),
            v(0.065, 0.9),
            v(0.058, 2.0),
            v(0.05, 3.0),
            v(0.065, 3.03),
            v(0.065, 3.08),
            v(0.05, 3.12),
            v(0.05, 3.3),
            v(0.07, 3.36),
            v(0.1, 3.5),
            v(0.14, 3.56),
            v(0.17, 3.58),
            v(0.17, 3.62),
            v(0.0, 3.62),
        ],
        16,
        c,
        0.0,
    );
    // Flutes on the shaft: thin ribs.
    if !far {
        for i in 0..8 {
            let a = i as f32 / 8.0 * std::f32::consts::TAU;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            let path = [d * 0.066 + Vec3::Y * 0.9, d * 0.052 + Vec3::Y * 2.95];
            iron.sweep(&path, &|_| 0.009, 4, c, true);
        }
    }
    // Ladder bar with ball ends.
    let bar_y = 2.85;
    iron.sweep(
        &[Vec3::new(-0.32, bar_y, 0.0), Vec3::new(0.32, bar_y, 0.0)],
        &|_| 0.014,
        8,
        c,
        false,
    );
    for s in [-1.0f32, 1.0] {
        iron.ball(Vec3::new(s * 0.34, bar_y, 0.0), 0.03, 10, c);
    }
    // Scroll brackets: a volute at the post curling into a sweep up to the platform's rim.
    if !far {
        for k in 0..4 {
            let a = k as f32 / 4.0 * std::f32::consts::TAU + std::f32::consts::FRAC_PI_4;
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            let mut path = Vec::new();
            let center = Vec2::new(0.11, 3.2);
            let turns = 1.3;
            let steps = 18;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let ang = PI * 0.5 + t * turns * std::f32::consts::TAU;
                let r = 0.012 + 0.045 * t;
                let q = center + Vec2::new(ang.cos(), ang.sin()) * r;
                path.push(out * q.x + Vec3::Y * q.y);
            }
            let start = *path.last().unwrap();
            let p0 = Vec2::new(start.dot(out), start.y);
            let p3 = Vec2::new(0.17, 3.58);
            let p1 = p0 + Vec2::new(0.06, 0.12);
            let p2 = p3 + Vec2::new(0.06, -0.12);
            for i in 1..=10 {
                let t = i as f32 / 10.0;
                let u = 1.0 - t;
                let q =
                    p0 * u * u * u + p1 * 3.0 * u * u * t + p2 * 3.0 * u * t * t + p3 * t * t * t;
                path.push(out * q.x + Vec3::Y * q.y);
            }
            // Attach to the post at the top as well.
            iron.sweep(&path, &|t| 0.007 + 0.004 * t, 6, c, true);
            iron.sweep(
                &[out * 0.05 + Vec3::Y * 3.15, out * 0.105 + Vec3::Y * 3.21],
                &|_| 0.008,
                5,
                c,
                false,
            );
        }
    }
    // Lantern: a square frustum, narrow at the bottom.
    let (yb, yt) = (3.64, 4.14);
    let (rb, rt) = (0.12, 0.17);
    let corner = |r: f32, y: f32, k: usize| {
        let a = k as f32 / 4.0 * std::f32::consts::TAU + std::f32::consts::FRAC_PI_4;
        Vec3::new(a.cos() * r, y, a.sin() * r)
            * Vec3::new(std::f32::consts::SQRT_2, 1.0, std::f32::consts::SQRT_2)
    };
    // Bottom tray and top ring.
    iron.lathe(
        Vec3::ZERO,
        &[
            v(0.0, 3.6),
            v(0.13 * SQ2, 3.6),
            v(0.135 * SQ2, 3.66),
            v(0.0, 3.66),
        ],
        4,
        c,
        std::f32::consts::FRAC_PI_4,
    );
    iron.lathe(
        Vec3::ZERO,
        &[
            v(0.18 * SQ2, 4.12),
            v(0.21 * SQ2, 4.12),
            v(0.21 * SQ2, 4.17),
            v(0.18 * SQ2, 4.17),
        ],
        4,
        c,
        std::f32::consts::FRAC_PI_4,
    );
    for k in 0..4 {
        let a = corner(rb, yb, k);
        let b = corner(rt, yt, k);
        iron.sweep(&[a, b], &|_| 0.011, 4, c, false);
        // Glass pane between this corner and the next.
        let (a2, b2) = (corner(rb, yb, (k + 1) % 4), corner(rt, yt, (k + 1) % 4));
        let inset = |q: Vec3| q * Vec3::new(0.97, 1.0, 0.97);
        let pts = [inset(a), inset(a2), inset(b2), inset(b)];
        let mid = (a + a2 + b + b2) * 0.25;
        let facing = Vec3::new(mid.x, 0.0, mid.z);
        let uvs = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
        glass.poly(&pts, &uvs, &[Vec3::ONE], facing);
        // A cross bar halfway up.
        if !far {
            iron.sweep(&[(a + b) * 0.5, (a2 + b2) * 0.5], &|_| 0.006, 4, c, false);
        }
    }
    glass.ball(Vec3::new(0.0, 3.86, 0.0), 0.05, 10, Vec3::ONE);
    // Roof: a flared four-sided pyramid, then a little dome and a finial.
    iron.lathe(
        Vec3::ZERO,
        &[
            v(0.0, 4.15),
            v(0.26 * SQ2, 4.15),
            v(0.26 * SQ2, 4.17),
            v(0.2 * SQ2, 4.24),
            v(0.12 * SQ2, 4.33),
            v(0.05 * SQ2, 4.42),
            v(0.0, 4.44),
        ],
        4,
        c,
        std::f32::consts::FRAC_PI_4,
    );
    iron.lathe(
        Vec3::ZERO,
        &[
            v(0.0, 4.4),
            v(0.05, 4.42),
            v(0.055, 4.46),
            v(0.03, 4.5),
            v(0.012, 4.53),
            v(0.02, 4.56),
            v(0.008, 4.62),
            v(0.0, 4.66),
        ],
        12,
        c,
        0.0,
    );
    if !far {
        for k in 0..4 {
            let q = corner(0.235, 4.19, k);
            iron.ball(q, 0.018, 8, c);
        }
    }
    (iron.mesh, glass.mesh)
}

// =============================================================================== props

/// Props: (part, mesh) lists like houses, built once and placed by the scenery.
pub struct PropMeshes {
    pub parts: Vec<(Part, Mesh)>,
}

fn finish(p: Parts) -> PropMeshes {
    PropMeshes {
        parts: Part::ALL
            .iter()
            .zip(p.m)
            .filter(|(_, mb)| !mb.mesh.indices.is_empty())
            .map(|(part, mb)| (*part, mb.mesh))
            .collect(),
    }
}

/// A wooden bench on two trestles with a back rest (1.6 m long, faces +Z).
pub fn bench(seed: u64) -> PropMeshes {
    let mut rng = Rng::new(seed);
    let mut p = Parts::new(false);
    let tm = p.get(Part::Timber);
    let len = 1.6;
    for i in 0..2 {
        let z = -0.05 + i as f32 * 0.17;
        let c = Vec3::splat(rng.range(0.9, 1.2));
        tm.abox(
            Vec3::new(-len * 0.5, 0.42, z),
            Vec3::new(len * 0.5, 0.47, z + 0.15),
            0.01,
            0,
            c,
        );
    }
    for x in [-len * 0.5 + 0.18, len * 0.5 - 0.18] {
        for z in [-0.03, 0.22] {
            tm.cbox(
                Vec3::new(x, 0.21, z),
                [Vec3::Y, Vec3::Z, Vec3::X],
                Vec3::new(0.21, 0.04, 0.035),
                0.01,
                Vec3::ONE,
                0,
            );
        }
        tm.abox(
            Vec3::new(x - 0.04, 0.36, -0.08),
            Vec3::new(x + 0.04, 0.42, 0.28),
            0.01,
            2,
            Vec3::ONE,
        );
        // Back posts.
        tm.cbox(
            Vec3::new(x, 0.66, -0.1),
            [Vec3::new(0.0, 1.0, -0.12).normalize(), Vec3::Z, Vec3::X],
            Vec3::new(0.26, 0.025, 0.035),
            0.008,
            Vec3::ONE,
            0,
        );
    }
    for y in [0.68, 0.84] {
        tm.cbox(
            Vec3::new(0.0, y, -0.12 - (y - 0.68) * 0.12),
            [Vec3::X, Vec3::new(0.0, 1.0, -0.12).normalize(), Vec3::Z],
            Vec3::new(len * 0.5, 0.05, 0.016),
            0.008,
            Vec3::splat(rng.range(0.9, 1.15)),
            0,
        );
    }
    finish(p)
}

/// A group of two or three oak barrels with iron hoops (one may lie on its side).
pub fn barrels(seed: u64) -> PropMeshes {
    let mut rng = Rng::new(seed);
    let mut p = Parts::new(false);
    let n = rng.range_i32(2, 4);
    for i in 0..n {
        let h = rng.range(0.8, 0.95);
        let r = rng.range(0.27, 0.32);
        let at = Vec3::new(i as f32 * (r * 2.05) - 0.3, 0.0, rng.range(-0.15, 0.15));
        barrel(&mut p, at, h, r, &mut rng);
    }
    finish(p)
}

fn barrel(p: &mut Parts, at: Vec3, h: f32, r: f32, rng: &mut Rng) {
    let staves = 18;
    let bulge = |y: f32| r * (0.86 + 0.14 * (PI * y / h).sin());
    let tm = p.get(Part::Timber);
    for s in 0..staves {
        let a0 = s as f32 / staves as f32 * std::f32::consts::TAU;
        let a1 = (s as f32 + 0.96) / staves as f32 * std::f32::consts::TAU;
        let c = Vec3::splat(rng.range(0.75, 1.15));
        let rows = 6;
        for k in 0..rows {
            let (y0, y1) = (h * k as f32 / rows as f32, h * (k + 1) as f32 / rows as f32);
            let q = |a: f32, y: f32| at + Vec3::new(a.cos() * bulge(y), y, a.sin() * bulge(y));
            let pts = [q(a0, y0), q(a1, y0), q(a1, y1), q(a0, y1)];
            let mid = (a0 + a1) * 0.5;
            tm.face(
                &pts,
                Vec3::new(mid.cos(), 0.0, mid.sin()),
                Vec3::Y,
                Vec3::X,
                c,
            );
        }
    }
    // Lid set into the chime.
    let lid_r = bulge(h) * 0.93;
    tm.lathe(
        at,
        &[
            Vec2::new(bulge(h), h),
            Vec2::new(lid_r, h),
            Vec2::new(lid_r, h - 0.025),
            Vec2::new(0.0, h - 0.025),
        ],
        staves,
        Vec3::splat(0.85),
        0.0,
    );
    let im = p.get(Part::Iron);
    for y in [0.07, 0.25, h - 0.25, h - 0.07] {
        let rr = bulge(y) + 0.004;
        im.lathe(
            at,
            &[
                Vec2::new(rr, y - 0.025),
                Vec2::new(rr + 0.004, y - 0.02),
                Vec2::new(rr + 0.004, y + 0.02),
                Vec2::new(rr, y + 0.025),
            ],
            staves * 2,
            Vec3::ONE,
            0.0,
        );
    }
}

/// Split logs stacked against a wall (faces +Z, about 2.2 m long), on two rails.
pub fn wood_pile(seed: u64) -> PropMeshes {
    let mut rng = Rng::new(seed);
    let mut p = Parts::new(false);
    let tm = p.get(Part::Timber);
    let len = 2.2;
    for z in [-0.12, 0.12] {
        tm.abox(
            Vec3::new(-len * 0.5, 0.0, z - 0.04),
            Vec3::new(len * 0.5, 0.08, z + 0.04),
            0.008,
            0,
            Vec3::splat(0.6),
        );
    }
    let mut y = 0.08;
    let rows = rng.range_i32(7, 10);
    for row in 0..rows {
        let r = 0.065;
        let mut x = -len * 0.5 + r + if row % 2 == 1 { r } else { 0.0 };
        let width = len - row as f32 * 0.03;
        while x < -len * 0.5 + width - r {
            let rr = r * rng.range(0.75, 1.1);
            let c = Vec3::new(0.95, 0.85, 0.7) * rng.range(0.75, 1.15);
            let z0 = -0.2 + rng.range(-0.03, 0.03);
            let z1 = 0.2 + rng.range(-0.03, 0.03);
            let center = Vec3::new(x, y + rr, 0.0);
            // Split logs: a wedge-ish 6-sided bar, the cut faces a little lighter.
            tm.sweep(
                &[center + Vec3::Z * z0, center + Vec3::Z * z1],
                &|_| rr,
                6,
                c,
                true,
            );
            x += r * 2.0 + rng.range(0.0, 0.01);
        }
        y += r * 1.75;
    }
    finish(p)
}

/// A wooden hand cart with two spoked wheels, tilted onto its shafts (faces +Z).
pub fn cart(seed: u64) -> PropMeshes {
    let mut rng = Rng::new(seed);
    let mut p = Parts::new(false);
    let wheel_r = 0.55;
    let tilt = 0.18f32;
    let rot = Quat::from_rotation_x(tilt);
    let axle = Vec3::new(0.0, wheel_r, 0.0);
    let tm = p.get(Part::Timber);
    // Bed and sides.
    let place = |q: Vec3| axle + rot * (q - axle);
    for i in 0..6 {
        let x = -0.6 + i as f32 * 0.2 + 0.1;
        tm.cbox(
            place(Vec3::new(x, 0.75, -0.2)),
            [rot * Vec3::Z, rot * Vec3::Y, rot * Vec3::X],
            Vec3::new(0.75, 0.02, 0.095),
            0.006,
            Vec3::splat(rng.range(0.8, 1.15)),
            0,
        );
    }
    for x in [-0.62, 0.62] {
        for y in [0.86, 1.02] {
            tm.cbox(
                place(Vec3::new(x, y, -0.2)),
                [rot * Vec3::Z, rot * Vec3::Y, rot * Vec3::X],
                Vec3::new(0.75, 0.06, 0.018),
                0.006,
                Vec3::splat(rng.range(0.8, 1.15)),
                0,
            );
        }
    }
    // Shafts reaching forward to the ground.
    for x in [-0.45, 0.45] {
        let a = place(Vec3::new(x, 0.7, -0.9));
        let b = place(Vec3::new(x, 0.7, 1.6));
        let b = Vec3::new(b.x, b.y.max(0.05), b.z);
        tm.sweep(&[a, b], &|t| 0.04 - 0.012 * t, 6, Vec3::splat(0.9), true);
    }
    // Wheels: rim, spokes, hub; iron tyres.
    for side in [-1.0f32, 1.0] {
        let tm = p.get(Part::Timber);
        let x = side * 0.78;
        let c = Vec3::new(x, wheel_r, 0.0);
        let rim: Vec<Vec3> = (0..=24)
            .map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                c + Vec3::new(0.0, a.cos(), a.sin()) * (wheel_r - 0.04)
            })
            .collect();
        tm.sweep(&rim, &|_| 0.035, 6, Vec3::splat(0.9), false);
        for s in 0..10 {
            let a = s as f32 / 10.0 * std::f32::consts::TAU;
            let d = Vec3::new(0.0, a.cos(), a.sin());
            tm.sweep(
                &[c + d * 0.08, c + d * (wheel_r - 0.06)],
                &|t| 0.022 - 0.006 * t,
                5,
                Vec3::ONE,
                false,
            );
        }
        tm.lathe_axis(
            c - Vec3::X * 0.09,
            Vec3::X,
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.07, 0.0),
                Vec2::new(0.09, 0.05),
                Vec2::new(0.09, 0.13),
                Vec2::new(0.06, 0.18),
                Vec2::new(0.0, 0.18),
            ],
            10,
            Vec3::splat(0.85),
            0.0,
        );
        let tyre: Vec<Vec3> = (0..=32)
            .map(|i| {
                let a = i as f32 / 32.0 * std::f32::consts::TAU;
                c + Vec3::new(0.0, a.cos(), a.sin()) * (wheel_r - 0.005)
            })
            .collect();
        p.get(Part::Iron)
            .sweep(&tyre, &|_| 0.012, 5, Vec3::ONE, false);
    }
    p.get(Part::Iron).sweep(
        &[
            Vec3::new(-0.85, wheel_r, 0.0),
            Vec3::new(0.85, wheel_r, 0.0),
        ],
        &|_| 0.025,
        8,
        Vec3::ONE,
        true,
    );
    finish(p)
}

// =============================================================================== the set

/// Every village mesh: each house archetype (near, far), the street lamp ((iron, glass) near
/// and far) and the street furniture variants.
pub struct VillageMeshes {
    pub houses: Vec<(HouseMeshes, HouseMeshes)>,
    pub lamp: ((Mesh, Mesh), (Mesh, Mesh)),
    pub bench: PropMeshes,
    pub barrels: Vec<PropMeshes>,
    pub wood_piles: Vec<PropMeshes>,
    pub cart: PropMeshes,
}

/// Build the whole set, the house archetypes in parallel.
pub fn build_meshes() -> VillageMeshes {
    std::thread::scope(|s| {
        let jobs: Vec<_> = archetypes()
            .into_iter()
            .map(|spec| s.spawn(move || (house(&spec, false), house(&spec, true))))
            .collect();
        let lamp = (lamp_post(false), lamp_post(true));
        let bench = bench(1);
        let barrels = (0..3).map(|i| barrels(10 + i)).collect();
        let wood_piles = (0..2).map(|i| wood_pile(20 + i)).collect();
        let cart = cart(1);
        VillageMeshes {
            houses: jobs
                .into_iter()
                .map(|j| j.join().expect("house mesh thread"))
                .collect(),
            lamp,
            bench,
            barrels,
            wood_piles,
            cart,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &Mesh, name: &str) {
        assert!(!m.indices.is_empty(), "{name} is empty");
        assert!(
            m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
            "{name} has bad indices"
        );
        assert!(
            m.vertices.iter().all(|v| v
                .position
                .iter()
                .chain(&v.normal)
                .chain(&v.uv)
                .all(|c| c.is_finite())),
            "{name} has NaNs"
        );
    }

    #[test]
    fn houses_are_valid() {
        for spec in archetypes() {
            for far in [false, true] {
                let h = house(&spec, far);
                for (part, m) in &h.parts {
                    check(m, &format!("house {} {part:?} far={far}", spec.seed));
                }
                assert!(h.half.x > spec.width * 0.4 && h.half.y > 2.0);
            }
            let near = house(&spec, false).triangles();
            let far = house(&spec, true).triangles();
            assert!(
                far * 8 < near,
                "far mesh should be much cheaper ({far} vs {near})"
            );
        }
    }

    #[test]
    fn faces_point_where_their_normals_say() {
        // Every triangle's winding agrees with its vertices' normals.
        let h = house(&archetypes()[1], false);
        for (part, m) in &h.parts {
            let mut bad = 0;
            for tri in m.indices.as_chunks::<3>().0 {
                let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
                let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
                if n.length_squared() < 1e-12 {
                    continue;
                }
                let vn = Vec3::from(m.vertices[tri[0] as usize].normal);
                if n.dot(vn) < 0.0 {
                    bad += 1;
                }
            }
            assert!(
                bad * 50 < m.indices.len() / 3,
                "{part:?}: {bad} of {} triangles wound backwards",
                m.indices.len() / 3
            );
        }
    }

    #[test]
    fn props_and_lamp_are_valid() {
        let (iron, glass) = lamp_post(false);
        check(&iron, "lamp iron");
        check(&glass, "lamp glass");
        for p in [bench(1), barrels(2), wood_pile(3), cart(4)] {
            for (part, m) in &p.parts {
                check(m, &format!("{part:?}"));
            }
        }
    }

    #[test]
    fn clipping_keeps_convex_pieces() {
        let sq = Rect::new(0.0, 0.0, 1.0, 1.0).poly();
        let half = clip(&sq, Vec2::X, 0.5);
        assert!((area(&half) - 0.5).abs() < 1e-5);
        let tri = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(1.0, 1.0),
        ];
        let piece = clip_to(&sq, &tri);
        assert!(area(&piece) > 0.0 && area(&piece) < 1.0);
    }
}
