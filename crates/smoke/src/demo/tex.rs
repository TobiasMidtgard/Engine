//! Procedural, tileable PBR textures for the range: poured and precast concrete, woven
//! sandbag cloth, painted metal with chips and rust, parkerized gun steel, rusty and polished
//! steel, weathered timber, berm earth, track ballast, worn line paint, galvanized steel and
//! shot-up steel target faces.
//!
//! Each material is baked once into three images (glTF conventions): base color (sRGB, alpha
//! for cut-outs), a tangent-space normal map from a height field, and an ORM image (R ambient
//! occlusion, G roughness, B metallic) that serves as both the metallic/roughness and the
//! occlusion texture. All noise wraps at the texture edges, so every texture tiles.

use engine_core::math::{Vec2, Vec3};
use engine_render::Image;

/// One texel of a material before baking.
#[derive(Debug, Clone, Copy)]
pub struct Texel {
    /// Linear albedo.
    pub albedo: Vec3,
    /// Coverage (alpha-masked materials), 0..1.
    pub alpha: f32,
    /// Surface height, meters.
    pub height: f32,
    pub rough: f32,
    pub metal: f32,
    /// Cavity / ambient occlusion, 0..1.
    pub ao: f32,
}

impl Default for Texel {
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

/// The three images of a baked material.
pub struct Maps {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
}

// ------------------------------------------------------------------------------- noise

#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// Hash to 0..1.
#[inline]
pub fn hash01(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / (1u32 << 24) as f32
}

fn grads() -> &'static [Vec2; 64] {
    static G: std::sync::OnceLock<[Vec2; 64]> = std::sync::OnceLock::new();
    G.get_or_init(|| {
        let mut g = [Vec2::ZERO; 64];
        for (i, v) in g.iter_mut().enumerate() {
            let a = (i as f32 + 0.5) / 64.0 * std::f32::consts::TAU;
            *v = Vec2::new(a.cos(), a.sin());
        }
        g
    })
}

#[inline]
fn wrap(i: i32, p: i32) -> i32 {
    i.rem_euclid(p.max(1))
}

/// Gradient noise at (x, y) in lattice units, repeating every `px` × `py` cells. About -1..1.
pub fn noise(x: f32, y: f32, px: i32, py: i32, seed: u32) -> f32 {
    let g = grads();
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (fx, fy) = (x - xf, y - yf);
    let dot = |ix: i32, iy: i32, dx: f32, dy: f32| {
        let h = hash(wrap(ix, px), wrap(iy, py), seed);
        let v = g[(h & 63) as usize];
        v.x * dx + v.y * dy
    };
    let n00 = dot(xi, yi, fx, fy);
    let n10 = dot(xi + 1, yi, fx - 1.0, fy);
    let n01 = dot(xi, yi + 1, fx, fy - 1.0);
    let n11 = dot(xi + 1, yi + 1, fx - 1.0, fy - 1.0);
    let s = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v) = (s(fx), s(fy));
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 1.41
}

/// Fractal noise over the unit tile (u, v in 0..1): `fx` × `fy` cells at the first octave,
/// doubling each octave, amplitude × `gain`. About -1..1.
pub fn fbm(u: f32, v: f32, fx: i32, fy: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut px, mut py) = (fx, fy);
    for o in 0..octaves {
        sum += amp
            * noise(
                u * px as f32,
                v * py as f32,
                px,
                py,
                seed.wrapping_add(o * 977),
            );
        norm += amp;
        amp *= gain;
        px *= 2;
        py *= 2;
    }
    sum / norm
}

/// Ridged fractal (sharp creases at zero crossings), 0..1 with 1 on the creases.
pub fn ridged(u: f32, v: f32, f: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let mut p = f;
    for o in 0..octaves {
        let n = 1.0 - noise(u * p as f32, v * p as f32, p, p, seed.wrapping_add(o * 313)).abs();
        sum += amp * n * n;
        norm += amp;
        amp *= 0.5;
        p *= 2;
    }
    sum / norm
}

/// Cellular noise over the unit tile with `f` × `f` cells: distance to the nearest and
/// second-nearest feature point (in cell units), the nearest point's cell hash, and the offset
/// from it (cell units).
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub f1: f32,
    pub f2: f32,
    pub id: u32,
    pub d: Vec2,
}

pub fn worley(u: f32, v: f32, f: i32, jitter: f32, seed: u32) -> Cell {
    let (x, y) = (u * f as f32, v * f as f32);
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let mut best = Cell {
        f1: 9.0,
        f2: 9.0,
        id: 0,
        d: Vec2::ZERO,
    };
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (xi + dx, yi + dy);
            let h = hash(wrap(cx, f), wrap(cy, f), seed);
            let jx = (h & 0xffff) as f32 / 65535.0;
            let jy = (h >> 16) as f32 / 65535.0;
            let p = Vec2::new(
                cx as f32 + 0.5 + (jx - 0.5) * jitter,
                cy as f32 + 0.5 + (jy - 0.5) * jitter,
            );
            let d = Vec2::new(x, y) - p;
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
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// sRGB hex to linear RGB.
pub fn hex(rgb: u32) -> Vec3 {
    Vec3::from(engine_render::material::srgb_hex_to_linear(rgb))
}

// ------------------------------------------------------------------------------- baking

fn par_rows<T: Send>(data: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let rows = data.len() / width.max(1);
    let per = rows.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (k, chunk) in data.chunks_mut(per * width).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (r, row) in chunk.chunks_mut(width).enumerate() {
                    f(k * per + r, row);
                }
            });
        }
    });
}

/// Evaluate `f(u, v)` for every texel of a `size` × `size` tile (u, v at texel centers).
pub fn grid<T: Clone + Send>(size: u32, init: T, f: impl Fn(f32, f32) -> T + Sync) -> Vec<T> {
    let n = size as usize;
    let mut out = vec![init; n * n];
    par_rows(&mut out, n, |y, row| {
        let v = (y as f32 + 0.5) / n as f32;
        for (x, t) in row.iter_mut().enumerate() {
            *t = f((x as f32 + 0.5) / n as f32, v);
        }
    });
    out
}

/// [`grid`] of texels.
pub fn texels(size: u32, f: impl Fn(f32, f32) -> Texel + Sync) -> Vec<Texel> {
    grid(size, Texel::default(), f)
}

fn to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn unorm(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// The albedo image of baked texels.
pub fn albedo_image(size: u32, t: &[Texel]) -> Image {
    let mut data = vec![0u8; t.len() * 4];
    par_rows(&mut data, size as usize * 4, |y, row| {
        for x in 0..size as usize {
            let s = &t[y * size as usize + x];
            row[x * 4] = to_srgb(s.albedo.x);
            row[x * 4 + 1] = to_srgb(s.albedo.y);
            row[x * 4 + 2] = to_srgb(s.albedo.z);
            row[x * 4 + 3] = unorm(s.alpha);
        }
    });
    Image::new(size, size, data, true)
}

/// Bake texels into the three maps. `tile_m` is the size of the tile in meters (for the
/// normal map's slopes).
pub fn bake(size: u32, tile_m: f32, t: &[Texel]) -> Maps {
    let n = size as usize;
    let texel_m = tile_m / size as f32;
    let mut normal = vec![0u8; n * n * 4];
    par_rows(&mut normal, n * 4, |y, row| {
        let (ym, yp) = ((y + n - 1) % n, (y + 1) % n);
        for x in 0..n {
            let (xm, xp) = ((x + n - 1) % n, (x + 1) % n);
            let dhdu = (t[y * n + xp].height - t[y * n + xm].height) / (2.0 * texel_m);
            let dhdv = (t[yp * n + x].height - t[ym * n + x].height) / (2.0 * texel_m);
            let nn = Vec3::new(-dhdu, dhdv, 1.0).normalize();
            row[x * 4] = unorm(nn.x * 0.5 + 0.5);
            row[x * 4 + 1] = unorm(nn.y * 0.5 + 0.5);
            row[x * 4 + 2] = unorm(nn.z * 0.5 + 0.5);
            row[x * 4 + 3] = 255;
        }
    });
    let mut orm = vec![0u8; n * n * 4];
    par_rows(&mut orm, n * 4, |y, row| {
        for x in 0..n {
            let s = &t[y * n + x];
            row[x * 4] = unorm(s.ao);
            row[x * 4 + 1] = unorm(s.rough);
            row[x * 4 + 2] = unorm(s.metal);
            row[x * 4 + 3] = 255;
        }
    });
    Maps {
        albedo: albedo_image(size, t),
        normal: Image::new(size, size, normal, false),
        orm: Image::new(size, size, orm, false),
    }
}

// ------------------------------------------------------------------------------- materials

/// Poured concrete (also used, tinted, for precast barriers): mottled cement paste, sand
/// grain, exposed aggregate, air pores, a broom finish, hairline cracks, damp and oil stains
/// and chalky efflorescence. Tile: about 3 m.
pub fn concrete(u: f32, v: f32, seed: u32) -> Texel {
    let s = seed;
    // Tone: large mottling, medium blotches, dark damp/oil stains, light chalky patches.
    let mottle = fbm(u, v, 3, 3, 5, 0.55, s);
    let blotch = fbm(u, v, 9, 9, 4, 0.5, s + 11);
    let stain = smoothstep(0.12, 0.5, fbm(u, v, 2, 2, 5, 0.6, s + 23));
    let efflo = smoothstep(0.25, 0.65, fbm(u, v, 4, 4, 4, 0.55, s + 29));
    // Sand grain and broom streaks (along u, about 5 mm apart).
    let grain = noise(u * 900.0, v * 900.0, 900, 900, s + 31) * 0.5
        + noise(u * 420.0, v * 420.0, 420, 420, s + 37) * 0.5;
    let broom = noise(u * 5.0, v * 600.0, 5, 600, s + 41) * 0.65
        + noise(u * 11.0, v * 280.0, 11, 280, s + 43) * 0.35;
    let broom_fade = smoothstep(-0.4, 0.3, blotch);
    // Aggregate: stones peeking through where the paste is worn.
    let agg = worley(u, v, 140, 0.9, s + 53);
    let stone =
        smoothstep(0.42, 0.26, agg.f1) * smoothstep(0.05, 0.45, blotch + mottle * 0.5 + 0.25);
    let stone_tone = (agg.id & 255) as f32 / 255.0;
    let stone_hue = ((agg.id >> 8) & 255) as f32 / 255.0;
    // Pores: small round pits.
    let pore_cell = worley(u, v, 260, 0.95, s + 61);
    let pore_size = 0.12 + ((pore_cell.id >> 16) & 255) as f32 / 255.0 * 0.22;
    let pore = if ((pore_cell.id >> 8) & 15) == 0 {
        smoothstep(pore_size, pore_size * 0.4, pore_cell.f1) * 0.7
    } else {
        0.0
    };
    // Hairline cracks.
    let crack_n = ridged(u, v, 4, 4, s + 71);
    let crack_mask = smoothstep(0.1, 0.5, fbm(u, v, 3, 3, 3, 0.5, s + 79));
    let crack = smoothstep(0.955, 0.99, crack_n) * crack_mask;

    let paste = Vec3::new(0.30, 0.295, 0.28)
        * (1.0 + mottle * 0.24 + blotch * 0.1 + grain * 0.07 + broom * broom_fade * 0.03);
    let stone_col = lerp3(
        Vec3::new(0.17, 0.165, 0.16),
        Vec3::new(0.44, 0.41, 0.37),
        stone_tone,
    ) * lerp3(
        Vec3::new(1.05, 1.0, 0.94),
        Vec3::new(0.95, 0.98, 1.04),
        stone_hue,
    );
    let mut albedo = lerp3(paste, stone_col, stone * 0.75);
    albedo = lerp3(albedo, Vec3::new(0.40, 0.395, 0.38), efflo * 0.3);
    albedo = lerp3(albedo, albedo * Vec3::new(0.62, 0.6, 0.56), stain * 0.6);
    albedo *= 1.0 - pore * 0.45 - crack * 0.65;
    let height = mottle * 0.0012 + grain * 0.00012 + broom * broom_fade * 0.00016 + stone * 0.00025
        - pore * 0.0009
        - crack * 0.0006;
    let rough = (0.88 + grain * 0.05 - stone * 0.12 + pore * 0.06 - stain * 0.14 + efflo * 0.05)
        .clamp(0.4, 1.0);
    Texel {
        albedo,
        height,
        rough,
        ao: 1.0 - pore * 0.6 - crack * 0.5,
        ..Default::default()
    }
}

/// Woven polypropylene sandbag cloth: flat tapes in a plain weave with fibers, fraying and
/// dust caught in the weave. Tile: 0.12 m (24 tapes each way). Neutral tan; bags are tinted
/// per vertex.
pub fn cloth(u: f32, v: f32, seed: u32) -> Texel {
    let n = 24.0;
    let (x, y) = (u * n, v * n);
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let warp_up = (xi as i32 + yi as i32).rem_euclid(2) == 0;
    // Tape cross-section: rounded across its width with a small gap between tapes.
    let across_w = (fx - 0.5).abs() * 2.0; // warp runs along v; its width is across u
    let across_f = (fy - 0.5).abs() * 2.0;
    let tape_w = (1.0 - across_w.powf(4.0)).max(0.0).sqrt();
    let tape_f = (1.0 - across_f.powf(4.0)).max(0.0).sqrt();
    // Over/under: each tape bends as it passes over the crossing ones.
    let bend_w = (fy * std::f32::consts::PI).sin() * if warp_up { 1.0 } else { -1.0 };
    let bend_f = (fx * std::f32::consts::PI).sin() * if warp_up { -1.0 } else { 1.0 };
    let hw = tape_w * 0.6 + bend_w * 0.4;
    let hf = tape_f * 0.6 + bend_f * 0.4;
    let (h, warp) = if hw >= hf { (hw, true) } else { (hf, false) };
    // Fibers along each tape.
    let fiber = if warp {
        noise(u * 18.0, v * 1400.0, 18, 1400, seed + 3)
    } else {
        noise(u * 1400.0, v * 18.0, 1400, 18, seed + 5)
    };
    let tape_tone = hash01(xi as i32, yi as i32, seed + if warp { 7 } else { 9 });
    let dirt = smoothstep(-0.1, 0.6, fbm(u, v, 3, 3, 5, 0.55, seed + 13));
    let gap = 1.0 - smoothstep(0.0, 0.35, h.max(0.0));
    let base = Vec3::new(0.50, 0.43, 0.30) * (0.92 + tape_tone * 0.1 + fiber * 0.06);
    let mut albedo = lerp3(base, Vec3::new(0.26, 0.21, 0.15), dirt * 0.45);
    albedo *= 1.0 - gap * 0.45;
    Texel {
        albedo,
        height: h * 0.0006 + fiber * 0.00004,
        rough: (0.72 + gap * 0.2 + dirt * 0.1 - fiber * 0.04).clamp(0.4, 1.0),
        ao: 1.0 - gap * 0.5,
        ..Default::default()
    }
}

/// Shared pattern of painted metal: small chips through the paint to the primer and, in the
/// bigger ones, to bare steel (a few of them rusting), fine scratches, and grime. Wear gathers
/// in patches; elsewhere the paint is nearly whole. Tile: 0.5 m.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaintWear {
    /// 1 = paint, 0 = chipped.
    pub paint: f32,
    /// Primer showing (in chips), 0..1.
    pub primer: f32,
    /// Rust on bare steel, 0..1.
    pub rust: f32,
    pub dirt: f32,
    /// Sun-faded paint, about -1..1.
    pub fade: f32,
}

pub fn paint_wear(u: f32, v: f32, wear: f32, seed: u32) -> (PaintWear, Texel) {
    // Where the wear gathers.
    let zone = smoothstep(-0.25, 0.45, fbm(u, v, 3, 3, 4, 0.5, seed));
    // Chips: 2-15 mm flakes with ragged edges.
    let n = fbm(u, v, 36, 36, 4, 0.62, seed + 1);
    let t = 0.5 - wear * 0.24 * zone;
    let chip = smoothstep(t, t + 0.012, n);
    let deep = smoothstep(t + 0.05, t + 0.065, n);
    // Scratches: long thin creases in two directions, in places.
    let s1 = noise((u + v) * 3.0, (v - u) * 140.0, 3, 140, seed + 3).abs();
    let s2 = noise((u - v * 2.0) * 2.0, (v + u * 2.0) * 90.0, 2, 90, seed + 4).abs();
    let scratch_mask = smoothstep(0.15, 0.55, fbm(u, v, 4, 4, 3, 0.5, seed + 5)) * wear;
    let scratch =
        (smoothstep(0.022, 0.006, s1) + smoothstep(0.016, 0.004, s2) * 0.8) * scratch_mask;
    let scratch = scratch.clamp(0.0, 1.0);
    let bare = (deep + scratch * 0.7).clamp(0.0, 1.0);
    let primer = (chip - deep).clamp(0.0, 1.0) * (1.0 - scratch * 0.7);
    let paint = (1.0 - chip.max(scratch * 0.9)).clamp(0.0, 1.0);
    let rust = bare * smoothstep(0.1, 0.45, fbm(u, v, 7, 7, 3, 0.5, seed + 7));
    let dirt = smoothstep(-0.05, 0.6, fbm(u, v, 2, 2, 5, 0.6, seed + 13));
    let fade = fbm(u, v, 4, 4, 4, 0.5, seed + 21);
    let orange = noise(u * 160.0, v * 160.0, 160, 160, seed + 11);
    // Paint and primer layers 0.08 mm each: chip edges step down; rust lifts a little.
    let height =
        paint * 0.00008 + (paint + primer) * 0.00008 + orange * 0.000008 * paint + rust * 0.00003;
    let rough = paint * (0.48 + dirt * 0.22 + orange * 0.03 + fade.max(0.0) * 0.08)
        + primer * 0.72
        + bare * (0.32 * (1.0 - rust) + 0.88 * rust);
    (
        PaintWear {
            paint,
            primer,
            rust,
            dirt,
            fade,
        },
        Texel {
            albedo: Vec3::ZERO,
            height,
            rough: rough.clamp(0.2, 1.0),
            metal: bare * (1.0 - rust),
            ao: 1.0 - chip * 0.15,
            alpha: 1.0,
        },
    )
}

/// Albedo of painted metal in `color` (linear) over the shared wear pattern.
pub fn paint_albedo(w: &PaintWear, color: Vec3, u: f32, v: f32, seed: u32) -> Vec3 {
    let luma = color.dot(Vec3::new(0.3, 0.59, 0.11));
    // Sun-faded paint turns lighter and grayer; grime darkens it.
    let faded = lerp3(color, Vec3::splat(luma * 1.2 + 0.02), 0.3);
    let mut paint =
        lerp3(color, faded, smoothstep(0.0, 0.8, w.fade) * 0.5) * (1.0 + w.fade * 0.035);
    paint = lerp3(paint, Vec3::new(0.11, 0.095, 0.075), w.dirt * 0.16);
    let primer =
        Vec3::new(0.2, 0.075, 0.045) * (1.0 + noise(u * 60.0, v * 60.0, 60, 60, seed) * 0.15);
    let steel = Vec3::new(0.48, 0.48, 0.49);
    let rust = lerp3(
        Vec3::new(0.12, 0.05, 0.022),
        Vec3::new(0.26, 0.11, 0.04),
        noise(u * 90.0, v * 90.0, 90, 90, seed + 23) * 0.5 + 0.5,
    );
    let bare = lerp3(steel, rust, w.rust);
    let under = if w.primer + (1.0 - w.paint) > 0.0 {
        lerp3(
            bare,
            primer,
            (w.primer / (1.0 - w.paint).max(1e-3)).clamp(0.0, 1.0),
        )
    } else {
        bare
    };
    lerp3(under, paint, w.paint)
}

/// Parkerized gun steel: dark gray phosphate grain, oily patches, and bright steel where
/// hands and belts have worn the finish off. Tile 0.3 m.
pub fn gunmetal(u: f32, v: f32, seed: u32) -> Texel {
    let grain = noise(u * 700.0, v * 700.0, 700, 700, seed) * 0.6
        + noise(u * 300.0, v * 300.0, 300, 300, seed + 1) * 0.4;
    let n = fbm(u, v, 6, 6, 5, 0.62, seed + 2);
    let wear = smoothstep(0.32, 0.5, n);
    let rub = smoothstep(0.12, 0.45, n) * 0.5;
    let scratch_n = noise(u * 4.0, v * 220.0, 4, 220, seed + 4).abs();
    let scratch = smoothstep(0.02, 0.005, scratch_n)
        * smoothstep(0.0, 0.4, fbm(u, v, 3, 3, 3, 0.5, seed + 5));
    let bright = (wear + scratch * 0.7).clamp(0.0, 1.0);
    let oil = smoothstep(0.1, 0.6, fbm(u, v, 3, 3, 3, 0.5, seed + 3));
    let base = Vec3::new(0.085, 0.087, 0.09) * (1.0 + grain * 0.22 + rub * 0.5);
    let worn = Vec3::new(0.42, 0.42, 0.43);
    Texel {
        albedo: lerp3(base, worn, bright * 0.85),
        height: grain * 0.00002 - scratch * 0.00001,
        rough: (0.6 - rub * 0.12 - bright * 0.25 - oil * 0.14 + grain * 0.05).clamp(0.2, 1.0),
        metal: 0.3 + bright * 0.7,
        ..Default::default()
    }
}

/// Rusty mill-scale steel (rails, chains, old stands). Tile 0.6 m.
pub fn rusty(u: f32, v: f32, seed: u32) -> Texel {
    let n = fbm(u, v, 4, 4, 6, 0.58, seed);
    let rust = smoothstep(-0.25, 0.25, n);
    let flake = worley(u, v, 60, 1.0, seed + 1);
    let pit = smoothstep(0.25, 0.05, flake.f1) * rust;
    let fine = noise(u * 400.0, v * 400.0, 400, 400, seed + 2);
    let tone = (flake.id & 255) as f32 / 255.0;
    let scale = Vec3::new(0.085, 0.083, 0.082) * (1.0 + fine * 0.2);
    let rust_col = lerp3(
        Vec3::new(0.09, 0.045, 0.025),
        Vec3::new(0.22, 0.11, 0.05),
        tone * 0.7 + fine * 0.15 + 0.15,
    );
    Texel {
        albedo: lerp3(scale, rust_col, rust) * (1.0 - pit * 0.3),
        height: rust * 0.0002 + fine * 0.00003 - pit * 0.0002
            + (flake.f2 - flake.f1) * rust * 0.00008,
        rough: (0.55 + rust * 0.35 + fine * 0.05).clamp(0.3, 1.0),
        metal: (1.0 - rust) * 0.85,
        ao: 1.0 - pit * 0.4,
        ..Default::default()
    }
}

/// Polished, scratched steel (rail heads, machined parts; tinted for brass). Tile 0.25 m.
pub fn polished(u: f32, v: f32, seed: u32) -> Texel {
    let streak = noise(u * 4.0, v * 600.0, 4, 600, seed) * 0.6
        + noise(u * 8.0, v * 1500.0, 8, 1500, seed + 1) * 0.4;
    let smudge = fbm(u, v, 3, 3, 4, 0.5, seed + 2);
    Texel {
        albedo: Vec3::splat(0.62 + streak * 0.04 + smudge * 0.03),
        height: streak * 0.000004,
        rough: (0.24 + streak.abs() * 0.08 + smudge * 0.06).clamp(0.08, 1.0),
        metal: 1.0,
        ..Default::default()
    }
}

/// Weathered timber, grain along u: growth rings, checks (cracks) along the grain, knots and
/// gray weathering. Tile 1.2 m along the grain by 1.2 m.
pub fn timber(u: f32, v: f32, seed: u32) -> Texel {
    let warp = fbm(u, v, 2, 6, 4, 0.5, seed) * 2.0;
    let knot = worley(u, v, 5, 0.8, seed + 1);
    let knot_on = (knot.id & 3) == 0;
    let k = if knot_on {
        smoothstep(0.22, 0.0, knot.f1)
    } else {
        0.0
    };
    let ring_coord = v * 70.0 + warp + k * 4.0 * (knot.d.x * 6.0).sin();
    let ring = (ring_coord * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    let ring = ring.powf(3.0);
    let fiber = noise(u * 30.0, v * 900.0, 30, 900, seed + 2);
    let check_n = noise(u * 3.0, v * 120.0, 3, 120, seed + 3).abs();
    let check =
        smoothstep(0.03, 0.0, check_n) * smoothstep(0.0, 0.4, fbm(u, v, 4, 2, 3, 0.5, seed + 4));
    let weather = smoothstep(-0.3, 0.5, fbm(u, v, 2, 3, 5, 0.55, seed + 5));
    let wood = lerp3(
        Vec3::new(0.30, 0.19, 0.10),
        Vec3::new(0.18, 0.105, 0.055),
        ring,
    );
    let gray = Vec3::new(0.30, 0.28, 0.26) * (1.0 + fiber * 0.1);
    let mut albedo = lerp3(wood, gray, weather * 0.75);
    albedo = lerp3(albedo, Vec3::new(0.08, 0.05, 0.03), k * 0.7);
    albedo *= 1.0 - check * 0.7 + fiber * 0.05;
    Texel {
        albedo,
        height: -ring * 0.0004 * weather + fiber * 0.0001 - check * 0.0015 + k * 0.0003,
        rough: (0.78 + weather * 0.12 - k * 0.1 + fiber * 0.04).clamp(0.4, 1.0),
        ao: 1.0 - check * 0.6,
        ..Default::default()
    }
}

/// Packed earth with pebbles, clods and dry plant bits (berm). Tile 2.5 m.
pub fn earth(u: f32, v: f32, seed: u32) -> Texel {
    let base_n = fbm(u, v, 4, 4, 6, 0.55, seed);
    let clod = worley(u, v, 36, 0.9, seed + 1);
    let pebble = worley(u, v, 90, 0.9, seed + 2);
    let peb_on = (pebble.id & 3) == 0;
    let peb = if peb_on {
        smoothstep(0.38, 0.22, pebble.f1)
    } else {
        0.0
    };
    let peb_tone = ((pebble.id >> 4) & 255) as f32 / 255.0;
    let fine = noise(u * 500.0, v * 500.0, 500, 500, seed + 3);
    let dry = smoothstep(-0.2, 0.4, fbm(u, v, 3, 3, 4, 0.5, seed + 4));
    let straw_n = noise(u * 60.0 + v * 20.0, v * 200.0, 60, 200, seed + 5).abs();
    let straw =
        smoothstep(0.02, 0.0, straw_n) * smoothstep(0.2, 0.6, fbm(u, v, 5, 5, 3, 0.5, seed + 6));
    let soil = lerp3(
        Vec3::new(0.14, 0.11, 0.08),
        Vec3::new(0.27, 0.22, 0.16),
        dry,
    ) * (1.0 + base_n * 0.15 + fine * 0.06);
    let stone = lerp3(
        Vec3::new(0.25, 0.24, 0.22),
        Vec3::new(0.48, 0.45, 0.40),
        peb_tone,
    );
    let mut albedo = lerp3(soil, stone, peb);
    albedo = lerp3(albedo, Vec3::new(0.45, 0.38, 0.22), straw * 0.8);
    let clod_h = smoothstep(0.0, 0.5, clod.f2 - clod.f1);
    Texel {
        albedo,
        height: base_n * 0.01 + clod_h * 0.004 + peb * 0.005 + fine * 0.0004,
        rough: (0.9 + fine * 0.05 - peb * 0.15).clamp(0.4, 1.0),
        ao: 0.75 + clod_h * 0.25,
        ..Default::default()
    }
}

/// Angular crushed granite (track ballast, the shoulder): two sizes of stones packed
/// together, each with its own tone and a tilted facet, shallow shadowed gaps. Tile 1.0 m.
pub fn gravel(u: f32, v: f32, seed: u32) -> Texel {
    let big = worley(u, v, 20, 1.0, seed);
    let small = worley(u + 0.013, v + 0.007, 46, 1.0, seed + 1);
    // The bigger stones sit on top where they reach; small ones fill the gaps.
    let big_dome = smoothstep(0.0, 0.3, big.f2 - big.f1);
    let small_dome = smoothstep(0.0, 0.3, small.f2 - small.f1);
    let on_big = big_dome > 0.35 && (big.id & 3) != 0;
    let (c, dome, scale) = if on_big {
        (big, big_dome, 1.0)
    } else {
        (small, small_dome * 0.8, 0.45)
    };
    let a = (c.id & 0xff) as f32 / 255.0 * std::f32::consts::TAU;
    let facet = (c.d.x * a.cos() + c.d.y * a.sin()) * 0.5;
    let tone = ((c.id >> 8) & 255) as f32 / 255.0;
    let hue = ((c.id >> 16) & 255) as f32 / 255.0;
    let speck = noise(u * 700.0, v * 700.0, 700, 700, seed + 2);
    let mut stone = lerp3(
        Vec3::new(0.2, 0.195, 0.19),
        Vec3::new(0.46, 0.44, 0.41),
        tone,
    ) * lerp3(Vec3::new(1.06, 1.0, 0.92), Vec3::new(0.95, 0.98, 1.04), hue)
        * (1.0 + speck * 0.16 + facet * 0.12);
    // Dust settled between the stones and rust from the rails.
    let dust = smoothstep(0.5, 0.0, dome);
    stone = lerp3(stone, Vec3::new(0.25, 0.22, 0.18), dust * 0.6);
    let rusty = smoothstep(0.55, 0.85, fbm(u, v, 3, 3, 3, 0.5, seed + 3)) * 0.35;
    let albedo = lerp3(stone, stone * Vec3::new(0.8, 0.62, 0.48), rusty) * (1.0 - dust * 0.3);
    Texel {
        albedo,
        height: (dome * 0.01 + facet * 0.004 * dome) * scale,
        rough: (0.82 + speck * 0.06 + dust * 0.08 - facet.abs() * 0.05).clamp(0.4, 1.0),
        ao: 0.55 + dome * 0.45,
        ..Default::default()
    }
}

/// Worn road paint: coverage in alpha (bare concrete shows through), slightly raised, with
/// grit. Tile 1.0 m. White; tint with the material's base color.
pub fn line_paint(u: f32, v: f32, seed: u32) -> Texel {
    let wear = fbm(u, v, 6, 6, 5, 0.6, seed) + fbm(u, v, 30, 30, 3, 0.5, seed + 1) * 0.4;
    let cover = smoothstep(-0.38, -0.3, wear);
    let grit = noise(u * 500.0, v * 500.0, 500, 500, seed + 2);
    let dirt = smoothstep(-0.1, 0.6, fbm(u, v, 3, 3, 4, 0.5, seed + 3));
    Texel {
        albedo: Vec3::splat(0.82 + grit * 0.05) * (1.0 - dirt * 0.3),
        alpha: cover,
        height: grit * 0.00004 + cover * 0.0002,
        rough: (0.55 + dirt * 0.2 + grit * 0.05).clamp(0.3, 1.0),
        ..Default::default()
    }
}

/// Hot-dip galvanized steel: spangle crystals, white rust and grime. Tile 0.3 m.
pub fn galvanized(u: f32, v: f32, seed: u32) -> Texel {
    let sp = worley(u, v, 30, 1.0, seed);
    let tone = (sp.id & 255) as f32 / 255.0;
    let edge = smoothstep(0.06, 0.0, sp.f2 - sp.f1);
    let white = smoothstep(0.3, 0.7, fbm(u, v, 4, 4, 4, 0.55, seed + 1));
    let grime = smoothstep(0.0, 0.7, fbm(u, v, 2, 2, 4, 0.55, seed + 2));
    let albedo = Vec3::splat(0.5 + tone * 0.15 - edge * 0.08);
    let albedo = lerp3(albedo, Vec3::splat(0.7), white * 0.5);
    let albedo = lerp3(albedo, Vec3::new(0.2, 0.19, 0.17), grime * 0.4);
    Texel {
        albedo,
        height: tone * 0.00001 - edge * 0.00001,
        rough: (0.32 + tone * 0.1 + white * 0.35 + grime * 0.15).clamp(0.1, 1.0),
        metal: 1.0 - white * 0.5,
        ..Default::default()
    }
}

/// One bullet strike on a target face (quadrant-local coordinates, 0..1).
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    c: Vec2,
    size: f32,
    /// 0 = fresh, 1 = painted over.
    age: f32,
    depth: f32,
    seed: f32,
}

/// The strikes of the four target-face variants, bucketed on a grid per quadrant.
pub struct TargetHits {
    cells: Vec<Vec<Hit>>,
}

const HIT_GRID: usize = 16;
/// Farthest a strike's splash reaches, quadrant units.
const HIT_REACH: f32 = 0.09;

impl TargetHits {
    pub fn new(seed: u32) -> Self {
        let mut cells = vec![Vec::new(); 4 * HIT_GRID * HIT_GRID];
        for q in 0..4i32 {
            let sd = seed + q as u32 * 1000;
            for i in 0..140 {
                let h = hash(i, q, sd);
                let r1 = (h & 0xffff) as f32 / 65535.0;
                let r2 = (h >> 16) as f32 / 65535.0;
                let h2 = hash(i, q + 7, sd);
                let r3 = (h2 & 0xffff) as f32 / 65535.0;
                let r4 = (h2 >> 16) as f32 / 65535.0;
                // Box-Muller: a Gaussian group around the aim point, a few strays.
                let rad = (-2.0 * r1.max(1e-4).ln()).sqrt() * if i % 9 == 0 { 0.28 } else { 0.12 };
                let ang = r2 * std::f32::consts::TAU;
                let hit = Hit {
                    c: Vec2::new(0.5 + rad * ang.cos(), 0.45 + rad * ang.sin()),
                    size: 0.007 + r4 * 0.009,
                    age: r3,
                    depth: 0.6 + r4 * 0.4,
                    seed: i as f32 * 1.7,
                };
                let lo = ((hit.c - HIT_REACH) * HIT_GRID as f32).floor();
                let hi = ((hit.c + HIT_REACH) * HIT_GRID as f32).floor();
                for gy in
                    (lo.y.max(0.0) as usize)..=(hi.y.min(HIT_GRID as f32 - 1.0).max(-1.0) as usize)
                {
                    for gx in (lo.x.max(0.0) as usize)
                        ..=(hi.x.min(HIT_GRID as f32 - 1.0).max(-1.0) as usize)
                    {
                        cells[(q as usize * HIT_GRID + gy) * HIT_GRID + gx].push(hit);
                    }
                }
            }
        }
        Self { cells }
    }

    fn near(&self, q: usize, p: Vec2) -> &[Hit] {
        let gx = ((p.x * HIT_GRID as f32) as usize).min(HIT_GRID - 1);
        let gy = ((p.y * HIT_GRID as f32) as usize).min(HIT_GRID - 1);
        &self.cells[(q * HIT_GRID + gy) * HIT_GRID + gx]
    }
}

/// A shot-up steel target face: range-white paint over AR500 steel, a lead-gray splash
/// with radial spatter and a dimple for every hit, older hits half painted over, rust streaks
/// running down from the bottom edge. One variant per quadrant of the tile (u, v in 0..1).
pub fn target_face(hits: &TargetHits, u: f32, v: f32, seed: u32) -> Texel {
    let qx = if u < 0.5 { 0 } else { 1 };
    let qy = if v < 0.5 { 0 } else { 1 };
    let q = qx + qy * 2;
    // Local coords in the quadrant, 0..1, center 0.5.
    let lu = (u - qx as f32 * 0.5) * 2.0;
    let lv = (v - qy as f32 * 0.5) * 2.0;
    let sd = seed + q as u32 * 1000;
    let (w, wt) = paint_wear(u, v, 0.25, sd + 7);
    let mut paint = 1.0f32;
    let mut lead = 0.0f32;
    let mut dent = 0.0f32;
    let mut old = 0.0f32;
    let p = Vec2::new(lu, lv);
    for hit in hits.near(q, p) {
        let d = p - hit.c;
        let dist = d.length();
        if dist > HIT_REACH {
            continue;
        }
        let size = hit.size;
        // Spatter: rays out from the hit.
        let a = d.y.atan2(d.x);
        let rays = (noise(
            (a / std::f32::consts::TAU + 0.5) * 16.0,
            hit.seed,
            16,
            1000,
            sd + 3,
        ) * 0.5
            + 0.5)
            .powf(3.0);
        let splash = smoothstep(size * (2.0 + rays * 4.5), size * 0.6, dist);
        let core = smoothstep(size, size * 0.4, dist);
        if hit.age > 0.55 {
            old = old.max(splash * 0.6);
        } else {
            lead = lead.max(splash * (0.6 + 0.4 * (1.0 - hit.age)));
            paint = paint.min(1.0 - core);
        }
        dent += smoothstep(size * 1.4, 0.0, dist) * hit.depth;
    }
    let rust_streak = smoothstep(0.65, 1.0, lv)
        * smoothstep(
            0.3,
            0.7,
            noise(lu * 40.0, lv * 3.0, 1000, 1000, sd + 9) * 0.5 + 0.5,
        );
    let white = Vec3::new(0.74, 0.74, 0.7) * (1.0 + fbm(u, v, 6, 6, 4, 0.5, sd + 11) * 0.06);
    let mut albedo = lerp3(white, white * Vec3::new(0.83, 0.82, 0.8), old);
    albedo = lerp3(albedo, Vec3::new(0.2, 0.2, 0.21), lead * 0.85);
    albedo = lerp3(albedo, paint_albedo(&w, white, u, v, sd), 1.0 - w.paint);
    albedo = lerp3(Vec3::new(0.25, 0.25, 0.26), albedo, paint);
    albedo = lerp3(albedo, Vec3::new(0.3, 0.13, 0.05), rust_streak * 0.5);
    Texel {
        albedo,
        height: wt.height - dent.min(1.5) * 0.0015,
        rough: (0.55 + lead * 0.15 - (1.0 - paint) * 0.2 + old * 0.05).clamp(0.2, 1.0),
        metal: (1.0 - paint) * 0.8 + lead * 0.3,
        ao: 1.0 - dent.min(1.0) * 0.3,
        ..Default::default()
    }
}

/// The perforated cooling jacket: gun steel with round holes (alpha 0) on a staggered 10 mm
/// grid. Tile: 0.1 m (10 × 10 holes).
pub fn jacket(u: f32, v: f32, seed: u32) -> Texel {
    let mut t = gunmetal(u, v, seed);
    let n = 10.0;
    let y = v * n;
    let row = y.floor();
    let x = u * n + if (row as i32) % 2 == 0 { 0.0 } else { 0.5 };
    let d = Vec2::new(x.fract() - 0.5, y.fract() - 0.5).length();
    let hole_r = 0.31;
    let rim = smoothstep(hole_r + 0.12, hole_r, d);
    t.alpha = if d < hole_r { 0.0 } else { 1.0 };
    t.height -= rim * 0.0004;
    t.ao *= 1.0 - rim * 0.4;
    t
}

/// Molded rubber: fine grain and a little dust. Tile 0.1 m.
pub fn rubber(u: f32, v: f32, seed: u32) -> Texel {
    let grain = noise(u * 200.0, v * 200.0, 200, 200, seed) * 0.6
        + noise(u * 64.0, v * 64.0, 64, 64, seed + 1) * 0.4;
    let dust = smoothstep(0.1, 0.7, fbm(u, v, 2, 2, 4, 0.5, seed + 2));
    Texel {
        albedo: lerp3(Vec3::splat(0.035), Vec3::new(0.12, 0.11, 0.1), dust * 0.6)
            * (1.0 + grain * 0.15),
        height: grain * 0.00003,
        rough: (0.82 + grain * 0.06 + dust * 0.08).clamp(0.4, 1.0),
        ..Default::default()
    }
}

/// A soot scorch for the ground (one decal over the tile, u, v in 0..1): a ragged,
/// streaky ring of soot, darkest at the middle, fading out (alpha) toward the edge.
pub fn scorch(u: f32, v: f32, seed: u32) -> Texel {
    let d = Vec2::new(u - 0.5, v - 0.5);
    let r = d.length() * 2.0;
    let a = d.y.atan2(d.x) / std::f32::consts::TAU + 0.5;
    let ragged = fbm(a, r * 0.4, 7, 2, 4, 0.55, seed) * 0.3;
    let streaks = (noise(a * 20.0, r * 2.0, 20, 1000, seed + 2) * 0.5 + 0.5)
        * (noise(a * 7.0, r * 1.2, 7, 1000, seed + 3) * 0.5 + 0.5);
    let rr = r + ragged;
    let core = smoothstep(0.45, 0.0, rr);
    let halo = smoothstep(1.0, 0.15, rr).powf(1.6);
    let mottle = fbm(u, v, 8, 8, 4, 0.5, seed + 1);
    let cover =
        (halo * (0.55 + 0.45 * streaks) * (0.85 + 0.3 * mottle) + core * 0.35).clamp(0.0, 1.0);
    Texel {
        albedo: lerp3(Vec3::new(0.08, 0.072, 0.064), Vec3::splat(0.02), core),
        alpha: cover * 0.85,
        rough: 0.92,
        ..Default::default()
    }
}

/// An oil stain for the ground (one decal over the tile): a dark, slightly glossy blot with a
/// darker rim where it dried, a few drips around it, fading out (alpha) at the edge.
pub fn oil(u: f32, v: f32, seed: u32) -> Texel {
    let d = Vec2::new(u - 0.5, v - 0.5);
    let r = d.length() * 2.0;
    let a = d.y.atan2(d.x) / std::f32::consts::TAU + 0.5;
    let wobble = fbm(a, r * 0.3, 7, 2, 4, 0.55, seed) * 0.35;
    let rr = r + wobble;
    let body = smoothstep(0.75, 0.55, rr);
    let rim = smoothstep(0.12, 0.0, (rr - 0.68).abs()) * 0.5;
    let drip_cell = worley(u, v, 12, 0.9, seed + 1);
    let drip = if (drip_cell.id & 3) == 0 {
        smoothstep(0.25, 0.12, drip_cell.f1) * smoothstep(1.0, 0.6, r)
    } else {
        0.0
    };
    let mottle = fbm(u, v, 6, 6, 4, 0.5, seed + 2);
    let soak = smoothstep(-0.3, 0.5, fbm(u, v, 5, 5, 4, 0.55, seed + 3));
    let cover =
        (body * (0.3 + soak * 0.4 + mottle * 0.15) + rim * 0.6 + drip * 0.5).clamp(0.0, 1.0);
    Texel {
        albedo: Vec3::new(0.045, 0.04, 0.034) * (1.0 + mottle * 0.3),
        alpha: cover * 0.7,
        rough: (0.42 - body * 0.1 + mottle * 0.08).clamp(0.2, 1.0),
        ..Default::default()
    }
}

/// A tuft of dry grass for cross cards: tapered, curving blades rising from the bottom edge
/// (v = 1) of the tile, green at the roots and straw toward the tips, transparent around them.
pub fn grass(u: f32, v: f32, seed: u32) -> Texel {
    let mut top: Option<(f32, f32, u32)> = None;
    let mut cover = 0.0f32;
    let px = 1.5 / 512.0;
    for i in 0..60 {
        let h = hash(i, 0, seed);
        let r = |k: u32| ((h >> (k * 8)) & 255) as f32 / 255.0;
        let x0 = 0.5 + (r(0) - 0.5) * 0.56;
        let height = 0.42 + r(1) * 0.55;
        let t = (1.0 - v) / height;
        if !(0.0..=1.0).contains(&t) {
            continue;
        }
        let bend = (r(2) - 0.5) * 0.5 + (x0 - 0.5) * 0.9;
        let w0 = 0.008 + r(3) * 0.012;
        let cx = x0 + bend * t * t;
        let hw = w0 * (1.0 - t).powf(0.7) + 0.0012;
        let dx = (u - cx).abs();
        let c = smoothstep(hw + px * 0.5, hw - px * 0.5, dx);
        if c > 0.0 {
            cover = cover.max(c);
            top = Some((t, dx / hw, i as u32));
        }
    }
    let Some((t, across, i)) = top else {
        return Texel {
            albedo: Vec3::new(0.3, 0.28, 0.16),
            alpha: 0.0,
            rough: 0.8,
            ..Default::default()
        };
    };
    let tone = hash01(i as i32, 1, seed);
    let green = lerp3(
        Vec3::new(0.07, 0.11, 0.03),
        Vec3::new(0.15, 0.19, 0.05),
        tone,
    );
    let straw = lerp3(
        Vec3::new(0.27, 0.23, 0.12),
        Vec3::new(0.38, 0.32, 0.18),
        tone,
    );
    let dry = (tone * 0.7 + t * 0.45 - 0.1).clamp(0.0, 1.0);
    let mut albedo = lerp3(green, straw, smoothstep(0.2, 0.9, dry));
    albedo *= 0.45 + 0.55 * smoothstep(0.0, 0.45, t);
    albedo *= 1.0 + (1.0 - across).max(0.0) * 0.12;
    Texel {
        albedo,
        alpha: (cover * 1.4).min(1.0),
        height: (1.0 - across * across).max(0.0) * 0.0003,
        rough: 0.72 + t * 0.1,
        ao: 0.6 + 0.4 * smoothstep(0.0, 0.5, t),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_and_stays_in_range() {
        for i in 0..50 {
            let (u, v) = (i as f32 * 0.0173 % 1.0, i as f32 * 0.0311 % 1.0);
            let a = fbm(u, v, 3, 5, 4, 0.5, 9);
            let b = fbm(u + 1.0, v + 1.0, 3, 5, 4, 0.5, 9);
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
            assert!(a.abs() <= 1.5);
            let c = worley(u, v, 7, 1.0, 3);
            let d = worley(u + 1.0, v, 7, 1.0, 3);
            assert!((c.f1 - d.f1).abs() < 1e-3);
        }
    }

    #[test]
    fn bakes_valid_images() {
        let t = texels(32, |u, v| concrete(u, v, 1));
        let maps = bake(32, 1.0, &t);
        assert!(maps.albedo.is_valid() && maps.normal.is_valid() && maps.orm.is_valid());
        assert!(maps.albedo.srgb && !maps.normal.srgb);
        // Normal map points mostly out of the surface.
        let p = maps.normal.pixel(5, 5);
        assert!(p[2] > 200, "{p:?}");
    }
}
