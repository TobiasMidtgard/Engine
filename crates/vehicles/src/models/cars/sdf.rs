//! Signed-distance helpers for sculpting car bodies: profile curves, rounded booleans and a
//! few primitives. Distances are meters; "approximate" fields only need the right zero set
//! and a gradient of about one, which is what the mesher and the ray caster rely on.

use engine_core::math::{Vec2, Vec3};

/// A smooth curve through `(t, value)` knots (monotone cubic Hermite: no overshoot between
/// knots, so a profile never bulges past its control points). Constant outside the knots.
#[derive(Debug, Clone)]
pub struct Curve {
    t: Vec<f32>,
    v: Vec<f32>,
    m: Vec<f32>,
}

impl Curve {
    /// Knots must be sorted by `t`.
    pub fn new(points: &[(f32, f32)]) -> Self {
        let n = points.len();
        assert!(n >= 2, "a curve needs two knots");
        let t: Vec<f32> = points.iter().map(|p| p.0).collect();
        let v: Vec<f32> = points.iter().map(|p| p.1).collect();
        let d: Vec<f32> = (0..n - 1)
            .map(|i| (v[i + 1] - v[i]) / (t[i + 1] - t[i]).max(1e-6))
            .collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0.0 {
                0.0
            } else {
                // Weighted harmonic mean (Fritsch–Butland): keeps monotone stretches monotone.
                let (h0, h1) = (t[i] - t[i - 1], t[i + 1] - t[i]);
                let w1 = 2.0 * h1 + h0;
                let w2 = h1 + 2.0 * h0;
                (w1 + w2) / (w1 / d[i - 1] + w2 / d[i])
            };
        }
        Self { t, v, m }
    }

    fn segment(&self, x: f32) -> usize {
        let n = self.t.len();
        // Binary search for the segment (few knots, so this is cheap).
        let mut lo = 0;
        let mut hi = n - 1;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if self.t[mid] <= x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Value at `x`.
    pub fn at(&self, x: f32) -> f32 {
        self.eval(x).0
    }

    /// Value and slope at `x`.
    pub fn eval(&self, x: f32) -> (f32, f32) {
        let n = self.t.len();
        if x <= self.t[0] {
            return (self.v[0], 0.0);
        }
        if x >= self.t[n - 1] {
            return (self.v[n - 1], 0.0);
        }
        let i = self.segment(x);
        let h = self.t[i + 1] - self.t[i];
        let s = (x - self.t[i]) / h;
        let (s2, s3) = (s * s, s * s * s);
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        let h11 = s3 - s2;
        let v =
            h00 * self.v[i] + h10 * h * self.m[i] + h01 * self.v[i + 1] + h11 * h * self.m[i + 1];
        let d00 = 6.0 * s2 - 6.0 * s;
        let d10 = 3.0 * s2 - 4.0 * s + 1.0;
        let d01 = -6.0 * s2 + 6.0 * s;
        let d11 = 3.0 * s2 - 2.0 * s;
        let dv =
            (d00 * self.v[i] + d01 * self.v[i + 1]) / h + d10 * self.m[i] + d11 * self.m[i + 1];
        (v, dv)
    }

    /// First and last knot positions.
    pub fn range(&self) -> (f32, f32) {
        (self.t[0], self.t[self.t.len() - 1])
    }
}

/// A [`Curve`] sampled into a table (fast lookups in hot loops): value and slope by linear
/// interpolation between samples.
#[derive(Debug, Clone)]
pub struct Table {
    t0: f32,
    inv_dt: f32,
    /// (value, slope) per sample.
    vd: Vec<[f32; 2]>,
}

impl Table {
    /// Sample `curve` at `n` points over its knot range.
    pub fn new(curve: &Curve, n: usize) -> Self {
        let (a, b) = curve.range();
        Self::from_fn(a, b, n, |t| curve.eval(t))
    }

    /// Sample `f(t) -> (value, slope)` at `n` points over `[a, b]`.
    pub fn from_fn(a: f32, b: f32, n: usize, f: impl Fn(f32) -> (f32, f32)) -> Self {
        let n = n.max(2);
        let dt = (b - a) / (n - 1) as f32;
        let vd = (0..n)
            .map(|i| {
                let (v, d) = f(a + dt * i as f32);
                [v, d]
            })
            .collect();
        Self {
            t0: a,
            inv_dt: 1.0 / dt,
            vd,
        }
    }

    /// Shorthand: a curve through `points`, sampled every ~2 mm.
    pub fn of(points: &[(f32, f32)]) -> Self {
        let c = Curve::new(points);
        let (a, b) = c.range();
        Self::new(&c, (((b - a) / 0.002) as usize).clamp(16, 4096))
    }

    /// Value at `x` (constant beyond the ends).
    #[inline(always)]
    pub fn at(&self, x: f32) -> f32 {
        self.eval(x).0
    }

    /// Value and slope at `x`. (Always inlined: it sits in the body fields' hot paths.)
    #[inline(always)]
    pub fn eval(&self, x: f32) -> (f32, f32) {
        let n = self.vd.len();
        let s = ((x - self.t0) * self.inv_dt).clamp(0.0, (n - 1) as f32);
        let i = (s as usize).min(n - 2);
        let f = s - i as f32;
        let (a, b) = (self.vd[i], self.vd[i + 1]);
        (a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f)
    }
}

/// Smooth step from 0 at `a` to 1 at `b`.
#[inline]
pub fn sstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A bump: 1 at `c`, falling smoothly to 0 at `c ± w`.
pub fn bump(c: f32, w: f32, x: f32) -> f32 {
    let t = ((x - c) / w).abs();
    if t >= 1.0 {
        0.0
    } else {
        let u = 1.0 - t * t;
        u * u
    }
}

/// Intersection with a circular fillet of radius `r` where the two surfaces meet.
pub fn rmax(a: f32, b: f32, r: f32) -> f32 {
    let u = Vec2::new((a + r).max(0.0), (b + r).max(0.0));
    (-r).min(a.max(b)) + u.length()
}

/// Union with a circular fillet of radius `r`.
pub fn rmin(a: f32, b: f32, r: f32) -> f32 {
    -rmax(-a, -b, r)
}

/// Subtract `b` from `a` with a rounded edge of radius `r`.
pub fn rsub(a: f32, b: f32, r: f32) -> f32 {
    rmax(a, -b, r)
}

/// Polynomial smooth minimum (blend width `k`).
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

/// Polynomial smooth maximum (blend width `k`).
pub fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// Box centered at the origin with half size `b`, edges rounded by `r` (inside `b`).
pub fn sd_round_box(p: Vec3, b: Vec3, r: f32) -> f32 {
    let q = p.abs() - b + Vec3::splat(r);
    q.max(Vec3::ZERO).length() + q.x.max(q.y.max(q.z)).min(0.0) - r
}

/// 2D rounded rectangle (half size `b`, corner radius `r`).
pub fn sd_round_rect(p: Vec2, b: Vec2, r: f32) -> f32 {
    let q = p.abs() - b + Vec2::splat(r);
    q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - r
}

/// Capsule from `a` to `b`, radius `r`.
pub fn sd_capsule(p: Vec3, a: Vec3, b: Vec3, r: f32) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = (pa.dot(ba) / ba.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (pa - ba * h).length() - r
}

/// Capsule whose radius goes from `ra` at `a` to `rb` at `b` (approximate).
pub fn sd_cone_capsule(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = (pa.dot(ba) / ba.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (pa - ba * h).length() - (ra + (rb - ra) * h)
}

/// Cylinder along X centered at the origin: radius `r`, half length `hx`, edges rounded `e`.
pub fn sd_cyl_x(p: Vec3, r: f32, hx: f32, e: f32) -> f32 {
    let d = Vec2::new(Vec2::new(p.y, p.z).length() - r + e, p.x.abs() - hx + e);
    d.x.max(d.y).min(0.0) + d.max(Vec2::ZERO).length() - e
}

/// Cylinder along Z centered at the origin.
pub fn sd_cyl_z(p: Vec3, r: f32, hz: f32, e: f32) -> f32 {
    sd_cyl_x(Vec3::new(p.z, p.y, p.x), r, hz, e)
}

/// Cylinder along Y centered at the origin.
pub fn sd_cyl_y(p: Vec3, r: f32, hy: f32, e: f32) -> f32 {
    sd_cyl_x(Vec3::new(p.y, p.x, p.z), r, hy, e)
}

/// Ellipsoid (approximate distance, good near the surface).
pub fn sd_ellipsoid(p: Vec3, r: Vec3) -> f32 {
    let k0 = (p / r).length();
    let k1 = (p / (r * r)).length();
    if k1 < 1e-9 {
        return -r.min_element();
    }
    k0 * (k0 - 1.0) / k1
}

/// Torus in the YZ plane (axis X): ring radius `r`, tube radius `t`.
pub fn sd_torus_x(p: Vec3, r: f32, t: f32) -> f32 {
    let q = Vec2::new(Vec2::new(p.y, p.z).length() - r, p.x);
    q.length() - t
}

/// Distance from `p` to the segment `a`–`b` in 2D.
pub fn seg_dist2(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = (pa.dot(ba) / ba.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (pa - ba * h).length()
}

/// Signed distance to a closed 2D polygon (negative inside).
pub fn sd_polygon(p: Vec2, v: &[Vec2]) -> f32 {
    let n = v.len();
    let mut d = (p - v[0]).length_squared();
    let mut s = 1.0f32;
    let mut j = n - 1;
    for i in 0..n {
        let e = v[j] - v[i];
        let w = p - v[i];
        let b = w - e * (w.dot(e) / e.length_squared().max(1e-12)).clamp(0.0, 1.0);
        d = d.min(b.length_squared());
        let c = [p.y >= v[i].y, p.y < v[j].y, e.x * w.y > e.y * w.x];
        if (c[0] && c[1] && c[2]) || (!c[0] && !c[1] && !c[2]) {
            s = -s;
        }
        j = i;
    }
    s * d.sqrt()
}

/// Rotate a 2D vector by `a` radians.
pub fn rot2(p: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y)
}

/// Repeat angle around the X axis into `n` sectors: returns the point rotated into the
/// sector centered on +Y and the sector index.
pub fn polar_repeat_x(p: Vec3, n: u32) -> (Vec3, u32) {
    let sector = std::f32::consts::TAU / n as f32;
    let a = p.z.atan2(p.y);
    let k = (a / sector).round();
    let q = rot2(Vec2::new(p.y, p.z), -k * sector);
    let idx = ((k as i32).rem_euclid(n as i32)) as u32;
    (Vec3::new(p.x, q.x, q.y), idx)
}

/// Gradient of `f` at `p` (tetrahedral differences, step `e`).
pub fn gradient(f: &(dyn Fn(Vec3) -> f32 + Sync), p: Vec3, e: f32) -> Vec3 {
    let k = [
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(-1.0, -1.0, 1.0),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(1.0, 1.0, 1.0),
    ];
    let mut g = Vec3::ZERO;
    for k in k {
        g += k * f(p + k * e);
    }
    g / (4.0 * e)
}

/// The point on the zero surface of `f` under `p` (one Newton step from `p`, where `f` is `fp`).
/// Reading a view-plane outline there instead of at `p` stands an opening's walls square to the
/// skin, so they meet a raked panel at a right angle instead of grazing it in a ragged shelf.
pub fn on_surface(f: &(dyn Fn(Vec3) -> f32 + Sync), p: Vec3, fp: f32) -> Vec3 {
    let g = gradient(f, p, 0.002);
    p - g * (fp / g.length_squared().max(0.25))
}

/// March from `from` along `dir` (unit) until the field crosses zero; the hit point, or
/// `None` within `max_t` meters. The field may be approximate: steps are damped.
pub fn raycast(
    f: &(dyn Fn(Vec3) -> f32 + Sync),
    from: Vec3,
    dir: Vec3,
    max_t: f32,
) -> Option<Vec3> {
    let mut t = 0.0;
    let mut prev_t = 0.0;
    let mut prev_d = f(from);
    if prev_d < 0.0 {
        return Some(from);
    }
    for _ in 0..160 {
        let step = (prev_d * 0.7).max(0.002);
        t += step;
        if t > max_t {
            return None;
        }
        let d = f(from + dir * t);
        if d < 0.0 {
            // Bisect between the last outside and this inside sample.
            let (mut a, mut b) = (prev_t, t);
            for _ in 0..12 {
                let m = 0.5 * (a + b);
                if f(from + dir * m) < 0.0 {
                    b = m;
                } else {
                    a = m;
                }
            }
            return Some(from + dir * (0.5 * (a + b)));
        }
        prev_t = t;
        prev_d = d;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_passes_through_knots_and_stays_monotone() {
        let c = Curve::new(&[(0.0, 0.0), (1.0, 1.0), (2.0, 1.5), (3.0, 1.5)]);
        assert!((c.at(1.0) - 1.0).abs() < 1e-5);
        assert!((c.at(2.0) - 1.5).abs() < 1e-5);
        let mut last = -1.0;
        for i in 0..=300 {
            let v = c.at(i as f32 / 100.0);
            assert!(v >= last - 1e-5 && v <= 1.5 + 1e-5, "{i}: {v}");
            last = v;
        }
        let (_, slope) = c.eval(0.5);
        assert!(slope > 0.5, "{slope}");
    }

    #[test]
    fn rounded_booleans_and_polygons() {
        assert!((rmax(-1.0, -2.0, 0.1) + 1.0).abs() < 1e-6);
        assert!(rmax(0.0, 0.0, 0.1) > 0.0);
        let sq = [
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, 1.0),
        ];
        assert!((sd_polygon(Vec2::ZERO, &sq) + 1.0).abs() < 1e-5);
        assert!((sd_polygon(Vec2::new(3.0, 0.0), &sq) - 2.0).abs() < 1e-5);
        let sphere = |p: Vec3| p.length() - 1.0;
        let hit = raycast(&sphere, Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, 10.0).unwrap();
        assert!((hit.z - 1.0).abs() < 1e-3, "{hit}");
    }
}
