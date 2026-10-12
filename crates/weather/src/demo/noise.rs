//! Small, fast, tileable noise for the station's procedural textures and meshes.
//!
//! Every function takes a `period` in lattice cells: the result repeats every `period` units in
//! x and y, so a texture sampled over `[0, period)` tiles seamlessly. All results are
//! deterministic for a given `seed`.

/// 32-bit integer hash of a lattice point (good avalanche, cheap).
#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f).wrapping_add(0x165_667b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// A hash mapped to `[0, 1)`.
#[inline]
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Small deterministic random sequence (xorshift), for layouts.
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(hash(seed as i32, 77, 0x9e37) | 1)
    }
    /// Next value in `[0, 1)`.
    pub fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        unit(x)
    }
    /// Next value in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

/// Tiling period in lattice cells along x and y (0 = no tiling).
pub type Period = (i32, i32);

/// The same period along both axes.
pub const fn sq(p: i32) -> Period {
    (p, p)
}

#[inline]
fn wrap(i: i32, period: i32) -> i32 {
    if period <= 0 || (0..period).contains(&i) {
        i
    } else {
        i.rem_euclid(period)
    }
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
pub fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Value noise in `[0, 1]` (quintic interpolation), tiling every `period` units.
pub fn value(x: f32, y: f32, period: Period, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (fade(x - fx), fade(y - fy));
    let (xi, yi) = (fx as i32, fy as i32);
    let (x0, y0) = (wrap(xi, period.0), wrap(yi, period.1));
    let (x1, y1) = (step(x0, 1, period.0), step(y0, 1, period.1));
    let a = unit(hash(x0, y0, seed));
    let b = unit(hash(x1, y0, seed));
    let c = unit(hash(x0, y1, seed));
    let d = unit(hash(x1, y1, seed));
    lerp(lerp(a, b, tx), lerp(c, d, tx), ty)
}

const GRADIENTS: [(f32, f32); 16] = [
    (1.0, 0.0),
    (0.924, 0.383),
    (0.707, 0.707),
    (0.383, 0.924),
    (0.0, 1.0),
    (-0.383, 0.924),
    (-0.707, 0.707),
    (-0.924, 0.383),
    (-1.0, 0.0),
    (-0.924, -0.383),
    (-0.707, -0.707),
    (-0.383, -0.924),
    (0.0, -1.0),
    (0.383, -0.924),
    (0.707, -0.707),
    (0.924, -0.383),
];

#[inline]
fn grad(h: u32, x: f32, y: f32) -> f32 {
    let (gx, gy) = GRADIENTS[(h >> 28) as usize];
    gx * x + gy * y
}

/// Gradient (Perlin) noise, roughly in `[-1, 1]`, tiling every `period` units.
pub fn perlin(x: f32, y: f32, period: Period, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (rx, ry) = (x - fx, y - fy);
    let (xi, yi) = (fx as i32, fy as i32);
    let (x0, y0) = (wrap(xi, period.0), wrap(yi, period.1));
    let (x1, y1) = (step(x0, 1, period.0), step(y0, 1, period.1));
    let n00 = grad(hash(x0, y0, seed), rx, ry);
    let n10 = grad(hash(x1, y0, seed), rx - 1.0, ry);
    let n01 = grad(hash(x0, y1, seed), rx, ry - 1.0);
    let n11 = grad(hash(x1, y1, seed), rx - 1.0, ry - 1.0);
    let (u, v) = (fade(rx), fade(ry));
    lerp(lerp(n00, n10, u), lerp(n01, n11, u), v) * 1.42
}

/// Fractal Brownian motion of Perlin noise, roughly in `[-1, 1]`. Each octave doubles the
/// frequency and the period, so the sum still tiles every `period` units of the first octave.
pub fn fbm(x: f32, y: f32, period: Period, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut fx, mut fy, mut p) = (x, y, period);
    for o in 0..octaves {
        sum += perlin(fx, fy, p, seed.wrapping_add(o * 1013)) * amp;
        norm += amp;
        amp *= 0.5;
        fx *= 2.0;
        fy *= 2.0;
        p = (p.0 * 2, p.1 * 2);
    }
    sum / norm
}

/// Result of a Worley (cellular) lookup.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    /// Distance to the nearest feature point (cell units).
    pub f1: f32,
    /// Distance to the second nearest feature point.
    pub f2: f32,
    /// Hash of the nearest cell (stable per cell, use for per-cell randomness).
    pub id: u32,
    /// Offset from the sample to the nearest feature point.
    pub dx: f32,
    pub dy: f32,
}

/// Worley noise with one jittered feature point per cell, tiling every `period` cells.
pub fn worley(x: f32, y: f32, period: Period, jitter: f32, seed: u32) -> Cell {
    let (fx, fy) = (x.floor(), y.floor());
    let (xi, yi) = (wrap(fx as i32, period.0), wrap(fy as i32, period.1));
    let (rx, ry) = (x - fx, y - fy);
    let (mut d1, mut d2) = (81.0f32, 81.0f32);
    let mut best = Cell {
        f1: 9.0,
        f2: 9.0,
        id: 0,
        dx: 0.0,
        dy: 0.0,
    };
    for oy in -1..=1 {
        let cy = step(yi, oy, period.1);
        for ox in -1..=1 {
            let cx = step(xi, ox, period.0);
            let h = hash(cx, cy, seed);
            let px = ox as f32 + 0.5 + (unit(h) - 0.5) * jitter;
            let py = oy as f32 + 0.5 + (unit(h.rotate_left(13)) - 0.5) * jitter;
            let (dx, dy) = (px - rx, py - ry);
            let d = dx * dx + dy * dy;
            if d < d1 {
                d2 = d1;
                d1 = d;
                best.id = h;
                best.dx = dx;
                best.dy = dy;
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    best.f1 = d1.sqrt();
    best.f2 = d2.sqrt();
    best
}

/// Neighbor `i + o` (|o| ≤ 1) of an already wrapped lattice index.
#[inline]
fn step(i: i32, o: i32, period: i32) -> i32 {
    let j = i + o;
    if period <= 0 {
        j
    } else if j < 0 {
        j + period
    } else if j >= period {
        j - period
    } else {
        j
    }
}

/// sRGB-encoded byte for a linear value.
#[inline]
pub fn to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// Linear value of an sRGB hex color (0xRRGGBB).
pub fn hex(rgb: u32) -> [f32; 3] {
    let c = |v: u32| {
        let s = (v & 0xff) as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    [c(rgb >> 16), c(rgb >> 8), c(rgb)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_and_stays_in_range() {
        for (x, y) in [(0.3f32, 0.7f32), (2.9, 1.1), (5.5, 3.25)] {
            let p = 4;
            let a = value(x, y, sq(p), 3);
            let b = value(x + p as f32, y - p as f32, sq(p), 3);
            assert!((a - b).abs() < 1e-5);
            assert!((0.0..=1.0).contains(&a));
            let a = fbm(x, y, (p, 2 * p), 5, 9);
            let b = fbm(x + p as f32, y + 2.0 * p as f32, (p, 2 * p), 5, 9);
            assert!((a - b).abs() < 1e-4, "{a} {b}");
            assert!(a.abs() <= 1.2);
            let c = worley(x, y, sq(p), 0.9, 1);
            let d = worley(x - p as f32, y, sq(p), 0.9, 1);
            assert!((c.f1 - d.f1).abs() < 1e-5 && c.id == d.id);
            assert!(c.f1 <= c.f2);
        }
    }
}
