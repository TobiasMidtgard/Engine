//! Procedural, tileable PBR textures for the village and the materials that use them, built at
//! startup on worker threads (no asset files).
//!
//! Every set has an sRGB albedo, a tangent-space normal map (+Y up, glTF convention) and an ORM
//! map (R ambient occlusion, G roughness, B metallic). Generators return heights in meters and
//! the baker knows the size of a texel in meters, so the normal maps have physical slopes.
//!
//! All village materials are made in [`village_materials`], so a renderer-side material library
//! can replace them in one place later.

use engine::prelude::*;

// ------------------------------------------------------------------------------------ noise

#[inline]
pub(crate) fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B9)
        ^ (x as u32).wrapping_mul(0x85EB_CA6B)
        ^ (y as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^ (h >> 16)
}

/// Bits of a hash to [0, 1).
#[inline]
pub(crate) fn bits01(h: u32, shift: u32) -> f32 {
    ((h >> shift) & 0xff) as f32 / 255.0
}

const GRAD: [(f32, f32); 16] = {
    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
    // cos/sin of k * 22.5°.
    [
        (1.0, 0.0),
        (0.92388, 0.38268),
        (S, S),
        (0.38268, 0.92388),
        (0.0, 1.0),
        (-0.38268, 0.92388),
        (-S, S),
        (-0.92388, 0.38268),
        (-1.0, 0.0),
        (-0.92388, -0.38268),
        (-S, -S),
        (-0.38268, -0.92388),
        (0.0, -1.0),
        (0.38268, -0.92388),
        (S, -S),
        (0.92388, -0.38268),
    ]
};

/// Tileable gradient noise, about [-1, 1]. The lattice repeats every `px` × `py` cells.
#[inline]
pub(crate) fn noise(x: f32, y: f32, px: i32, py: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (fx, fy) = (x - x0, y - y0);
    let g = |i: i32, j: i32, dx: f32, dy: f32| {
        let (gx, gy) = GRAD[(hash(i.rem_euclid(px), j.rem_euclid(py), seed) & 15) as usize];
        gx * dx + gy * dy
    };
    let u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let a = g(ix, iy, fx, fy);
    let b = g(ix + 1, iy, fx - 1.0, fy);
    let c = g(ix, iy + 1, fx, fy - 1.0);
    let d = g(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    (ab + (cd - ab) * v) * 1.41
}

/// Tileable fractal noise over texture coordinates in [0, 1): `fx` × `fy` cells at the first
/// octave, doubling per octave. About [-1, 1].
pub(crate) fn fbm(u: f32, v: f32, fx: i32, fy: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for o in 0..octaves {
        let (px, py) = (fx << o, fy << o);
        sum += amp
            * noise(
                u * px as f32,
                v * py as f32,
                px,
                py,
                seed.wrapping_add(o * 7919),
            );
        norm += amp;
        amp *= gain;
    }
    sum / norm
}

/// Nearest feature points of a tileable cellular (Worley) pattern with `nx` × `ny` cells.
#[derive(Clone, Copy)]
pub(crate) struct Cell {
    /// Distance to the nearest point, in cell units.
    pub f1: f32,
    /// Distance to the second nearest point.
    pub f2: f32,
    /// Hash of the nearest point's cell (stable per cell).
    pub id: u32,
    /// Offset from the sample to the nearest point, in cell units.
    pub d: Vec2,
}

pub(crate) fn worley(u: f32, v: f32, nx: i32, ny: i32, jitter: f32, seed: u32) -> Cell {
    let (x, y) = (u * nx as f32, v * ny as f32);
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let mut best = Cell {
        f1: 9.0,
        f2: 9.0,
        id: 0,
        d: Vec2::ZERO,
    };
    for j in -1..=1 {
        for i in -1..=1 {
            let (cx, cy) = (ix + i, iy + j);
            let h = hash(cx.rem_euclid(nx), cy.rem_euclid(ny), seed);
            let ox = 0.5 + jitter * (bits01(h, 0) - 0.5);
            let oy = 0.5 + jitter * (bits01(h, 8) - 0.5);
            let d = Vec2::new(cx as f32 + ox - x, cy as f32 + oy - y);
            let dist = d.length();
            if dist < best.f1 {
                best.f2 = best.f1;
                best.f1 = dist;
                best.id = h;
                best.d = d;
            } else if dist < best.f2 {
                best.f2 = dist;
            }
        }
    }
    best
}

#[inline]
pub(crate) fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    engine::core::math::smoothstep(e0, e1, x)
}

/// Linear → sRGB byte.
#[inline]
pub(crate) fn srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

#[inline]
fn lin8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ------------------------------------------------------------------------------------ baking

/// One texel of a material, as a generator returns it.
#[derive(Clone, Copy)]
pub(crate) struct Px {
    /// Linear albedo.
    pub albedo: Vec3,
    /// Height in meters (only differences matter).
    pub height: f32,
    pub rough: f32,
    pub ao: f32,
    pub metal: f32,
}

impl Px {
    pub(crate) fn new(albedo: Vec3, height: f32, rough: f32, ao: f32) -> Self {
        Self {
            albedo,
            height,
            rough,
            ao,
            metal: 0.0,
        }
    }
}

/// A material's textures (and an emission map for glass).
pub struct TexSet {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
    pub emissive: Option<Image>,
}

/// Evaluate `f` over a `w` × `h` texture covering `span` meters and pack the maps (with mips).
/// `cavity` darkens the albedo by that share of the occlusion, so crevices read in sunlight
/// too.
pub(crate) fn bake(
    w: u32,
    h: u32,
    span: Vec2,
    cavity: f32,
    f: impl Fn(f32, f32) -> Px + Sync,
) -> TexSet {
    let (nw, nh) = (w as usize, h as usize);
    // Bands of rows on their own threads, so one big texture doesn't hold up the whole set.
    let rows = nh.div_ceil(4);
    let mut px = vec![Px::new(Vec3::ZERO, 0.0, 0.0, 0.0); nw * nh];
    std::thread::scope(|s| {
        for (band, chunk) in px.chunks_mut(rows * nw).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, p) in chunk.iter_mut().enumerate() {
                    let (x, y) = (i % nw, band * rows + i / nw);
                    *p = f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                }
            });
        }
    });
    let (tx, ty) = (span.x / w as f32, span.y / h as f32);
    let mut albedo = Vec::with_capacity(nw * nh * 4);
    let mut normal = Vec::with_capacity(nw * nh * 4);
    let mut orm = Vec::with_capacity(nw * nh * 4);
    let at = |x: usize, y: usize| px[y * nw + x].height;
    for y in 0..nh {
        let (yu, yd) = ((y + nh - 1) % nh, (y + 1) % nh);
        for x in 0..nw {
            let p = px[y * nw + x];
            let (xl, xr) = ((x + nw - 1) % nw, (x + 1) % nw);
            let a = p.albedo * (1.0 - cavity * (1.0 - p.ao));
            albedo.extend_from_slice(&[srgb8(a.x), srgb8(a.y), srgb8(a.z), 255]);
            let dx = (at(xr, y) - at(xl, y)) / (2.0 * tx);
            let dy = (at(x, yd) - at(x, yu)) / (2.0 * ty);
            let n = Vec3::new(-dx, dy, 1.0).normalize();
            normal.extend_from_slice(&[
                lin8(n.x * 0.5 + 0.5),
                lin8(n.y * 0.5 + 0.5),
                lin8(n.z * 0.5 + 0.5),
                255,
            ]);
            orm.extend_from_slice(&[lin8(p.ao), lin8(p.rough), lin8(p.metal), 255]);
        }
    }
    let mut set = TexSet {
        albedo: Image::new(w, h, albedo, true),
        normal: Image::new(w, h, normal, false),
        orm: Image::new(w, h, orm, false),
        emissive: None,
    };
    // Mips here, on the worker thread, instead of on the render thread at upload.
    set.albedo.generate_mips();
    set.normal.generate_mips();
    set.orm.generate_mips();
    set
}

// ------------------------------------------------------------------------------------ materials

/// Lime plaster: hand-troweled undulations, sand grain, a few hairline cracks, patched areas
/// and unevenly worn limewash. Nearly white; the material's base color tints it. Covers
/// [`PLASTER_SPAN`].
fn plaster(size: u32) -> TexSet {
    let s = 100;
    bake(size, size, Vec2::splat(PLASTER_SPAN), 0.5, |u, v| {
        let wu = u + 0.012 * noise(u * 4.0, v * 4.0, 4, 4, s);
        let wv = v + 0.012 * noise(u * 4.0 + 17.0, v * 4.0, 4, 4, s + 1);
        let broad = fbm(wu, wv, 3, 3, 3, 0.5, s + 2);
        let mid = fbm(wu, wv, 14, 14, 3, 0.5, s + 3);
        let sand = noise(u * 360.0, v * 360.0, 360, 360, s + 4);
        let grit = noise(u * 150.0, v * 150.0, 150, 150, s + 5);
        // Trowel strokes: short sweeps, stretched along a slant.
        let sweep = noise(u * 10.0 + v * 6.0, v * 30.0 - u * 18.0, 10, 30, s + 6);
        let mut h = broad * 0.003 + mid * 0.001 + sweep * 0.0006 + grit * 0.00015 + sand * 0.00008;
        // Old repairs: slightly raised, smoother patches.
        let patch = smooth(0.32, 0.36, fbm(u, v, 2, 2, 3, 0.55, s + 7));
        h += patch * 0.0006;
        let cu = u + 0.025 * noise(u * 6.0, v * 6.0, 6, 6, s + 9);
        let cv = v + 0.025 * noise(u * 6.0 + 31.0, v * 6.0, 6, 6, s + 10);
        let c = worley(cu, cv, 4, 4, 1.0, s + 11);
        let crack_mask = smooth(0.3, 0.55, fbm(u, v, 3, 3, 3, 0.5, s + 12));
        let crack = smooth(0.006, 0.001, c.f2 - c.f1) * crack_mask;
        h -= crack * 0.0006;
        // Limewash worn unevenly: blotches, faint yellowed and gray stains.
        let blotch = fbm(u, v, 5, 5, 3, 0.55, s + 13);
        let stain = smooth(0.0, 0.6, fbm(u, v, 2, 2, 3, 0.5, s + 14));
        let gray = smooth(0.1, 0.7, fbm(u, v, 3, 3, 3, 0.5, s + 15));
        let base = Vec3::new(0.80, 0.78, 0.745)
            .lerp(Vec3::new(0.76, 0.76, 0.74), patch)
            .lerp(Vec3::new(0.78, 0.72, 0.6), stain * 0.35)
            .lerp(Vec3::new(0.62, 0.62, 0.6), gray * 0.25);
        let albedo = base
            * (1.0 + 0.09 * blotch + 0.05 * mid + 0.04 * grit + 0.03 * sweep)
            * (1.0 - 0.2 * crack);
        let ao = 1.0 - 0.4 * crack;
        Px::new(albedo, h, 0.9 - 0.08 * patch + 0.04 * mid, ao)
    })
}

/// Old oak timber, hand-hewn: wandering growth rings along U, fibers, adze scallops, drying
/// checks and knots. A light neutral brown; the material tints it. Covers [`TIMBER_SPAN`].
fn timber(size: u32) -> TexSet {
    let s = 200;
    bake(size, size, Vec2::splat(TIMBER_SPAN), 0.6, |u, v| {
        let warp = fbm(u, v, 2, 2, 3, 0.5, s) * 0.05 + noise(u * 6.0, v * 3.0, 6, 3, s + 1) * 0.008;
        let w = v + warp;
        let ring_t = (w * 40.0 + 0.5 * fbm(u, v, 3, 10, 2, 0.5, s + 2)).rem_euclid(1.0);
        let late = smooth(0.5, 0.8, ring_t) * smooth(1.0, 0.88, ring_t);
        let fiber = noise(u * 6.0, w * 520.0, 6, 520, s + 3) * 0.6
            + noise(u * 20.0, w * 260.0, 20, 260, s + 4) * 0.4;
        let adze_t = (u * 12.0 + 0.35 * fbm(u, v, 2, 3, 2, 0.5, s + 5)).rem_euclid(1.0);
        let adze = 1.0 - (2.0 * adze_t - 1.0).powi(2);
        let ck = noise(u * 3.0, w * 22.0, 3, 22, s + 6);
        let ck_mask = smooth(0.3, 0.5, noise(u * 4.0, v * 5.0, 4, 5, s + 7));
        let check = smooth(0.025, 0.0, ck.abs()) * ck_mask;
        let k = worley(u, v, 3, 5, 0.8, s + 8);
        let kd = Vec2::new(k.d.x / 3.0 * 0.45, k.d.y / 5.0 * TIMBER_SPAN).length();
        let knot = if k.id & 3 == 0 {
            smooth(0.028, 0.01, kd)
        } else {
            0.0
        };
        let weather = smooth(-0.2, 0.5, fbm(u, v, 3, 3, 3, 0.5, s + 9));
        let h = adze * 0.0016 + fiber * 0.0003 - late * 0.00025 - check * 0.004 + knot * 0.0004;
        let wood = Vec3::new(0.6, 0.5, 0.39).lerp(Vec3::new(0.58, 0.55, 0.5), weather * 0.6);
        let albedo =
            wood * (0.93 - 0.14 * late + 0.1 * fiber) * (1.0 - 0.55 * check) * (1.0 - 0.6 * knot);
        let ao = 1.0 - 0.7 * check - 0.25 * knot;
        Px::new(
            albedo,
            h,
            0.72 + 0.08 * fiber.abs() + 0.2 * check + 0.06 * weather,
            ao,
        )
    })
}

/// Rough-dressed stone: lumpy surface, pointed-chisel strokes, crystals, iron stains and
/// lichen. Neutral gray; vertex colors tint each stone. Covers [`STONE_SPAN`].
fn stone(size: u32) -> TexSet {
    let s = 300;
    bake(size, size, Vec2::splat(STONE_SPAN), 0.6, |u, v| {
        let wu = u + 0.04 * noise(u * 3.0, v * 3.0, 3, 3, s);
        let wv = v + 0.04 * noise(u * 3.0 + 11.0, v * 3.0, 3, 3, s + 1);
        let big = fbm(wu, wv, 3, 3, 6, 0.55, s + 2);
        let chisel_t =
            (u * 34.0 + v * 20.0 + 1.5 * noise(u * 5.0, v * 5.0, 5, 5, s + 3)).rem_euclid(1.0);
        let chisel = (1.0 - (2.0 * chisel_t - 1.0).powi(2))
            * smooth(-0.1, 0.35, fbm(u, v, 2, 2, 3, 0.5, s + 4));
        let crystal = worley(u, v, 80, 80, 1.0, s + 5);
        let grain = bits01(crystal.id, 4);
        let pit_cell = worley(u, v, 30, 30, 0.9, s + 6);
        let pit = if pit_cell.id & 0xff < 70 {
            smooth(0.3, 0.08, pit_cell.f1)
        } else {
            0.0
        };
        let h = big * 0.005 + chisel * 0.0006 + (grain - 0.5) * 0.0002 - pit * 0.0012;
        let stain = smooth(0.1, 0.6, fbm(u, v, 2, 2, 4, 0.5, s + 7));
        let mut albedo = Vec3::new(0.5, 0.49, 0.46).lerp(Vec3::new(0.55, 0.44, 0.33), stain * 0.6)
            * (0.86 + 0.16 * big + 0.16 * (grain - 0.5));
        // Lichen: pale crusty rosettes and a few dark spots.
        let lc = worley(u, v, 9, 9, 0.9, s + 8);
        let edge_noise = 0.08 * noise(u * 90.0, v * 90.0, 90, 90, s + 9);
        let lr = 0.12 + 0.18 * bits01(lc.id, 8);
        let lichen = if lc.id & 7 < 3 {
            smooth(lr, lr - 0.06, lc.f1 + edge_noise)
        } else {
            0.0
        };
        let lichen_col = if lc.id & 16 == 0 {
            Vec3::new(0.56, 0.58, 0.45)
        } else {
            Vec3::new(0.62, 0.55, 0.3)
        };
        albedo = albedo.lerp(lichen_col, lichen * 0.85);
        let dark = if lc.id & 7 == 5 {
            smooth(0.09, 0.05, lc.f1 + edge_noise)
        } else {
            0.0
        };
        albedo *= 1.0 - 0.6 * dark;
        let ao = (0.72 + 0.28 * smooth(-0.6, 0.35, big)) * (1.0 - 0.4 * pit);
        Px::new(
            albedo,
            h + lichen * 0.0003,
            0.82 + 0.1 * lichen + 0.05 * grain,
            ao,
        )
    })
}

/// Hand-made bricks in running bond with recessed lime mortar. Full color.
fn brick(size: u32) -> TexSet {
    let s = 400;
    const COLS: f32 = 4.0;
    const ROWS: f32 = 14.0;
    bake(size, size, Vec2::splat(BRICK_SPAN), 0.6, |u, v| {
        let row = (v * ROWS).floor();
        let fv = v * ROWS - row;
        let shift = if row as i32 % 2 == 1 { 0.5 } else { 0.0 };
        let x = u * COLS + shift;
        let col = x.floor();
        let fu = x - col;
        let id = hash((col as i32).rem_euclid(COLS as i32), row as i32, s);
        // Distance to the brick's edge in meters, wobbly so edges look hand-made and chipped.
        let wob = 0.0025 * noise(u * 120.0, v * 120.0, 120, 120, s + 1)
            + 0.0015 * noise(u * 300.0, v * 300.0, 300, 300, s + 2);
        let du = fu.min(1.0 - fu) * BRICK_SPAN / COLS;
        let dv = fv.min(1.0 - fv) * BRICK_SPAN / ROWS;
        let edge = du.min(dv) + wob;
        let mortar_w = 0.006;
        let on_brick = smooth(mortar_w, mortar_w + 0.004, edge);
        let surface = fbm(u, v, 6, 6, 4, 0.5, s + 3);
        let h = on_brick * 0.007
            + surface * 0.0006 * on_brick
            + noise(u * 200.0, v * 200.0, 200, 200, s + 4) * 0.0002;
        let palette = [
            Vec3::new(0.42, 0.14, 0.08),
            Vec3::new(0.36, 0.12, 0.07),
            Vec3::new(0.48, 0.2, 0.11),
            Vec3::new(0.3, 0.12, 0.08),
            Vec3::new(0.5, 0.27, 0.15),
            Vec3::new(0.2, 0.09, 0.06),
        ];
        let base = palette[(id % palette.len() as u32) as usize] * (0.85 + 0.3 * bits01(id, 8));
        let brick_col =
            base * (0.9 + 0.12 * surface) * (1.0 + 0.12 * noise(u * 60.0, v * 60.0, 60, 60, s + 5));
        let mortar_col = Vec3::new(0.48, 0.46, 0.42)
            * (0.85 + 0.2 * noise(u * 150.0, v * 150.0, 150, 150, s + 6));
        let albedo = mortar_col.lerp(brick_col, on_brick);
        let ao = 0.55 + 0.45 * smooth(0.0, mortar_w + 0.006, edge);
        Px::new(albedo, h, 0.92 - 0.1 * on_brick, ao)
    })
}

/// Roof tile style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileStyle {
    /// Fired clay beaver-tail tiles with round ends, lichen and moss.
    Clay,
    /// Dark natural slates with fish-scale ends and flaky surfaces.
    Slate,
}

/// Roof tiles: [`TILES_ACROSS`] tiles across × [`TILE_ROWS`] courses, every other course
/// shifted half a tile. Each row is the visible part of one course; its lower edge is the
/// course's lip (the roof geometry steps there). Full color.
fn roof_tiles(size: u32, style: TileStyle) -> TexSet {
    let s = if style == TileStyle::Clay { 500 } else { 600 };
    let span = Vec2::new(
        TILE_WIDTH * TILES_ACROSS as f32,
        TILE_EXPOSURE * TILE_ROWS as f32,
    );
    // Half as many texels down the slope: the repeat is about half as long as it is wide.
    bake(size, size / 2, span, 0.7, move |u, v| {
        let rowf = v * TILE_ROWS as f32;
        let row = rowf.floor();
        let tv = rowf - row;
        let shift = if row as i32 % 2 == 1 { 0.5 } else { 0.0 };
        let x = u * TILES_ACROSS as f32 + shift;
        let col = x.floor();
        let tu = x - col;
        let id = hash((col as i32).rem_euclid(TILES_ACROSS as i32), row as i32, s);
        // Tile-local meters: x across from the tile's center, y up from the lip.
        let half = TILE_WIDTH * 0.5;
        let lx = (tu - 0.5) * TILE_WIDTH;
        let ly = (1.0 - tv) * TILE_EXPOSURE;
        let ragged = if style == TileStyle::Slate {
            0.004 * noise(u * 90.0, v * 60.0, 90, 60, s + 1)
                + 0.002 * noise(u * 300.0, v * 200.0, 300, 200, s + 2)
        } else {
            0.0008 * noise(u * 200.0, v * 120.0, 200, 120, s + 1)
        };
        // Signed distance inside the tile's outline (positive inside), meters.
        let gap = if style == TileStyle::Slate {
            0.003
        } else {
            0.0035
        };
        let side = half - gap - lx.abs();
        let a = half - gap;
        let end = if style == TileStyle::Clay {
            // Round end: a semicircle as wide as the tile.
            let c = Vec2::new(lx, ly - a);
            if ly < a { a - c.length() } else { side }
        } else {
            // Fish scale: a pointed arch of two arcs that continue the straight sides.
            let r = a * 1.15;
            let yc = (2.0 * a * r - a * a).sqrt();
            if ly < yc {
                let p = Vec2::new(lx, ly);
                (r - (p - Vec2::new(a - r, yc)).length())
                    .min(r - (p - Vec2::new(r - a, yc)).length())
            } else {
                side
            }
        };
        let inside = end.min(side) + ragged;
        let on_tile = smooth(-0.0005, 0.0025, inside);
        // The course underneath shows through the gaps, in shadow.
        let under_id = hash(
            ((x + 0.5).floor() as i32).rem_euclid(TILES_ACROSS as i32),
            row as i32 + 1,
            s,
        );
        let tilt = (bits01(id, 16) - 0.5) * 0.006;
        let surface = fbm(u, v, 16, 8, 4, 0.5, s + 3);
        let grain = noise(u * 400.0, v * 220.0, 400, 220, s + 4);
        let mut h = on_tile * (0.012 + tilt * lx / half + surface * 0.0005 + grain * 0.0001);
        // Rounded edges.
        h -= on_tile * 0.002 * smooth(0.006, 0.0, inside.max(0.0));
        let flake = if style == TileStyle::Slate {
            // Slate splits in layers: quantized height steps.
            let f = fbm(u, v, 24, 12, 3, 0.5, s + 5) * 3.0;
            (f - f.floor()) * 0.0004 + f.floor() * 0.0004
        } else {
            0.0
        };
        h += on_tile * flake;
        // Contact shadow below the next course's lip at the top of the row.
        let lip_shadow = smooth(0.0, 0.22, tv);
        let ao = (0.35 + 0.65 * on_tile) * (0.55 + 0.45 * lip_shadow);
        let vary = bits01(id, 0);
        let mut col = match style {
            TileStyle::Clay => {
                let palette = [
                    Vec3::new(0.45, 0.15, 0.075),
                    Vec3::new(0.4, 0.13, 0.07),
                    Vec3::new(0.5, 0.2, 0.1),
                    Vec3::new(0.34, 0.12, 0.07),
                    Vec3::new(0.48, 0.24, 0.12),
                    Vec3::new(0.27, 0.1, 0.065),
                ];
                palette[(id >> 8) as usize % palette.len()] * (0.85 + 0.3 * vary)
            }
            TileStyle::Slate => {
                let palette = [
                    Vec3::new(0.075, 0.08, 0.09),
                    Vec3::new(0.06, 0.065, 0.075),
                    Vec3::new(0.09, 0.09, 0.095),
                    Vec3::new(0.07, 0.075, 0.07),
                ];
                palette[(id >> 8) as usize % palette.len()] * (0.8 + 0.4 * vary)
            }
        };
        col *= 1.0 + 0.1 * surface + 0.05 * grain;
        // Weathering: rain streaks down the slope, darker toward each tile's lower end.
        let streak = noise(u * 64.0, v * 2.0, 64, 2, s + 6);
        col *= 1.0 - 0.12 * smooth(0.2, 0.8, streak) - 0.08 * (1.0 - ly / TILE_EXPOSURE);
        let mut rough = if style == TileStyle::Clay {
            0.72 + 0.1 * surface
        } else {
            0.5 + 0.12 * surface
        };
        // Lichen rosettes and moss in the damp joints.
        let lc = worley(u, v, 22, 12, 0.9, s + 7);
        let lr = 0.18 + 0.15 * bits01(lc.id, 8);
        let lichen = if lc.id & 7 < 3 {
            smooth(
                lr,
                lr - 0.08,
                lc.f1 + 0.08 * noise(u * 150.0, v * 80.0, 150, 80, s + 8),
            )
        } else {
            0.0
        };
        let lichen_col = if lc.id & 32 == 0 {
            Vec3::new(0.55, 0.5, 0.28)
        } else {
            Vec3::new(0.5, 0.52, 0.45)
        };
        col = col.lerp(lichen_col, lichen * 0.75 * on_tile);
        let damp = smooth(0.25, 0.7, fbm(u, v, 4, 2, 4, 0.55, s + 9));
        let moss_amount = damp * (1.0 - smooth(0.0, 0.012, inside)) * 0.9;
        let moss_col = Vec3::new(0.12, 0.16, 0.05) * (0.8 + 0.4 * grain.abs());
        let under_col = col * 0.45 * (0.8 + 0.4 * bits01(under_id, 3));
        col = under_col.lerp(col, on_tile);
        col = col.lerp(moss_col, moss_amount);
        rough = rough.max(moss_amount * 0.95 + lichen * 0.85);
        h += moss_amount * 0.002;
        Px::new(col, h, rough, ao)
    })
}

/// Window glass: slightly wavy old panes with smudges, 4 × 4 cells (one per window), and a
/// matching emission map of lit and dark rooms behind them.
fn glass(size: u32) -> TexSet {
    let s = 700;
    let mut set = bake(size, size, Vec2::splat(2.0), 0.0, |u, v| {
        let wave =
            noise(u * 8.0, v * 8.0, 8, 8, s) + 0.5 * noise(u * 24.0, v * 16.0, 24, 16, s + 1);
        let cv = (v * 4.0).fract();
        let smudge =
            smooth(0.2, 0.8, fbm(u, v, 8, 8, 3, 0.5, s + 2)) * 0.12 + smooth(0.75, 1.0, cv) * 0.08;
        Px::new(Vec3::ONE, wave * 0.0004, 0.06 + smudge, 1.0)
    });
    let n = size as usize;
    let mut em = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let (cx, cy) = ((u * 4.0).floor() as i32, (v * 4.0).floor() as i32);
            let (fu, fv) = (u * 4.0 - cx as f32, v * 4.0 - cy as f32);
            let id = hash(cx, cy, s + 3);
            let kind = id % 8;
            // Candle and oil-lamp light, some rooms warmer than others.
            let warm = Vec3::new(1.0, 0.62, 0.3).lerp(Vec3::new(1.0, 0.5, 0.2), bits01(id, 24));
            let c = match kind {
                // Dark rooms.
                0..=2 => Vec3::new(0.004, 0.0035, 0.003) * (1.0 + bits01(id, 8)),
                // A lamp in the room: bright spot, falling off.
                3 | 4 => {
                    let spot = Vec2::new(0.25 + 0.5 * bits01(id, 8), 0.3 + 0.3 * bits01(id, 16));
                    let d = (Vec2::new(fu, fv) - spot).length();
                    warm * (0.25 + 0.9 * (-d * d * 9.0).exp())
                }
                // Curtains drawn at the sides with folds.
                5 | 6 => {
                    let cur = smooth(0.2, 0.08, fu.min(1.0 - fu));
                    let folds = 0.6 + 0.4 * (fu * 60.0).sin().abs();
                    let room = 0.5 + 0.5 * (1.0 - fv);
                    warm * (room * (1.0 - cur) + cur * 0.55 * folds) * 0.9
                }
                // Dim, deep room.
                _ => warm * (0.12 + 0.12 * (1.0 - fv)),
            };
            em.extend_from_slice(&[srgb8(c.x), srgb8(c.y), srgb8(c.z), 255]);
        }
    }
    let mut emissive = Image::new(size, size, em, true);
    emissive.generate_mips();
    set.emissive = Some(emissive);
    set
}

/// Wrought iron: hammer dimples, fine pitting and patches of rust (rough, non-metallic).
fn iron(size: u32) -> TexSet {
    let s = 800;
    bake(size, size, Vec2::splat(IRON_SPAN), 0.4, |u, v| {
        let dimple = worley(u, v, 20, 20, 0.9, s);
        let pits = noise(u * 220.0, v * 220.0, 220, 220, s + 1);
        let rust_n = fbm(u, v, 3, 3, 5, 0.55, s + 2) + 0.15 * pits;
        let rust = smooth(0.15, 0.45, rust_n);
        let h = dimple.f1 * dimple.f1 * 0.0004 + pits * 0.00005 + rust * 0.0002;
        let metal_col =
            Vec3::new(0.055, 0.055, 0.06) * (0.85 + 0.3 * bits01(dimple.id, 0) + 0.1 * pits);
        let rust_col = Vec3::new(0.2, 0.085, 0.04) * (0.8 + 0.5 * (0.5 + 0.5 * pits));
        let albedo = metal_col.lerp(rust_col, rust);
        let mut p = Px::new(
            albedo,
            h,
            0.42 + 0.12 * bits01(dimple.id, 8) + 0.45 * rust,
            1.0 - 0.2 * rust,
        );
        p.metal = 1.0 - rust;
        p
    })
}

// ------------------------------------------------------------------------------------ the set

/// Meters covered by one repeat of the plaster texture.
pub const PLASTER_SPAN: f32 = 2.0;
/// Meters covered by one repeat of the timber texture (grain along U).
pub const TIMBER_SPAN: f32 = 1.0;
/// Meters covered by one repeat of the stone texture.
pub const STONE_SPAN: f32 = 0.8;
/// Meters covered by one repeat of the brick texture.
pub const BRICK_SPAN: f32 = 1.0;
/// Meters covered by one repeat of the iron texture.
pub const IRON_SPAN: f32 = 0.5;
/// Width of one roof tile, meters.
pub const TILE_WIDTH: f32 = 0.18;
/// Visible length of one course of tiles along the slope, meters.
pub const TILE_EXPOSURE: f32 = 0.17;
/// Tiles across one repeat of the roof texture.
pub const TILES_ACROSS: u32 = 8;
/// Courses in one repeat of the roof texture.
pub const TILE_ROWS: u32 = 4;
/// Window cells per side of the glass texture (each window shows one cell).
pub const GLASS_CELLS: u32 = 4;

/// Shared materials for the village. Per-instance color variety comes from several materials
/// that share one texture set (they still draw in one batch per mesh).
pub struct VillageMats {
    /// Lime plaster in a few tints (white, cream, ochre, pink, gray).
    pub plaster: Vec<Handle<StandardMaterial>>,
    /// Timber framing in a few stains (dark oak, ox-blood red, gray, black-brown).
    pub timber: Vec<Handle<StandardMaterial>>,
    /// Painted woodwork (window frames, shutters, doors, flower boxes): colors are in the
    /// vertex colors.
    pub paint: Handle<StandardMaterial>,
    /// Dressed stone (plinths, steps, sills, chimney caps, ridge mortar beds).
    pub stone: Handle<StandardMaterial>,
    /// Chimney brick.
    pub brick: Handle<StandardMaterial>,
    /// Clay roofs in a few ages.
    pub roof_clay: Vec<Handle<StandardMaterial>>,
    /// Slate roofs.
    pub roof_slate: Vec<Handle<StandardMaterial>>,
    /// Window glass with a warm glow behind it at night.
    pub glass: Handle<StandardMaterial>,
    /// Wrought iron (lamps, hinges, brackets, wheel tires).
    pub iron: Handle<StandardMaterial>,
    /// Lantern glass (emissive).
    pub lantern: Handle<StandardMaterial>,
    /// Potted plants and flowers (vertex colored).
    pub plants: Handle<StandardMaterial>,
}

/// Every village texture set (plain data: built off the frame thread).
pub struct VillageTextures([TexSet; 8]);

/// Generate every village texture in parallel.
pub fn village_textures() -> VillageTextures {
    let t0 = std::time::Instant::now();
    let sets = std::thread::scope(|sc| {
        let pl = sc.spawn(|| plaster(1024));
        let tb = sc.spawn(|| timber(1024));
        let st = sc.spawn(|| stone(512));
        let br = sc.spawn(|| brick(512));
        let clay = sc.spawn(|| roof_tiles(1024, TileStyle::Clay));
        let slate = sc.spawn(|| roof_tiles(1024, TileStyle::Slate));
        let gl = sc.spawn(|| glass(256));
        let ir = sc.spawn(|| iron(256));
        let j = |h: std::thread::ScopedJoinHandle<'_, TexSet>| h.join().expect("texture thread");
        [j(pl), j(tb), j(st), j(br), j(clay), j(slate), j(gl), j(ir)]
    });
    engine::core::log::info!(
        "village textures generated in {:.2} s",
        t0.elapsed().as_secs_f32()
    );
    VillageTextures(sets)
}

/// Add the village textures as images and make the materials.
pub fn village_materials(
    textures: VillageTextures,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> VillageMats {
    let [pl, tb, st, br, clay, slate, gl, ir] = textures.0;
    // Keyed, so the memory report names the showcase as their owner.
    let mut add = |name: &str, set: TexSet| {
        let key = |map: &str| format!("showcase/village/{name}/{map}");
        (
            images.insert_keyed(&key("albedo"), set.albedo),
            images.insert_keyed(&key("normal"), set.normal),
            images.insert_keyed(&key("orm"), set.orm),
            set.emissive
                .map(|e| images.insert_keyed(&key("emissive"), e)),
        )
    };
    let pl = add("plaster", pl);
    let tb = add("timber", tb);
    let st = add("stone", st);
    let br = add("brick", br);
    let clay = add("clay_roof", clay);
    let slate = add("slate_roof", slate);
    let gl = add("glass", gl);
    let ir = add("iron", ir);
    type Set = (
        Handle<Image>,
        Handle<Image>,
        Handle<Image>,
        Option<Handle<Image>>,
    );
    // A textured material: factors of 1 so the ORM map decides roughness and metalness.
    let textured = |set: &Set, span: Vec2, normal_scale: f32, tint: [f32; 3]| {
        let mut m = StandardMaterial::color(tint[0], tint[1], tint[2])
            .with_roughness(1.0)
            .with_base_color_texture(set.0)
            .with_normal_texture(set.1)
            .with_metallic_roughness_texture(set.2)
            .with_occlusion_texture(set.2)
            .with_uv_scale(1.0 / span.x, 1.0 / span.y);
        m.normal_scale = normal_scale;
        m.occlusion_strength = 1.0;
        m
    };
    let plaster = [
        [1.0, 0.98, 0.94],
        [1.02, 0.92, 0.76],
        [0.98, 0.8, 0.55],
        [1.0, 0.86, 0.8],
        [0.86, 0.88, 0.88],
    ]
    .iter()
    .map(|t| materials.add(textured(&pl, Vec2::splat(PLASTER_SPAN), 1.0, *t)))
    .collect();
    let timber = [
        [0.26, 0.17, 0.11],
        [0.42, 0.13, 0.08],
        [0.45, 0.42, 0.38],
        [0.15, 0.11, 0.085],
    ]
    .iter()
    .map(|t| materials.add(textured(&tb, Vec2::splat(TIMBER_SPAN), 1.0, *t)))
    .collect();
    // Roof meshes carry texture coordinates already in repeats.
    let roof_clay = [[1.0, 1.0, 1.0], [0.82, 0.76, 0.72], [0.66, 0.58, 0.55]]
        .iter()
        .map(|t| materials.add(textured(&clay, Vec2::ONE, 1.0, *t)))
        .collect();
    let roof_slate = [[1.0, 1.0, 1.0], [0.9, 0.95, 1.05]]
        .iter()
        .map(|t| materials.add(textured(&slate, Vec2::ONE, 1.0, *t)))
        .collect();
    let mut glass = StandardMaterial::color(0.03, 0.035, 0.04)
        .with_roughness(1.0)
        .with_normal_texture(gl.1)
        .with_metallic_roughness_texture(gl.2)
        .with_emissive([1.0, 1.0, 1.0], 10.0);
    glass.emissive_texture = gl.3;
    glass.reflectance = 0.6;
    glass.receives_wetness = false;
    glass.normal_scale = 1.0;
    let mut iron = textured(&ir, Vec2::splat(IRON_SPAN), 1.0, [1.0, 1.0, 1.0]);
    iron.metallic = 1.0;
    VillageMats {
        plaster,
        timber,
        paint: materials.add(textured(
            &tb,
            Vec2::splat(TIMBER_SPAN),
            0.6,
            [1.0, 1.0, 1.0],
        )),
        stone: materials.add(textured(&st, Vec2::splat(STONE_SPAN), 1.0, [1.0, 1.0, 1.0])),
        brick: materials.add(textured(&br, Vec2::splat(BRICK_SPAN), 1.0, [1.0, 1.0, 1.0])),
        roof_clay,
        roof_slate,
        glass: materials.add(glass),
        iron: materials.add(iron),
        lantern: materials.add(
            StandardMaterial::hex(0x332211)
                .with_emissive([1.0, 0.66, 0.34], 600.0)
                .with_roughness(0.2),
        ),
        plants: materials.add(
            StandardMaterial::color(1.0, 1.0, 1.0)
                .with_roughness(0.7)
                .with_subsurface(0.5),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles() {
        // The same value one period apart in both directions.
        for (x, y) in [(0.3, 0.7), (2.9, 1.1)] {
            let a = noise(x, y, 4, 6, 3);
            assert!((a - noise(x + 4.0, y, 4, 6, 3)).abs() < 1e-5);
            assert!((a - noise(x, y + 6.0, 4, 6, 3)).abs() < 1e-5);
        }
        let a = worley(0.01, 0.5, 5, 5, 1.0, 9).f1;
        let b = worley(1.01, 0.5, 5, 5, 1.0, 9).f1;
        assert!((a - b).abs() < 1e-4);
    }

    #[test]
    fn texture_sets_are_sane() {
        let set = roof_tiles(64, TileStyle::Clay);
        assert_eq!(set.albedo.data.len(), 64 * 32 * 4);
        assert!(set.albedo.srgb && !set.normal.srgb && !set.orm.srgb);
        assert!(!set.albedo.mips.is_empty());
        // Normal maps point out of the surface.
        assert!(set.normal.data.chunks(4).all(|p| p[2] > 128));
        let g = glass(32);
        assert!(g.emissive.is_some());
    }

    /// Writes every texture to `$VILLAGE_TEX_DUMP` as raw RGBA (for looking at them).
    #[test]
    #[ignore]
    fn dump_textures() {
        let Ok(dir) = std::env::var("VILLAGE_TEX_DUMP") else {
            return;
        };
        let sets = [
            ("plaster", plaster(1024)),
            ("timber", timber(1024)),
            ("stone", stone(512)),
            ("brick", brick(512)),
            ("clay", roof_tiles(1024, TileStyle::Clay)),
            ("slate", roof_tiles(1024, TileStyle::Slate)),
            ("glass", glass(256)),
            ("iron", iron(256)),
        ];
        for (name, set) in sets {
            let mut maps = vec![
                ("albedo", &set.albedo),
                ("normal", &set.normal),
                ("orm", &set.orm),
            ];
            if let Some(e) = &set.emissive {
                maps.push(("emissive", e));
            }
            for (kind, img) in maps {
                let path = format!("{dir}/{name}_{kind}_{}x{}.rgba", img.width, img.height);
                std::fs::write(path, &img.data).unwrap();
            }
        }
    }
}
