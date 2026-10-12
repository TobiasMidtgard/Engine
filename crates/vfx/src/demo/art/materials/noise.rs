//! Tileable noise for generated textures.
//!
//! Every function takes a point in *tile units* (the texture spans 0..1 on each axis) and a
//! lattice period in cells per tile, and repeats exactly when the point moves by a whole tile,
//! so textures made from it tile without seams. Points outside 0..1 (after domain warping)
//! wrap. Results depend only on the inputs and the seed.

use engine_core::math::Vec2;

/// Integer finalizer (lowbias32): a fast, well mixed 32-bit hash.
#[inline(always)]
pub fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Hash of a lattice point. `s` should already be mixed (`mix32(seed)`).
#[inline(always)]
pub fn hash2(x: i32, y: i32, s: u32) -> u32 {
    mix32(
        (x as u32)
            .wrapping_mul(0x8da6_b343)
            .wrapping_add((y as u32).wrapping_mul(0xd816_3841))
            ^ s,
    )
}

/// A hash as a float in `[0, 1)`.
#[inline(always)]
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Seed for sub-feature `k` of seed `seed`.
#[inline(always)]
pub fn sub(seed: u32, k: u32) -> u32 {
    mix32(seed ^ k.wrapping_mul(0x632b_e5ab).wrapping_add(0x1234_5677))
}

/// Uniform `[0, 1)` value for `(seed, k)`; handy for per-feature randomness.
#[inline(always)]
pub fn rand(seed: u32, k: u32) -> f32 {
    unit(sub(seed, k))
}

/// `x.floor()` without a library call (the default x86-64 target has no rounding
/// instruction). Exact for |x| < 2^31, which covers every coordinate used here.
#[inline(always)]
pub fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

/// `x.round()` (halves away from zero) without a library call; exact for |x| < 2^31.
#[inline(always)]
pub fn round(x: f32) -> f32 {
    let t = x as i32 as f32;
    let d = x - t;
    if d >= 0.5 {
        t + 1.0
    } else if d <= -0.5 {
        t - 1.0
    } else {
        t
    }
}

#[inline(always)]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline(always)]
fn wrap(i: i32, n: i32) -> i32 {
    if i < 0 {
        i + n
    } else if i >= n {
        i - n
    } else {
        i
    }
}

/// 16 unit gradients, rotated off the axes so the lattice does not show.
const GRAD: [(f32, f32); 16] = [
    (0.980_785, 0.195_09),
    (0.831_47, 0.555_57),
    (0.555_57, 0.831_47),
    (0.195_09, 0.980_785),
    (-0.195_09, 0.980_785),
    (-0.555_57, 0.831_47),
    (-0.831_47, 0.555_57),
    (-0.980_785, 0.195_09),
    (-0.980_785, -0.195_09),
    (-0.831_47, -0.555_57),
    (-0.555_57, -0.831_47),
    (-0.195_09, -0.980_785),
    (0.195_09, -0.980_785),
    (0.555_57, -0.831_47),
    (0.831_47, -0.555_57),
    (0.980_785, -0.195_09),
];

/// Lattice cell of `p` for a period: integer cell (wrapped), next cell (wrapped) and the
/// fractional position inside the cell.
#[inline(always)]
fn cell(p: f32, n: i32) -> (i32, i32, f32) {
    let x = (p - floor(p)) * n as f32;
    let i = (x as i32).min(n - 1);
    let i1 = if i + 1 == n { 0 } else { i + 1 };
    (i, i1, x - i as f32)
}

/// Integer cell and position inside it for Voronoi lookups (`p` wrapped into the tile first).
#[inline(always)]
fn vcell(p: f32, n: i32) -> (i32, f32) {
    let x = (p - floor(p)) * n as f32;
    let i = (x as i32).min(n - 1);
    (i, x - i as f32)
}

/// Gradient (Perlin) noise, about -1..1, with `period` cells across the tile per axis.
pub fn gradient(p: Vec2, period: [i32; 2], seed: u32) -> f32 {
    let s = mix32(seed);
    let (x0, x1, fx) = cell(p.x, period[0]);
    let (y0, y1, fy) = cell(p.y, period[1]);
    let g = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let (gx, gy) = GRAD[(hash2(cx, cy, s) & 15) as usize];
        gx * dx + gy * dy
    };
    let n00 = g(x0, y0, fx, fy);
    let n10 = g(x1, y0, fx - 1.0, fy);
    let n01 = g(x0, y1, fx, fy - 1.0);
    let n11 = g(x1, y1, fx - 1.0, fy - 1.0);
    let sx = fade(fx);
    let sy = fade(fy);
    let a = n00 + (n10 - n00) * sx;
    let b = n01 + (n11 - n01) * sx;
    (a + (b - a) * sy) * 1.414
}

/// Value noise, 0..1, with `period` cells across the tile per axis.
pub fn value(p: Vec2, period: [i32; 2], seed: u32) -> f32 {
    let s = mix32(seed);
    let (x0, x1, fx) = cell(p.x, period[0]);
    let (y0, y1, fy) = cell(p.y, period[1]);
    let v = |cx: i32, cy: i32| unit(hash2(cx, cy, s));
    let sx = fade(fx);
    let sy = fade(fy);
    let a = v(x0, y0) + (v(x1, y0) - v(x0, y0)) * sx;
    let b = v(x0, y1) + (v(x1, y1) - v(x0, y1)) * sx;
    a + (b - a) * sy
}

/// Per-octave shift so octaves do not share lattice lines.
#[inline(always)]
fn octave_seed(seed: u32, o: u32) -> (u32, Vec2) {
    let s = sub(seed, o + 1);
    (
        s,
        Vec2::new(unit(mix32(s ^ 0xa5a5_a5a5)), unit(mix32(s ^ 0x5a5a_5a5a))),
    )
}

/// Fractal sum of gradient noise: `octaves` layers, each with twice the cells of the last and
/// `gain` times its amplitude. Normalized so the result stays about -1..1 (most values fall in
/// -0.6..0.6).
pub fn fbm(p: Vec2, period: [i32; 2], octaves: u32, gain: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    for o in 0..octaves {
        let (s, shift) = octave_seed(seed, o);
        sum += amp * gradient(p + shift, [period[0] << o, period[1] << o], s);
        norm += amp;
        amp *= gain;
    }
    sum / norm
}

/// Ridged fractal noise, 0..1: sharp crests where the gradient noise crosses zero. Good for
/// cracks, rock ridges and veins.
pub fn ridged(p: Vec2, period: [i32; 2], octaves: u32, gain: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut weight = 1.0f32;
    for o in 0..octaves {
        let (s, shift) = octave_seed(seed, o);
        let n = 1.0 - gradient(p + shift, [period[0] << o, period[1] << o], s).abs();
        let n = n * n;
        sum += amp * n * weight;
        weight = (n * 1.5).clamp(0.0, 1.0);
        norm += amp;
        amp *= gain;
    }
    sum / norm
}

/// The result of a Voronoi (Worley) lookup. Distances are in cell units.
#[derive(Debug, Clone, Copy, Default)]
pub struct Voronoi {
    /// Distance to the nearest feature point.
    pub f1: f32,
    /// Distance to the second nearest feature point.
    pub f2: f32,
    /// Hash of the nearest point's cell (stable per cell; use it for per-cell randomness).
    pub id: u32,
    /// Vector from the sample to the nearest feature point (cell units).
    pub to_point: Vec2,
    /// True distance to the nearest cell border (only filled by [`voronoi_edge`]).
    pub edge: f32,
}

/// Wrap for offsets up to two periods away (small periods with a 5×5 search).
#[inline(always)]
fn wrap2(i: i32, n: i32) -> i32 {
    i.rem_euclid(n)
}

#[inline(always)]
fn feature(cx: i32, cy: i32, s: u32, jitter: f32) -> (u32, Vec2) {
    let h = hash2(cx, cy, s);
    let jx = unit(h) - 0.5;
    let jy = unit(mix32(h ^ 0x68e3_1da4)) - 0.5;
    (h, Vec2::new(0.5 + jitter * jx, 0.5 + jitter * jy))
}

/// Voronoi cells with `period` cells across the tile per axis. `jitter` 0 gives a square grid,
/// 1 fully random points.
pub fn voronoi(p: Vec2, period: [i32; 2], jitter: f32, seed: u32) -> Voronoi {
    let s = mix32(seed);
    let (ix, fx) = vcell(p.x, period[0]);
    let (iy, fy) = vcell(p.y, period[1]);
    let f = Vec2::new(fx, fy);
    let mut out = Voronoi {
        f1: f32::MAX,
        f2: f32::MAX,
        ..Default::default()
    };
    for j in -1..=1 {
        for i in -1..=1 {
            let cx = wrap(ix + i, period[0]);
            let cy = wrap(iy + j, period[1]);
            let (h, o) = feature(cx, cy, s, jitter);
            let r = Vec2::new(i as f32, j as f32) + o - f;
            let d = r.length_squared();
            if d < out.f1 {
                out.f2 = out.f1;
                out.f1 = d;
                out.id = h;
                out.to_point = r;
            } else if d < out.f2 {
                out.f2 = d;
            }
        }
    }
    out.f1 = out.f1.sqrt();
    out.f2 = out.f2.sqrt();
    out
}

/// [`voronoi`] plus the true distance to the nearest cell border in [`Voronoi::edge`] (for
/// cracks, cell outlines and paving joints).
pub fn voronoi_edge(p: Vec2, period: [i32; 2], jitter: f32, seed: u32) -> Voronoi {
    let s = mix32(seed);
    let (ix, fx) = vcell(p.x, period[0]);
    let (iy, fy) = vcell(p.y, period[1]);
    let f = Vec2::new(fx, fy);
    let mut out = Voronoi {
        f1: f32::MAX,
        f2: f32::MAX,
        ..Default::default()
    };
    let mut best = (0, 0);
    for j in -1..=1 {
        for i in -1..=1 {
            let (h, o) = feature(wrap(ix + i, period[0]), wrap(iy + j, period[1]), s, jitter);
            let r = Vec2::new(i as f32, j as f32) + o - f;
            let d = r.length_squared();
            if d < out.f1 {
                out.f2 = out.f1;
                out.f1 = d;
                out.id = h;
                out.to_point = r;
                best = (i, j);
            } else if d < out.f2 {
                out.f2 = d;
            }
        }
    }
    let mr = out.to_point;
    let mut edge = f32::MAX;
    for j in -1..=1 {
        for i in -1..=1 {
            let (ci, cj) = (best.0 + i, best.1 + j);
            if ci == best.0 && cj == best.1 {
                continue;
            }
            let (_, o) = feature(
                wrap2(ix + ci, period[0]),
                wrap2(iy + cj, period[1]),
                s,
                jitter,
            );
            let r = Vec2::new(ci as f32, cj as f32) + o - f;
            let d = r - mr;
            let len2 = d.length_squared();
            if len2 > 1e-8 {
                edge = edge.min((mr + r).dot(d) * 0.5 / len2.sqrt());
            }
        }
    }
    out.f1 = out.f1.sqrt();
    out.f2 = out.f2.sqrt();
    out.edge = edge.max(0.0);
    out
}

/// Calls `f(id, r)` for the feature point of each of the 3×3 cells around `p`, with `r` the
/// vector from that point to `p` in cell units. For scattering small things (pebbles,
/// strands, leaves) that may reach into neighboring cells.
pub fn cells3(p: Vec2, period: [i32; 2], jitter: f32, seed: u32, mut f: impl FnMut(u32, Vec2)) {
    let s = mix32(seed);
    let (ix, fx) = vcell(p.x, period[0]);
    let (iy, fy) = vcell(p.y, period[1]);
    let fp = Vec2::new(fx, fy);
    for j in -1..=1 {
        for i in -1..=1 {
            let (h, o) = feature(wrap(ix + i, period[0]), wrap(iy + j, period[1]), s, jitter);
            f(h, fp - (Vec2::new(i as f32, j as f32) + o));
        }
    }
}

/// Smoothly blended Voronoi: a soft minimum of the squared distances to nearby feature points
/// (cell units²). Each point makes a round dish; neighboring dishes meet in rounded ridges
/// instead of sharp creases. `k` sets how soft the ridges are (larger is sharper).
pub fn smooth_cells(p: Vec2, period: [i32; 2], jitter: f32, k: f32, seed: u32) -> f32 {
    let mut sum = 0.0f32;
    cells3(p, period, jitter, seed, |_, r| {
        sum += (-k * r.length_squared()).exp();
    });
    -(sum.max(1e-12)).ln() / k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_repeats_every_tile() {
        let period = [5, 3];
        for k in 0..50 {
            let p = Vec2::new(rand(1, k) * 1.0, rand(2, k));
            let q = p + Vec2::new(1.0, -2.0);
            assert!((gradient(p, period, 7) - gradient(q, period, 7)).abs() < 1e-3);
            assert!((value(p, period, 7) - value(q, period, 7)).abs() < 1e-3);
            assert!((fbm(p, period, 4, 0.5, 7) - fbm(q, period, 4, 0.5, 7)).abs() < 1e-3);
            let a = voronoi_edge(p, period, 0.9, 3);
            let b = voronoi_edge(q, period, 0.9, 3);
            assert_eq!(a.id, b.id);
            assert!((a.f1 - b.f1).abs() < 1e-3 && (a.edge - b.edge).abs() < 1e-3);
        }
    }

    #[test]
    fn gradient_noise_stays_in_range() {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for k in 0..4000 {
            let p = Vec2::new(rand(11, k), rand(12, k));
            let n = gradient(p, [8, 8], 1);
            lo = lo.min(n);
            hi = hi.max(n);
        }
        assert!(lo > -1.05 && hi < 1.05, "{lo} {hi}");
        assert!(lo < -0.5 && hi > 0.5, "{lo} {hi}");
    }
}
