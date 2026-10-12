//! Procedural, tileable PBR textures for rock and moss: albedo, tangent-space normal and
//! occlusion/roughness maps, generated at startup on all cores.
//!
//! Rock is a weathered sandstone/limestone: broad bumps, flaking plates, a warped crack network,
//! pits, mineral grains and faint bedding lines. The albedo is near neutral (about 0.5 linear on
//! average) so the vertex colors of each rock carry its tint, moss, wetness and baked occlusion.
//! Moss is clumpy cushion moss whose alpha holds a coverage pattern: with an alpha-masked
//! material and per-vertex coverage in the vertex alpha, the moss layer ends in ragged patches.

use engine::prelude::*;

/// World size of one rock texture tile (m). Meshes set their UVs as position / this.
pub const ROCK_TILE_M: f32 = 2.0;
/// World size of one moss texture tile (m).
pub const MOSS_TILE_M: f32 = 0.6;

/// The generated images (RGBA8; albedo in sRGB, the rest linear).
pub struct RockImages {
    pub rock_albedo: Image,
    pub rock_normal: Image,
    /// R = ambient occlusion, G = roughness, B = metallic (0): serves as both the occlusion and
    /// the metallic/roughness texture.
    pub rock_orm: Image,
    pub moss_albedo: Image,
    pub moss_normal: Image,
}

/// Generate every rock and moss texture (`rock_size`² and `moss_size`² pixels, powers of two).
pub fn generate(rock_size: u32, moss_size: u32) -> RockImages {
    let (rock, moss) = std::thread::scope(|s| {
        let moss = s.spawn(|| moss_maps(moss_size));
        let rock = rock_maps(rock_size);
        (rock, moss.join().expect("moss texture thread panicked"))
    });
    RockImages {
        rock_albedo: rock.0,
        rock_normal: rock.1,
        rock_orm: rock.2,
        moss_albedo: moss.0,
        moss_normal: moss.1,
    }
}

// ---------------------------------------------------------------------------------------------
// Tileable noise on the unit square (u, v in [0, 1)). Periods are whole lattice cells per tile.

#[inline]
fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

#[inline]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

const GRADS: [(f32, f32); 16] = {
    let mut g = [(0.0, 0.0); 16];
    // cos/sin of k·22.5°, written out (no const trig).
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let c = [
        1.0, 0.92388, H, 0.38268, 0.0, -0.38268, -H, -0.92388, -1.0, -0.92388, -H, -0.38268, 0.0,
        0.38268, H, 0.92388,
    ];
    let mut k = 0;
    while k < 16 {
        g[k] = (c[k], c[(k + 12) % 16]);
        k += 1;
    }
    g
};

/// Gradient noise at lattice coordinates (x, y) that repeats every `period` cells, about
/// [-1, 1].
#[inline]
fn gnoise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    gnoise2(x, y, period, period, seed)
}

/// [`gnoise`] with separate periods along x and y.
#[inline]
fn gnoise2(x: f32, y: f32, px: i32, py: i32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (ix, iy) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let g = |i: i32, j: i32, dx: f32, dy: f32| {
        let h = hash(i.rem_euclid(px), j.rem_euclid(py), seed);
        let (gx, gy) = GRADS[(h >> 28) as usize];
        gx * dx + gy * dy
    };
    let (u, v) = (fade(fx), fade(fy));
    let a = g(ix, iy, fx, fy);
    let b = g(ix + 1, iy, fx - 1.0, fy);
    let c = g(ix, iy + 1, fx, fy - 1.0);
    let d = g(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    (ab + (cd - ab) * v) * 1.4
}

/// Tileable fractal noise over the unit square, about [-1, 1].
fn fbm(u: f32, v: f32, period: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut p) = (0.0, 1.0, 0.0, period);
    for k in 0..octaves {
        sum += amp * gnoise(u * p as f32, v * p as f32, p, seed.wrapping_add(k * 7919));
        norm += amp;
        amp *= gain;
        p *= 2;
    }
    sum / norm
}

/// Anisotropic [`fbm`]: `px` × `py` cells per tile at the first octave.
fn fbm2a(u: f32, v: f32, px: i32, py: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut k2) = (0.0, 1.0, 0.0, 1);
    for k in 0..octaves {
        let (a, b) = (px * k2, py * k2);
        sum += amp
            * gnoise2(
                u * a as f32,
                v * b as f32,
                a,
                b,
                seed.wrapping_add(k * 7919),
            );
        norm += amp;
        amp *= gain;
        k2 *= 2;
    }
    sum / norm
}

/// Thin lines along the zero crossings of a noise value (`width` in noise units).
fn contour(n: f32, width: f32) -> f32 {
    1.0 - smooth(0.0, width, n.abs())
}

/// Cell of a tileable jittered grid's nearest feature point: distance to the cell's border (in
/// cell units) and the cell's hash.
struct Cell {
    border: f32,
    id: u32,
}

fn voronoi(u: f32, v: f32, period: i32, jitter: f32, seed: u32) -> Cell {
    let (x, y) = (u * period as f32, v * period as f32);
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let point = |gx: i32, gy: i32| {
        let h = hash(gx.rem_euclid(period), gy.rem_euclid(period), seed);
        let h2 = hash(h as i32, 17, seed ^ 0x5bd1_e995);
        (
            gx as f32 + 0.5 + jitter * (unit(h) - 0.5) - x,
            gy as f32 + 0.5 + jitter * (unit(h2) - 0.5) - y,
            h,
        )
    };
    let (mut best, mut mr, mut id) = (f32::MAX, (0.0, 0.0), 0);
    let mut cell_of_best = (0, 0);
    for j in -1..=1 {
        for i in -1..=1 {
            let (dx, dy, h) = point(cx + i, cy + j);
            let d = dx * dx + dy * dy;
            if d < best {
                best = d;
                mr = (dx, dy);
                id = h;
                cell_of_best = (cx + i, cy + j);
            }
        }
    }
    // Distance to the border: the closest bisector between the nearest point and its
    // neighbors (the 3×3 around it is enough for jitter below 1).
    let mut border = f32::MAX;
    for j in -1..=1 {
        for i in -1..=1 {
            let (gx, gy) = (cell_of_best.0 + i, cell_of_best.1 + j);
            if (gx, gy) == cell_of_best {
                continue;
            }
            let (dx, dy, _) = point(gx, gy);
            let (ex, ey) = (dx - mr.0, dy - mr.1);
            let len = (ex * ex + ey * ey).sqrt();
            if len < 1e-6 {
                continue;
            }
            let d = ((mr.0 + dx) * 0.5 * ex + (mr.1 + dy) * 0.5 * ey) / len;
            border = border.min(d);
        }
    }
    Cell { border, id }
}

/// Offset from `(u, v)` to the nearest feature point of a tileable jittered grid (cell
/// units), and that cell's hash.
fn nearest(u: f32, v: f32, period: i32, jitter: f32, seed: u32) -> (f32, f32, u32) {
    let (x, y) = (u * period as f32, v * period as f32);
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let (mut best, mut out) = (f32::MAX, (0.0, 0.0, 0));
    for j in -1..=1 {
        for i in -1..=1 {
            let (gx, gy) = (cx + i, cy + j);
            let h = hash(gx.rem_euclid(period), gy.rem_euclid(period), seed);
            let h2 = hash(h as i32, 17, seed ^ 0x5bd1_e995);
            let dx = gx as f32 + 0.5 + jitter * (unit(h) - 0.5) - x;
            let dy = gy as f32 + 0.5 + jitter * (unit(h2) - 0.5) - y;
            let d = dx * dx + dy * dy;
            if d < best {
                best = d;
                out = (dx, dy, h);
            }
        }
    }
    out
}

/// Distance to the nearest feature point (cell units) and its cell's hash; cheaper than
/// [`voronoi`] when the border distance isn't needed.
fn voronoi_f1(u: f32, v: f32, period: i32, jitter: f32, seed: u32) -> (f32, u32) {
    let (dx, dy, id) = nearest(u, v, period, jitter, seed);
    ((dx * dx + dy * dy).sqrt(), id)
}

/// A tileable field of `K` smooth values sampled at low resolution and read back with
/// bilinear filtering (for masks and color variation that don't need every texel).
struct LowRes<const K: usize> {
    n: usize,
    data: Vec<[f32; K]>,
}

impl<const K: usize> LowRes<K> {
    fn new(n: usize, f: impl Fn(f32, f32) -> [f32; K] + Sync) -> Self {
        let mut data = vec![[0.0; K]; n * n];
        par_rows(&mut data, n, |y, row| {
            let v = y as f32 / n as f32;
            for (x, o) in row.iter_mut().enumerate() {
                *o = f(x as f32 / n as f32, v);
            }
        });
        Self { n, data }
    }

    fn at(&self, u: f32, v: f32) -> [f32; K] {
        let (x, y) = (u * self.n as f32, v * self.n as f32);
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let n = self.n as i64;
        let idx = |i: i64, j: i64| (j.rem_euclid(n) * n + i.rem_euclid(n)) as usize;
        let (i, j) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (
            self.data[idx(i, j)],
            self.data[idx(i + 1, j)],
            self.data[idx(i, j + 1)],
            self.data[idx(i + 1, j + 1)],
        );
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    engine::core::math::smoothstep(e0, e1, x)
}

fn to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn to_unorm(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Run `f(row, out_row)` for every row of an `n`-row image with `stride` values per row, on
/// all cores.
fn par_rows<T: Send>(data: &mut [T], stride: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len() / stride;
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    let band = rows.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (b, chunk) in data.chunks_mut(band * stride).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (k, row) in chunk.chunks_mut(stride).enumerate() {
                    f(b * band + k, row);
                }
            });
        }
    });
}

/// Tangent-space normal map (+Y = toward the top of the image) from a tileable height field in
/// texels (`strength` scales the slopes).
fn normal_map(height: &[f32], n: usize, strength: f32) -> Image {
    let mut out = vec![0u8; n * n * 4];
    let at = |x: isize, y: isize| {
        height[(y.rem_euclid(n as isize) as usize) * n + x.rem_euclid(n as isize) as usize]
    };
    par_rows(&mut out, n * 4, |y, row| {
        let y = y as isize;
        for x in 0..n as isize {
            // Sobel.
            let dx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let dy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
            let nrm = Vec3::new(-dx * strength / 8.0, dy * strength / 8.0, 1.0).normalize();
            let i = x as usize * 4;
            row[i] = to_unorm(nrm.x * 0.5 + 0.5);
            row[i + 1] = to_unorm(nrm.y * 0.5 + 0.5);
            row[i + 2] = to_unorm(nrm.z * 0.5 + 0.5);
            row[i + 3] = 255;
        }
    });
    Image::new(n as u32, n as u32, out, false)
}

/// Separable wrap-around box blur (radius `r` texels).
fn blur(src: &[f32], n: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; n * n];
    let mut out = vec![0.0f32; n * n];
    let w = 1.0 / (2 * r + 1) as f32;
    par_rows(&mut tmp, n, |y, row| {
        let line = &src[y * n..y * n + n];
        let mut acc: f32 = (0..=2 * r).map(|k| line[(k + n - r) % n]).sum();
        for (x, o) in row.iter_mut().enumerate() {
            *o = acc * w;
            acc += line[(x + r + 1) % n] - line[(x + n - r) % n];
        }
    });
    par_rows(&mut out, n, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = 0.0;
            for k in 0..=2 * r {
                acc += tmp[((y + n + k - r) % n) * n + x];
            }
            *o = acc * w;
        }
    });
    out
}

// ---------------------------------------------------------------------------------------------
// Rock.

/// Low-frequency rock fields, computed at a quarter of the resolution.
const LO_BROAD: usize = 0;
const LO_FLAKE: usize = 1;
const LO_WARP_U: usize = 2;
const LO_WARP_V: usize = 3;
const LO_BED_MASK: usize = 4;
const LO_JOINT_MASK: usize = 5;
const LO_HAIR_MASK: usize = 6;
const LO_TONE: usize = 7;
const LO_STAIN: usize = 8;
const LO_MOTTLE: usize = 9;
const LO_BED_WOBBLE: usize = 10;
const LO_ROUGH: usize = 11;

fn rock_low(u: f32, v: f32) -> [f32; 12] {
    let mut f = [0.0; 12];
    // The first three octaves of the broad bumps (the rest are added at full resolution).
    f[LO_BROAD] = gnoise(u * 4.0, v * 4.0, 4, 11)
        + 0.5 * gnoise(u * 8.0, v * 8.0, 8, 11 + 7919)
        + 0.25 * gnoise(u * 16.0, v * 16.0, 16, 11 + 2 * 7919);
    f[LO_FLAKE] = smooth(0.1, 0.5, fbm(u, v, 3, 3, 0.5, 12));
    f[LO_WARP_U] = 0.012 * fbm(u, v, 6, 2, 0.5, 15);
    f[LO_WARP_V] = 0.012 * fbm(u + 0.37, v + 0.11, 6, 2, 0.5, 16);
    f[LO_BED_MASK] = smooth(0.0, 0.3, fbm(u, v, 3, 2, 0.5, 18));
    f[LO_JOINT_MASK] = smooth(0.05, 0.35, fbm(u, v, 4, 2, 0.5, 20));
    f[LO_HAIR_MASK] = smooth(-0.1, 0.4, fbm(u, v, 5, 2, 0.5, 30));
    f[LO_TONE] = fbm(u, v, 2, 4, 0.55, 25);
    f[LO_STAIN] = smooth(0.25, 0.75, fbm(u, v, 3, 4, 0.5, 26));
    f[LO_MOTTLE] = 0.5 + 0.5 * fbm(u, v, 9, 3, 0.5, 27);
    f[LO_BED_WOBBLE] = 0.015 * fbm(u, v, 4, 2, 0.5, 24);
    f[LO_ROUGH] = fbm(u, v, 16, 2, 0.5, 29);
    f
}

/// Per-texel rock features (height in mm, plus the masks the color and roughness use).
#[derive(Clone, Copy, Default)]
struct RockTexel {
    height: f32,
    crack: f32,
    pit: f32,
    plate: f32,
    grain: f32,
    speck: f32,
    /// Random value of the chip the texel is on (0–1).
    cell: f32,
}

/// A small tilted plane per cell of a jittered grid (height in cell units, about -1..1), so
/// the surface reads as broken grains and chipped facets rather than smooth plaster; also the
/// cell's own random value (0–1).
fn facets(u: f32, v: f32, period: i32, seed: u32) -> (f32, f32) {
    let (dx, dy, id) = nearest(u, v, period, 0.9, seed);
    let a = unit(id.rotate_left(5)) * 2.0 - 1.0;
    let b = unit(id.rotate_left(13)) * 2.0 - 1.0;
    (a * dx + b * dy + 0.3 * (unit(id) - 0.5), unit(id))
}

fn rock_texel(u: f32, v: f32, lo: &[f32; 12]) -> RockTexel {
    // Broad weathered bumps (mm): low octaves from the quarter-resolution field, plus sharp
    // ridges where the weathering has etched the softer grains out.
    let fine = 0.125 * gnoise(u * 32.0, v * 32.0, 32, 11 + 3 * 7919)
        + 0.0625 * gnoise(u * 64.0, v * 64.0, 64, 11 + 4 * 7919);
    let mut h = (lo[LO_BROAD] + fine) / 1.94 * 12.0;
    let ridge = 1.0 - gnoise(u * 12.0, v * 12.0, 12, 33).abs() * 1.6;
    h += ridge.max(0.0).powi(3) * 4.0;
    // Chipped facets at two scales.
    let (big, cell) = facets(u, v, 22, 31);
    h += big * 3.2 + facets(u, v, 60, 32).0 * 0.9;
    // Flaking scales with soft rims, only in patches where the rock is exfoliating; hairline
    // cracks run along their edges.
    let pc = voronoi(u + lo[LO_WARP_V], v + lo[LO_WARP_U], 9, 0.85, 14);
    let plate = lo[LO_FLAKE] * smooth(0.0, 0.16, pc.border);
    h += plate * (0.6 + 1.8 * unit(pc.id));
    // A fine jitter makes every crack edge jagged.
    let ju = 0.0025 * gnoise(u * 48.0, v * 48.0, 48, 34);
    let jv = 0.0025 * gnoise(u * 48.0 + 0.5, v * 48.0, 48, 35);
    let (wu, wv) = (u + lo[LO_WARP_U] + ju, v + lo[LO_WARP_V] + jv);
    let hair = (1.0 - smooth(0.0, 0.012, pc.border)) * lo[LO_HAIR_MASK] * 0.55;
    // Joints: a sparse polygonal fracture network with straight runs and sharp corners, fading
    // in and out along its length.
    let jc = voronoi(wu, wv, 3, 1.0, 19);
    let joints = (1.0 - smooth(0.0, 0.0065, jc.border)) * lo[LO_JOINT_MASK];
    // Bedding partings: long, nearly level lines (horizontal on vertical faces).
    let bedding = contour(fbm2a(wu, wv, 1, 4, 2, 0.35, 17), 0.022) * lo[LO_BED_MASK];
    let crack = bedding.max(joints).max(hair);
    h -= crack * 7.0;
    // Solution pits, sparse.
    let (pf, pid) = voronoi_f1(u, v, 72, 0.8, 22);
    let pit_r = 0.12 + 0.2 * unit(pid.rotate_left(7));
    let pit = if unit(pid) < 0.1 {
        1.0 - smooth(pit_r * 0.4, pit_r, pf)
    } else {
        0.0
    };
    h -= pit * 2.0;
    // Grain and faint bedding lines (horizontal on vertical faces).
    let grain = 0.67 * gnoise(u * 128.0, v * 128.0, 128, 23)
        + 0.33 * gnoise(u * 256.0, v * 256.0, 256, 23 + 7919);
    h += grain * 0.6;
    let bed = ((v + lo[LO_BED_WOBBLE]) * 7.0 * std::f32::consts::TAU).sin();
    h -= smooth(0.82, 1.0, bed) * 1.2;
    // Mineral grains: dark specks and a few bright ones (-1..1).
    let (gf, gid) = voronoi_f1(u, v, 200, 0.9, 28);
    let r = 0.2 + 0.18 * unit(gid.rotate_left(11));
    let dot = 1.0 - smooth(r * 0.5, r, gf);
    let kind = unit(gid);
    let speck = if kind < 0.14 {
        -dot
    } else if kind > 0.93 {
        dot
    } else {
        0.0
    };
    RockTexel {
        height: h,
        crack,
        pit,
        plate,
        grain,
        speck,
        cell,
    }
}

fn rock_maps(size: u32) -> (Image, Image, Image) {
    let n = size as usize;
    let lo = LowRes::new((n / 4).max(8), rock_low);
    let mut texels = vec![RockTexel::default(); n * n];
    par_rows(&mut texels, n, |y, row| {
        let v = y as f32 / n as f32;
        for (x, t) in row.iter_mut().enumerate() {
            let u = x as f32 / n as f32;
            *t = rock_texel(u, v, &lo.at(u, v));
        }
    });
    let height: Vec<f32> = texels.iter().map(|t| t.height).collect();
    let cavity_src = blur(&height, n, 6);
    let wide = blur(&height, n, 20);
    let mean = height.iter().sum::<f32>() / height.len() as f32;

    // Albedo, occlusion and roughness.
    let mut albedo = vec![0u8; n * n * 4];
    let mut orm = vec![0u8; n * n * 4];
    {
        let mut rows: Vec<(&mut [u8], &mut [u8])> = albedo
            .chunks_mut(n * 4)
            .zip(orm.chunks_mut(n * 4))
            .collect();
        par_rows(&mut rows, 1, |y, row| {
            let (arow, orow) = &mut row[0];
            let v = y as f32 / n as f32;
            for x in 0..n {
                let u = x as f32 / n as f32;
                let l = lo.at(u, v);
                let t = texels[y * n + x];
                let warm = Vec3::new(0.60, 0.545, 0.47);
                let cool = Vec3::new(0.50, 0.505, 0.505);
                let mut c = warm.lerp(cool, smooth(-0.5, 0.5, l[LO_TONE]));
                c *= 0.86 + 0.28 * l[LO_MOTTLE];
                // Raised spots are lighter (dusty, sun-bleached), hollows darker.
                let relief = (t.height - wide[y * n + x]) / 10.0;
                c *= (1.0 + 0.35 * relief).clamp(0.6, 1.3);
                c *= 1.0 - 0.5 * t.crack;
                c *= 1.0 - 0.3 * t.pit;
                c *= 1.0 + 0.08 * t.plate;
                // Iron staining.
                c *= Vec3::ONE.lerp(Vec3::new(1.0, 0.84, 0.66), l[LO_STAIN] * 0.45);
                c *= 1.0 + if t.speck < 0.0 { 0.4 } else { 0.3 } * t.speck;
                c *= 1.0 + 0.06 * t.grain;
                // Each chip a slightly different shade, so the rock stays mottled from afar.
                c *= 0.9 + 0.2 * t.cell;
                let i = x * 4;
                arow[i] = to_srgb(c.x);
                arow[i + 1] = to_srgb(c.y);
                arow[i + 2] = to_srgb(c.z);
                arow[i + 3] = 255;
                let cav = (cavity_src[y * n + x] - t.height).max(0.0);
                let ao = (1.0 - cav * 0.09 - t.crack * 0.35).clamp(0.25, 1.0);
                let raised = (t.height - mean) / 14.0;
                let rough =
                    0.86 + 0.08 * l[LO_ROUGH] - 0.1 * t.plate - 0.08 * raised + 0.1 * t.crack;
                orow[i] = to_unorm(ao);
                orow[i + 1] = to_unorm(rough.clamp(0.45, 1.0));
                orow[i + 2] = 0;
                orow[i + 3] = 255;
            }
        });
    }

    // Height is in mm and a texel is ROCK_TILE_M / size, so slopes come out right.
    let texel_mm = ROCK_TILE_M * 1000.0 / size as f32;
    (
        Image::new(size, size, albedo, true),
        normal_map(&height, n, 1.0 / texel_mm),
        Image::new(size, size, orm, false),
    )
}

// ---------------------------------------------------------------------------------------------
// Moss.

fn moss_maps(size: u32) -> (Image, Image) {
    let n = size as usize;
    let lo = LowRes::new((n / 4).max(8), |u, v| {
        [
            0.5 + 0.5 * fbm(u, v, 5, 4, 0.55, 42),
            smooth(-0.4, 0.5, fbm(u, v, 3, 4, 0.5, 44)),
            smooth(0.3, 0.8, fbm(u, v, 4, 3, 0.5, 45)) * 0.6,
            smooth(
                -0.45,
                0.45,
                fbm(u + 0.05 * fbm(u, v, 6, 2, 0.5, 46), v, 4, 4, 0.55, 47),
            ),
        ]
    });
    // Cushions of moss made of small domed tufts.
    let mut cells = vec![(0.0f32, 0.0f32); n * n];
    par_rows(&mut cells, n, |y, row| {
        let v = y as f32 / n as f32;
        for (x, o) in row.iter_mut().enumerate() {
            let u = x as f32 / n as f32;
            let (f1, id) = voronoi_f1(u, v, 40, 0.9, 41);
            let dome = 1.0 - (f1 * 1.1).min(1.0).powi(2);
            let fine = 0.67 * gnoise(u * 96.0, v * 96.0, 96, 43)
                + 0.33 * gnoise(u * 192.0, v * 192.0, 192, 43 + 7919);
            let cushion = lo.at(u, v)[0];
            *o = (
                cushion * 2.5 + dome * 1.2 * (0.6 + 0.4 * unit(id)) + fine * 0.35,
                unit(id),
            );
        }
    });
    let mut albedo = vec![0u8; n * n * 4];
    par_rows(&mut albedo, n * 4, |y, row| {
        let v = y as f32 / n as f32;
        for x in 0..n {
            let u = x as f32 / n as f32;
            let (h, tuft) = cells[y * n + x];
            let l = lo.at(u, v);
            let dark = Vec3::new(0.16, 0.24, 0.05);
            let lush = Vec3::new(0.34, 0.46, 0.08);
            let yellow = Vec3::new(0.55, 0.55, 0.14);
            let mut c = dark.lerp(lush, l[1]);
            c = c.lerp(yellow, l[2]);
            c *= 0.8 + 0.4 * tuft;
            c *= (0.55 + 0.18 * h).clamp(0.45, 1.25);
            // Coverage pattern for ragged edges: alpha 0.5 (always shown when the vertex says
            // full coverage) to 1.
            let alpha = 0.5 + 0.5 * l[3] * (0.75 + 0.25 * smooth(0.0, 3.0, h));
            let i = x * 4;
            row[i] = to_srgb(c.x);
            row[i + 1] = to_srgb(c.y);
            row[i + 2] = to_srgb(c.z);
            row[i + 3] = to_unorm(alpha);
        }
    });
    let height: Vec<f32> = cells.iter().map(|c| c.0).collect();
    let texel_mm = MOSS_TILE_M * 1000.0 / size as f32;
    (
        Image::new(size, size, albedo, true),
        normal_map(&height, n, 0.6 / texel_mm),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_seamlessly() {
        for k in 0..20 {
            let v = k as f32 / 20.0;
            let a = fbm(0.0, v, 4, 5, 0.5, 3);
            let b = fbm(1.0, v, 4, 5, 0.5, 3);
            assert!((a - b).abs() < 1e-4, "fbm does not wrap: {a} vs {b}");
            let c = voronoi(0.0, v, 7, 0.9, 5);
            let d = voronoi(1.0, v, 7, 0.9, 5);
            assert_eq!(c.id, d.id);
            assert!((c.border - d.border).abs() < 1e-3);
        }
    }

    #[test]
    fn small_textures_have_the_right_sizes() {
        let t = generate(64, 32);
        assert_eq!(t.rock_albedo.data.len(), 64 * 64 * 4);
        assert_eq!(t.rock_normal.data.len(), 64 * 64 * 4);
        assert_eq!(t.rock_orm.data.len(), 64 * 64 * 4);
        assert_eq!(t.moss_albedo.data.len(), 32 * 32 * 4);
        assert!(t.rock_albedo.srgb && !t.rock_normal.srgb);
        // Moss alpha never drops below the mask cutoff's half, so full coverage is solid.
        assert!(t.moss_albedo.data.chunks(4).all(|p| p[3] >= 127));
    }
}
