//! Fast, tileable noise for the scenery textures and vertex colors: integer hashes, value and
//! gradient noise on a periodic lattice, fractal sums and cellular (Worley) noise.
//!
//! "Periodic" means the noise repeats every `period` lattice cells, so a texture sampled over
//! `0..period` tiles seamlessly. Pass a large period (e.g. 1 << 20) for non-repeating noise in
//! world space.

/// 32-bit integer hash of two lattice coordinates and a seed (lowbias32-style mixing).
#[inline]
pub fn hash2(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32)
        .wrapping_mul(0x8da6_b343)
        .wrapping_add((y as u32).wrapping_mul(0xd816_3841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

/// Hash of one integer and a seed.
#[inline]
pub fn hash1(x: i32, seed: u32) -> u32 {
    hash2(x, 0x5bd1_e995u32 as i32, seed)
}

/// `hash2` mapped to `[0, 1)`.
#[inline]
pub fn rand2(x: i32, y: i32, seed: u32) -> f32 {
    (hash2(x, y, seed) >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// `hash1` mapped to `[0, 1)`.
#[inline]
pub fn rand1(x: i32, seed: u32) -> f32 {
    (hash1(x, seed) >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
fn wrap(i: i32, p: i32) -> i32 {
    i.rem_euclid(p)
}

#[inline]
fn quintic(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Smooth value noise in `[0, 1]`, periodic every `period` cells in x and y.
pub fn value(x: f32, y: f32, period: (i32, i32), seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (u, v) = (quintic(x - xf), quintic(y - yf));
    let (x0, x1) = (wrap(xi, period.0), wrap(xi + 1, period.0));
    let (y0, y1) = (wrap(yi, period.1), wrap(yi + 1, period.1));
    let a = rand2(x0, y0, seed);
    let b = rand2(x1, y0, seed);
    let c = rand2(x0, y1, seed);
    let d = rand2(x1, y1, seed);
    let top = a + (b - a) * u;
    let bottom = c + (d - c) * u;
    top + (bottom - top) * v
}

const GRADS: [(f32, f32); 16] = [
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

/// Gradient (Perlin-style) noise in about `[-1, 1]`, periodic every `period` cells.
pub fn gradient(x: f32, y: f32, period: (i32, i32), seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let (u, v) = (quintic(fx), quintic(fy));
    let (x0, x1) = (wrap(xi, period.0), wrap(xi + 1, period.0));
    let (y0, y1) = (wrap(yi, period.1), wrap(yi + 1, period.1));
    let g = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let (gx, gy) = GRADS[(hash2(cx, cy, seed) & 15) as usize];
        gx * dx + gy * dy
    };
    let a = g(x0, y0, fx, fy);
    let b = g(x1, y0, fx - 1.0, fy);
    let c = g(x0, y1, fx, fy - 1.0);
    let d = g(x1, y1, fx - 1.0, fy - 1.0);
    let top = a + (b - a) * u;
    let bottom = c + (d - c) * u;
    (top + (bottom - top) * v) * 1.4
}

/// Fractal sum of [`gradient`] noise in about `[-1, 1]`: `octaves` layers, each twice the
/// frequency (and twice the period, so it still tiles) at `gain` times the amplitude.
pub fn fbm(x: f32, y: f32, period: (i32, i32), octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut fx, mut fy, mut p) = (x, y, period);
    for o in 0..octaves {
        sum += amp * gradient(fx, fy, p, seed.wrapping_add(o * 977));
        norm += amp;
        amp *= gain;
        fx *= 2.0;
        fy *= 2.0;
        p = (p.0 * 2, p.1 * 2);
    }
    sum / norm
}

/// Non-repeating [`fbm`] for world-space coordinates (meters / `scale`).
pub fn world_fbm(x: f32, z: f32, scale: f32, octaves: u32, seed: u32) -> f32 {
    fbm(x / scale, z / scale, (1 << 20, 1 << 20), octaves, 0.5, seed)
}

/// Cellular noise result.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    /// Distance to the nearest feature point (cell units).
    pub f1: f32,
    /// Distance to the second nearest.
    pub f2: f32,
    /// Hash of the nearest cell (stable per stone / pebble).
    pub id: u32,
    /// Offset from the nearest feature point to the sample (cell units).
    pub dx: f32,
    pub dy: f32,
}

/// Worley noise with one jittered feature point per cell, periodic every `period` cells.
/// `jitter` 0..1 (1 = fully random positions).
pub fn worley(x: f32, y: f32, period: (i32, i32), jitter: f32, seed: u32) -> Cell {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let mut best = Cell {
        f1: 9.0,
        f2: 9.0,
        id: 0,
        dx: 0.0,
        dy: 0.0,
    };
    for oy in -1..=1 {
        for ox in -1..=1 {
            let cx = wrap(xi + ox, period.0);
            let cy = wrap(yi + oy, period.1);
            let h = hash2(cx, cy, seed);
            let px = ox as f32 + 0.5 + ((h & 0xffff) as f32 / 65535.0 - 0.5) * jitter;
            let py = oy as f32 + 0.5 + ((h >> 16) as f32 / 65535.0 - 0.5) * jitter;
            let (dx, dy) = (fx - px, fy - py);
            let d = (dx * dx + dy * dy).sqrt();
            if d < best.f1 {
                best.f2 = best.f1;
                best.f1 = d;
                best.id = h;
                best.dx = dx;
                best.dy = dy;
            } else if d < best.f2 {
                best.f2 = d;
            }
        }
    }
    best
}

/// Smooth Hermite step.
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear interpolation.
#[inline]
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_and_stays_in_range() {
        for i in 0..200 {
            let x = i as f32 * 0.137;
            let y = i as f32 * 0.071;
            let a = fbm(x, y, (8, 8), 4, 0.5, 3);
            let b = fbm(x + 8.0, y + 8.0, (8, 8), 4, 0.5, 3);
            assert!((a - b).abs() < 1e-4, "{a} {b}");
            assert!(a.abs() <= 1.5);
            let v = value(x, y, (4, 4), 1);
            assert!((0.0..=1.0).contains(&v));
            let c = worley(x, y, (5, 5), 1.0, 2);
            let d = worley(x + 5.0, y, (5, 5), 1.0, 2);
            assert!((c.f1 - d.f1).abs() < 1e-4 && c.id == d.id);
            assert!(c.f1 <= c.f2);
        }
    }
}
