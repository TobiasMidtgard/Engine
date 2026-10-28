//! Scenery for the foliage showcase: the ground the plants stand on, rocks, forest debris and
//! pond reeds.
//!
//! - **Ground** ([`GroundSurface`], built from a [`GroundCover`]): a layered material drawn by
//!   [`SceneryFeature`]. Nine procedural, tileable texture sets (humus, broadleaf litter, pine
//!   needles, moss, packed earth, ash and charcoal, wet mud, rock, mineral crust; each albedo +
//!   height, normal, roughness and occlusion) are height-blended per pixel from eight weights
//!   per vertex. The weights come from the ecology: soil water and minerals, the crowns above
//!   (needles under conifers, leaves under oaks and birches), the trail, burn scars, pools and
//!   rocks. Two decorrelated samples per layer and world-space macro variation hide the tiling;
//!   wet ground darkens and gets glossy, and low spots fill with puddles.
//! - **Rocks** ([`Rock`]): noise-displaced, faceted procedural boulders and stones (flat
//!   bottoms, half buried), drawn with triplanar rock texture, moss on top faces, lichen, soil
//!   creeping up the base and a wet line where they stand in water. They cast shadows.
//! - **Debris** (scattered by [`build_scenery`] from the grown plants): twigs, fallen branches,
//!   pine and spruce cones under conifers, fallen leaf clumps under broadleaf trees, pebbles;
//!   and reeds and cattails around pond shores. Shared meshes and materials, so they draw in a
//!   few instanced batches.
//!
//! All of it is procedural: no files are loaded.

use std::collections::{HashMap, HashSet};
use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};
use engine_core::math::hash_u64;
use engine_core::math::noise::{fbm2, fbm3, perlin2};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::shadows::{MAX_CASCADES, SKY_OCCLUSION_VIEW};
use engine_render::wgpu;
use engine_render::{
    AlphaMode, FeatureStats, FrameContext, GpuShared, GrowBuffer, Image, Mesh, MeshRenderer,
    RenderFeature, RenderPhase, RenderSettings, StandardMaterial, ViewData,
};
use engine_world::Ground;

use crate::demo::{CoverStyle, GroundCover, polyline_distance};
use crate::ecology::RegionState;
use crate::loading::FoliageLoading;
use crate::meshgen::MeshBuilder;
use crate::plant::EcologyRegion;
use crate::plant::{Plant, PlantStage};
use crate::sim::Soil;
use crate::species::{PlantForm, PlantLibrary, srgb};

// ---------------------------------------------------------------- small helpers

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn to_unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ---------------------------------------------------------------- tileable noise

fn lattice(x: i32, y: i32, period: i32, seed: u64) -> u64 {
    let (x, y) = (x.rem_euclid(period) as u64, y.rem_euclid(period) as u64);
    hash_u64((x | (y << 21) | ((period as u64) << 42)) ^ hash_u64(seed))
}

/// Periodic gradient noise in about `[-1, 1]`. `u`, `v` are tile coordinates (the pattern
/// repeats every 1.0); `period` is the number of lattice cells per tile.
pub fn tile_noise(u: f32, v: f32, period: i32, seed: u64) -> f32 {
    let (x, y) = (u * period as f32, v * period as f32);
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (xf, yf) = (x - xi as f32, y - yi as f32);
    let g = |dx: i32, dy: i32| {
        let h = lattice(xi + dx, yi + dy, period, seed);
        let a = (h >> 40) as f32 / (1u64 << 24) as f32 * TAU;
        a.cos() * (xf - dx as f32) + a.sin() * (yf - dy as f32)
    };
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (fu, fv) = (fade(xf), fade(yf));
    let a = g(0, 0) + (g(1, 0) - g(0, 0)) * fu;
    let b = g(0, 1) + (g(1, 1) - g(0, 1)) * fu;
    (a + (b - a) * fv) * 1.41
}

/// Periodic fractal noise in about `[-1, 1]`: `octaves` layers of [`tile_noise`], each twice
/// the frequency and half the amplitude of the previous.
pub fn tile_fbm(u: f32, v: f32, period: i32, octaves: u32, seed: u64) -> f32 {
    let (mut sum, mut amp, mut norm, mut p) = (0.0, 1.0, 0.0, period);
    for i in 0..octaves {
        sum += amp * tile_noise(u, v, p, seed.wrapping_add(i as u64 * 7919));
        norm += amp;
        amp *= 0.5;
        p *= 2;
    }
    sum / norm
}

/// Periodic cellular noise: distance to the nearest and second-nearest feature point (in
/// cells) and a hash of the nearest cell.
pub fn tile_cells(u: f32, v: f32, period: i32, seed: u64) -> (f32, f32, u64) {
    let (x, y) = (u * period as f32, v * period as f32);
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2, mut id) = (9.0f32, 9.0f32, 0u64);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let h = lattice(xi + dx, yi + dy, period, seed);
            let px = (xi + dx) as f32 + (h & 0xffff) as f32 / 65536.0;
            let py = (yi + dy) as f32 + ((h >> 16) & 0xffff) as f32 / 65536.0;
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
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

fn unit(h: u64, salt: u64) -> f32 {
    (hash_u64(h ^ salt) >> 40) as f32 / (1u64 << 24) as f32
}

// ---------------------------------------------------------------- ground layers

/// One material of the ground. The texture array holds them in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GroundLayer {
    /// Dark forest soil: crumbs, rootlets, decomposed leaf bits.
    Humus,
    /// Fallen oak, birch and hazel leaves.
    Litter,
    /// Pine and spruce needle duff.
    Needles,
    /// Moss cushions.
    Moss,
    /// Packed earth with gravel, cracks and footprints (trails, beaches).
    Earth,
    /// Ash, charcoal and charred twigs.
    Ash,
    /// Wet mud with tracks.
    Mud,
    /// Granite-like rock (boulders, outcrops, the cave floor).
    Rock,
    /// Pale travertine mineral crust.
    Crust,
}

/// Number of [`GroundLayer`]s.
pub const LAYER_COUNT: usize = 9;

/// Texels per side of each ground layer texture (512: 9 layers × 2 textures ≈ 25 MB with mips).
pub const LAYER_SIZE: usize = 512;

impl GroundLayer {
    pub const ALL: [GroundLayer; LAYER_COUNT] = [
        GroundLayer::Humus,
        GroundLayer::Litter,
        GroundLayer::Needles,
        GroundLayer::Moss,
        GroundLayer::Earth,
        GroundLayer::Ash,
        GroundLayer::Mud,
        GroundLayer::Rock,
        GroundLayer::Crust,
    ];

    /// Meters covered by one repeat of the texture.
    pub fn tile_m(self) -> f32 {
        match self {
            GroundLayer::Humus => 2.0,
            GroundLayer::Litter => 2.2,
            GroundLayer::Needles => 1.5,
            GroundLayer::Moss => 1.8,
            GroundLayer::Earth => 2.4,
            GroundLayer::Ash => 2.0,
            GroundLayer::Mud => 2.2,
            GroundLayer::Rock => 1.8,
            GroundLayer::Crust => 1.6,
        }
    }
}

/// A texel of the painter: height (m), linear albedo, roughness, baked occlusion.
#[derive(Debug, Clone, Copy)]
struct Texel {
    h: f32,
    c: [f32; 3],
    r: f32,
    ao: f32,
}

/// A tileable material canvas. Everything painted wraps around the edges.
struct Painter {
    n: i32,
    texel_m: f32,
    t: Vec<Texel>,
}

impl Painter {
    fn new(n: usize, tile_m: f32) -> Self {
        Self {
            n: n as i32,
            texel_m: tile_m / n as f32,
            t: vec![
                Texel {
                    h: 0.0,
                    c: [0.2; 3],
                    r: 0.9,
                    ao: 1.0,
                };
                n * n
            ],
        }
    }

    fn idx(&self, x: i32, y: i32) -> usize {
        (y.rem_euclid(self.n) * self.n + x.rem_euclid(self.n)) as usize
    }

    /// Meters to texels.
    fn px(&self, m: f32) -> f32 {
        m / self.texel_m
    }

    fn size(&self) -> f32 {
        self.n as f32
    }

    /// Set every texel from its tile coordinates: `f(u, v) -> (color, height m, roughness)`.
    fn fill(&mut self, f: impl Fn(f32, f32) -> ([f32; 3], f32, f32)) {
        let n = self.n;
        for y in 0..n {
            for x in 0..n {
                let (u, v) = ((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
                let (c, h, r) = f(u, v);
                let i = self.idx(x, y);
                self.t[i] = Texel { h, c, r, ao: 1.0 };
            }
        }
    }

    fn height_at(&self, x: f32, y: f32) -> f32 {
        self.t[self.idx(x.floor() as i32, y.floor() as i32)].h
    }

    /// Visit the texels within `r` texels of (cx, cy): `f(dx, dy, texel)`.
    fn around(&mut self, cx: f32, cy: f32, r: f32, mut f: impl FnMut(f32, f32, &mut Texel)) {
        let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
        let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let i = self.idx(x, y);
                f(dx, dy, &mut self.t[i]);
            }
        }
    }

    /// Lay a shape on the canvas. `f(dx, dy)` gives (coverage 0..1, height above `base` m,
    /// color, roughness); the shape shows where it lies above what is already there.
    fn stamp(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        base: f32,
        mut f: impl FnMut(f32, f32) -> Option<(f32, f32, [f32; 3], f32)>,
    ) {
        self.around(cx, cy, r, |dx, dy, t| {
            let Some((cov, dh, c, rough)) = f(dx, dy) else {
                return;
            };
            let h = base + dh;
            if cov <= 0.0 || h <= t.h {
                return;
            }
            let cov = cov.min(1.0);
            t.h += (h - t.h) * cov;
            t.c = mix3(t.c, c, cov);
            t.r += (rough - t.r) * cov;
        });
    }

    /// A round lump (pebble, crumb, ash fleck): radius `r_m`, raised `lift_m` above the
    /// surface under its center.
    fn lump(&mut self, cx: f32, cy: f32, r_m: f32, lift_m: f32, c: [f32; 3], rough: f32) {
        let r = self.px(r_m).max(0.6);
        let base = self.height_at(cx, cy);
        self.stamp(cx, cy, r + 1.0, base, |dx, dy| {
            let d = (dx * dx + dy * dy).sqrt();
            if d > r + 0.5 {
                return None;
            }
            let q = (d / r).min(1.0);
            let shade = 1.0 - 0.25 * q * q;
            Some((
                (r + 0.5 - d).clamp(0.0, 1.0),
                lift_m * (1.0 - q * q).max(0.0).sqrt(),
                scale3(c, shade),
                rough,
            ))
        });
    }

    /// A stone with an irregular outline, embedded so `lift_m` shows above the surface.
    #[allow(clippy::too_many_arguments)]
    fn stone(
        &mut self,
        cx: f32,
        cy: f32,
        r_m: f32,
        lift_m: f32,
        c: [f32; 3],
        rough: f32,
        seed: u64,
    ) {
        let r = self.px(r_m).max(1.0);
        let base = self.height_at(cx, cy);
        let (stretch, angle) = (0.6 + 0.4 * unit(seed, 1), unit(seed, 2) * TAU);
        let (s, co) = angle.sin_cos();
        self.stamp(cx, cy, r + 2.0, base, |dx, dy| {
            let a = dx * co + dy * s;
            let b = (-dx * s + dy * co) / stretch;
            let ang = b.atan2(a);
            let wob = 1.0 + 0.18 * perlin2(ang.cos() as f64 * 1.3, ang.sin() as f64 * 1.3, seed);
            let d = (a * a + b * b).sqrt() / wob;
            if d > r + 0.5 {
                return None;
            }
            let q = (d / r).min(1.0);
            // Lit from the top-left a little, speckled.
            let speck =
                0.9 + 0.2 * unit(hash_u64(seed ^ (dx as i64 as u64) << 8), dy as i64 as u64);
            let shade = (1.0 - 0.3 * q * q) * speck;
            Some((
                (r + 0.5 - d).clamp(0.0, 1.0),
                lift_m * (1.0 - q.powf(2.5)).max(0.0).sqrt(),
                scale3(c, shade),
                rough,
            ))
        });
    }

    /// A round-section line (needle, twig, rootlet) along `pts` (texels): `half_px` wide,
    /// raised `lift_m`, colored by `color(t)` along its length.
    fn line(
        &mut self,
        pts: &[Vec2],
        half_px: f32,
        lift_m: f32,
        rough: f32,
        color: &dyn Fn(f32) -> [f32; 3],
    ) {
        if pts.len() < 2 {
            return;
        }
        let mid = pts[pts.len() / 2];
        let base = self
            .height_at(mid.x, mid.y)
            .max(self.height_at(pts[0].x, pts[0].y));
        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let mut along = 0.0;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let len2 = ab.length_squared().max(1e-6);
            let seg = ab.length();
            let min = a.min(b) - Vec2::splat(half_px + 1.5);
            let max = a.max(b) + Vec2::splat(half_px + 1.5);
            for y in min.y.floor() as i32..=max.y.ceil() as i32 {
                for x in min.x.floor() as i32..=max.x.ceil() as i32 {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                    let d = p.distance(a + ab * t);
                    let half = half_px.max(0.35);
                    if d > half + 0.5 {
                        continue;
                    }
                    let cov = (half + 0.5 - d).clamp(0.0, 1.0) * (half_px / 0.5).min(1.0);
                    let q = (d / half).min(1.0);
                    let h = base + lift_m * (0.35 + 0.65 * (1.0 - q * q).max(0.0).sqrt());
                    let i = self.idx(x, y);
                    let tex = &mut self.t[i];
                    if h <= tex.h {
                        continue;
                    }
                    let c = color((along + t * seg) / total.max(1e-3));
                    let c = scale3(c, 1.0 - 0.3 * q * q);
                    tex.h += (h - tex.h) * cov;
                    tex.c = mix3(tex.c, c, cov);
                    tex.r += (rough - tex.r) * cov;
                }
            }
            along += seg;
        }
    }

    /// A gently curved path of `segments` from `start` heading `angle`, `len` texels long.
    fn curve(start: Vec2, angle: f32, len: f32, bend: f32, segments: usize) -> Vec<Vec2> {
        let mut pts = Vec::with_capacity(segments + 1);
        let mut p = start;
        let mut a = angle;
        pts.push(p);
        for _ in 0..segments {
            a += bend / segments as f32;
            p += Vec2::new(a.cos(), a.sin()) * (len / segments as f32);
            pts.push(p);
        }
        pts
    }

    /// A twig: bark-colored, `len_m` long, `width_m` thick, with a lighter broken end.
    #[allow(clippy::too_many_arguments)]
    fn twig(&mut self, rng: &mut Rng, cx: f32, cy: f32, len_m: f32, width_m: f32, bark: [f32; 3]) {
        let angle = rng.range(0.0, TAU);
        let len = self.px(len_m);
        let start = Vec2::new(cx, cy) - Vec2::new(angle.cos(), angle.sin()) * len * 0.5;
        let pts = Self::curve(start, angle, len, rng.range(-0.6, 0.6), 6);
        let wood = srgb(0x9a8462);
        let lift = width_m * 0.5;
        self.line(&pts, self.px(width_m) * 0.5, lift, 0.85, &|t: f32| {
            mix3(bark, wood, smooth(0.93, 1.0, t))
        });
        // A side twig or two.
        for _ in 0..rng.range_i32(0, 3) {
            let k = rng.range_i32(2, 5) as usize;
            let a2 = angle + rng.range(0.4, 0.9) * if rng.chance(0.5) { 1.0 } else { -1.0 };
            let side = Self::curve(pts[k], a2, len * rng.range(0.15, 0.35), 0.3, 3);
            self.line(&side, self.px(width_m) * 0.3, lift * 0.6, 0.85, &|_| bark);
        }
    }
}

/// Leaf outlines for litter and 3D leaf clumps.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LeafShape {
    Oak,
    Birch,
    Hazel,
    Beech,
}

impl LeafShape {
    /// Half width at `u` (0 base .. 1 tip), in leaf lengths.
    fn half_width(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            LeafShape::Oak => {
                let w = (PI * u.powf(0.85)).sin() * 0.3;
                let lobes = 0.5 + 0.5 * (u * 4.5 * TAU + 1.2).cos();
                w * (1.0 - 0.42 * lobes * smooth(0.08, 0.3, u))
            }
            LeafShape::Birch => {
                let w = (PI * u.powf(0.6)).sin().max(0.0).powf(0.9) * 0.36;
                w * (1.0 - 0.07 * ((u * 26.0 * TAU).sin()).abs())
            }
            LeafShape::Hazel => (PI * u.powf(0.55)).sin().max(0.0) * 0.42,
            LeafShape::Beech => (PI * u).sin().max(0.0).powf(0.8) * 0.3,
        }
    }
}

/// Paint one leaf centered at (cx, cy). `curl` cups it (0 flat .. 1 strongly curled), `decay`
/// eats holes into it and darkens its rim.
#[allow(clippy::too_many_arguments)]
fn paint_leaf(
    p: &mut Painter,
    cx: f32,
    cy: f32,
    angle: f32,
    len_m: f32,
    shape: LeafShape,
    color: [f32; 3],
    curl: f32,
    decay: f32,
    rough: f32,
    seed: u64,
) {
    let len = p.px(len_m);
    let (s, c) = angle.sin_cos();
    let base = p.height_at(cx, cy) + 0.0004;
    let stem = scale3(color, 0.7);
    let thick = 0.0005;
    p.stamp(cx, cy, len * 0.65 + 2.0, base, |dx, dy| {
        let a = dx * c + dy * s;
        let b = -dx * s + dy * c;
        let u = a / len + 0.5;
        if u < 0.0 {
            // The stalk.
            if u > -0.14 && b.abs() < 0.9 {
                return Some((((0.9 - b.abs()) * 1.5).min(1.0), thick * 1.2, stem, rough));
            }
            return None;
        }
        if u > 1.0 {
            return None;
        }
        let hw = shape.half_width(u) * len;
        let edge = hw - b.abs();
        if edge < -0.5 {
            return None;
        }
        let cov = (edge + 0.5).clamp(0.0, 1.0);
        let across = (b / hw.max(0.5)).clamp(-1.0, 1.0);
        let noise = perlin2(
            (a / len * 6.0) as f64 + seed as f64 * 0.001,
            (b / len * 6.0) as f64,
            seed,
        );
        if decay > 0.0 && noise > 0.62 - decay * 0.5 && across.abs() > 0.1 {
            return None;
        }
        let dh = thick + curl * len_m * (0.16 * across * across + 0.05 * (PI * u).sin());
        let midrib = 1.0 + 0.14 * (1.0 - (b.abs() / 0.9).min(1.0));
        let veins = if across.abs() > 0.1 {
            1.0 - 0.08 * ((u * 8.0 + across.abs() * 2.2) * TAU).cos().max(0.0)
        } else {
            1.0
        };
        let rim = 1.0 - (0.15 + 0.35 * decay) * smooth(0.65, 1.0, across.abs());
        let blot = 1.0 - 0.3 * smooth(0.3, 0.75, noise) * (0.3 + decay);
        let shade = midrib * veins * rim * blot * (0.9 + 0.14 * u);
        Some((cov, dh, scale3(color, shade), rough))
    });
}

/// Painted ground layer, ready to upload: RGBA8 albedo (sRGB) + height, and RGBA8 normal XY,
/// roughness and occlusion (linear). Rows top to bottom; +X texels run along world +X, +Y
/// texels along world +Z (the normal's G channel points toward +Z).
#[derive(Debug, Clone)]
pub struct LayerTexture {
    pub albedo_height: Vec<u8>,
    pub normal_rough_ao: Vec<u8>,
}

impl Painter {
    /// Bake normals (`normal_gain` × the true slope), cavity occlusion (crevices `cavity_m`
    /// deep are fully dark) and normalized height into the two textures.
    fn finish(&self, normal_gain: f32, cavity_m: f32, cavity_albedo: f32) -> LayerTexture {
        let n = self.n as usize;
        let heights: Vec<f32> = self.t.iter().map(|t| t.h).collect();
        let blur_small = box_blur(&heights, n, 2);
        let blur_wide = box_blur(&box_blur(&heights, n, 7), n, 7);
        let mut sorted = heights.clone();
        sorted.sort_by(f32::total_cmp);
        let lo = sorted[sorted.len() / 200];
        let hi = sorted[sorted.len() - 1 - sorted.len() / 400];
        let range = (hi - lo).max(1e-5);
        let mut albedo_height = Vec::with_capacity(n * n * 4);
        let mut normal_rough_ao = Vec::with_capacity(n * n * 4);
        for y in 0..self.n {
            for x in 0..self.n {
                let i = self.idx(x, y);
                let t = self.t[i];
                let hx = (self.t[self.idx(x + 1, y)].h - self.t[self.idx(x - 1, y)].h)
                    / (2.0 * self.texel_m);
                let hy = (self.t[self.idx(x, y + 1)].h - self.t[self.idx(x, y - 1)].h)
                    / (2.0 * self.texel_m);
                let nrm = Vec3::new(-hx * normal_gain, -hy * normal_gain, 1.0).normalize();
                let cav = (blur_small[i] - t.h).max(0.0) * 0.6 + (blur_wide[i] - t.h).max(0.0);
                let ao = (1.0 - (cav / cavity_m).min(1.0) * 0.8) * t.ao;
                let c = scale3(t.c, 1.0 - (1.0 - ao) * cavity_albedo);
                albedo_height.extend_from_slice(&[
                    to_srgb8(c[0]),
                    to_srgb8(c[1]),
                    to_srgb8(c[2]),
                    to_unorm8((t.h - lo) / range),
                ]);
                normal_rough_ao.extend_from_slice(&[
                    to_unorm8(nrm.x * 0.5 + 0.5),
                    to_unorm8(nrm.y * 0.5 + 0.5),
                    to_unorm8(t.r),
                    to_unorm8(ao),
                ]);
            }
        }
        LayerTexture {
            albedo_height,
            normal_rough_ao,
        }
    }
}

/// Separable box blur with wrap-around.
fn box_blur(src: &[f32], n: usize, r: i32) -> Vec<f32> {
    let w = (2 * r + 1) as f32;
    let ni = n as i32;
    let mut tmp = vec![0.0; n * n];
    for y in 0..ni {
        for x in 0..ni {
            let mut s = 0.0;
            for k in -r..=r {
                s += src[(y * ni + (x + k).rem_euclid(ni)) as usize];
            }
            tmp[(y * ni + x) as usize] = s / w;
        }
    }
    let mut out = vec![0.0; n * n];
    for y in 0..ni {
        for x in 0..ni {
            let mut s = 0.0;
            for k in -r..=r {
                s += tmp[((y + k).rem_euclid(ni) * ni + x) as usize];
            }
            out[(y * ni + x) as usize] = s / w;
        }
    }
    out
}

fn count(per_m2: f32, tile_m: f32) -> usize {
    (per_m2 * tile_m * tile_m) as usize
}

fn jitter(rng: &mut Rng, c: [f32; 3], amount: f32) -> [f32; 3] {
    let l = 1.0 + rng.range(-amount, amount);
    let warm = rng.range(-amount, amount) * 0.5;
    [c[0] * l * (1.0 + warm), c[1] * l, c[2] * l * (1.0 - warm)]
}

fn paint_humus(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Humus.tile_m();
    let mut p = Painter::new(n, tile);
    let (dark, mid, light) = (srgb(0x21180f), srgb(0x33261a), srgb(0x4a3826));
    p.fill(|u, v| {
        let big = tile_fbm(u, v, 3, 4, seed);
        let fine = tile_fbm(u, v, 48, 3, seed ^ 1);
        let h = 0.010 * big + 0.0025 * fine;
        let t = (0.5 + 0.7 * big + 0.5 * fine).clamp(0.0, 1.0);
        let c = mix3(mix3(dark, mid, t), light, smooth(0.2, 0.7, fine) * 0.35);
        (c, h, 0.93)
    });
    let mut rng = Rng::new(seed);
    let s = p.size();
    for _ in 0..count(900.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let r = rng.range(0.0015, 0.005);
        let base = mix3(dark, light, rng.f32());
        let c = jitter(&mut rng, base, 0.15);
        p.lump(cx, cy, r, r * 0.7, c, 0.92);
    }
    let fragments = [0x3a2a1c, 0x4f3a25, 0x2a2018, 0x5a4630, 0x463626];
    for _ in 0..count(160.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let shape = *rng
            .pick(&[LeafShape::Oak, LeafShape::Beech, LeafShape::Birch])
            .unwrap();
        let base = srgb(*rng.pick(&fragments).unwrap());
        let c = jitter(&mut rng, base, 0.12);
        paint_leaf(
            &mut p,
            cx,
            cy,
            rng.range(0.0, TAU),
            rng.range(0.02, 0.055),
            shape,
            c,
            0.05,
            rng.range(0.4, 0.9),
            0.82,
            rng.next_u64(),
        );
    }
    for _ in 0..count(12.0, tile) {
        let start = Vec2::new(rng.f32() * s, rng.f32() * s);
        let len = p.px(rng.range(0.06, 0.25));
        let pts = Painter::curve(start, rng.range(0.0, TAU), len, rng.range(-2.0, 2.0), 10);
        let c = jitter(&mut rng, srgb(0x6a5038), 0.15);
        p.line(&pts, p.px(rng.range(0.0008, 0.002)), 0.0012, 0.85, &|_| c);
    }
    for _ in 0..count(6.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let bark = jitter(&mut rng, srgb(0x4a3c30), 0.2);
        let (len, width) = (rng.range(0.05, 0.22), rng.range(0.002, 0.006));
        p.twig(&mut rng, cx, cy, len, width, bark);
    }
    for _ in 0..count(8.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let c = jitter(&mut rng, srgb(0x5e5a52), 0.15);
        let r = rng.range(0.004, 0.012);
        p.stone(cx, cy, r, r * 0.6, c, 0.7, rng.next_u64());
    }
    p.finish(1.6, 0.006, 0.45)
}

fn paint_litter(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Litter.tile_m();
    let mut p = Painter::new(n, tile);
    let soil = srgb(0x1f170f);
    p.fill(|u, v| {
        let big = tile_fbm(u, v, 3, 3, seed);
        (scale3(soil, 0.8 + 0.3 * big), 0.006 * big, 0.92)
    });
    let mut rng = Rng::new(seed ^ 0x11);
    let s = p.size();
    let oak = [0x6b4a2b, 0x7a5532, 0x5a3d24, 0x8a6236, 0x4a3422, 0x93683a];
    let birch = [0x9a7a3a, 0x8a6a30, 0xa88a48, 0x7a6232, 0xb09048];
    let hazel = [0x7a5a30, 0x6a4e2e, 0x86602f, 0x5e4630];
    let old = [0x5e554a, 0x4a4036, 0x3e3328];
    for _ in 0..count(1050.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let k = rng.f32();
        let (shape, len, hex) = if k < 0.5 {
            (
                LeafShape::Oak,
                rng.range(0.06, 0.11),
                *rng.pick(&oak).unwrap(),
            )
        } else if k < 0.75 {
            (
                LeafShape::Birch,
                rng.range(0.035, 0.055),
                *rng.pick(&birch).unwrap(),
            )
        } else if k < 0.9 {
            (
                LeafShape::Hazel,
                rng.range(0.05, 0.09),
                *rng.pick(&hazel).unwrap(),
            )
        } else {
            (
                LeafShape::Beech,
                rng.range(0.05, 0.08),
                *rng.pick(&old).unwrap(),
            )
        };
        let decay = if rng.chance(0.25) {
            rng.range(0.3, 0.8)
        } else {
            0.0
        };
        let c = jitter(&mut rng, srgb(hex), 0.12);
        paint_leaf(
            &mut p,
            cx,
            cy,
            rng.range(0.0, TAU),
            len,
            shape,
            c,
            rng.range(0.1, 0.9).powi(2),
            decay,
            rng.range(0.62, 0.8),
            rng.next_u64(),
        );
    }
    for _ in 0..count(4.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let bark = jitter(&mut rng, srgb(0x4e4236), 0.2);
        let (len, width) = (rng.range(0.08, 0.3), rng.range(0.003, 0.008));
        p.twig(&mut rng, cx, cy, len, width, bark);
    }
    // Acorns and their cups.
    for _ in 0..count(5.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let c = jitter(&mut rng, srgb(0x6e4c24), 0.1);
        p.stone(cx, cy, 0.008, 0.009, c, 0.45, rng.next_u64());
    }
    p.finish(2.2, 0.008, 0.55)
}

fn paint_needles(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Needles.tile_m();
    let mut p = Painter::new(n, tile);
    let duff = srgb(0x2a1e14);
    p.fill(|u, v| {
        let big = tile_fbm(u, v, 3, 3, seed);
        (scale3(duff, 0.8 + 0.3 * big), 0.005 * big, 0.9)
    });
    let mut rng = Rng::new(seed ^ 0x22);
    let s = p.size();
    let colors = [
        0x8b5a2b, 0x9c6a38, 0x6e4a2c, 0x5a4030, 0xa07848, 0x7a6650, 0x83552e,
    ];
    for _ in 0..count(4200.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let spruce = rng.chance(0.3);
        let len = if spruce {
            rng.range(0.012, 0.022)
        } else {
            rng.range(0.045, 0.09)
        };
        let base = srgb(*rng.pick(&colors).unwrap());
        let c = jitter(&mut rng, base, 0.12);
        let angle = rng.range(0.0, TAU);
        let width = if spruce { 0.0012 } else { 0.0015 };
        let half = (p.px(width) * 0.5).max(0.45);
        // Pine needles come in pairs from one sheath.
        let pair = if spruce { 1 } else { 2 };
        let start = Vec2::new(cx, cy);
        for k in 0..pair {
            let a = angle + k as f32 * rng.range(0.05, 0.25);
            let pts = Painter::curve(start, a, p.px(len), rng.range(-0.3, 0.3), 3);
            let tip = scale3(c, 0.8);
            p.line(&pts, half, width * 0.6, 0.72, &|t: f32| mix3(c, tip, t));
        }
    }
    // Cone scales and bits of cone.
    for _ in 0..count(25.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let c = jitter(&mut rng, srgb(0x5a3a20), 0.15);
        paint_leaf(
            &mut p,
            cx,
            cy,
            rng.range(0.0, TAU),
            rng.range(0.012, 0.02),
            LeafShape::Hazel,
            c,
            0.6,
            0.0,
            0.75,
            rng.next_u64(),
        );
    }
    for _ in 0..count(5.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let bark = jitter(&mut rng, srgb(0x4a3a2c), 0.2);
        let (len, width) = (rng.range(0.06, 0.25), rng.range(0.003, 0.007));
        p.twig(&mut rng, cx, cy, len, width, bark);
    }
    p.finish(2.0, 0.006, 0.55)
}

fn paint_moss(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Moss.tile_m();
    let mut p = Painter::new(n, tile);
    let greens = [
        0x4a6a22, 0x5f7a26, 0x3e5a20, 0x6f7e30, 0x56622a, 0x4e6a2a, 0x667024,
    ];
    let crevice = srgb(0x202a16);
    let tips = srgb(0x8f9c3c);
    p.fill(|u, v| {
        // A carpet that swells over the ground, made of small warped cushions and tufts
        // whose outlines blur into each other.
        let wu = (u + 0.05 * tile_fbm(u, v, 5, 2, seed ^ 11)).rem_euclid(1.0);
        let wv = (v + 0.05 * tile_fbm(u, v, 5, 2, seed ^ 12)).rem_euclid(1.0);
        let swell = tile_fbm(u, v, 3, 3, seed ^ 13);
        let (f1, f2, id) = tile_cells(wu, wv, 11, seed);
        let (g1, _, id2) = tile_cells(wu, wv, 29, seed ^ 7);
        let cushion = (1.0 - (f1 / 0.95).powi(2)).max(0.0).sqrt();
        let tuft = (1.0 - (g1 / 0.9).powi(2)).max(0.0).sqrt();
        let seam = smooth(0.0, 0.35, f2 - f1);
        let fuzz = tile_fbm(u, v, 128, 2, seed ^ 3);
        let grain = tile_noise(u, v, 320, seed ^ 4);
        let h = 0.008 * swell
            + (0.005 * cushion * (0.7 + 0.3 * unit(id, 1)) + 0.003 * tuft) * (0.85 + 0.15 * seam)
            + 0.0022 * fuzz
            + 0.0009 * grain;
        let c1 = srgb(greens[(id % greens.len() as u64) as usize]);
        let c2 = srgb(greens[(id2 % greens.len() as u64) as usize]);
        let tone = tile_fbm(u, v, 6, 3, seed ^ 14);
        let mut c = mix3(srgb(0x4a6626), srgb(0x6a7a2a), smooth(-0.5, 0.5, tone));
        c = mix3(c, mix3(c1, c2, 0.4), 0.35);
        let dead = smooth(0.3, 0.65, tile_fbm(u, v, 4, 3, seed ^ 9));
        c = mix3(c, srgb(0x6a5a32), dead * 0.5);
        let crest = smooth(-0.1, 0.6, swell + 0.5 * cushion - 0.3);
        c = mix3(c, tips, crest * 0.35 * (1.0 - dead));
        let lit = 0.8 + 0.3 * (0.5 + 0.5 * fuzz) + 0.14 * grain;
        c = scale3(c, lit);
        c = mix3(crevice, c, 0.84 + 0.16 * seam);
        (c, h, 0.95)
    });
    let mut rng = Rng::new(seed ^ 0x33);
    let s = p.size();
    // Shoots: short, light strokes give the carpet its fibrous look.
    for _ in 0..count(2600.0, tile) {
        let start = Vec2::new(rng.f32() * s, rng.f32() * s);
        let len = p.px(rng.range(0.003, 0.007));
        let pts = Painter::curve(start, rng.range(0.0, TAU), len, rng.range(-0.8, 0.8), 2);
        let base = srgb(*rng.pick(&[0x7a8c34, 0x6a8030, 0x94a046, 0x5a7228]).unwrap());
        let c = jitter(&mut rng, base, 0.12);
        p.line(&pts, 0.45, 0.0012, 0.9, &|_| c);
    }
    // Spore capsules on their stalks, seen from above.
    for _ in 0..count(120.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let c = jitter(&mut rng, srgb(0x7a4a26), 0.2);
        p.lump(cx, cy, 0.0012, 0.004, c, 0.6);
    }
    p.finish(1.5, 0.008, 0.6)
}

fn paint_earth(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Earth.tile_m();
    let mut p = Painter::new(n, tile);
    let (dark, base, light) = (srgb(0x33271c), srgb(0x524030), srgb(0x7a684f));
    p.fill(|u, v| {
        let big = tile_fbm(u, v, 3, 4, seed);
        let mid = tile_fbm(u, v, 16, 3, seed ^ 6);
        let grit = tile_fbm(u, v, 128, 2, seed ^ 1);
        let (f1, f2, _) = tile_cells(u, v, 9, seed ^ 2);
        // Hairline cracks only where it has dried out.
        let dry = smooth(0.1, 0.5, tile_fbm(u, v, 3, 2, seed ^ 5));
        let crack = (1.0 - smooth(0.0, 0.03, f2 - f1)) * dry;
        let h = 0.005 * big + 0.0014 * mid + 0.0005 * grit - 0.0012 * crack - 0.0004 * f1;
        let mut c = mix3(dark, base, smooth(-0.6, 0.3, big + 0.3 * mid));
        c = mix3(
            c,
            light,
            smooth(0.05, 0.7, big + 0.4 * mid) * 0.45 * (0.4 + 0.6 * dry),
        );
        // Damp, darker patches where the ground stays in shade.
        let damp = smooth(0.15, 0.55, tile_fbm(u, v, 5, 3, seed ^ 7) - 0.3 * big);
        c = scale3(
            c,
            (1.0 + 0.14 * grit) * (1.0 - 0.3 * crack) * (1.0 - 0.28 * damp),
        );
        (c, h, 0.84 - 0.06 * (1.0 - dry) - 0.12 * damp)
    });
    let mut rng = Rng::new(seed ^ 0x44);
    let s = p.size();
    // Roots worn bare by feet, half sunk in the earth.
    for _ in 0..3 {
        let start = Vec2::new(rng.f32() * s, rng.f32() * s);
        let len = p.px(rng.range(0.6, 1.2));
        let pts = Painter::curve(start, rng.range(0.0, TAU), len, rng.range(-1.2, 1.2), 14);
        let width = rng.range(0.012, 0.03);
        let bark = jitter(&mut rng, srgb(0x4a3a2a), 0.12);
        let worn = srgb(0x7a6448);
        p.line(&pts, p.px(width) * 0.5, width * 0.35, 0.7, &|t: f32| {
            mix3(bark, worn, 0.3 + 0.3 * (t * 23.0).sin().abs())
        });
        let k = pts.len() / 2;
        let side = Painter::curve(pts[k], rng.range(0.0, TAU), len * 0.3, 0.6, 6);
        p.line(&side, p.px(width) * 0.25, width * 0.2, 0.75, &|_| bark);
    }
    // Boot prints pressed into the earth.
    for _ in 0..3 {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let angle = rng.range(0.0, TAU);
        let (sn, cs) = angle.sin_cos();
        let (l, w) = (p.px(0.29), p.px(0.105));
        p.around(cx, cy, l * 0.6 + 2.0, |dx, dy, t| {
            let a = (dx * cs + dy * sn) / l;
            let b = (-dx * sn + dy * cs) / w;
            let sole = (a / 0.3).powi(2) + (b / 0.5).powi(2);
            let heel = ((a + 0.36) / 0.13).powi(2) + (b / 0.42).powi(2);
            let inside = if a > -0.2 { sole } else { heel };
            if inside > 1.25 {
                return;
            }
            let depth = smooth(1.0, 0.75, inside);
            let tread = 0.5 + 0.5 * (a * 60.0).sin();
            let rim = smooth(0.85, 1.0, inside) * smooth(1.25, 1.0, inside);
            t.h += -0.003 * depth * (0.8 + 0.2 * tread) + 0.001 * rim;
            t.c = scale3(t.c, 1.0 - 0.1 * depth);
            t.r -= 0.08 * depth;
        });
    }
    let leaf_bits = [0x4a3a28, 0x5a4630, 0x3a2e22, 0x6a5232];
    for _ in 0..count(30.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let base = srgb(*rng.pick(&leaf_bits).unwrap());
        let c = jitter(&mut rng, base, 0.1);
        paint_leaf(
            &mut p,
            cx,
            cy,
            rng.range(0.0, TAU),
            rng.range(0.03, 0.07),
            LeafShape::Oak,
            c,
            0.0,
            rng.range(0.3, 0.8),
            0.8,
            rng.next_u64(),
        );
    }
    let gravel = [
        0x8a8478, 0x6e6a62, 0x9a8a72, 0x5a5650, 0x7e705e, 0xa8a090, 0x4e4a44,
    ];
    // Grit and gravel trodden into the surface, a few bigger stones.
    for _ in 0..count(1400.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let r = rng.range(0.001, 0.0028);
        let base = scale3(srgb(*rng.pick(&gravel).unwrap()), 0.7);
        let c = jitter(&mut rng, base, 0.15);
        p.lump(cx, cy, r, r * 0.5, c, 0.6);
    }
    for _ in 0..count(260.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let r = rng.range(0.003, 0.008);
        let base = srgb(*rng.pick(&gravel).unwrap());
        let c = jitter(&mut rng, base, 0.12);
        p.stone(cx, cy, r, r * 0.5, c, rng.range(0.45, 0.7), rng.next_u64());
    }
    for _ in 0..count(28.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let r = rng.range(0.01, 0.032);
        let base = scale3(srgb(*rng.pick(&gravel).unwrap()), 0.85);
        let c = jitter(&mut rng, base, 0.1);
        p.stone(cx, cy, r, r * 0.4, c, 0.6, rng.next_u64());
    }
    p.finish(2.2, 0.005, 0.6)
}

fn paint_ash(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Ash.tile_m();
    let mut p = Painter::new(n, tile);
    let (ash, dark, soil, white) = (
        srgb(0x55524d),
        srgb(0x151413),
        srgb(0x3a2a1e),
        srgb(0x9a9690),
    );
    p.fill(|u, v| {
        let big = tile_fbm(u, v, 3, 4, seed);
        let mid = tile_fbm(u, v, 12, 3, seed ^ 3);
        let fine = tile_fbm(u, v, 96, 2, seed ^ 1);
        let mut c = mix3(dark, ash, smooth(-0.3, 0.6, big + 0.35 * mid + 0.2 * fine));
        // Drifts of pale ash in the hollows the wind left them.
        let drift = smooth(0.25, 0.55, mid - 0.4 * big + 0.15 * fine);
        c = mix3(c, white, drift * 0.75);
        c = mix3(
            c,
            soil,
            smooth(0.3, 0.65, tile_fbm(u, v, 4, 3, seed ^ 2)) * 0.5,
        );
        (
            scale3(c, 1.0 + 0.12 * fine),
            0.005 * big + 0.0015 * mid + 0.0008 * fine + 0.0012 * drift,
            0.95 - 0.1 * (1.0 - drift) * smooth(0.0, -0.5, big),
        )
    });
    let mut rng = Rng::new(seed ^ 0x55);
    let s = p.size();
    for _ in 0..count(25.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let c = jitter(&mut rng, srgb(0x2a2826), 0.15);
        paint_leaf(
            &mut p,
            cx,
            cy,
            rng.range(0.0, TAU),
            rng.range(0.04, 0.09),
            LeafShape::Oak,
            c,
            0.4,
            0.8,
            0.85,
            rng.next_u64(),
        );
    }
    // Charcoal: angular black chunks with a cracked, slightly glossy surface.
    for _ in 0..count(30.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let r = p.px(rng.range(0.005, 0.016));
        let sides = rng.range_i32(4, 7) as f32;
        let rot = rng.range(0.0, TAU);
        let base = p.height_at(cx, cy);
        let lift = rng.range(0.003, 0.008);
        let tint = rng.range(0.7, 1.3);
        let id = rng.next_u64();
        p.stamp(cx, cy, r * 1.3 + 2.0, base, |dx, dy| {
            let ang = dy.atan2(dx) - rot;
            let sector = TAU / sides;
            let k = (ang / sector).floor();
            let local = ang - (k + 0.5) * sector;
            let edge_r = r * (sector * 0.5).cos() / local.cos().max(0.2);
            let d = (dx * dx + dy * dy).sqrt();
            if d > edge_r + 0.5 {
                return None;
            }
            let (f1, f2, _) = tile_cells(
                (dx / r * 0.35 + unit(id, 1)).rem_euclid(1.0),
                (dy / r * 0.35 + unit(id, 2)).rem_euclid(1.0),
                4,
                id,
            );
            let crackle = smooth(0.0, 0.08, f2 - f1);
            let c = scale3(srgb(0x161412), tint * (0.55 + 0.45 * crackle));
            let q = (d / edge_r).min(1.0);
            Some((
                (edge_r + 0.5 - d).clamp(0.0, 1.0),
                lift * (1.0 - q.powi(4)) * (0.85 + 0.15 * crackle),
                c,
                0.38 + 0.25 * (1.0 - crackle),
            ))
        });
    }
    for _ in 0..count(6.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let bark = jitter(&mut rng, srgb(0x141210), 0.2);
        let (len, width) = (rng.range(0.08, 0.3), rng.range(0.004, 0.01));
        p.twig(&mut rng, cx, cy, len, width, bark);
    }
    for _ in 0..count(500.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let base = srgb(*rng.pick(&[0x8f8a84, 0xb0aca6, 0x6e6a66]).unwrap());
        let c = jitter(&mut rng, base, 0.1);
        p.lump(cx, cy, rng.range(0.0008, 0.0025), 0.0008, c, 0.95);
    }
    p.finish(1.6, 0.006, 0.4)
}

fn paint_mud(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Mud.tile_m();
    let mut p = Painter::new(n, tile);
    let (wet, dry) = (srgb(0x2b231a), srgb(0x4c4034));
    p.fill(|u, v| {
        let big = tile_fbm(u, v, 2, 4, seed);
        let warp = tile_fbm(u, v, 5, 2, seed ^ 3);
        let ripple = ((u * 0.8 + v * 0.6) * 38.0 * TAU + warp * 6.0).sin();
        let fine = tile_fbm(u, v, 64, 2, seed ^ 4);
        let h = 0.007 * big + 0.0006 * ripple * smooth(-0.2, 0.4, warp) + 0.0004 * fine;
        let c = mix3(wet, dry, smooth(-0.3, 0.6, big + 0.2 * fine));
        (c, h, 0.6 - 0.15 * smooth(0.0, -0.5, big))
    });
    let mut rng = Rng::new(seed ^ 0x66);
    let s = p.size();
    // Deer tracks: two pointed toes, pressed in with a raised rim.
    let mut walk = Vec2::new(rng.f32() * s, rng.f32() * s);
    let heading = rng.range(0.0, TAU);
    for i in 0..6 {
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        let dir = Vec2::new(heading.cos(), heading.sin());
        let perp = Vec2::new(-dir.y, dir.x);
        let at = walk + perp * p.px(0.07) * side;
        walk += dir * p.px(0.33);
        let (l, w) = (p.px(0.07), p.px(0.025));
        let (sn, cs) = heading.sin_cos();
        p.around(at.x, at.y, l + 2.0, |dx, dy, t| {
            let a = (dx * cs + dy * sn) / l;
            let b = (-dx * sn + dy * cs) / w;
            let toe = |off: f32| {
                let bb = b - off;
                let hw = 0.6 * (1.0 - (a / 0.5).powi(2)).max(0.0).sqrt() * (1.0 - 0.4 * a);
                bb.abs() / hw.max(1e-3)
            };
            let inside = toe(0.75).min(toe(-0.75));
            if inside > 1.4 || a.abs() > 0.6 {
                return;
            }
            let depth = smooth(1.0, 0.6, inside);
            let rim = smooth(0.9, 1.1, inside) * smooth(1.4, 1.1, inside);
            t.h += -0.006 * depth + 0.0015 * rim;
            t.c = scale3(t.c, 1.0 - 0.2 * depth);
            t.r -= 0.15 * depth;
        });
    }
    // Bird tracks.
    for _ in 0..4 {
        let at = Vec2::new(rng.f32() * s, rng.f32() * s);
        let heading = rng.range(0.0, TAU);
        for k in [-0.6f32, 0.0, 0.6] {
            let a = heading + k;
            let tip = at + Vec2::new(a.cos(), a.sin()) * p.px(0.035);
            let (a0, b0) = (at, tip);
            let ab = b0 - a0;
            let len2 = ab.length_squared();
            p.around(at.x, at.y, p.px(0.04) + 2.0, |dx, dy, t| {
                let q = Vec2::new(at.x + dx, at.y + dy);
                let tt = ((q - a0).dot(ab) / len2).clamp(0.0, 1.0);
                let d = q.distance(a0 + ab * tt);
                if d < 0.9 {
                    t.h -= 0.002 * (1.0 - d / 0.9);
                    t.c = scale3(t.c, 0.85);
                }
            });
        }
    }
    let pebbles = [0x6a655c, 0x57534c, 0x7a7266, 0x4e4a44];
    for _ in 0..count(30.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let r = rng.range(0.003, 0.012);
        let base = srgb(*rng.pick(&pebbles).unwrap());
        let c = jitter(&mut rng, base, 0.12);
        p.stone(cx, cy, r, r * 0.35, c, 0.4, rng.next_u64());
    }
    for _ in 0..count(10.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let c = jitter(&mut rng, srgb(0x3a2c1e), 0.15);
        paint_leaf(
            &mut p,
            cx,
            cy,
            rng.range(0.0, TAU),
            rng.range(0.05, 0.1),
            LeafShape::Oak,
            c,
            0.02,
            0.3,
            0.5,
            rng.next_u64(),
        );
    }
    for _ in 0..count(2.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let bark = jitter(&mut rng, srgb(0x3a3028), 0.2);
        let (len, width) = (rng.range(0.08, 0.2), rng.range(0.003, 0.006));
        p.twig(&mut rng, cx, cy, len, width, bark);
    }
    p.finish(1.6, 0.006, 0.4)
}

fn paint_rock(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Rock.tile_m();
    let mut p = Painter::new(n, tile);
    let (grey, light, ochre, crack_c) = (
        srgb(0x6f6b63),
        srgb(0x8b867c),
        srgb(0x8a7258),
        srgb(0x2a2724),
    );
    let (mica, feldspar, pink) = (srgb(0x2c2a27), srgb(0xb4afa4), srgb(0x9c8678));
    p.fill(|u, v| {
        let big = tile_fbm(u, v, 2, 5, seed);
        let ridged = 1.0 - tile_fbm(u, v, 6, 3, seed ^ 1).abs();
        let pit = tile_fbm(u, v, 64, 3, seed ^ 2);
        let (f1, f2, _) = tile_cells(u, v, 4, seed ^ 3);
        let crack =
            (1.0 - smooth(0.0, 0.025, f2 - f1)) * smooth(0.0, 0.45, tile_fbm(u, v, 3, 2, seed ^ 8));
        let (g1, g2, _) = tile_cells(u, v, 13, seed ^ 9);
        let crack2 =
            (1.0 - smooth(0.0, 0.02, g2 - g1)) * smooth(0.25, 0.6, tile_fbm(u, v, 5, 2, seed ^ 10));
        let grain = tile_noise(u, v, 220, seed ^ 4);
        let grain2 = tile_noise(u, v, 160, seed ^ 5);
        let h = 0.006 * big + 0.002 * ridged + 0.0008 * pit - 0.007 * crack - 0.0025 * crack2;
        let mut c = mix3(grey, light, smooth(-0.6, 0.6, big + 0.4 * pit));
        let stain = smooth(0.15, 0.65, tile_fbm(u, v, 3, 3, seed ^ 6));
        c = mix3(c, ochre, stain * 0.35);
        c = mix3(
            c,
            scale3(grey, 0.6),
            smooth(0.2, 0.7, tile_fbm(u, v, 5, 2, seed ^ 7)) * 0.3,
        );
        let mut r = 0.82;
        if grain > 0.55 {
            c = mix3(c, mica, 0.85);
            r = 0.45;
        } else if grain2 > 0.42 {
            c = mix3(c, feldspar, 0.6);
        } else if grain2 < -0.55 {
            c = mix3(c, pink, 0.5);
        }
        c = mix3(c, crack_c, crack.max(crack2 * 0.7));
        (c, h, r + 0.12 * crack)
    });
    p.finish(1.4, 0.006, 0.5)
}

fn paint_crust(n: usize, seed: u64) -> LayerTexture {
    let tile = GroundLayer::Crust.tile_m();
    let mut p = Painter::new(n, tile);
    let (cream, white, mint, ochre) = (
        srgb(0xd2cfbf),
        srgb(0xeceae2),
        srgb(0xa9c4b8),
        srgb(0xc4ae86),
    );
    p.fill(|u, v| {
        let f = (tile_fbm(u, v, 3, 4, seed) * 0.5 + 0.5) * 4.0;
        let level = f.floor();
        let frac = f - level;
        let rim = smooth(0.8, 0.95, frac) * smooth(1.0, 0.96, frac);
        let fine = tile_fbm(u, v, 96, 2, seed ^ 1);
        let h = 0.006 * (level + smooth(0.0, 0.9, frac) * 0.5) + 0.0035 * rim + 0.0005 * fine;
        let pool = smooth(0.5, 0.1, frac);
        let mut c = mix3(cream, mint, pool * 0.75);
        c = mix3(c, white, rim * 0.8);
        c = mix3(
            c,
            ochre,
            smooth(0.2, 0.7, tile_fbm(u, v, 3, 3, seed ^ 2)) * 0.35,
        );
        (
            scale3(c, 1.0 + 0.06 * fine),
            h,
            0.55 - 0.2 * pool + 0.1 * rim,
        )
    });
    let mut rng = Rng::new(seed ^ 0x77);
    let s = p.size();
    for _ in 0..count(700.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        let r = p.px(rng.range(0.0006, 0.0018)).max(0.6);
        p.around(cx, cy, r + 1.0, |dx, dy, t| {
            let d = (dx * dx + dy * dy).sqrt();
            if d < r {
                t.h -= 0.0012 * (1.0 - d / r);
                t.c = scale3(t.c, 0.7);
            }
        });
    }
    for _ in 0..count(300.0, tile) {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        p.lump(cx, cy, 0.0008, 0.0006, white, 0.2);
    }
    p.finish(1.2, 0.004, 0.35)
}

/// Paint one ground layer at `n`×`n` texels.
pub fn paint_layer(layer: GroundLayer, n: usize) -> LayerTexture {
    let seed = 0x6e0d_5eed ^ (layer as u64) << 8;
    match layer {
        GroundLayer::Humus => paint_humus(n, seed),
        GroundLayer::Litter => paint_litter(n, seed),
        GroundLayer::Needles => paint_needles(n, seed),
        GroundLayer::Moss => paint_moss(n, seed),
        GroundLayer::Earth => paint_earth(n, seed),
        GroundLayer::Ash => paint_ash(n, seed),
        GroundLayer::Mud => paint_mud(n, seed),
        GroundLayer::Rock => paint_rock(n, seed),
        GroundLayer::Crust => paint_crust(n, seed),
    }
}

/// Paint every ground layer (in parallel, a second or two in release builds).
pub fn paint_layers(n: usize) -> Vec<LayerTexture> {
    std::thread::scope(|scope| {
        let jobs: Vec<_> = GroundLayer::ALL
            .iter()
            .map(|&layer| scope.spawn(move || paint_layer(layer, n)))
            .collect();
        jobs.into_iter()
            .map(|j| j.join().expect("ground layer painter"))
            .collect()
    })
}

/// Fallen leaves for 3D leaf clumps: a 2×2 atlas (oak, birch, decayed oak, hazel) with alpha.
pub fn fallen_leaf_atlas(size: u32) -> Image {
    let half = size as f32 * 0.5;
    let leaves = [
        (LeafShape::Oak, 0x7a5532u32, 0.0f32),
        (LeafShape::Birch, 0xa08438, 0.0),
        (LeafShape::Oak, 0x4f3a26, 0.5),
        (LeafShape::Hazel, 0x8a6230, 0.0),
    ];
    let mut px = vec![[0.0f32; 4]; (size * size) as usize];
    for (k, (shape, hex, decay)) in leaves.into_iter().enumerate() {
        let (ox, oy) = ((k % 2) as f32 * half, (k / 2) as f32 * half);
        let color = srgb(hex);
        let len = half * 0.86;
        for y in 0..half as u32 {
            for x in 0..half as u32 {
                // Leaf along +Y of its cell, stalk at the bottom.
                let a = (y as f32 + 0.5) - half * 0.5;
                let b = (x as f32 + 0.5) - half * 0.5;
                let u = 0.5 - a / len;
                let (cov, shade) = if u < 0.0 {
                    if u > -0.07 && b.abs() < 1.2 {
                        (1.0, 0.7)
                    } else {
                        (0.0, 1.0)
                    }
                } else if u > 1.0 {
                    (0.0, 1.0)
                } else {
                    let hw = shape.half_width(u) * len;
                    let cov = (hw - b.abs() + 0.5).clamp(0.0, 1.0);
                    let across = (b / hw.max(0.5)).clamp(-1.0, 1.0);
                    let noise = perlin2(x as f64 * 0.09, y as f64 * 0.09, k as u64 + 3);
                    let hole = decay > 0.0 && noise > 0.62 - decay * 0.5 && across.abs() > 0.15;
                    let midrib = 1.0 + 0.15 * (1.0 - (b.abs() / 1.2).min(1.0));
                    let veins = if across.abs() > 0.1 {
                        1.0 - 0.1 * ((u * 8.0 + across.abs() * 2.2) * TAU).cos().max(0.0)
                    } else {
                        1.0
                    };
                    let rim = 1.0 - (0.2 + 0.3 * decay) * smooth(0.7, 1.0, across.abs());
                    let blot = 1.0 - 0.25 * smooth(0.3, 0.75, noise);
                    (
                        if hole { 0.0 } else { cov },
                        midrib * veins * rim * blot * (0.9 + 0.15 * u),
                    )
                };
                let i = ((oy as u32 + y) * size + ox as u32 + x) as usize;
                px[i] = [color[0] * shade, color[1] * shade, color[2] * shade, cov];
            }
        }
    }
    // Transparent texels take the average leaf color so filtering adds no dark fringe.
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for p in &px {
        if p[3] > 0.5 {
            for c in 0..3 {
                sum[c] += p[c];
            }
            n += 1.0;
        }
    }
    let avg = sum.map(|v| v / n.max(1.0));
    let mut data = Vec::with_capacity(px.len() * 4);
    for p in &px {
        let a = p[3];
        let c = [0, 1, 2].map(|i| p[i] * a + avg[i] * (1.0 - a));
        data.extend_from_slice(&[to_srgb8(c[0]), to_srgb8(c[1]), to_srgb8(c[2]), to_unorm8(a)]);
    }
    Image::new(size, size, data, true)
}

// ---------------------------------------------------------------- ground cover

/// A pool of water in a ground cover: its shore sinks toward the water, turns to wet mud with
/// gravel at the waterline (or to mineral crust with a rimstone lip), and stays damp around.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pool {
    /// Center, local `[x, z]` meters.
    pub center: [f32; 2],
    /// Radius of the open water, meters.
    pub radius: f32,
    /// Height of the water surface above the ground, meters.
    pub water_m: f32,
    /// Mineral water: travertine crust and a rimstone lip instead of mud and reeds.
    pub mineral: bool,
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            radius: 4.0,
            water_m: 0.06,
            mineral: false,
        }
    }
}

/// Weight slots of a forest floor.
const SLOT_HUMUS: usize = 0;
const SLOT_LITTER: usize = 1;
const SLOT_NEEDLES: usize = 2;
const SLOT_MOSS: usize = 3;
const SLOT_EARTH: usize = 4;
const SLOT_ASH: usize = 5;
const SLOT_MUD: usize = 6;
const SLOT_ROCK: usize = 7;
/// Weight slots of a cave floor.
const CAVE_ROCK: usize = 0;
const CAVE_CRUST: usize = 1;
const CAVE_MUD: usize = 2;
const CAVE_EARTH: usize = 3;

/// The layer drawn in each of the eight weight slots of a cover style.
pub fn style_slots(style: CoverStyle) -> [GroundLayer; 8] {
    use GroundLayer::*;
    match style {
        CoverStyle::ForestFloor => [Humus, Litter, Needles, Moss, Earth, Ash, Mud, Rock],
        CoverStyle::CaveFloor => [Rock, Crust, Mud, Earth, Humus, Moss, Ash, Needles],
    }
}

/// A tree or shrub as the ground sees it (positions local to the cover).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeSpot {
    pub pos: Vec2,
    /// Crown radius, meters.
    pub crown: f32,
    /// Trunk radius at the base, meters.
    pub trunk: f32,
    /// Needles (true) or leaves under it.
    pub conifer: bool,
    /// Spruce (long cones) rather than pine.
    pub spruce: bool,
    /// How much it sheds: grown, alive trees most; stumps and snags less.
    pub shed: f32,
    /// Shrub rather than tree.
    pub shrub: bool,
}

/// A rock as the ground sees it: soil, moss and shade gather around its base.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RockSpot {
    pub pos: Vec2,
    /// Footprint radius, meters.
    pub radius: f32,
    pub moss: f32,
}

/// Crown cover of broadleaf and conifer trees on a 1 m grid (local to the cover).
pub struct Canopy {
    origin: Vec2,
    w: usize,
    h: usize,
    broad: Vec<f32>,
    conifer: Vec<f32>,
}

impl Canopy {
    /// Rasterize the crowns of `trees` over a cover of `half_extents`.
    pub fn build(half_extents: [f32; 2], trees: &[TreeSpot]) -> Self {
        let origin = Vec2::new(-half_extents[0] - 1.0, -half_extents[1] - 1.0);
        let w = (half_extents[0] * 2.0 + 3.0).ceil() as usize;
        let h = (half_extents[1] * 2.0 + 3.0).ceil() as usize;
        let mut broad = vec![0.0f32; w * h];
        let mut conifer = vec![0.0f32; w * h];
        for t in trees {
            let r = t.crown * 1.15 + 0.8;
            let grid = if t.conifer { &mut conifer } else { &mut broad };
            let (cx, cy) = (t.pos.x - origin.x, t.pos.y - origin.y);
            let (x0, x1) = ((cx - r).floor().max(0.0) as usize, (cx + r).ceil() as usize);
            let (y0, y1) = ((cy - r).floor().max(0.0) as usize, (cy + r).ceil() as usize);
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let d = Vec2::new(x as f32 + 0.5 - cx, y as f32 + 0.5 - cy).length();
                    if d >= r {
                        continue;
                    }
                    let f = (1.0 - d / r).powf(0.7) * t.shed;
                    let c = &mut grid[y * w + x];
                    *c = 1.0 - (1.0 - *c) * (1.0 - f.min(1.0));
                }
            }
        }
        Self {
            origin,
            w,
            h,
            broad,
            conifer,
        }
    }

    /// Broadleaf and conifer crown cover at a local point, 0..1 (bilinear).
    pub fn sample(&self, p: Vec2) -> (f32, f32) {
        let fx = (p.x - self.origin.x - 0.5).clamp(0.0, self.w as f32 - 1.001);
        let fy = (p.y - self.origin.y - 0.5).clamp(0.0, self.h as f32 - 1.001);
        let (x, y) = (fx as usize, fy as usize);
        let (tx, ty) = (fx - x as f32, fy - y as f32);
        let at = |g: &[f32]| {
            let i = y * self.w + x;
            let a = g[i] + (g[i + 1] - g[i]) * tx;
            let b = g[i + self.w] + (g[i + self.w + 1] - g[i + self.w]) * tx;
            a + (b - a) * ty
        };
        (at(&self.broad), at(&self.conifer))
    }
}

/// Points bucketed on a coarse grid for "what is near here" queries.
struct Buckets {
    cell: f32,
    map: HashMap<(i32, i32), Vec<u32>>,
}

impl Buckets {
    fn new(cell: f32, points: impl Iterator<Item = (Vec2, f32)>) -> Self {
        let mut map: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, (p, r)) in points.enumerate() {
            let (x0, x1) = (
                ((p.x - r) / cell).floor() as i32,
                ((p.x + r) / cell).floor() as i32,
            );
            let (y0, y1) = (
                ((p.y - r) / cell).floor() as i32,
                ((p.y + r) / cell).floor() as i32,
            );
            for y in y0..=y1 {
                for x in x0..=x1 {
                    map.entry((x, y)).or_default().push(i as u32);
                }
            }
        }
        Self { cell, map }
    }

    fn near(&self, p: Vec2) -> &[u32] {
        let key = (
            (p.x / self.cell).floor() as i32,
            (p.y / self.cell).floor() as i32,
        );
        self.map.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Everything the look of a ground cover depends on, gathered from the world.
pub struct CoverInputs {
    /// World position of the cover's center.
    pub center: DVec3,
    /// Soil of the ecology regions under it.
    pub soils: Vec<Soil>,
    pub trees: Vec<TreeSpot>,
    pub rocks: Vec<RockSpot>,
    canopy: Option<Canopy>,
    tree_buckets: Option<Buckets>,
    rock_buckets: Option<Buckets>,
}

impl CoverInputs {
    /// Inputs for a cover at `center` (call [`CoverInputs::prepare`] after filling them).
    pub fn new(
        center: DVec3,
        soils: Vec<Soil>,
        trees: Vec<TreeSpot>,
        rocks: Vec<RockSpot>,
    ) -> Self {
        Self {
            center,
            soils,
            trees,
            rocks,
            canopy: None,
            tree_buckets: None,
            rock_buckets: None,
        }
    }

    /// Build the canopy map and lookup grids for `cover`.
    pub fn prepare(&mut self, cover: &GroundCover) {
        self.canopy = Some(Canopy::build(cover.half_extents, &self.trees));
        let trees = self.trees.iter().map(|t| (t.pos, t.trunk + 1.6));
        self.tree_buckets = Some(Buckets::new(4.0, trees));
        let rocks = self.rocks.iter().map(|r| (r.pos, r.radius * 2.0 + 0.5));
        self.rock_buckets = Some(Buckets::new(4.0, rocks));
    }

    /// Soil `[water, nutrients, minerals]` at a world point (bilinear; defaults outside).
    pub fn soil(&self, p: DVec3) -> [f32; 3] {
        let Some(s) = self.soils.iter().find(|s| s.contains(p)) else {
            return [0.4, 0.4, 0.0];
        };
        let fx = ((p.x - s.origin.x) / s.cell as f64 - 0.5).clamp(0.0, s.width as f64 - 1.001);
        let fy = ((p.z - s.origin.y) / s.cell as f64 - 0.5).clamp(0.0, s.height as f64 - 1.001);
        let (x, y) = (fx as usize, fy as usize);
        let (tx, ty) = ((fx - x as f64) as f32, (fy - y as f64) as f32);
        let w = s.width as usize;
        let x1 = (x + 1).min(w - 1);
        let y1 = (y + 1).min(s.height as usize - 1);
        let at = |g: &[f32], extra: Option<&[f32]>| {
            let v = |i: usize| g[i] + extra.map_or(0.0, |e| e[i]);
            let a = v(y * w + x) + (v(y * w + x1) - v(y * w + x)) * tx;
            let b = v(y1 * w + x) + (v(y1 * w + x1) - v(y1 * w + x)) * tx;
            a + (b - a) * ty
        };
        [
            at(&s.water, None),
            at(&s.nutrients, Some(&s.bonus)).min(1.0),
            at(&s.minerals, None),
        ]
    }
}

/// The ground cover at one point: height above the ground, layer weights and extras.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverSample {
    /// Height above the ground (the `Ground` surface), meters. Negative at a round cover's
    /// edge, where it dips under the ground.
    pub height: f32,
    /// Layer weights per slot ([`style_slots`]), summing to 1.
    pub weights: [f32; 8],
    /// 0 dry .. 1 soaked (puddles form above 0.55).
    pub wet: f32,
    /// Contact shade from trunks and rocks, 0..1.
    pub shade: f32,
    /// Trampled (flattened) ground, 0..1.
    pub trample: f32,
    /// Albedo multiplier.
    pub tint: [f32; 3],
}

/// Sample a cover at a local point. `inputs` must be prepared for this cover.
pub fn sample_cover(cover: &GroundCover, inputs: &CoverInputs, local: Vec2) -> CoverSample {
    let mut s = match cover.style {
        CoverStyle::ForestFloor => sample_forest(cover, inputs, local),
        CoverStyle::CaveFloor => sample_cave(cover, inputs, local),
    };
    let sum: f32 = s.weights.iter().map(|w| w.max(0.0)).sum();
    for w in &mut s.weights {
        *w = w.max(0.0) / sum.max(1e-4);
    }
    s
}

fn noises(cover: &GroundCover, world: DVec3) -> (f32, f32, f32) {
    let seed = cover.seed;
    (
        fbm2(world.x * 0.07, world.z * 0.07, seed, 3, 2.0, 0.5),
        fbm2(world.x * 0.35, world.z * 0.35, seed ^ 0x77, 2, 2.0, 0.5),
        fbm2(world.x * 1.3, world.z * 1.3, seed ^ 0x99, 2, 2.0, 0.5),
    )
}

/// Contact shade, moss and litter piling up around trunks and rocks.
fn near_things(inputs: &CoverInputs, local: Vec2, w: &mut [f32; 8], shade: &mut f32, forest: bool) {
    if let Some(b) = &inputs.tree_buckets {
        for &i in b.near(local) {
            let t = &inputs.trees[i as usize];
            let d = (local - t.pos).length() - t.trunk;
            if d > 1.6 {
                continue;
            }
            *shade = shade.max(smooth(0.9, -0.05, d) * (0.35 + 0.4 * t.shed));
            if forest {
                let pile = smooth(1.4, 0.1, d) * t.shed;
                if t.conifer {
                    w[SLOT_NEEDLES] += 0.6 * pile;
                } else {
                    w[SLOT_LITTER] += 0.5 * pile;
                }
                w[SLOT_MOSS] += 0.25 * smooth(0.6, 0.0, d);
            }
        }
    }
    if let Some(b) = &inputs.rock_buckets {
        for &i in b.near(local) {
            let r = &inputs.rocks[i as usize];
            let d = (local - r.pos).length() / r.radius.max(0.05);
            if d > 2.0 {
                continue;
            }
            *shade = shade.max(smooth(1.45, 0.85, d) * 0.7);
            let ring = smooth(2.0, 1.0, d);
            if forest {
                w[SLOT_MOSS] += 0.7 * ring * r.moss;
                w[SLOT_HUMUS] += 0.35 * ring;
                w[SLOT_LITTER] += 0.25 * ring;
            } else {
                w[CAVE_EARTH] += 0.5 * ring;
            }
        }
    }
}

fn sample_forest(cover: &GroundCover, inputs: &CoverInputs, local: Vec2) -> CoverSample {
    let world = inputs.center + DVec3::new(local.x as f64, 0.0, local.y as f64);
    let [water, nutrients, _] = inputs.soil(world);
    let (n1, n2, n3) = noises(cover, world);
    let (broad, conifer) = inputs
        .canopy
        .as_ref()
        .map_or((0.5, 0.3), |c| c.sample(local));
    let dry = smooth(0.32, 0.12, water + 0.05 * n2);

    let mut w = [0.0f32; 8];
    w[SLOT_HUMUS] = 0.3 + 0.25 * smooth(0.5, 0.9, nutrients) + 0.2 * smooth(-0.2, 0.6, n2);
    w[SLOT_LITTER] = (0.1 + 1.0 * broad + 0.15 * n1).max(0.0) * (1.0 - 0.6 * dry);
    w[SLOT_NEEDLES] = (0.03 + 1.05 * conifer + 0.45 * dry + 0.1 * n2).max(0.0);
    w[SLOT_MOSS] = smooth(0.45, 0.78, water + 0.15 * n1) * 0.9 * (1.0 - 0.5 * broad)
        + 0.4 * smooth(0.25, 0.7, n3) * smooth(0.38, 0.6, water);
    // Dry ground: bedrock showing through, gravelly earth between.
    w[SLOT_ROCK] = dry * smooth(-0.1, 0.4, n1 + 0.35 * n3) * 1.4;
    w[SLOT_EARTH] = dry * 0.45 * smooth(-0.25, 0.3, n2 - 0.3 * n1);
    let mut wet = smooth(0.62, 0.95, water) * 0.45;
    let mut shade = 0.0f32;
    let mut trample = 0.0f32;
    let mut tint = [1.0f32; 3];
    let mut h = 0.032 + 0.016 * (n1 * 0.5 + 0.5) + 0.005 * n3;

    near_things(inputs, local, &mut w, &mut shade, true);

    // Burn scars: ash and charcoal, less of it the longer ago.
    for s in &cover.scorched {
        let d = (local - Vec2::new(s[0], s[1])).length() + 1.6 * n2;
        let f = (1.0 - d / s[2].max(0.1)).clamp(0.0, 1.0).powf(0.4) / (1.0 + s[3] * 0.05);
        let f = (f * 1.6).min(1.0);
        w[SLOT_ASH] = w[SLOT_ASH].max(f * (0.85 + 0.3 * n3));
        w[SLOT_LITTER] *= 1.0 - f;
        w[SLOT_NEEDLES] *= 1.0 - f;
        w[SLOT_MOSS] *= 1.0 - 0.85 * f;
        w[SLOT_HUMUS] *= 1.0 - 0.5 * f;
        w[SLOT_ROCK] *= 1.0 - f;
    }

    // The trail: packed, slightly sunken earth, a trampled margin and a low berm.
    if cover.trail.len() >= 2 {
        let hw = cover.trail_half_width;
        let d = polyline_distance(local, &cover.trail) + 0.3 * n2 + 0.08 * n3;
        let core = smooth(hw * 1.0, hw * 0.4, d);
        let edge = smooth(hw * 1.9, hw * 0.85, d);
        let keep = 1.0 - core;
        w[SLOT_EARTH] = w[SLOT_EARTH].max(core * 1.5);
        w[SLOT_LITTER] *= keep * (1.0 - 0.4 * edge);
        w[SLOT_NEEDLES] *= keep * (1.0 - 0.3 * edge);
        w[SLOT_MOSS] *= keep * (1.0 - 0.85 * edge);
        w[SLOT_ROCK] *= keep;
        // Leaves and needles blown onto the path in drifts; the middle trodden darker.
        let drift = smooth(0.1, 0.5, n3 + 0.5 * n2) * core;
        w[SLOT_LITTER] += drift * (0.25 + 0.7 * broad);
        w[SLOT_NEEDLES] += drift * (0.1 + 0.7 * conifer);
        tint = scale3(tint, 1.0 - 0.12 * smooth(hw * 0.45, 0.0, d));
        w[SLOT_HUMUS] = w[SLOT_HUMUS] * (1.0 - 0.7 * core) + 0.4 * edge * keep;
        // Darker, moister soil in the low spots of the path.
        w[SLOT_HUMUS] += core * 0.9 * smooth(0.05, 0.4, -n3 - 0.4 * n2);
        w[SLOT_MUD] = w[SLOT_MUD].max(core * smooth(0.55, 0.72, water + 0.1 * n1) * 1.6);
        wet = wet.max(core * smooth(0.55, 0.75, water + 0.1 * n1) * 0.85);
        trample = trample.max(edge);
        h -= 0.024 * core;
        h += 0.010 * smooth(hw * 2.1, hw * 1.2, d) * smooth(hw * 0.6, hw * 1.1, d);
    }

    // Pools: a damp, mossy surround, a low bank, wet mud and gravel down to the water.
    for pool in &cover.pools {
        let shore_noise = fbm2(
            world.x * 0.45,
            world.z * 0.45,
            cover.seed ^ 0x51,
            3,
            2.0,
            0.5,
        );
        let d = (local - Vec2::from(pool.center)).length() + 0.55 * shore_noise + 0.08 * n3;
        let t = d - pool.radius;
        let shore = smooth(2.6, 0.5, t);
        w[SLOT_MUD] = w[SLOT_MUD].max(shore * 1.4);
        let beach = smooth(1.1, 0.25, t) * smooth(-0.35, 0.1, t) * smooth(-0.15, 0.45, n3);
        w[SLOT_EARTH] = w[SLOT_EARTH].max(beach * 1.6);
        w[SLOT_MOSS] += 0.7 * smooth(1.4, 2.6, t) * smooth(5.5, 3.2, t);
        for k in [SLOT_LITTER, SLOT_NEEDLES, SLOT_ROCK, SLOT_ASH] {
            w[k] *= 1.0 - shore;
        }
        w[SLOT_HUMUS] *= 1.0 - 0.7 * shore;
        wet = wet.max(smooth(3.4, 0.1, t));
        let water_h = pool.water_m;
        let shore_h = if t < 0.0 {
            water_h - 0.05 + 0.064 * smooth(-1.6, 0.0, t)
        } else {
            water_h + 0.014 + 0.05 * smooth(0.0, 1.4, t)
        } + 0.012 * n3 * smooth(1.5, 0.0, t.abs());
        let blend = smooth(4.2, 1.4, t);
        h += (shore_h - h) * blend;
        tint = mix3(tint, [0.9, 0.92, 0.88], shore * 0.5);
    }

    // The round edge: greener (it meets the meadow) and dipping under the ground.
    if cover.round {
        let e = Vec2::new(
            local.x / cover.half_extents[0],
            local.y / cover.half_extents[1],
        )
        .length();
        let edge = smooth(0.86 + 0.05 * n1, 0.99, e);
        w[SLOT_MOSS] += 2.5 * edge;
        for k in [SLOT_LITTER, SLOT_NEEDLES, SLOT_ROCK, SLOT_ASH] {
            w[k] *= 1.0 - edge;
        }
        tint = mix3(tint, [1.05, 1.25, 0.85], edge);
        h += (-0.03 - h) * smooth(0.965, 1.0, e);
    }
    CoverSample {
        height: h,
        weights: w,
        wet,
        shade,
        trample,
        tint,
    }
}

fn sample_cave(cover: &GroundCover, inputs: &CoverInputs, local: Vec2) -> CoverSample {
    let world = inputs.center + DVec3::new(local.x as f64, 0.0, local.y as f64);
    let [water, _, minerals] = inputs.soil(world);
    let (n1, n2, n3) = noises(cover, world);
    let mut w = [0.0f32; 8];
    w[CAVE_ROCK] = 0.75 + 0.25 * n2;
    w[CAVE_MUD] = smooth(0.42, 0.8, water + 0.25 * n1) * 0.9;
    w[CAVE_CRUST] = smooth(0.4, 0.85, minerals + 0.1 * n2) * (0.6 + 0.7 * smooth(-0.3, 0.5, n2));
    w[CAVE_EARTH] = 0.5 * smooth(0.05, 0.6, n1 + 0.4 * n3);
    let mut wet = 0.3 + 0.6 * smooth(0.35, 0.75, water + 0.15 * n2);
    let mut shade = 0.0f32;
    let mut h = 0.012 + 0.014 * (n1 * 0.5 + 0.5) + 0.004 * n3;
    near_things(inputs, local, &mut w, &mut shade, false);
    for pool in &cover.pools {
        let rim_noise = fbm2(world.x * 0.6, world.z * 0.6, cover.seed ^ 0x52, 3, 2.0, 0.5);
        let d = (local - Vec2::from(pool.center)).length() + 0.35 * rim_noise;
        let t = d - pool.radius;
        // Rimstone: a pale lip of crust holds the pool, crust spreads where it overflows.
        let crust = smooth(1.8, 0.0, t.abs()) * 1.6 + smooth(4.0, 1.0, t) * 0.4;
        w[CAVE_CRUST] = w[CAVE_CRUST].max(crust);
        w[CAVE_MUD] *= 1.0 - smooth(1.2, 0.3, t);
        wet = wet.max(smooth(2.5, 0.0, t));
        let lip = (-((t - 0.18) / 0.28).powi(2)).exp();
        let shore_h = if t < 0.0 {
            pool.water_m - 0.04 + 0.05 * smooth(-1.2, 0.0, t)
        } else {
            pool.water_m + 0.006
        } + 0.03 * lip;
        h += (shore_h - h) * smooth(2.0, 0.6, t);
    }
    CoverSample {
        height: h,
        weights: w,
        wet,
        shade,
        trample: 0.0,
        tint: [1.0; 3],
    }
}

/// One vertex of a ground surface (40 bytes on the GPU).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GroundVertex {
    /// Local to the cover entity, meters.
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Layer weights per slot, 0..255.
    pub weights: [u8; 8],
    /// Wetness, contact shade, trampling, unused (0..255).
    pub extra: [u8; 4],
    /// Albedo tint: 128 = unchanged, 255 ≈ ×2.
    pub tint: [u8; 4],
}

/// Size of a [`GroundVertex`] on the GPU, bytes.
pub const GROUND_VERTEX_BYTES: usize = 40;

impl GroundVertex {
    fn write(&self, out: &mut Vec<u8>) {
        for v in self.position.iter().chain(self.normal.iter()) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.weights);
        out.extend_from_slice(&self.extra);
        out.extend_from_slice(&self.tint);
    }
}

/// The built mesh of a [`GroundCover`], drawn by [`SceneryFeature`] (runtime data, not saved).
#[derive(Component, Clone)]
pub struct GroundSurface {
    pub vertices: Arc<Vec<GroundVertex>>,
    pub indices: Arc<Vec<u32>>,
    /// The layer drawn in each weight slot.
    pub slots: [GroundLayer; 8],
    /// Changes whenever the mesh is rebuilt.
    pub version: u64,
    /// The cover settings it was built from (rebuilt when they change).
    pub built_from: GroundCover,
}

impl GroundSurface {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Build the surface of `cover` (centered at `inputs.center`), following `ground`.
pub fn build_surface(
    cover: &GroundCover,
    inputs: &CoverInputs,
    ground: Option<&Ground>,
    version: u64,
) -> GroundSurface {
    let center = inputs.center;
    let cell = cover.cell_m.max(0.2);
    let nx = (cover.half_extents[0] * 2.0 / cell).ceil() as usize + 1;
    let nz = (cover.half_extents[1] * 2.0 / cell).ceil() as usize + 1;
    let mut heights = vec![0.0f32; nx * nz];
    let mut samples = Vec::with_capacity(nx * nz);
    let mut inside = vec![true; nx * nz];
    for j in 0..nz {
        for i in 0..nx {
            let lx = -cover.half_extents[0] + i as f32 * cell;
            let lz = -cover.half_extents[1] + j as f32 * cell;
            let world = center + DVec3::new(lx as f64, 0.0, lz as f64);
            let gy = ground
                .and_then(|g| g.height(world + DVec3::Y * 4.0))
                .unwrap_or(center.y);
            let s = sample_cover(cover, inputs, Vec2::new(lx, lz));
            heights[j * nx + i] = (gy - center.y) as f32 + s.height;
            if cover.round {
                let e = Vec2::new(lx / cover.half_extents[0], lz / cover.half_extents[1]).length();
                inside[j * nx + i] = e <= 1.0;
            }
            samples.push(s);
        }
    }
    let mut vertices = Vec::with_capacity(nx * nz);
    for j in 0..nz {
        for i in 0..nx {
            let k = j * nx + i;
            let hx = heights[j * nx + (i + 1).min(nx - 1)] - heights[j * nx + i.saturating_sub(1)];
            let hz = heights[(j + 1).min(nz - 1) * nx + i] - heights[j.saturating_sub(1) * nx + i];
            let normal = Vec3::new(-hx / (2.0 * cell), 1.0, -hz / (2.0 * cell)).normalize();
            let s = &samples[k];
            vertices.push(GroundVertex {
                position: [
                    -cover.half_extents[0] + i as f32 * cell,
                    heights[k],
                    -cover.half_extents[1] + j as f32 * cell,
                ],
                normal: normal.to_array(),
                weights: s.weights.map(to_unorm8),
                extra: [
                    to_unorm8(s.wet),
                    to_unorm8(s.shade),
                    to_unorm8(s.trample),
                    0,
                ],
                tint: [
                    to_unorm8(s.tint[0] * 0.5),
                    to_unorm8(s.tint[1] * 0.5),
                    to_unorm8(s.tint[2] * 0.5),
                    255,
                ],
            });
        }
    }
    let mut indices = Vec::with_capacity((nx - 1) * (nz - 1) * 6);
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = (j * nx + i) as u32;
            let (b, c, d) = (a + 1, a + nx as u32 + 1, a + nx as u32);
            if [a, b, c, d].iter().any(|&k| !inside[k as usize]) {
                continue;
            }
            indices.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }
    GroundSurface {
        vertices: Arc::new(vertices),
        indices: Arc::new(indices),
        slots: style_slots(cover.style),
        version,
        built_from: cover.clone(),
    }
}

// ---------------------------------------------------------------- rock meshes

/// Procedural rock shapes: angular ones first, then rounded river stones.
pub const ROCK_SHAPES: u32 = 12;
/// Rounded shapes are the last few.
const ROUNDED_SHAPES: u32 = 4;
/// Levels of detail per shape (1280, 320 and 80 triangles).
pub const ROCK_LODS: usize = 3;

/// One rock vertex (28 bytes on the GPU): position in the unit box, normal, and cavity
/// occlusion in `data[0]` (255 = open).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RockVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub data: [u8; 4],
}

/// Size of a [`RockVertex`] on the GPU, bytes.
pub const ROCK_VERTEX_BYTES: usize = 28;

/// A rock mesh in a unit box: x and z in −0.5..0.5, y from 0 (the flat bottom) to 1.
/// A [`Rock`]'s `Transform` scale is its size in meters. (A [`crate::cave::CaveChamber`]'s mesh
/// is in meters.) `data` is the cavity occlusion (255 = open), then, both 0 on rocks, how dark
/// it is (255: no moss or lichen grow) and how wet.
#[derive(Debug, Clone, Default)]
pub struct RockMesh {
    pub vertices: Vec<RockVertex>,
    pub indices: Vec<u32>,
}

/// An icosphere: unit directions and counter-clockwise (outward) triangles.
fn icosphere(subdivisions: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut v: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize())
    .collect();
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |a: u32, b: u32, v: &mut Vec<Vec3>| {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                v.push(((v[a as usize] + v[b as usize]) * 0.5).normalize());
                v.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(f.len() * 4);
        for [a, b, c] in f {
            let ab = mid(a, b, &mut v);
            let bc = mid(b, c, &mut v);
            let ca = mid(c, a, &mut v);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = next;
    }
    (v, f)
}

/// The shape of rock `shape` in direction `d` (before normalizing to the unit box).
struct RockShape {
    radii: Vec3,
    cuts: Vec<(Vec3, f32)>,
    chips: Vec<(Vec3, f32)>,
    lumps: f32,
    detail: f32,
    seed: u64,
}

impl RockShape {
    fn new(shape: u32) -> Self {
        let shape = shape % ROCK_SHAPES;
        let rounded = shape >= ROCK_SHAPES - ROUNDED_SHAPES;
        let seed = hash_u64(0x5_70ae ^ shape as u64);
        let mut rng = Rng::new(seed);
        let radii = if rounded {
            Vec3::new(1.0, rng.range(0.45, 0.7), rng.range(0.65, 0.95))
        } else {
            Vec3::new(1.0, rng.range(0.55, 0.95), rng.range(0.7, 1.05))
        };
        let mut cuts = Vec::new();
        let mut chips = Vec::new();
        if !rounded {
            for _ in 0..rng.range_i32(8, 13) {
                let mut n = rng.unit_vector();
                n.y *= 0.6;
                let n = n.normalize();
                let reach = (n * radii).length();
                cuts.push((n, reach * rng.range(0.45, 0.75)));
            }
            for _ in 0..rng.range_i32(5, 9) {
                let n = rng.unit_vector();
                let reach = (n * radii).length();
                chips.push((n, reach * rng.range(0.78, 0.92)));
            }
        }
        Self {
            radii,
            cuts,
            chips,
            lumps: if rounded { 0.07 } else { 0.12 },
            detail: if rounded { 0.008 } else { 0.022 },
            seed,
        }
    }

    /// The surface in direction `d`, and which flat face it lies on: a split face (its index
    /// into the cuts, then the chips) or `usize::MAX` for the flattened bottom.
    fn point(&self, d: Vec3) -> (Vec3, Option<usize>) {
        let s = self.seed;
        let n = |p: Vec3, f: f32, salt: u64| {
            fbm3(
                (p.x * f) as f64,
                (p.y * f) as f64,
                (p.z * f) as f64,
                s ^ salt,
                3,
                2.0,
                0.5,
            )
        };
        let mut p = d * self.radii * (1.0 + self.lumps * n(d, 1.3, 1));
        // Every step moves the point along its own ray from the center, so the surface stays
        // a radial graph of the sphere: the cuts come out exactly planar and no triangle
        // folds over. The last cut that clips a point is the one it lies on.
        let mut face = None;
        for (i, &(nrm, off)) in self.cuts.iter().chain(&self.chips).enumerate() {
            let reach = p.dot(nrm);
            if reach > off {
                p *= off / reach;
                face = Some(i);
            }
        }
        // Weathering roughens the faces a little.
        let ray = p.normalize_or(d);
        p += ray * self.detail * n(d, 7.0, 2) * if face.is_some() { 0.35 } else { 1.0 };
        // A flattened, buried bottom.
        let floor = -self.radii.y * 0.5;
        if p.y < floor {
            p *= (floor + (p.y - floor) * 0.12) / p.y;
            face = Some(usize::MAX);
        }
        (p, face)
    }

    /// Normal of flat face `face` (from [`RockShape::point`]), in the shape's space.
    fn face_normal(&self, face: usize) -> Vec3 {
        if face == usize::MAX {
            return -Vec3::Y;
        }
        self.cuts
            .iter()
            .chain(&self.chips)
            .nth(face)
            .map_or(Vec3::Y, |c| c.0)
    }
}

/// Build rock `shape` (any number; wraps) at level of detail `lod` (0 = finest).
pub fn rock_mesh(shape: u32, lod: usize) -> RockMesh {
    let rs = RockShape::new(shape);
    let (dirs, faces) = icosphere(3 - lod.min(2) as u32);
    let shaped: Vec<(Vec3, Option<usize>)> = dirs.iter().map(|&d| rs.point(d)).collect();
    let mut pos: Vec<Vec3> = shaped.iter().map(|s| s.0).collect();
    // Into the unit box: bottom at y = 0.
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &pos {
        min = min.min(*p);
        max = max.max(*p);
    }
    let size = (max - min).max(Vec3::splat(1e-4));
    for p in &mut pos {
        let q = (*p - min) / size;
        *p = Vec3::new(q.x - 0.5, q.y, q.z - 0.5);
    }
    // Split faces are shaded flat and meet at crisp edges: a triangle lying on one (two of
    // its corners on it) takes that face's normal; the weathered rest stays smooth, its
    // normals averaged over the smooth triangles only so the crease does not bleed into it.
    let flat: Vec<Option<usize>> = faces
        .iter()
        .map(|tri| {
            let label = |k: usize| shaped[tri[k] as usize].1;
            if label(0).is_some() && (label(0) == label(1) || label(0) == label(2)) {
                label(0)
            } else if label(1).is_some() && label(1) == label(2) {
                label(1)
            } else {
                None
            }
        })
        .collect();
    // Normals and cavity (how far a vertex sits below its neighbors' average).
    let mut normals = vec![Vec3::ZERO; pos.len()];
    let mut smooth_normals = vec![Vec3::ZERO; pos.len()];
    let mut neighbor_sum = vec![Vec3::ZERO; pos.len()];
    let mut neighbor_count = vec![0u32; pos.len()];
    for (&[a, b, c], flat) in faces.iter().zip(&flat) {
        let (pa, pb, pc) = (pos[a as usize], pos[b as usize], pos[c as usize]);
        let n = (pb - pa).cross(pc - pa);
        for (i, others) in [(a, [b, c]), (b, [c, a]), (c, [a, b])] {
            normals[i as usize] += n;
            if flat.is_none() {
                smooth_normals[i as usize] += n;
            }
            for o in others {
                neighbor_sum[i as usize] += pos[o as usize];
                neighbor_count[i as usize] += 1;
            }
        }
    }
    // Typical edge length in the unit box.
    let edge = 0.55 / (1u32 << (3 - lod.min(2))) as f32;
    let open: Vec<u8> = pos
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let n = normals[i].normalize_or(Vec3::Y);
            let avg = neighbor_sum[i] / neighbor_count[i].max(1) as f32;
            let cavity = (avg - *p).dot(n) / edge;
            to_unorm8((1.0 - cavity * 0.9).clamp(0.35, 1.0))
        })
        .collect();
    // Flat normals go into the unit box through the inverse transpose of its scale.
    let mut vertices = Vec::with_capacity(pos.len() * 2);
    let mut indices = Vec::with_capacity(faces.len() * 3);
    let mut corners: HashMap<(u32, Option<usize>), u32> = HashMap::new();
    for (tri, &flat) in faces.iter().zip(&flat) {
        for &v in tri {
            let index = *corners.entry((v, flat)).or_insert_with(|| {
                let n = match flat {
                    Some(f) => (rs.face_normal(f) * size).normalize_or(Vec3::Y),
                    None => smooth_normals[v as usize].normalize_or(Vec3::Y),
                };
                vertices.push(RockVertex {
                    position: pos[v as usize].to_array(),
                    normal: n.to_array(),
                    data: [open[v as usize], 0, 0, 255],
                });
                vertices.len() as u32 - 1
            });
            indices.push(index);
        }
    }
    RockMesh { vertices, indices }
}

/// Whether rock shape `shape` is a rounded river stone.
pub fn rock_is_rounded(shape: u32) -> bool {
    shape % ROCK_SHAPES >= ROCK_SHAPES - ROUNDED_SHAPES
}

/// A boulder, stone or pebble drawn by [`SceneryFeature`]: one of [`ROCK_SHAPES`] procedural
/// shapes scaled by the entity's `Transform` (scale = width, height, depth in meters). The
/// transform's origin is the rock's bottom center; [`build_scenery`] settles it onto the ground
/// cover, `bury_m` deep.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Rock {
    /// Which shape (wraps; the last few are rounded river stones).
    pub shape: u32,
    /// How deep it sits in the ground, meters.
    pub bury_m: f32,
    /// Moss on its top faces, 0..1.
    pub moss: f32,
    /// Lichen spots, 0..1.
    pub lichen: f32,
    /// Water surface above the ground at the rock, meters (0 = dry): wet, dark and slimy
    /// below it.
    pub water_m: f32,
    /// Albedo multiplier (linear RGB).
    pub tint: [f32; 3],
    /// Set once the rock has been placed on the ground cover.
    pub settled: bool,
}

impl Default for Rock {
    fn default() -> Self {
        Self {
            shape: 0,
            bury_m: 0.1,
            moss: 0.3,
            lichen: 0.3,
            water_m: 0.0,
            tint: [1.0; 3],
            settled: false,
        }
    }
}

/// A rock to spawn: where (local `x, z` on its cover), how it is turned, its size in meters
/// (width, height, depth) and its look.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedRock {
    pub pos: Vec2,
    pub rotation: Quat,
    pub size: Vec3,
    pub rock: Rock,
}

impl PlacedRock {
    /// Footprint radius, meters.
    pub fn radius(&self) -> f32 {
        0.5 * self.size.x.max(self.size.z)
    }
}

/// One rock about `width` meters across: an angular shape (or a rounded stone), squat,
/// sunk a fifth to a third of its height, mossier the bigger it is.
pub fn random_rock(
    rng: &mut Rng,
    pos: Vec2,
    width: f32,
    rounded: bool,
    moss: f32,
    tint: [f32; 3],
) -> PlacedRock {
    let angular = (ROCK_SHAPES - ROUNDED_SHAPES) as i32;
    let shape = if rounded {
        (angular + rng.range_i32(0, ROUNDED_SHAPES as i32)) as u32
    } else {
        rng.range_i32(0, angular) as u32
    };
    let height = width * rng.range(0.42, 0.8);
    let size = Vec3::new(width, height, width * rng.range(0.62, 1.0));
    let t = rng.range(0.88, 1.08);
    PlacedRock {
        pos,
        rotation: Quat::from_rotation_y(rng.range(0.0, TAU)),
        size,
        rock: Rock {
            shape,
            bury_m: height * rng.range(0.2, 0.36),
            moss: (moss * rng.range(0.75, 1.2) * (0.35 + 0.65 * smooth(0.1, 1.2, width)))
                .clamp(0.0, 1.0),
            lichen: rng.range(0.25, 0.7),
            tint: tint.map(|c| c * t),
            ..Default::default()
        },
    }
}

/// An outcrop around `at`: a main rock about `size` meters across, smaller ones leaning on it
/// and stones broken off around it, all of one stone.
pub fn rock_cluster(
    rng: &mut Rng,
    at: Vec2,
    size: f32,
    moss: f32,
    tint: [f32; 3],
    out: &mut Vec<PlacedRock>,
) {
    let mut main = random_rock(rng, at, size, false, moss, tint);
    main.size.y = size * rng.range(0.55, 0.85);
    main.rock.bury_m = main.size.y * rng.range(0.22, 0.32);
    out.push(main);
    let leaning = rng.range_i32(2, 5);
    let start = rng.range(0.0, TAU);
    for i in 0..leaning {
        let a = start + i as f32 * TAU / leaning as f32 + rng.range(-0.5, 0.5);
        let w = size * rng.range(0.32, 0.6);
        let dir = Vec2::from_angle(a);
        let mut r = random_rock(
            rng,
            at + dir * (size * 0.42 + w * 0.25),
            w,
            false,
            moss,
            tint,
        );
        // Tipped toward the main rock.
        let axis = Vec3::new(-dir.y, 0.0, dir.x);
        r.rotation = Quat::from_axis_angle(axis, -rng.range(0.08, 0.3)) * r.rotation;
        r.rock.bury_m = r.size.y * rng.range(0.3, 0.45);
        out.push(r);
    }
    let stones = rng.range_i32(4, 9);
    for _ in 0..stones {
        let dir = Vec2::from_angle(rng.range(0.0, TAU));
        let p = at + dir * size * rng.range(0.6, 1.5);
        let w = size * rng.range(0.07, 0.2);
        out.push(random_rock(rng, p, w, false, moss * 0.6, tint));
    }
}

/// Spawn rocks on a ground cover centered at `center`; [`build_scenery`] settles them on it.
pub fn spawn_rocks(commands: &mut Commands, center: DVec3, rocks: Vec<PlacedRock>) {
    commands.spawn_batch(rocks.into_iter().map(move |r| {
        (
            Transform::from_translation(center + DVec3::new(r.pos.x as f64, 0.0, r.pos.y as f64))
                .with_rotation(r.rotation)
                .with_scale(r.size),
            r.rock,
        )
    }));
}

// ---------------------------------------------------------------- debris meshes

fn lin4(c: [f32; 3], s: f32) -> Vec4 {
    Vec4::new(c[0] * s, c[1] * s, c[2] * s, 1.0)
}

/// A twig (`branch = false`, 20–50 cm) or a fallen branch (0.8–1.7 m with forks), lying on
/// the ground along +X, bark-colored with pale broken ends; some branches are mossy on top.
pub fn twig_mesh(seed: u64, branch: bool) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut b = MeshBuilder::new();
    let (len, r0, sides, n) = if branch {
        (rng.range(0.8, 1.7), rng.range(0.018, 0.034), 7u32, 10usize)
    } else {
        (rng.range(0.22, 0.5), rng.range(0.005, 0.011), 5u32, 6usize)
    };
    let barks = [0x4a3e33, 0x5a4c3e, 0x3e352c, 0x6a5e50, 0x52463a];
    let bark = srgb(*rng.pick(&barks).unwrap());
    let wood = srgb(0x9a8462);
    let mut path = Vec::with_capacity(n + 1);
    let mut radii = Vec::with_capacity(n + 1);
    let wob = rng.range(0.0, TAU);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let r = r0 * (1.0 - 0.55 * t);
        let lift = r0 * 0.6 * (0.5 + 0.5 * (t * 7.0 + wob).sin()) * (t * PI).sin();
        let z = len * 0.06 * (t * 3.0 + wob).sin();
        path.push(Vec3::new(-len * 0.5 + len * t, r + lift, z));
        radii.push(r);
    }
    let knot = rng.next_u64();
    let bark_color = move |i: usize, around: f32| {
        let k = perlin2(i as f64 * 0.9, around as f64 * 5.0, knot) * 0.18;
        lin4(bark, 1.0 + k)
    };
    let end = |_r: f32| lin4(wood, 1.0);
    b.tube(
        &path,
        &radii,
        sides,
        1.0,
        &bark_color,
        Some(&end),
        Some(&end),
    );
    let limbs = if branch {
        rng.range_i32(2, 6)
    } else {
        rng.range_i32(0, 3)
    };
    for _ in 0..limbs {
        let k = rng.range_i32(2, n as i32 - 1) as usize;
        let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
        let a = rng.range(0.45, 1.0) * side;
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        let l = len * rng.range(0.18, 0.42);
        let rise = if branch { rng.range(0.0, 0.25) } else { 0.02 };
        let r1 = radii[k] * 0.55;
        let m = 4;
        let mut limb = Vec::new();
        let mut lr = Vec::new();
        for i in 0..=m {
            let t = i as f32 / m as f32;
            limb.push(path[k] + dir * l * t + Vec3::Y * rise * t * t);
            lr.push(r1 * (1.0 - 0.7 * t));
        }
        b.tube(&limb, &lr, sides.min(5), 1.0, &bark_color, None, Some(&end));
        if branch && rng.chance(0.6) {
            let j = 2;
            let a2 = a + rng.range(0.4, 0.8) * side;
            let d2 = Vec3::new(a2.cos(), 0.0, a2.sin());
            let l2 = l * rng.range(0.3, 0.5);
            let tw: Vec<Vec3> = (0..=3)
                .map(|i| limb[j] + d2 * l2 * (i as f32 / 3.0) + Vec3::Y * 0.02 * i as f32)
                .collect();
            let twr: Vec<f32> = (0..=3)
                .map(|i| lr[j] * 0.5 * (1.0 - 0.2 * i as f32))
                .collect();
            b.tube(&tw, &twr, 4, 1.0, &bark_color, None, None);
        }
    }
    if branch && rng.chance(0.5) {
        let moss = srgb(0x3d5222);
        let mseed = rng.next_u64();
        for v in &mut b.vertices {
            let top = smooth(0.35, 0.85, v.normal[1]);
            let patch = smooth(
                0.05,
                0.45,
                perlin2(
                    v.position[0] as f64 * 5.0,
                    v.position[2] as f64 * 5.0,
                    mseed,
                ),
            );
            let f = top * patch * 0.85;
            for (c, m) in v.color.iter_mut().zip(moss) {
                *c += (m - *c) * f;
            }
        }
    }
    b.finish()
}

/// A pine cone (`spruce = false`: 6–7 cm, egg-shaped, thick scales) or a spruce cone (12 cm,
/// slender), lying along +X.
pub fn cone_mesh(spruce: bool, seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let (len, rmax) = if spruce {
        (rng.range(0.1, 0.13), 0.018)
    } else {
        (rng.range(0.055, 0.07), 0.024)
    };
    let (dark, tip) = if spruce {
        (srgb(0x4a321c), srgb(0x9a7448))
    } else {
        (srgb(0x34200f), srgb(0x9c7650))
    };
    let (rings, seg) = (14usize, 12usize);
    let mut b = MeshBuilder::new();
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let profile = if spruce {
            rmax * (PI * t.powf(0.85)).sin().max(0.0).powf(0.45) * (1.0 - 0.3 * t)
        } else {
            rmax * (PI * t.powf(0.7)).sin().max(0.0).powf(0.6)
        };
        for j in 0..=seg {
            let a = j as f32 / seg as f32 * TAU;
            let s1 = (a / TAU * 8.0 + t * 7.0).fract();
            let s2 = (-a / TAU * 5.0 + t * 11.0).rem_euclid(1.0);
            let bump = (s1 * s2).powf(1.5);
            let r = profile * (1.0 + if spruce { 0.22 } else { 0.4 } * bump);
            let p = Vec3::new((t - 0.5) * len, rmax * 0.85 + r * a.cos(), r * a.sin());
            let c = mix3(dark, tip, bump.powf(0.7) * smooth(0.0, 0.1, t));
            b.vertex(
                p,
                Vec3::new(0.0, a.cos(), a.sin()),
                Vec2::new(j as f32 / seg as f32, t),
                lin4(c, 1.0),
            );
        }
    }
    let row = seg as u32 + 1;
    for i in 0..rings as u32 {
        for j in 0..seg as u32 {
            let a = i * row + j;
            let (bb, c, d) = (a + 1, a + row + 1, a + row);
            b.tri(a, c, bb);
            b.tri(a, d, c);
        }
    }
    let mut mesh = b.finish();
    mesh.compute_normals();
    mesh
}

/// A clump of fallen leaves (cards with the [`fallen_leaf_atlas`]), about 35 cm across.
pub fn leaf_clump_mesh(seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut b = MeshBuilder::new();
    let count = rng.range_i32(7, 14);
    for k in 0..count {
        let c = rng.in_unit_disk() * 0.17;
        let yaw = rng.range(0.0, TAU);
        let (sy, cy) = yaw.sin_cos();
        let leaf = rng.range(0.06, 0.11);
        let card = leaf / 0.86;
        let cell = rng.range_i32(0, 4) as usize;
        let (cu, cv) = ((cell % 2) as f32 * 0.5, (cell / 2) as f32 * 0.5);
        let curl = rng.range(0.0, 0.5);
        let arch = rng.range(-0.1, 0.25);
        let lift = 0.003 + 0.0025 * k as f32;
        let tint = rng.range(0.8, 1.1);
        let first = b.vertices.len() as u32;
        for r in 0..4 {
            let t = r as f32 / 3.0;
            for col in 0..3 {
                let a = col as f32 - 1.0;
                let x = a * card * 0.5;
                let z = (t - 0.5) * card;
                let y = lift + curl * a * a * card * 0.25 + arch * (PI * t).sin() * card * 0.2;
                let p = Vec3::new(c.x + x * cy - z * sy, y, c.y + x * sy + z * cy);
                let uv = Vec2::new(cu + 0.5 * (0.5 + a * 0.5), cv + 0.5 * (1.0 - t));
                let n = Vec3::new(-a * curl * 0.6 * cy, 1.0, -a * curl * 0.6 * sy);
                b.vertex(p, n, uv, Vec4::new(tint, tint, tint, 1.0));
            }
        }
        for r in 0..3u32 {
            for col in 0..2u32 {
                let a = first + r * 3 + col;
                let (bb, c2, d) = (a + 1, a + 4, a + 3);
                b.tri(a, c2, bb);
                b.tri(a, d, c2);
            }
        }
    }
    b.finish()
}

/// A clump of reeds and cattails, 1–1.8 m tall, its base at the origin.
pub fn reed_mesh(seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut b = MeshBuilder::new();
    let (base_c, mid_c, straw) = (srgb(0x8a9a5a), srgb(0x4f6e2a), srgb(0xa89a58));
    for _ in 0..rng.range_i32(24, 36) {
        let at = rng.in_unit_disk() * 0.24;
        let base = Vec3::new(at.x, -0.05, at.y);
        let height = rng.range(0.8, 1.7);
        let out = (Vec3::new(at.x, 0.0, at.y).normalize_or(Vec3::X)
            + Vec3::new(rng.range(-0.6, 0.6), 0.0, rng.range(-0.6, 0.6)))
        .normalize_or(Vec3::X);
        let lean = out * height * rng.range(0.08, 0.35);
        let droop = if rng.chance(0.35) {
            rng.range(0.1, 0.35)
        } else {
            0.0
        };
        let p1 = base + Vec3::Y * height * 0.6 + lean * 0.3;
        let p2 = base + Vec3::Y * height * (1.0 - droop) + lean * (1.0 + droop * 2.0);
        let width = rng.range(0.011, 0.02);
        let twist = rng.range(0.0, TAU);
        let tip_c = if rng.chance(0.4) { straw } else { mid_c };
        let shade = rng.range(0.85, 1.1);
        let segs = 7;
        let (mut left, mut right, mut normals, mut vs, mut colors) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for i in 0..=segs {
            let t = i as f32 / segs as f32;
            let p = crate::meshgen::bezier(base, p1, p2, t);
            let ahead = crate::meshgen::bezier(base, p1, p2, (t + 0.02).min(1.0));
            let tangent = if t < 1.0 {
                (ahead - p).normalize_or(Vec3::Y)
            } else {
                (p - crate::meshgen::bezier(base, p1, p2, 0.98)).normalize_or(Vec3::Y)
            };
            let a = twist + t * 1.2;
            let flat = Vec3::new(a.cos(), 0.0, a.sin());
            let side = (flat - tangent * flat.dot(tangent)).normalize_or(Vec3::X);
            let w = width * (1.0 - t.powf(1.6)).max(0.05) * 0.5;
            left.push(p - side * w);
            right.push(p + side * w);
            normals.push(side.cross(tangent).normalize_or(Vec3::Y));
            vs.push(t);
            let c = if t < 0.3 {
                mix3(base_c, mid_c, t / 0.3)
            } else {
                mix3(mid_c, tip_c, smooth(0.55, 1.0, t))
            };
            colors.push(lin4(c, shade));
        }
        b.strip(&left, &right, &normals, &vs, &colors);
    }
    let (stem_c, head_c) = (srgb(0x5a7432), srgb(0x4a2a16));
    for _ in 0..rng.range_i32(2, 6) {
        let at = rng.in_unit_disk() * 0.18;
        let h = rng.range(1.1, 1.75);
        let lean = Vec3::new(rng.range(-0.1, 0.1), 0.0, rng.range(-0.1, 0.1)) * h;
        let stem: Vec<Vec3> = (0..=4)
            .map(|i| {
                let t = i as f32 / 4.0;
                Vec3::new(at.x, -0.05, at.y) + Vec3::Y * h * t + lean * t * t
            })
            .collect();
        b.tube(
            &stem,
            &[0.004; 5],
            4,
            1.0,
            &|_, _| lin4(stem_c, 1.0),
            None,
            None,
        );
        let top = stem[4];
        let dir = (stem[4] - stem[3]).normalize();
        let head_len = rng.range(0.14, 0.2);
        let head: Vec<Vec3> = (0..=3)
            .map(|i| top - dir * (0.08 + head_len * (1.0 - i as f32 / 3.0)))
            .collect();
        let shade = rng.range(0.8, 1.15);
        let head_color = move |_: usize, _: f32| lin4(head_c, shade);
        let head_cap = move |_: f32| lin4(head_c, shade);
        b.tube(
            &head,
            &[0.011, 0.014, 0.014, 0.011],
            8,
            1.0,
            &head_color,
            Some(&head_cap),
            Some(&head_cap),
        );
        let spike: Vec<Vec3> = [0.0, 0.5, 1.0]
            .iter()
            .map(|t| head[3] + dir * 0.11 * t)
            .collect();
        b.tube(
            &spike,
            &[0.002, 0.0015, 0.0008],
            3,
            1.0,
            &|_, _| lin4(straw, 0.8),
            None,
            None,
        );
    }
    b.finish()
}

// ---------------------------------------------------------------- scattering

/// What a piece of scattered debris is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebrisKind {
    Twig,
    Branch,
    PineCone,
    SpruceCone,
    Leaves,
    Reeds,
    /// A small stone (spawned as a [`Rock`]).
    Pebble,
}

/// Where one piece of debris goes (local to its cover).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebrisSpot {
    pub kind: DebrisKind,
    pub variant: u32,
    pub pos: Vec2,
    pub yaw: f32,
    pub scale: f32,
}

/// Most pieces of each kind one cover scatters.
const DEBRIS_CAP: [(DebrisKind, usize); 7] = [
    (DebrisKind::Twig, 1400),
    (DebrisKind::Branch, 220),
    (DebrisKind::PineCone, 700),
    (DebrisKind::SpruceCone, 500),
    (DebrisKind::Leaves, 800),
    (DebrisKind::Reeds, 60),
    (DebrisKind::Pebble, 1300),
];

/// Variants of each debris mesh.
const TWIG_VARIANTS: u32 = 5;
const BRANCH_VARIANTS: u32 = 3;
const CONE_VARIANTS: u32 = 2;
const LEAF_VARIANTS: u32 = 3;
const REED_VARIANTS: u32 = 3;

/// Decide where debris lies on a cover: twigs, branches and cones or leaf clumps under each
/// tree (by its kind), pebbles here and there (more on dry ground, along the trail and on
/// shores), reeds along pond shores. Nothing in water (but reeds), on the trail's packed core
/// (but a few twigs and pebbles), inside rocks or trunks, or on fresh ash.
pub fn scatter_debris(cover: &GroundCover, inputs: &CoverInputs) -> Vec<DebrisSpot> {
    let mut rng = Rng::new(cover.seed ^ 0xdeb_415);
    let density = cover.debris.max(0.0);
    let mut out = Vec::new();
    if density <= 0.0 {
        return out;
    }
    let mut counts: HashMap<DebrisKind, usize> = HashMap::new();
    let ok = |kind: DebrisKind, p: Vec2, rng: &mut Rng| -> bool {
        let e = Vec2::new(p.x / cover.half_extents[0], p.y / cover.half_extents[1]);
        let e = if cover.round {
            e.length()
        } else {
            e.x.abs().max(e.y.abs())
        };
        if e > 0.93 {
            return false;
        }
        for pool in &cover.pools {
            let t = (p - Vec2::from(pool.center)).length() - pool.radius;
            let allowed = match kind {
                DebrisKind::Reeds => t > -0.9,
                DebrisKind::Pebble => t > -0.6,
                _ => t > 0.6,
            };
            if !allowed {
                return false;
            }
        }
        if cover.trail.len() >= 2 {
            let d = polyline_distance(p, &cover.trail) / cover.trail_half_width.max(0.1);
            let pass = match kind {
                DebrisKind::Twig | DebrisKind::Pebble => 0.15,
                DebrisKind::PineCone | DebrisKind::SpruceCone => 0.1,
                _ => 0.0,
            };
            if d < 0.85 && !rng.chance(pass) {
                return false;
            }
        }
        for s in &cover.scorched {
            if (p - Vec2::new(s[0], s[1])).length() < s[2] * 0.8 && kind != DebrisKind::Pebble {
                return false;
            }
        }
        if inputs
            .rocks
            .iter()
            .any(|r| (p - r.pos).length() < r.radius * 0.95)
        {
            return false;
        }
        !inputs
            .trees
            .iter()
            .any(|t| (p - t.pos).length() < t.trunk + 0.12)
    };
    let mut push = |kind: DebrisKind, p: Vec2, rng: &mut Rng, out: &mut Vec<DebrisSpot>| {
        let cap = DEBRIS_CAP
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(0, |c| c.1);
        let n = counts.entry(kind).or_insert(0);
        if *n >= cap || !ok(kind, p, rng) {
            return;
        }
        *n += 1;
        let variants = match kind {
            DebrisKind::Twig => TWIG_VARIANTS,
            DebrisKind::Branch => BRANCH_VARIANTS,
            DebrisKind::PineCone | DebrisKind::SpruceCone => CONE_VARIANTS,
            DebrisKind::Leaves => LEAF_VARIANTS,
            DebrisKind::Reeds => REED_VARIANTS,
            DebrisKind::Pebble => ROCK_SHAPES,
        };
        out.push(DebrisSpot {
            kind,
            variant: rng.next_u32() % variants,
            pos: p,
            yaw: rng.range(0.0, TAU),
            scale: rng.range(0.8, 1.2),
        });
    };
    if cover.style == CoverStyle::ForestFloor {
        // How much each tree sheds of each kind; thinned evenly so the caps hold everywhere.
        let shed = |t: &TreeSpot| {
            let k = t.shed * density;
            let reach = t.crown.max(0.8);
            let twigs = k * reach * if t.shrub { 0.6 } else { 1.1 };
            let branch = if t.shrub { 0.0 } else { 0.4 * k };
            let (cones, leaves) = if t.conifer {
                (k * reach * 2.2, 0.0)
            } else {
                (0.0, k * reach * 1.1)
            };
            [twigs, branch, cones, leaves]
        };
        let mut demand = [0.0f32; 4];
        for t in inputs.trees.iter().filter(|t| t.shed >= 0.05) {
            for (d, s) in demand.iter_mut().zip(shed(t)) {
                *d += s;
            }
        }
        let cap = |kind: DebrisKind| {
            DEBRIS_CAP
                .iter()
                .find(|(k, _)| *k == kind)
                .map_or(0, |c| c.1)
        };
        let cone_cap = cap(DebrisKind::PineCone).min(cap(DebrisKind::SpruceCone)) as f32 * 1.6;
        let thin = [
            (cap(DebrisKind::Twig) as f32 * 0.95 / demand[0].max(1.0)).min(1.0),
            (cap(DebrisKind::Branch) as f32 * 0.95 / demand[1].max(1.0)).min(1.0),
            (cone_cap * 0.95 / demand[2].max(1.0)).min(1.0),
            (cap(DebrisKind::Leaves) as f32 * 0.95 / demand[3].max(1.0)).min(1.0),
        ];
        let pieces = |x: f32, rng: &mut Rng| x.floor() as i32 + rng.chance(x.fract()) as i32;
        for t in &inputs.trees {
            if t.shed < 0.05 {
                continue;
            }
            let reach = t.crown.max(0.8);
            let around = |rng: &mut Rng, spread: f32| {
                let a = rng.range(0.0, TAU);
                let r = t.trunk + 0.25 + rng.f32().sqrt() * reach * spread;
                t.pos + Vec2::new(a.cos(), a.sin()) * r
            };
            let [twigs, branch, cones, leaves] = shed(t);
            for _ in 0..pieces(twigs * thin[0], &mut rng) {
                let p = around(&mut rng, 1.0);
                push(DebrisKind::Twig, p, &mut rng, &mut out);
            }
            if rng.chance(branch * thin[1]) {
                let p = around(&mut rng, 0.9);
                push(DebrisKind::Branch, p, &mut rng, &mut out);
            }
            let cone = if t.spruce {
                DebrisKind::SpruceCone
            } else {
                DebrisKind::PineCone
            };
            for _ in 0..pieces(cones * thin[2], &mut rng) {
                let p = around(&mut rng, 0.85);
                push(cone, p, &mut rng, &mut out);
            }
            for _ in 0..pieces(leaves * thin[3], &mut rng) {
                let p = around(&mut rng, 1.0);
                push(DebrisKind::Leaves, p, &mut rng, &mut out);
            }
        }
        // Reeds and shore pebbles around ponds.
        for pool in cover.pools.iter().filter(|p| !p.mineral) {
            let c = Vec2::from(pool.center);
            let steps = (TAU * pool.radius / 1.1) as i32;
            for i in 0..steps {
                let a = i as f32 / steps as f32 * TAU + rng.range(-0.15, 0.15);
                let gap = perlin2(
                    a.cos() as f64 * 1.4,
                    a.sin() as f64 * 1.4,
                    cover.seed ^ 0x2eed,
                );
                if gap < -0.15 {
                    continue;
                }
                let r = pool.radius + rng.range(-0.75, 0.05);
                push(
                    DebrisKind::Reeds,
                    c + Vec2::new(a.cos(), a.sin()) * r,
                    &mut rng,
                    &mut out,
                );
            }
            for _ in 0..(pool.radius * 14.0) as i32 {
                let a = rng.range(0.0, TAU);
                let r = pool.radius + rng.range(-0.5, 1.3);
                push(
                    DebrisKind::Pebble,
                    c + Vec2::new(a.cos(), a.sin()) * r,
                    &mut rng,
                    &mut out,
                );
            }
        }
    }
    // Pebbles: a sparse sprinkle, thicker on dry ground and along the trail's edges.
    let area = cover.half_extents[0] * cover.half_extents[1] * 4.0;
    for _ in 0..(area * 0.06 * density) as i32 {
        let p = Vec2::new(
            rng.range(-cover.half_extents[0], cover.half_extents[0]),
            rng.range(-cover.half_extents[1], cover.half_extents[1]),
        );
        let world = inputs.center + DVec3::new(p.x as f64, 0.0, p.y as f64);
        let [water, _, _] = inputs.soil(world);
        let mut keep = 0.12 + 0.7 * smooth(0.35, 0.15, water);
        if cover.trail.len() >= 2 {
            let d = polyline_distance(p, &cover.trail) / cover.trail_half_width.max(0.1);
            keep += 0.8 * smooth(1.6, 1.0, d);
        }
        if cover.style == CoverStyle::CaveFloor {
            keep = 0.5;
        }
        if rng.chance(keep) {
            push(DebrisKind::Pebble, p, &mut rng, &mut out);
        }
    }
    out
}

/// Shared meshes and materials of scattered debris and reeds.
#[derive(Resource, Clone)]
pub struct SceneryAssets {
    pub twigs: Vec<Handle<Mesh>>,
    pub branches: Vec<Handle<Mesh>>,
    pub pine_cones: Vec<Handle<Mesh>>,
    pub spruce_cones: Vec<Handle<Mesh>>,
    pub leaf_clumps: Vec<Handle<Mesh>>,
    pub reeds: Vec<Handle<Mesh>>,
    pub bark: Handle<StandardMaterial>,
    pub cones: Handle<StandardMaterial>,
    pub leaves: Handle<StandardMaterial>,
    pub reed: Handle<StandardMaterial>,
}

fn scenery_assets(world: &mut World) -> Option<SceneryAssets> {
    if let Some(a) = world.get_resource::<SceneryAssets>() {
        return Some(a.clone());
    }
    let atlas = world
        .get_resource_mut::<Assets<Image>>()?
        .add(fallen_leaf_atlas(256));
    let mut meshes = world.get_resource_mut::<Assets<Mesh>>()?;
    let mut add_all = |n: u32, f: &dyn Fn(u64) -> Mesh| -> Vec<Handle<Mesh>> {
        (0..n)
            .map(|i| meshes.add(f(0x5ce0 + i as u64 * 7919)))
            .collect()
    };
    let twigs = add_all(TWIG_VARIANTS, &|s| twig_mesh(s, false));
    let branches = add_all(BRANCH_VARIANTS, &|s| twig_mesh(s ^ 0xb7, true));
    let pine_cones = add_all(CONE_VARIANTS, &|s| cone_mesh(false, s));
    let spruce_cones = add_all(CONE_VARIANTS, &|s| cone_mesh(true, s));
    let leaf_clumps = add_all(LEAF_VARIANTS, &leaf_clump_mesh);
    let reeds = add_all(REED_VARIANTS, &reed_mesh);
    let mut materials = world.get_resource_mut::<Assets<StandardMaterial>>()?;
    let bark = materials.add(StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.9));
    let cones = materials.add(StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.78));
    let mut leaf = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.72)
        .with_subsurface(0.25)
        .with_base_color_texture(atlas);
    leaf.alpha_mode = AlphaMode::Mask(0.5);
    leaf.double_sided = true;
    leaf.reflectance = 0.35;
    let leaves = materials.add(leaf);
    let mut reed = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.62)
        .with_subsurface(0.35)
        .with_wind_sway(0.07);
    reed.reflectance = 0.35;
    reed.double_sided = true;
    let reed = materials.add(reed);
    let assets = SceneryAssets {
        twigs,
        branches,
        pine_cones,
        spruce_cones,
        leaf_clumps,
        reeds,
        bark,
        cones,
        leaves,
        reed,
    };
    world.insert_resource(assets.clone());
    Some(assets)
}

/// A piece of scattered debris (twig, branch, cone, leaves, reeds); hidden beyond
/// `draw_distance_m` from the camera.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LitterPiece {
    pub draw_distance_m: f32,
}

impl Default for LitterPiece {
    fn default() -> Self {
        Self {
            draw_distance_m: 45.0,
        }
    }
}

/// Hide debris far from the camera (every few frames).
pub fn cull_debris(
    mut frame: Local<u32>,
    cams: Query<(&engine_render::Camera, &GlobalTransform)>,
    mut debris: Query<(&LitterPiece, &GlobalTransform, &mut MeshRenderer)>,
) {
    *frame = frame.wrapping_add(1);
    if *frame % 8 != 1 {
        return;
    }
    let Some(cam) = cams
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
    else {
        return;
    };
    for (d, t, mut r) in &mut debris {
        let visible = t.translation().distance(cam) < d.draw_distance_m as f64;
        if r.visible != visible {
            r.visible = visible;
        }
    }
}

/// A cover waiting for work in [`build_scenery`].
struct CoverJob {
    entity: Entity,
    cover: GroundCover,
    center: DVec3,
    new: bool,
    version: u64,
}

fn cover_local(cover: &GroundCover, center: DVec3, p: DVec3) -> Option<Vec2> {
    let l = Vec2::new((p.x - center.x) as f32, (p.z - center.z) as f32);
    let e = Vec2::new(l.x / cover.half_extents[0], l.y / cover.half_extents[1]);
    let inside = if cover.round {
        e.length() <= 1.0
    } else {
        e.x.abs() <= 1.0 && e.y.abs() <= 1.0
    };
    inside.then_some(l)
}

fn tree_spots(world: &mut World, cover: &GroundCover, center: DVec3) -> Vec<TreeSpot> {
    let Some(library) = world.get_resource::<PlantLibrary>().cloned() else {
        return Vec::new();
    };
    let mut q = world.query::<(&Plant, &Transform)>();
    let mut out = Vec::new();
    for (p, t) in q.iter(world) {
        let sp = library.get(p.species);
        if !sp.is_woody() {
            continue;
        }
        let Some(pos) = cover_local(cover, center, t.translation) else {
            continue;
        };
        let shed = match p.stage {
            PlantStage::Alive => {
                if p.size < 0.12 {
                    continue;
                }
                0.3 + 0.7 * p.size.min(1.0)
            }
            PlantStage::Stump { .. } => 0.25,
            PlantStage::Dead { .. } => 0.45,
            PlantStage::Burnt { .. } => 0.0,
        };
        out.push(TreeSpot {
            pos,
            crown: p.crown_radius(sp),
            trunk: (sp.trunk_radius_m * p.size * p.genes.crown).max(0.03),
            conifer: sp.form == PlantForm::Conifer,
            spruce: sp.name.contains("spruce"),
            shed: shed
                * if sp.form == PlantForm::Shrub {
                    0.5
                } else {
                    1.0
                },
            shrub: sp.form == PlantForm::Shrub,
        });
    }
    out
}

/// A cover's rebuild worked out: the rocks it settled, its surface and where debris goes.
struct CoverBuilt {
    job: CoverJob,
    inputs: CoverInputs,
    moved: Vec<(Entity, Rock, Transform)>,
    surface: GroundSurface,
    debris: PlacedDebris,
    ms: f32,
}

/// Cover rebuilds running off the frame, by cover entity.
#[derive(Resource, Default)]
pub struct SceneryBuilds {
    pending: HashMap<Entity, Arc<Mutex<Option<CoverBuilt>>>>,
}

/// Settle `rocks` on the job's cover, then build its surface and (for a new cover) place its
/// debris: the slow part of a rebuild, needing nothing from the world.
fn build_cover(
    job: CoverJob,
    mut inputs: CoverInputs,
    mut rocks: Vec<(Entity, Rock, Transform)>,
    ground: Option<Ground>,
) -> CoverBuilt {
    let start = std::time::Instant::now();
    inputs.prepare(&job.cover);
    let ground_y = |p: DVec3| {
        ground
            .as_ref()
            .and_then(|g| g.height(p + DVec3::Y * 4.0))
            .unwrap_or(job.center.y)
    };
    let mut moved = Vec::new();
    for (e, rock, t) in &mut rocks {
        if rock.settled {
            continue;
        }
        let radius = 0.5 * t.scale.x.max(t.scale.z) * 0.85;
        let mut local = cover_local(&job.cover, job.center, t.translation).unwrap_or_default();
        for _ in 0..3 {
            for tree in &inputs.trees {
                let d = local - tree.pos;
                let need = radius + tree.trunk + 0.15;
                if d.length() < need {
                    local = tree.pos + d.normalize_or(Vec2::X) * need;
                }
            }
        }
        let s = sample_cover(&job.cover, &inputs, local);
        let world_p = job.center + DVec3::new(local.x as f64, 0.0, local.y as f64);
        let y = ground_y(world_p) + s.height as f64 - rock.bury_m as f64;
        t.translation = DVec3::new(world_p.x, y, world_p.z);
        rock.water_m = job
            .cover
            .pools
            .iter()
            .filter(|p| (local - Vec2::from(p.center)).length() < p.radius + 1.5)
            .map(|p| (p.water_m - s.height).max(0.0))
            .fold(rock.water_m, f32::max);
        rock.settled = true;
        moved.push((*e, rock.clone(), *t));
    }
    inputs.rocks = rocks
        .iter()
        .filter_map(|(_, r, t)| {
            Some(RockSpot {
                pos: cover_local(&job.cover, job.center, t.translation)?,
                radius: 0.5 * t.scale.x.max(t.scale.z) * 0.85,
                moss: r.moss,
            })
        })
        .collect();
    inputs.prepare(&job.cover);
    let surface = build_surface(&job.cover, &inputs, ground.as_ref(), job.version + 1);
    let debris = if job.new {
        place_debris(
            &job.cover,
            &inputs,
            &scatter_debris(&job.cover, &inputs),
            &ground_y,
        )
    } else {
        PlacedDebris::default()
    };
    CoverBuilt {
        job,
        inputs,
        moved,
        surface,
        debris,
        ms: start.elapsed().as_secs_f32() * 1000.0,
    }
}

/// Builds and keeps up the scenery: settles new [`Rock`]s on their ground cover (away from
/// trunks, at the right depth, wet where they stand in a pool), builds the [`GroundSurface`]
/// of every new or edited [`GroundCover`] (and rebuilds it when rocks settle on it), and
/// scatters debris and reeds over a new cover from the plants that grew there. Covers wait
/// for the ecology regions to be planted (their trees shape the floor); the rebuilds run on
/// the async task pool with [`FoliageLoading::background`] and land when done.
pub fn build_scenery(world: &mut World) {
    world.init_resource::<SceneryBuilds>();
    let background = world
        .get_resource::<FoliageLoading>()
        .is_none_or(|l| l.background);
    // Put the finished rebuilds in place.
    let done: Vec<CoverBuilt> = {
        let mut builds = world.resource_mut::<SceneryBuilds>();
        let ready: Vec<Entity> = builds
            .pending
            .iter()
            .filter(|(_, slot)| slot.lock().is_ok_and(|s| s.is_some()))
            .map(|(&e, _)| e)
            .collect();
        ready
            .into_iter()
            .filter_map(|e| builds.pending.remove(&e))
            .filter_map(|slot| slot.lock().ok().and_then(|mut s| s.take()))
            .collect()
    };
    for built in done {
        place_cover(world, built);
    }
    let regions_growing = world
        .query_filtered::<(), (With<EcologyRegion>, Without<RegionState>)>()
        .iter(world)
        .next()
        .is_some();
    let pending: HashSet<Entity> = world
        .resource::<SceneryBuilds>()
        .pending
        .keys()
        .copied()
        .collect();
    let mut covers = world.query::<(Entity, &GroundCover, &Transform, Option<&GroundSurface>)>();
    let mut jobs: Vec<CoverJob> = Vec::new();
    let mut all_covers: Vec<(Entity, GroundCover, DVec3, u64)> = Vec::new();
    for (e, c, t, s) in covers.iter(world) {
        let version = s.map_or(0, |s| s.version);
        all_covers.push((e, c.clone(), t.translation, version));
        if s.is_none_or(|s| &s.built_from != c) && !pending.contains(&e) {
            jobs.push(CoverJob {
                entity: e,
                cover: c.clone(),
                center: t.translation,
                new: s.is_none(),
                version,
            });
        }
    }
    let mut rq = world.query::<(Entity, &Rock, &Transform)>();
    let unsettled: Vec<(Entity, Rock, Transform)> = rq
        .iter(world)
        .filter(|(_, r, _)| !r.settled)
        .map(|(e, r, t)| (e, r.clone(), *t))
        .collect();
    let mut outside = Vec::new();
    for (re, rock, t) in unsettled {
        let mut on_one = false;
        for (e, c, center, version) in &all_covers {
            if cover_local(c, *center, t.translation).is_none() {
                continue;
            }
            on_one = true;
            if !pending.contains(e) && !jobs.iter().any(|j| j.entity == *e) {
                jobs.push(CoverJob {
                    entity: *e,
                    cover: c.clone(),
                    center: *center,
                    new: false,
                    version: *version,
                });
            }
        }
        if !on_one {
            outside.push((re, rock));
        }
    }
    // Rocks outside every cover keep their place.
    for (e, mut rock) in outside {
        rock.settled = true;
        world.entity_mut(e).insert(rock);
    }
    let waiting = jobs.len();
    let set_building = |world: &mut World, n: usize| {
        if let Some(mut l) = world.get_resource_mut::<FoliageLoading>() {
            l.bypass_change_detection().scenery_building = n;
        }
    };
    if jobs.is_empty() || regions_growing {
        set_building(world, pending.len() + waiting);
        return;
    }
    let ground = world.get_resource::<Ground>().cloned();
    let soils: Vec<Soil> = world
        .query::<&RegionState>()
        .iter(world)
        .map(|r| r.sim.soil.clone())
        .collect();
    let pool = AsyncComputeTaskPool::try_get().filter(|_| background);
    for job in jobs {
        let trees = tree_spots(world, &job.cover, job.center);
        let inputs = CoverInputs::new(job.center, soils.clone(), trees, Vec::new());
        let rocks: Vec<(Entity, Rock, Transform)> = rq
            .iter(world)
            .filter(|(_, _, t)| cover_local(&job.cover, job.center, t.translation).is_some())
            .map(|(e, r, t)| (e, r.clone(), *t))
            .collect();
        let ground = ground.clone();
        match pool {
            Some(pool) => {
                let slot = Arc::new(Mutex::new(None));
                let out = slot.clone();
                let entity = job.entity;
                pool.spawn(async move {
                    let built = build_cover(job, inputs, rocks, ground);
                    if let Ok(mut s) = out.lock() {
                        *s = Some(built);
                    }
                })
                .detach();
                world
                    .resource_mut::<SceneryBuilds>()
                    .pending
                    .insert(entity, slot);
            }
            None => place_cover(world, build_cover(job, inputs, rocks, ground)),
        }
    }
    let pending = world.resource::<SceneryBuilds>().pending.len();
    set_building(world, pending);
}

/// Put a cover's rebuild into the world: its settled rocks (those still there and still
/// unsettled), its surface and, for a new cover, its debris.
fn place_cover(world: &mut World, built: CoverBuilt) {
    let CoverBuilt {
        job,
        inputs,
        moved,
        surface,
        debris,
        ms,
    } = built;
    if world.get_entity(job.entity).is_err() {
        return;
    }
    for (e, rock, t) in moved {
        if world.get::<Rock>(e).is_some_and(|r| !r.settled) {
            world.entity_mut(e).insert((rock, t));
        }
    }
    let triangles = surface.triangle_count();
    world.entity_mut(job.entity).insert(surface);
    let scattered = debris.len();
    if scattered > 0
        && let Some(assets) = scenery_assets(world)
    {
        debris.spawn(world, &assets, job.entity);
    }
    tracing::info!(
        "foliage: ground cover {} built ({} triangles, {} rocks, {} debris) in {:.0} ms",
        job.entity,
        triangles,
        inputs.rocks.len(),
        scattered,
        ms
    );
}

/// Where each piece of debris and each pebble goes: the slow part of scattering (a cover
/// sample per piece), needing nothing from the world.
fn place_debris(
    cover: &GroundCover,
    inputs: &CoverInputs,
    spots: &[DebrisSpot],
    ground_y: &dyn Fn(DVec3) -> f64,
) -> PlacedDebris {
    let mut batch = Vec::new();
    let mut rocks = Vec::new();
    for s in spots {
        let sample = sample_cover(cover, inputs, s.pos);
        let p = inputs.center + DVec3::new(s.pos.x as f64, 0.0, s.pos.y as f64);
        let y = ground_y(p) + sample.height as f64;
        let rotation = Quat::from_rotation_y(s.yaw);
        if s.kind == DebrisKind::Pebble {
            let mut rng = Rng::new(hash_u64(
                s.pos.x.to_bits() as u64 ^ (s.pos.y.to_bits() as u64) << 32,
            ));
            let shore = cover
                .pools
                .iter()
                .any(|p| (s.pos - Vec2::from(p.center)).length() < p.radius + 1.5);
            let big = rng.chance(0.08);
            let w = if big {
                rng.range(0.14, 0.3)
            } else {
                rng.range(0.035, 0.11)
            } * s.scale;
            let size = Vec3::new(w, w * rng.range(0.45, 0.8), w * rng.range(0.6, 1.0));
            let shape = if shore || cover.style == CoverStyle::CaveFloor {
                ROCK_SHAPES - 1 - s.variant % ROUNDED_SHAPES
            } else {
                s.variant % ROCK_SHAPES
            };
            let water_m = cover
                .pools
                .iter()
                .map(|p| (p.water_m - sample.height).max(0.0))
                .fold(0.0, f32::max);
            let bury = size.y * rng.range(0.2, 0.45);
            rocks.push((
                Transform::from_translation(DVec3::new(p.x, y - bury as f64, p.z))
                    .with_rotation(rotation)
                    .with_scale(size),
                Rock {
                    shape,
                    bury_m: bury,
                    moss: if shore { 0.0 } else { 0.25 },
                    lichen: 0.2,
                    water_m,
                    tint: [rng.range(0.85, 1.1); 3],
                    settled: true,
                },
            ));
            continue;
        }
        // Reeds stand in the water: their base a little below the surface.
        let y = if s.kind == DebrisKind::Reeds {
            y.min(ground_y(p) + cover.pools.first().map_or(0.0, |p| p.water_m) as f64)
        } else {
            y
        };
        batch.push((
            Transform::from_translation(DVec3::new(p.x, y, p.z))
                .with_rotation(rotation)
                .with_scale(Vec3::splat(s.scale)),
            s.kind,
            s.variant as usize,
        ));
    }
    PlacedDebris {
        pieces: batch,
        pebbles: rocks,
    }
}

/// Debris placed by [`place_debris`]: pieces by kind and variant, and pebbles as rocks.
#[derive(Default)]
struct PlacedDebris {
    pieces: Vec<(Transform, DebrisKind, usize)>,
    pebbles: Vec<(Transform, Rock)>,
}

impl PlacedDebris {
    fn len(&self) -> usize {
        self.pieces.len() + self.pebbles.len()
    }

    fn spawn(self, world: &mut World, assets: &SceneryAssets, cover: Entity) {
        let batch: Vec<_> = self
            .pieces
            .into_iter()
            .map(|(t, kind, v)| {
                let (mesh, material, shadows, distance) = match kind {
                    DebrisKind::Twig => (assets.twigs[v], assets.bark, true, 30.0),
                    DebrisKind::Branch => (assets.branches[v], assets.bark, true, 70.0),
                    DebrisKind::PineCone => (assets.pine_cones[v], assets.cones, true, 22.0),
                    DebrisKind::SpruceCone => (assets.spruce_cones[v], assets.cones, true, 22.0),
                    DebrisKind::Leaves | DebrisKind::Pebble => {
                        (assets.leaf_clumps[v], assets.leaves, false, 30.0)
                    }
                    DebrisKind::Reeds => (assets.reeds[v], assets.reed, true, 90.0),
                };
                let mut renderer = MeshRenderer::new(mesh, material);
                renderer.cast_shadows = shadows;
                (
                    t,
                    renderer,
                    LitterPiece {
                        draw_distance_m: distance,
                    },
                    CoverPiece(cover),
                )
            })
            .collect();
        world.spawn_batch(batch);
        world.spawn_batch(
            self.pebbles
                .into_iter()
                .map(|(t, rock)| (t, rock, CoverPiece(cover))),
        );
    }
}

/// Litter and pebbles a ground cover scattered (world roots, gone with the cover).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct CoverPiece(pub Entity);

/// When a ground cover goes (its zone unloaded), the litter and pebbles it scattered go too.
pub fn drop_unloaded_covers(
    mut commands: Commands,
    mut removed: RemovedComponents<GroundCover>,
    covers: Query<(), With<GroundCover>>,
    pieces: Query<(Entity, &CoverPiece)>,
) {
    let gone: Vec<Entity> = removed.read().filter(|e| !covers.contains(*e)).collect();
    if gone.is_empty() {
        return;
    }
    for (e, p) in &pieces {
        if gone.contains(&p.0) {
            commands.entity(e).despawn();
        }
    }
}

// ---------------------------------------------------------------- rendering

/// Scenery shader (composed after `lib_types` + `lib_frame`): the layered ground and rocks.
pub const SCENERY_SHADER: &str = r#"
// Ground layers: a texture array, one layer per GroundLayer.
@group(1) @binding(0) var layer_albedo: texture_2d_array<f32>;  // rgb albedo (sRGB), a height
@group(1) @binding(1) var layer_normal: texture_2d_array<f32>;  // rg normal xy, b roughness, a occlusion
@group(1) @binding(2) var layer_sampler: sampler;

struct Cover {
    offset: vec4<f32>,               // camera-relative origin xyz
    layers: array<vec4<u32>, 2>,     // texture layer of each weight slot
    inv_tile: array<vec4<f32>, 2>,   // 1 / tile size (m) of each slot
    params: vec4<f32>,               // height influence, blend depth, normal strength, litter season
};
@group(2) @binding(0) var<uniform> cover: Cover;

const L_HUMUS: u32 = 0u;
const L_LITTER: u32 = 1u;
const L_NEEDLES: u32 = 2u;
const L_MOSS: u32 = 3u;
const L_EARTH: u32 = 4u;
const L_ASH: u32 = 5u;
const L_MUD: u32 = 6u;
const L_ROCK: u32 = 7u;
const L_CRUST: u32 = 8u;

// How much each layer darkens when wet (porosity), its translucency, its normal strength.
fn layer_porosity(l: u32) -> f32 {
    switch l {
        case 0u: { return 0.55; }
        case 1u: { return 0.45; }
        case 2u: { return 0.4; }
        case 3u: { return 0.35; }
        case 4u: { return 0.5; }
        case 5u: { return 0.6; }
        case 6u: { return 0.35; }
        case 7u: { return 0.3; }
        default: { return 0.25; }
    }
}

fn layer_subsurface(l: u32) -> f32 {
    switch l {
        case 1u: { return 0.15; }
        case 3u: { return 0.3; }
        default: { return 0.0; }
    }
}

// Fallen leaves through the year (`cover.params.w`, from `season::litter_season`): above 0
// freshly fallen autumn leaves in patches of birch yellow, maple orange and oak russet; below
// 0 the dark, sodden litter of winter. The painted litter's own light and dark leaves show
// through.
fn seasonal_litter(albedo: vec3<f32>, wp: vec2<f32>) -> vec3<f32> {
    let s = cover.params.w;
    if (s > 0.0) {
        let hue = value_noise2(wp * 0.45 + vec2(17.0, 5.0)) + 0.35 * value_noise2(wp * 2.3);
        let yellow = vec3(0.66, 0.38, 0.045);
        let orange = vec3(0.5, 0.15, 0.018);
        let russet = vec3(0.27, 0.085, 0.02);
        let c = mix(mix(russet, orange, smoothstep(0.25, 0.6, hue)), yellow, smoothstep(0.7, 1.05, hue));
        let w = vec3(0.3, 0.55, 0.15);
        let fresh = c * (dot(albedo, w) / dot(c, w)) * 1.4;
        return mix(albedo, fresh, s * 0.85);
    }
    return albedo * mix(vec3(1.0), vec3(0.68, 0.66, 0.64), -s);
}

struct Mat {
    albedo: vec3<f32>,
    n: vec2<f32>,
    rough: f32,
    ao: f32,
    height: f32,
};

fn rot2(a: f32) -> mat2x2<f32> {
    let c = cos(a);
    let s = sin(a);
    return mat2x2<f32>(c, s, -s, c);
}

fn fetch(layer: u32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> Mat {
    let a = textureSampleGrad(layer_albedo, layer_sampler, uv, i32(layer), dx, dy);
    let t = textureSampleGrad(layer_normal, layer_sampler, uv, i32(layer), dx, dy);
    var m: Mat;
    m.albedo = a.rgb;
    m.height = a.a;
    m.n = t.rg * 2.0 - 1.0;
    m.rough = t.b;
    m.ao = t.a;
    return m;
}

fn mix_mat(a: Mat, b: Mat, t: f32) -> Mat {
    var m: Mat;
    m.albedo = mix(a.albedo, b.albedo, t);
    m.height = mix(a.height, b.height, t);
    m.n = mix(a.n, b.n, t);
    m.rough = mix(a.rough, b.rough, t);
    m.ao = mix(a.ao, b.ao, t);
    return m;
}

// A layer at local position p (meters): two differently rotated and scaled samples, blended by
// a world-space noise (`mixer`), so the tile repeat never lines up. Normals come back in the
// cover's x/z frame.
fn sample_layer(layer: u32, p: vec2<f32>, inv_tile: f32, dpx: vec2<f32>, dpy: vec2<f32>, mixer: f32) -> Mat {
    let r1 = rot2(f32(layer) * 2.3999);
    let r2 = rot2(f32(layer) * 2.3999 + 1.9);
    let s2 = inv_tile * 0.79;
    var m: Mat;
    if (mixer < 0.99) {
        m = fetch(layer, r1 * p * inv_tile, r1 * dpx * inv_tile, r1 * dpy * inv_tile);
        m.n = transpose(r1) * m.n;
    }
    if (mixer > 0.01) {
        var b = fetch(layer, r2 * p * s2 + vec2(0.37, 0.71), r2 * dpx * s2, r2 * dpy * s2);
        b.n = transpose(r2) * b.n;
        if (mixer >= 0.99) {
            m = b;
        } else {
            m = mix_mat(m, b, mixer);
        }
    }
    return m;
}

fn slot_layer(i: u32) -> u32 {
    return cover.layers[i >> 2u][i & 3u];
}

fn slot_inv_tile(i: u32) -> f32 {
    return cover.inv_tile[i >> 2u][i & 3u];
}

struct GroundIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) w0: vec4<f32>,
    @location(3) w1: vec4<f32>,
    @location(4) extra: vec4<f32>,
    @location(5) tint: vec4<f32>,
};

struct GroundOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) local: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) w0: vec4<f32>,
    @location(4) w1: vec4<f32>,
    @location(5) extra: vec4<f32>,
    @location(6) tint: vec4<f32>,
};

@vertex
fn vs_ground(in: GroundIn) -> GroundOut {
    var out: GroundOut;
    out.rel = cover.offset.xyz + in.position;
    out.local = in.position;
    out.normal = in.normal;
    out.w0 = in.w0;
    out.w1 = in.w1;
    out.extra = in.extra;
    out.tint = in.tint;
    out.clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    return out;
}

// Wet ground darkens and turns glossy; above `puddles` water stands in the low spots.
fn wet_surface(s: ptr<function, Surface>, wet: f32, porosity: f32, height: f32, geo_n: vec3<f32>) {
    (*s).albedo *= mix(1.0, 0.45, wet * porosity * 1.6);
    (*s).roughness = mix((*s).roughness, 0.16, wet * 0.8);
    let level = saturate((wet - 0.55) * 2.2) * 0.62;
    let puddle = smoothstep(level - 0.02, level - 0.09, height) * step(0.56, wet);
    (*s).albedo *= mix(1.0, 0.32, puddle);
    (*s).roughness = mix((*s).roughness, 0.03, puddle);
    (*s).normal = normalize(mix((*s).normal, geo_n, puddle));
    (*s).occlusion = mix((*s).occlusion, 1.0, puddle * 0.5);
}

fn weather_surface(s: ptr<function, Surface>, rel: vec3<f32>) {
    let n = (*s).normal;
    let wet = frame.weather.x * saturate(n.y * 0.7 + 0.5);
    (*s).albedo *= mix(1.0, 0.55, wet);
    (*s).roughness = mix((*s).roughness, 0.12, wet * 0.85);
    let snow_cover = saturate(frame.weather.y * 10.0) * smoothstep(0.35, 0.8, n.y);
    let snow_noise = value_noise2(tiling_pos(rel).xz * 1.7);
    let snow = saturate(snow_cover * (0.75 + 0.5 * snow_noise));
    (*s).albedo = mix((*s).albedo, vec3<f32>(0.9, 0.92, 0.95), snow);
    (*s).roughness = mix((*s).roughness, 0.75, snow);
}

fn finish(s: Surface, rel: vec3<f32>) -> vec4<f32> {
    if (frame.counts.z == 1u) {
        return vec4<f32>(s.normal * 0.5 + 0.5, 1.0);
    }
    if (frame.counts.z == 3u) {
        return vec4<f32>(vec3(sky_visibility(rel, s.normal)), 1.0);
    }
    if (frame.counts.z == 2u) {
        // The sun's visibility, tinted by shadow cascade.
        let depth = view_depth(rel);
        var tint = vec3<f32>(1.0, 0.4, 0.4);
        if (depth > frame.shadow_splits[0]) { tint = vec3(0.4, 1.0, 0.4); }
        if (depth > frame.shadow_splits[1]) { tint = vec3(0.4, 0.4, 1.0); }
        if (depth > frame.shadow_splits[2]) { tint = vec3(1.0, 1.0, 0.4); }
        return vec4<f32>(tint * (0.05 + 0.95 * shadow_factor(rel, s.normal)), 1.0);
    }
    if (frame.counts.z == 4u) {
        return vec4<f32>(vec3(screen_ao(rel)), 1.0);
    }
    if (frame.counts.z == 5u) {
        let r = screen_reflection(rel);
        return vec4<f32>(pre_expose(r.rgb) * r.a, 1.0);
    }
    if (frame.counts.z == 6u) {
        return vec4<f32>(pre_expose(screen_gi(rel)), 1.0);
    }
    return vec4<f32>(apply_fog(shade_surface(s, rel), rel), 1.0);
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    return finish(ground_surface(in), in.rel);
}

// Depth, normal, roughness and motion for ambient occlusion, reflections and bounce light.
@fragment
fn fs_ground_prepass(in: GroundOut) -> PrepassOutput {
    return surface_prepass(ground_surface(in), in.rel);
}

fn surface_prepass(s: Surface, rel: vec3<f32>) -> PrepassOutput {
    return prepass_output(s.normal, s.roughness, motion_clip(rel), prev_motion_clip(static_prev_rel(rel)));
}

fn ground_surface(in: GroundOut) -> Surface {
    let p = in.local.xz;
    let dpx = dpdx(p);
    let dpy = dpdy(p);
    let wp = tiling_pos(in.rel);
    let mixer = smoothstep(0.36, 0.64, value_noise2(wp.xz * 0.21 + vec2(3.1, 7.7)));
    var w = array<f32, 8>(in.w0.x, in.w0.y, in.w0.z, in.w0.w, in.w1.x, in.w1.y, in.w1.z, in.w1.w);
    var mats: array<Mat, 8>;
    var score: array<f32, 8>;
    var best = -10.0;
    for (var i = 0u; i < 8u; i++) {
        score[i] = -10.0;
        if (w[i] > 0.004) {
            mats[i] = sample_layer(slot_layer(i), p, slot_inv_tile(i), dpx, dpy, mixer);
            if (slot_layer(i) == L_LITTER) {
                mats[i].albedo = seasonal_litter(mats[i].albedo, wp.xz);
            }
            score[i] = w[i] + mats[i].height * cover.params.x;
            best = max(best, score[i]);
        }
    }
    // Height blend: the highest layer (weight + its own height) wins, neighbors within
    // `depth` mix in, so leaves lie on soil and moss fills the gaps between stones.
    let floor_s = best - cover.params.y;
    var total = 0.0;
    var albedo = vec3(0.0);
    var nxy = vec2(0.0);
    var rough = 0.0;
    var ao = 0.0;
    var height = 0.0;
    var porosity = 0.0;
    var subsurface = 0.0;
    for (var i = 0u; i < 8u; i++) {
        let b = max(score[i] - floor_s, 0.0);
        if (b > 0.0) {
            let m = mats[i];
            let l = slot_layer(i);
            albedo += m.albedo * b;
            nxy += m.n * b;
            rough += m.rough * b;
            ao += m.ao * b;
            height += m.height * b;
            porosity += layer_porosity(l) * b;
            subsurface += layer_subsurface(l) * b;
            total += b;
        }
    }
    let inv = 1.0 / max(total, 1e-4);
    albedo *= inv;
    nxy *= inv;
    rough *= inv;
    ao *= inv;
    height *= inv;
    porosity *= inv;
    subsurface *= inv;

    // Macro variation: patches a few meters across, slightly lighter, darker, warmer.
    let m1 = fbm2(wp.xz * 0.09 + vec2(11.0, 3.0), 3);
    let m2 = value_noise2(wp.xz * 0.55 + vec2(5.0, 1.0));
    albedo *= in.tint.rgb * 2.0 * (0.8 + 0.45 * m1) * (0.93 + 0.14 * m2);
    albedo = mix(albedo, albedo * vec3(1.06, 1.0, 0.9), saturate(m2 - 0.3));

    let geo_n = normalize(in.normal);
    let t = normalize(vec3<f32>(1.0, 0.0, 0.0) - geo_n * geo_n.x);
    let bt = cross(t, geo_n);
    let strength = cover.params.z * (1.0 - 0.55 * in.extra.z);
    let txy = nxy * strength;
    let tz = sqrt(max(1.0 - dot(txy, txy), 0.05));
    var s = default_surface();
    s.normal = normalize(t * txy.x + bt * txy.y + geo_n * tz);
    s.albedo = albedo * (1.0 - 0.3 * in.extra.y);
    s.roughness = clamp(rough, 0.045, 1.0);
    s.reflectance = 0.4;
    s.occlusion = ao * (1.0 - 0.65 * in.extra.y);
    s.subsurface = subsurface;
    wet_surface(&s, in.extra.x, porosity, height, geo_n);
    weather_surface(&s, in.rel);
    return s;
}

// ---------------------------------------------------------------- rocks

struct RockIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) data: vec4<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) n0: vec4<f32>,
    @location(7) n1: vec4<f32>,
    @location(8) n2: vec4<f32>,
    @location(9) params: vec4<f32>,  // moss, lichen, ground rel y, water rel y
    @location(10) tint: vec4<f32>,   // rgb tint, seed
};

struct RockOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) data: vec4<f32>,
    @location(3) params: vec4<f32>,
    @location(4) tint: vec4<f32>,
};

@vertex
fn vs_rock(in: RockIn) -> RockOut {
    let p = vec4<f32>(in.position, 1.0);
    var out: RockOut;
    out.rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    out.normal = normalize(mat3x3<f32>(in.n0.xyz, in.n1.xyz, in.n2.xyz) * in.normal);
    out.data = in.data;
    out.params = in.params;
    out.tint = in.tint;
    out.clip = frame.view_proj * vec4<f32>(out.rel, 1.0);
    return out;
}

@fragment
fn fs_rock(in: RockOut) -> @location(0) vec4<f32> {
    return finish(rock_surface(in), in.rel);
}

@fragment
fn fs_rock_prepass(in: RockOut) -> PrepassOutput {
    return surface_prepass(rock_surface(in), in.rel);
}

fn rock_surface(in: RockOut) -> Surface {
    let wp = tiling_pos(in.rel);
    let q = wp + vec3(in.tint.w * 13.1, in.tint.w * 7.3, in.tint.w * 5.7);
    let ng = normalize(in.normal);
    var bw = pow(abs(ng), vec3(4.0));
    bw /= (bw.x + bw.y + bw.z);
    let it = 1.0 / 1.8;
    let ax = textureSample(layer_albedo, layer_sampler, q.zy * it, i32(L_ROCK));
    let ay = textureSample(layer_albedo, layer_sampler, q.xz * it, i32(L_ROCK));
    let az = textureSample(layer_albedo, layer_sampler, q.xy * it, i32(L_ROCK));
    let tx = textureSample(layer_normal, layer_sampler, q.zy * it, i32(L_ROCK));
    let ty = textureSample(layer_normal, layer_sampler, q.xz * it, i32(L_ROCK));
    let tz = textureSample(layer_normal, layer_sampler, q.xy * it, i32(L_ROCK));
    let mossy = textureSample(layer_albedo, layer_sampler, wp.xz / 1.8, i32(L_MOSS));
    let mossn = textureSample(layer_normal, layer_sampler, wp.xz / 1.8, i32(L_MOSS));
    let soil = textureSample(layer_albedo, layer_sampler, wp.xz / 2.0, i32(L_HUMUS));

    var albedo = ax.rgb * bw.x + ay.rgb * bw.y + az.rgb * bw.z;
    // Weathering: each rock a little different, rain streaks down the steep faces.
    albedo *= 0.52 + 0.32 * value_noise3(q * 0.6);
    let streak = value_noise3(vec3(q.x * 2.2, q.y * 0.3, q.z * 2.2));
    albedo *= mix(1.0, 0.68, smoothstep(0.45, 0.8, streak) * (1.0 - saturate(ng.y)));
    let height = ax.a * bw.x + ay.a * bw.y + az.a * bw.z;
    var rough = tx.b * bw.x + ty.b * bw.y + tz.b * bw.z;
    var ao = tx.a * bw.x + ty.a * bw.y + tz.a * bw.z;
    let k = 1.3;
    let nx = (tx.rg * 2.0 - 1.0) * k;
    let ny = (ty.rg * 2.0 - 1.0) * k;
    let nz = (tz.rg * 2.0 - 1.0) * k;
    var n = normalize(
        vec3(ng.x, ng.y + nx.y, ng.z + nx.x) * bw.x +
        vec3(ng.x + ny.x, ng.y, ng.z + ny.y) * bw.y +
        vec3(ng.x + nz.x, ng.y + nz.y, ng.z) * bw.z);
    albedo *= in.tint.rgb;

    // Lichen: crusty pale spots on the open faces.
    let l1 = value_noise3(q * 9.0) * 0.6 + value_noise3(q * 31.0) * 0.4;
    // In the dark (data.y), nothing grows.
    let lit = 1.0 - in.data.y;
    let lichen = smoothstep(0.63, 0.67, l1) * in.params.y * saturate(ng.y + 0.7) * lit;
    let lichen_c = mix(vec3(0.2, 0.22, 0.17), vec3(0.3, 0.22, 0.06), smoothstep(0.7, 0.8, value_noise3(q * 2.3)));
    albedo = mix(albedo, lichen_c, lichen * 0.65);
    rough = mix(rough, 0.92, lichen);

    // Moss on top: where faces look up, more in hollows, height-blended with the rock.
    let mn = value_noise2(wp.xz * 2.5) * 0.6 + value_noise2(wp.xz * 9.0) * 0.4;
    let up = smoothstep(0.1, 0.75, n.y + (mn - 0.5) * 0.7);
    let moss_w = in.params.x * up * lit;
    let moss = smoothstep(-0.06, 0.06, moss_w * 1.25 + mossy.a * 0.45 - 0.55 - height * 0.35);
    albedo = mix(albedo, mossy.rgb, moss);
    rough = mix(rough, mossn.b, moss);
    ao = mix(ao, mossn.a, moss);
    let mxy = mossn.rg * 2.0 - 1.0;
    n = normalize(mix(n, normalize(vec3(n.x + mxy.x * 0.6, n.y, n.z + mxy.y * 0.6)), moss));

    // Soil creeping up the base, and contact shade.
    let above = in.rel.y - in.params.z;
    let dirt = (1.0 - smoothstep(0.0, 0.16 + 0.12 * mn, above)) * (1.0 - moss);
    albedo = mix(albedo, soil.rgb * 0.85, dirt * 0.85);
    rough = mix(rough, 0.95, dirt);
    ao *= mix(0.45, 1.0, smoothstep(-0.02, 0.3, above));

    // Cavities from the mesh.
    ao *= in.data.x;
    albedo *= mix(0.5, 1.0, in.data.x);
    // Seeping water (data.z): darker, glossy rock.
    albedo *= 1.0 - 0.3 * in.data.z * (1.0 - moss);
    rough = mix(rough, 0.18, in.data.z * (1.0 - moss));

    var s = default_surface();
    s.albedo = albedo;
    s.normal = n;
    s.roughness = clamp(rough, 0.045, 1.0);
    s.reflectance = 0.45;
    s.occlusion = ao;
    s.subsurface = 0.3 * moss;

    // Standing in water: wet and dark below the surface, a slimy green band at the line.
    let below = in.params.w - in.rel.y;
    let wet = smoothstep(-0.05, 0.005, below);
    let band = smoothstep(-0.03, 0.0, below) * (1.0 - smoothstep(0.0, 0.2, below));
    s.albedo = mix(s.albedo, s.albedo * 0.45, wet);
    s.albedo = mix(s.albedo, vec3(0.035, 0.045, 0.02), band * 0.65);
    s.roughness = mix(s.roughness, 0.1, wet);
    weather_surface(&s, in.rel);
    return s;
}
"#;

/// Rock shadow shader (composed after `lib_types` only).
pub const ROCK_SHADOW_SHADER: &str = r#"
struct ShadowView {
    view_proj: mat4x4<f32>,
    wind: vec4<f32>,
    time: vec4<f32>,
    camera_frac: vec4<f32>,
};
@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

struct RockIn {
    @location(0) position: vec3<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
};

@vertex
fn vs_rock_shadow(in: RockIn) -> @builtin(position) vec4<f32> {
    let p = vec4<f32>(in.position, 1.0);
    let rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    var clip = shadow_view.view_proj * vec4<f32>(rel, 1.0);
    clip.z = max(clip.z, 0.0);
    return clip;
}
"#;

/// Bytes per rock instance: model rows, normal matrix columns, params, tint.
const ROCK_INSTANCE_BYTES: u64 = 128;

fn push_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

struct GpuCover {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    version: u64,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    slots: [GroundLayer; 8],
    /// Camera-relative origin this frame.
    offset: Vec3,
    seen: u64,
}

struct GpuRockMesh {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
}

#[derive(Clone, Copy)]
struct RockItem {
    mesh: u32,
    center: Vec3,
    radius: f32,
    casts: bool,
    visible: bool,
    instance: [f32; 32],
}

#[derive(Clone, Copy)]
struct RockBatch {
    mesh: u32,
    first: u32,
    count: u32,
}

/// The ground layers at [`LAYER_SIZE`], painted once per process.
static GROUND_LAYERS: std::sync::OnceLock<Vec<LayerTexture>> = std::sync::OnceLock::new();

/// Start painting the ground layers on a thread of their own, so the renderer finds them
/// ready when it first draws a cover instead of painting them in that frame.
pub fn start_painting_ground_layers() {
    if GROUND_LAYERS.get().is_none() {
        std::thread::spawn(ground_layers);
    }
}

/// The painted ground layers (waiting for the painting if it is still under way).
fn ground_layers() -> &'static [LayerTexture] {
    GROUND_LAYERS.get_or_init(|| paint_layers(LAYER_SIZE))
}

/// Draws [`GroundSurface`]s and [`Rock`]s (added by `FoliagePlugin`).
#[derive(Default)]
pub struct SceneryFeature {
    ground_pipeline: Option<wgpu::RenderPipeline>,
    ground_prepass_pipeline: Option<wgpu::RenderPipeline>,
    rock_pipeline: Option<wgpu::RenderPipeline>,
    rock_prepass_pipeline: Option<wgpu::RenderPipeline>,
    rock_shadow_pipeline: Option<wgpu::RenderPipeline>,
    texture_layout: Option<wgpu::BindGroupLayout>,
    cover_layout: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
    /// Layer textures (kept alive) and their bind group.
    textures: Option<(wgpu::Texture, wgpu::Texture, wgpu::BindGroup)>,
    covers: HashMap<Entity, GpuCover>,
    rock_meshes: Vec<GpuRockMesh>,
    /// Cave chambers' meshes in `rock_meshes`, by their recipe.
    chambers: HashMap<u64, u32>,
    instances: Option<GrowBuffer>,
    instance_bytes: Vec<u8>,
    items: Vec<RockItem>,
    rock_draws: Vec<RockBatch>,
    shadow_draws: Vec<Vec<RockBatch>>,
    frame: u64,
    stats: FeatureStats,
}

fn upload_layer_array(gpu: &GpuShared, layers: &[Vec<u8>], n: u32, srgb: bool) -> wgpu::Texture {
    let mips = 32 - n.leading_zeros();
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ground layers"),
        size: wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: layers.len() as u32,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, data) in layers.iter().enumerate() {
        let image = Image::new(n, n, data.clone(), srgb);
        let chain = image.mip_chain(srgb);
        let levels = std::iter::once(&image.data).chain(chain.iter());
        for (level, bytes) in levels.enumerate() {
            let size = (n >> level).max(1);
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture
}

fn buffer_init(
    gpu: &GpuShared,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    gpu.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        })
}

impl SceneryFeature {
    fn ensure_textures(&mut self, gpu: &GpuShared) {
        if self.textures.is_some() {
            return;
        }
        let (Some(layout), Some(sampler)) = (&self.texture_layout, &self.sampler) else {
            return;
        };
        let start = std::time::Instant::now();
        let layers = ground_layers();
        let albedo: Vec<Vec<u8>> = layers.iter().map(|l| l.albedo_height.clone()).collect();
        let normal: Vec<Vec<u8>> = layers.iter().map(|l| l.normal_rough_ao.clone()).collect();
        let n = LAYER_SIZE as u32;
        let albedo = upload_layer_array(gpu, &albedo, n, true);
        let normal = upload_layer_array(gpu, &normal, n, false);
        let view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ground layers"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view(&albedo)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&normal)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        tracing::info!(
            "foliage: painted {} ground layers ({}²) in {:.0} ms",
            LAYER_COUNT,
            LAYER_SIZE,
            start.elapsed().as_secs_f32() * 1000.0
        );
        self.textures = Some((albedo, normal, group));
    }

    fn ensure_rock_meshes(&mut self, gpu: &GpuShared) {
        if !self.rock_meshes.is_empty() {
            return;
        }
        for shape in 0..ROCK_SHAPES {
            for lod in 0..ROCK_LODS {
                self.rock_meshes
                    .push(upload_rock_mesh(gpu, &rock_mesh(shape, lod)));
            }
        }
    }

    fn prepare_covers(&mut self, world: &mut World, gpu: &GpuShared, view: &ViewData) {
        let (Some(layout), true) = (&self.cover_layout, self.textures.is_some()) else {
            return;
        };
        let frame = self.frame;
        let litter = world
            .get_resource::<crate::FoliageSeason>()
            .map_or(0.0, |s| crate::season::litter_season(s.day_of_year));
        let mut q = world.query::<(Entity, &GroundSurface, &GlobalTransform)>();
        for (e, surface, t) in q.iter(world) {
            let offset = (t.translation() - view.origin).as_vec3();
            if self
                .covers
                .get(&e)
                .is_none_or(|c| c.version != surface.version)
            {
                let mut bytes = Vec::with_capacity(surface.vertices.len() * GROUND_VERTEX_BYTES);
                for v in surface.vertices.iter() {
                    v.write(&mut bytes);
                }
                let index: Vec<u8> = surface
                    .indices
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect();
                let uniform = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("ground cover"),
                    size: 96,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ground cover"),
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    }],
                });
                self.covers.insert(
                    e,
                    GpuCover {
                        vertex: buffer_init(
                            gpu,
                            "ground vertices",
                            &bytes,
                            wgpu::BufferUsages::VERTEX,
                        ),
                        index: buffer_init(
                            gpu,
                            "ground indices",
                            &index,
                            wgpu::BufferUsages::INDEX,
                        ),
                        index_count: surface.indices.len() as u32,
                        version: surface.version,
                        uniform,
                        group,
                        slots: surface.slots,
                        offset,
                        seen: frame,
                    },
                );
            }
            if let Some(c) = self.covers.get_mut(&e) {
                c.offset = offset;
                c.seen = frame;
                let mut u = Vec::with_capacity(96);
                push_f32s(&mut u, &[offset.x, offset.y, offset.z, 0.0]);
                for s in c.slots {
                    u.extend_from_slice(&(s as u32).to_le_bytes());
                }
                for s in c.slots {
                    push_f32s(&mut u, &[1.0 / s.tile_m()]);
                }
                push_f32s(&mut u, &[0.55, 0.16, 1.0, litter]);
                gpu.queue.write_buffer(&c.uniform, 0, &u);
            }
        }
        self.covers.retain(|_, c| c.seen == frame);
    }

    /// Draw the ground covers and the rocks with these pipelines (the scene's or the
    /// prepass's).
    fn draw_opaque(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ground: Option<&wgpu::RenderPipeline>,
        rocks: Option<&wgpu::RenderPipeline>,
    ) {
        let Some((_, _, textures)) = &self.textures else {
            return;
        };
        if let Some(pipeline) = ground
            && !self.covers.is_empty()
        {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, textures, &[]);
            for c in self.covers.values() {
                pass.set_bind_group(2, &c.group, &[]);
                pass.set_vertex_buffer(0, c.vertex.slice(..));
                pass.set_index_buffer(c.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..c.index_count, 0, 0..1);
            }
        }
        if let (Some(pipeline), Some(instances)) = (rocks, &self.instances)
            && !self.rock_draws.is_empty()
        {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, textures, &[]);
            pass.set_vertex_buffer(1, instances.buffer.slice(..));
            for b in &self.rock_draws {
                let m = &self.rock_meshes[b.mesh as usize];
                pass.set_vertex_buffer(0, m.vertex.slice(..));
                pass.set_index_buffer(m.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..m.index_count, 0, b.first..b.first + b.count);
            }
        }
    }

    fn prepare_rocks(&mut self, world: &mut World, gpu: &GpuShared, view: &ViewData) {
        self.items.clear();
        self.prepare_chambers(world, gpu, view);
        let mut q = world.query::<(&Rock, &GlobalTransform)>();
        let shadows = view.sun_shadows || view.sky_occlusion.enabled;
        for (rock, t) in q.iter(world) {
            let scale = t.scale();
            let size = scale.max_element();
            let base = (t.translation() - view.origin).as_vec3();
            let center = base + Vec3::Y * scale.y * 0.5;
            let radius = size * 0.75;
            let dist = center.length();
            // Pebbles vanish in the distance; big rocks get coarser.
            if dist > 25.0 + size * 160.0 {
                continue;
            }
            let visible = view.frustum.intersects_sphere(center, radius);
            let casts = shadows && size > 0.2;
            if !visible && !casts {
                continue;
            }
            let lod = if dist < 12.0 + size * 10.0 {
                0
            } else if dist < 30.0 + size * 25.0 {
                1
            } else {
                2
            };
            let model = t.matrix_relative_to(view.origin);
            let normal = Mat3::from_mat4(model).inverse().transpose();
            let mut inst = [0.0f32; 32];
            inst[0..4].copy_from_slice(&model.row(0).to_array());
            inst[4..8].copy_from_slice(&model.row(1).to_array());
            inst[8..12].copy_from_slice(&model.row(2).to_array());
            inst[12..15].copy_from_slice(&normal.x_axis.to_array());
            inst[16..19].copy_from_slice(&normal.y_axis.to_array());
            inst[20..23].copy_from_slice(&normal.z_axis.to_array());
            let ground = base.y + rock.bury_m;
            let water = if rock.water_m > 0.0 {
                ground + rock.water_m
            } else {
                -1e6
            };
            inst[24..28].copy_from_slice(&[rock.moss, rock.lichen, ground, water]);
            let seed = unit(rock.shape as u64, (t.translation().x * 7.0) as i64 as u64);
            inst[28..32].copy_from_slice(&[rock.tint[0], rock.tint[1], rock.tint[2], seed]);
            self.items.push(RockItem {
                mesh: (rock.shape % ROCK_SHAPES) * ROCK_LODS as u32 + lod,
                center,
                radius,
                casts,
                visible,
                instance: inst,
            });
        }
        self.items.sort_unstable_by_key(|i| i.mesh);
        self.instance_bytes.clear();
        self.rock_draws.clear();
        let push_batches = |items: &mut dyn Iterator<Item = (u32, [f32; 32])>,
                            bytes: &mut Vec<u8>,
                            out: &mut Vec<RockBatch>| {
            for (mesh, inst) in items {
                let index = (bytes.len() as u64 / ROCK_INSTANCE_BYTES) as u32;
                push_f32s(bytes, &inst);
                match out.last_mut() {
                    Some(b) if b.mesh == mesh && b.first + b.count == index => b.count += 1,
                    _ => out.push(RockBatch {
                        mesh,
                        first: index,
                        count: 1,
                    }),
                }
            }
        };
        let mut visible = self
            .items
            .iter()
            .filter(|i| i.visible)
            .map(|i| (i.mesh, i.instance));
        push_batches(&mut visible, &mut self.instance_bytes, &mut self.rock_draws);
        // One list per sun cascade, then the sky occlusion map: rocks and cave roofs keep the
        // open sky off whatever lies under them.
        let cascades = if view.sun_shadows {
            (view.cascades.count as usize).min(MAX_CASCADES)
        } else {
            0
        };
        self.shadow_draws
            .resize_with(SKY_OCCLUSION_VIEW + 1, Vec::new);
        for (c, draws) in self.shadow_draws.iter_mut().enumerate() {
            draws.clear();
            let inside = |i: &RockItem| {
                if c == SKY_OCCLUSION_VIEW {
                    view.sky_occlusion.enabled
                        && view.sky_occlusion.contains_sphere(i.center, i.radius)
                } else {
                    c < cascades && view.cascades.sphere_in_cascade(c, i.center, i.radius)
                }
            };
            let mut casters: Vec<(u32, [f32; 32])> = self
                .items
                .iter()
                .filter(|i| i.casts && inside(i))
                .map(|i| {
                    // Shadows use the coarser shape; chambers their shadow caster, next to
                    // their rock.
                    if i.mesh >= BUILT_IN_ROCK_MESHES {
                        return (i.mesh + 1, i.instance);
                    }
                    let shape = i.mesh / ROCK_LODS as u32;
                    let lod = (i.mesh % ROCK_LODS as u32).max(1);
                    (shape * ROCK_LODS as u32 + lod, i.instance)
                })
                .collect();
            casters.sort_unstable_by_key(|c| c.0);
            push_batches(&mut casters.into_iter(), &mut self.instance_bytes, draws);
        }
    }
}

/// Rock meshes the feature makes itself: every shape at every level of detail. Cave chambers'
/// meshes follow them.
const BUILT_IN_ROCK_MESHES: u32 = ROCK_SHAPES * ROCK_LODS as u32;

fn upload_rock_mesh(gpu: &GpuShared, m: &RockMesh) -> GpuRockMesh {
    let mut bytes = Vec::with_capacity(m.vertices.len() * ROCK_VERTEX_BYTES);
    for v in &m.vertices {
        push_f32s(&mut bytes, &v.position);
        push_f32s(&mut bytes, &v.normal);
        bytes.extend_from_slice(&v.data);
    }
    let index: Vec<u8> = m.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    GpuRockMesh {
        vertex: buffer_init(gpu, "rock vertices", &bytes, wgpu::BufferUsages::VERTEX),
        index: buffer_init(gpu, "rock indices", &index, wgpu::BufferUsages::INDEX),
        index_count: m.indices.len() as u32,
    }
}

impl SceneryFeature {
    /// Cave chambers, drawn (and casting shadows) like rocks from their own meshes, built the
    /// first time each recipe is seen.
    fn prepare_chambers(&mut self, world: &mut World, gpu: &GpuShared, view: &ViewData) {
        use std::hash::{Hash, Hasher};
        let mut q = world.query::<(&crate::cave::CaveChamber, &GlobalTransform)>();
        for (chamber, t) in q.iter(world) {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{chamber:?}").hash(&mut h);
            let key = h.finish();
            let mesh = match self.chambers.get(&key) {
                Some(&i) => i,
                None => {
                    let start = std::time::Instant::now();
                    let m = crate::cave::cave_chamber_mesh(chamber);
                    tracing::info!(
                        "foliage: built a cave chamber ({} triangles) in {:.0} ms",
                        m.indices.len() / 3,
                        start.elapsed().as_secs_f32() * 1000.0
                    );
                    // The rock, then its shadow caster.
                    let i = self.rock_meshes.len() as u32;
                    self.rock_meshes.push(upload_rock_mesh(gpu, &m));
                    let shadow = crate::cave::cave_chamber_shadow_mesh(&m);
                    self.rock_meshes.push(upload_rock_mesh(gpu, &shadow));
                    self.chambers.insert(key, i);
                    i
                }
            };
            let [hx, hz] = chamber.half_extents;
            let base = (t.translation() - view.origin).as_vec3();
            let center = base + Vec3::Y * chamber.height_m * 0.5;
            let radius = Vec3::new(hx + 2.0, chamber.height_m * 0.5 + 2.0, hz + 2.0).length();
            let visible = view.frustum.intersects_sphere(center, radius);
            let casts = view.sun_shadows || view.sky_occlusion.enabled;
            if !visible && !casts {
                continue;
            }
            let model = t.matrix_relative_to(view.origin);
            let normal = Mat3::from_mat4(model).inverse().transpose();
            let mut inst = [0.0f32; 32];
            inst[0..4].copy_from_slice(&model.row(0).to_array());
            inst[4..8].copy_from_slice(&model.row(1).to_array());
            inst[8..12].copy_from_slice(&model.row(2).to_array());
            inst[12..15].copy_from_slice(&normal.x_axis.to_array());
            inst[16..19].copy_from_slice(&normal.y_axis.to_array());
            inst[20..23].copy_from_slice(&normal.z_axis.to_array());
            inst[24..28].copy_from_slice(&[chamber.moss, chamber.lichen, base.y, -1e6]);
            let seed = unit(chamber.seed, 0xca7e);
            let [r, g, b] = chamber.tint;
            inst[28..32].copy_from_slice(&[r, g, b, seed]);
            self.items.push(RockItem {
                mesh,
                center,
                radius,
                casts,
                visible,
                instance: inst,
            });
        }
    }
}

impl RenderFeature for SceneryFeature {
    fn name(&self) -> &str {
        "foliage scenery"
    }

    fn order(&self) -> i32 {
        5
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ground layers"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let cover_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ground cover"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ground layers"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        }));
        let shader = gpu.shader("foliage scenery", SCENERY_SHADER, true);
        let target = [Some(wgpu::ColorTargetState {
            format: gpu.formats.hdr,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let ground_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ground"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&texture_layout),
                Some(&cover_layout),
            ],
            immediate_size: 0,
        });
        let ground_attrs = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Unorm8x4, 4 => Unorm8x4, 5 => Unorm8x4,
        ];
        let ground_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: GROUND_VERTEX_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ground_attrs,
        })];
        let primitive = wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        };
        self.ground_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("ground"),
                layout: Some(&ground_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_ground"),
                    compilation_options: Default::default(),
                    buffers: &ground_buffers,
                },
                primitive,
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_ground"),
                    compilation_options: Default::default(),
                    targets: &target,
                }),
                multiview_mask: None,
                cache: None,
            },
        ));
        // Same vertices as the scene pass (the clip position is invariant), so its depth test
        // passes exactly where the prepass drew.
        self.ground_prepass_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("ground prepass"),
                layout: Some(&ground_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_ground"),
                    compilation_options: Default::default(),
                    buffers: &ground_buffers,
                },
                primitive,
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_ground_prepass"),
                    compilation_options: Default::default(),
                    targets: &gpu.prepass_targets(),
                }),
                multiview_mask: None,
                cache: None,
            },
        ));
        let rock_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rocks"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let rock_vertex_attrs =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4];
        let rock_instance_attrs = wgpu::vertex_attr_array![
            3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4,
            8 => Float32x4, 9 => Float32x4, 10 => Float32x4,
        ];
        let rock_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: ROCK_VERTEX_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &rock_vertex_attrs,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: ROCK_INSTANCE_BYTES,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &rock_instance_attrs,
            }),
        ];
        self.rock_pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rocks"),
                layout: Some(&rock_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_rock"),
                    compilation_options: Default::default(),
                    buffers: &rock_buffers,
                },
                primitive,
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_rock"),
                    compilation_options: Default::default(),
                    targets: &target,
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
        self.rock_prepass_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("rocks prepass"),
                layout: Some(&rock_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_rock"),
                    compilation_options: Default::default(),
                    buffers: &rock_buffers,
                },
                primitive,
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_rock_prepass"),
                    compilation_options: Default::default(),
                    targets: &gpu.prepass_targets(),
                }),
                multiview_mask: None,
                cache: None,
            },
        ));
        let shadow_shader = gpu.shader("foliage rock shadow", ROCK_SHADOW_SHADER, false);
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rock shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout)],
            immediate_size: 0,
        });
        self.rock_shadow_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("rock shadow"),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_rock_shadow"),
                    compilation_options: Default::default(),
                    buffers: &rock_buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
                    ..Default::default()
                },
                depth_stencil: Some(gpu.shadow_depth_state()),
                multisample: Default::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            },
        ));
        self.instances = Some(GrowBuffer::new(
            device,
            "rock instances",
            wgpu::BufferUsages::VERTEX,
            256 * ROCK_INSTANCE_BYTES,
        ));
        self.texture_layout = Some(texture_layout);
        self.cover_layout = Some(cover_layout);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        if self.ground_pipeline.is_none() {
            return;
        }
        self.frame += 1;
        let any = world
            .query_filtered::<(), Or<(
                With<GroundSurface>,
                With<Rock>,
                With<crate::cave::CaveChamber>,
            )>>()
            .iter(world)
            .next()
            .is_some();
        if !any {
            self.covers.clear();
            self.items.clear();
            self.rock_draws.clear();
            self.shadow_draws.clear();
            self.stats = FeatureStats::default();
            return;
        }
        self.ensure_textures(gpu);
        self.ensure_rock_meshes(gpu);
        self.prepare_covers(world, gpu, view);
        self.prepare_rocks(world, gpu, view);
        if let Some(buf) = &mut self.instances {
            buf.write(&gpu.device, &gpu.queue, &self.instance_bytes);
        }
        let mut stats = FeatureStats::default();
        for c in self.covers.values() {
            stats.draw_calls += 1;
            stats.instances += 1;
            stats.triangles += (c.index_count / 3) as u64;
        }
        for b in &self.rock_draws {
            stats.draw_calls += 1;
            stats.instances += b.count;
            stats.triangles +=
                (self.rock_meshes[b.mesh as usize].index_count / 3) as u64 * b.count as u64;
        }
        stats.draw_calls += self
            .shadow_draws
            .iter()
            .map(|d| d.len() as u32)
            .sum::<u32>();
        self.stats = stats;
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        self.draw_opaque(
            pass,
            self.ground_prepass_pipeline.as_ref(),
            self.rock_prepass_pipeline.as_ref(),
        );
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque {
            return;
        }
        self.draw_opaque(
            pass,
            self.ground_pipeline.as_ref(),
            self.rock_pipeline.as_ref(),
        );
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        let (Some(pipeline), Some(instances)) = (&self.rock_shadow_pipeline, &self.instances)
        else {
            return;
        };
        let Some(draws) = self.shadow_draws.get(cascade) else {
            return;
        };
        if draws.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        for b in draws {
            let m = &self.rock_meshes[b.mesh as usize];
            pass.set_vertex_buffer(0, m.vertex.slice(..));
            pass.set_index_buffer(m.index.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.index_count, 0, b.first..b.first + b.count);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "covers": self.covers.len(),
            "rocks_drawn": self.rock_draws.iter().map(|b| b.count).sum::<u32>(),
            "layers": if self.textures.is_some() { LAYER_COUNT } else { 0 },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_validate() {
        engine_render::validate_wgsl(
            "foliage scenery",
            &engine_render::compose_shader(SCENERY_SHADER, true),
        )
        .unwrap();
        engine_render::validate_wgsl(
            "foliage rock shadow",
            &engine_render::compose_shader(ROCK_SHADOW_SHADER, false),
        )
        .unwrap();
    }

    #[test]
    fn noise_repeats_every_tile() {
        for (u, v) in [(0.13, 0.71), (0.5, 0.02), (0.99, 0.37), (0.25, 0.5)] {
            let d = |a: f32, b: f32| (a - b).abs();
            assert!(d(tile_noise(u, v, 8, 3), tile_noise(u + 1.0, v - 2.0, 8, 3)) < 1e-3);
            assert!(d(tile_fbm(u, v, 4, 4, 9), tile_fbm(u - 1.0, v + 1.0, 4, 4, 9)) < 1e-3);
            assert!(d(tile_cells(u, v, 6, 1).0, tile_cells(u + 1.0, v, 6, 1).0) < 1e-3);
        }
        // Feature points scatter: cells are not a regular grid.
        let offsets: Vec<f32> = (0..16)
            .map(|i| {
                let h = lattice(i, 3, 16, 5);
                (h & 0xffff) as f32 / 65536.0
            })
            .collect();
        let spread = offsets
            .iter()
            .fold(0.0f32, |m, &o| m.max((o - offsets[0]).abs()));
        assert!(spread > 0.3, "{offsets:?}");
    }

    #[test]
    fn layer_textures_wrap_without_seams() {
        let n = 128;
        for layer in GroundLayer::ALL {
            let t = paint_layer(layer, n);
            assert_eq!(t.albedo_height.len(), n * n * 4);
            assert_eq!(t.normal_rough_ao.len(), n * n * 4);
            let h = |x: usize, y: usize| t.albedo_height[(y * n + x) * 4 + 3] as f32;
            let col = |a: usize, b: usize| (0..n).map(|y| (h(a, y) - h(b, y)).abs()).sum::<f32>();
            let row = |a: usize, b: usize| (0..n).map(|x| (h(x, a) - h(x, b)).abs()).sum::<f32>();
            let inner_x = (0..n - 1).map(|i| col(i, i + 1)).sum::<f32>() / (n - 1) as f32;
            let inner_y = (0..n - 1).map(|i| row(i, i + 1)).sum::<f32>() / (n - 1) as f32;
            assert!(
                col(n - 1, 0) < inner_x * 2.5 + n as f32,
                "{layer:?}: seam along x"
            );
            assert!(
                row(n - 1, 0) < inner_y * 2.5 + n as f32,
                "{layer:?}: seam along y"
            );
            // Not a flat color.
            let lum: Vec<f32> = t.albedo_height.chunks(4).map(|c| c[1] as f32).collect();
            let mean = lum.iter().sum::<f32>() / lum.len() as f32;
            let var = lum.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lum.len() as f32;
            assert!(var.sqrt() > 2.0, "{layer:?} is flat");
        }
    }

    #[test]
    fn rocks_fill_the_unit_box_and_face_out() {
        for shape in 0..ROCK_SHAPES {
            for lod in 0..ROCK_LODS {
                let m = rock_mesh(shape, lod);
                let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for v in &m.vertices {
                    lo = lo.min(Vec3::from(v.position));
                    hi = hi.max(Vec3::from(v.position));
                }
                let box_lo = Vec3::new(-0.5, 0.0, -0.5);
                let box_hi = Vec3::new(0.5, 1.0, 0.5);
                assert!(
                    (lo - box_lo).abs().max_element() < 1e-3,
                    "{shape}/{lod}: {lo}"
                );
                assert!(
                    (hi - box_hi).abs().max_element() < 1e-3,
                    "{shape}/{lod}: {hi}"
                );
                let center = Vec3::new(0.0, 0.45, 0.0);
                let tris = m.indices.len() / 3;
                let outward = m
                    .indices
                    .chunks(3)
                    .filter(|t| {
                        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                        (b - a).cross(c - a).dot((a + b + c) / 3.0 - center) > 0.0
                    })
                    .count();
                assert_eq!(outward, tris, "{shape}/{lod}: faces turned in");
                // Vertex normals agree with the faces they belong to.
                let agree = m
                    .indices
                    .chunks(3)
                    .filter(|t| {
                        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                        let nf = (b - a).cross(c - a).normalize_or_zero();
                        t.iter()
                            .all(|&i| Vec3::from(m.vertices[i as usize].normal).dot(nf) > 0.0)
                    })
                    .count();
                assert_eq!(agree, tris, "{shape}/{lod}: normals disagree");
            }
        }
        assert_eq!(rock_mesh(0, 0).indices.len() / 3, 1280);
        assert_eq!(rock_mesh(0, 2).indices.len() / 3, 80);
    }

    fn test_cover() -> (GroundCover, CoverInputs) {
        let cover = GroundCover {
            half_extents: [30.0, 30.0],
            round: true,
            cell_m: 0.5,
            scorched: vec![[-15.0, -15.0, 6.0, 2.0]],
            trail: vec![[-30.0, 10.0], [30.0, 10.0]],
            trail_half_width: 1.5,
            pools: vec![Pool {
                center: [10.0, -10.0],
                radius: 3.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut trees = Vec::new();
        for i in 0..6 {
            for j in 0..3 {
                let conifer = i % 2 == 0;
                trees.push(TreeSpot {
                    pos: Vec2::new(
                        -20.0 + i as f32 * 7.0,
                        -2.0 + j as f32 * 6.0 - 20.0 * (j / 2) as f32,
                    ),
                    crown: 3.0,
                    trunk: 0.25,
                    conifer,
                    spruce: conifer && j == 1,
                    shed: 1.0,
                    shrub: false,
                });
            }
        }
        let rocks = vec![RockSpot {
            pos: Vec2::new(0.0, -20.0),
            radius: 1.0,
            moss: 0.8,
        }];
        let mut inputs = CoverInputs::new(DVec3::ZERO, Vec::new(), trees, rocks);
        inputs.prepare(&cover);
        (cover, inputs)
    }

    #[test]
    fn cover_samples_follow_trail_pool_and_burn() {
        let (cover, inputs) = test_cover();
        let slots = style_slots(cover.style);
        let slot = |l: GroundLayer| slots.iter().position(|s| *s == l).unwrap();
        let mut rng = Rng::new(4);
        for _ in 0..500 {
            let p = rng.in_unit_disk() * 29.0;
            let s = sample_cover(&cover, &inputs, p);
            let sum: f32 = s.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-3, "{p}: {s:?}");
            assert!(s.weights.iter().all(|w| (0.0..=1.0).contains(w)));
            assert!((-0.06..0.15).contains(&s.height), "{p}: {}", s.height);
            assert!((0.0..=1.0).contains(&s.wet));
        }
        // The trail: packed earth, sunk below the floor beside it.
        let on = sample_cover(&cover, &inputs, Vec2::new(5.0, 10.0));
        let beside = sample_cover(&cover, &inputs, Vec2::new(5.0, 14.0));
        let most = (0..8).max_by(|&a, &b| on.weights[a].total_cmp(&on.weights[b]));
        assert_eq!(most, Some(slot(GroundLayer::Earth)), "{on:?}");
        assert!(on.height < beside.height, "{on:?} {beside:?}");
        // The pool: its floor under the water, its shore above, mud and wet between.
        let pool = &cover.pools[0];
        let inside = sample_cover(&cover, &inputs, Vec2::new(10.0, -10.0));
        let shore = sample_cover(&cover, &inputs, Vec2::new(10.0, -10.0 + pool.radius + 1.0));
        assert!(inside.height < pool.water_m, "{inside:?}");
        assert!(shore.height > pool.water_m - 0.01, "{shore:?}");
        assert!(shore.wet > 0.5, "{shore:?}");
        assert!(shore.weights[slot(GroundLayer::Mud)] > 0.2, "{shore:?}");
        // The burn: ash.
        let burnt = sample_cover(&cover, &inputs, Vec2::new(-15.0, -15.0));
        assert!(burnt.weights[slot(GroundLayer::Ash)] > 0.5, "{burnt:?}");
    }

    #[test]
    fn debris_scatters_by_species_and_stays_off_trail_and_water() {
        let (cover, inputs) = test_cover();
        let spots = scatter_debris(&cover, &inputs);
        assert!(!spots.is_empty());
        let total_cap: usize = DEBRIS_CAP.iter().map(|(_, c)| c).sum();
        assert!(spots.len() <= total_cap);
        for (kind, cap) in DEBRIS_CAP {
            assert!(
                spots.iter().filter(|s| s.kind == kind).count() <= cap,
                "{kind:?}"
            );
        }
        let pool = &cover.pools[0];
        for s in &spots {
            let t = (s.pos - Vec2::from(pool.center)).length() - pool.radius;
            if s.kind != DebrisKind::Reeds && s.kind != DebrisKind::Pebble {
                assert!(t > 0.0, "{s:?} in the water");
            }
            if matches!(s.kind, DebrisKind::Branch | DebrisKind::Leaves) {
                let d = polyline_distance(s.pos, &cover.trail);
                assert!(d > cover.trail_half_width * 0.8, "{s:?} on the trail");
            }
        }
        let near = |s: &DebrisSpot, conifer: bool| {
            inputs
                .trees
                .iter()
                .filter(|t| t.pos.distance(s.pos) < 4.0)
                .any(|t| t.conifer == conifer)
        };
        let cones = spots
            .iter()
            .filter(|s| matches!(s.kind, DebrisKind::PineCone | DebrisKind::SpruceCone));
        assert!(cones.clone().count() > 0);
        assert!(
            cones.clone().all(|s| near(s, true)),
            "cones away from conifers"
        );
        let leaves = spots.iter().filter(|s| s.kind == DebrisKind::Leaves);
        assert!(leaves.clone().count() > 0);
        assert!(
            leaves.clone().all(|s| near(s, false)),
            "leaves away from broadleaves"
        );
        assert!(spots.iter().any(|s| s.kind == DebrisKind::Reeds));
    }

    #[test]
    fn surface_follows_the_cover() {
        let (cover, inputs) = test_cover();
        let surface = build_surface(&cover, &inputs, None, 1);
        assert!(surface.triangle_count() > 10_000);
        assert_eq!(surface.vertices.len(), 121 * 121);
        assert!(
            surface
                .indices
                .iter()
                .all(|&i| (i as usize) < surface.vertices.len())
        );
        for v in surface.vertices.iter() {
            let sum: u32 = v.weights.iter().map(|&w| w as u32).sum();
            assert!((240..=270).contains(&sum), "{v:?}");
            assert!(v.normal[1] > 0.7);
        }
        let mut bytes = Vec::new();
        surface.vertices[0].write(&mut bytes);
        assert_eq!(bytes.len(), GROUND_VERTEX_BYTES);
    }

    #[test]
    fn a_cover_built_off_the_frame_matches_one_built_in_it() {
        use crate::plant::{EcologyRegion, RegionHistory};
        let build = |loading: FoliageLoading| {
            let mut app = App::new();
            app.add_plugins(engine_world::WorldPlugin);
            app.init_resource::<Assets<Mesh>>()
                .init_resource::<Assets<StandardMaterial>>()
                .init_resource::<Assets<Image>>()
                .add_plugins(crate::FoliagePlugin);
            app.world.insert_resource(loading);
            let all = PlantLibrary::default();
            let keep = ["pine", "fern"].map(|n| all.get(all.id(n).unwrap()).clone());
            app.world.insert_resource(PlantLibrary {
                species: keep.to_vec(),
            });
            app.world
                .resource_mut::<crate::plant::EcologySettings>()
                .paused = true;
            app.world.spawn((
                Transform::default(),
                EcologyRegion {
                    half_extents: [25.0, 25.0],
                    species: vec!["pine".into(), "fern".into()],
                    history: RegionHistory {
                        initial_plants: [20, 0, 40],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ));
            let (cover, _) = test_cover();
            let ground = app.world.spawn((Transform::default(), cover)).id();
            let rock = app
                .world
                .spawn((
                    Transform::from_xyz(3.0, 5.0, -4.0).with_scale(Vec3::splat(1.2)),
                    Rock::default(),
                ))
                .id();
            let start = std::time::Instant::now();
            app.update();
            while (app.world.get::<GroundSurface>(ground).is_none()
                || app.world.resource::<FoliageLoading>().scenery_building > 0)
                && start.elapsed().as_secs() < 120
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
                app.update();
            }
            let surface = app
                .world
                .get::<GroundSurface>(ground)
                .expect("built")
                .clone();
            let settled = *app.world.get::<Transform>(rock).unwrap();
            assert!(app.world.get::<Rock>(rock).unwrap().settled);
            let debris = app
                .world
                .query_filtered::<(), With<LitterPiece>>()
                .iter(&app.world)
                .count();
            (
                surface.vertices.len(),
                surface.indices.clone(),
                settled.translation,
                debris,
            )
        };
        let in_frame = build(FoliageLoading::in_frame());
        let background = build(FoliageLoading::default());
        assert!(in_frame.3 > 0, "no debris");
        assert_eq!(in_frame, background);
    }

    #[test]
    fn showcase_rocks_leave_the_trail_walkable() {
        use crate::demo::{TRAIL, TRAIL_HALF_WIDTH, cave_rocks, forest_rocks};
        let rocks = forest_rocks();
        assert!(rocks.len() > 150, "{}", rocks.len());
        for r in &rocks {
            assert!(r.size.min_element() > 0.0);
            let d = polyline_distance(r.pos, &TRAIL);
            if d < TRAIL_HALF_WIDTH + r.radius() * 0.5 {
                // Only small stones, mostly sunk into the path.
                assert!(r.size.x < 0.45, "{r:?} blocks the trail");
                assert!(
                    r.size.y - r.rock.bury_m < 0.08,
                    "{r:?} sticks out of the trail"
                );
            }
        }
        assert!(!cave_rocks().is_empty());
    }
}
