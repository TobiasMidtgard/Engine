//! Implicit sculpting for the demo characters.
//!
//! A body is a list of shaped primitives (ellipsoids, round cones) blended with smooth unions,
//! carved with smooth subtractions and clipped by planes, each tagged with the bone it belongs
//! to. [`Sculpt::mesh`] samples the field on a sparse grid near its surface, meshes it with
//! surface nets (vertices projected onto the exact surface), simplifies it with quadric error
//! metrics, relaxes it into even triangles and takes normals from the field gradient, so the
//! mesh is dense where the shape is detailed (fingers, ears, face) and smooth everywhere.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use engine_core::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    Ellipsoid {
        center: Vec3,
        inv_rot: Quat,
        inv_r: Vec3,
        inv_r2: Vec3,
        min_r: f32,
    },
    RoundCone {
        a: Vec3,
        ba: Vec3,
        l2: f32,
        rr: f32,
        a2: f32,
        il2: f32,
        r1: f32,
        r2: f32,
    },
    Sphere {
        center: Vec3,
        radius: f32,
    },
    HalfSpace {
        normal: Vec3,
        offset: f32,
    },
    /// Index into [`Sculpt::lofts`].
    Loft(u32),
}

impl Shape {
    #[inline]
    fn distance(&self, p: Vec3, lofts: &[LoftData]) -> f32 {
        match *self {
            Shape::Ellipsoid {
                center,
                inv_rot,
                inv_r,
                inv_r2,
                min_r,
            } => {
                let q = inv_rot * (p - center);
                let k1 = (q * inv_r2).length();
                if k1 < 1e-9 {
                    return -min_r;
                }
                let k0 = (q * inv_r).length();
                k0 * (k0 - 1.0) / k1
            }
            Shape::RoundCone {
                a,
                ba,
                l2,
                rr,
                a2,
                il2,
                r1,
                r2,
            } => {
                // Inigo Quilez's exact round cone.
                let pa = p - a;
                let y = pa.dot(ba);
                let z = y - l2;
                let x2 = (pa * l2 - ba * y).length_squared();
                let y2 = y * y * l2;
                let z2 = z * z * l2;
                let k = rr.signum() * rr * rr * x2;
                if z.signum() * a2 * z2 > k {
                    return (x2 + z2).sqrt() * il2 - r2;
                }
                if y.signum() * a2 * y2 < k {
                    return (x2 + y2).sqrt() * il2 - r1;
                }
                ((x2 * a2 * il2).sqrt() + y * rr) * il2 - r1
            }
            Shape::Sphere { center, radius } => p.distance(center) - radius,
            Shape::HalfSpace { normal, offset } => normal.dot(p) - offset,
            Shape::Loft(i) => lofts[i as usize].distance(p),
        }
    }
}

/// How a part combines with what came before it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Smooth union.
    Add,
    /// Smooth subtraction (dents, grooves, sockets).
    Carve,
    /// Smooth intersection (flat soles).
    Clip,
    /// The next `len` parts blend among themselves first (with their own radii, so fingers
    /// don't melt into each other), then into the rest with this part's radius.
    Group(u32),
    /// Raise (or, negative, sink) the surface so far inside an ellipsoid, fading smoothly to
    /// nothing at its boundary: muscle definition, grooves and dents that sit exactly on the
    /// form instead of inflating it.
    Bump(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Part {
    shape: Shape,
    op: Op,
    /// Blend radius, meters.
    k: f32,
    bone: u8,
    /// Bounding sphere of the shape (culling).
    center: Vec3,
    radius: f32,
    /// Half extents of a group's bounding box around `center`.
    extent: Vec3,
}

/// Cubic smooth minimum (C2 continuous, so glossy reflections flow over the blends).
#[inline]
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * h * k * (1.0 / 6.0)
}

/// Cubic smooth maximum.
#[inline]
pub fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// A generalized cylinder: a cross-section swept along an axis, its shape given at keys and
/// interpolated smoothly between them (torsos, necks, limbs: one coherent silhouette instead
/// of a pile of blobs). Built with [`Loft::new`] and the `key` methods, added with
/// [`Sculpt::loft`].
///
/// The section frame: `t` runs along the axis, `x` toward the given lateral direction and the
/// section depth splits into `f` toward the front and `g` toward the back, so one key can be
/// flat-fronted and round-backed. The section is a superellipse (`round`: 2 = ellipse,
/// larger = boxier). The ends are flat.
#[derive(Debug, Clone, PartialEq)]
pub struct Loft {
    origin: Vec3,
    /// Columns: lateral, axis, back.
    frame: Mat3,
    keys: Vec<LoftKey>,
    bone: u8,
    round: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LoftKey {
    t: f32,
    /// x, z (back), half-width, front depth, back depth, exponent.
    s: [f32; 6],
    bone: u8,
}

impl Loft {
    /// A loft starting at `origin` along `axis`; `front` and `lateral` orient its sections
    /// (made perpendicular to the axis).
    pub fn new(origin: Vec3, axis: Vec3, front: Vec3, lateral: Vec3) -> Self {
        let y = axis.normalize_or(Vec3::Y);
        let z = (-front + y * y.dot(front)).normalize_or(y.any_orthonormal_vector());
        let x0 = lateral - y * y.dot(lateral) - z * z.dot(lateral);
        let x = if x0.dot(y.cross(z)) < 0.0 {
            -y.cross(z)
        } else {
            y.cross(z)
        };
        Self {
            origin,
            frame: Mat3::from_cols(x, y, z),
            keys: Vec::new(),
            bone: 0,
            round: 2.0,
        }
    }

    /// Bone of the segments starting at the following keys.
    pub fn bone(mut self, bone: u8) -> Self {
        self.bone = bone;
        self
    }

    /// Section exponent of the following keys (2 = ellipse).
    pub fn round(mut self, n: f32) -> Self {
        self.round = n.max(1.2);
        self
    }

    /// A centered section `t` along the axis: half-width `a`, depth `f` in front, `g` behind.
    pub fn key(self, t: f32, a: f32, f: f32, g: f32) -> Self {
        self.key_at(t, 0.0, 0.0, a, f, g)
    }

    /// A section centered on the world point `center` (bent limbs: the section stays square
    /// to the loft's axis).
    pub fn key_through(self, center: Vec3, a: f32, f: f32, g: f32) -> Self {
        let l = self.frame.transpose() * (center - self.origin);
        self.key_at(l.y, l.x, l.z, a, f, g)
    }

    /// A section whose center is offset by `x` (lateral) and `z` (backward).
    pub fn key_at(mut self, t: f32, x: f32, z: f32, a: f32, f: f32, g: f32) -> Self {
        let m = 1e-4;
        self.keys.push(LoftKey {
            t,
            s: [x, z, a.max(m), f.max(m), g.max(m), self.round],
            bone: self.bone,
        });
        self
    }
}

/// A loft prepared for evaluation: keys sorted, Hermite tangents precomputed.
#[derive(Debug, Clone, PartialEq)]
struct LoftData {
    origin: Vec3,
    inv: Mat3,
    /// Local bounding box (culling).
    lo: Vec3,
    hi: Vec3,
    t: Vec<f32>,
    s: Vec<[f32; 6]>,
    m: Vec<[f32; 6]>,
    bones: Vec<u8>,
}

impl LoftData {
    fn new(loft: Loft) -> Self {
        let mut keys = loft.keys;
        keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        keys.dedup_by(|b, a| (b.t - a.t).abs() < 1e-6);
        if keys.len() == 1 {
            let mut k = keys[0];
            k.t += 1e-3;
            keys.push(k);
        }
        let n = keys.len();
        let slope =
            |i: usize, j: usize, c: usize| (keys[j].s[c] - keys[i].s[c]) / (keys[j].t - keys[i].t);
        // Tangents: the weighted mean of the neighboring slopes (non-uniform Catmull-Rom), held
        // flat at extremes so radii don't overshoot between keys.
        let m = (0..n)
            .map(|i| {
                std::array::from_fn(|c| {
                    if n < 2 {
                        return 0.0;
                    }
                    if i == 0 || i == n - 1 {
                        return 0.0;
                    }
                    let (s0, s1) = (slope(i - 1, i, c), slope(i, i + 1, c));
                    if s0 * s1 <= 0.0 {
                        return 0.0;
                    }
                    let (h0, h1) = (keys[i].t - keys[i - 1].t, keys[i + 1].t - keys[i].t);
                    let m = (s0 * h1 + s1 * h0) / (h0 + h1);
                    // Fritsch-Carlson limit: monotone between monotone keys.
                    m.clamp(-3.0 * s0.abs().min(s1.abs()), 3.0 * s0.abs().min(s1.abs()))
                })
            })
            .collect();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for k in &keys {
            let s = k.s;
            lo = lo.min(Vec3::new(s[0] - s[2], k.t, s[1] - s[3]));
            hi = hi.max(Vec3::new(s[0] + s[2], k.t, s[1] + s[4]));
        }
        Self {
            origin: loft.origin,
            inv: loft.frame.inverse(),
            lo,
            hi,
            t: keys.iter().map(|k| k.t).collect(),
            s: keys.iter().map(|k| k.s).collect(),
            m,
            bones: keys.iter().map(|k| k.bone).collect(),
        }
    }

    /// Bounding sphere (world).
    fn bounds(&self, frame: Mat3) -> (Vec3, f32) {
        let c = (self.lo + self.hi) * 0.5;
        (
            self.origin + frame * c,
            (self.hi - self.lo).length() * 0.5 + 1e-3,
        )
    }

    /// A lower bound of the distance: the signed distance to the local bounding box.
    #[inline]
    fn box_distance(&self, p: Vec3) -> f32 {
        let q = self.inv * (p - self.origin);
        let e = (self.lo - q).max(q - self.hi);
        e.max(Vec3::ZERO).length() + e.max_element().min(0.0)
    }

    /// Segment index and the section parameters with their derivative along the axis at `t`
    /// (clamped to the ends).
    #[inline]
    fn section(&self, t: f32) -> (usize, [f32; 6], [f32; 6]) {
        let n = self.t.len();
        let mut i = 0;
        while i + 2 < n && t > self.t[i + 1] {
            i += 1;
        }
        let h = self.t[i + 1] - self.t[i];
        let u = ((t - self.t[i]) / h).clamp(0.0, 1.0);
        let (u2, u3) = (u * u, u * u * u);
        let (h00, h10, h01, h11) = (
            2.0 * u3 - 3.0 * u2 + 1.0,
            u3 - 2.0 * u2 + u,
            -2.0 * u3 + 3.0 * u2,
            u3 - u2,
        );
        let (d00, d10, d01, d11) = (
            6.0 * u2 - 6.0 * u,
            3.0 * u2 - 4.0 * u + 1.0,
            -6.0 * u2 + 6.0 * u,
            3.0 * u2 - 2.0 * u,
        );
        let (a, b, ma, mb) = (&self.s[i], &self.s[i + 1], &self.m[i], &self.m[i + 1]);
        let mut v = [0.0; 6];
        let mut dv = [0.0; 6];
        for c in 0..6 {
            v[c] = h00 * a[c] + h10 * h * ma[c] + h01 * b[c] + h11 * h * mb[c];
            dv[c] = (d00 * a[c] + d10 * h * ma[c] + d01 * b[c] + d11 * h * mb[c]) / h;
        }
        let i = if u < 1.0 || i + 2 >= n { i } else { i + 1 };
        (i, v, dv)
    }

    /// Distance within the section plane (first order: the implicit over its gradient).
    #[inline]
    fn section_distance(x: f32, z: f32, s: &[f32; 6]) -> f32 {
        let u = (x - s[0]) / s[2];
        let w = z - s[1];
        let b = if w < 0.0 { s[3] } else { s[4] };
        let v = w / b;
        let n = s[5];
        if (n - 2.0).abs() < 1e-3 {
            let k0 = (u * u + v * v).sqrt();
            let k1 = ((u / s[2]).powi(2) + (v / b).powi(2)).sqrt();
            if k1 < 1e-9 {
                return -s[2].min(b);
            }
            return k0 * (k0 - 1.0) / k1;
        }
        let (au, av) = (u.abs(), v.abs());
        let (pu, pv) = (au.powf(n), av.powf(n));
        let sum = pu + pv;
        if sum < 1e-12 {
            return -s[2].min(b);
        }
        let rho = sum.powf(1.0 / n);
        // ∂ρ/∂x = ρ^(1−n) |u|^(n−1) / a.
        let c = rho.powf(1.0 - n);
        let gx = c * pu / au.max(1e-12) / s[2];
        let gz = c * pv / av.max(1e-12) / b;
        let g = (gx * gx + gz * gz).sqrt().max(1e-9);
        (rho - 1.0) / g
    }

    fn distance(&self, p: Vec3) -> f32 {
        let q = self.inv * (p - self.origin);
        let (t0, t1) = (self.t[0], self.t[self.t.len() - 1]);
        let t = q.y.clamp(t0, t1);
        let (_, s, ds) = self.section(t);
        let d2 = Self::section_distance(q.x, q.z, &s);
        // Along a tapering section the surface leans: scale by the slope.
        let e = 1e-3;
        let sb: [f32; 6] = std::array::from_fn(|c| s[c] + ds[c] * e);
        let slope = (Self::section_distance(q.x, q.z, &sb) - d2) / e;
        let d = d2 / (1.0 + slope * slope).sqrt();
        let cap = (t0 - q.y).max(q.y - t1);
        if cap > 0.0 && d > 0.0 {
            (d * d + cap * cap).sqrt()
        } else {
            d.max(cap)
        }
    }

    fn bone(&self, p: Vec3) -> u8 {
        let q = self.inv * (p - self.origin);
        self.bones[self.section(q.y).0]
    }
}

/// A signed distance field built from tagged primitives, in order.
#[derive(Debug, Clone)]
pub struct Sculpt {
    parts: Vec<Part>,
    lofts: Vec<LoftData>,
    /// Skip parts beyond their bounds (off only to check that skipping changes nothing).
    cull: bool,
}

impl Default for Sculpt {
    fn default() -> Self {
        Self {
            parts: Vec::new(),
            lofts: Vec::new(),
            cull: true,
        }
    }
}

/// Meshing quality.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshSettings {
    /// Grid cell, meters.
    pub cell: f32,
    /// Simplify down to about this many triangles (0 = keep every surface-net triangle).
    pub target_triangles: usize,
    /// Simplification never makes edges longer than this, meters.
    pub max_edge: f32,
    /// How far a simplified edge may sag below a curved surface, meters: edges shorten where
    /// the surface curves (fingers, face), so silhouettes stay round (0 = only `max_edge`).
    pub tolerance: f32,
    /// Tangential relaxation passes after simplification.
    pub relax: usize,
    /// Ambient occlusion probe distance, meters (0 = no occlusion bake).
    pub occlusion: f32,
}

/// A meshed sculpt: counter-clockwise triangles, unit normals, the bone each vertex belongs
/// to and a baked ambient occlusion (1 = open, 0 = fully occluded).
#[derive(Debug, Clone, Default)]
pub struct SculptMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub bones: Vec<u8>,
    pub occlusion: Vec<f32>,
}

impl Sculpt {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, op: Op, bone: u8, k: f32, shape: Shape, center: Vec3, radius: f32) {
        self.parts.push(Part {
            shape,
            op,
            k: k.max(0.0),
            bone,
            center,
            radius,
            extent: Vec3::ZERO,
        });
    }

    fn ellipsoid_shape(center: Vec3, radii: Vec3, rot: Quat) -> (Shape, Vec3, f32) {
        let r = radii.max(Vec3::splat(1e-4));
        (
            Shape::Ellipsoid {
                center,
                inv_rot: rot.normalize().conjugate(),
                inv_r: r.recip(),
                inv_r2: (r * r).recip(),
                min_r: r.min_element(),
            },
            center,
            r.max_element(),
        )
    }

    fn cone_shape(a: Vec3, b: Vec3, ra: f32, rb: f32) -> (Shape, Vec3, f32) {
        let ba = b - a;
        let l2 = ba.length_squared();
        let len = l2.sqrt();
        let bound = len * 0.5 + ra.max(rb);
        // One end's ball swallows the other: just that ball.
        if len < 1e-5 || (ra - rb).abs() >= len {
            let (c, r) = if ra >= rb { (a, ra) } else { (b, rb) };
            return (
                Shape::Sphere {
                    center: c,
                    radius: r,
                },
                c,
                r,
            );
        }
        let rr = ra - rb;
        (
            Shape::RoundCone {
                a,
                ba,
                l2,
                rr,
                a2: l2 - rr * rr,
                il2: 1.0 / l2,
                r1: ra,
                r2: rb,
            },
            (a + b) * 0.5,
            bound,
        )
    }

    /// Blend in an ellipsoid (semi-axes `radii` in a frame turned by `rot`).
    pub fn ellipsoid(&mut self, bone: u8, k: f32, center: Vec3, radii: Vec3, rot: Quat) {
        let (s, c, r) = Self::ellipsoid_shape(center, radii, rot);
        self.push(Op::Add, bone, k, s, c, r);
    }

    /// Blend in a cone with rounded ends (radius `ra` at `a`, `rb` at `b`).
    pub fn cone(&mut self, bone: u8, k: f32, a: Vec3, b: Vec3, ra: f32, rb: f32) {
        let (s, c, r) = Self::cone_shape(a, b, ra, rb);
        self.push(Op::Add, bone, k, s, c, r);
    }

    /// Blend in a ball.
    pub fn sphere(&mut self, bone: u8, k: f32, center: Vec3, radius: f32) {
        self.push(
            Op::Add,
            bone,
            k,
            Shape::Sphere { center, radius },
            center,
            radius,
        );
    }

    /// Raise the surface by `height` meters (negative: sink it) at `center`, fading out
    /// smoothly toward the ellipsoid `radii` (turned by `rot`).
    pub fn bump(&mut self, center: Vec3, radii: Vec3, rot: Quat, height: f32) {
        let (s, c, r) = Self::ellipsoid_shape(center, radii, rot);
        self.push(Op::Bump(height), 0, 0.0, s, c, r);
    }

    /// [`Self::bump`] centered where the surface built so far is nearest to `near`.
    pub fn bump_surface(&mut self, near: Vec3, radii: Vec3, rot: Quat, height: f32) {
        let center = self.project(near, 12, 1e-4, 1.0);
        self.bump(center, radii, rot, height);
    }

    /// Carve an ellipsoid out.
    pub fn carve_ellipsoid(&mut self, k: f32, center: Vec3, radii: Vec3, rot: Quat) {
        let (s, c, r) = Self::ellipsoid_shape(center, radii, rot);
        self.push(Op::Carve, 0, k, s, c, r);
    }

    /// Carve a round cone out (grooves).
    pub fn carve_cone(&mut self, k: f32, a: Vec3, b: Vec3, ra: f32, rb: f32) {
        let (s, c, r) = Self::cone_shape(a, b, ra, rb);
        self.push(Op::Carve, 0, k, s, c, r);
    }

    /// Blend in a [`Loft`].
    pub fn loft(&mut self, k: f32, loft: Loft) {
        if loft.keys.is_empty() {
            return;
        }
        let frame = loft.frame;
        let bone = loft.bone;
        let data = LoftData::new(loft);
        let (c, r) = data.bounds(frame);
        let index = self.lofts.len() as u32;
        self.lofts.push(data);
        self.push(Op::Add, bone, k, Shape::Loft(index), c, r);
    }

    /// Parts added by `build` blend among themselves with their own radii, then into the rest
    /// of the sculpt with `k` (and carves inside only cut the group).
    pub fn group(&mut self, k: f32, build: impl FnOnce(&mut Sculpt)) {
        let at = self.parts.len();
        self.push(
            Op::Group(0),
            0,
            k,
            Shape::Sphere {
                center: Vec3::ZERO,
                radius: 0.0,
            },
            Vec3::ZERO,
            0.0,
        );
        build(self);
        let len = self.parts.len() - at - 1;
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for p in self.parts[at + 1..]
            .iter()
            .filter(|p| matches!(p.op, Op::Add))
        {
            lo = lo.min(p.center - Vec3::splat(p.radius + p.k));
            hi = hi.max(p.center + Vec3::splat(p.radius + p.k));
        }
        let group = &mut self.parts[at];
        group.op = Op::Group(len as u32);
        if lo.x.is_finite() {
            group.center = (lo + hi) * 0.5;
            group.extent = (hi - lo) * 0.5;
            group.radius = group.extent.length();
        } else {
            group.extent = Vec3::splat(-1e9);
        }
    }

    /// Keep only what lies below the plane `normal · p = offset` (blended by `k`).
    pub fn clip(&mut self, k: f32, normal: Vec3, offset: f32) {
        let n = normal.normalize_or(Vec3::Y);
        self.push(
            Op::Clip,
            0,
            k,
            Shape::HalfSpace { normal: n, offset },
            Vec3::ZERO,
            f32::INFINITY,
        );
    }

    /// Signed distance at `p` (negative inside), meters.
    #[inline]
    pub fn distance(&self, p: Vec3) -> f32 {
        self.evaluate(p).0
    }

    /// Signed distance and the bone of the nearest added part.
    pub fn evaluate(&self, p: Vec3) -> (f32, u8) {
        let mut nearest = f32::INFINITY;
        let mut bone = 0;
        let d = self.eval_range(0, self.parts.len(), p, &mut nearest, &mut bone);
        (d, bone)
    }

    fn eval_range(
        &self,
        start: usize,
        end: usize,
        p: Vec3,
        nearest: &mut f32,
        bone: &mut u8,
    ) -> f32 {
        let mut d = 1e9f32;
        let mut i = start;
        while i < end {
            let part = &self.parts[i];
            i += 1;
            match part.op {
                Op::Add => {
                    // The bound is part of the field (the shape estimates can dip below it),
                    // so skipping a part beyond it never changes the result.
                    let mut lb = (p - part.center).length() - part.radius;
                    if let Shape::Loft(l) = part.shape {
                        lb = lb.max(self.lofts[l as usize].box_distance(p));
                    }
                    if self.cull && lb >= d + part.k && lb >= *nearest {
                        continue;
                    }
                    let di = part.shape.distance(p, &self.lofts).max(lb);
                    if di < *nearest {
                        *nearest = di;
                        *bone = match part.shape {
                            Shape::Loft(l) => self.lofts[l as usize].bone(p),
                            _ => part.bone,
                        };
                    }
                    d = smin(d, di, part.k);
                }
                Op::Group(len) => {
                    let group_end = i + len as usize;
                    // Distance to the group's box (center, half extents), also a floor of
                    // the group's value.
                    let e = (p - part.center).abs() - part.extent;
                    let lb = e.max(Vec3::ZERO).length() + e.max_element().min(0.0);
                    if !(self.cull && lb >= d + part.k && lb >= *nearest) {
                        let g = self.eval_range(i, group_end, p, nearest, bone).max(lb);
                        d = smin(d, g, part.k);
                    }
                    i = group_end;
                }
                Op::Bump(height) => {
                    if (p - part.center).length_squared() >= part.radius * part.radius {
                        continue;
                    }
                    if let Shape::Ellipsoid {
                        center,
                        inv_rot,
                        inv_r,
                        ..
                    } = part.shape
                    {
                        let r2 = (inv_rot * (p - center) * inv_r).length_squared();
                        if r2 < 1.0 {
                            let f = 1.0 - r2;
                            d -= height * f * f * f;
                        }
                    }
                }
                Op::Carve => {
                    let lb = (p - part.center).length() - part.radius;
                    if self.cull && lb >= part.k - d {
                        continue;
                    }
                    d = smax(d, -part.shape.distance(p, &self.lofts).max(lb), part.k);
                }
                Op::Clip => d = smax(d, part.shape.distance(p, &self.lofts), part.k),
            }
        }
        d
    }

    /// Field gradient (not normalized) from four samples.
    #[inline]
    pub fn gradient(&self, p: Vec3, e: f32) -> Vec3 {
        const K: [Vec3; 4] = [
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0),
        ];
        K.iter()
            .map(|k| *k * self.distance(p + *k * e))
            .sum::<Vec3>()
            / (4.0 * e)
    }

    /// Move `p` onto the surface (Newton steps along the gradient), at most `max_move` away.
    pub fn project(&self, p: Vec3, steps: usize, e: f32, max_move: f32) -> Vec3 {
        let start = p;
        let mut p = p;
        for _ in 0..steps {
            let d = self.distance(p);
            if d.abs() < 1e-6 {
                break;
            }
            let g = self.gradient(p, e);
            let g2 = g.length_squared();
            if g2 < 1e-10 {
                break;
            }
            p -= g * (d / g2);
        }
        let moved = p - start;
        if moved.length() > max_move {
            start + moved.normalize() * max_move
        } else {
            p
        }
    }

    /// The same field evaluating every part (checks that culling never changes it).
    #[cfg(test)]
    pub fn without_culling(&self) -> Sculpt {
        Sculpt {
            cull: false,
            ..self.clone()
        }
    }

    /// Bounds of everything added, meters.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for p in self.parts.iter().filter(|p| p.op == Op::Add) {
            lo = lo.min(p.center - Vec3::splat(p.radius + p.k));
            hi = hi.max(p.center + Vec3::splat(p.radius + p.k));
        }
        (lo, hi)
    }

    /// Mesh the surface (see the module docs).
    pub fn mesh(&self, settings: &MeshSettings) -> SculptMesh {
        let h = settings.cell.max(1e-4);
        let (mut positions, mut triangles) = surface_nets(self, h);
        // Project the surface-net vertices onto the exact surface.
        par_for_each_mut(&mut positions, |p| *p = self.project(*p, 3, h * 0.25, h));
        if settings.target_triangles > 0 && triangles.len() > settings.target_triangles {
            let max_edge = settings.max_edge.max(h * 2.0);
            let mut limit = vec![max_edge; positions.len()];
            if settings.tolerance > 0.0 {
                // Curvature from how fast the normal turns between neighbors; a chord of length
                // L on a circle of radius R sags L² / 8R.
                let adjacency = vertex_neighbors(positions.len(), &triangles);
                let mut normals = vec![Vec3::ZERO; positions.len()];
                par_zip(&positions, &mut normals, |_, p, n| {
                    *n = self.gradient(*p, h * 0.25).normalize_or(Vec3::Y);
                });
                let mut curvature = vec![0.0f32; positions.len()];
                par_zip(&adjacency, &mut curvature, |i, ring, c| {
                    *c = ring
                        .iter()
                        .map(|&j| {
                            let j = j as usize;
                            normals[i].distance(normals[j])
                                / positions[i].distance(positions[j]).max(1e-6)
                        })
                        .fold(0.0, f32::max);
                });
                // Spread over two rings so the dense zone covers the whole curved feature.
                for _ in 0..2 {
                    let before = curvature.clone();
                    par_zip(&adjacency, &mut curvature, |i, ring, c| {
                        *c = ring
                            .iter()
                            .map(|&j| before[j as usize])
                            .fold(before[i], f32::max);
                    });
                }
                for (l, c) in limit.iter_mut().zip(&curvature) {
                    *l = (8.0 * settings.tolerance / c.max(1e-6))
                        .sqrt()
                        .clamp(h * 1.2, max_edge);
                }
            }
            decimate(
                &mut positions,
                &mut triangles,
                settings.target_triangles,
                &mut limit,
            );
        }
        let adjacency = vertex_neighbors(positions.len(), &triangles);
        for _ in 0..settings.relax {
            relax(self, &mut positions, &adjacency, h);
        }
        let mut normals = vec![Vec3::ZERO; positions.len()];
        let face = face_normals(&positions, &triangles);
        par_zip(&positions, &mut normals, |i, p, n| {
            *n = self
                .gradient(*p, h * 0.25)
                .try_normalize()
                .unwrap_or(face[i]);
        });
        let mut bones = vec![0u8; positions.len()];
        par_zip(&positions, &mut bones, |i, p, b| {
            *b = self.evaluate(*p - normals[i] * (h * 0.5)).1;
        });
        let mut occlusion = vec![1.0f32; positions.len()];
        if settings.occlusion > 0.0 {
            let reach = settings.occlusion;
            par_zip(&positions, &mut occlusion, |i, p, o| {
                *o = self.occlusion(*p, normals[i], reach);
            });
        }
        SculptMesh {
            positions,
            normals,
            triangles,
            bones,
            occlusion,
        }
    }

    /// Ambient occlusion from the field: how much the surface around the normal is closed in,
    /// probed along the normal and four directions around it.
    fn occlusion(&self, p: Vec3, n: Vec3, reach: f32) -> f32 {
        let t = n.any_orthonormal_vector();
        let b = n.cross(t);
        let dirs = [
            n,
            (n + t * 0.8).normalize(),
            (n - t * 0.8).normalize(),
            (n + b * 0.8).normalize(),
            (n - b * 0.8).normalize(),
        ];
        let mut occ = 0.0;
        for (k, dir) in dirs.iter().enumerate() {
            let weight = if k == 0 { 2.0 } else { 1.0 };
            // Over an open flat surface the distance along `dir` grows as s · cos θ.
            let open = dir.dot(n);
            let mut o = 0.0;
            for i in 1..=5 {
                let s = reach * i as f32 / 5.0;
                let d = self.distance(p + *dir * s);
                o += ((s * open - d) / s).max(0.0) / i as f32;
            }
            occ += weight * o;
        }
        (1.0 - occ / 6.0 * 0.9).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------------------
// Parallel helpers (plain scoped threads: meshing runs once, possibly before task pools).

fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4)
}

fn par_for_each_mut<T: Send>(data: &mut [T], f: impl Fn(&mut T) + Sync) {
    let chunk = data.len().div_ceil(threads()).max(256);
    std::thread::scope(|s| {
        for c in data.chunks_mut(chunk) {
            let f = &f;
            s.spawn(move || c.iter_mut().for_each(f));
        }
    });
}

fn par_zip<A: Sync, B: Send>(a: &[A], b: &mut [B], f: impl Fn(usize, &A, &mut B) + Sync) {
    let chunk = b.len().div_ceil(threads()).max(256);
    std::thread::scope(|s| {
        for (k, c) in b.chunks_mut(chunk).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (j, out) in c.iter_mut().enumerate() {
                    let i = k * chunk + j;
                    f(i, &a[i], out);
                }
            });
        }
    });
}

fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let chunk = n.div_ceil(threads()).max(16);
    let mut parts: Vec<Vec<T>> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|start| {
                let f = &f;
                s.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        parts = handles.into_iter().map(|h| h.join().unwrap()).collect();
    });
    parts.into_iter().flatten().collect()
}

// ---------------------------------------------------------------------------------------
// Surface nets on a sparse block grid

/// Cells per block side.
const B: usize = 8;
const S: usize = B + 1;

fn surface_nets(sculpt: &Sculpt, h: f32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (lo, hi) = sculpt.bounds();
    let origin = lo - Vec3::splat(2.0 * h);
    let size = hi - lo + Vec3::splat(4.0 * h);
    let cells = (size / h).ceil().as_uvec3();
    let nb = UVec3::new(
        cells.x.div_ceil(B as u32),
        cells.y.div_ceil(B as u32),
        cells.z.div_ceil(B as u32),
    );
    let block_count = (nb.x * nb.y * nb.z) as usize;
    let block_xyz = |id: usize| {
        let id = id as u32;
        UVec3::new(id % nb.x, (id / nb.x) % nb.y, id / (nb.x * nb.y))
    };
    let point = |g: UVec3| origin + g.as_vec3() * h;
    // Blocks near the surface (generous: blends and ellipsoids aren't exact distances).
    let half_diag = B as f32 * h * 0.866;
    let near: Vec<bool> = par_map(block_count, |id| {
        let b = block_xyz(id);
        let center = point(b * B as u32) + Vec3::splat(B as f32 * h * 0.5);
        sculpt.distance(center).abs() < half_diag * 1.6 + 2.0 * h
    });
    let active: Vec<usize> = (0..block_count).filter(|&i| near[i]).collect();
    let mut slot = vec![u32::MAX; block_count];
    for (k, &id) in active.iter().enumerate() {
        slot[id] = k as u32;
    }
    // Samples per active block, (B+1)³ including the far faces; only 2³-cell sub-blocks near
    // the surface are sampled (the rest stay NaN: no surface there).
    const SUB: usize = 2;
    let reach = SUB as f32 * h * 0.866 * 2.5 + h;
    let samples: Vec<f32> = par_map(active.len(), |k| {
        let base = block_xyz(active[k]) * B as u32;
        let mut out = vec![f32::NAN; S * S * S];
        for sz in 0..B / SUB {
            for sy in 0..B / SUB {
                for sx in 0..B / SUB {
                    let corner = UVec3::new(sx as u32, sy as u32, sz as u32) * SUB as u32;
                    let center = point(base + corner) + Vec3::splat(SUB as f32 * h * 0.5);
                    if sculpt.distance(center).abs() > reach {
                        continue;
                    }
                    for z in corner.z..=corner.z + SUB as u32 {
                        for y in corner.y..=corner.y + SUB as u32 {
                            for x in corner.x..=corner.x + SUB as u32 {
                                let i = x as usize + S * (y as usize + S * z as usize);
                                if out[i].is_nan() {
                                    out[i] = sculpt.distance(point(base + UVec3::new(x, y, z)));
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    })
    .into_iter()
    .flatten()
    .collect();
    let sample =
        |k: usize, x: usize, y: usize, z: usize| samples[k * S * S * S + x + S * (y + S * z)];

    // One vertex per cell the surface crosses: the mean of its edge crossings.
    let mut cell_vertex = vec![u32::MAX; active.len() * B * B * B];
    let mut positions = Vec::new();
    const CORNERS: [[usize; 3]; 8] = [
        [0, 0, 0],
        [1, 0, 0],
        [0, 1, 0],
        [1, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [0, 1, 1],
        [1, 1, 1],
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for (k, &id) in active.iter().enumerate() {
        let base = block_xyz(id) * B as u32;
        for z in 0..B {
            for y in 0..B {
                for x in 0..B {
                    let v: [f32; 8] = std::array::from_fn(|c| {
                        let [dx, dy, dz] = CORNERS[c];
                        sample(k, x + dx, y + dy, z + dz)
                    });
                    let inside = v.iter().filter(|d| **d < 0.0).count();
                    if inside == 0 || inside == 8 || v.iter().any(|d| d.is_nan()) {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    let mut n = 0.0;
                    for (a, b) in EDGES {
                        if (v[a] < 0.0) != (v[b] < 0.0) {
                            let t = v[a] / (v[a] - v[b]);
                            let pa = Vec3::from(CORNERS[a].map(|c| c as f32));
                            let pb = Vec3::from(CORNERS[b].map(|c| c as f32));
                            sum += pa + (pb - pa) * t;
                            n += 1.0;
                        }
                    }
                    let local = sum / n + Vec3::new(x as f32, y as f32, z as f32);
                    cell_vertex[k * B * B * B + x + B * (y + B * z)] = positions.len() as u32;
                    positions.push(point(base) + local * h);
                }
            }
        }
    }
    let lookup = |g: IVec3| -> Option<u32> {
        if g.min_element() < 0 {
            return None;
        }
        let g = g.as_uvec3();
        let b = g / B as u32;
        if b.x >= nb.x || b.y >= nb.y || b.z >= nb.z {
            return None;
        }
        let s = slot[(b.x + nb.x * (b.y + nb.y * b.z)) as usize];
        if s == u32::MAX {
            return None;
        }
        let l = (g % B as u32).as_ivec3();
        let v = cell_vertex
            [s as usize * B * B * B + (l.x + B as i32 * (l.y + B as i32 * l.z)) as usize];
        (v != u32::MAX).then_some(v)
    };
    // A quad around every grid edge the surface crosses, facing out (toward positive
    // distance).
    let mut triangles = Vec::new();
    for (k, &id) in active.iter().enumerate() {
        let base = (block_xyz(id) * B as u32).as_ivec3();
        for z in 0..B {
            for y in 0..B {
                for x in 0..B {
                    let s0 = sample(k, x, y, z);
                    for axis in 0..3 {
                        let mut e = [x, y, z];
                        e[axis] += 1;
                        let s1 = sample(k, e[0], e[1], e[2]);
                        if (s0 < 0.0) == (s1 < 0.0) || s0.is_nan() || s1.is_nan() {
                            continue;
                        }
                        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
                        let g = base + IVec3::new(x as i32, y as i32, z as i32);
                        let cell = |db: i32, dc: i32| {
                            let mut q = g;
                            q[b] -= db;
                            q[c] -= dc;
                            lookup(q)
                        };
                        let (Some(v0), Some(v1), Some(v2), Some(v3)) =
                            (cell(1, 1), cell(0, 1), cell(0, 0), cell(1, 0))
                        else {
                            continue;
                        };
                        let mut quad = [v0, v1, v2, v3];
                        if s0 >= 0.0 {
                            quad.reverse();
                        }
                        let p = |i: u32| positions[i as usize];
                        let [a, b, c, d] = quad;
                        if p(a).distance_squared(p(c)) < p(b).distance_squared(p(d)) {
                            triangles.push([a, b, c]);
                            triangles.push([a, c, d]);
                        } else {
                            triangles.push([a, b, d]);
                            triangles.push([b, c, d]);
                        }
                    }
                }
            }
        }
    }
    (positions, triangles)
}

// ---------------------------------------------------------------------------------------
// Quadric error simplification (Garland & Heckbert 1997)

type Quadric = [f64; 10];

fn plane_quadric(n: [f64; 3], d: f64, w: f64) -> Quadric {
    let [a, b, c] = n;
    [
        w * a * a,
        w * a * b,
        w * a * c,
        w * a * d,
        w * b * b,
        w * b * c,
        w * b * d,
        w * c * c,
        w * c * d,
        w * d * d,
    ]
}

fn quadric_error(q: &Quadric, p: [f64; 3]) -> f64 {
    let [x, y, z] = p;
    q[0] * x * x
        + 2.0 * q[1] * x * y
        + 2.0 * q[2] * x * z
        + 2.0 * q[3] * x
        + q[4] * y * y
        + 2.0 * q[5] * y * z
        + 2.0 * q[6] * y
        + q[7] * z * z
        + 2.0 * q[8] * z
        + q[9]
}

fn dvec(p: Vec3) -> [f64; 3] {
    [p.x as f64, p.y as f64, p.z as f64]
}

/// Best position for merging an edge and its error.
fn collapse_target(q: &Quadric, a: Vec3, b: Vec3) -> (f64, Vec3) {
    let m = [[q[0], q[1], q[2]], [q[1], q[4], q[5]], [q[2], q[5], q[7]]];
    let rhs = [-q[3], -q[6], -q[8]];
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let scale = (q[0] + q[4] + q[7]).max(1e-30);
    let mid = (a + b) * 0.5;
    if det.abs() > 1e-9 * scale * scale * scale {
        let solve = |col: usize| {
            let mut mm = m;
            for (r, row) in mm.iter_mut().enumerate() {
                row[col] = rhs[r];
            }
            (mm[0][0] * (mm[1][1] * mm[2][2] - mm[1][2] * mm[2][1])
                - mm[0][1] * (mm[1][0] * mm[2][2] - mm[1][2] * mm[2][0])
                + mm[0][2] * (mm[1][0] * mm[2][1] - mm[1][1] * mm[2][0]))
                / det
        };
        let x = Vec3::new(solve(0) as f32, solve(1) as f32, solve(2) as f32);
        if x.is_finite() && x.distance(mid) <= a.distance(b) * 0.75 {
            return (quadric_error(q, dvec(x)).max(0.0), x);
        }
    }
    [a, b, mid]
        .into_iter()
        .map(|p| (quadric_error(q, dvec(p)).max(0.0), p))
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .unwrap()
}

fn tri_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    (b - a).cross(c - a)
}

fn tri_quality(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let area2 = tri_normal(a, b, c).length();
    let l = a.distance_squared(b) + b.distance_squared(c) + c.distance_squared(a);
    // 1 for an equilateral triangle.
    if l < 1e-20 {
        0.0
    } else {
        3.0_f32.sqrt() * 2.0 * area2 / l
    }
}

/// Collapse edges in order of quadric error until about `target` triangles remain.
/// `limit` is the longest edge allowed at each vertex.
fn decimate(
    positions: &mut Vec<Vec3>,
    triangles: &mut Vec<[u32; 3]>,
    target: usize,
    limit: &mut [f32],
) {
    let n = positions.len();
    let mut quadrics = vec![[0.0f64; 10]; n];
    for t in triangles.iter() {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let nrm = tri_normal(a, b, c);
        let len = nrm.length();
        if len < 1e-14 {
            continue;
        }
        let u = nrm / len;
        let nd = [u.x as f64, u.y as f64, u.z as f64];
        let d = -(u.dot(a) as f64);
        let q = plane_quadric(nd, d, len as f64 * 0.5);
        for &i in t {
            for (x, y) in quadrics[i as usize].iter_mut().zip(q) {
                *x += y;
            }
        }
    }
    let mut vertex_tris: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (k, t) in triangles.iter().enumerate() {
        for &i in t {
            vertex_tris[i as usize].push(k as u32);
        }
    }
    let mut tri_alive = vec![true; triangles.len()];
    let mut vert_alive = vec![true; n];
    let mut stamp = vec![0u32; n];
    let mut alive = triangles.len();
    // Cheapest collapse first: (cost bits, u, v, stamps of u and v when queued; a stale
    // stamp means the edge changed since).
    type Collapse = Reverse<(u64, u32, u32, u32, u32)>;
    let mut heap: BinaryHeap<Collapse> = BinaryHeap::new();
    let push_edge = |heap: &mut BinaryHeap<_>,
                     quadrics: &[Quadric],
                     positions: &[Vec3],
                     stamp: &[u32],
                     u: u32,
                     v: u32| {
        let mut q = quadrics[u as usize];
        for (x, y) in q.iter_mut().zip(quadrics[v as usize]) {
            *x += y;
        }
        let (cost, _) = collapse_target(&q, positions[u as usize], positions[v as usize]);
        heap.push(Reverse((
            cost.to_bits(),
            u,
            v,
            stamp[u as usize],
            stamp[v as usize],
        )));
    };
    let mut edges: Vec<(u32, u32)> = triangles
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| (a.min(b), a.max(b)))
        .collect();
    edges.sort_unstable();
    edges.dedup();
    for (u, v) in edges {
        push_edge(&mut heap, &quadrics, positions, &stamp, u, v);
    }
    let mut ring_u = Vec::new();
    let mut ring_v = Vec::new();
    let neighbors = |vertex_tris: &[Vec<u32>],
                     tri_alive: &[bool],
                     triangles: &[[u32; 3]],
                     v: u32,
                     out: &mut Vec<u32>| {
        out.clear();
        for &t in &vertex_tris[v as usize] {
            if tri_alive[t as usize] {
                out.extend(triangles[t as usize].iter().filter(|&&w| w != v));
            }
        }
        out.sort_unstable();
        out.dedup();
    };
    while alive > target {
        let Some(Reverse((_, u, v, su, sv))) = heap.pop() else {
            break;
        };
        let (ui, vi) = (u as usize, v as usize);
        if !vert_alive[ui] || !vert_alive[vi] || stamp[ui] != su || stamp[vi] != sv {
            continue;
        }
        let mut q = quadrics[ui];
        for (x, y) in q.iter_mut().zip(quadrics[vi]) {
            *x += y;
        }
        let (_, target_pos) = collapse_target(&q, positions[ui], positions[vi]);
        neighbors(&vertex_tris, &tri_alive, triangles, u, &mut ring_u);
        neighbors(&vertex_tris, &tri_alive, triangles, v, &mut ring_v);
        if ring_u.binary_search(&v).is_err() {
            continue;
        }
        // Link condition: exactly the two vertices opposite the edge are shared.
        let shared = ring_u
            .iter()
            .filter(|w| ring_v.binary_search(w).is_ok())
            .count();
        if shared != 2 {
            continue;
        }
        let max_edge = limit[ui].min(limit[vi]);
        let too_long = ring_u.iter().chain(&ring_v).any(|&w| {
            w != u
                && w != v
                && positions[w as usize].distance(target_pos) > max_edge.min(limit[w as usize])
        });
        if too_long {
            continue;
        }
        // No flipped or sliver triangles around the merged vertex.
        let mut ok = true;
        'check: for (keep, gone) in [(u, v), (v, u)] {
            for &t in &vertex_tris[keep as usize] {
                let t = t as usize;
                if !tri_alive[t] || triangles[t].contains(&gone) {
                    continue;
                }
                let p = triangles[t].map(|i| positions[i as usize]);
                let moved = triangles[t].map(|i| {
                    if i == keep {
                        target_pos
                    } else {
                        positions[i as usize]
                    }
                });
                let before = tri_normal(p[0], p[1], p[2]);
                let after = tri_normal(moved[0], moved[1], moved[2]);
                let q_after = tri_quality(moved[0], moved[1], moved[2]);
                let q_before = tri_quality(p[0], p[1], p[2]);
                if before.dot(after) < 0.3 * before.length() * after.length()
                    || q_after < 0.25_f32.min(q_before * 0.9)
                {
                    ok = false;
                    break 'check;
                }
            }
        }
        if !ok {
            continue;
        }
        // Merge v into u.
        let v_tris = std::mem::take(&mut vertex_tris[vi]);
        for t in v_tris {
            let ti = t as usize;
            if !tri_alive[ti] {
                continue;
            }
            if triangles[ti].contains(&u) {
                tri_alive[ti] = false;
                alive -= 1;
            } else {
                for i in triangles[ti].iter_mut() {
                    if *i == v {
                        *i = u;
                    }
                }
                vertex_tris[ui].push(t);
            }
        }
        vertex_tris[ui].retain(|&t| tri_alive[t as usize]);
        vert_alive[vi] = false;
        positions[ui] = target_pos;
        limit[ui] = max_edge;
        quadrics[ui] = q;
        stamp[ui] += 1;
        neighbors(&vertex_tris, &tri_alive, triangles, u, &mut ring_u);
        for &w in &ring_u {
            push_edge(&mut heap, &quadrics, positions, &stamp, u.min(w), u.max(w));
        }
    }
    // Compact.
    let mut remap = vec![u32::MAX; n];
    let mut new_positions = Vec::with_capacity(n / 2);
    let mut new_triangles = Vec::with_capacity(alive);
    for (k, t) in triangles.iter().enumerate() {
        if !tri_alive[k] {
            continue;
        }
        let mapped = t.map(|i| {
            let r = &mut remap[i as usize];
            if *r == u32::MAX {
                *r = new_positions.len() as u32;
                new_positions.push(positions[i as usize]);
            }
            *r
        });
        new_triangles.push(mapped);
    }
    *positions = new_positions;
    *triangles = new_triangles;
}

/// Neighbors of every vertex (sorted, unique).
pub fn vertex_neighbors(n: usize, triangles: &[[u32; 3]]) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); n];
    for t in triangles {
        for k in 0..3 {
            out[t[k] as usize].push(t[(k + 1) % 3]);
            out[t[k] as usize].push(t[(k + 2) % 3]);
        }
    }
    for list in &mut out {
        list.sort_unstable();
        list.dedup();
    }
    out
}

fn face_normals(positions: &[Vec3], triangles: &[[u32; 3]]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in triangles {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let n = tri_normal(a, b, c);
        for &i in t {
            normals[i as usize] += n;
        }
    }
    normals.iter().map(|n| n.normalize_or(Vec3::Y)).collect()
}

/// One pass of tangential relaxation (vertices slide toward the middle of their neighbors
/// within the tangent plane), then back onto the surface.
fn relax(sculpt: &Sculpt, positions: &mut [Vec3], adjacency: &[Vec<u32>], h: f32) {
    let before = positions.to_vec();
    par_zip(adjacency, positions, |i, ring, p| {
        if ring.is_empty() {
            return;
        }
        let here = before[i];
        let center = ring.iter().map(|&j| before[j as usize]).sum::<Vec3>() / ring.len() as f32;
        let n = sculpt.gradient(here, h * 0.25).normalize_or(Vec3::Y);
        let delta = center - here;
        let moved = here + (delta - n * n.dot(delta)) * 0.5;
        *p = sculpt.project(moved, 2, h * 0.25, h * 2.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(mesh: &SculptMesh) {
        assert!(!mesh.triangles.is_empty());
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            assert!(p.is_finite() && n.is_finite());
            assert!((n.length() - 1.0).abs() < 1e-3, "normal {n}");
        }
        // Counter-clockwise: geometric normals agree with the vertex normals.
        let wrong = mesh
            .triangles
            .iter()
            .filter(|t| {
                let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
                let n: Vec3 = t.iter().map(|&i| mesh.normals[i as usize]).sum();
                tri_normal(a, b, c).dot(n) < 0.0
            })
            .count();
        assert!(
            wrong * 200 < mesh.triangles.len(),
            "{wrong} triangles face inward"
        );
    }

    #[test]
    fn a_ball_meshes_onto_its_surface_and_simplifies() {
        let mut s = Sculpt::new();
        s.sphere(0, 0.0, Vec3::new(0.1, 0.2, 0.0), 0.3);
        let dense = s.mesh(&MeshSettings {
            cell: 0.02,
            target_triangles: 0,
            max_edge: 1.0,
            tolerance: 0.0,
            relax: 0,
            occlusion: 0.0,
        });
        check(&dense);
        for p in &dense.positions {
            let r = p.distance(Vec3::new(0.1, 0.2, 0.0));
            assert!((r - 0.3).abs() < 2e-3, "vertex {r} m from the center");
        }
        let target = dense.triangles.len() / 4;
        let coarse = s.mesh(&MeshSettings {
            cell: 0.02,
            target_triangles: target,
            max_edge: 0.2,
            tolerance: 0.003,
            relax: 2,
            occlusion: 0.05,
        });
        check(&coarse);
        assert!(
            coarse.triangles.len() <= target + 2,
            "{}",
            coarse.triangles.len()
        );
        // A closed surface: every edge has exactly two triangles.
        let mut edges: Vec<(u32, u32)> = coarse
            .triangles
            .iter()
            .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        edges.sort_unstable();
        let mut k = 0;
        while k < edges.len() {
            let mut j = k;
            while j < edges.len() && edges[j] == edges[k] {
                j += 1;
            }
            assert_eq!(j - k, 2, "edge {:?} has {} triangles", edges[k], j - k);
            k = j;
        }
        // Euler characteristic of a sphere.
        let v = coarse.positions.len() as i64;
        let f = coarse.triangles.len() as i64;
        let e = edges.len() as i64 / 2;
        assert_eq!(v - e + f, 2);
        // An open ball sees all of its sky.
        assert!(coarse.occlusion.iter().all(|o| *o > 0.95));
    }

    #[test]
    fn lofts_follow_their_sections_and_groups_blend_inside_first() {
        let mut s = Sculpt::new();
        // Flat in front, deep behind, boxy in the middle, tapering to the top.
        s.loft(
            0.0,
            Loft::new(Vec3::ZERO, Vec3::Y, Vec3::NEG_Z, Vec3::X)
                .bone(3)
                .key(0.0, 0.1, 0.05, 0.15)
                .key(0.5, 0.1, 0.05, 0.15)
                .bone(4)
                .round(4.0)
                .key(1.0, 0.1, 0.1, 0.1)
                .round(2.0)
                .key(1.5, 0.05, 0.05, 0.05),
        );
        let d = |x: f32, y: f32, z: f32| s.distance(Vec3::new(x, y, z));
        assert!(d(0.0, 0.25, -0.05).abs() < 1e-3 && d(0.0, 0.25, 0.15).abs() < 1e-3);
        assert!((d(0.2, 0.25, 0.0) - 0.1).abs() < 1e-3);
        assert!((d(0.0, 0.25, -0.15) - 0.1).abs() < 0.02);
        // Flat ends.
        assert!((d(0.0, -0.1, 0.0) - 0.1).abs() < 1e-3 && (d(0.0, 1.6, 0.0) - 0.1).abs() < 1e-3);
        // The boxy section holds its corners where an ellipse wouldn't.
        assert!(d(0.08, 1.0, 0.08) < 0.0 && d(0.08, 0.25, 0.02) > -0.02);
        assert!(d(0.0, 1.25, 0.0) < -0.05 && d(0.12, 1.25, 0.0) > 0.0);
        assert_eq!(s.evaluate(Vec3::new(0.11, 0.2, 0.0)).1, 3);
        assert_eq!(s.evaluate(Vec3::new(0.1, 1.2, 0.0)).1, 4);
        let mesh = s.mesh(&MeshSettings {
            cell: 0.01,
            target_triangles: 4000,
            max_edge: 0.05,
            tolerance: 0.0001,
            relax: 1,
            occlusion: 0.0,
        });
        check(&mesh);

        // Two touching balls in a sharp group, blended softly into a third.
        let mut g = Sculpt::new();
        g.sphere(0, 0.0, Vec3::new(0.0, -0.3, 0.0), 0.2);
        g.group(0.1, |g| {
            g.sphere(1, 0.0, Vec3::new(-0.1, 0.0, 0.0), 0.1);
            g.sphere(2, 0.0, Vec3::new(0.1, 0.0, 0.0), 0.1);
            g.carve_ellipsoid(
                0.0,
                Vec3::new(0.1, 0.1, 0.0),
                Vec3::splat(0.05),
                Quat::IDENTITY,
            );
        });
        g.sphere(5, 0.0, Vec3::new(0.0, 1.0, 0.0), 0.1);
        // Sharp between the grouped balls (the crease stays at the surface)...
        assert!(g.distance(Vec3::new(0.0, 0.0, 0.0)).abs() < 1e-4);
        // ...the carve only cuts the group, the outer blend fills toward the base ball.
        assert!(g.distance(Vec3::new(0.1, 0.09, 0.0)) > 0.0);
        assert!(g.distance(Vec3::new(0.0, -0.1, 0.0)) < -0.002);
        assert_eq!(g.evaluate(Vec3::new(0.12, 0.0, 0.0)).1, 2);
        assert_eq!(g.evaluate(Vec3::new(0.0, 1.05, 0.0)).1, 5);

        // A bump raises the surface by its height at its center and nothing at its rim.
        let mut b = Sculpt::new();
        b.sphere(0, 0.0, Vec3::ZERO, 0.2);
        b.bump(
            Vec3::new(0.0, 0.2, 0.0),
            Vec3::splat(0.05),
            Quat::IDENTITY,
            0.01,
        );
        b.bump(
            Vec3::new(0.2, 0.0, 0.0),
            Vec3::splat(0.05),
            Quat::IDENTITY,
            -0.01,
        );
        assert!((b.distance(Vec3::new(0.0, 0.2, 0.0)) + 0.01).abs() < 1e-5);
        assert!((b.distance(Vec3::new(0.2, 0.0, 0.0)) - 0.01).abs() < 1e-5);
        assert!(b.distance(Vec3::new(0.0, 0.0, 0.2)).abs() < 1e-6);
    }

    #[test]
    fn blends_carves_and_clips_combine() {
        let mut s = Sculpt::new();
        s.cone(1, 0.0, Vec3::ZERO, Vec3::Y, 0.2, 0.1);
        s.ellipsoid(
            2,
            0.05,
            Vec3::new(0.25, 0.5, 0.0),
            Vec3::new(0.15, 0.05, 0.05),
            Quat::IDENTITY,
        );
        s.carve_ellipsoid(
            0.01,
            Vec3::new(0.0, 0.5, -0.17),
            Vec3::splat(0.05),
            Quat::IDENTITY,
        );
        s.clip(0.01, Vec3::NEG_Y, 0.0);
        // Round cone: exact distance along its side.
        assert!(s.distance(Vec3::new(0.0, 0.0, 0.5)).abs() < 0.32);
        // Clipped below y = 0.
        assert!(s.distance(Vec3::new(0.0, -0.1, 0.0)) > 0.05);
        // The ellipsoid belongs to bone 2, the cone to bone 1.
        assert_eq!(s.evaluate(Vec3::new(0.38, 0.5, 0.0)).1, 2);
        assert_eq!(s.evaluate(Vec3::new(0.0, 0.2, 0.25)).1, 1);
        let mesh = s.mesh(&MeshSettings {
            cell: 0.015,
            target_triangles: 3000,
            max_edge: 0.08,
            tolerance: 0.0001,
            relax: 1,
            occlusion: 0.04,
        });
        check(&mesh);
        // The crease where the ellipsoid meets the cone is darker than the open top.
        let crease = mesh
            .positions
            .iter()
            .zip(&mesh.occlusion)
            .filter(|(p, _)| p.distance(Vec3::new(0.14, 0.45, 0.0)) < 0.05)
            .map(|(_, o)| *o)
            .fold(1.0f32, f32::min);
        assert!(crease < 0.9, "crease occlusion {crease}");
    }
}
