//! Procedural bark and cut-wood textures.
//!
//! One texture set per kind of bark ([`BarkKind`]): an sRGB albedo, a tangent-space normal map
//! (+Y up, OpenGL convention) and an occlusion/roughness map (R = ambient occlusion,
//! G = roughness, glTF order). Each set is painted once per process ([`bark_textures`]) and
//! shared by every plant whose bark looks like it, so a forest of ten thousand trees still
//! draws in a few instanced batches.
//!
//! ## Atlas layout ([`layout`])
//!
//! Every image is twice as wide as it is tall. The **left half** holds the bark: a tile that
//! repeats seamlessly in both directions, framed by gutters (1/64 of the width on each side)
//! that continue the pattern, so mip-mapped sampling never bleeds across the halves. The
//! **right half** holds a **cut face** (end grain: pith, growth rings, heartwood and
//! sapwood, drying cracks, the bark rim) as a disc, for stumps, log ends and broken limbs.
//! Tube meshes ([`super::MeshBuilder::tube`]) map their sides into the bark tile, with seams at
//! the tile's edges, and their caps onto the disc, so one material draws both.
//!
//! ## Colors
//!
//! The albedo is the bark's real color when a plant's vertex color equals the kind's
//! [`BarkTextures::reference`] (the bark color of the built-in species it was painted for;
//! for the pine, its darker lower trunk).
//! Materials multiply by `1 / reference`, so vertex colors act as relative tints: moss at the
//! foot, the dark birch base, the orange upper pine trunk and a designer's own bark color all
//! still show. The cut face is painted relative to the vertex colors the tree generator gives
//! cut caps ([`CUT_VERTEX_WOOD`]).
//!
//! Relief is modeled as a height field in meters at the tile's real size
//! ([`layout::TILE_WIDTH_M`]), and normals, cavities and ambient occlusion all come from it, so
//! the light and dark of the albedo agree with the shading.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};

use engine_core::math::{Vec2, Vec3, hash_u64, smoothstep};
use engine_render::Image;

use crate::species::{BarkStyle, PlantForm, PlantSpecies};

/// Where the bark tile and the cut face sit in every bark texture (u, v in 0..1), and the
/// real-world size of one tile. Shared by the meshes ([`super::MeshBuilder::tube`]) and the
/// painter.
pub mod layout {
    /// Left edge of the bark tile (u). Left of it is a gutter that continues the pattern.
    pub const BARK_U0: f32 = 1.0 / 64.0;
    /// Right edge of the bark tile (u). The gutter right of it ends at u = 0.5.
    pub const BARK_U1: f32 = 31.0 / 64.0;
    /// Center of the cut-face disc (u, v).
    pub const CUT_CENTER: [f32; 2] = [0.75, 0.5];
    /// Radius of the cut-face disc in u and in v (a circle in texels: images are 2:1).
    pub const CUT_RADIUS: [f32; 2] = [0.23, 0.46];
    /// Nominal width of one bark tile around a trunk, meters. A tube repeats the tile a whole
    /// number of times around, so the real width varies a little.
    pub const TILE_WIDTH_M: f32 = 0.5;
    /// Height / width of the bark tile (in texels, and so on the trunk).
    pub const TILE_ASPECT: f32 = 0.5 / (BARK_U1 - BARK_U0);
}

/// The vertex color the tree generator paints on cut faces (linear RGB, before its growth-ring
/// shading): the cut-face texture is painted relative to it. Keep in sync with
/// `tree::cut_color`.
pub const CUT_VERTEX_WOOD: [f32; 3] = [0.62, 0.45, 0.26];

/// Which bark a texture set paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BarkKind {
    /// Deep, interlaced vertical furrows between grey ridges broken into blocks, with lichen
    /// (oak and other big broadleaves).
    Oak,
    /// Chalky white paper bark with dark horizontal lenticels, peeling strips and a few black
    /// fissured patches.
    Birch,
    /// Thick plates of layered, flaking scales split by deep dark fissures; reddish where
    /// scales have flaked off (Scots pine).
    Pine,
    /// Thin grey-brown bark of small rounded scales (Norway spruce and other conifers).
    Spruce,
    /// Smooth, slightly glossy grey-brown bark with pale lenticels (hazel and other shrubs).
    Hazel,
    /// Mushroom stems: pale, fibrous, silky.
    Stem,
}

impl BarkKind {
    /// Every kind.
    pub const ALL: [BarkKind; 6] = [
        BarkKind::Oak,
        BarkKind::Birch,
        BarkKind::Pine,
        BarkKind::Spruce,
        BarkKind::Hazel,
        BarkKind::Stem,
    ];

    /// The bark that suits a species, from its form and bark style. `None` for plants without
    /// woody parts worth texturing (grass, flowers, ferns).
    pub fn for_species(sp: &PlantSpecies) -> Option<BarkKind> {
        Some(match (sp.form, sp.shape.bark) {
            (PlantForm::Mushroom, _) => BarkKind::Stem,
            (PlantForm::Grass | PlantForm::Flower | PlantForm::Fern, _) => return None,
            (_, BarkStyle::Birch) => BarkKind::Birch,
            (_, BarkStyle::Pine) => BarkKind::Pine,
            (PlantForm::Conifer, _) => BarkKind::Spruce,
            (PlantForm::Shrub, _) => BarkKind::Hazel,
            _ => BarkKind::Oak,
        })
    }

    /// Image widths (albedo, normal, occlusion/roughness), pixels; heights are half. Sized to
    /// the detail each bark shows up close, within a ~20 MB budget for all kinds together.
    pub fn sizes(self) -> [u32; 3] {
        match self {
            BarkKind::Oak | BarkKind::Pine => [1024, 1024, 256],
            BarkKind::Birch | BarkKind::Spruce => [1024, 512, 256],
            BarkKind::Hazel => [512, 512, 256],
            BarkKind::Stem => [256, 256, 128],
        }
    }

    /// The bark color (linear RGB) of the built-in species the albedo was painted for. `None`
    /// for kinds painted as a neutral pattern (their reference is the albedo's mean).
    fn species_bark(self) -> Option<[f32; 3]> {
        Some(match self {
            BarkKind::Oak => srgb(0x6a6055),
            BarkKind::Birch => srgb(0xe2ddd2),
            BarkKind::Pine => srgb(0x8a5a3c),
            BarkKind::Spruce => srgb(0x5b4a3e),
            BarkKind::Hazel => srgb(0x7a6553),
            BarkKind::Stem => return None,
        })
    }

    /// The vertex color the bark tile shows its painted colors under. Usually the species'
    /// bark color; the pine is painted as the plated grey-brown bark of its lower trunk, which
    /// the tree generator colors darker than the species' orange (the crown color), so the
    /// upper trunk comes out fox-red.
    fn painted_for(self) -> Option<[f32; 3]> {
        match self {
            // tree.rs `bark_color`: Pine foot (0.12, 0.09, 0.07) × the mean ring variation.
            BarkKind::Pine => Some([0.117, 0.088, 0.068]),
            _ => self.species_bark(),
        }
    }

    /// Suggested `normal_scale` for materials using this bark.
    pub fn normal_scale(self) -> f32 {
        match self {
            BarkKind::Oak => 1.0,
            BarkKind::Pine => 1.0,
            BarkKind::Spruce => 0.9,
            BarkKind::Birch => 0.8,
            BarkKind::Hazel => 0.7,
            BarkKind::Stem => 0.6,
        }
    }

    fn seed(self) -> u64 {
        0xba2c_0000 + self as u64 * 0x1_0001
    }
}

/// One painted bark texture set.
#[derive(Debug, Clone)]
pub struct BarkTextures {
    pub kind: BarkKind,
    /// sRGB albedo.
    pub albedo: Image,
    /// Tangent-space normal map (+Y up), linear.
    pub normal: Image,
    /// R = ambient occlusion, G = absolute roughness (use with a material roughness of 1),
    /// linear.
    pub orm: Image,
    /// Linear color a vertex color is relative to: materials multiply by `1 / reference`.
    pub reference: [f32; 3],
    /// Mean linear albedo of the bark tile (for recoloring dead or burnt bark).
    pub mean_albedo: [f32; 3],
}

impl BarkTextures {
    /// GPU bytes of the three images including their mip chains.
    pub fn gpu_bytes(&self) -> usize {
        [&self.albedo, &self.normal, &self.orm]
            .iter()
            .map(|i| i.data.len() * 4 / 3)
            .sum()
    }
}

/// The shared texture set of a kind of bark, painted on first use.
pub fn bark_textures(kind: BarkKind) -> &'static BarkTextures {
    static CACHE: OnceLock<Mutex<HashMap<BarkKind, &'static BarkTextures>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(t) = cache.lock().unwrap().get(&kind) {
        return t;
    }
    let started = std::time::Instant::now();
    let painted: &'static BarkTextures = Box::leak(Box::new(paint(kind, kind.sizes())));
    tracing::debug!(
        "foliage: painted {kind:?} bark in {:.0} ms",
        started.elapsed().as_secs_f32() * 1000.0
    );
    cache.lock().unwrap().entry(kind).or_insert(painted)
}

// ---------------------------------------------------------------------------------------------
// Painting

/// What a painter returns for one texel.
#[derive(Debug, Clone, Copy)]
struct Texel {
    /// Height above the bark's mean surface, meters.
    h: f32,
    /// Linear albedo.
    albedo: Vec3,
    /// Perceptual roughness.
    rough: f32,
    /// Extra occlusion (1 = none), multiplied with the one from the relief.
    ao: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            h: 0.0,
            albedo: Vec3::splat(0.5),
            rough: 0.9,
            ao: 1.0,
        }
    }
}

/// How the relief turns into shading.
struct Relief {
    /// Radius of the neighborhood a cavity is measured against, meters.
    cavity_radius_m: f32,
    /// Depth below the neighborhood at which a texel counts as fully in a cavity, meters.
    cavity_depth_m: f32,
    /// How much a full cavity darkens the albedo (baked dirt and shadowing).
    cavity_albedo: f32,
    /// How much a full cavity occludes ambient light.
    cavity_ao: f32,
}

fn relief(kind: BarkKind) -> Relief {
    let r = |cavity_radius_m, cavity_depth_m, cavity_albedo, cavity_ao| Relief {
        cavity_radius_m,
        cavity_depth_m,
        cavity_albedo,
        cavity_ao,
    };
    match kind {
        BarkKind::Oak => r(0.014, 0.014, 0.55, 0.8),
        BarkKind::Pine => r(0.012, 0.012, 0.5, 0.8),
        BarkKind::Spruce => r(0.006, 0.0035, 0.45, 0.6),
        BarkKind::Birch => r(0.004, 0.0012, 0.3, 0.45),
        BarkKind::Hazel => r(0.003, 0.0006, 0.2, 0.35),
        BarkKind::Stem => r(0.003, 0.0006, 0.15, 0.3),
    }
}

/// Paint a texture set with images `widths` = (albedo, normal, orm) pixels wide (powers of
/// two, the largest first used for painting).
fn paint(kind: BarkKind, widths: [u32; 3]) -> BarkTextures {
    let w = widths.iter().copied().max().unwrap().max(64) as usize;
    let h = w / 2;
    let tile_w = w * 30 / 64;
    let gutter = w / 64;
    let tile = Tile {
        w: tile_w,
        h,
        wm: layout::TILE_WIDTH_M,
        hm: layout::TILE_WIDTH_M * h as f32 / tile_w as f32,
        seed: kind.seed(),
    };
    let rel = relief(kind);

    // The bark tile (periodic).
    let texels = par_grid(tile.w, tile.h, |x, y| {
        let (xm, ym) = (
            (x as f32 + 0.5) / tile.w as f32 * tile.wm,
            (y as f32 + 0.5) / tile.h as f32 * tile.hm,
        );
        paint_bark(kind, &tile, xm, ym)
    });
    let px_m = tile.wm / tile.w as f32;
    let bark = shade(&texels, tile.w, tile.h, px_m, &rel, true);
    let mean = bark.albedo.iter().fold(Vec3::ZERO, |a, c| a + *c) / bark.albedo.len() as f32;
    let mean_rough = bark.rough.iter().sum::<f32>() / bark.rough.len() as f32;
    let reference = kind.painted_for().unwrap_or(mean.to_array());

    // The cut face (a disc in a square), painted relative to the cut vertex colors.
    let side = w / 2;
    let radius_px = layout::CUT_RADIUS[0] * w as f32;
    let wood = wood_params(kind);
    let cut_texels = par_grid(side, side, |x, y| {
        let p = Vec2::new(
            (x as f32 + 0.5 - side as f32 * 0.5) / radius_px,
            (y as f32 + 0.5 - side as f32 * 0.5) / radius_px,
        );
        if p.length() > 1.0 {
            Texel {
                h: 0.0,
                albedo: mean,
                rough: mean_rough,
                ao: 1.0,
            }
        } else {
            let mut t = paint_cut(&wood, kind.seed() ^ 0xc07, p);
            // Divide out the vertex color the cap will multiply in (relative to the bark's
            // reference), so the product is the painted wood.
            let rim_bark = kind.species_bark().unwrap_or(reference);
            let vertex = cut_vertex_color(Vec3::from(rim_bark), p.length());
            t.albedo *= Vec3::from(reference) / vertex.max(Vec3::splat(1e-3));
            t
        }
    });
    let cut_px_m = CUT_REFERENCE_RADIUS_M / radius_px;
    let cut_relief = Relief {
        cavity_radius_m: 0.006,
        cavity_depth_m: 0.002,
        cavity_albedo: 0.35,
        cavity_ao: 0.5,
    };
    let cut = shade(&cut_texels, side, side, cut_px_m, &cut_relief, false);

    // Assemble the atlas at full size: bark (with wrapped gutters) left, cut face right.
    let mut albedo = vec![[0u8; 4]; w * h];
    let mut normal = vec![[0u8; 4]; w * h];
    let mut orm = vec![[0u8; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let (src, i) = if x < w / 2 {
                let tx = (x + tile.w - gutter) % tile.w;
                (&bark, y * tile.w + tx)
            } else {
                (&cut, y * side + (x - w / 2))
            };
            let o = y * w + x;
            let a = src.albedo[i];
            albedo[o] = [
                linear_to_srgb_u8(a.x),
                linear_to_srgb_u8(a.y),
                linear_to_srgb_u8(a.z),
                255,
            ];
            let n = src.normal[i];
            normal[o] = [unorm(n.x), unorm(n.y), unorm(n.z), 255];
            orm[o] = [
                (src.ao[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                (src.rough[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                0,
                255,
            ];
        }
    }
    let image = |px: Vec<[u8; 4]>, target: u32, srgb: bool, renormalize: bool| {
        let (mut px, mut cw) = (px, w);
        while cw > target as usize {
            px = halve(&px, cw, cw / 2, srgb, renormalize);
            cw /= 2;
        }
        Image::new(cw as u32, cw as u32 / 2, px.concat(), srgb)
    };
    BarkTextures {
        kind,
        albedo: image(albedo, widths[0], true, false),
        normal: image(normal, widths[1], false, true),
        orm: image(orm, widths[2], false, false),
        reference,
        mean_albedo: mean.to_array(),
    }
}

/// Real radius the cut-face relief is modeled at, meters (a mature trunk).
const CUT_REFERENCE_RADIUS_M: f32 = 0.35;

/// The vertex color a cut cap carries at relative radius `r` (0 center .. 1 rim): wood inside,
/// blending to darkened bark over the outer rings, as the tree generator paints it.
fn cut_vertex_color(bark: Vec3, r: f32) -> Vec3 {
    let wood = Vec3::from(CUT_VERTEX_WOOD) * 0.93;
    wood.lerp(bark * 0.6, smoothstep(0.86, 1.0, r))
}

/// Texels turned into shading data: albedo with baked cavities, unit normals, AO, roughness.
struct Shaded {
    albedo: Vec<Vec3>,
    normal: Vec<Vec3>,
    ao: Vec<f32>,
    rough: Vec<f32>,
}

fn shade(texels: &[Texel], w: usize, h: usize, px_m: f32, rel: &Relief, wrap: bool) -> Shaded {
    let height: Vec<f32> = texels.iter().map(|t| t.h).collect();
    let radius = ((rel.cavity_radius_m / px_m).round() as usize).max(1);
    // Two box blurs approximate a smooth (tent) neighborhood average.
    let around = blur(&blur(&height, w, h, radius, wrap), w, h, radius, wrap);
    let near = blur(&height, w, h, (radius / 4).max(1), wrap);
    let at = |x: isize, y: isize| -> f32 {
        let (x, y) = if wrap {
            (x.rem_euclid(w as isize), y.rem_euclid(h as isize))
        } else {
            (x.clamp(0, w as isize - 1), y.clamp(0, h as isize - 1))
        };
        height[y as usize * w + x as usize]
    };
    let mut out = Shaded {
        albedo: Vec::with_capacity(w * h),
        normal: Vec::with_capacity(w * h),
        ao: Vec::with_capacity(w * h),
        rough: Vec::with_capacity(w * h),
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let t = texels[i];
            let (xi, yi) = (x as isize, y as isize);
            // Slopes in meters per meter; image y points down the trunk.
            let dx = (at(xi + 1, yi) - at(xi - 1, yi)) / (2.0 * px_m);
            let dy = (at(xi, yi + 1) - at(xi, yi - 1)) / (2.0 * px_m);
            out.normal.push(Vec3::new(-dx, dy, 1.0).normalize());
            let cavity = ((around[i] - near[i]) / rel.cavity_depth_m).clamp(0.0, 1.0);
            let cavity = cavity.powf(0.8);
            out.albedo
                .push(t.albedo * (1.0 - rel.cavity_albedo * cavity));
            out.ao.push((1.0 - rel.cavity_ao * cavity) * t.ao);
            out.rough.push(t.rough);
        }
    }
    out
}

/// Separable box blur of `radius` texels (wrapping or clamped at the edges).
fn blur(src: &[f32], w: usize, h: usize, radius: usize, wrap: bool) -> Vec<f32> {
    let pass = |src: &[f32], len: usize, count: usize, stride: usize, step: usize| {
        let mut out = vec![0.0f32; src.len()];
        let r = radius as isize;
        let n = (2 * radius + 1) as f32;
        for line in 0..count {
            let base = line * stride;
            let get = |k: isize| {
                let k = if wrap {
                    k.rem_euclid(len as isize)
                } else {
                    k.clamp(0, len as isize - 1)
                };
                src[base + k as usize * step]
            };
            let mut sum: f32 = (-r..=r).map(get).sum();
            for k in 0..len as isize {
                out[base + k as usize * step] = sum / n;
                sum += get(k + r + 1) - get(k - r);
            }
        }
        out
    };
    let horizontal = pass(src, w, h, w, 1);
    pass(&horizontal, h, w, 1, w)
}

/// Halve an RGBA8 image (2×2 box filter); `srgb` averages in linear light, `renormalize`
/// treats RGB as a unit normal.
fn halve(src: &[[u8; 4]], w: usize, nw: usize, srgb: bool, renormalize: bool) -> Vec<[u8; 4]> {
    let h = w / 2;
    let nh = nw / 2;
    let mut out = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = src[(y * 2 + dy).min(h - 1) * w + (x * 2 + dx).min(w - 1)];
                for c in 0..4 {
                    let v = p[c] as f32 / 255.0;
                    acc[c] += 0.25 * if srgb && c < 3 { srgb_to_linear(v) } else { v };
                }
            }
            let px = if renormalize {
                let n = Vec3::new(acc[0] * 2.0 - 1.0, acc[1] * 2.0 - 1.0, acc[2] * 2.0 - 1.0)
                    .normalize_or(Vec3::Z);
                [unorm(n.x), unorm(n.y), unorm(n.z), 255]
            } else if srgb {
                [
                    linear_to_srgb_u8(acc[0]),
                    linear_to_srgb_u8(acc[1]),
                    linear_to_srgb_u8(acc[2]),
                    (acc[3] * 255.0 + 0.5) as u8,
                ]
            } else {
                acc.map(|v| (v * 255.0 + 0.5) as u8)
            };
            out.push(px);
        }
    }
    out
}

/// Evaluate `f(x, y)` over a `w × h` grid on every core.
fn par_grid<T: Send + Copy + Default>(
    w: usize,
    h: usize,
    f: impl Fn(usize, usize) -> T + Sync,
) -> Vec<T> {
    let mut out = vec![T::default(); w * h];
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 16);
    let rows = h.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in out.chunks_mut(rows * w).enumerate() {
            let f = &f;
            scope.spawn(move || {
                let first = chunk_index * rows * w;
                for (k, t) in chunk.iter_mut().enumerate() {
                    let i = first + k;
                    *t = f(i % w, i / w);
                }
            });
        }
    });
    out
}

// ---------------------------------------------------------------------------------------------
// Noise that tiles

/// The bark tile: `w × h` texels covering `wm × hm` meters.
struct Tile {
    w: usize,
    h: usize,
    wm: f32,
    hm: f32,
    seed: u64,
}

static GRADIENTS: LazyLock<[Vec2; 64]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let a = (i as f32 + 0.37) / 64.0 * std::f32::consts::TAU;
        Vec2::new(a.cos(), a.sin())
    })
});

fn hash2(i: i32, j: i32, seed: u64) -> u64 {
    hash_u64(((i as u32 as u64) << 32 | j as u32 as u64) ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// A float in [0, 1) from a hash and a salt.
fn rnd(h: u64, salt: u64) -> f32 {
    (hash_u64(h ^ salt.wrapping_mul(0xd6e8_feb8_6659_fd93)) >> 40) as f32 / (1u64 << 24) as f32
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Gradient noise on a lattice that wraps every `px × py` cells (x, y in lattice units),
/// about [-1, 1]. Smooth, deterministic, and seamless across the wrap.
pub fn periodic_noise(x: f32, y: f32, px: i32, py: i32, seed: u64) -> f32 {
    gnoise(x, y, px, py, seed)
}

/// Gradient noise on a lattice that wraps every `px × py` cells, about [-1, 1].
fn gnoise(x: f32, y: f32, px: i32, py: i32, seed: u64) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (i, j) = (fx as i32, fy as i32);
    let (tx, ty) = (x - fx, y - fy);
    let g = |di: i32, dj: i32| {
        let h = hash2((i + di).rem_euclid(px), (j + dj).rem_euclid(py), seed);
        GRADIENTS[(h & 63) as usize]
    };
    let dot = |g: Vec2, ox: f32, oy: f32| g.x * (tx - ox) + g.y * (ty - oy);
    let n00 = dot(g(0, 0), 0.0, 0.0);
    let n10 = dot(g(1, 0), 1.0, 0.0);
    let n01 = dot(g(0, 1), 0.0, 1.0);
    let n11 = dot(g(1, 1), 1.0, 1.0);
    let (u, v) = (fade(tx), fade(ty));
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 1.41
}

/// Fractal noise over the unit tile (u, v in 0..1 wrap), `fx × fy` lattice cells in the first
/// octave, doubling per octave. About [-1, 1].
fn fbm(u: f32, v: f32, fx: i32, fy: i32, octaves: u32, gain: f32, seed: u64) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for o in 0..octaves {
        let (px, py) = (fx << o, fy << o);
        sum += amp
            * gnoise(
                u * px as f32,
                v * py as f32,
                px,
                py,
                seed.wrapping_add(o as u64 * 0x2545),
            );
        norm += amp;
        amp *= gain;
    }
    sum / norm
}

/// The nearest feature point of a periodic Voronoi diagram.
#[derive(Debug, Clone, Copy)]
struct Cell {
    /// Distance to the cell's border.
    edge: f32,
    /// Stable id of the cell.
    id: u64,
    /// Position relative to the feature point.
    rel: Vec2,
    /// Unit direction across the nearest border (toward the neighbor).
    across: Vec2,
}

/// Periodic Voronoi cells: `cells` across a tile of `size` (same units as `p`), feature points
/// jittered by `jitter` (0..1) of a cell. Distances are true Euclidean (borders via the
/// bisector of the two nearest points).
fn voronoi(p: Vec2, size: Vec2, cells: (i32, i32), jitter: f32, seed: u64) -> Cell {
    let cs = size / Vec2::new(cells.0 as f32, cells.1 as f32);
    let g = (p / cs).floor();
    let (gx, gy) = (g.x as i32, g.y as i32);
    let feature = |cx: i32, cy: i32| -> (Vec2, u64) {
        let h = hash2(cx.rem_euclid(cells.0), cy.rem_euclid(cells.1), seed);
        let o = Vec2::new(rnd(h, 1) - 0.5, rnd(h, 2) - 0.5) * jitter + Vec2::splat(0.5);
        ((Vec2::new(cx as f32, cy as f32) + o) * cs, h)
    };
    let (mut best, mut best_d, mut id, mut bc) = (Vec2::ZERO, f32::MAX, 0, (gx, gy));
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (f, h) = feature(gx + dx, gy + dy);
            let d = p.distance_squared(f);
            if d < best_d {
                (best, best_d, id, bc) = (f, d, h, (gx + dx, gy + dy));
            }
        }
    }
    let (mut edge, mut across) = (f32::MAX, Vec2::X);
    for dy in -2..=2 {
        for dx in -2..=2 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (f, _) = feature(bc.0 + dx, bc.1 + dy);
            let d = f - best;
            let len = d.length();
            if len > 1e-6 {
                let e = ((best + f) * 0.5 - p).dot(d / len);
                if e < edge {
                    (edge, across) = (e, d / len);
                }
            }
        }
    }
    Cell {
        edge: edge.max(0.0),
        id,
        rel: p - best,
        across,
    }
}

/// A short horizontal mark (lenticel) scattered on a grid: returns the signed distance to the
/// nearest mark (negative inside, meters), its position along the mark (-1..1) and its id.
/// `cells` grid across the tile, `chance(center)` of a mark per cell, half lengths and half
/// thicknesses in meters.
#[allow(clippy::too_many_arguments)]
fn dashes(
    x: f32,
    y: f32,
    tile: &Tile,
    cells: (i32, i32),
    chance: &dyn Fn(Vec2) -> f32,
    half_len: (f32, f32),
    half_thick: (f32, f32),
    seed: u64,
) -> (f32, f32, u64) {
    let cs = Vec2::new(tile.wm / cells.0 as f32, tile.hm / cells.1 as f32);
    let (gx, gy) = ((x / cs.x).floor() as i32, (y / cs.y).floor() as i32);
    let reach = (half_len.1 / cs.x).ceil() as i32 + 1;
    let mut best = (f32::MAX, 0.0, 0u64);
    for dy in -1..=1 {
        for dx in -reach..=reach {
            let (cx, cy) = (gx + dx, gy + dy);
            let h = hash2(cx.rem_euclid(cells.0), cy.rem_euclid(cells.1), seed);
            let c = Vec2::new(
                (cx as f32 + rnd(h, 1)) * cs.x,
                (cy as f32 + rnd(h, 2)) * cs.y,
            );
            if rnd(h, 3) > chance(c) {
                continue;
            }
            // Mostly short marks, a few long ones.
            let len = half_len.0 + (half_len.1 - half_len.0) * rnd(h, 4).powi(2);
            let thick = half_thick.0 + (half_thick.1 - half_thick.0) * rnd(h, 5);
            let tilt = (rnd(h, 6) - 0.5) * 0.08;
            let d = Vec2::new(x, y) - c;
            let along = d.x + d.y * tilt;
            let across = d.y - d.x * tilt;
            let s = (along / len).clamp(-1.0, 1.0);
            // Lens-shaped: thickest in the middle, pointed ends.
            let half = thick * (1.0 - s * s).max(0.0).sqrt();
            let sd = if along.abs() > len {
                Vec2::new(along.abs() - len, across).length()
            } else {
                across.abs() - half
            };
            if sd < best.0 {
                best = (sd, along / len, h);
            }
        }
    }
    best
}

/// One flake of a "dead leaves" layer: overlapping, slightly irregular elliptical flakes
/// scattered on a grid, the topmost one covering a point wins. Layered bark scales.
#[derive(Debug, Clone, Copy)]
struct Flake {
    id: u64,
    /// Stacking order, 0 (bottom) .. 1 (top).
    order: f32,
    /// Distance in from the flake's rim, meters (approximate).
    inset: f32,
    /// Position within the flake, about -1..1 per axis (y down).
    local: Vec2,
}

/// The topmost flake at (x, y): `per_cell` flakes per grid cell, half widths in `radius`
/// (meters), height = width × `aspect`. `None` where no flake lies.
#[allow(clippy::too_many_arguments)]
fn flakes(
    x: f32,
    y: f32,
    tile: &Tile,
    cells: (i32, i32),
    per_cell: u32,
    radius: (f32, f32),
    aspect: f32,
    seed: u64,
) -> Option<Flake> {
    let cs = Vec2::new(tile.wm / cells.0 as f32, tile.hm / cells.1 as f32);
    let p = Vec2::new(x, y);
    let (gx, gy) = ((x / cs.x).floor() as i32, (y / cs.y).floor() as i32);
    let reach_x = (radius.1 * 1.2 / cs.x).ceil() as i32;
    let reach_y = (radius.1 * aspect * 1.45 / cs.y).ceil() as i32;
    let mut best: Option<Flake> = None;
    for dy in -reach_y..=reach_y {
        for dx in -reach_x..=reach_x {
            let (cx, cy) = (gx + dx, gy + dy);
            for k in 0..per_cell {
                let h = hash2(
                    cx.rem_euclid(cells.0),
                    cy.rem_euclid(cells.1),
                    seed ^ (k as u64 + 1).wrapping_mul(0x5851_f42d),
                );
                let order = rnd(h, 6);
                if best.is_some_and(|b| b.order >= order) {
                    continue;
                }
                let c = Vec2::new(
                    (cx as f32 + rnd(h, 1)) * cs.x,
                    (cy as f32 + rnd(h, 2)) * cs.y,
                );
                let rx = radius.0 + (radius.1 - radius.0) * rnd(h, 3);
                let ry = rx * aspect * (0.8 + 0.4 * rnd(h, 4));
                let (s, co) = ((rnd(h, 5) - 0.5) * 0.7).sin_cos();
                let d = p - c;
                let l = Vec2::new(d.x * co + d.y * s, -d.x * s + d.y * co);
                let q = Vec2::new(l.x / rx, l.y / ry);
                let len = q.length();
                if len >= 1.25 {
                    continue;
                }
                // A ragged rim.
                let a = q.y.atan2(q.x);
                let rim = 1.0
                    + 0.12 * (3.0 * a + rnd(h, 7) * 6.3).sin()
                    + 0.07 * (7.0 * a + rnd(h, 8) * 6.3).sin();
                if len < rim {
                    best = Some(Flake {
                        id: h,
                        order,
                        inset: (rim - len) * rx.min(ry),
                        local: q,
                    });
                }
            }
        }
    }
    best
}

/// `f` and the distance (meters, to first order) from (x, y) to the zero contour of `f`.
fn contour(f: &dyn Fn(f32, f32) -> f32, x: f32, y: f32, eps: f32) -> (f32, f32) {
    let v = f(x, y);
    let gx = (f(x + eps, y) - v) / eps;
    let gy = (f(x, y + eps) - v) / eps;
    (v, v.abs() / (gx * gx + gy * gy).sqrt().max(1e-3))
}

// ---------------------------------------------------------------------------------------------
// Barks

fn paint_bark(kind: BarkKind, t: &Tile, x: f32, y: f32) -> Texel {
    match kind {
        BarkKind::Oak => oak(t, x, y),
        BarkKind::Birch => birch(t, x, y),
        BarkKind::Pine => pine(t, x, y),
        BarkKind::Spruce => spruce(t, x, y),
        BarkKind::Hazel => hazel(t, x, y),
        BarkKind::Stem => stem(t, x, y),
    }
}

/// Fine speckle shared by every bark: grit, pores, dirt, about 2–6 mm.
fn speckle(u: f32, v: f32, seed: u64) -> f32 {
    fbm(u, v, 96, 102, 2, 0.6, seed)
}

/// Oak: thick ridges that meander, split and merge between deep V-shaped furrows, broken into
/// blocks by cross cracks; scaly grey tops with crusty lichen and green algae, brown walls,
/// nearly black furrow floors.
fn oak(t: &Tile, x: f32, y: f32) -> Texel {
    let s = t.seed;
    let uv = |x: f32, y: f32| (x / t.wm, y / t.hm);
    let (u, v) = uv(x, y);
    let warp = |x: f32, y: f32| {
        let (u, v) = uv(x, y);
        0.02 * fbm(u, v, 2, 2, 3, 0.5, s + 1) + 0.005 * fbm(u, v, 8, 6, 2, 0.5, s + 2)
    };
    // Furrows: the zero contours of noise stretched along the trunk.
    let ridges = |x: f32, y: f32| {
        let (u, v) = uv(x + warp(x, y), y);
        fbm(u, v, 9, 1, 3, 0.42, s + 3)
    };
    let (_, rd) = contour(&ridges, x, y, 0.0015);
    let wn = 0.5 + 0.5 * fbm(u, v, 3, 3, 2, 0.5, s + 4);
    let fw = 0.007 + 0.009 * wn * wn;
    let f = (rd / fw).clamp(0.0, 1.0);
    let furrow = 1.0 - f;
    let depth = 0.022 + 0.012 * wn;
    let top = smoothstep(fw * 0.9, fw * 2.6, rd);
    // V-shaped furrows with rounded shoulders onto slightly domed ridge tops.
    let mut h = -depth * furrow.powf(1.15) + 0.005 * top.sqrt();
    // Cross cracks break the ridges into blocks.
    let cross = |x: f32, y: f32| {
        let (u, v) = uv(x + 0.5 * warp(x, y), y);
        fbm(u, v, 3, 9, 2, 0.45, s + 6)
    };
    let (_, cd) = contour(&cross, x, y, 0.0015);
    let crack_on = smoothstep(-0.1, 0.25, fbm(u, v, 6, 4, 2, 0.5, s + 7));
    let crack = (1.0 - smoothstep(0.001, 0.0032, cd)) * crack_on * smoothstep(0.0, 0.6, top);
    h -= 0.009 * crack;
    // Scaly, fibrous ridge surfaces.
    let flake = flakes(x, y, t, (24, 18), 1, (0.009, 0.017), 1.5, s + 8);
    let (order, edge) = match flake {
        Some(fl) => (fl.order, 1.0 - smoothstep(0.0, 0.0012, fl.inset)),
        None => (0.0, 0.0),
    };
    h += top * (0.0022 * order - 0.0012 * edge);
    h += 0.0006 * gnoise(u * 140.0, v * 12.0, 140, 12, s + 9);
    let grit = speckle(u, v, s + 10);
    h += 0.0004 * grit;

    let low = fbm(u, v, 2, 2, 3, 0.5, s + 11);
    let mid = fbm(u, v, 9, 7, 3, 0.5, s + 12);
    let mut c = srgb3(0x5d554d).lerp(
        srgb3(0x8a8279),
        (0.5 + 0.35 * low + 0.3 * mid).clamp(0.0, 1.0),
    );
    c = c.lerp(srgb3(0xa39b91), top * order * 0.45);
    // Brown walls, nearly black floors.
    c = c.lerp(srgb3(0x4d3826), smoothstep(0.0, 0.6, furrow) * 0.9);
    c = c.lerp(srgb3(0x1c130c), smoothstep(0.45, 0.95, furrow) * 0.9);
    c = c.lerp(srgb3(0x0e0906), furrow.powf(3.0));
    c *= 1.0 - 0.35 * edge * top - 0.6 * crack;
    // Crusty pale lichen and a little green algae on the ridges.
    let lichen_area = smoothstep(0.3, 0.6, fbm(u, v, 6, 5, 3, 0.6, s + 13));
    let crust = smoothstep(-0.1, 0.3, fbm(u, v, 50, 54, 2, 0.5, s + 14));
    let lichen = lichen_area * crust * top;
    let lichen_color = srgb3(0xa8aa9a).lerp(srgb3(0xb3aa7c), smoothstep(0.2, 0.6, mid));
    c = c.lerp(lichen_color, lichen * 0.65);
    h += 0.0005 * lichen;
    let algae = smoothstep(0.0, 0.6, fbm(u, v, 1, 2, 3, 0.5, s + 15)) * (1.0 - lichen);
    c = c.lerp(c * Vec3::new(0.8, 0.95, 0.6), algae * 0.35 * top);
    c *= 0.9 + 0.2 * grit.clamp(-0.5, 0.5);
    Texel {
        h,
        albedo: c,
        rough: 0.8 + 0.12 * furrow + 0.05 * lichen,
        ao: 1.0,
    }
}

/// Birch: chalky white paper with grey bands and smudges, dark lens-shaped lenticels of every
/// size clustered in bands, thin strips peeling back to the peach inner bark, and a few rough
/// black patches.
fn birch(t: &Tile, x: f32, y: f32) -> Texel {
    let s = t.seed;
    let (u, v) = (x / t.wm, y / t.hm);
    let low = fbm(u, v, 2, 3, 4, 0.5, s + 1);
    let mut c = srgb3(0xd2cec5).lerp(srgb3(0xe4ddd0), 0.5 + 0.5 * low);
    let band = smoothstep(0.15, 0.7, fbm(u, v, 1, 7, 3, 0.5, s + 2))
        * smoothstep(-0.3, 0.4, fbm(u, v, 3, 2, 2, 0.5, s + 3));
    c = c.lerp(srgb3(0xaba69c), band * 0.45);
    let smudge = smoothstep(0.2, 0.7, fbm(u, v, 4, 6, 4, 0.55, s + 4));
    c = c.lerp(srgb3(0x9c978d), smudge * 0.3);
    let warm = smoothstep(0.25, 0.7, fbm(u, v, 4, 3, 3, 0.5, s + 5));
    c = c.lerp(srgb3(0xe0cbb8), warm * 0.35);
    // Paper striations around the trunk.
    let stria = gnoise(u * 5.0, v * 150.0, 5, 150, s + 6);
    let mut h = 0.00012 * stria;
    c *= 1.0 - 0.04 * stria.abs();
    let mut rough = 0.6 + 0.1 * smudge;

    // Lenticels: bold ones clustered in bands, and many fine ones.
    let density = |p: Vec2| {
        let n = fbm(p.x / t.wm, p.y / t.hm, 2, 8, 2, 0.5, s + 7);
        (0.25 + 0.65 * (0.5 + 0.5 * n)).clamp(0.0, 1.0)
    };
    let ragged = 0.00035 * fbm(u, v, 60, 140, 2, 0.5, s + 8);
    let (bold, _, bid) = dashes(
        x,
        y,
        t,
        (10, 34),
        &density,
        (0.007, 0.042),
        (0.0011, 0.0028),
        s + 9,
    );
    let (fine, _, _) = dashes(
        x,
        y,
        t,
        (22, 70),
        &|_| 0.45,
        (0.002, 0.008),
        (0.0003, 0.0007),
        s + 10,
    );
    let core = 1.0 - smoothstep(-0.0002, 0.0002, bold + ragged);
    let halo = (1.0 - smoothstep(0.0, 0.0025, bold)) * (1.0 - core);
    let dark = srgb3(0x26211e).lerp(srgb3(0x4d453e), rnd(bid, 9));
    c = c.lerp(dark, core * 0.95);
    c *= 1.0 - 0.12 * halo;
    let faint = 1.0 - smoothstep(-0.0001, 0.0002, fine + ragged * 0.5);
    c = c.lerp(srgb3(0x7a736a), faint * 0.55);
    h += -0.0004 * core + 0.0003 * halo - 0.0001 * faint;
    rough = rough.max(0.82 * core);

    // Thin strips of paper curling away, showing the peach inner bark.
    let peel = voronoi(
        Vec2::new(x, y * 4.0),
        Vec2::new(t.wm, t.hm * 4.0),
        (3, 10),
        0.8,
        s + 11,
    );
    if rnd(peel.id, 1) < 0.3 {
        let wob = 0.0012 * fbm(u, v, 14, 24, 2, 0.5, s + 12);
        let half_w = 0.012 + 0.03 * rnd(peel.id, 2);
        let half_h = 0.0015 + 0.0025 * rnd(peel.id, 3);
        let d = peel.rel * Vec2::new(1.0, 0.25);
        let along = 1.0 - smoothstep(half_w * 0.6, half_w, d.x.abs() + 4.0 * wob);
        let inside = (1.0 - smoothstep(-0.0004, 0.0004, d.y.abs() + wob - half_h)) * along;
        let inner = srgb3(0xe6c4a8).lerp(srgb3(0xd9a787), rnd(peel.id, 4));
        c = c.lerp(inner, inside);
        h -= 0.0003 * inside;
        // The curled lip above the strip and its shadow.
        let lip = (1.0 - smoothstep(0.0, 0.0012, (d.y + half_h - wob).abs())) * along;
        c *= 1.0 - 0.4 * lip;
        h += 0.0006 * lip;
    }

    // A few rough black patches.
    let blot = voronoi(Vec2::new(x, y), Vec2::new(t.wm, t.hm), (3, 4), 0.75, s + 13);
    if rnd(blot.id, 1) < 0.3 {
        let shape = fbm(u, v, 14, 14, 3, 0.55, s + 14);
        let size = 0.016 + 0.026 * rnd(blot.id, 2);
        let d = (blot.rel * Vec2::new(0.5, 1.8)).length() + 0.01 * shape;
        let m = 1.0 - smoothstep(size * 0.7, size, d);
        if m > 0.0 {
            let cracks = fbm(u, v, 40, 70, 3, 0.55, s + 15);
            c = c.lerp(srgb3(0x221d1a) * (0.8 + 0.4 * cracks), m * 0.95);
            h += m * (0.0012 * cracks - 0.0006);
            rough = rough.max(0.9 * m);
        }
    }
    c *= 0.96 + 0.08 * speckle(u, v, s + 16).clamp(-0.5, 0.5);
    Texel {
        h,
        albedo: c,
        rough,
        ao: 1.0,
    }
}

/// Pine: tall plates split by deep dark fissures, the long (vertical) fissures wide and deep,
/// the cross breaks narrower; plates partly split again by shallow cracks, domed, each a jigsaw
/// of thin layered scales, weathered grey-brown on top and orange-red where the outer scales
/// have flaked off and down the fissure walls.
fn pine(t: &Tile, x: f32, y: f32) -> Texel {
    let s = t.seed;
    let (u, v) = (x / t.wm, y / t.hm);
    let wx = 0.018 * fbm(u, v, 2, 3, 3, 0.5, s + 1) + 0.005 * fbm(u, v, 18, 14, 2, 0.5, s + 2);
    let wy = 0.02 * fbm(u, v, 3, 2, 3, 0.5, s + 3) + 0.004 * fbm(u, v, 14, 18, 2, 0.5, s + 4);
    let stretch = 2.0;
    let q = Vec2::new(x + wx, (y + wy) / stretch);
    let size = Vec2::new(t.wm, t.hm / stretch);
    let cell = voronoi(q, size, (4, 3), 0.95, s + 5);
    let r = rnd(cell.id, 1);
    // Vertical fissures (neighbor to the side) are wide and deep, cross breaks narrow; their
    // width wanders, and here and there two plates nearly touch.
    let vertical = smoothstep(0.35, 0.8, cell.across.x.abs());
    let width_noise = (0.5 + 0.6 * fbm(u, v, 5, 3, 3, 0.55, s + 6)).clamp(0.0, 1.0);
    let fw = (0.003 + 0.022 * width_noise + 0.003 * r) * (0.3 + 0.7 * vertical);
    let fissure = 1.0 - (cell.edge / fw).clamp(0.0, 1.0);
    let depth = (0.01 + 0.025 * width_noise) * (0.4 + 0.6 * vertical);
    // The walls step down in layers (the plates are stacks of thin bark sheets).
    let layers = 5.0 + 3.0 * r;
    let step = fissure * layers;
    let terrace = (step.floor() + smoothstep(0.65, 1.0, step.fract())) / layers;
    let layer_lip = smoothstep(0.75, 1.0, step.fract()) * smoothstep(0.02, 0.15, fissure);
    // Shallow cracks splitting some plates again.
    let sub = voronoi(q + Vec2::new(0.013, 0.007), size, (9, 5), 0.9, s + 11);
    let split = if rnd(sub.id, 1) < 0.45 && sub.edge < cell.edge {
        let w = 0.0018 + 0.0022 * rnd(sub.id, 2);
        (1.0 - (sub.edge / w).clamp(0.0, 1.0)) * smoothstep(fw, 2.5 * fw + 0.004, cell.edge)
    } else {
        0.0
    };
    let plate = smoothstep(fw * 0.7, fw * 1.8, cell.edge) * (1.0 - split);
    // Domed plates: rounded shoulders down into the fissures.
    let dome = smoothstep(0.0, 0.03, cell.edge - fw).sqrt();
    let wall_profile = 0.6 * terrace + 0.4 * fissure;
    let mut h = -depth * wall_profile.powf(1.3) + 0.004 * dome * (0.6 + 0.4 * r) - 0.006 * split;
    // Layered scales on the plates.
    let flake = flakes(x, y, t, (22, 20), 2, (0.009, 0.017), 0.85, s + 7);
    let (order, inset, fid) = match flake {
        Some(fl) => (fl.order, fl.inset, fl.id),
        None => (0.0, 0.0, 0),
    };
    let edge = 1.0 - smoothstep(0.0, 0.0012, inset);
    h += plate * (0.0035 * order + 0.0006 * smoothstep(0.0, 0.006, inset) - 0.0008 * edge);
    let grain = fbm(u, v, 40, 42, 3, 0.55, s + 8);
    h += 0.0006 * grain;

    let low = fbm(u, v, 2, 2, 3, 0.5, s + 9);
    let mid = fbm(u, v, 7, 5, 2, 0.5, s + 12);
    let tone = (0.5 + 0.4 * low + 0.3 * mid).clamp(0.0, 1.0);
    let weathered = srgb3(0x6f5a4d).lerp(srgb3(0x9a8270), tone);
    let fresh = srgb3(0xa9582f).lerp(srgb3(0xc87a4b), rnd(fid, 2));
    // Low scales are freshly exposed, high ones weathered; in between a bit of both.
    let exposed = 1.0 - smoothstep(0.2, 0.6, order);
    let mut c = weathered.lerp(fresh, exposed * 0.6 * (0.5 + 0.5 * r));
    // Every plate weathers a little differently.
    c *= 0.82 + 0.3 * rnd(cell.id, 3);
    c *= 0.9 + 0.2 * grain.clamp(-0.5, 0.5);
    c *= 1.0 - 0.45 * edge * plate;
    // Layered red-brown fissure walls with pale broken lips, a dark floor.
    let wall = smoothstep(0.02, 0.4, fissure) + 0.6 * split;
    let wall_color = srgb3(0x7a4127).lerp(srgb3(0x58301e), rnd(cell.id, 4));
    c = c.lerp(wall_color, wall.min(1.0) * 0.7);
    c = c.lerp(srgb3(0xa06a48), layer_lip * 0.35);
    c = c.lerp(srgb3(0x2a1810), smoothstep(0.5, 0.95, fissure) * 0.85);
    c = c.lerp(srgb3(0x0e0805), fissure.powf(4.0));
    c *= 0.94 + 0.12 * speckle(u, v, s + 10).clamp(-0.5, 0.5);
    Texel {
        h,
        albedo: c,
        rough: 0.82 + 0.12 * fissure - 0.08 * exposed * plate,
        ao: 1.0,
    }
}

/// Spruce: thin bark of small rounded scales, their lower edges lifted like shingles, some
/// fallen away to reddish-brown underneath.
fn spruce(t: &Tile, x: f32, y: f32) -> Texel {
    let s = t.seed;
    let (u, v) = (x / t.wm, y / t.hm);
    let wx = 0.003 * fbm(u, v, 6, 6, 2, 0.5, s + 1);
    let flake = flakes(x + wx, y, t, (26, 26), 2, (0.006, 0.011), 1.0, s + 2);
    let (order, inset, local, fid) = match flake {
        Some(fl) => (fl.order, fl.inset, fl.local, fl.id),
        None => (0.0, 0.0, Vec2::ZERO, 0),
    };
    let edge = 1.0 - smoothstep(0.0, 0.0012, inset);
    // Image y points down the trunk: a scale's lower part stands away from the trunk.
    let lift = local.y.clamp(0.0, 1.0);
    let mut h = 0.0016 * order + 0.0009 * lift * smoothstep(0.0, 0.004, inset) - 0.0006 * edge;
    let grain = fbm(u, v, 36, 34, 3, 0.55, s + 3);
    h += 0.0003 * grain;

    let low = fbm(u, v, 2, 2, 3, 0.5, s + 4);
    let mut c = srgb3(0x6a594c).lerp(srgb3(0x88745f), 0.5 + 0.5 * low);
    c *= 0.85 + 0.3 * rnd(fid, 1);
    let fresh = srgb3(0x8e5639).lerp(srgb3(0xa06a48), rnd(fid, 2));
    let exposed = 1.0 - smoothstep(0.08, 0.3, order);
    c = c.lerp(fresh, exposed * 0.85);
    // Dark gaps under lifted lower edges, a lighter weathered rim on top.
    let under = edge * smoothstep(0.2, 0.8, local.y);
    c *= 1.0 - 0.55 * under - 0.2 * edge;
    c *= 0.92 + 0.16 * grain.clamp(-0.5, 0.5);
    // Rare specks of dried resin.
    let (resin_d, _, _) = dashes(
        x,
        y,
        t,
        (7, 7),
        &|_| 0.25,
        (0.001, 0.003),
        (0.0008, 0.0016),
        s + 5,
    );
    let resin = 1.0 - smoothstep(-0.0002, 0.0003, resin_d);
    c = c.lerp(srgb3(0xd8cdb4), resin * 0.8);
    h += 0.0004 * resin;
    c *= 0.95 + 0.1 * speckle(u, v, s + 6).clamp(-0.5, 0.5);
    Texel {
        h,
        albedo: c,
        rough: (0.84 + 0.1 * edge - 0.45 * resin).max(0.3),
        ao: 1.0,
    }
}

/// Hazel: smooth, slightly glossy grey-brown with a coppery cast, faint streaks and sparse
/// pale raised lenticels.
fn hazel(t: &Tile, x: f32, y: f32) -> Texel {
    let s = t.seed;
    let (u, v) = (x / t.wm, y / t.hm);
    let low = fbm(u, v, 2, 2, 4, 0.5, s + 1);
    let streak = fbm(u, v, 24, 3, 3, 0.5, s + 2);
    let mut c = srgb3(0x725f50).lerp(srgb3(0x8a705a), 0.5 + 0.5 * low);
    let copper = smoothstep(0.1, 0.6, fbm(u, v, 3, 4, 3, 0.5, s + 3));
    c = c.lerp(srgb3(0x8c5f43), copper * 0.45);
    let grey = smoothstep(0.2, 0.7, fbm(u, v, 3, 3, 3, 0.5, s + 4));
    c = c.lerp(srgb3(0x8f8a82), grey * 0.45);
    c *= 0.95 + 0.1 * streak;
    let mut h = 0.00015 * streak + 0.0001 * fbm(u, v, 40, 40, 2, 0.5, s + 5);
    let (sd, _, _) = dashes(
        x,
        y,
        t,
        (18, 30),
        &|_| 0.32,
        (0.0025, 0.0065),
        (0.0007, 0.0012),
        s + 6,
    );
    let len = 1.0 - smoothstep(-0.0003, 0.0003, sd);
    c = c.lerp(srgb3(0xab9c84), len * 0.75);
    h += 0.0003 * len;
    c *= 0.95 + 0.1 * speckle(u, v, s + 7).clamp(-0.5, 0.5);
    Texel {
        h,
        albedo: c,
        rough: 0.55 + 0.08 * grey + 0.25 * len,
        ao: 1.0,
    }
}

/// Mushroom stems: off-white silky fibrils running along the stem, a few darker flecks.
fn stem(t: &Tile, x: f32, y: f32) -> Texel {
    let s = t.seed;
    let (u, v) = (x / t.wm, y / t.hm);
    let fib = gnoise(u * 70.0, v * 5.0, 70, 5, s + 1) * 0.6
        + gnoise(u * 160.0, v * 9.0, 160, 9, s + 2) * 0.4;
    let low = fbm(u, v, 2, 2, 3, 0.5, s + 3);
    let mut c = Vec3::splat(0.8) * (0.93 + 0.08 * low + 0.07 * fib);
    let (sd, _, _) = dashes(
        x,
        y,
        t,
        (24, 20),
        &|_| 0.35,
        (0.002, 0.006),
        (0.001, 0.002),
        s + 4,
    );
    let fleck = 1.0 - smoothstep(-0.0003, 0.0004, sd);
    c *= 1.0 - 0.25 * fleck;
    Texel {
        h: 0.0002 * fib + 0.0002 * fleck,
        albedo: c,
        rough: 0.5 + 0.1 * fleck,
        ao: 1.0,
    }
}

// ---------------------------------------------------------------------------------------------
// Cut faces

/// How a kind of wood looks in cross-section (linear colors).
struct WoodParams {
    /// Growth rings from the pith to the bark.
    rings: f32,
    /// Early-wood color of the sapwood (outer, pale) and heartwood (inner).
    sap: Vec3,
    heart: Vec3,
    /// Heartwood radius (0..1); 0 = no distinct heartwood.
    heart_r: f32,
    /// How much darker the late wood of each ring is.
    late: f32,
    /// Strength of radial medullary rays (oak's silver grain).
    rays: f32,
    /// Where the bark starts (0..1).
    bark_r: f32,
    /// Thin living inner bark, then the outer bark.
    inner_bark: Vec3,
    outer_bark: Vec3,
    /// Radial drying cracks running in from the rim.
    checks: u32,
}

fn wood_params(kind: BarkKind) -> WoodParams {
    let w = |rings,
             sap: u32,
             heart: u32,
             heart_r,
             late,
             rays,
             bark_r,
             inner: u32,
             outer: u32,
             checks| WoodParams {
        rings,
        sap: srgb3(sap),
        heart: srgb3(heart),
        heart_r,
        late,
        rays,
        bark_r,
        inner_bark: srgb3(inner),
        outer_bark: srgb3(outer),
        checks,
    };
    match kind {
        BarkKind::Oak => w(
            50.0, 0xd0b48b, 0xa8794a, 0.74, 0.42, 1.0, 0.92, 0x9a5f3b, 0x575048, 4,
        ),
        BarkKind::Birch => w(
            34.0, 0xe2cfae, 0xd8c19c, 0.0, 0.24, 0.1, 0.965, 0xa8603e, 0xe6e2d8, 2,
        ),
        BarkKind::Pine => w(
            42.0, 0xe5c995, 0xc4844e, 0.58, 0.5, 0.0, 0.93, 0xa55f3a, 0x6b4a36, 3,
        ),
        BarkKind::Spruce => w(
            46.0, 0xe9d6ad, 0xe0c99c, 0.0, 0.42, 0.0, 0.955, 0xa4684a, 0x6a5646, 3,
        ),
        BarkKind::Hazel => w(
            16.0, 0xdcc8a4, 0xd2bb94, 0.0, 0.26, 0.15, 0.94, 0x9a6a48, 0x7a6552, 2,
        ),
        BarkKind::Stem => w(
            6.0, 0xe8e2d6, 0xe8e2d6, 0.0, 0.05, 0.0, 0.97, 0xe0dacd, 0xe8e2d6, 0,
        ),
    }
}

/// Paint end grain at disc coordinates `p` (|p| ≤ 1).
fn paint_cut(w: &WoodParams, seed: u64, p: Vec2) -> Texel {
    let r = p.length();
    let ang = p.y.atan2(p.x);
    // Noise on the plane of the disc (the period is far larger than the disc).
    let n2 = |q: Vec2, f: f32, oct: u32, salt: u64| {
        fbm(
            q.x * f / 64.0 + 0.5,
            q.y * f / 64.0 + 0.5,
            64,
            64,
            oct,
            0.5,
            seed + salt,
        )
    };
    // The pith sits off center; rings wobble and bunch up on one side.
    let pith = Vec2::new(0.07, -0.05);
    let q = p - pith * (1.0 - r);
    let rq = q.length();
    let wobble = 0.03 * n2(q, 3.0, 3, 1) + 0.008 * n2(q, 11.0, 2, 2);
    let rr = (rq + wobble * rq.min(0.3) / 0.3).max(0.0);
    // Wide young rings near the pith, narrow old ones near the bark, a few odd years.
    let pos = w.rings * rr.powf(1.2) + 0.7 * n2(Vec2::new(rr * 4.0, 0.3), 1.0, 2, 3);
    let f = pos.fract();
    let late = smoothstep(0.6, 0.93, f) * (1.0 - smoothstep(0.95, 1.0, f));
    let heart = if w.heart_r > 0.0 {
        1.0 - smoothstep(
            w.heart_r - 0.04,
            w.heart_r + 0.02,
            rr + 0.03 * n2(q, 5.0, 2, 4),
        )
    } else {
        0.0
    };
    let mut c = w.sap.lerp(w.heart, heart);
    c *= 1.0 - w.late * late;
    // Fine pores and torn fibres; the chopped face undulates a little.
    let pores = n2(p, 90.0, 2, 5);
    c *= 0.93 + 0.12 * pores;
    let mut h = 0.00025 * late + 0.00008 * pores + 0.0025 * n2(p, 1.6, 2, 11);
    let mut rough = 0.78 - 0.08 * late;
    // Medullary rays: thin pale radial streaks.
    if w.rays > 0.0 {
        let a = ang / std::f32::consts::TAU + 0.5;
        let ray = gnoise(a * 220.0, rr * 6.0, 220, 512, seed + 6);
        let rays = smoothstep(0.55, 0.9, ray) * w.rays * smoothstep(0.05, 0.3, rr);
        c = c.lerp(c * 1.25 + Vec3::splat(0.03), rays * 0.5);
    }
    // Tool marks: faint parallel ridges across the face.
    let marks = (p.dot(Vec2::new(0.8, 0.6)) * 70.0 + 2.0 * n2(p, 4.0, 2, 7)).sin();
    h += 0.00015 * marks;
    // The pith itself.
    let pith_d = q.length();
    let pith_m = 1.0 - smoothstep(0.008, 0.02, pith_d);
    c = c.lerp(w.heart * 0.45, pith_m);
    // Radial drying cracks from the rim, widest at the bark.
    for k in 0..w.checks {
        let hk = hash_u64(seed ^ ((k as u64 + 11) * 0x9e37));
        let a0 = rnd(hk, 1) * std::f32::consts::TAU;
        let reach = 0.35 + 0.45 * rnd(hk, 2);
        let start = 1.0 - reach;
        if r < start || r > w.bark_r + 0.01 {
            continue;
        }
        let along = (r - start) / reach;
        let bend = 0.04 * n2(Vec2::new(r * 3.0, k as f32), 1.0, 2, 8 + k as u64);
        let da = ((ang - a0 - bend + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI)
            .abs();
        let width = 0.001 + 0.009 * along.powf(1.3);
        let m = 1.0 - smoothstep(width * 0.5, width, da * r);
        if m > 0.0 {
            c = c.lerp(Vec3::new(0.03, 0.022, 0.015), m);
            h -= 0.004 * m * along;
            rough = rough.max(0.95 * m);
        }
    }
    // The bark rim: a thin living layer, then rough outer bark.
    let rim_noise = 0.006 * n2(p, 25.0, 2, 9);
    let inner = smoothstep(w.bark_r - 0.004, w.bark_r + 0.002, r + rim_noise);
    let outer = smoothstep(w.bark_r + 0.012, w.bark_r + 0.02, r + rim_noise);
    c = c.lerp(w.inner_bark, inner);
    let bark_tex = n2(p, 60.0, 3, 10);
    c = c.lerp(w.outer_bark * (0.85 + 0.3 * bark_tex), outer);
    h += outer * (0.002 + 0.0015 * bark_tex);
    rough = rough + (0.9 - rough) * inner;
    Texel {
        h,
        albedo: c,
        rough,
        ao: 1.0,
    }
}

// ---------------------------------------------------------------------------------------------
// Color helpers

fn srgb_to_linear(s: f32) -> f32 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_u8(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn unorm(v: f32) -> u8 {
    ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// sRGB hex to linear RGB.
fn srgb(hex: u32) -> [f32; 3] {
    [hex >> 16, hex >> 8, hex].map(|c| srgb_to_linear((c & 0xff) as f32 / 255.0))
}

fn srgb3(hex: u32) -> Vec3 {
    Vec3::from(srgb(hex))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(img: &Image, x: u32, y: u32) -> [i32; 3] {
        let i = ((y * img.width + x) * 4) as usize;
        [0, 1, 2].map(|c| img.data[i + c] as i32)
    }

    /// Every painter is periodic in the tile: a tile's width (or height) away it paints the
    /// same bark, so the tile wraps around trunks and along them without seams.
    #[test]
    fn bark_painters_wrap_around_and_along() {
        for kind in BarkKind::ALL {
            let tile = Tile {
                w: 480,
                h: 512,
                wm: layout::TILE_WIDTH_M,
                hm: layout::TILE_WIDTH_M * 512.0 / 480.0,
                seed: kind.seed(),
            };
            let (mut same, mut total) = (0, 0);
            for i in 0..400 {
                let x = (i as f32 * 0.6180339).fract() * tile.wm;
                let y = (i as f32 * 0.4142136).fract() * tile.hm;
                let a = paint_bark(kind, &tile, x, y);
                for (dx, dy) in [(tile.wm, 0.0), (0.0, tile.hm), (-tile.wm, -tile.hm)] {
                    let b = paint_bark(kind, &tile, x + dx, y + dy);
                    total += 1;
                    if (a.h - b.h).abs() < 2e-4 && (a.albedo - b.albedo).abs().max_element() < 0.01
                    {
                        same += 1;
                    }
                }
            }
            // Float rounding may move a sample across a cell border now and then.
            assert!(same * 100 >= total * 98, "{kind:?}: {same} of {total} wrap");
        }
    }

    /// The atlas' gutters continue the bark tile, and the cut face is a disc.
    #[test]
    fn bark_tiles_seamlessly_and_the_cut_face_is_a_disc() {
        for kind in BarkKind::ALL {
            let t = paint(kind, [256, 256, 128]);
            for img in [&t.albedo, &t.normal, &t.orm] {
                assert!(img.is_valid(), "{kind:?}");
                assert_eq!(img.width, img.height * 2);
            }
            let img = &t.albedo;
            let (w, h) = (img.width, img.height);
            let x0 = (layout::BARK_U0 * w as f32) as u32;
            let x1 = (layout::BARK_U1 * w as f32) as u32;
            let diff = |a: [i32; 3], b: [i32; 3]| (0..3).map(|c| (a[c] - b[c]).abs()).sum::<i32>();
            // The gutter left of the tile continues it from its right edge.
            let seam: i32 = (0..h)
                .map(|y| diff(rgb(img, x0 - 1, y), rgb(img, x1 - 1, y)))
                .sum();
            assert_eq!(seam, 0, "{kind:?}: gutter does not continue the tile");
            // The cut face differs from the bark mean at its center, and is flat outside.
            let (cx, cy) = (
                (layout::CUT_CENTER[0] * w as f32) as u32,
                (layout::CUT_CENTER[1] * h as f32) as u32,
            );
            let flat = rgb(&t.normal, w - 1, 0);
            assert!(
                flat[0].abs_diff(128) <= 1 && flat[1].abs_diff(128) <= 1,
                "{flat:?}"
            );
            assert!(rgb(&t.normal, cx, cy)[2] > 200);
        }
    }

    #[test]
    fn every_species_with_wood_has_a_bark_and_textures_fit_the_budget() {
        let lib = crate::species::PlantLibrary::default();
        for name in ["oak", "birch", "pine", "spruce", "hazel", "glowcap"] {
            let sp = lib.get(lib.id(name).unwrap());
            assert!(BarkKind::for_species(sp).is_some(), "{name}");
        }
        assert_eq!(
            BarkKind::for_species(lib.get(lib.id("grass").unwrap())),
            None
        );
        let bytes: usize = BarkKind::ALL
            .iter()
            .map(|k| {
                k.sizes()
                    .iter()
                    .map(|w| (*w as usize * *w as usize / 2) * 4 * 4 / 3)
                    .sum::<usize>()
            })
            .sum();
        assert!(bytes < 24 << 20, "{} MB of bark textures", bytes >> 20);
    }

    #[test]
    fn voronoi_borders_and_noise_wrap() {
        let size = Vec2::new(1.0, 2.0);
        for i in 0..50 {
            let p = Vec2::new(i as f32 * 0.137 % 1.0, i as f32 * 0.291 % 2.0);
            let a = voronoi(p, size, (5, 7), 0.9, 3);
            let b = voronoi(p + size, size, (5, 7), 0.9, 3);
            assert!((a.edge - b.edge).abs() < 1e-4 && a.id == b.id);
            assert!(a.edge >= 0.0 && a.edge < 0.5);
            let u = i as f32 * 0.0731 % 1.0;
            assert!(
                (fbm(u, 0.3, 3, 2, 4, 0.5, 1) - fbm(u + 1.0, 1.3, 3, 2, 4, 0.5, 1)).abs() < 1e-4
            );
        }
    }
}
