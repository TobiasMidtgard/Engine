//! Procedural textures for the yard's props and ground: tileable noise (gradient, value,
//! cellular), and generators for weathered wood (an atlas with long and end grain), painted
//! steel, galvanized steel, painted cast iron, worn metal, stone, concrete, rope, packed
//! earth with gravel, and grass.
//!
//! Every generator returns a [`TexSet`]: an sRGB albedo, a tangent-space normal map (+Y up,
//! derived from a height field in meters) and a half-resolution ORM map (R ambient
//! occlusion, G roughness, B metallic: glTF order, so one image serves both the occlusion and
//! the metallic/roughness slot of a `StandardMaterial`). All of them tile seamlessly.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Vec2, Vec3, smoothstep};
use engine_render::Image;

use super::meshkit::{WOOD_END_M, WOOD_GRAIN_H_M, WOOD_GRAIN_U, WOOD_GRAIN_W_M};

// ---------------------------------------------------------------------------------------
// Noise

#[inline]
fn hash3(x: i32, y: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// Uniform random number in 0..1 for an integer coordinate.
#[inline]
pub fn rand01(x: i32, y: i32, seed: u32) -> f32 {
    (hash3(x, y, seed as i32) >> 8) as f32 / 16_777_216.0
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

const GRADS: [(f32, f32); 16] = {
    let mut g = [(0.0, 0.0); 16];
    // cos/sin of k·22.5°, written out (const fn has no trig).
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

/// Tileable 2D gradient noise, about −1..1, with a period of `(px, py)` lattice cells.
pub fn gnoise(x: f32, y: f32, px: i32, py: i32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (fx, fy) = (x - xf, y - yf);
    let (ix, iy) = (xf as i32, yf as i32);
    let g = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let h = hash3(
            cx.rem_euclid(px.max(1)),
            cy.rem_euclid(py.max(1)),
            seed as i32,
        );
        let (gx, gy) = GRADS[(h & 15) as usize];
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

/// Tileable fractal noise over the unit square: `cells` lattice cells across at the first
/// octave, doubling each octave. About −1..1 (mostly −0.6..0.6).
pub fn fbm(u: f32, v: f32, cells: (i32, i32), octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut fx, mut fy) = cells;
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += amp
            * gnoise(
                u * fx as f32,
                v * fy as f32,
                fx,
                fy,
                seed.wrapping_add(o * 1013),
            );
        norm += amp;
        amp *= gain;
        fx *= 2;
        fy *= 2;
    }
    sum / norm
}

/// Smooth 3D value noise (not tiled), −1..1. Used to jitter mesh vertices.
pub fn noise3(p: Vec3, seed: u32) -> f32 {
    let f = p.floor();
    let t = p - f;
    let (ix, iy, iz) = (f.x as i32, f.y as i32, f.z as i32);
    let r = |x: i32, y: i32, z: i32| {
        rand01(
            x.wrapping_mul(73_856_093) ^ z.wrapping_mul(19_349_663),
            y,
            seed,
        ) * 2.0
            - 1.0
    };
    let (u, v, w) = (fade(t.x), fade(t.y), fade(t.z));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(r(ix, iy, iz), r(ix + 1, iy, iz), u);
    let x10 = lerp(r(ix, iy + 1, iz), r(ix + 1, iy + 1, iz), u);
    let x01 = lerp(r(ix, iy, iz + 1), r(ix + 1, iy, iz + 1), u);
    let x11 = lerp(r(ix, iy + 1, iz + 1), r(ix + 1, iy + 1, iz + 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

/// Nearest feature points of tileable cellular noise.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    /// Distance to the nearest point, in cells.
    pub f1: f32,
    /// Distance to the second nearest point, in cells.
    pub f2: f32,
    /// Stable id of the nearest point's cell.
    pub id: u32,
    /// From the sample to the nearest point, in cells.
    pub delta: Vec2,
}

/// Tileable cellular (Worley) noise with `n`×`n` cells over the unit square.
pub fn worley(u: f32, v: f32, n: i32, jitter: f32, seed: u32) -> Cell {
    let (x, y) = (u * n as f32, v * n as f32);
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let mut best = Cell {
        f1: f32::MAX,
        f2: f32::MAX,
        id: 0,
        delta: Vec2::ZERO,
    };
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (ix + dx, iy + dy);
            let (wx, wy) = (cx.rem_euclid(n), cy.rem_euclid(n));
            let h = hash3(wx, wy, seed as i32);
            let px = cx as f32 + 0.5 + (((h & 0xffff) as f32 / 65535.0) - 0.5) * jitter;
            let py = cy as f32 + 0.5 + (((h >> 16) as f32 / 65535.0) - 0.5) * jitter;
            let d = Vec2::new(px - x, py - y);
            let dist = d.length();
            if dist < best.f1 {
                best.f2 = best.f1;
                best.f1 = dist;
                best.id = (wy * n + wx) as u32;
                best.delta = d;
            } else if dist < best.f2 {
                best.f2 = dist;
            }
        }
    }
    best
}

/// Distance from `p` to segment `a`–`b` on the unit torus (wrapping across edges).
fn seg_dist_wrapped(p: Vec2, a: Vec2, b: Vec2) -> (f32, f32) {
    let mut d = p - a;
    d -= d.round();
    let ab = b - a;
    let t = (d.dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    ((d - ab * t).length(), t)
}

fn srgb(r: f32, g: f32, b: f32) -> Vec3 {
    let c = |s: f32| {
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec3::new(c(r), c(g), c(b))
}

fn to_srgb8(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ---------------------------------------------------------------------------------------
// Surfaces

/// One texel of a material before it is packed into images.
#[derive(Debug, Clone, Copy)]
pub struct Px {
    /// Linear RGB.
    pub albedo: Vec3,
    pub alpha: f32,
    /// Meters above the mean surface.
    pub height: f32,
    pub rough: f32,
    pub metal: f32,
    pub ao: f32,
}

impl Default for Px {
    fn default() -> Self {
        Self {
            albedo: Vec3::splat(0.5),
            alpha: 1.0,
            height: 0.0,
            rough: 0.8,
            metal: 0.0,
            ao: 1.0,
        }
    }
}

/// Albedo, normal and ORM images of one material.
pub struct TexSet {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
}

impl TexSet {
    /// GPU bytes including mip chains.
    pub fn bytes(&self) -> usize {
        [&self.albedo, &self.normal, &self.orm]
            .iter()
            .map(|i| i.data.len() * 4 / 3)
            .sum()
    }
}

/// Evaluate `f(x, y)` for every texel of a `w`×`h` image on the compute pool.
pub fn par_texels<T: Send + Clone + Default>(
    w: u32,
    h: u32,
    f: impl Fn(u32, u32) -> T + Send + Sync,
) -> Vec<T> {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let mut out = vec![T::default(); w * h];
    // Idempotent; lets this run outside an `App` (tests).
    engine_core::tasks::init_task_pools();
    let rows = 16;
    engine_core::tasks::par_chunks_mut(&mut out, rows * w, |ci, chunk| {
        for (k, t) in chunk.iter_mut().enumerate() {
            let i = ci * rows * w + k;
            *t = f((i % w) as u32, (i / w) as u32);
        }
    });
    out
}

/// Wrapping box blur of a scalar field (two separable passes).
fn blur(src: &[f32], w: usize, h: usize, r: i32) -> Vec<f32> {
    let n = (2 * r + 1) as f32;
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in -r..=r {
                s += src[y * w + (x as i32 + k).rem_euclid(w as i32) as usize];
            }
            tmp[y * w + x] = s / n;
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in -r..=r {
                s += tmp[(y as i32 + k).rem_euclid(h as i32) as usize * w + x];
            }
            out[y * w + x] = s / n;
        }
    }
    out
}

/// Pack texels into images. `texel(x, y)` gives the texel's size in meters along u and v
/// (for the normal map). `cavity` darkens ambient occlusion where the surface sits below its
/// surroundings: AO loses `cavity` per millimeter of depth.
fn finish(
    w: u32,
    h: u32,
    px: &[Px],
    texel: impl Fn(u32, u32) -> (f32, f32),
    cavity: f32,
) -> TexSet {
    let (wu, hu) = (w as usize, h as usize);
    let heights: Vec<f32> = px.iter().map(|p| p.height).collect();
    let at = |x: i32, y: i32| {
        heights[y.rem_euclid(h as i32) as usize * wu + x.rem_euclid(w as i32) as usize]
    };
    let mut normal = Vec::with_capacity(wu * hu * 4);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (du, dv) = texel(x as u32, y as u32);
            let sx = (at(x + 1, y) - at(x - 1, y)) / (2.0 * du);
            let sy = (at(x, y + 1) - at(x, y - 1)) / (2.0 * dv);
            let n = Vec3::new(-sx, sy, 1.0).normalize();
            normal.extend_from_slice(&[
                to8(n.x * 0.5 + 0.5),
                to8(n.y * 0.5 + 0.5),
                to8(n.z * 0.5 + 0.5),
                255,
            ]);
        }
    }
    let blurred = blur(&heights, wu, hu, ((w / 128).max(2)) as i32);
    let mut albedo = Vec::with_capacity(wu * hu * 4);
    let mut ao = vec![0.0f32; wu * hu];
    for (i, p) in px.iter().enumerate() {
        let depth_mm = (blurred[i] - p.height).max(0.0) * 1000.0;
        ao[i] = (p.ao * (1.0 - cavity * depth_mm)).clamp(0.05, 1.0);
        albedo.extend_from_slice(&[
            to_srgb8(p.albedo.x),
            to_srgb8(p.albedo.y),
            to_srgb8(p.albedo.z),
            to8(p.alpha),
        ]);
    }
    let (ow, oh) = ((w / 2).max(1) as usize, (h / 2).max(1) as usize);
    let mut orm = Vec::with_capacity(ow * oh * 4);
    for y in 0..oh {
        for x in 0..ow {
            let mut s = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((2 * x + dx).min(wu - 1), (2 * y + dy).min(hu - 1));
                let i = sy * wu + sx;
                s[0] += ao[i];
                s[1] += px[i].rough;
                s[2] += px[i].metal;
            }
            orm.extend_from_slice(&[to8(s[0] / 4.0), to8(s[1] / 4.0), to8(s[2] / 4.0), 255]);
        }
    }
    TexSet {
        albedo: Image::new(w, h, albedo, true),
        normal: Image::new(w, h, normal, false),
        orm: Image::new(ow as u32, oh as u32, orm, false),
    }
}

fn uv_of(x: u32, y: u32, w: u32, h: u32) -> (f32, f32) {
    ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
}

// ---------------------------------------------------------------------------------------
// Wood

struct Knot {
    x: f32,
    y: f32,
    r: f32,
}

const RINGS_PER_M: f32 = 150.0;

fn ring_profile(phase: f32, seed: u32) -> (f32, f32) {
    let t = phase.rem_euclid(1.0);
    let late = smoothstep(0.55, 0.9, t) * (1.0 - smoothstep(0.96, 1.0, t));
    let ring = rand01(phase.floor() as i32, 7, seed);
    (late, ring)
}

fn long_grain(u: f32, v: f32, knots: &[Knot]) -> Px {
    let xm = u / WOOD_GRAIN_U * WOOD_GRAIN_W_M;
    let ym = v * WOOD_GRAIN_H_M;
    // A tangentially sawn board: rings are the intersection of the log's cones with the
    // board's plane, so they read as cathedral arches where the distance to the pith changes.
    let warp = fbm(u, v, (3, 2), 4, 0.5, 11) * 0.006;
    let cut = 0.2
        + 0.035 * (TAU * v).sin()
        + 0.012 * (TAU * 2.0 * v + 1.7).sin()
        + 0.008 * fbm(u, v, (2, 1), 2, 0.5, 5);
    let cx = 0.18 + 0.05 * (TAU * v + 0.5).sin() + 0.02 * fbm(u, v, (1, 2), 2, 0.5, 6);
    let dx = xm - cx + warp;
    let mut r = (dx * dx + cut * cut).sqrt();
    let mut core = 0.0f32;
    let mut core_rings = 0.0f32;
    let mut rim = 0.0f32;
    for k in knots {
        let kx = xm - k.x;
        let mut ky = ym - k.y;
        ky -= (ky / WOOD_GRAIN_H_M).round() * WOOD_GRAIN_H_M;
        let e = (kx * kx + (ky * 0.4).powi(2)).sqrt();
        r += k.r * 0.5 * (-(e / (k.r * 1.8)).powi(2)).exp();
        let dc = (kx * kx + ky * ky).sqrt() / k.r;
        let c = 1.0 - smoothstep(0.8, 1.05, dc);
        rim = rim.max(1.0 - smoothstep(0.0, 0.12, (dc - 1.0).abs()));
        if c > core {
            core = c;
            core_rings = (0.5 + 0.5 * (dc * 7.0 * PI).cos()) * c;
        }
    }
    // Rings vary in width from year to year.
    let phase = r * RINGS_PER_M
        + 1.2 * fbm(u, v, (3, 2), 3, 0.5, 21)
        + 0.25 * fbm(u, v, (16, 4), 2, 0.5, 22);
    let (late, ring) = ring_profile(phase, 3);
    let fibers = fbm(u, v, (240, 8), 3, 0.6, 31) + 0.5 * fbm(u, v, (60, 3), 2, 0.5, 32);
    let pores = gnoise(u * 700.0, v * 60.0, 700, 60, 41);
    // Checks: a few long thin cracks along the grain.
    let mut crack = 0.0f32;
    for c in 0..6u32 {
        let x0 = 0.04 + rand01(c as i32, 1, 77) * 0.28;
        let y0 = rand01(c as i32, 2, 77) * WOOD_GRAIN_H_M;
        let len = 0.15 + rand01(c as i32, 3, 77) * 0.45;
        let width = 0.0004 + rand01(c as i32, 4, 77) * 0.0007;
        let mut dy = ym - y0;
        dy -= (dy / WOOD_GRAIN_H_M).round() * WOOD_GRAIN_H_M;
        let env = 1.0 - smoothstep(len * 0.25, len * 0.5, dy.abs());
        if env <= 0.0 {
            continue;
        }
        let line = x0 + 0.0025 * gnoise(v * 24.0, c as f32 * 3.1, 24, 64, 9);
        let d = (xm - line).abs();
        crack = crack.max((1.0 - smoothstep(width * env * 0.3, width * env, d)) * env);
    }
    let weather = 0.5 + 0.5 * fbm(u, v, (2, 3), 4, 0.55, 51);
    let dirt = 0.5 + 0.5 * fbm(u, v, (4, 6), 4, 0.5, 61);

    let early = srgb(0.68, 0.54, 0.39);
    let latec = srgb(0.56, 0.43, 0.29);
    let mut col = early.lerp(latec, late * (0.6 + 0.4 * ring));
    col *= 0.93 + 0.1 * fibers + 0.03 * pores;
    let knot_col = srgb(0.46, 0.30, 0.17).lerp(srgb(0.36, 0.22, 0.12), core_rings);
    col = col.lerp(knot_col, core * 0.8);
    col = col.lerp(srgb(0.22, 0.14, 0.08), rim * 0.55);
    // Weathered timber turns silver where it is exposed.
    let lum = col.dot(Vec3::new(0.3, 0.55, 0.15));
    col = col.lerp(
        Vec3::new(lum * 1.02, lum, lum * 0.97),
        0.18 + 0.27 * weather,
    );
    col *= 0.86 + 0.14 * dirt;
    col = col.lerp(srgb(0.10, 0.07, 0.05), crack * 0.85);

    Px {
        albedo: col,
        height: late * 0.00035 + fibers * 0.00012 + pores * 0.00003 - crack * 0.0025
            + core * 0.0002,
        rough: (0.7 + 0.14 * (1.0 - late) + 0.06 * fibers + 0.1 * crack - 0.08 * core)
            .clamp(0.3, 1.0),
        metal: 0.0,
        ao: 1.0 - crack * 0.6,
        alpha: 1.0,
    }
}

fn end_grain(a: f32, b: f32, u: f32, v: f32) -> Px {
    // Pith off to one side: a beam sawn next to the heart.
    let pith = Vec2::new(-0.06, -0.16);
    let d = Vec2::new(a, b) - pith;
    let r = d.length() + 0.004 * fbm(u, v, (6, 6), 3, 0.5, 71);
    let phase = r * RINGS_PER_M + 0.4 * fbm(u, v, (10, 10), 3, 0.5, 72);
    let (late, ring) = ring_profile(phase, 4);
    let theta = d.y.atan2(d.x);
    // Radial checks.
    let mut check = 0.0f32;
    for (k, ang) in [0.9f32, 1.35, 2.2].iter().enumerate() {
        let dt = (theta - ang + PI).rem_euclid(TAU) - PI;
        let width = 0.0012 * (1.0 - smoothstep(0.05, 0.3, r));
        let lateral = (dt * r).abs();
        let reach = smoothstep(0.03 + 0.04 * k as f32, 0.08 + 0.05 * k as f32, r);
        check = check.max((1.0 - smoothstep(width * 0.3, width, lateral)) * (1.0 - reach));
    }
    let saw = (a * 0.6 + b * 0.8) * 900.0 + 3.0 * fbm(u, v, (8, 8), 2, 0.5, 73);
    let saw = 0.5 + 0.5 * saw.sin();
    let rays = gnoise(theta * 60.0, r * 40.0, 1 << 20, 1 << 20, 74).abs();
    let mut col = srgb(0.62, 0.47, 0.32).lerp(srgb(0.36, 0.23, 0.13), late * (0.6 + 0.4 * ring));
    col *= 0.72 + 0.06 * rays + 0.05 * saw;
    col = col.lerp(srgb(0.08, 0.05, 0.03), check * 0.9);
    Px {
        albedo: col,
        height: late * 0.0002 + saw * 0.00012 - check * 0.002,
        rough: 0.85 + 0.1 * (1.0 - late),
        metal: 0.0,
        ao: 1.0 - 0.5 * check,
        alpha: 1.0,
    }
}

/// The wood atlas (`size`² albedo and normal): long grain with cathedral arches, knots and
/// checks for `u < WOOD_GRAIN_U` (repeats along v), end grain with rings and radial checks in
/// the corner square. Tint it with the material's base color for different species/stains.
pub fn wood(size: u32) -> TexSet {
    let knots: Vec<Knot> = (0..5)
        .map(|k| Knot {
            x: 0.05 + rand01(k, 1, 91) * 0.26,
            y: rand01(k, 2, 91) * WOOD_GRAIN_H_M,
            r: 0.006 + rand01(k, 3, 91) * 0.01,
        })
        .collect();
    let end_v = 1.0 - WOOD_GRAIN_U;
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        if u >= WOOD_GRAIN_U && v < end_v {
            let a = ((u - WOOD_GRAIN_U) / end_v - 0.5) * WOOD_END_M;
            let b = (v / end_v - 0.5) * WOOD_END_M;
            end_grain(a, b, u, v)
        } else {
            long_grain(u, v, &knots)
        }
    });
    let s = size as f32;
    finish(
        size,
        size,
        &px,
        |x, y| {
            let (u, v) = uv_of(x, y, size, size);
            if u >= WOOD_GRAIN_U && v < end_v {
                (WOOD_END_M / end_v / s, WOOD_END_M / end_v / s)
            } else {
                (WOOD_GRAIN_W_M / WOOD_GRAIN_U / s, WOOD_GRAIN_H_M / s)
            }
        },
        0.25,
    )
}

// ---------------------------------------------------------------------------------------
// Metals

/// Short scratches spread over the unit square: (start, end, width in uv).
fn scratches(count: u32, len: (f32, f32), width: (f32, f32), seed: u32) -> Vec<(Vec2, Vec2, f32)> {
    (0..count as i32)
        .map(|i| {
            let a = Vec2::new(rand01(i, 0, seed), rand01(i, 1, seed));
            let ang = rand01(i, 2, seed) * TAU;
            let l = len.0 + (len.1 - len.0) * rand01(i, 3, seed);
            let w = width.0 + (width.1 - width.0) * rand01(i, 4, seed);
            (a, a + Vec2::new(ang.cos(), ang.sin()) * l, w)
        })
        .collect()
}

fn scratch_mask(p: Vec2, lines: &[(Vec2, Vec2, f32)]) -> f32 {
    let mut m = 0.0f32;
    for &(a, b, w) in lines {
        let (d, t) = seg_dist_wrapped(p, a, b);
        if d < w {
            let taper = (t * (1.0 - t) * 4.0).sqrt();
            m = m.max((1.0 - d / w) * taper);
        }
    }
    m
}

/// Weathered painted steel, tile 1 m: chipped paint over steel, rust blooms around chips,
/// rust run-off streaks along v, grime and scratches. One albedo per paint color (linear
/// RGB); the normal and ORM maps are shared by all of them.
pub fn painted_steel(size: u32, paints: &[Vec3]) -> (Vec<Image>, Image, Image) {
    #[derive(Clone, Copy, Default)]
    struct F {
        chip: f32,
        rust: f32,
        bare: f32,
        runoff: f32,
        grime: f32,
        scratch: f32,
        var: f32,
        peel: f32,
    }
    let lines = scratches(50, (0.02, 0.08), (0.0005, 0.0013), 501);
    let fields = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let big = fbm(u, v, (3, 3), 4, 0.55, 511);
        let mid = fbm(u, v, (12, 12), 4, 0.55, 512);
        let fine = fbm(u, v, (48, 48), 3, 0.5, 518);
        // Wear gathers in a few patches with ragged edges; small dings elsewhere.
        let wear = 0.9 * big + 0.45 * mid + 0.18 * fine;
        let ding = worley(u, v, 40, 1.0, 519);
        let ding_on = rand01(ding.id as i32, 0, 520) < 0.03;
        let ding = if ding_on {
            1.0 - smoothstep(0.06, 0.12 + 0.06 * fine, ding.f1)
        } else {
            0.0
        };
        let chip = smoothstep(0.40, 0.43, wear).max(ding);
        let rust = smoothstep(0.27, 0.42, wear).max(ding * 0.8);
        let bare = smoothstep(0.1, 0.3, fbm(u, v, (5, 5), 3, 0.5, 513));
        let streak = fbm(u, v, (28, 2), 3, 0.5, 514);
        let runoff = smoothstep(0.1, 0.45, streak) * smoothstep(0.05, 0.35, big);
        F {
            chip,
            rust,
            bare,
            runoff,
            grime: 0.5 + 0.5 * fbm(u, v, (4, 2), 4, 0.5, 515),
            scratch: scratch_mask(Vec2::new(u, v), &lines),
            var: 0.5 + 0.5 * fbm(u, v, (40, 40), 3, 0.5, 516),
            peel: gnoise(u * 180.0, v * 180.0, 180, 180, 517),
        }
    });
    let rust_a = srgb(0.47, 0.29, 0.16);
    let rust_b = srgb(0.30, 0.18, 0.11);
    let steel = srgb(0.52, 0.52, 0.53);
    let make = |paint: Vec3| -> Vec<Px> {
        fields
            .iter()
            .map(|f| {
                let rusty = rust_a.lerp(rust_b, f.var);
                let mut col = paint * (0.82 + 0.18 * f.grime);
                col = col.lerp(rusty * 1.05, (f.rust - f.chip).max(0.0) * 0.4);
                col = col.lerp(rusty * 0.9, f.runoff * 0.18);
                let bare_metal = f.chip * f.bare;
                col = col.lerp(rusty, f.chip * (1.0 - f.bare));
                col = col.lerp(steel, bare_metal);
                col = col.lerp(steel * 1.1, f.scratch * 0.8);
                let metal = (bare_metal + f.scratch * 0.8).min(1.0);
                let rust_amt = (f.chip * (1.0 - f.bare)).max((f.rust - f.chip).max(0.0) * 0.5);
                Px {
                    albedo: col,
                    height: (1.0 - f.chip) * 0.00018
                        + f.peel * 0.000008
                        + rust_amt * (f.var - 0.5) * 0.00025
                        - f.scratch * 0.00006,
                    rough: (0.45 + 0.2 * f.grime + 0.35 * rust_amt
                        - 0.15 * bare_metal
                        - 0.15 * f.scratch)
                        .clamp(0.2, 1.0),
                    metal,
                    ao: 1.0,
                    alpha: 1.0,
                }
            })
            .collect()
    };
    let tile = 1.0 / size as f32;
    let mut albedos = Vec::new();
    let mut shared = None;
    for (i, &p) in paints.iter().enumerate() {
        let px = make(p);
        let set = finish(size, size, &px, |_, _| (tile, tile), 0.2);
        albedos.push(set.albedo);
        if i == 0 {
            shared = Some((set.normal, set.orm));
        }
    }
    let (normal, orm) = shared.unwrap_or_else(|| {
        let set = finish(
            size,
            size,
            &make(Vec3::splat(0.5)),
            |_, _| (tile, tile),
            0.2,
        );
        (set.normal, set.orm)
    });
    (albedos, normal, orm)
}

/// Hot-dip galvanized steel, tile 0.5 m: spangle crystals, white rust, grime.
pub fn galvanized(size: u32) -> TexSet {
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let warp = Vec2::new(
            fbm(u, v, (6, 6), 2, 0.5, 601),
            fbm(u, v, (6, 6), 2, 0.5, 602),
        ) * 0.01;
        let c = worley(u + warp.x, v + warp.y, 24, 0.9, 603);
        let b = rand01(c.id as i32, 0, 604);
        let ang = rand01(c.id as i32, 1, 604) * PI;
        let dir = Vec2::new(ang.cos(), ang.sin());
        let feather =
            0.5 + 0.5 * (c.delta.dot(dir) * 190.0 + 4.0 * fbm(u, v, (20, 20), 2, 0.5, 605)).sin();
        let edge = smoothstep(0.0, 0.08, c.f2 - c.f1);
        let white = smoothstep(0.3, 0.6, fbm(u, v, (6, 6), 5, 0.55, 606));
        let grime = 0.5 + 0.5 * fbm(u, v, (4, 8), 4, 0.5, 607);
        let mut col = Vec3::splat(0.56 + 0.06 * b + 0.025 * feather) * (0.96 + 0.04 * edge);
        col *= 0.88 + 0.12 * grime;
        col = col.lerp(srgb(0.74, 0.74, 0.72), white * 0.4);
        Px {
            albedo: col,
            height: (edge - 1.0) * 0.00002
                + feather * 0.000005
                + white * 0.00005 * gnoise(u * 300.0, v * 300.0, 300, 300, 608),
            rough: (0.42 + 0.14 * b + 0.12 * (1.0 - grime) + 0.3 * white).clamp(0.3, 1.0),
            metal: 1.0 - white * 0.5,
            ao: 1.0,
            alpha: 1.0,
        }
    });
    let t = 0.5 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.1)
}

/// Sand-cast iron under worn machine paint (color `paint`, linear), tile 0.6 m: casting
/// pits, paint worn through to dark iron, oily grime, rust in the pits.
pub fn cast_iron(size: u32, paint: Vec3) -> TexSet {
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let sand = fbm(u, v, (90, 90), 3, 0.55, 701);
        let pit = worley(u, v, 70, 1.0, 702);
        let pit_on = rand01(pit.id as i32, 0, 703) < 0.12;
        let pits = if pit_on {
            1.0 - smoothstep(0.05, 0.16, pit.f1)
        } else {
            0.0
        };
        let wear = smoothstep(0.42, 0.5, fbm(u, v, (5, 5), 5, 0.6, 704));
        let oil = smoothstep(0.0, 0.5, fbm(u, v, (3, 3), 4, 0.5, 705));
        let grime = 0.5 + 0.5 * fbm(u, v, (8, 8), 3, 0.5, 706);
        let iron = srgb(0.16, 0.16, 0.17);
        let rust = srgb(0.30, 0.19, 0.12);
        let mut col = paint * (0.85 + 0.15 * grime);
        col = col.lerp(iron, wear);
        col = col.lerp(rust, pits * 0.35);
        col *= 1.0 - 0.25 * oil;
        Px {
            albedo: col,
            height: sand * 0.00022 - pits * 0.0003 + (1.0 - wear) * 0.00008,
            rough: (0.58 + 0.15 * grime - 0.2 * oil + 0.2 * pits - 0.1 * wear).clamp(0.2, 1.0),
            metal: wear * 0.9,
            ao: 1.0 - pits * 0.3,
            alpha: 1.0,
        }
    });
    let t = 0.6 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.2)
}

/// Worn metal, tile 0.5 m: a near-white albedo with grime to multiply by the metal's color
/// (brass, gilt, steel), roughness modulation around 1 (multiplied by the material's
/// roughness), fine scratches and small dents.
pub fn worn_metal(size: u32) -> TexSet {
    let lines = scratches(260, (0.01, 0.06), (0.0004, 0.0011), 801);
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let grime = smoothstep(-0.1, 0.5, fbm(u, v, (8, 8), 5, 0.55, 802)) * 0.6;
        let smudge = 0.5 + 0.5 * fbm(u, v, (12, 12), 3, 0.5, 803);
        let s = scratch_mask(Vec2::new(u, v), &lines);
        let dent = worley(u, v, 9, 1.0, 804);
        let dent_h = if rand01(dent.id as i32, 0, 805) < 0.3 {
            -(1.0 - smoothstep(0.0, 0.35, dent.f1)).powi(2)
        } else {
            0.0
        };
        let col = Vec3::splat((0.92 - 0.25 * grime) * (0.95 + 0.05 * smudge) + 0.04 * s);
        Px {
            albedo: col,
            height: -s * 0.00003 + dent_h * 0.0004 + smudge * 0.00001,
            rough: (0.8 + 0.25 * grime + 0.1 * smudge - 0.35 * s).clamp(0.3, 1.0),
            metal: 1.0 - grime * 0.25,
            ao: 1.0,
            alpha: 1.0,
        }
    });
    let t = 0.5 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.05)
}

// ---------------------------------------------------------------------------------------
// Mineral

/// Weathered sandstone, tile 1 m: grains, bedding bands, pits, lichen.
pub fn stone(size: u32) -> TexSet {
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let big = fbm(u, v, (3, 3), 6, 0.55, 901);
        let mid = fbm(u, v, (16, 16), 4, 0.5, 902);
        let grain = gnoise(u * 400.0, v * 400.0, 400, 400, 903);
        // Faint, irregular bedding, with an occasional darker seam.
        let bend = 5.0 * fbm(u, v, (3, 2), 3, 0.5, 904);
        let band = (v * 9.0 * TAU + bend).sin();
        let seam = 1.0 - smoothstep(0.0, 0.06, (v * 3.0 * TAU + bend * 0.6).sin().abs());
        let speck = worley(u, v, 220, 1.0, 909);
        let speck_t = rand01(speck.id as i32, 0, 910);
        let speck_on = (1.0 - smoothstep(0.12, 0.3, speck.f1)) * (speck_t < 0.25) as u8 as f32;
        let pit = worley(u, v, 40, 1.0, 905);
        let pits = if rand01(pit.id as i32, 0, 906) < 0.12 {
            1.0 - smoothstep(0.05, 0.25, pit.f1)
        } else {
            0.0
        };
        // Lichen in a few ragged clumps.
        let ragged = 0.3 * fbm(u, v, (32, 32), 3, 0.5, 911);
        let lichen = smoothstep(0.34, 0.38, fbm(u, v, (4, 4), 4, 0.55, 907) + ragged);
        let lichen2 = smoothstep(0.44, 0.47, fbm(u, v, (6, 6), 4, 0.5, 908) + ragged);
        let mut col = srgb(0.68, 0.61, 0.51).lerp(srgb(0.54, 0.48, 0.40), 0.5 + 0.9 * big);
        col *= 0.95 + 0.035 * band + 0.06 * mid;
        col *= 1.0 - 0.1 * seam;
        col *= 0.95 + 0.1 * grain;
        col = col.lerp(
            if speck_t < 0.12 {
                srgb(0.34, 0.30, 0.26)
            } else {
                srgb(0.80, 0.76, 0.68)
            },
            speck_on * 0.5,
        );
        col = col.lerp(srgb(0.33, 0.30, 0.26), pits * 0.5);
        col = col.lerp(srgb(0.62, 0.63, 0.56), lichen * 0.55);
        col = col.lerp(srgb(0.64, 0.53, 0.32), lichen2 * 0.4);
        Px {
            albedo: col,
            height: big * 0.003 + mid * 0.001 + grain * 0.00012 - pits * 0.0014 - seam * 0.0004
                + (lichen + lichen2) * 0.00015,
            rough: (0.86 + 0.08 * grain - 0.05 * mid).clamp(0.5, 1.0),
            metal: 0.0,
            ao: 1.0 - pits * 0.3,
            alpha: 1.0,
        }
    });
    let t = 1.0 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.12)
}

/// Cast concrete, tile 1.5 m: aggregate, air pores, stains.
pub fn concrete(size: u32) -> TexSet {
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let big = fbm(u, v, (3, 3), 5, 0.55, 1001);
        let agg = worley(u, v, 110, 1.0, 1002);
        let stone = rand01(agg.id as i32, 0, 1003);
        let agg_on = smoothstep(0.32, 0.25, agg.f1) * (stone > 0.55) as u8 as f32;
        let pore = worley(u, v, 150, 1.0, 1004);
        let pores = if rand01(pore.id as i32, 0, 1005) < 0.18 {
            1.0 - smoothstep(0.05, 0.16, pore.f1)
        } else {
            0.0
        };
        let fine = gnoise(u * 500.0, v * 500.0, 500, 500, 1006);
        let stain = smoothstep(0.0, 0.5, fbm(u, v, (2, 4), 4, 0.5, 1007));
        let mut col = Vec3::splat(0.29 + 0.06 * big) * Vec3::new(1.0, 0.985, 0.95);
        col *= 0.95 + 0.07 * fine;
        col = col.lerp(srgb(0.52, 0.5, 0.46) * (0.75 + 0.4 * stone), agg_on * 0.45);
        col *= 1.0 - 0.22 * stain;
        col = col.lerp(Vec3::splat(0.08), pores * 0.8);
        Px {
            albedo: col,
            height: big * 0.0012 + fine * 0.00008 + agg_on * 0.0002 - pores * 0.0012,
            rough: (0.88 + 0.06 * fine - 0.1 * stain).clamp(0.5, 1.0),
            metal: 0.0,
            ao: 1.0 - pores * 0.5,
            alpha: 1.0,
        }
    });
    let t = 1.5 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.15)
}

/// Three-strand laid rope: u goes around the rope (0..1), v along it in rope circumferences.
pub fn rope(size: u32) -> TexSet {
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let q = (u * 3.0 + v * 2.0).rem_euclid(1.0);
        let strand = (1.0 - (2.0 * q - 1.0).powi(2)).max(0.0).sqrt();
        // Fibers: fine across the strands, long along them (periods chosen so both wrap).
        let fibers = gnoise(
            (u * 3.0 + v * 2.0) * 8.0,
            (u * 2.0 - v * 3.0) * 4.0,
            8,
            4,
            1101,
        );
        let fuzz = gnoise(u * 64.0, v * 64.0, 64, 64, 1102);
        let col =
            srgb(0.66, 0.56, 0.40) * (0.55 + 0.45 * strand) * (0.92 + 0.1 * fibers + 0.05 * fuzz);
        Px {
            albedo: col,
            height: strand * 0.002 + fibers * 0.00015,
            rough: 0.9,
            metal: 0.0,
            ao: 0.6 + 0.4 * strand,
            alpha: 1.0,
        }
    });
    let t = 0.03 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.0)
}

// ---------------------------------------------------------------------------------------
// Ground

/// Packed earth with gravel, tile 3 m. Alpha holds a coverage noise (0.55..1) so a masked
/// ground patch can fray into grass at its edge.
pub fn earth(size: u32) -> TexSet {
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        // Low-frequency variation stays faint: the ground mesh adds the large-scale color.
        let big = fbm(u, v, (3, 3), 6, 0.55, 1201);
        let mid = fbm(u, v, (12, 12), 4, 0.5, 1207);
        let damp = smoothstep(-0.05, 0.45, fbm(u, v, (5, 5), 4, 0.5, 1202));
        let fine = gnoise(u * 600.0, v * 600.0, 600, 600, 1203);
        let soil =
            srgb(0.44, 0.38, 0.30).lerp(srgb(0.54, 0.47, 0.38), 0.5 + 0.5 * big + 0.35 * mid);
        let mut col = soil * (0.94 + 0.08 * fine) * (1.0 - 0.2 * damp);
        let mut h = big * 0.003 + mid * 0.0008 + fine * 0.0002;
        let mut rough = 0.94 - 0.08 * damp;
        let mut ao = 1.0;
        // Dried mud cracks in some areas.
        let cr = worley(u, v, 14, 1.0, 1204);
        let crack_area = smoothstep(0.15, 0.35, fbm(u, v, (2, 2), 3, 0.5, 1205));
        let crack = (1.0 - smoothstep(0.0, 0.025, cr.f2 - cr.f1)) * crack_area;
        col *= 1.0 - 0.35 * crack;
        h -= crack * 0.0015;
        // Gravel in three sizes, gathered in drifts, half buried and dusty.
        let drift = smoothstep(-0.25, 0.35, fbm(u, v, (5, 5), 3, 0.5, 1206));
        let layers = [
            (45, 0.018f32, 1210u32, 0.04 + 0.22 * drift),
            (110, 0.008, 1220, 0.1 + 0.35 * drift),
            (260, 0.003, 1240, 0.2 + 0.35 * drift),
        ];
        for (cells, size_m, seed, density) in layers {
            let c = worley(u, v, cells, 1.0, seed);
            let r = |k: i32| rand01(c.id as i32, k, seed + 1);
            if r(0) > density {
                continue;
            }
            let rad = 0.2 + 0.24 * r(1);
            // Squash pebbles into ellipses along a random direction.
            let ang = r(2) * PI;
            let (s, co) = ang.sin_cos();
            let d = Vec2::new(
                c.delta.x * co + c.delta.y * s,
                (-c.delta.x * s + c.delta.y * co) * 1.45,
            );
            let k = 1.0 - (d.length() / rad).min(1.0);
            if k > 0.0 {
                let dome = (1.0 - (1.0 - k) * (1.0 - k)).sqrt();
                let kind = r(5);
                let stone = if kind < 0.3 {
                    srgb(0.53, 0.43, 0.33)
                } else if kind < 0.6 {
                    srgb(0.42, 0.40, 0.37)
                } else if kind < 0.85 {
                    srgb(0.56, 0.53, 0.48)
                } else {
                    srgb(0.30, 0.28, 0.26)
                };
                let pc = stone * (0.8 + 0.3 * r(4));
                // Dust settles on the pebbles' flanks.
                let pc = pc.lerp(soil, 0.25 + 0.35 * (1.0 - dome));
                let cover = smoothstep(0.0, 0.3, k);
                col = col.lerp(pc * (0.85 + 0.2 * dome), cover);
                h = h.max(h * (1.0 - cover) + dome * size_m * 0.5);
                rough = rough * (1.0 - cover) + 0.78 * cover;
                ao *= 1.0 - 0.3 * (1.0 - smoothstep(0.0, 0.35, k)) * cover;
            }
        }
        Px {
            albedo: col,
            alpha: 0.55 + 0.45 * smoothstep(-0.35, 0.35, fbm(u, v, (6, 6), 4, 0.55, 1230)),
            height: h,
            rough,
            metal: 0.0,
            ao,
        }
    });
    let t = 3.0 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.12)
}

/// Short grass seen from above, tile 2 m: thousands of blades over thatch and soil.
pub fn grass(size: u32) -> TexSet {
    const CELLS: i32 = 150;
    let px = par_texels(size, size, |x, y| {
        let (u, v) = uv_of(x, y, size, size);
        let (gx, gy) = (u * CELLS as f32, v * CELLS as f32);
        let (ix, iy) = (gx.floor() as i32, gy.floor() as i32);
        // Dryness varies blade to blade here; the ground mesh adds the large-scale patches.
        let dry = smoothstep(-0.1, 0.45, fbm(u, v, (24, 24), 3, 0.5, 1301));
        let thatch = 0.5 + 0.5 * gnoise(u * 300.0, v * 300.0, 300, 300, 1302);
        let mut col = srgb(0.20, 0.17, 0.09).lerp(srgb(0.33, 0.29, 0.15), thatch);
        let mut top = -1.0f32;
        let mut h = thatch * 0.0015;
        let mut rough = 0.95;
        let mut ao = 0.55 + 0.2 * thatch;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (cx, cy) = (ix + dx, iy + dy);
                let (wx, wy) = (cx.rem_euclid(CELLS), cy.rem_euclid(CELLS));
                for b in 0..2 {
                    let id = (wy * CELLS + wx) * 2 + b;
                    let r = |k: i32| rand01(id, k, 1303);
                    let root = Vec2::new(cx as f32 + r(0), cy as f32 + r(1));
                    let ang = r(2) * TAU;
                    let len = 0.9 + 1.3 * r(3);
                    let dir = Vec2::new(ang.cos(), ang.sin());
                    let tip = root + dir * len;
                    let p = Vec2::new(gx, gy);
                    let ab = tip - root;
                    let t = ((p - root).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                    let d = (p - root - ab * t).length();
                    let width = 0.16 * (1.0 - t * 0.85);
                    if d < width {
                        let layer = r(4) + t * 0.6;
                        if layer > top {
                            top = layer;
                            let across = d / width;
                            let shade = 0.65 + 0.35 * t;
                            let g = srgb(0.20, 0.36, 0.10)
                                .lerp(srgb(0.36, 0.48, 0.14), r(5))
                                .lerp(srgb(0.55, 0.52, 0.26), dry * (0.4 + 0.6 * r(6)));
                            col = g * shade * (1.0 - 0.25 * across * across);
                            h = 0.002 + layer * 0.004 + (1.0 - across * across) * 0.0006;
                            rough = 0.78 + 0.1 * r(7);
                            ao = 0.75 + 0.25 * layer.min(1.0);
                        }
                    }
                }
            }
        }
        Px {
            albedo: col,
            height: h,
            rough,
            metal: 0.0,
            ao,
            alpha: 1.0,
        }
    });
    let t = 2.0 / size as f32;
    finish(size, size, &px, |_, _| (t, t), 0.08)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, set: &TexSet, size: u32) {
        assert_eq!(
            (set.albedo.width, set.albedo.height),
            (size, size),
            "{name}"
        );
        assert_eq!(
            (set.normal.width, set.normal.height),
            (size, size),
            "{name}"
        );
        assert_eq!(
            (set.orm.width, set.orm.height),
            (size / 2, size / 2),
            "{name}"
        );
        assert!(
            set.albedo.srgb && !set.normal.srgb && !set.orm.srgb,
            "{name}"
        );
        assert_eq!(set.albedo.data.len(), (size * size * 4) as usize);
        // Normals point out of the surface and are unit length.
        for t in set.normal.data.as_chunks::<4>().0 {
            let n = Vec3::new(t[0] as f32, t[1] as f32, t[2] as f32) / 127.5 - Vec3::ONE;
            assert!(n.z > 0.0, "{name}: normal {n}");
            assert!((n.length() - 1.0).abs() < 0.03, "{name}: normal length {n}");
        }
        // Albedo stays in a physically plausible range (no pure black or white), and the
        // texture is not flat.
        let lum: Vec<f32> = set
            .albedo
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|t| (t[0] as f32 + t[1] as f32 + t[2] as f32) / 765.0)
            .collect();
        let (lo, hi) = lum
            .iter()
            .fold((1.0f32, 0.0f32), |(a, b), &l| (a.min(l), b.max(l)));
        assert!(lo > 0.02 && hi < 0.98, "{name}: albedo {lo}..{hi}");
        assert!(hi - lo > 0.05, "{name}: flat albedo");
        for t in set.orm.data.as_chunks::<4>().0 {
            assert!(t[1] >= 40, "{name}: roughness {} too glossy", t[1]);
            assert!(t[0] > 0, "{name}: black occlusion");
        }
    }

    #[test]
    fn textures_have_the_right_sizes_and_ranges() {
        let n = 64;
        check("wood", &wood(n), n);
        check("galvanized", &galvanized(n), n);
        check("cast iron", &cast_iron(n, Vec3::new(0.05, 0.08, 0.06)), n);
        check("worn metal", &worn_metal(n), n);
        check("stone", &stone(n), n);
        check("concrete", &concrete(n), n);
        check("rope", &rope(n), n);
        check("earth", &earth(n), n);
        check("grass", &grass(n), n);
        let (albedos, normal, orm) =
            painted_steel(n, &[Vec3::new(0.6, 0.4, 0.02), Vec3::splat(0.1)]);
        assert_eq!(albedos.len(), 2);
        check(
            "paint",
            &TexSet {
                albedo: albedos[0].clone(),
                normal,
                orm,
            },
            n,
        );
        // Earth alpha (coverage for frayed edges) never drops below the mask cutoff range.
        let e = earth(n);
        assert!(e.albedo.data.as_chunks::<4>().0.iter().all(|t| t[3] >= 138));
    }

    #[test]
    fn noise_tiles_seamlessly() {
        for i in 0..20 {
            let v = i as f32 / 20.0;
            let a = fbm(0.0, v, (4, 4), 4, 0.5, 9);
            let b = fbm(1.0, v, (4, 4), 4, 0.5, 9);
            assert!((a - b).abs() < 1e-4, "{a} {b}");
            let c = worley(0.0, v, 8, 1.0, 3).f1;
            let d = worley(1.0, v, 8, 1.0, 3).f1;
            assert!((c - d).abs() < 1e-4);
        }
        let n = noise3(Vec3::new(0.3, 1.7, -2.2), 1);
        assert!((-1.0..=1.0).contains(&n));
    }
}
