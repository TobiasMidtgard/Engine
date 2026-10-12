//! Small, fast, deterministic noise for the tour's procedural landscape and textures.
//!
//! * `f32` noise with an optional integer `period` (lattice wraps), so fractal sums of it tile
//!   seamlessly: textures use these.
//! * `f64` gradient and value noise (with analytic derivatives) for the terrain, which spans
//!   tens of kilometers.

/// Integer hash of a lattice point (a few multiply-xorshift rounds, good enough for noise).
#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f).wrapping_add(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// Hash to `[0, 1)`.
#[inline]
pub fn rand01(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / 16_777_216.0
}

/// A tiny deterministic random number generator (PCG-ish) for scattering.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xd1b5_4a32_d192_ed03)
    }

    pub fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let x = ((self.0 >> 18) ^ self.0) >> 27;
        let rot = (self.0 >> 59) as u32;
        (x as u32).rotate_right(rot)
    }

    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }

    /// Uniform in `[a, b)`.
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }

    /// Uniform in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        self.f32() * 2.0 - 1.0
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
}

#[inline]
fn wrap(i: i32, period: i32) -> i32 {
    if period > 0 { i.rem_euclid(period) } else { i }
}

/// Lattice period in X and Y (0 = not periodic in that axis).
pub type Period = [i32; 2];

#[inline]
fn quintic(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

const GRADIENTS: [(f32, f32); 16] = {
    let mut g = [(0.0, 0.0); 16];
    // Unit vectors at 22.5° steps (computed at compile time from a small table).
    const D: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let c = [
        1.0, 0.92388, D, 0.38268, 0.0, -0.38268, -D, -0.92388, -1.0, -0.92388, -D, -0.38268, 0.0,
        0.38268, D, 0.92388,
    ];
    let mut i = 0;
    while i < 16 {
        g[i] = (c[i], c[(i + 12) % 16]);
        i += 1;
    }
    g
};

/// 2D gradient (Perlin) noise in about `[-1, 1]`; the lattice repeats every `period` cells
/// when `period > 0`.
pub fn gradient(x: f32, y: f32, period: Period, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let g = |dx: i32, dy: i32| {
        let h = hash(wrap(xi + dx, period[0]), wrap(yi + dy, period[1]), seed);
        let (gx, gy) = GRADIENTS[(h & 15) as usize];
        gx * (fx - dx as f32) + gy * (fy - dy as f32)
    };
    let (u, v) = (quintic(fx), quintic(fy));
    let a = g(0, 0) + (g(1, 0) - g(0, 0)) * u;
    let b = g(0, 1) + (g(1, 1) - g(0, 1)) * u;
    (a + (b - a) * v) * 1.4
}

/// 2D value noise in `[0, 1]`, periodic like [`gradient`].
pub fn value(x: f32, y: f32, period: Period, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let r = |dx: i32, dy: i32| rand01(wrap(xi + dx, period[0]), wrap(yi + dy, period[1]), seed);
    let (u, v) = (quintic(fx), quintic(fy));
    let a = r(0, 0) + (r(1, 0) - r(0, 0)) * u;
    let b = r(0, 1) + (r(1, 1) - r(0, 1)) * u;
    a + (b - a) * v
}

/// Fractal sum of [`gradient`] noise (lacunarity 2), about `[-1, 1]`. `period` is the period of
/// the first octave in lattice cells (0 = not periodic); later octaves stay periodic.
pub fn fbm(x: f32, y: f32, octaves: u32, gain: f32, period: Period, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for i in 0..octaves {
        let p = [period[0] << i, period[1] << i];
        sum += amp * gradient(x * f, y * f, p, seed.wrapping_add(i * 1013));
        norm += amp;
        amp *= gain;
        f *= 2.0;
    }
    sum / norm
}

/// Result of [`worley`]: distances to the nearest and second nearest feature points (in
/// lattice units), and a random id of the nearest cell.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub f1: f32,
    pub f2: f32,
    pub id: u32,
    /// Offset from the sample to the nearest feature point.
    pub to_center: (f32, f32),
}

/// 2D Worley (cellular) noise with one jittered feature point per cell; periodic like
/// [`gradient`]. `jitter` 0 gives a regular grid, 1 fully random points.
pub fn worley(x: f32, y: f32, period: Period, jitter: f32, seed: u32) -> Cell {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let mut best = Cell {
        f1: f32::MAX,
        f2: f32::MAX,
        id: 0,
        to_center: (0.0, 0.0),
    };
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (wrap(xi + dx, period[0]), wrap(yi + dy, period[1]));
            let h = hash(cx, cy, seed);
            let px = dx as f32 + 0.5 + jitter * (((h & 0xffff) as f32 / 65535.0) - 0.5);
            let py = dy as f32 + 0.5 + jitter * (((h >> 16) as f32 / 65535.0) - 0.5);
            let (ox, oy) = (px - fx, py - fy);
            let d = (ox * ox + oy * oy).sqrt();
            if d < best.f1 {
                best.f2 = best.f1;
                best.f1 = d;
                best.id = hash(cx, cy, seed ^ 0x5bd1_e995);
                best.to_center = (ox, oy);
            } else if d < best.f2 {
                best.f2 = d;
            }
        }
    }
    best
}

/// 3D gradient noise in about `[-1, 1]` (not periodic), for displacing rocks.
pub fn gradient3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (xf, yf, zf) = (x.floor(), y.floor(), z.floor());
    let (xi, yi, zi) = (xf as i32, yf as i32, zf as i32);
    let (fx, fy, fz) = (x - xf, y - yf, z - zf);
    let g = |dx: i32, dy: i32, dz: i32| {
        let h = hash(
            xi + dx,
            yi + dy,
            seed ^ ((zi + dz) as u32).wrapping_mul(0x27d4_eb2d),
        );
        let (px, py, pz) = (fx - dx as f32, fy - dy as f32, fz - dz as f32);
        match h % 12 {
            0 => px + py,
            1 => -px + py,
            2 => px - py,
            3 => -px - py,
            4 => px + pz,
            5 => -px + pz,
            6 => px - pz,
            7 => -px - pz,
            8 => py + pz,
            9 => -py + pz,
            10 => py - pz,
            _ => -py - pz,
        }
    };
    let (u, v, w) = (quintic(fx), quintic(fy), quintic(fz));
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = l(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = l(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = l(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = l(g(0, 1, 1), g(1, 1, 1), u);
    l(l(x00, x10, v), l(x01, x11, v), w) * 0.9
}

/// Fractal sum of [`gradient3`], about `[-1, 1]`.
pub fn fbm3(x: f32, y: f32, z: f32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for i in 0..octaves {
        sum += amp * gradient3(x * f, y * f, z * f, seed.wrapping_add(i * 31));
        norm += amp;
        amp *= gain;
        f *= 2.03;
    }
    sum / norm
}

// ------------------------------------------------------------------ f64 terrain noise

#[inline]
fn hash64(x: i64, y: i64, seed: u32) -> u32 {
    hash(
        x as i32 ^ (x >> 32) as i32,
        y as i32 ^ (y >> 32) as i32,
        seed,
    )
}

/// 2D gradient noise in about `[-1, 1]` for world-scale coordinates.
pub fn gradient64(x: f64, y: f64, seed: u32) -> f64 {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i64, yf as i64);
    let (fx, fy) = (x - xf, y - yf);
    let g = |dx: i64, dy: i64| {
        let h = hash64(xi + dx, yi + dy, seed);
        let (gx, gy) = GRADIENTS[(h & 15) as usize];
        gx as f64 * (fx - dx as f64) + gy as f64 * (fy - dy as f64)
    };
    let q = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v) = (q(fx), q(fy));
    let a = g(0, 0) + (g(1, 0) - g(0, 0)) * u;
    let b = g(0, 1) + (g(1, 1) - g(0, 1)) * u;
    (a + (b - a) * v) * 1.4
}

/// Value noise in `[-1, 1]` with its analytic derivatives `(v, dv/dx, dv/dy)`.
pub fn value_d64(x: f64, y: f64, seed: u32) -> (f64, f64, f64) {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i64, yf as i64);
    let (fx, fy) = (x - xf, y - yf);
    let u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let du = 30.0 * fx * fx * (fx * (fx - 2.0) + 1.0);
    let dv = 30.0 * fy * fy * (fy * (fy - 2.0) + 1.0);
    let r = |dx: i64, dy: i64| (hash64(xi + dx, yi + dy, seed) >> 8) as f64 / 8_388_608.0 - 1.0;
    let (a, b, c, d) = (r(0, 0), r(1, 0), r(0, 1), r(1, 1));
    let k1 = b - a;
    let k2 = c - a;
    let k4 = a - b - c + d;
    (
        a + k1 * u + k2 * v + k4 * u * v,
        du * (k1 + k4 * v),
        dv * (k2 + k4 * u),
    )
}

/// Fractal sum of [`gradient64`] (lacunarity ~2, octaves rotated so the lattice doesn't
/// line up), about `[-1, 1]`.
pub fn fbm64(x: f64, y: f64, octaves: u32, gain: f64, seed: u32) -> f64 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut px, mut py) = (x, y);
    for i in 0..octaves {
        sum += amp * gradient64(px, py, seed.wrapping_add(i * 7919));
        norm += amp;
        amp *= gain;
        let (nx, ny) = (1.6 * px + 1.2 * py, -1.2 * px + 1.6 * py);
        (px, py) = (nx + 13.7, ny - 7.3);
    }
    sum / norm
}

/// "Eroded" fractal noise: each octave is damped where the octaves below it are steep, which
/// leaves smooth valleys and sharp, branching ridges (after Inigo Quilez's terrain fbm).
/// Returns about `[-1, 1]`.
pub fn eroded64(x: f64, y: f64, octaves: u32, seed: u32) -> f64 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut dx, mut dy) = (0.0, 0.0);
    let (mut px, mut py) = (x, y);
    // Each octave's coordinates are rotated and doubled; the derivative accumulates in the
    // original frame, scaled with the frequency.
    let (mut m00, mut m01, mut m10, mut m11) = (1.0, 0.0, 0.0, 1.0);
    for i in 0..octaves {
        let (v, gx, gy) = value_d64(px, py, seed.wrapping_add(i * 7919));
        // Chain rule back to the original coordinates: d/dx = J^T * grad.
        let (wx, wy) = (m00 * gx + m10 * gy, m01 * gx + m11 * gy);
        dx += wx * amp;
        dy += wy * amp;
        sum += amp * v / (1.0 + dx * dx + dy * dy);
        norm += amp;
        amp *= 0.5;
        let (nx, ny) = (1.6 * px + 1.2 * py, -1.2 * px + 1.6 * py);
        (px, py) = (nx + 31.4, ny + 17.9);
        (m00, m01, m10, m11) = (
            1.6 * m00 + 1.2 * m10,
            1.6 * m01 + 1.2 * m11,
            -1.2 * m00 + 1.6 * m10,
            -1.2 * m01 + 1.6 * m11,
        );
    }
    sum / norm
}

/// Ridged fractal noise in `[0, 1]`: sharp crests where the noise crosses zero, each octave
/// weighted by the one below so detail gathers along the ridges.
pub fn ridged64(x: f64, y: f64, octaves: u32, seed: u32) -> f64 {
    let (mut sum, mut amp, mut norm, mut weight) = (0.0, 1.0, 0.0, 1.0);
    let (mut px, mut py) = (x, y);
    for i in 0..octaves {
        let n = 1.0 - gradient64(px, py, seed.wrapping_add(i * 104_729)).abs();
        let n = n * n * weight;
        weight = (n * 1.6).clamp(0.0, 1.0);
        sum += amp * n;
        norm += amp;
        amp *= 0.5;
        let (nx, ny) = (1.7 * px + 1.1 * py, -1.1 * px + 1.7 * py);
        (px, py) = (nx + 5.1, ny + 9.7);
    }
    sum / norm
}

#[inline]
pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn smoothstep32(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_noise_tiles() {
        for (x, y) in [(0.3, 0.7), (2.9, 1.1), (5.5, 3.25)] {
            let a = fbm(x, y, 4, 0.5, [8, 8], 3);
            let b = fbm(x + 8.0, y - 16.0, 4, 0.5, [8, 8], 3);
            assert!((a - b).abs() < 1e-4, "{a} {b}");
            let c = worley(x, y, [6, 6], 1.0, 1);
            let d = worley(x + 6.0, y + 12.0, [6, 6], 1.0, 1);
            assert!((c.f1 - d.f1).abs() < 1e-4);
        }
    }

    #[test]
    fn value_noise_derivative_matches_finite_difference() {
        let (x, y) = (3.37, -1.62);
        let (v, dx, dy) = value_d64(x, y, 9);
        let e = 1e-6;
        let fx = (value_d64(x + e, y, 9).0 - v) / e;
        let fy = (value_d64(x, y + e, 9).0 - v) / e;
        assert!((fx - dx).abs() < 1e-3 && (fy - dy).abs() < 1e-3);
    }
}
