//! Procedural PBR textures for the station, generated on the CPU in parallel: base color
//! (sRGB), tangent-space normal map (+Y up, glTF) and ORM (R occlusion, G roughness,
//! B metalness, glTF). Every material texture tiles; sizes are noted per recipe.
//!
//! Each recipe evaluates one function per pixel that returns albedo, height, roughness,
//! metalness and occlusion together, and the normal map is derived from the height field in
//! meters (so bumps have physical depth at the texture's real-world size).

use engine_core::math::Vec2;
use engine_render::Image;

use super::font::Canvas;
use super::noise::{fbm, hash, hex, lerp, smooth, sq, to_srgb8, unit, value, worley};

/// One texel of a material.
#[derive(Clone, Copy, Debug)]
pub struct Px {
    /// Linear base color.
    pub albedo: [f32; 3],
    pub alpha: f32,
    /// Surface height, meters.
    pub height: f32,
    pub rough: f32,
    pub metal: f32,
    /// Ambient occlusion, 0..1.
    pub ao: f32,
}

impl Default for Px {
    fn default() -> Self {
        Self {
            albedo: [0.5; 3],
            alpha: 1.0,
            height: 0.0,
            rough: 0.5,
            metal: 0.0,
            ao: 1.0,
        }
    }
}

/// Base color, normal and ORM images of one material.
pub struct TexSet {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
}

/// Evaluate `f(x, y)` for every pixel of a `w × h` image, rows split across threads.
pub fn par_pixels<T: Send + Copy + Default>(
    w: u32,
    h: u32,
    f: impl Fn(u32, u32) -> T + Sync,
) -> Vec<T> {
    let mut out = vec![T::default(); (w * h) as usize];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunks = (threads * 4).max(1);
    let rows = (h as usize).div_ceil(chunks).max(1);
    let row_len = w as usize;
    let pieces: Vec<(usize, &mut [T])> = out.chunks_mut(rows * row_len).enumerate().collect();
    let pieces = std::sync::Mutex::new(pieces);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                // Take the lock only to pop the next piece, not while working on it.
                let next = || pieces.lock().ok().and_then(|mut p| p.pop());
                while let Some((ci, chunk)) = next() {
                    let first = ci * rows * row_len;
                    for (i, px) in chunk.iter_mut().enumerate() {
                        let idx = first + i;
                        *px = f((idx % row_len) as u32, (idx / row_len) as u32);
                    }
                }
            });
        }
    });
    out
}

/// Evaluate a material over normalized coordinates and pack its three maps. `size_m` is the
/// real-world size the texture covers (for the slope of the normal map).
pub fn bake(w: u32, h: u32, size_m: (f32, f32), f: impl Fn(f32, f32) -> Px + Sync) -> TexSet {
    let px = par_pixels(w, h, |x, y| {
        f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
    });
    pack(w, h, size_m, &px)
}

/// Pack evaluated texels into base color, normal and ORM images.
pub fn pack(w: u32, h: u32, size_m: (f32, f32), px: &[Px]) -> TexSet {
    let (sx, sy) = (size_m.0 / w as f32, size_m.1 / h as f32);
    let n = (w * h) as usize;
    let mut albedo = Vec::with_capacity(n * 4);
    let mut normal = Vec::with_capacity(n * 4);
    let mut orm = Vec::with_capacity(n * 4);
    let hgt = |x: i64, y: i64| {
        let x = x.rem_euclid(w as i64) as usize;
        let y = y.rem_euclid(h as i64) as usize;
        px[y * w as usize + x].height
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let p = px[(y * w as i64 + x) as usize];
            albedo.extend_from_slice(&[
                to_srgb8(p.albedo[0]),
                to_srgb8(p.albedo[1]),
                to_srgb8(p.albedo[2]),
                (p.alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            ]);
            let dx = (hgt(x + 1, y) - hgt(x - 1, y)) / (2.0 * sx);
            let dy = (hgt(x, y + 1) - hgt(x, y - 1)) / (2.0 * sy);
            // glTF: +X along u, +Y toward decreasing v (image up).
            let (nx, ny, nz) = (-dx, dy, 1.0);
            let l = (nx * nx + ny * ny + nz * nz).sqrt();
            let e = |c: f32| ((c / l * 0.5 + 0.5) * 255.0 + 0.5) as u8;
            normal.extend_from_slice(&[e(nx), e(ny), e(nz), 255]);
            let b = |c: f32| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            orm.extend_from_slice(&[b(p.ao), b(p.rough), b(p.metal), 255]);
        }
    }
    TexSet {
        albedo: Image::new(w, h, albedo, true),
        normal: Image::new(w, h, normal, false),
        orm: Image::new(w, h, orm, false),
    }
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn flag(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Random scratches: one segment per lattice cell (period `p`), returns 0..1 coverage.
fn scratches(u: f32, v: f32, p: i32, width: f32, seed: u32) -> f32 {
    let (x, y) = (u * p as f32, v * p as f32);
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let mut best: f32 = 0.0;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (gx, gy) = (cx + ox, cy + oy);
            let h = hash(gx.rem_euclid(p), gy.rem_euclid(p), seed);
            if unit(h) > 0.55 {
                continue;
            }
            let c = Vec2::new(
                gx as f32 + unit(h.rotate_left(7)),
                gy as f32 + unit(h.rotate_left(14)),
            );
            let a = unit(h.rotate_left(21)) * std::f32::consts::TAU;
            let len = 0.3 + 0.9 * unit(h.rotate_left(27));
            let d = Vec2::new(a.cos(), a.sin()) * len * 0.5;
            let dist = seg_dist(Vec2::new(x, y), c - d, c + d);
            let w = width * (0.5 + unit(h.rotate_left(3)));
            best = best.max(smooth(w, w * 0.3, dist) * (0.4 + 0.6 * unit(h.rotate_left(11))));
        }
    }
    best
}

/// Weathered asphalt (1024², tiles every 2 m): binder, exposed aggregate polished by traffic,
/// chippings, pores and broad stains (spots and oil come from the yard's vertex colors, so
/// they don't repeat).
pub fn asphalt() -> TexSet {
    bake(1024, 1024, (2.0, 2.0), |u, v| {
        let a = worley(u * 190.0, v * 190.0, sq(190), 0.9, 11);
        let ra = unit(a.id);
        let rb = unit(a.id.rotate_left(9));
        let rc = unit(a.id.rotate_left(19));
        let gap = 0.10 + 0.22 * rb;
        let stone = smooth(gap, gap + 0.10, a.f2 - a.f1);
        let wear = fbm(u * 3.0, v * 3.0, sq(3), 4, 12) * 0.5 + 0.5;
        let exposed = stone * smooth(0.0, 0.1, 0.2 + 0.45 * wear - ra);
        let b = worley(u * 480.0, v * 480.0, sq(480), 0.9, 15);
        let chip = smooth(0.12, 0.3, b.f2 - b.f1) * flag(unit(b.id) < 0.5) * (1.0 - exposed);
        let p = worley(u * 260.0, v * 260.0, sq(260), 1.0, 16);
        let pore = flag(unit(p.id) < 0.1) * smooth(0.38, 0.16, p.f1) * (1.0 - exposed);
        let grit = value(u * 1024.0, v * 1024.0, sq(1024), 17);
        let stain = fbm(u * 2.0, v * 2.0, sq(2), 5, 18);
        let binder = 0.034 + 0.03 * wear + 0.008 * grit;
        let g = 0.075 + 0.15 * rc * rc;
        let stone_c = if rb > 0.82 {
            [g * 1.15, g * 0.96, g * 0.82]
        } else if rb < 0.12 {
            [g * 0.9, g * 0.93, g * 1.0]
        } else {
            [g, g, g * 0.97]
        };
        let chip_g = 0.04 + 0.05 * unit(b.id.rotate_left(5));
        let mut c = [binder; 3];
        for k in 0..3 {
            c[k] = lerp(c[k], chip_g, chip * 0.8);
            c[k] = lerp(c[k], stone_c[k], exposed);
            c[k] *= (1.0 - 0.6 * pore) * (0.9 + 0.2 * (stain * 0.5 + 0.5));
        }
        let dome = (1.0 - (a.f1 / 0.62).powi(2)).max(0.0).sqrt().min(0.75);
        let height = exposed * dome * 0.0016 + chip * 0.0005 + grit * 0.00015 - pore * 0.0012;
        let rough = lerp(0.93 - 0.05 * grit, 0.6 + 0.15 * rb - 0.1 * wear, exposed);
        Px {
            albedo: c,
            height,
            rough,
            ao: (1.0 - 0.55 * pore) * (0.82 + 0.18 * (exposed + 0.5 * chip).min(1.0)),
            ..Default::default()
        }
    })
}

/// Sandstone paving (512², tiles every 1 m): mottled color, faint bedding, dark and light
/// mineral grains, a riven (gently undulating) surface with small pits.
pub fn stone() -> TexSet {
    bake(512, 512, (1.0, 1.0), |u, v| {
        let m = fbm(u * 4.0, v * 4.0, sq(4), 5, 21);
        let hue = fbm(u * 2.0 + 0.37, v * 2.0, sq(2), 3, 22) * 0.5 + 0.5;
        let band =
            (v * 9.0 * std::f32::consts::TAU + fbm(u * 3.0, v * 3.0, sq(3), 3, 23) * 4.0).sin();
        let grain = value(u * 512.0, v * 512.0, sq(512), 24);
        let grain2 = value(u * 160.0, v * 160.0, sq(160), 25);
        let speck = worley(u * 90.0, v * 90.0, sq(90), 0.9, 26);
        let dark = flag(unit(speck.id) < 0.08) * smooth(0.3, 0.1, speck.f1);
        let light = flag(unit(speck.id) > 0.93) * smooth(0.25, 0.08, speck.f1);
        let pits_c = worley(u * 40.0, v * 40.0, sq(40), 1.0, 28);
        let pit = flag(unit(pits_c.id) < 0.12) * smooth(0.25, 0.1, pits_c.f1);
        let base = mix3(hex(0xb4a993), hex(0xa7a59f), hue);
        let k = 0.86 + 0.16 * m + 0.03 * band + 0.08 * (grain - 0.5) + 0.06 * (grain2 - 0.5);
        let mut c = scale3(base, k);
        c = mix3(c, hex(0x564c44), dark * 0.7);
        c = mix3(c, hex(0xdcd6c8), light * 0.6);
        c = scale3(c, 1.0 - 0.3 * pit);
        let riven = fbm(u * 5.0, v * 5.0, sq(5), 6, 27);
        Px {
            albedo: c,
            height: riven * 0.0012 + grain2 * 0.0002 + grain * 0.0001 - pit * 0.0006,
            rough: 0.76 + 0.1 * grain - 0.05 * m,
            ao: 1.0 - 0.35 * pit,
            ..Default::default()
        }
    })
}

/// Sand (512², tiles every 1.5 m): wind ripples with gentle stoss and steep lee faces, fading
/// in patches, and dark and light grains.
pub fn sand() -> TexSet {
    bake(512, 512, (1.5, 1.5), |u, v| {
        let warp =
            fbm(u * 2.0, v * 2.0, sq(2), 4, 31) * 1.3 + fbm(u * 5.0, v * 5.0, sq(5), 3, 32) * 0.25;
        let phase = v * 14.0 + u * 2.0 + warp;
        let s = phase - phase.floor();
        let r = if s < 0.72 { s / 0.72 } else { (1.0 - s) / 0.28 };
        let r = r * r * (3.0 - 2.0 * r);
        let amp = smooth(-0.35, 0.45, fbm(u * 3.0, v * 3.0, sq(3), 3, 33));
        let grain = value(u * 512.0, v * 512.0, sq(512), 34);
        let g2 = unit(hash((u * 512.0) as i32, (v * 512.0) as i32, 35));
        let tone = fbm(u * 3.0, v * 3.0, sq(3), 4, 36);
        let base = hex(0xc9b088);
        let mut c = scale3(
            base,
            0.93 + 0.08 * tone + 0.12 * (grain - 0.5) - 0.07 * (1.0 - r) * amp,
        );
        c = mix3(c, hex(0x6a5d50), flag(g2 < 0.05) * 0.55);
        c = mix3(c, hex(0xf3ecdc), flag(g2 > 0.94) * 0.45);
        Px {
            albedo: c,
            height: r * amp * 0.005 + grain * 0.0004 + (g2 - 0.5) * 0.0002,
            rough: 0.92 + 0.06 * grain,
            ao: 0.86 + 0.14 * (r * amp + 1.0 - amp),
            ..Default::default()
        }
    })
}

/// Cast concrete (512², tiles every 1 m): mottling, pores, a little exposed aggregate.
pub fn concrete() -> TexSet {
    bake(512, 512, (1.0, 1.0), |u, v| {
        let m = fbm(u * 3.0, v * 3.0, sq(3), 5, 41);
        let fine = value(u * 256.0, v * 256.0, sq(256), 44);
        let f2 = value(u * 90.0, v * 90.0, sq(90), 45);
        let agg = worley(u * 55.0, v * 55.0, sq(55), 0.9, 42);
        let ag = flag(unit(agg.id) < 0.25) * smooth(0.3, 0.15, agg.f1);
        let pores = worley(u * 130.0, v * 130.0, sq(130), 1.0, 43);
        let pore = flag(unit(pores.id) < 0.16) * smooth(0.3, 0.1, pores.f1);
        let mut c = scale3(
            hex(0xaba9a3),
            0.86 + 0.16 * m + 0.07 * (fine - 0.5) + 0.05 * (f2 - 0.5),
        );
        c = mix3(c, scale3(hex(0x8f8a82), 0.9 + 0.3 * unit(agg.id)), ag * 0.5);
        c = scale3(c, 1.0 - 0.45 * pore);
        Px {
            albedo: c,
            height: -pore * 0.0009 + fine * 0.00015 + f2 * 0.0002 + ag * 0.0002,
            rough: 0.9 - 0.05 * m,
            ao: 1.0 - 0.5 * pore,
            ..Default::default()
        }
    })
}

/// Per-texel wear of a painted surface (shared by every paint color).
#[derive(Clone, Copy, Default)]
struct Paint {
    peel: f32,
    chip: f32,
    rim: f32,
    rust: f32,
    scratch: f32,
    dirt: f32,
    grain: f32,
}

fn paint_fields(u: f32, v: f32, wood: bool) -> Paint {
    let peel = value(u * 120.0, v * 120.0, sq(120), 51) * 0.6
        + value(u * 240.0, v * 240.0, sq(240), 52) * 0.4;
    let wear = smooth(0.05, 0.6, fbm(u * 4.0, v * 4.0, sq(4), 4, 53));
    let c = worley(u * 26.0, v * 26.0, sq(26), 0.9, 54);
    let size = 0.12 + 0.22 * unit(c.id.rotate_left(5));
    let shape = c.f1 + fbm(u * 60.0, v * 60.0, sq(60), 3, 55) * 0.14;
    let on = flag(unit(c.id) < 0.45 * wear);
    let chip = on * smooth(size, size - 0.04, shape);
    let rim = on * smooth(size + 0.07, size, shape) * (1.0 - chip);
    let rust = chip * smooth(0.0, 0.5, fbm(u * 30.0, v * 30.0, sq(30), 3, 56) + 0.1);
    let scratch = scratches(u, v, 7, 0.012, 57) * (0.3 + 0.7 * wear);
    let streak = value(u * 48.0, v * 4.0, (48, 4), 58);
    let dirt = (smooth(0.0, 0.9, fbm(u * 2.0, v * 2.0, sq(2), 4, 59) * 0.5 + 0.3) * 0.6
        + smooth(0.55, 0.95, streak) * 0.4)
        .min(1.0);
    let grain = if wood {
        let w = v * 20.0 + fbm(u * 2.0, v * 2.0, sq(2), 3, 60) * 1.5;
        let ring = w - w.floor();
        smooth(0.55, 0.75, ring) * smooth(1.0, 0.85, ring)
            + 0.5 * value(u * 6.0, v * 200.0, (6, 200), 61)
    } else {
        0.0
    };
    Paint {
        peel,
        chip,
        rim,
        rust,
        scratch,
        dirt,
        grain,
    }
}

/// Painted surfaces (512², tiles every 0.5 m): orange peel, chips down to primer and steel
/// (or grey wood for painted timber), scratches, rust in the chips, dirt and run-off streaks.
/// Returns one base color per entry of `colors` (linear), plus the shared normal and ORM maps.
pub fn painted(colors: &[[f32; 3]], wood: bool) -> (Vec<Image>, Image, Image) {
    const N: u32 = 512;
    let fields = par_pixels(N, N, |x, y| {
        paint_fields(
            (x as f32 + 0.5) / N as f32,
            (y as f32 + 0.5) / N as f32,
            wood,
        )
    });
    let px = |f: &Paint, color: [f32; 3]| {
        let dirt_c = hex(0x585046);
        let mut c = mix3(scale3(color, 1.0 - 0.18 * f.dirt), dirt_c, f.dirt * 0.22);
        c = mix3(c, scale3(color, 1.25), f.scratch * 0.35);
        let primer = if wood { hex(0xb9b2a4) } else { hex(0x7a5547) };
        c = mix3(c, primer, f.rim * 0.85);
        let bare = if wood {
            scale3(hex(0x8a8073), 0.8 + 0.3 * f.grain)
        } else {
            hex(0x8d9095)
        };
        c = mix3(c, bare, f.chip);
        c = mix3(c, hex(0x6e3b22), f.rust * 0.8);
        let metal = if wood { 0.0 } else { f.chip * (1.0 - f.rust) };
        let rough = (0.36 + 0.08 * f.peel + 0.32 * f.dirt + 0.1 * f.scratch)
            * (1.0 - f.chip - f.rim)
            + 0.72 * f.rim
            + f.chip * lerp(if wood { 0.85 } else { 0.38 }, 0.85, f.rust);
        Px {
            albedo: c,
            height: f.peel * 0.00003 - f.chip * 0.00016 - f.rim * 0.00005 - f.scratch * 0.00004
                + f.grain * 0.00012,
            rough,
            metal,
            ao: 1.0 - 0.25 * f.chip,
            alpha: 1.0,
        }
    };
    let mut albedos = Vec::new();
    let mut shared = None;
    for (i, color) in colors.iter().enumerate() {
        let texels: Vec<Px> = fields.iter().map(|f| px(f, *color)).collect();
        let set = pack(N, N, (0.5, 0.5), &texels);
        albedos.push(set.albedo);
        if i == 0 {
            shared = Some((set.normal, set.orm));
        }
    }
    let (normal, orm) = shared.unwrap_or_else(|| {
        let set = pack(1, 1, (1.0, 1.0), &[Px::default()]);
        (set.normal, set.orm)
    });
    (albedos, normal, orm)
}

/// Weathered hardwood (1024 × 256, tiles every 1 m along the grain (u) and 0.25 m across):
/// growth rings, pores, checking cracks and silver-grey weathering.
pub fn wood() -> TexSet {
    bake(1024, 256, (1.0, 0.25), |u, v| {
        // Growth rings on the face of a flat-sawn plank: fine, nearly straight lines along
        // the board (about 1 cm apart, irregular), drifting gently.
        let drift =
            fbm(u, v * 2.0, (1, 2), 3, 71) * 2.4 + fbm(u * 6.0, v * 4.0, (6, 4), 2, 72) * 0.2;
        let w = v * 26.0 + drift;
        let id = w.floor();
        let ring = w - id;
        let rh = hash((id as i32).rem_euclid(26), 0, 78);
        let width = 0.1 + 0.2 * unit(rh);
        let late = smooth(1.0 - width - 0.1, 1.0 - width, ring) * smooth(1.0, 0.95, ring);
        let tone = 0.93 + 0.14 * unit(rh.rotate_left(9));
        let fibre = value(u * 24.0, v * 420.0, (24, 420), 73);
        let pores = smooth(0.6, 0.95, value(u * 10.0, v * 300.0, (10, 300), 74));
        let ck = value(u * 3.0, v * 40.0, (3, 40), 75);
        let check = smooth(0.965, 0.99, 1.0 - (ck * 2.0 - 1.0).abs())
            * smooth(0.3, 0.7, fbm(u * 4.0, v, (4, 1), 3, 76));
        let grey = smooth(-0.2, 0.8, fbm(u * 2.0, v * 3.0, (2, 3), 4, 77)) * 0.35;
        let mut c = mix3(hex(0xa58463), hex(0x84634a), late * 0.8);
        c = scale3(c, tone * (0.95 + 0.08 * (fibre - 0.5)) - 0.06 * pores);
        c = mix3(c, hex(0x9a948b), grey);
        c = scale3(c, 1.0 - 0.55 * check);
        Px {
            albedo: c,
            // Weathering wears the soft early wood away: the late rings stand proud.
            height: late * 0.00035 + fibre * 0.00004 - pores * 0.00008 - check * 0.0007,
            rough: 0.62 + 0.15 * grey + 0.06 * pores + 0.1 * check - 0.05 * late,
            ao: 1.0 - 0.45 * check - 0.06 * pores,
            ..Default::default()
        }
    })
}

/// Hot-dip galvanised steel (512², tiles every 0.6 m): spangle crystals with different sheen,
/// dendrites, dull white-rust patches.
pub fn galvanised() -> TexSet {
    bake(512, 512, (0.6, 0.6), |u, v| {
        // Weathered hot-dip zinc: small spangle crystals under a dull, mottled patina, with
        // faint run streaks.
        let s = worley(u * 56.0, v * 56.0, sq(56), 0.9, 61);
        let r1 = unit(s.id);
        let edge = smooth(0.0, 0.04, s.f2 - s.f1);
        let dull = smooth(-0.3, 0.6, fbm(u * 4.0, v * 4.0, sq(4), 4, 63));
        let speck = value(u * 256.0, v * 256.0, sq(256), 62);
        let streak = smooth(0.7, 1.0, value(u * 90.0, v * 2.0, (90, 2), 64)) * 0.5;
        let mut c = scale3([0.6, 0.61, 0.62], 0.96 + 0.06 * r1 + 0.03 * speck);
        c = mix3(c, [0.5, 0.51, 0.5], dull * 0.45);
        c = scale3(c, 1.0 - 0.06 * streak);
        Px {
            albedo: c,
            height: (1.0 - edge) * 0.00002 + speck * 0.00001,
            rough: (0.36 + 0.08 * r1 + 0.3 * dull + 0.05 * streak + 0.04 * speck).min(0.9),
            metal: 0.95 - 0.35 * dull,
            ..Default::default()
        }
    })
}

/// Plain-woven polyester (256², tiles every 4 cm) for the windsock.
pub fn fabric() -> TexSet {
    bake(256, 256, (0.04, 0.04), |u, v| {
        let n = 24.0;
        let (tx, ty) = (u * n, v * n);
        let (ix, iy) = (tx.floor() as i32, ty.floor() as i32);
        let (fx, fy) = (tx - tx.floor(), ty - ty.floor());
        let warp = (fx * std::f32::consts::PI).sin();
        let weft = (fy * std::f32::consts::PI).sin();
        let over = (ix + iy).rem_euclid(2) == 0;
        let hgt = if over { warp } else { weft };
        let slub = value(u * 6.0, v * 48.0, (6, 48), 81) * 0.5
            + value(u * 48.0, v * 6.0, (48, 6), 82) * 0.5;
        let dirt = smooth(0.1, 0.8, fbm(u * 2.0, v * 2.0, sq(2), 3, 83) * 0.5 + 0.4) * 0.2;
        let k = (0.86 + 0.1 * hgt + 0.06 * (slub - 0.5)) * (1.0 - dirt);
        Px {
            albedo: [k, k, k * 0.98],
            height: hgt * 0.00012,
            rough: 0.82 - 0.08 * hgt,
            ao: 0.75 + 0.25 * hgt,
            ..Default::default()
        }
    })
}

/// Toughened glass (256 × 1024 over a 0.5 m × 2.2 m panel, v from the top): clear glass with
/// two bands of frosted manifestation dots and splash grime along the bottom. Base color
/// carries the opacity; returns (base color, ORM).
pub fn glass() -> (Image, Image) {
    let set = bake(256, 1024, (0.5, 2.2), |u, v| {
        let up = (1.0 - v) * 2.2;
        let (gx, gy) = (u * 0.5 / 0.025, up / 0.025);
        let d = Vec2::new(gx - gx.round(), gy - gy.round()).length() * 0.025;
        let in_band = |c: f32| smooth(0.05, 0.035, (up - c).abs());
        let band = in_band(1.0).max(in_band(1.5));
        let dot = band * smooth(0.0072, 0.0058, d);
        let grime = smooth(0.4, 0.0, up) * (0.55 + 0.45 * fbm(u * 6.0, v * 3.0, (6, 3), 3, 91))
            + smooth(0.75, 0.97, value(u * 30.0, v * 4.0, (30, 4), 92)) * 0.12;
        let grime = grime.clamp(0.0, 1.0) * 0.7;
        // Clear float glass: a dark, slightly green body so that blending shows mostly the
        // reflection; ceramic frit dots and grime are opaque-ish and lit.
        let mut c = mix3([0.05, 0.07, 0.068], [0.9, 0.9, 0.88], dot);
        c = mix3(c, hex(0x6b6253), grime);
        Px {
            albedo: c,
            alpha: (0.26 + 0.7 * dot + 0.35 * grime).min(1.0),
            rough: 0.03 + 0.5 * dot + 0.4 * grime,
            ..Default::default()
        }
    });
    (set.albedo, set.orm)
}

/// Worn thermoplastic road paint (512², tiles every 1 m): alpha cuts out where the paint is
/// worn through (use with an alpha mask), raised edges in the normal map.
pub fn road_paint() -> TexSet {
    bake(512, 512, (1.0, 1.0), |u, v| {
        let wear = fbm(u * 5.0, v * 5.0, sq(5), 5, 101) * 0.5 + 0.5;
        let grit = value(u * 300.0, v * 300.0, sq(300), 102);
        let keep = smooth(0.30, 0.38, 1.0 - wear * 0.75 + (grit - 0.5) * 0.25);
        let dirt = smooth(0.3, 0.9, fbm(u * 3.0, v * 3.0, sq(3), 3, 103) * 0.5 + 0.5) * 0.25;
        let k = (0.92 - dirt) * (0.94 + 0.08 * grit);
        Px {
            albedo: [k, k, k],
            alpha: keep,
            height: keep * 0.0006 + grit * 0.0001,
            rough: 0.55 + 0.2 * grit + 0.2 * dirt,
            ..Default::default()
        }
    })
}

/// Cast iron with a raised pattern (512² over the whole part): `pattern(u, v)` returns 1 on the
/// raised parts. Worn tops are polished, recesses rusty and dirty.
pub fn cast_iron(pattern: impl Fn(f32, f32) -> f32 + Sync) -> TexSet {
    bake(512, 512, (0.7, 0.7), |u, v| {
        let raised = pattern(u, v);
        let n = fbm(u * 8.0, v * 8.0, sq(8), 4, 111);
        let grit = value(u * 256.0, v * 256.0, sq(256), 112);
        let rust = (1.0 - raised) * smooth(-0.2, 0.5, n);
        let mut c = scale3([0.2, 0.2, 0.21], 0.85 + 0.2 * grit);
        c = mix3(c, hex(0x5a3622), rust * 0.7);
        c = mix3(c, [0.42, 0.42, 0.43], raised * 0.6);
        Px {
            albedo: c,
            height: raised * 0.004 + grit * 0.0001,
            rough: lerp(0.82, 0.5, raised) + 0.1 * grit,
            metal: lerp(0.25, 0.7, raised) * (1.0 - rust * 0.8),
            ao: 0.7 + 0.3 * raised,
            ..Default::default()
        }
    })
}

/// Monocrystalline solar module (256 × 512, whole panel): 3 × 6 cells with busbars and
/// fingers under glass, white backsheet in the gaps.
pub fn solar() -> TexSet {
    bake(256, 512, (0.5, 1.0), |u, v| {
        let (cx, cy) = (u * 3.0, v * 6.0);
        let (fx, fy) = (cx - cx.floor(), cy - cy.floor());
        let gap = smooth(0.02, 0.035, fx.min(1.0 - fx)) * smooth(0.012, 0.02, fy.min(1.0 - fy));
        let corner = Vec2::new((fx - 0.5).abs(), (fy - 0.5).abs());
        let cut = smooth(0.0, 0.02, 0.92 - (corner.x * 0.9 + corner.y * 0.45));
        let cell = gap * cut;
        let bus = flag(((fx * 3.0) - (fx * 3.0).round()).abs() < 0.02) * cell;
        let finger = flag(((fy * 40.0) - (fy * 40.0).round()).abs() < 0.07) * cell * 0.5;
        let tone = value(u * 6.0, v * 12.0, (6, 12), 121) * 0.15;
        let mut c = mix3([0.85, 0.86, 0.87], scale3(hex(0x141b2c), 0.9 + tone), cell);
        c = mix3(c, [0.6, 0.6, 0.62], bus.max(finger));
        Px {
            albedo: c,
            height: bus * 0.0002,
            rough: 0.12 + 0.1 * (1.0 - cell),
            metal: 0.0,
            ..Default::default()
        }
    })
}

/// Fallen leaves, a 2 × 2 atlas of 8 cm leaves (512², alpha-masked): an elliptic leaf, a
/// lobed maple-like leaf, an oak-like leaf and a narrow curled one, in autumn colors.
pub fn leaves() -> TexSet {
    let colors = [hex(0xc49a3c), hex(0xa8572a), hex(0x7a5232), hex(0x9a9442)];
    bake(512, 512, (0.16, 0.16), move |u, v| {
        let (cx, cy) = ((u * 2.0).floor() as usize, (v * 2.0).floor() as usize);
        let cell = cy * 2 + cx;
        // Leaf frame: x across, y from the stalk (-1) to the tip (+1).
        let x = (u * 2.0).fract() * 2.0 - 1.0;
        let y = 1.0 - (v * 2.0).fract() * 2.0;
        let t = ((y + 0.78) / 1.68).clamp(0.0, 1.0);
        let along = (y + 0.78) / 1.68;
        let half = match cell {
            0 => 0.46 * (std::f32::consts::PI * t).sin().powf(0.75),
            1 => {
                let p = Vec2::new(x, y + 0.08);
                let th = p.x.atan2(p.y);
                let r = 0.84 * (0.5 + 0.5 * (th * 2.5).cos().abs().powf(1.3));
                // Polar shape: express as a half width at this height.
                if p.length() < r && y > -0.72 {
                    1.0
                } else {
                    0.0
                }
            }
            2 => {
                0.38 * (std::f32::consts::PI * t).sin().powf(0.6)
                    * (0.72 + 0.28 * (t * std::f32::consts::PI * 9.0).sin().abs())
            }
            _ => 0.22 * (std::f32::consts::PI * t).sin().powf(0.9),
        };
        let inside_blade = if cell == 1 {
            half
        } else {
            smooth(0.0, 0.03, half - x.abs())
                * if (0.0..=1.0).contains(&along) {
                    1.0
                } else {
                    0.0
                }
        };
        // Stalk.
        let stalk = smooth(0.035, 0.015, x.abs()) * smooth(-0.97, -0.92, y) * smooth(-0.6, -0.7, y);
        let alpha = inside_blade.max(stalk);
        // Veins: midrib and side veins sweeping toward the tip.
        let mid = smooth(0.03, 0.008, x.abs());
        let side = {
            let s = (y - x.abs() * 0.9) * 5.5;
            smooth(0.09, 0.0, (s - s.round()).abs()) * smooth(0.0, 0.06, x.abs())
        };
        let veins = mid.max(side * 0.6);
        let n = fbm(u * 24.0, v * 24.0, sq(24), 3, 131);
        let blotch = smooth(0.1, 0.55, fbm(u * 10.0, v * 10.0, sq(10), 3, 132));
        let dry_edge = if cell == 1 {
            0.0
        } else {
            smooth(0.08, 0.0, half - x.abs())
        };
        let mut c = scale3(colors[cell], 0.9 + 0.2 * n);
        c = mix3(c, hex(0x5a3a22), blotch * 0.45 + dry_edge * 0.5);
        c = mix3(c, scale3(colors[cell], 1.25), veins * 0.5);
        Px {
            albedo: c,
            alpha,
            height: veins * 0.00025 + n * 0.0001,
            rough: 0.62 + 0.1 * blotch,
            ao: 0.9 + 0.1 * (1.0 - veins),
            ..Default::default()
        }
    })
}

/// Grass tufts for crossed cards: two clumps side by side (512 × 256, alpha-masked), blades
/// rooted at the bottom edge, curving and tapering, green with dry yellow tips.
pub fn grass_tufts() -> TexSet {
    struct Blade {
        a: Vec2,
        b: Vec2,
        c: Vec2,
        w: f32,
        color: [f32; 3],
    }
    let greens = [
        hex(0x4f6b2c),
        hex(0x66803a),
        hex(0x8b8a48),
        hex(0x5a7633),
        hex(0x72863c),
    ];
    let mut blades = Vec::new();
    for clump in 0..2 {
        for k in 0..56u32 {
            let h = hash(k as i32, clump, 141);
            let r = |s: u32| unit(h.rotate_left(s));
            let cx = 0.25 + 0.5 * clump as f32;
            let base = Vec2::new(cx + (r(0) - 0.5) * 0.24, 1.0);
            let height = 0.45 + 0.5 * r(5);
            let lean = (r(10) - 0.5) * 0.5;
            let tip = Vec2::new(base.x + lean * 0.5 * height, 1.0 - height);
            let ctrl = Vec2::new(base.x + lean * 0.1, 1.0 - height * 0.6);
            blades.push(Blade {
                a: base,
                b: ctrl,
                c: tip,
                w: 0.008 + 0.008 * r(15),
                color: greens[(r(20) * greens.len() as f32) as usize % greens.len()],
            });
        }
    }
    bake(512, 256, (0.4, 0.2), move |u, v| {
        let p = Vec2::new(u, v);
        let (mut best, mut tone, mut along) = (0.0f32, [0.0; 3], 0.0f32);
        let mut round = 0.0f32;
        for b in &blades {
            if p.x < b.a.x.min(b.c.x) - 0.03 || p.x > b.a.x.max(b.c.x) + 0.03 || p.y < b.c.y - 0.01
            {
                continue;
            }
            // Distance to the quadratic curve, sampled as a polyline.
            let mut prev = b.a;
            for s in 1..=6 {
                let t = s as f32 / 6.0;
                let q = b.a * (1.0 - t) * (1.0 - t) + b.b * 2.0 * t * (1.0 - t) + b.c * t * t;
                let seg = q - prev;
                let k = ((p - prev).dot(seg) / seg.length_squared().max(1e-9)).clamp(0.0, 1.0);
                let tt = (s as f32 - 1.0 + k) / 6.0;
                let width = b.w * (1.0 - tt).powf(0.7) + 0.0008;
                let d = p.distance(prev + seg * k);
                let cover = smooth(width, width * 0.6, d);
                if cover > best {
                    best = cover;
                    tone = b.color;
                    along = tt;
                    round = 1.0 - (d / width).min(1.0).powi(2);
                }
                prev = q;
            }
        }
        let dry = smooth(0.55, 1.0, along);
        let c = mix3(scale3(tone, 0.7 + 0.3 * along), hex(0xb2a660), dry * 0.5);
        Px {
            albedo: c,
            alpha: best,
            height: round * 0.0004,
            rough: 0.55,
            ao: 0.55 + 0.45 * along.sqrt(),
            ..Default::default()
        }
    })
}

/// Paint `canvas` coverage over a background, returning an sRGB RGBA8 image.
pub fn compose(
    w: u32,
    h: u32,
    layers: &[(&Canvas, [f32; 3])],
    background: impl Fn(f32, f32) -> [f32; 3] + Sync,
) -> Image {
    let px = par_pixels(w, h, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        let mut c = background(u, v);
        for (canvas, color) in layers {
            c = mix3(c, *color, canvas.at(x, y));
        }
        [to_srgb8(c[0]), to_srgb8(c[1]), to_srgb8(c[2]), 255u8]
    });
    Image::new(w, h, px.concat(), true)
}

/// Subtle enamel/print wear for signs: slight mottling and edge grime.
pub fn sign_grime(u: f32, v: f32, seed: u32) -> f32 {
    let n = fbm(u * 6.0, v * 6.0, sq(6), 4, seed) * 0.5 + 0.5;
    let edge = smooth(0.06, 0.0, u.min(1.0 - u).min(v.min(1.0 - v)));
    let streak = smooth(0.7, 1.0, value(u * 30.0, v * 2.0, (30, 2), seed + 1)) * v;
    (0.04 * n + 0.25 * edge + 0.12 * streak).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materials_have_plausible_values() {
        let set = asphalt();
        assert_eq!((set.albedo.width, set.albedo.height), (1024, 1024));
        // Weathered asphalt is dark: mean albedo ~0.05–0.15 linear (sRGB 60–110).
        let mean: f32 =
            set.albedo.data.chunks(4).map(|p| p[0] as f32).sum::<f32>() / (1024.0 * 1024.0);
        assert!((50.0..120.0).contains(&mean), "asphalt sRGB mean {mean}");
        // Normals point mostly up (blue).
        let blue: f32 =
            set.normal.data.chunks(4).map(|p| p[2] as f32).sum::<f32>() / (1024.0 * 1024.0);
        assert!(blue > 200.0, "{blue}");
        let (albedos, normal, orm) = painted(&[[0.8, 0.8, 0.8], [0.05, 0.05, 0.05]], false);
        assert_eq!(albedos.len(), 2);
        assert!(normal.is_valid() && orm.is_valid());
    }
}
