//! Procedural textures for the fire demo's props and ground: bark, weathered planks, rusty
//! corrugated metal, straw, granite, leaves, split firewood and five ground layers.
//!
//! Everything is generated once in code (no downloaded art): tileable gradient and cellular
//! noise shaped into height fields, which give the normal maps, ambient occlusion and much of
//! the albedo and roughness. Each generator returns a [`TexSet`] in the engine's material
//! conventions: sRGB albedo (alpha = coverage for cut-out leaves, or height for ground
//! layers), a tangent-space normal map (+Y up, glTF), and an ORM map (R occlusion,
//! G roughness, B metallic) that serves as both `occlusion_texture` and
//! `metallic_roughness_texture`. Sizes are 1K–2K; generation runs on all cores and takes
//! well under a second per set in release builds.

use std::f32::consts::FRAC_1_SQRT_2;

use engine_core::math::{Vec2, Vec3};
use engine_render::Image;

// ---------------------------------------------------------------------------------- noise

/// 32-bit integer hash of a lattice point and a seed.
#[inline]
pub fn hash2(x: i32, y: i32, seed: u32) -> u32 {
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
pub fn hash01(x: i32, y: i32, seed: u32) -> f32 {
    (hash2(x, y, seed) >> 8) as f32 / 16_777_216.0
}

const GRADIENTS: [(f32, f32); 16] = [
    (1.0, 0.0),
    (0.923_88, 0.382_68),
    (FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (0.382_68, 0.923_88),
    (0.0, 1.0),
    (-0.382_68, 0.923_88),
    (-FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (-0.923_88, 0.382_68),
    (-1.0, 0.0),
    (-0.923_88, -0.382_68),
    (-FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (-0.382_68, -0.923_88),
    (0.0, -1.0),
    (0.382_68, -0.923_88),
    (FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (0.923_88, -0.382_68),
];

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Gradient noise in about `[-1, 1]` that repeats every `px` × `py` lattice cells (`x`, `y`
/// are in cells). Periods ≤ 0 mean "doesn't repeat" on that axis.
pub fn noise(x: f32, y: f32, px: i32, py: i32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (ix, iy) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let wrap = |v: i32, p: i32| if p > 0 { v.rem_euclid(p) } else { v };
    let (x0, x1) = (wrap(ix, px), wrap(ix + 1, px));
    let (y0, y1) = (wrap(iy, py), wrap(iy + 1, py));
    let g = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let (gx, gy) = GRADIENTS[(hash2(cx, cy, seed) & 15) as usize];
        gx * dx + gy * dy
    };
    let n00 = g(x0, y0, fx, fy);
    let n10 = g(x1, y0, fx - 1.0, fy);
    let n01 = g(x0, y1, fx, fy - 1.0);
    let n11 = g(x1, y1, fx - 1.0, fy - 1.0);
    let (u, v) = (fade(fx), fade(fy));
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 1.414
}

/// Tileable fractal noise in about `[-1, 1]`: `octaves` layers of [`noise`], each twice the
/// frequency (and period) and `gain` times the amplitude of the last.
pub fn fbm(x: f32, y: f32, px: i32, py: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for i in 0..octaves {
        let m = 1 << i;
        sum += amp * noise(x * f, y * f, px * m, py * m, seed.wrapping_add(i * 7919));
        norm += amp;
        amp *= gain;
        f *= 2.0;
    }
    sum / norm
}

/// Ridged fractal noise in `[0, 1]` (sharp creases where the noise crosses zero).
pub fn ridged(x: f32, y: f32, px: i32, py: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for i in 0..octaves {
        let m = 1 << i;
        let n = 1.0 - noise(x * f, y * f, px * m, py * m, seed.wrapping_add(i * 104_729)).abs();
        sum += amp * n * n;
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

/// Nearest feature points of tileable cellular (Worley) noise.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    /// Distance to the nearest feature point, in cells.
    pub f1: f32,
    /// Distance to the second nearest.
    pub f2: f32,
    /// Random id of the nearest cell.
    pub id: u32,
    /// Offset from the nearest feature point to the sample, in cells.
    pub offset: Vec2,
}

/// Tileable cellular noise with one jittered feature point per lattice cell (`jitter` 0..1).
pub fn cells(x: f32, y: f32, px: i32, py: i32, jitter: f32, seed: u32) -> Cell {
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let mut best = Cell {
        f1: f32::MAX,
        f2: f32::MAX,
        id: 0,
        offset: Vec2::ZERO,
    };
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (ix + dx, iy + dy);
            let wrap = |v: i32, p: i32| if p > 0 { v.rem_euclid(p) } else { v };
            let (wx, wy) = (wrap(cx, px), wrap(cy, py));
            let h = hash2(wx, wy, seed);
            let jx = 0.5 + (hash01(wx, wy, seed ^ 0x51ed) - 0.5) * jitter;
            let jy = 0.5 + (hash01(wx, wy, seed ^ 0xa3c5) - 0.5) * jitter;
            let o = Vec2::new(x - (cx as f32 + jx), y - (cy as f32 + jy));
            let d = o.length();
            if d < best.f1 {
                best.f2 = best.f1;
                best.f1 = d;
                best.id = h;
                best.offset = o;
            } else if d < best.f2 {
                best.f2 = d;
            }
        }
    }
    best
}

#[inline]
pub fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// sRGB hex to linear RGB.
pub fn rgb(hex: u32) -> Vec3 {
    Vec3::from(engine_render::material::srgb_hex_to_linear(hex))
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ---------------------------------------------------------------------------------- texels

/// What a generator computes for one texel.
#[derive(Debug, Clone, Copy)]
pub struct Texel {
    /// Linear albedo.
    pub albedo: Vec3,
    /// Alpha (coverage for cut-outs; ground layers store height here instead).
    pub alpha: f32,
    /// Height in meters (drives the normal map).
    pub height: f32,
    pub roughness: f32,
    pub metallic: f32,
    /// Ambient occlusion 0..1 (multiplied with the occlusion derived from the height).
    pub ao: f32,
    /// Emission mask 0..1 (glowing cracks).
    pub emissive: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            albedo: Vec3::splat(0.5),
            alpha: 1.0,
            height: 0.0,
            roughness: 0.8,
            metallic: 0.0,
            ao: 1.0,
            emissive: 0.0,
        }
    }
}

/// A material's textures.
pub struct TexSet {
    pub albedo: Image,
    pub normal: Image,
    /// R occlusion, G roughness, B metallic.
    pub orm: Image,
    pub emissive: Option<Image>,
}

/// Run `f(x, y)` for every texel of a `w`×`h` image on all cores, rows top to bottom.
pub fn par_texels<T: Send + Copy + Default>(
    w: u32,
    h: u32,
    f: impl Fn(u32, u32) -> T + Sync,
) -> Vec<T> {
    let mut out = vec![T::default(); (w * h) as usize];
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let rows = (h as usize).div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (i, chunk) in out.chunks_mut(rows * w as usize).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (j, t) in chunk.iter_mut().enumerate() {
                    let y = (i * rows + j / w as usize) as u32;
                    *t = f(j as u32 % w, y);
                }
            });
        }
    });
    out
}

/// Bake texels into images. `texel_m` is the size of a texel in meters along x and y (for
/// normal-map slopes); `cavity` darkens AO where the height dips below its surroundings.
pub fn bake(w: u32, h: u32, texels: &[Texel], texel_m: [f32; 2], cavity: f32) -> TexSet {
    let height = |x: i32, y: i32| {
        texels[(y.rem_euclid(h as i32) as u32 * w + x.rem_euclid(w as i32) as u32) as usize].height
    };
    let normals = par_texels(w, h, |x, y| {
        let (x, y) = (x as i32, y as i32);
        let dx = (height(x + 1, y) - height(x - 1, y)) / (2.0 * texel_m[0]);
        let dy = (height(x, y + 1) - height(x, y - 1)) / (2.0 * texel_m[1]);
        // Tangent space: +X along u, +Y toward decreasing v (image up).
        let n = Vec3::new(-dx, dy, 1.0).normalize();
        // Cavity: how far below the 8-neighborhood average (5 texels out) the texel is.
        let r = 4;
        let avg = (height(x - r, y)
            + height(x + r, y)
            + height(x, y - r)
            + height(x, y + r)
            + height(x - r, y - r)
            + height(x + r, y + r)
            + height(x - r, y + r)
            + height(x + r, y - r))
            / 8.0;
        let dip = (avg - height(x, y)) / (texel_m[0] * r as f32);
        let ao = (1.0 - dip * cavity).clamp(0.25, 1.0);
        [n.x, n.y, n.z, ao]
    });
    let mut albedo = Vec::with_capacity(texels.len() * 4);
    let mut normal = Vec::with_capacity(texels.len() * 4);
    let mut orm = Vec::with_capacity(texels.len() * 4);
    let mut emissive = Vec::new();
    let has_emissive = texels.iter().any(|t| t.emissive > 0.0);
    for (t, n) in texels.iter().zip(&normals) {
        albedo.extend_from_slice(&[
            linear_to_srgb(t.albedo.x),
            linear_to_srgb(t.albedo.y),
            linear_to_srgb(t.albedo.z),
            unorm(t.alpha),
        ]);
        normal.extend_from_slice(&[
            unorm(n[0] * 0.5 + 0.5),
            unorm(n[1] * 0.5 + 0.5),
            unorm(n[2] * 0.5 + 0.5),
            255,
        ]);
        orm.extend_from_slice(&[
            unorm(t.ao * n[3]),
            unorm(t.roughness),
            unorm(t.metallic),
            255,
        ]);
        if has_emissive {
            let e = linear_to_srgb(t.emissive);
            emissive.extend_from_slice(&[e, e, e, 255]);
        }
    }
    TexSet {
        albedo: Image::new(w, h, albedo, true),
        normal: Image::new(w, h, normal, false),
        orm: Image::new(w, h, orm, false),
        emissive: has_emissive.then(|| Image::new(w, h, emissive, true)),
    }
}

/// Mips for a cut-out texture that keep each level's alpha coverage at `cutoff` (alpha-tested
/// leaves otherwise thin out into nothing in the distance).
pub fn coverage_preserving_mips(image: &mut Image, cutoff: f32) {
    let coverage = |data: &[u8], scale: f32| {
        let n = data.len() / 4;
        let mut c = 0usize;
        for px in data.as_chunks::<4>().0 {
            if px[3] as f32 / 255.0 * scale >= cutoff {
                c += 1;
            }
        }
        c as f32 / n.max(1) as f32
    };
    let target = coverage(&image.data, 1.0);
    let mut mips = image.mip_chain(true);
    for level in &mut mips {
        // Binary search the alpha scale that restores the coverage.
        let (mut lo, mut hi) = (0.5f32, 4.0f32);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if coverage(level, mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        for px in level.as_chunks_mut::<4>().0 {
            px[3] = unorm(px[3] as f32 / 255.0 * hi);
        }
    }
    image.mips = mips;
}

// ---------------------------------------------------------------------------------- bark

/// Furrowed bark (an old oak or ash): vertical plates split by deep fissures, pale weathered
/// ridges, lichen. One tile covers 1 × 1 m (u around the trunk, v along it).
pub fn bark(size: u32, seed: u32) -> TexSet {
    let s = size as f32;
    let texels = par_texels(size, size, |x, y| {
        let (u, v) = (x as f32 / s, y as f32 / s);
        // Plates: cells stretched 4:1 along the trunk, broken by ridged fissures.
        let warp = fbm(u * 6.0, v * 3.0, 6, 3, 3, 0.5, seed) * 0.35;
        let c = cells(u * 14.0 + warp, v * 4.0, 14, 4, 0.9, seed ^ 0x77);
        let edge = (c.f2 - c.f1).min(1.0);
        let plate = smooth(0.0, 0.35, edge);
        let fissure = ridged(u * 18.0 + warp * 2.0, v * 3.0, 18, 3, 3, seed ^ 0x99);
        let fiber = fbm(u * 120.0, v * 12.0, 120, 12, 3, 0.55, seed ^ 0x1234);
        let crack = smooth(
            0.82,
            0.97,
            ridged(u * 40.0, v * 9.0, 40, 9, 2, seed ^ 0x4321),
        );
        let plate_id = (c.id >> 8) as f32 / 16_777_216.0;
        let height = 0.012 * plate * (0.6 + 0.4 * plate_id) - 0.008 * fissure.powi(3)
            + 0.0012 * fiber
            - 0.003 * crack;
        let depth = (1.0 - plate).max(fissure.powi(3));
        // Color: grey-brown plates, pale weathered tops, dark fissures, lichen on the tops.
        let base = mix3(rgb(0x4a3d33), rgb(0x6b6156), plate_id * 0.6 + 0.2 * fiber);
        let mut a = mix3(base, rgb(0x1c1511), depth.powf(0.8) * 0.85);
        a = mix3(
            a,
            rgb(0x8a8378),
            plate * smooth(0.2, 0.8, fiber * 0.5 + 0.5) * 0.25,
        );
        let lichen_n = fbm(u * 5.0, v * 5.0, 5, 5, 4, 0.55, seed ^ 0x5151);
        let lichen = smooth(0.25, 0.4, lichen_n) * plate * (0.6 + 0.4 * fiber);
        a = mix3(a, rgb(0x9da383), lichen * 0.7);
        let moss = smooth(
            0.35,
            0.55,
            fbm(u * 3.0, v * 6.0, 3, 6, 3, 0.5, seed ^ 0x3131),
        ) * depth
            * 0.6;
        a = mix3(a, rgb(0x3d4a22), moss);
        // Emission mask, seen only once the bark chars: the fissures and an alligator
        // pattern of cracks glow.
        let blocks = cells(u * 16.0, v * 7.0, 16, 7, 0.85, seed ^ 0xA11);
        let alligator = 1.0 - smooth(0.02, 0.1, blocks.f2 - blocks.f1);
        Texel {
            albedo: a,
            height,
            roughness: 0.82 + 0.15 * depth - 0.1 * lichen,
            ao: 1.0 - 0.45 * depth,
            emissive: (alligator * 0.8).max(smooth(0.55, 0.95, depth)),
            ..Default::default()
        }
    });
    bake(size, size, &texels, [1.0 / s, 1.0 / s], 3.0)
}

// ---------------------------------------------------------------------------------- planks

/// Weathered barn boards: silver-grey sun-bleached grain with warm brown in the grooves,
/// raised late wood, knots, checks. One tile is one board 0.2 m wide (u) and 2 m long (v).
pub fn planks(w: u32, h: u32, seed: u32) -> TexSet {
    let (wm, hm) = (0.2f32, 2.0f32);
    let texels = par_texels(w, h, |x, y| {
        let (px, py) = (x as f32 / w as f32 * wm, y as f32 / h as f32 * hm);
        // Knots: two per board, grain flows around them.
        let mut deflect = 0.0f32;
        let mut knot = 0.0f32;
        for k in 0..2 {
            let kx = (0.25 + 0.5 * hash01(k, 1, seed)) * wm;
            let ky = (k as f32 + hash01(k, 2, seed)) * hm * 0.5;
            let r = 0.008 + 0.012 * hash01(k, 3, seed);
            let mut d = Vec2::new(px - kx, py - ky);
            // Shortest offset on the tiling board.
            d.x -= (d.x / wm).round() * wm;
            d.y -= (d.y / hm).round() * hm;
            let e = Vec2::new(d.x, d.y * 0.45).length();
            knot = knot.max(1.0 - smooth(r * 0.6, r, e));
            deflect += d.x.signum() * (r * 3.0) * (-(e / (r * 3.0)).powi(2)).exp();
        }
        // Grain: rings seen edge-on, wavy along the board.
        let wave = fbm(px * 10.0, py * 1.5, 2, 3, 3, 0.5, seed ^ 0x11) * 0.012;
        let ring_x = (px + wave + deflect) * 160.0;
        let ring = (ring_x * std::f32::consts::TAU / 4.0).sin() * 0.5 + 0.5;
        let late = smooth(0.55, 0.95, ring);
        let streak = fbm(px * 90.0, py * 3.0, 18, 6, 4, 0.6, seed ^ 0x22);
        let check = smooth(
            0.9,
            0.985,
            ridged(px * 40.0, py * 2.0, 8, 4, 2, seed ^ 0x33),
        );
        let height =
            0.0009 * late + 0.0004 * streak - 0.0015 * check - 0.0008 * knot + 0.0003 * knot * ring;
        let bleach = smooth(-0.3, 0.6, fbm(px * 5.0, py, 1, 2, 3, 0.5, seed ^ 0x44));
        let mut a = mix3(rgb(0x5b4636), rgb(0x8e877c), bleach * 0.75 + 0.2 * late);
        a = mix3(a, rgb(0x3a2a1e), (1.0 - late) * 0.35);
        a *= 0.9 + 0.2 * streak;
        a = mix3(a, rgb(0x2a1d14), knot * 0.85);
        a = mix3(a, rgb(0x16100c), check * 0.9);
        // Emission mask for when the boards char: checks and alligator cracks glow.
        let blocks = cells(px * 30.0, py * 14.0, 6, 28, 0.85, seed ^ 0xA11);
        let alligator = 1.0 - smooth(0.02, 0.1, blocks.f2 - blocks.f1);
        Texel {
            albedo: a,
            height,
            roughness: 0.78 + 0.15 * (1.0 - late) + 0.05 * check,
            ao: 1.0 - 0.5 * check,
            emissive: (alligator * 0.8).max(check),
            ..Default::default()
        }
    });
    bake(w, h, &texels, [wm / w as f32, hm / h as f32], 1.5)
}

// ---------------------------------------------------------------------------------- metal

/// Galvanized sheet steel gone patchy with rust: spangle, rust blooms and streaks that run
/// down the sheet, small dents. One tile is 1 × 1 m; v runs down the slope.
pub fn rusty_metal(size: u32, seed: u32) -> TexSet {
    let s = size as f32;
    let texels = par_texels(size, size, |x, y| {
        let (u, v) = (x as f32 / s, y as f32 / s);
        let spangle = cells(u * 40.0, v * 40.0, 40, 40, 1.0, seed);
        let sp = (spangle.id >> 8) as f32 / 16_777_216.0;
        let rust_n = fbm(u * 4.0, v * 4.0, 4, 4, 5, 0.55, seed ^ 0x10);
        // Streaks: rust washed downhill (stretched along v).
        let streak = fbm(u * 30.0, v * 1.0, 30, 1, 3, 0.5, seed ^ 0x20) * 0.5 + 0.5;
        let rust = smooth(0.05, 0.35, rust_n + 0.25 * streak - 0.1);
        let pitting = smooth(
            0.6,
            0.9,
            fbm(u * 60.0, v * 60.0, 60, 60, 2, 0.5, seed ^ 0x30),
        );
        let dent = fbm(u * 3.0, v * 3.0, 3, 3, 2, 0.5, seed ^ 0x40);
        let metal = mix3(rgb(0x8d9093), rgb(0xa9acaf), sp) * (0.85 + 0.1 * dent);
        let rust_c = mix3(rgb(0x6b3418), rgb(0x9a5a2c), smooth(-0.4, 0.6, rust_n));
        let dirt = smooth(0.2, 0.8, streak) * 0.25;
        let mut a = mix3(metal, rust_c, rust);
        a = mix3(a, rgb(0x3a2e24), dirt * (1.0 - rust * 0.5));
        Texel {
            albedo: a,
            height: 0.0006 * dent + 0.00025 * rust * pitting,
            roughness: 0.35 + 0.15 * sp + 0.5 * rust + 0.1 * dirt,
            metallic: (1.0 - rust) * (1.0 - dirt),
            ao: 1.0 - 0.2 * pitting * rust,
            ..Default::default()
        }
    });
    bake(size, size, &texels, [1.0 / s, 1.0 / s], 1.0)
}

// ---------------------------------------------------------------------------------- straw

/// Cured hay is warmer than fresh straw, and stays straw-colored (not olive) in blue shade.
const STRAW_WARM: Vec3 = Vec3::new(1.08, 0.95, 0.8);

/// Hay-bale atlas, 2:1. Left half: the wrapped side (stalks running around the bale, u
/// around, v across). Right half: the rolled end, a spiral of compressed layers in the disc of
/// radius 0.5 around (0.75, 0.5); the corner at u > 0.97, v < 0.03 is plain white for twine.
pub fn straw(size: u32, seed: u32) -> TexSet {
    let (w, h) = (size * 2, size);
    let s = size as f32;
    let stalk_colors = [
        rgb(0xcca556),
        rgb(0xdcbc72),
        rgb(0xb48c44),
        rgb(0xae8f50),
        rgb(0xe2c888),
        rgb(0x927438),
        rgb(0xa08a66),
        rgb(0xc4a462),
    ];
    let texels = par_texels(w, h, |x, y| {
        if x >= size {
            let (u, v) = ((x - size) as f32 / s, y as f32 / s);
            if u > 0.97 && v < 0.03 {
                return Texel {
                    albedo: Vec3::splat(0.85),
                    roughness: 0.6,
                    ..Default::default()
                };
            }
            // End face: the rolled layers spiral out, wavering, packed with cut stalk ends
            // and loose straw, darker in the cracks between layers.
            let d = Vec2::new(u - 0.5, v - 0.5);
            let r = d.length() * 2.0;
            let a = d.y.atan2(d.x);
            let turns = 11.0;
            let waver = 0.35 * fbm(u * 6.0, v * 6.0, 0, 0, 3, 0.5, seed ^ 0x3)
                + 0.12 * fbm(u * 25.0, v * 25.0, 0, 0, 2, 0.5, seed ^ 0x4);
            let spiral = (r * turns - a / std::f32::consts::TAU + waver).rem_euclid(1.0);
            let gaps = smooth(
                0.35,
                0.65,
                fbm(u * 18.0, v * 18.0, 0, 0, 2, 0.5, seed ^ 0x8) + 0.5,
            );
            let crack = smooth(0.84, 0.97, spiral) * gaps;
            // Stalk ends and short stalks lying every which way.
            let ends = cells(u * 150.0, v * 150.0, 0, 0, 1.0, seed ^ 0x6);
            let e_id = ends.id;
            // Short stalks lie mostly along the layers (around the spiral).
            let dir = a + std::f32::consts::FRAC_PI_2 + (hash01(e_id as i32, 3, seed) - 0.5) * 1.2;
            let (sn, cs) = dir.sin_cos();
            let along = (ends.offset.x * cs + ends.offset.y * sn).abs();
            let across = (-ends.offset.x * sn + ends.offset.y * cs).abs();
            let bit = smooth(0.16, 0.06, across) * smooth(0.55, 0.25, along);
            let k = (e_id % 8) as usize;
            let tone = fbm(u * 5.0, v * 5.0, 0, 0, 3, 0.5, seed ^ 0x9);
            let mut c = mix3(rgb(0xaa8a48), rgb(0xcaa964), tone * 0.5 + 0.5);
            c = mix3(
                c,
                stalk_colors[k] * (0.85 + 0.25 * hash01(e_id as i32, 4, seed)),
                bit * 0.8,
            );
            c = mix3(c, rgb(0x54401e), crack * 0.6);
            let inside = 1.0 - smooth(0.96, 1.0, r);
            return Texel {
                albedo: mix3(rgb(0x8a7440), c, inside) * STRAW_WARM,
                height: (0.004 * (1.0 - crack) + 0.0012 * bit) * inside,
                roughness: 0.88,
                ao: 1.0 - 0.6 * crack,
                // Smoldering hay glows deepest between the layers.
                emissive: 0.3 + 0.7 * crack,
                ..Default::default()
            };
        }
        let (u, v) = (x as f32 / s, y as f32 / s);
        // Side: thin stalks pressed into a mat, mostly running around the bale (along u) but
        // criss-crossing at all sorts of angles, a few lying loose on top.
        let mut top = -1.0f32;
        let mut col = rgb(0x5e4c26);
        let mut hgt = 0.0f32;
        let shears = [0i32, 14, -14, 30, -30, 6, -50, 70];
        for (l, shear) in shears.iter().enumerate() {
            let l = l as u32;
            let seed_l = seed.wrapping_add(l * 911);
            // Stalks: long thin cells stretched along u and sheared by whole periods (so the
            // texture still tiles).
            let c = cells(
                u * 12.0,
                v * 240.0 + u * *shear as f32,
                12,
                240,
                1.0,
                seed_l,
            );
            let width = 0.5 - c.offset.y.abs();
            let len = 0.6 + 0.4 * hash01(c.id as i32, l as i32 + 9, seed);
            let along = 1.0 - (c.offset.x.abs() * 2.0 / len).min(1.0);
            let present = hash01(c.id as i32, l as i32 + 17, seed) < 0.8;
            let stalk = smooth(0.08, 0.3, width) * smooth(0.0, 0.2, along) * present as u8 as f32;
            let z = stalk * (0.6 + 0.4 * hash01(c.id as i32, l as i32, seed)) + l as f32 * 0.06;
            if stalk > 0.05 && z > top {
                top = z;
                let k = (c.id % 8) as usize;
                // Round, glossy stalks: lighter along their middle and toward one end.
                let shade = (0.68 + 0.32 * (1.0 - (c.offset.y.abs() * 2.0).powi(2)))
                    * (0.9 + 0.15 * c.offset.x);
                col = stalk_colors[k] * shade * (0.85 + 0.25 * hash01(c.id as i32, 5, seed));
                hgt = z;
            }
        }
        let grime = fbm(u * 8.0, v * 6.0, 8, 6, 4, 0.5, seed ^ 0x77);
        let patch = fbm(u * 3.0, v * 2.0, 3, 2, 3, 0.5, seed ^ 0x78);
        col *= 0.85 + 0.2 * grime;
        // Weathered, greyer patches and a few greener ones.
        col = mix3(
            col,
            col.dot(Vec3::splat(0.333)) * rgb(0xb8b0a0) * 1.6,
            smooth(0.1, 0.5, patch) * 0.45,
        );
        col = mix3(
            col,
            col * rgb(0xa8b078) * 1.3,
            smooth(0.25, 0.6, -patch) * 0.15,
        );
        let depth = 1.0 - (hgt / 0.5).clamp(0.0, 1.0);
        Texel {
            albedo: mix3(col, rgb(0x3e3218), depth * 0.6) * STRAW_WARM,
            height: 0.005 * hgt,
            roughness: 0.7 + 0.25 * depth,
            ao: 1.0 - 0.5 * depth,
            emissive: (0.25 + 0.75 * depth) * smooth(-0.35, 0.35, grime),
            ..Default::default()
        }
    });
    bake(w, h, &texels, [1.88 / s, 1.3 / s], 2.0)
}

// ---------------------------------------------------------------------------------- rock

/// Weathered granite fieldstone: speckled crystals, soft tonal patches, hairline cracks,
/// lichen. One tile covers 0.5 × 0.5 m.
pub fn granite(size: u32, seed: u32) -> TexSet {
    let s = size as f32;
    let texels = par_texels(size, size, |x, y| {
        let (u, v) = (x as f32 / s, y as f32 / s);
        let tone = fbm(u * 3.0, v * 3.0, 3, 3, 5, 0.55, seed);
        let crystals = cells(u * 90.0, v * 90.0, 90, 90, 1.0, seed ^ 0x1);
        let cid = (crystals.id >> 8) as f32 / 16_777_216.0;
        let crack = smooth(0.93, 0.995, ridged(u * 6.0, v * 6.0, 6, 6, 3, seed ^ 0x2));
        let pit = smooth(
            0.55,
            0.8,
            fbm(u * 40.0, v * 40.0, 40, 40, 3, 0.5, seed ^ 0x3),
        );
        let mut a = mix3(rgb(0x7d776f), rgb(0xa29b90), tone * 0.5 + 0.5);
        if cid < 0.12 {
            a = mix3(a, rgb(0x2b2925), 0.8);
        } else if cid > 0.85 {
            a = mix3(a, rgb(0xd8d2c6), 0.6);
        } else if cid > 0.7 {
            a = mix3(a, rgb(0xa88b76), 0.4);
        }
        let lichen = smooth(0.3, 0.45, fbm(u * 4.0, v * 4.0, 4, 4, 4, 0.6, seed ^ 0x4));
        a = mix3(a, rgb(0xb5ae8a), lichen * 0.55);
        a = mix3(a, rgb(0x2d2a26), crack * 0.8);
        Texel {
            albedo: a,
            height: 0.004 * tone + 0.0004 * cid - 0.003 * crack - 0.0006 * pit,
            roughness: 0.7 + 0.2 * pit + 0.08 * lichen - 0.1 * (cid > 0.85) as u8 as f32,
            ao: 1.0 - 0.4 * crack,
            ..Default::default()
        }
    });
    bake(size, size, &texels, [0.5 / s, 0.5 / s], 2.0)
}

// ---------------------------------------------------------------------------------- leaves

/// Leaf-card atlas, 2 × 2 cards of a twig with alternate leaves (each card's twig grows up
/// from its bottom middle). Alpha is coverage times a per-leaf level in `[0.6, 1]`, so a
/// material whose `base_color` alpha drops below 1 sheds its leaves one by one (burning).
pub fn leaves(size: u32, seed: u32) -> TexSet {
    struct Leaf {
        base: Vec2,
        dir: Vec2,
        len: f32,
        width: f32,
        color: Vec3,
        level: f32,
    }
    let half = size / 2;
    let hs = half as f32;
    // Lay out each card's leaves in card space (0..1, y up from the twig's base).
    // Twig segments: from, to, radius.
    type Segment = (Vec2, Vec2, f32);
    let mut cards: Vec<(Vec<Leaf>, Vec<Segment>)> = Vec::new();
    let greens = [
        rgb(0x40632a),
        rgb(0x4f7230),
        rgb(0x5d7d32),
        rgb(0x3a5a26),
        rgb(0x6b8236),
        rgb(0x7f8a3a),
    ];
    for card in 0..4u32 {
        let mut rng = engine_core::math::Rng::new((seed as u64) << 8 | card as u64);
        let mut twig: Vec<Segment> = Vec::new();
        let mut leaves = Vec::new();
        // Main twig: a gentle curve up the card.
        let bend = rng.range(-0.15, 0.15);
        let point = |t: f32| Vec2::new(0.5 + bend * t * t, 0.04 + 0.86 * t);
        let n = 14;
        for i in 0..n {
            let (t0, t1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            twig.push((point(t0), point(t1), 0.007 * (1.0 - 0.6 * t0)));
        }
        let count = rng.range_i32(8, 12);
        for i in 0..count {
            let t = 0.18 + 0.8 * (i as f32 + rng.range(0.0, 0.4)) / count as f32;
            let side = if i % 2 == 0 { 1.0 } else { -1.0 };
            let p = point(t.min(0.98));
            let up = (point((t + 0.02).min(1.0)) - point((t - 0.02).max(0.0))).normalize();
            let ang = side * rng.range(0.6, 1.1) * (1.0 - 0.5 * t);
            let dir = Vec2::new(
                up.x * ang.cos() - up.y * ang.sin(),
                up.x * ang.sin() + up.y * ang.cos(),
            );
            let len = rng.range(0.2, 0.3) * (1.0 - 0.35 * t);
            // Short stalk to the blade.
            let stalk_end = p + dir * 0.03;
            twig.push((p, stalk_end, 0.003));
            let mut color = greens[rng.range_i32(0, greens.len() as i32) as usize];
            if rng.chance(0.1) {
                color = mix3(color, rgb(0xa08a3a), rng.range(0.4, 0.9));
            }
            leaves.push(Leaf {
                base: stalk_end,
                dir,
                len,
                width: len * rng.range(0.32, 0.42),
                color: color * rng.range(0.85, 1.12),
                level: rng.range(0.6, 1.0),
            });
        }
        // Tip leaf.
        let tip = point(0.9);
        leaves.push(Leaf {
            base: tip,
            dir: (point(1.0) - point(0.9)).normalize(),
            len: 0.16,
            width: 0.06,
            color: greens[1],
            level: 1.0,
        });
        cards.push((leaves, twig));
    }
    let texels = par_texels(size, size, |x, y| {
        let card = ((y / half) * 2 + x / half) as usize;
        let (leaves, twig) = &cards[card];
        // Card space: y up.
        let p = Vec2::new(
            ((x % half) as f32 + 0.5) / hs,
            1.0 - ((y % half) as f32 + 0.5) / hs,
        );
        let aa = 1.5 / hs;
        let mut out = Texel {
            albedo: rgb(0x40632a),
            alpha: 0.0,
            roughness: 0.55,
            ..Default::default()
        };
        // Leaves (later ones on top).
        for (i, l) in leaves.iter().enumerate() {
            let d = p - l.base;
            let along = d.dot(l.dir);
            if along < -0.01 || along > l.len + 0.01 {
                continue;
            }
            let side = Vec2::new(-l.dir.y, l.dir.x);
            let across = d.dot(side);
            let t = (along / l.len).clamp(0.0, 1.0);
            // Ovate blade, pointed tip, a little asymmetric; serrated edge.
            let profile = (t * std::f32::consts::PI).sin().powf(0.75) * (1.0 - 0.35 * t);
            let serr = 1.0 - 0.06 * ((t * 38.0).sin() * 0.5 + 0.5);
            let half_w = l.width * 0.5 * profile * serr;
            let edge = half_w - across.abs();
            let cov = smooth(-aa, aa, edge);
            if cov <= 0.0 {
                continue;
            }
            let rel = across / half_w.max(1e-4);
            let midrib = 1.0 - smooth(0.0, 0.06, rel.abs());
            let vein_phase = (t * 9.0 - rel.abs() * 1.6) * std::f32::consts::TAU;
            let vein = smooth(0.85, 1.0, vein_phase.cos()) * (1.0 - rel.abs()) * 0.6;
            let n = fbm(
                p.x * 60.0 + i as f32 * 3.1,
                p.y * 60.0,
                0,
                0,
                3,
                0.5,
                seed ^ 0x77,
            );
            let mut c = l.color * (0.9 + 0.15 * n);
            // Edges a little darker and drier; base and midrib paler.
            c = mix3(c, c * 0.7, smooth(0.6, 1.0, rel.abs()));
            c = mix3(c, rgb(0xa4b05c), midrib * 0.6 + vein * 0.25);
            let spot = smooth(
                0.62,
                0.75,
                fbm(p.x * 25.0, p.y * 25.0, 0, 0, 2, 0.5, seed ^ i as u32),
            );
            c = mix3(c, rgb(0x6d5a2a), spot * 0.6);
            // Folded along the midrib: each half rises away from it.
            out = Texel {
                albedo: mix3(out.albedo, c, cov),
                alpha: out.alpha.max(cov * l.level),
                height: rel.abs() * 0.0015 - midrib * 0.0006 - vein * 0.0003,
                roughness: 0.68 + 0.1 * n,
                ao: 1.0,
                ..Default::default()
            };
        }
        // Twig on top of the leaf bases.
        for (a, b, r) in twig {
            let ab = *b - *a;
            let t = ((p - *a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            let d = (p - (*a + ab * t)).length();
            let cov = smooth(r + aa, r - aa, d);
            if cov > 0.0 {
                let shade = 1.0 - (d / r).min(1.0).powi(2) * 0.5;
                out.albedo = mix3(out.albedo, rgb(0x5a4632) * shade, cov);
                out.alpha = out.alpha.max(cov);
                out.height += cov * 0.001;
                out.roughness = 0.8;
            }
        }
        out
    });
    let mut set = bake(size, size, &texels, [0.5 / hs, 0.5 / hs], 0.0);
    coverage_preserving_mips(&mut set.albedo, 0.5);
    set
}

// ---------------------------------------------------------------------------------- firewood

/// Split firewood for the campfire, charred toward one end. Atlas regions:
/// - `u < 0.5`: bark (u around the log's round side, v along the log from its fire end);
/// - `0.5 ≤ u < 0.875`: split faces (long grain), v along the log;
/// - `u ≥ 0.875`, `v < 0.125`: end grain of the outer (unburnt) end, a disc;
/// - `u ≥ 0.875`, `0.125 ≤ v < 0.25`: end grain of the charred end;
/// - `u ≥ 0.875`, `0.25 ≤ v < 0.5`: live charcoal, glowing in its cracks and wherever its ash
///   skin has flaked off.
///
/// The fire end (`v` near 0) is black, alligator-cracked char whose cracks glow (emissive);
/// it fades through scorched brown to fresh wood and bark by `v` = 0.6.
pub fn firewood(size: u32, seed: u32) -> TexSet {
    let s = size as f32;
    let texels = par_texels(size, size, |x, y| {
        let (u, v) = (x as f32 / s, y as f32 / s);
        if u >= 0.875 && (0.25..0.5).contains(&v) {
            // Live charcoal: blocky char split by glowing fissures, under a patchy grey skin.
            let blocks = cells(u * 40.0, v * 40.0, 0, 0, 0.8, seed ^ 0x61);
            let crack = 1.0 - smooth(0.06, 0.3, blocks.f2 - blocks.f1);
            let skin = smooth(
                0.45,
                0.7,
                fbm(u * 90.0, v * 90.0, 0, 0, 3, 0.5, seed ^ 0x62),
            );
            let shade = (blocks.id >> 8) as f32 / 16_777_216.0;
            let char_c = mix3(rgb(0x120e0c), rgb(0x2c2826), shade * 0.7);
            let mut a = mix3(char_c, rgb(0x9a948e), skin * 0.8);
            a = mix3(a, rgb(0x1a0804), crack);
            return Texel {
                albedo: a,
                height: 0.002 * (1.0 - crack) + 0.0008 * skin,
                roughness: 0.95,
                ao: 1.0 - 0.5 * crack,
                emissive: crack * (1.0 - 0.6 * skin),
                ..Default::default()
            };
        }
        let (fresh, along_char) = if u >= 0.875 && v < 0.25 {
            // End grain discs.
            let charred_end = v >= 0.125;
            let c = Vec2::new(0.9375, if charred_end { 0.1875 } else { 0.0625 });
            let d = (Vec2::new(u, v) - c) / 0.0625;
            let r = d.length();
            let wobble = fbm(d.x * 3.0, d.y * 3.0, 0, 0, 2, 0.5, seed ^ 0x9) * 0.06;
            let ring = ((r + wobble) * 22.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            let check = smooth(
                0.94,
                0.99,
                ridged(d.y.atan2(d.x) * 3.0, r * 2.0, 0, 0, 2, seed),
            );
            let mut a = mix3(rgb(0xb98d5c), rgb(0x8a5e38), ring * 0.6 + 0.3 * r);
            a = mix3(a, rgb(0x2a1c12), check);
            let bark_ring = smooth(0.86, 0.92, r);
            a = mix3(a, rgb(0x3b2d22), bark_ring);
            let t = Texel {
                albedo: a,
                height: 0.0006 * ring - 0.002 * check,
                roughness: 0.85,
                ..Default::default()
            };
            (t, if charred_end { 0.0 } else { 1.0 })
        } else if u < 0.5 {
            // Bark.
            let bu = u * 2.0;
            let c = cells(bu * 8.0, v * 3.0, 8, 3, 0.8, seed ^ 0x21);
            let edge = smooth(0.0, 0.25, c.f2 - c.f1);
            let fiber = fbm(bu * 50.0, v * 6.0, 50, 6, 3, 0.5, seed ^ 0x22);
            let lichen = smooth(0.3, 0.5, fbm(bu * 4.0, v * 4.0, 4, 4, 3, 0.5, seed ^ 0x23));
            let mut a = mix3(rgb(0x3a2c22), rgb(0x7a6a58), edge * (0.6 + 0.4 * fiber));
            a = mix3(a, rgb(0x8e8a78), lichen * edge * 0.5);
            let t = Texel {
                albedo: a,
                height: 0.004 * edge + 0.0008 * fiber,
                roughness: 0.9,
                ao: 0.6 + 0.4 * edge,
                ..Default::default()
            };
            (t, v)
        } else {
            // Split face: long grain, torn fibers.
            let su = (u - 0.5) / 0.375;
            let grain = fbm(su * 3.0, v * 60.0, 0, 0, 2, 0.5, seed ^ 0x31);
            let lines = ((su * 18.0 + grain * 0.6) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            let torn = fbm(su * 40.0, v * 4.0, 0, 0, 3, 0.6, seed ^ 0x32);
            let a = mix3(rgb(0xc49a68), rgb(0x8e6640), lines * 0.5 + 0.25 * torn);
            let t = Texel {
                albedo: a,
                height: 0.0015 * torn + 0.0005 * lines,
                roughness: 0.75,
                ..Default::default()
            };
            (t, v)
        };
        // Char: alligator cracks toward the fire end.
        let char_amount = 1.0
            - smooth(
                0.2,
                0.36,
                along_char + 0.08 * fbm(u * 9.0, v * 9.0, 0, 0, 2, 0.5, seed),
            );
        let scorch = 1.0 - smooth(0.3, 0.5, along_char);
        let blocks = cells(u * 40.0, v * 22.0, 40, 22, 0.7, seed ^ 0x41);
        let crack = 1.0 - smooth(0.02, 0.12, blocks.f2 - blocks.f1);
        let block_shade = (blocks.id >> 8) as f32 / 16_777_216.0;
        let mut a = mix3(
            fresh.albedo,
            fresh.albedo * rgb(0x6a4a30) * 2.0,
            scorch * 0.7,
        );
        let char_c = mix3(rgb(0x0d0b0a), rgb(0x2a2725), block_shade * 0.6);
        a = mix3(a, char_c, char_amount);
        // Pale ash on the char's ridges.
        let ash =
            smooth(0.5, 0.9, fbm(u * 70.0, v * 70.0, 0, 0, 2, 0.5, seed ^ 0x51)) * char_amount;
        a = mix3(a, rgb(0x8a8682), ash * 0.5 * (1.0 - crack));
        a = mix3(a, rgb(0x050403), crack * char_amount);
        Texel {
            albedo: a,
            height: fresh.height * (1.0 - char_amount)
                + char_amount * (0.003 * (1.0 - crack) + 0.001 * block_shade),
            roughness: mix(fresh.roughness, 0.95, char_amount),
            ao: fresh.ao * (1.0 - 0.4 * crack * char_amount),
            emissive: crack * char_amount * smooth(0.0, 0.25, 0.3 - along_char),
            ..fresh
        }
    });
    bake(size, size, &texels, [0.25 / s, 1.0 / s], 1.5)
}

#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------------- ground

/// The ground layers of the demo, in texture-array order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GroundKind {
    /// Meadow floor: soil under a mat of dry thatch.
    #[default]
    Litter,
    /// Bare, compacted dusty soil with pebbles and sparse cut stubble.
    Dirt,
    /// Road gravel: packed stones in dust.
    Gravel,
    /// Waterlogged soil with puddles and green slime.
    Mud,
    /// Wood ash and charcoal (a fire pit).
    Ash,
}

use serde::{Deserialize, Serialize};

impl GroundKind {
    pub const ALL: [GroundKind; 5] = [
        GroundKind::Litter,
        GroundKind::Dirt,
        GroundKind::Gravel,
        GroundKind::Mud,
        GroundKind::Ash,
    ];

    /// Texture-array layer.
    pub fn layer(self) -> u32 {
        self as u32
    }

    /// Meters covered by one texture tile.
    pub fn tile_m(self) -> f32 {
        match self {
            GroundKind::Litter => 2.0,
            GroundKind::Dirt => 3.0,
            GroundKind::Gravel => 1.25,
            GroundKind::Mud => 2.5,
            GroundKind::Ash => 1.5,
        }
    }

    /// Generate this layer: albedo with height (0..1 over the layer's relief) in alpha, and a
    /// packed map with the normal's X/Y in R/G, roughness in B and occlusion in A.
    pub fn generate(self, size: u32, seed: u32) -> (Image, Image) {
        let tile = self.tile_m();
        let s = size as f32;
        let texels = par_texels(size, size, |x, y| {
            let (u, v) = (x as f32 / s, y as f32 / s);
            match self {
                GroundKind::Litter => litter(u, v, seed),
                GroundKind::Dirt => dirt(u, v, seed ^ 0x1000),
                GroundKind::Gravel => gravel(u, v, seed ^ 0x2000),
                GroundKind::Mud => mud(u, v, seed ^ 0x3000),
                GroundKind::Ash => ash(u, v, seed ^ 0x4000),
            }
        });
        let relief = self.relief_m();
        let set = bake(size, size, &texels, [tile / s, tile / s], 2.0);
        // Repack: albedo + height, normal XY + roughness + AO.
        let mut albedo = set.albedo;
        for (px, t) in albedo.data.as_chunks_mut::<4>().0.iter_mut().zip(&texels) {
            px[3] = unorm(t.height / relief * 0.5 + 0.5);
        }
        let mut packed = set.normal;
        for (px, orm) in packed
            .data
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(set.orm.data.as_chunks::<4>().0)
        {
            px[2] = orm[1];
            px[3] = orm[0];
        }
        (albedo, packed)
    }

    /// Height range stored in the albedo alpha (± this many meters).
    fn relief_m(self) -> f32 {
        match self {
            GroundKind::Gravel => 0.02,
            GroundKind::Litter => 0.015,
            _ => 0.012,
        }
    }
}

/// Dry thatch over dark soil: layers of straight and bent dead blades, seeds, twigs.
fn litter(u: f32, v: f32, seed: u32) -> Texel {
    let soil = fbm(u * 6.0, v * 6.0, 6, 6, 4, 0.5, seed);
    let mut col = mix3(rgb(0x3a2e22), rgb(0x54432f), soil * 0.5 + 0.5);
    let mut top = 0.0f32;
    let mut height = -0.006 + 0.002 * soil;
    let blade_cols = [
        rgb(0xa8915c),
        rgb(0xc2aa70),
        rgb(0x8c7c50),
        rgb(0x9a9070),
        rgb(0x7a6e48),
        rgb(0xb59f62),
    ];
    for l in 0..7u32 {
        // Blades lie every which way: shear the stretched lattice by whole periods (so the
        // texture tiles) and swap the axes for every other layer.
        let k = [0.0, 1.0, -1.0, 2.0, -2.0, 0.0, 3.0][l as usize];
        let (a, b) = if l % 2 == 0 { (u, v) } else { (v, u) };
        let c = cells(
            a * 6.0,
            b * 72.0 + a * 72.0 * k,
            6,
            72,
            1.0,
            seed.wrapping_add(l * 31),
        );
        let w = 0.5 - c.offset.y.abs();
        let len = 1.0 - (c.offset.x.abs() * 1.3).min(1.0);
        let blade = smooth(0.12, 0.32, w) * smooth(0.0, 0.2, len);
        let present = hash01(c.id as i32, l as i32, seed) < 0.55 + 0.05 * l as f32;
        let z = l as f32 + blade;
        if present && blade > 0.1 && z > top {
            top = z;
            let k = (c.id % 6) as usize;
            let shade = 0.75 + 0.25 * (1.0 - (c.offset.y * 2.0).abs());
            col = blade_cols[k] * shade * (0.8 + 0.2 * l as f32 / 7.0);
            height = 0.0015 * l as f32 + 0.001 * blade;
        }
    }
    // Wrap-safe tonal variation.
    let tone = fbm(u * 3.0, v * 3.0, 3, 3, 3, 0.5, seed ^ 0x99);
    col *= 0.85 + 0.25 * tone;
    let depth = 1.0 - (top / 7.5).clamp(0.0, 1.0);
    Texel {
        albedo: col,
        height,
        roughness: 0.82 + 0.15 * depth,
        ao: 1.0 - 0.55 * depth,
        ..Default::default()
    }
}

/// Dusty compacted soil: clods and crumbs, pebbles, a few fine cracks where it dried out,
/// bits of dead grass.
fn dirt(u: f32, v: f32, seed: u32) -> Texel {
    let tone = fbm(u * 4.0, v * 4.0, 4, 4, 6, 0.55, seed);
    let fine = fbm(u * 64.0, v * 64.0, 64, 64, 3, 0.5, seed ^ 0x5);
    let clods = fbm(u * 24.0, v * 24.0, 24, 24, 3, 0.6, seed ^ 0x6);
    let mut col = mix3(rgb(0x5a4e42), rgb(0x86786a), tone * 0.5 + 0.5);
    col *= 0.88 + 0.14 * fine;
    col = mix3(col, rgb(0x4a3f34), smooth(0.1, 0.5, -clods) * 0.4);
    // Fine cracks: warped cell edges, only where the soil dried out.
    let wu = u + 0.02 * noise(u * 8.0, v * 8.0, 8, 8, seed ^ 0x71);
    let wv = v + 0.02 * noise(u * 8.0 + 3.1, v * 8.0, 8, 8, seed ^ 0x72);
    let cr = cells(wu * 10.0, wv * 10.0, 10, 10, 0.9, seed ^ 0x7);
    let dry = smooth(0.15, 0.45, fbm(u * 3.0, v * 3.0, 3, 3, 3, 0.5, seed ^ 0x73));
    let width = 0.012 + 0.02 * (fine * 0.5 + 0.5);
    let crack = (1.0 - smooth(0.0, width, cr.f2 - cr.f1)) * dry;
    // Stones: a few pebbles and a scatter of grit, gathered in patches rather than sprinkled
    // evenly, irregular, sunk into the soil with a dark rim, and of the soil's own rock (a
    // little paler or darker than it), not bright specks.
    let gather = smooth(-0.3, 0.5, fbm(u * 5.0, v * 5.0, 5, 5, 3, 0.5, seed ^ 0x13));
    let mut pebble = 0.0f32;
    let mut bump = 0.0f32;
    let mut rim = 0.0f32;
    let mut pk = 0.0f32;
    for (scale, chance, size, tall, k) in [
        (40.0f32, 0.02 + 0.12 * gather, 0.42f32, 0.004f32, 0x9u32),
        (90.0, 0.03 + 0.14 * gather, 0.3, 0.0015, 0x19),
    ] {
        let p = scale as i32;
        let c = cells(u * scale, v * scale, p, p, 1.0, seed ^ k);
        let id = c.id as i32;
        if hash01(id, 1, seed ^ k) >= chance {
            continue;
        }
        let angle = c.offset.y.atan2(c.offset.x);
        let phase = 6.0 * hash01(id, 4, seed ^ k);
        let lumpy = 1.0 + 0.1 * (angle * 2.0 + phase).sin() + 0.07 * (angle * 3.0 - phase).sin();
        let r = size * (0.5 + 0.5 * hash01(id, 2, seed ^ k)) * lumpy;
        let inside = 1.0 - smooth(r * 0.75, r, c.f1);
        rim = rim.max(smooth(r * 0.8, r, c.f1) * (1.0 - smooth(r, r * 1.35, c.f1)));
        if inside > pebble {
            pebble = inside;
            bump = tall * (1.0 - (c.f1 / r).min(1.0).powi(2)).sqrt();
            pk = hash01(id, 3, seed ^ k);
        }
    }
    let rock = mix3(rgb(0x564d43), rgb(0x8e8578), pk) * (0.8 + 0.3 * (bump * 250.0).min(1.0));
    col = mix3(col, col * 0.72, rim * 0.35);
    col = mix3(col, rock, pebble);
    col = mix3(col, rgb(0x2e261e), crack * 0.55);
    // Bits of dead grass lying on the dirt.
    let st = cells(u * 26.0, v * 140.0 + u * 140.0, 26, 140, 1.0, seed ^ 0x11);
    let stub = smooth(0.38, 0.46, 0.5 - st.offset.y.abs())
        * smooth(0.0, 0.3, 1.0 - st.offset.x.abs() * 2.0)
        * (hash01(st.id as i32, 5, seed) < 0.05) as u8 as f32;
    col = mix3(col, rgb(0x8c7a56) * (0.8 + 0.3 * pk), stub * 0.7);
    Texel {
        albedo: col,
        height: 0.003 * tone + 0.0012 * clods + 0.0004 * fine - 0.0015 * crack
            + bump * pebble
            + 0.001 * stub,
        roughness: 0.92 - 0.06 * pebble,
        ao: (1.0 - 0.3 * crack) * (1.0 - 0.2 * rim),
        ..Default::default()
    }
}

/// Road gravel: rounded stones of mixed rock in a matrix of dust.
fn gravel(u: f32, v: f32, seed: u32) -> Texel {
    let dust = fbm(u * 8.0, v * 8.0, 8, 8, 4, 0.5, seed);
    let mut col = mix3(rgb(0x7a6c5a), rgb(0x968a78), dust * 0.5 + 0.5);
    let mut height = -0.008 + 0.002 * dust;
    let mut rough = 0.95;
    let mut ao = 0.55;
    let palette = [
        rgb(0x8f8b85),
        rgb(0xa49c90),
        rgb(0x6e6a64),
        rgb(0xb7ad9c),
        rgb(0x9a7f66),
        rgb(0xb8b2a6),
        rgb(0x5d5954),
        rgb(0x8a8070),
    ];
    for (scale, s_seed) in [(48.0f32, 0x1u32), (75.0, 0x2), (110.0, 0x3)] {
        let p = scale as i32;
        let q = p / 2;
        let warp_u = u + 0.15 / scale * noise(u * q as f32, v * q as f32, q, q, seed ^ s_seed);
        let c = cells(warp_u * scale, v * scale, p, p, 0.85, seed ^ (s_seed << 8));
        let r = 0.36 + 0.12 * hash01(c.id as i32, 1, seed);
        if c.f1 < r {
            let dome = (1.0 - (c.f1 / r).powi(2)).sqrt();
            let h = dome * (0.012 * 48.0 / scale) * (0.7 + 0.3 * hash01(c.id as i32, 2, seed));
            if h > height {
                height = h;
                let k = (c.id % 8) as usize;
                let speck = noise(u * 900.0, v * 900.0, 900, 900, c.id);
                col = palette[k] * (0.72 + 0.18 * dome + 0.08 * speck);
                rough = 0.72 + 0.22 * hash01(c.id as i32, 3, seed);
                ao = 0.65 + 0.35 * dome;
            }
        }
    }
    Texel {
        albedo: col,
        height,
        roughness: rough,
        ao,
        ..Default::default()
    }
}

/// Waterlogged soil: dark mud, standing water in the hollows, slime and drowned grass.
fn mud(u: f32, v: f32, seed: u32) -> Texel {
    let tone = fbm(u * 5.0, v * 5.0, 5, 5, 6, 0.55, seed);
    let fine = fbm(u * 50.0, v * 50.0, 50, 50, 3, 0.5, seed ^ 0x3);
    let height = 0.006 * tone + 0.0008 * fine;
    let water = smooth(-0.2, -0.26, tone);
    let mut col = mix3(rgb(0x2a2018), rgb(0x40342a), fine * 0.5 + 0.5);
    let slime = smooth(0.1, 0.3, fbm(u * 7.0, v * 7.0, 7, 7, 4, 0.5, seed ^ 0x8)) * (1.0 - water);
    col = mix3(col, rgb(0x3b4a20), slime * 0.6);
    let grass = cells(u * 20.0, v * 120.0, 20, 120, 1.0, seed ^ 0x9);
    let blade = smooth(0.35, 0.45, 0.5 - grass.offset.y.abs())
        * (hash01(grass.id as i32, 1, seed) < 0.2) as u8 as f32
        * (1.0 - water);
    col = mix3(col, rgb(0x55652e), blade * 0.7);
    col = mix3(col, rgb(0x1a1612), water * 0.6);
    Texel {
        albedo: col,
        height: height * (1.0 - water) + water * -0.0012 + blade * 0.001,
        roughness: mix(0.5 - 0.1 * slime, 0.06, water),
        ao: 1.0,
        ..Default::default()
    }
}

/// Fire-pit ash: pale powdery ash, charcoal lumps, burnt soil showing through.
fn ash(u: f32, v: f32, seed: u32) -> Texel {
    let tone = fbm(u * 5.0, v * 5.0, 5, 5, 5, 0.5, seed);
    let fine = fbm(u * 80.0, v * 80.0, 80, 80, 2, 0.5, seed ^ 0x2);
    let mut col = mix3(rgb(0x6a6662), rgb(0xa7a39d), tone * 0.5 + 0.5);
    col *= 0.9 + 0.1 * fine;
    let soil = smooth(0.25, 0.45, -tone);
    col = mix3(col, rgb(0x2a2420), soil * 0.7);
    // Charcoal: angular chunks and grit of every size, gathered in drifts rather than
    // sprinkled evenly, some only half burnt (brown).
    let gather = smooth(-0.2, 0.6, fbm(u * 4.0, v * 4.0, 4, 4, 3, 0.5, seed ^ 0x6));
    let mut coal = 0.0f32;
    let mut lift = 0.0f32;
    let mut brown = 0.0f32;
    for (scale, chance, size, k) in [
        (14.0f32, 0.1 + 0.35 * gather, 0.4f32, 0x4u32),
        (36.0, 0.15 + 0.45 * gather, 0.38, 0x14),
        (90.0, 0.2 + 0.5 * gather, 0.3, 0x24),
    ] {
        let p = scale as i32;
        let ch = cells(u * scale, v * scale, p, p, 1.0, seed ^ k);
        let id = ch.id as i32;
        if hash01(id, 1, seed ^ k) >= chance {
            continue;
        }
        // Angular: a few flat sides, turned and stretched per chunk.
        let angle = ch.offset.y.atan2(ch.offset.x) + 6.0 * hash01(id, 4, seed ^ k);
        let sides = 3.0 + (hash01(id, 5, seed ^ k) * 3.0).floor();
        let facet = (std::f32::consts::PI / sides).cos()
            / ((angle * sides / std::f32::consts::TAU).rem_euclid(1.0) * std::f32::consts::TAU
                / sides
                - std::f32::consts::PI / sides)
                .cos();
        let r = size * (0.45 + 0.55 * hash01(id, 2, seed ^ k)) * facet;
        let inside = 1.0 - smooth(r * 0.85, r, ch.f1);
        if inside > coal {
            coal = inside;
            lift = (1.0 - ch.f1 / r).max(0.0) * size / scale;
            brown = smooth(0.75, 0.95, hash01(id, 3, seed ^ k));
        }
    }
    let facets = cells(u * 300.0, v * 300.0, 300, 300, 1.0, seed ^ 0x5);
    let sheen = (facets.id >> 8) as f32 / 16_777_216.0;
    let char_c = mix3(rgb(0x141210), rgb(0x3a2a1c), brown) * (0.8 + 0.6 * sheen);
    col = mix3(col, char_c, coal);
    Texel {
        albedo: col,
        height: 0.002 * tone + 0.0003 * fine + 0.25 * lift * coal,
        roughness: mix(0.95, 0.6 + 0.3 * sheen, coal),
        ao: 1.0 - 0.2 * soil,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_and_stays_in_range() {
        for i in 0..200 {
            let x = i as f32 * 0.137;
            let y = i as f32 * 0.071;
            let a = noise(x, y, 8, 4, 7);
            let b = noise(x + 8.0, y + 4.0, 8, 4, 7);
            assert!((a - b).abs() < 1e-4, "{a} {b}");
            assert!(a.abs() <= 1.5);
            let c = cells(x, y, 5, 5, 1.0, 3);
            let d = cells(x + 5.0, y - 5.0, 5, 5, 1.0, 3);
            assert!((c.f1 - d.f1).abs() < 1e-4 && c.id == d.id);
        }
    }

    #[test]
    fn small_sets_bake_with_consistent_sizes() {
        let set = bark(32, 1);
        assert_eq!(set.albedo.data.len(), 32 * 32 * 4);
        assert_eq!(set.normal.data.len(), 32 * 32 * 4);
        assert!(set.albedo.srgb && !set.normal.srgb && !set.orm.srgb);
        let leaf = leaves(64, 2);
        assert!(leaf.albedo.data.chunks(4).any(|p| p[3] == 0));
        assert!(leaf.albedo.data.chunks(4).any(|p| p[3] > 200));
        assert_eq!(leaf.albedo.mips.len(), 6);
        let wood = firewood(64, 3);
        assert!(wood.emissive.is_some());
        for kind in GroundKind::ALL {
            let (a, n) = kind.generate(16, 4);
            assert_eq!((a.width, n.width), (16, 16));
        }
    }
}
