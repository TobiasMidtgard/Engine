//! Gradient, fractal and cellular noise for the procedural textures and meshes.

use std::f32::consts::FRAC_1_SQRT_2 as H;

const G2: [[f32; 2]; 16] = [
    [1.0, 0.0],
    [0.92388, 0.38268],
    [H, H],
    [0.38268, 0.92388],
    [0.0, 1.0],
    [-0.38268, 0.92388],
    [-H, H],
    [-0.92388, 0.38268],
    [-1.0, 0.0],
    [-0.92388, -0.38268],
    [-H, -H],
    [-0.38268, -0.92388],
    [0.0, -1.0],
    [0.38268, -0.92388],
    [H, -H],
    [0.92388, -0.38268],
];
const G3: [[f32; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [0.0, -1.0, -1.0],
];

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
    h ^ (h >> 15)
}

#[inline]
pub fn wrap(i: i32, period: i32) -> i32 {
    if period > 0 { i.rem_euclid(period) } else { i }
}

/// 2D gradient noise with its derivatives: `[value, d/dx, d/dy]`, value in about
/// `[-1, 1]`. `period > 0` makes it tile every `period` units.
pub fn noised(x: f32, y: f32, period: i32, seed: u32) -> [f32; 3] {
    noised_xy(x, y, period, period, seed)
}

/// [`noised`] tiling every `px` units in x and `py` in y, for stretched patterns.
pub fn noised_xy(x: f32, y: f32, px: i32, py: i32, seed: u32) -> [f32; 3] {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (x0, y0) = (ix as i32, iy as i32);
    let g = |a: i32, b: i32| G2[(hash(wrap(a, px), wrap(b, py), 0, seed) & 15) as usize];
    let ga = g(x0, y0);
    let gb = g(x0 + 1, y0);
    let gc = g(x0, y0 + 1);
    let gd = g(x0 + 1, y0 + 1);
    let va = ga[0] * fx + ga[1] * fy;
    let vb = gb[0] * (fx - 1.0) + gb[1] * fy;
    let vc = gc[0] * fx + gc[1] * (fy - 1.0);
    let vd = gd[0] * (fx - 1.0) + gd[1] * (fy - 1.0);
    let ux = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let uy = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let dux = 30.0 * fx * fx * (fx * (fx - 2.0) + 1.0);
    let duy = 30.0 * fy * fy * (fy * (fy - 2.0) + 1.0);
    let k = va - vb - vc + vd;
    let v = va + ux * (vb - va) + uy * (vc - va) + ux * uy * k;
    let dx = ga[0]
        + ux * (gb[0] - ga[0])
        + uy * (gc[0] - ga[0])
        + ux * uy * (ga[0] - gb[0] - gc[0] + gd[0])
        + dux * (uy * k + vb - va);
    let dy = ga[1]
        + ux * (gb[1] - ga[1])
        + uy * (gc[1] - ga[1])
        + ux * uy * (ga[1] - gb[1] - gc[1] + gd[1])
        + duy * (ux * k + vc - va);
    [v * 1.4, dx * 1.4, dy * 1.4]
}

#[inline]
pub fn n2(x: f32, y: f32, seed: u32) -> f32 {
    noised(x, y, 0, seed)[0]
}

/// 3D gradient noise, about `[-1, 1]`.
pub fn n3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (ix, iy, iz) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (x - ix, y - iy, z - iz);
    let (x0, y0, z0) = (ix as i32, iy as i32, iz as i32);
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v, w) = (fade(fx), fade(fy), fade(fz));
    let g = |dx: i32, dy: i32, dz: i32| {
        let gr = G3[(hash(x0 + dx, y0 + dy, z0 + dz, seed) & 15) as usize];
        gr[0] * (fx - dx as f32) + gr[1] * (fy - dy as f32) + gr[2] * (fz - dz as f32)
    };
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = l(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = l(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = l(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = l(g(0, 1, 1), g(1, 1, 1), u);
    l(l(x00, x10, v), l(x01, x11, v), w) * 0.95
}

/// Fractal sum of `n2`, about `[-1, 1]`. Octaves are rotated against each other.
pub fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let (mut x, mut y) = (x, y);
    let (mut sum, mut amp) = (0.0, 0.5);
    for i in 0..octaves {
        sum += amp * n2(x, y, seed.wrapping_add(i * 7919));
        amp *= 0.5;
        (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
    }
    sum * 1.15
}

/// Like [`fbm`], but octaves with a wavelength (in input units) below `min_wl` fade out.
pub fn fbm_lod(x: f32, y: f32, octaves: u32, min_wl: f32, seed: u32) -> f32 {
    let (mut x, mut y) = (x, y);
    let (mut sum, mut amp, mut wl) = (0.0, 0.5, 1.0);
    for i in 0..octaves {
        let w = super::sstep(1.0, 2.0, wl / min_wl.max(1e-6));
        if w <= 0.0 {
            break;
        }
        sum += amp * w * n2(x, y, seed.wrapping_add(i * 7919));
        amp *= 0.5;
        wl *= 0.5;
        (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
    }
    sum * 1.15
}

/// Tileable fractal sum: tiles every `period` units (frequencies double, so it stays
/// tileable).
pub fn fbm_tile(x: f32, y: f32, octaves: u32, period: i32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut f) = (0.0, 0.5, 1.0);
    for i in 0..octaves {
        sum += amp * noised(x * f, y * f, period * f as i32, seed.wrapping_add(i * 7919))[0];
        amp *= 0.5;
        f *= 2.0;
    }
    sum * 1.15
}

/// [`fbm_tile`] tiling every `px` units in x and `py` in y.
pub fn fbm_tile_xy(x: f32, y: f32, octaves: u32, px: i32, py: i32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut f) = (0.0, 0.5, 1.0);
    for i in 0..octaves {
        let (fx, fy) = (px * f as i32, py * f as i32);
        sum += amp * noised_xy(x * f, y * f, fx, fy, seed.wrapping_add(i * 7919))[0];
        amp *= 0.5;
        f *= 2.0;
    }
    sum * 1.15
}

/// Ridged multifractal, about `[0, 1]`, with the same octave fade as [`fbm_lod`].
pub fn ridged_lod(x: f32, y: f32, octaves: u32, min_wl: f32, seed: u32) -> f32 {
    let (mut x, mut y) = (x, y);
    let (mut sum, mut amp, mut wl, mut weight, mut norm) = (0.0, 1.0, 1.0, 1.0f32, 0.0);
    for i in 0..octaves {
        let w = super::sstep(1.0, 2.0, wl / min_wl.max(1e-6));
        let n = 1.0 - n2(x, y, seed.wrapping_add(i * 977)).abs();
        let n = n * n * weight;
        weight = (n * 1.8).clamp(0.0, 1.0);
        sum += n * amp * w;
        norm += amp;
        amp *= 0.5;
        wl *= 0.5;
        (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
    }
    sum / norm
}

/// Erosion-like fractal sum: derivatives of the larger octaves damp the smaller ones, so
/// slopes get gullies and flats stay smooth. About `[-1, 1]`.
pub fn eroded(x: f32, y: f32, octaves: u32, min_wl: f32, seed: u32) -> f32 {
    let (mut x, mut y) = (x, y);
    let (mut sum, mut amp, mut wl) = (0.0, 0.5, 1.0);
    let (mut dx, mut dy) = (0.0, 0.0);
    for i in 0..octaves {
        let w = super::sstep(1.0, 2.0, wl / min_wl.max(1e-6));
        if w <= 0.0 {
            break;
        }
        let n = noised(x, y, 0, seed.wrapping_add(i * 131));
        dx += n[1] * amp;
        dy += n[2] * amp;
        sum += amp * w * n[0] / (1.0 + 2.0 * (dx * dx + dy * dy));
        amp *= 0.5;
        wl *= 0.5;
        (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
    }
    sum * 1.3
}

pub fn fbm3(x: f32, y: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut f) = (0.0, 0.5, 1.0);
    for i in 0..octaves {
        sum += amp
            * n3(
                x * f + i as f32 * 3.1,
                y * f,
                z * f,
                seed.wrapping_add(i * 31),
            );
        amp *= 0.5;
        f *= 2.03;
    }
    sum * 1.15
}

/// Tileable cellular noise: distances to the nearest and second nearest feature point
/// (in cell units) and the nearest cell's hash.
pub fn cells(x: f32, y: f32, period: i32, seed: u32) -> (f32, f32, u32) {
    cells_xy(x, y, period, period, seed)
}

/// [`cells`] tiling every `px` cells in x and `py` in y.
pub fn cells_xy(x: f32, y: f32, px: i32, py: i32, seed: u32) -> (f32, f32, u32) {
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2, mut id) = (9.0f32, 9.0f32, 0u32);
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (ix + ox, iy + oy);
            let h = hash(wrap(cx, px), wrap(cy, py), 7, seed);
            let fx = cx as f32 + (h & 0xffff) as f32 / 65536.0;
            let fy = cy as f32 + (h >> 16) as f32 / 65536.0;
            let d = ((fx - x).powi(2) + (fy - y).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = h;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}
