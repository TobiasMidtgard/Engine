//! Per-texel material functions for [`super::bake_set`]. Each takes texel coordinates in
//! `[0, 1)` across one tile.

use std::f32::consts::{PI, TAU};

use engine_core::prelude::*;

use super::{Texel, mix3, noise, sat, sstep};

/// Sun-bleached planks: grain along V, checks and knots (0.5 m wide, 1 m long tile).
pub fn wood_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    // Grain: rings stretched along v, warped.
    let warp = fbm_tile(u * 4.0, v * 2.0, 4, 2, 71) * 0.6;
    let knot = {
        let (f1, _, id) = cells_xy(u * 3.0, v * 2.0, 3, 2, 72);
        if (id & 3) == 0 {
            sstep(0.25, 0.0, f1)
        } else {
            0.0
        }
    };
    let rings = ((u * 60.0 + warp * 6.0 + knot * 4.0) * TAU).sin() * 0.5 + 0.5;
    let fibre = noised_xy(u * 300.0, v * 6.0, 300, 6, 73)[0];
    let check = (1.0 - noised_xy(u * 90.0, v * 3.0, 90, 3, 74)[0].abs() * 18.0).max(0.0)
        * sstep(0.1, 0.4, fbm_tile(u * 2.0, v * 4.0, 3, 2, 75));
    let tone = 0.85 + 0.15 * fbm_tile(u * 2.0, v * 1.0, 3, 1, 76);
    let mut albedo =
        Vec3::new(0.3, 0.215, 0.15) * tone * (0.82 + 0.18 * rings) * (0.95 + 0.1 * fibre);
    albedo *= 1.0 - 0.6 * check - 0.3 * knot;
    Texel {
        albedo,
        height: rings * 0.0006 + fibre * 0.0002 - check * 0.0015 - knot * 0.0008,
        rough: 0.82 - 0.05 * rings,
        ao: 1.0 - 0.5 * check,
        ..Default::default()
    }
}

/// Old Brannoc's awning: cream and madder stripes (see [`canvas_texel`]).
pub fn brannoc_canvas_texel(u: f32, v: f32) -> Texel {
    canvas_texel(u, v, CREAM, Vec3::new(0.40, 0.10, 0.05), 9.0, 0)
}

/// Unbleached canvas (linear), the ground of every striped awning.
pub const CREAM: Vec3 = Vec3::new(0.62, 0.52, 0.37);

/// An awning: `stripes` stripes of `a` and `b` (linear albedo) across, with weave, grime
/// (placed by `seed`) and a scalloped valance strip (not tiled: v in 0..0.82 is the roof, the
/// rest the valance).
pub fn canvas_texel(u: f32, v: f32, a: Vec3, b: Vec3, stripes: f32, seed: u32) -> Texel {
    use noise::*;
    let stripe = ((u * stripes).fract() < 0.5) as i32 as f32;
    let mut albedo = mix3(a, b, stripe);
    // Weave.
    let (wu, wv) = (u * 900.0, v * 900.0);
    let weave = (wu * TAU).sin() * (wv * TAU).sin();
    let fuzz = noised(u * 220.0, v * 220.0, 220, 81)[0];
    // Grime: darker toward the edges and in streaks running down the slope (v).
    let edge = 1.0 - sstep(0.0, 0.08, u.min(1.0 - u)) * sstep(0.0, 0.1, v.min(0.82 - v).abs());
    let streak = sat(fbm(u * 30.0, v * 2.0, 4, 82 + seed * 17) * 1.5);
    let stain = sat(fbm(u * 5.0, v * 5.0, 5, 83 + seed * 17));
    albedo *= (1.0 - 0.3 * edge - 0.22 * streak - 0.25 * stain) * (0.95 + 0.05 * fuzz);
    // Sun fading: the reds are paler.
    albedo = mix3(albedo, Vec3::splat(albedo.dot(Vec3::splat(0.333))), 0.15);
    let mut alpha = 1.0;
    if v > 0.82 {
        // Valance: scalloped bottom edge.
        let s = (u * 18.0).fract() - 0.5;
        let bottom = 0.99 - 0.06 * (1.0 - 4.0 * s * s);
        alpha = if v < bottom { 1.0 } else { 0.0 };
        albedo *= 0.92;
    }
    Texel {
        albedo,
        height: weave * 0.00012 + fuzz * 0.00005,
        rough: 0.9,
        ao: 1.0 - 0.15 * edge,
        alpha,
        ..Default::default()
    }
}

/// Painted metal: chipped paint over steel, rust streaks running down (2 m tile).
pub fn metal_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let chips = fbm_tile(u * 12.0, v * 12.0, 6, 12, 91);
    let chip = sstep(0.32, 0.36, chips);
    let streak = sat(fbm_tile_xy(u * 40.0, v * 3.0, 4, 40, 3, 92) * 2.0 - 0.2);
    let grime = fbm_tile(u * 4.0, v * 4.0, 4, 4, 93);
    let scratches = (1.0 - noised_xy(u * 6.0, v * 160.0, 6, 160, 94)[0].abs() * 30.0).max(0.0);
    let paint = Vec3::splat(0.85) * (0.92 + 0.08 * grime);
    let rust = Vec3::new(0.32, 0.14, 0.06) * (0.8 + 0.4 * grime);
    let steel = Vec3::new(0.55, 0.55, 0.56);
    let mut albedo = mix3(paint, rust, streak * 0.55);
    albedo = mix3(albedo, steel, chip);
    albedo = mix3(albedo, steel * 1.1, scratches * 0.4 * (1.0 - chip));
    Texel {
        albedo,
        height: -chip * 0.0002 + grime * 0.00008,
        rough: 0.5 + 0.3 * streak - 0.15 * chip + 0.1 * grime,
        metal: chip.max(scratches * 0.5),
        ..Default::default()
    }
}

/// Trodden soil with grit and a little fine gravel (2.5 m tile). Alpha is ragged noise: the
/// path decal cuts its edges with it.
pub fn dirt_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let soil = fbm_tile(u * 6.0, v * 6.0, 6, 6, 211);
    let clods = fbm_tile(u * 40.0, v * 40.0, 3, 40, 217);
    let grit = noised(u * 300.0, v * 300.0, 300, 212)[0];
    let mut albedo = Vec3::new(0.8, 0.72, 0.62)
        * (0.86 + 0.2 * soil)
        * (0.92 + 0.1 * clods)
        * (0.93 + 0.12 * grit);
    let mut height = soil * 0.006 + clods * 0.0015 + grit * 0.0003;
    let mut rough = 0.92;
    let mut ao: f32 = 1.0;
    let gravel_zone = sstep(-0.2, 0.4, fbm_tile(u * 5.0 + 3.0, v * 5.0, 3, 5, 218));
    for (n, density, lift, seed) in [
        (150.0f32, 0.55f32, 0.0025f32, 213u32),
        (60.0, 0.12, 0.005, 214),
    ] {
        let wx = fbm_tile(u * n * 0.25, v * n * 0.25, 2, (n * 0.25) as i32, seed + 10) * 0.45;
        let (f1, f2, id) = cells(u * n + wx, v * n - wx, n as i32, seed);
        let r = 0.18 + 0.25 * ((id >> 8) & 255) as f32 / 255.0;
        let has = (((id >> 16) & 255) as f32 / 255.0) < density * gravel_zone;
        if has && f1 < r && f2 - f1 > 0.05 {
            let dome = (1.0 - (f1 / r).powi(2)).max(0.0).sqrt();
            let tone = 0.7 + 0.35 * (id & 255) as f32 / 255.0;
            let warm = ((id >> 3) & 1) as f32;
            let stone = mix3(
                Vec3::new(0.78, 0.76, 0.74),
                Vec3::new(0.86, 0.74, 0.6),
                warm,
            ) * tone;
            let k = sstep(0.0, 0.5, dome);
            albedo = mix3(albedo * 0.8, stone, k * 0.8);
            height = height.max(dome * lift);
            rough = 0.92 - 0.15 * k;
        } else if has && f1 < r * 1.4 {
            ao *= 0.7 + 0.3 * sstep(r, r * 1.4, f1);
        }
    }
    let scuff = sstep(0.2, 0.5, fbm_tile_xy(u * 14.0, v * 5.0, 3, 14, 5, 215));
    albedo *= 1.0 - 0.08 * scuff;
    height -= scuff * 0.002;
    let alpha = 0.5 + 0.5 * sat(0.5 + 0.9 * fbm_tile(u * 12.0, v * 12.0, 5, 12, 216));
    Texel {
        albedo,
        height,
        rough,
        ao,
        alpha,
        ..Default::default()
    }
}

/// Weathered stone: grain, sparse thin cracks, mineral speckle and lichen (1.6 m tile).
pub fn rock_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let grain = fbm_tile(u * 5.0, v * 5.0, 8, 5, 61);
    let broad = fbm_tile(u * 2.0, v * 2.0, 3, 2, 67);
    let speck = noised(u * 420.0, v * 420.0, 420, 62)[0];
    // Cracks along the zero lines of warped noise, only in places.
    let w = fbm_tile(u * 4.0, v * 4.0, 3, 4, 63) * 0.5;
    let c1 = noised(u * 3.0 + w, v * 3.0 - w, 3, 64)[0].abs();
    let c2 = noised(u * 9.0 - w, v * 9.0 + w, 9, 68)[0].abs();
    let where1 = sstep(0.0, 0.4, fbm_tile(u * 2.0 + 0.7, v * 2.0, 3, 2, 70));
    let crack =
        sat(sstep(0.016, 0.0, c1) * where1 + 0.5 * sstep(0.012, 0.0, c2) * sstep(0.1, 0.5, grain));
    let pits = sstep(0.6, 0.8, noised(u * 90.0, v * 90.0, 90, 69)[0]);
    let base =
        Vec3::new(0.34, 0.27, 0.2) * (0.72 + 0.4 * grain + 0.2 * broad) * (0.93 + 0.1 * speck);
    // Grit: light and dark mineral grains.
    let grit = noised(u * 160.0, v * 160.0, 160, 71)[0];
    let (g1, _, gid) = cells(u * 220.0, v * 220.0, 220, 72);
    let spark = sstep(0.3, 0.15, g1) * ((gid & 7) == 0) as i32 as f32;
    let mut albedo =
        base * (1.0 - 0.45 * crack - 0.2 * pits) * (0.9 + 0.18 * grit) * (1.0 + 0.5 * spark);
    // Lichen: small round colonies, clustered.
    let cluster = sstep(0.05, 0.4, fbm_tile(u * 3.0 + 4.0, v * 3.0, 4, 3, 65));
    let (f1, _, id) = cells(u * 40.0, v * 40.0, 40, 66);
    let r = 0.15 + 0.35 * ((id >> 8) & 255) as f32 / 255.0;
    let colony = sstep(r, r * 0.6, f1) * cluster * ((id & 3) != 0) as i32 as f32;
    let lichen_col = match (id >> 4) & 3 {
        0 => Vec3::new(0.5, 0.3, 0.1),
        1 => Vec3::new(0.08, 0.08, 0.07),
        _ => Vec3::new(0.4, 0.4, 0.3),
    };
    albedo = mix3(
        albedo,
        lichen_col * (0.85 + 0.25 * speck),
        colony * (1.0 - crack) * 0.6,
    );
    Texel {
        albedo,
        height: grain * 0.006 + broad * 0.004 - crack * 0.002 - pits * 0.0006
            + colony * 0.0004
            + grit * 0.0005,
        rough: 0.86 + 0.08 * colony - 0.05 * speck.max(0.0),
        ao: 1.0 - 0.4 * crack - 0.2 * pits,
        ..Default::default()
    }
}

/// Market cobbles (2 m tile): rounded setts about 12 cm across in grey and ochre stone, packed
/// earth and grit in tight joints, moss where few feet go, polish on the crowns. Alpha frays
/// toward 0.5 in the joints, so a vertex alpha fading below 1 breaks the paving up stone by
/// stone at its edge.
pub fn cobble_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let n = 16i32;
    let wx = fbm_tile(u * 4.0, v * 4.0, 3, 4, 301) * 0.25;
    let wy = fbm_tile(u * 4.0 + 7.0, v * 4.0, 3, 4, 302) * 0.25;
    let (x, y) = (u * n as f32 + wx, v * n as f32 + wy);
    // Nearest two stone centers, jittered less than full cells so the setts stay even.
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2, mut id) = (9.0f32, 9.0f32, 0u32);
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (cx, cy) = (ix + ox, iy + oy);
            let h = hash(wrap(cx, n), wrap(cy, n), 7, 303);
            let jx = 0.2 + 0.6 * (h & 0xffff) as f32 / 65536.0;
            let jy = 0.2 + 0.6 * (h >> 16) as f32 / 65536.0;
            let d = ((cx as f32 + jx - x).powi(2) + (cy as f32 + jy - y).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = h;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    // Distance to the joint (the bisector between the two nearest centers), cell units.
    let edge = (f2 - f1) * 0.5;
    let joint_w = 0.035 + 0.025 * fbm_tile(u * 20.0, v * 20.0, 2, 20, 304);
    let crown = sstep(joint_w, joint_w + 0.3, edge);
    let r = |k: u32| ((id >> k) & 255) as f32 / 255.0;
    let warm = Vec3::new(0.40, 0.33, 0.25);
    let grey = Vec3::new(0.33, 0.32, 0.30);
    let dark = Vec3::new(0.22, 0.21, 0.20);
    let mut stone = mix3(grey, warm, r(8));
    stone = mix3(stone, dark, sstep(0.6, 1.0, r(16)) * 0.9);
    stone *= 0.78 + 0.4 * r(0);
    // Broad grime and wear across the square.
    stone *= 0.85 + 0.25 * fbm_tile(u * 2.0, v * 2.0, 3, 2, 309);
    let grain = fbm_tile(u * 60.0, v * 60.0, 4, 60, 305);
    let pits = sstep(0.4, 0.65, noised(u * 160.0, v * 160.0, 160, 306)[0]);
    let mut albedo = stone * (0.92 + 0.1 * grain) * (1.0 - 0.2 * pits);
    let earth =
        Vec3::new(0.20, 0.16, 0.12) * (0.85 + 0.3 * fbm_tile(u * 30.0, v * 30.0, 3, 30, 307));
    let moss_zone = sstep(0.15, 0.55, fbm_tile(u * 3.0, v * 3.0, 4, 3, 308));
    let moss = Vec3::new(0.10, 0.14, 0.05);
    let joint = 1.0 - sstep(joint_w * 0.6, joint_w * 1.6, edge);
    albedo = mix3(albedo, mix3(earth, moss, moss_zone * 0.6), joint);
    // Grime settles toward the joints; worn crowns are a little lighter.
    albedo *= 0.82 + 0.22 * crown;
    let dome = (crown * (2.0 - crown)).sqrt();
    Texel {
        albedo,
        height: dome * 0.012 + grain * 0.0008 - pits * 0.0008,
        rough: 0.6 + 0.3 * joint - 0.12 * crown * (1.0 - pits),
        ao: 0.5 + 0.5 * sstep(0.0, 0.5, crown),
        alpha: 0.5 + 0.5 * crown,
        ..Default::default()
    }
}

/// Woven wicker (0.4 m tile): over-and-under willow rods, for baskets and the cart's sides.
pub fn wicker_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let (n_rod, n_stake) = (40.0, 8.0);
    let (ru, sv) = (v * n_rod, u * n_stake);
    let row = ru.floor() as i32;
    let across = ru.fract();
    let rod = (1.0 - (2.0 * across - 1.0).powi(2)).max(0.0).sqrt();
    // Stakes stand at whole `sv`; each rod weaves over one and under the next, the next row
    // the other way round, rising and falling smoothly between them.
    let bulge = 0.5 + 0.5 * (PI * (sv + row as f32)).cos();
    let h = rod * (0.4 + 0.6 * bulge);
    // Fine fibres run along the rods.
    let fibre = noised_xy(u * 200.0, v * 800.0, 200, 800, 311)[0];
    let tone = 0.85 + 0.25 * noise::hash(row, 0, 0, 312) as f32 / u32::MAX as f32;
    let albedo = Vec3::new(0.42, 0.30, 0.16) * tone * (0.75 + 0.3 * h) * (0.95 + 0.08 * fibre);
    Texel {
        albedo,
        height: h * 0.004,
        rough: 0.7,
        ao: 0.45 + 0.55 * h,
        ..Default::default()
    }
}

/// Plain-woven linen (0.5 m tile), near white so vertex colors dye it.
pub fn linen_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    // 100 threads a tile: five texels each at 512², so the weave survives in the normals.
    let (wu, wv) = (u * 100.0, v * 100.0);
    let weave = (wu * TAU).sin() * (wv * TAU).sin();
    let slub = fbm_tile_xy(u * 30.0, v * 2.0, 3, 30, 2, 321);
    let soil = sat(fbm_tile(u * 3.0, v * 3.0, 4, 3, 322) * 0.8 + 0.2);
    let albedo = Vec3::splat(0.82) * (0.94 + 0.04 * weave + 0.04 * slub) * (1.0 - 0.12 * soil);
    Texel {
        albedo,
        height: weave * 0.00015 + slub * 0.0001,
        rough: 0.88,
        ..Default::default()
    }
}

/// A tavern chalkboard (0.6 × 0.95 m, v down from the top): smudged slate with a chalk
/// border, a yellow heading, three lines of a menu with prices and a little drawn tankard.
/// Read from a few metres off, the hand-written strokes pass for words.
pub fn chalkboard_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    // Wobbly hand-drawn line `d` px from its path, `w` half-width (all in UV units of v).
    let ink = |d: f32, w: f32| 1.0 - sstep(w * 0.6, w, d.abs());
    // Writing along a line centred at `vc` from `u0` to `u1`: loops of letters, gaps between
    // words.
    let words = |vc: f32, u0: f32, u1: f32, a: f32, w: f32, seed: u32| -> f32 {
        if u < u0 || u > u1 || (v - vc).abs() > a * 3.0 {
            return 0.0;
        }
        let gap = n2(u * 11.0, seed as f32, seed) < -0.3;
        if gap {
            return 0.0;
        }
        let f = 260.0;
        let ph = n2(u * 6.0, 0.5, seed + 1) * 2.0;
        let loops = (u * f + ph).sin() * 0.55 + (u * f * 0.43 + ph * 1.7).sin() * 0.45;
        // Taller strokes now and then: capitals and ascenders.
        let tall = sstep(0.55, 0.9, n2(u * 40.0, 1.5, seed + 2)) * 1.4;
        let y = vc + a * loops * (1.0 + tall * (loops < 0.0) as u8 as f32);
        let ends = sstep(u0, u0 + 0.01, u) * (1.0 - sstep(u1 - 0.01, u1, u));
        ink(v - y, w) * ends
    };
    let mut white = 0.0f32;
    let mut yellow = 0.0f32;
    // A border inset from the frame.
    let wob = |x: f32, s: u32| n2(x * 7.0, 0.3, s) * 0.004;
    let (mu, mv) = (0.07, 0.045);
    if (mu..1.0 - mu).contains(&u) {
        white = white.max(ink(v - mv - wob(u, 11), 0.0035));
        white = white.max(ink(v - (1.0 - mv) - wob(u, 12), 0.0035));
    }
    if (mv..1.0 - mv).contains(&v) {
        white = white.max(ink((u - mu - wob(v, 13)) * 0.632, 0.0035));
        white = white.max(ink((u - (1.0 - mu) - wob(v, 14)) * 0.632, 0.0035));
    }
    // Heading and its underline.
    yellow = yellow.max(words(0.13, 0.2, 0.8, 0.017, 0.0065, 21));
    if (0.24..0.76).contains(&u) {
        yellow = yellow.max(ink(v - 0.185 - wob(u, 22) * 0.6, 0.0028));
    }
    // Three dishes and their prices.
    for (k, vc) in [0.29f32, 0.38, 0.47].into_iter().enumerate() {
        let k = k as u32;
        white = white.max(words(
            vc,
            0.13,
            0.5 + 0.08 * (k % 2) as f32,
            0.011,
            0.0042,
            31 + k,
        ));
        white = white.max(words(vc, 0.73, 0.86, 0.011, 0.0042, 41 + k));
    }
    // A foaming tankard: body, handle and a froth cloud.
    let (cx, cy) = (0.5, 0.71);
    let (x, y) = ((u - cx) * 0.632, v - cy);
    let body = (x.abs() - 0.075).max(y.abs() - 0.09);
    white = white.max(ink(body, 0.0045));
    let handle = ((x - 0.075).hypot(y + 0.005) - 0.045).abs();
    if x > 0.075 {
        white = white.max(ink(handle, 0.0045));
    }
    let froth = (0..3)
        .map(|i| {
            let fx = -0.05 + 0.05 * i as f32;
            (x - fx).hypot(y + 0.095) - 0.035
        })
        .fold(f32::MAX, f32::min);
    if y < -0.085 {
        white = white.max(ink(froth, 0.0045));
    }
    for i in 0..3 {
        let sx = -0.04 + 0.04 * i as f32;
        if (-0.06..0.07).contains(&y) {
            white = white.max(ink((x - sx) * 1.0, 0.003) * 0.7);
        }
    }
    // Chalk is grainy and patchy; slate keeps the ghosts of older menus.
    let grain = sat(0.55 + 0.5 * fbm(u * 380.0, v * 240.0, 2, 51));
    let (white, yellow) = (white * grain, yellow * grain);
    let smudge = sat(fbm(u * 2.4, v * 1.5, 4, 52) * 0.9 + 0.15);
    let ghosts = sat(fbm(u * 30.0, v * 20.0, 3, 53) * 0.8) * smudge;
    let slate = Vec3::new(0.021, 0.026, 0.024) * (0.85 + 0.3 * fbm(u * 60.0, v * 40.0, 3, 54));
    let slate = slate + Vec3::splat(0.03 * smudge + 0.02 * ghosts);
    let albedo = mix3(
        mix3(slate, Vec3::new(0.78, 0.78, 0.74), white),
        Vec3::new(0.85, 0.66, 0.24),
        yellow,
    );
    let chalk = white.max(yellow);
    Texel {
        albedo,
        height: chalk * 0.00004 + fbm(u * 60.0, v * 40.0, 3, 55) * 0.00002,
        rough: 0.78 + 0.18 * chalk,
        ..Default::default()
    }
}

/// A heightfield sampled on a grid, for map textures: `h[y][x]`, with bilinear lookups.
pub struct HeightGrid {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl HeightGrid {
    /// Sample `f(u, v)` on a `w`×`h` grid over `[0, 1]²` (on all cores).
    pub fn sample(w: usize, h: usize, f: impl Fn(f32, f32) -> f32 + Sync) -> Self {
        let rows = super::par_map(h, |y| {
            (0..w)
                .map(|x| f(x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32))
                .collect::<Vec<f32>>()
        });
        Self {
            w,
            h,
            data: rows.concat(),
        }
    }

    pub fn at(&self, u: f32, v: f32) -> f32 {
        let x = sat(u) * (self.w - 1) as f32;
        let y = sat(v) * (self.h - 1) as f32;
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let g = |x: usize, y: usize| self.data[y * self.w + x];
        let a = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * fx;
        let b = g(x0, y1) + (g(x1, y1) - g(x0, y1)) * fx;
        a + (b - a) * fy
    }
}

/// An explorer's map on aged parchment (not tiled): ink coastlines, contour lines every
/// `contour` of height, hatched mountains, shaded seas, a frame and a compass rose. `land`
/// is the terrain over the sheet (`[0, 1]²`), `sea` its sea level and `marks` the places to
/// ring (sheet coordinates).
pub fn parchment_map_texel(
    u: f32,
    v: f32,
    land: &HeightGrid,
    sea: f32,
    contour: f32,
    marks: &[Vec2],
) -> Texel {
    use noise::*;
    let px = 1.0 / land.w as f32;
    let h = land.at(u, v);
    let hx = land.at(u + px, v) - land.at(u - px, v);
    let hy = land.at(u, v + px) - land.at(u, v - px);
    let slope = (hx * hx + hy * hy).sqrt() / (2.0 * px);
    // Paper: warm, blotchy, darker toward the burnt edges.
    let blotch = fbm(u * 6.0, v * 6.0, 5, 331);
    let fibres = noised(u * 500.0, v * 120.0, 0, 332)[0];
    let edge_d = u.min(1.0 - u).min(v.min(1.0 - v));
    let ragged = edge_d + 0.012 * fbm(u * 40.0, v * 40.0, 3, 333);
    let burn = sstep(0.06, 0.0, ragged);
    let mut paper = Vec3::new(0.78, 0.66, 0.46) * (0.9 + 0.12 * blotch) * (0.97 + 0.04 * fibres);
    paper = mix3(paper, Vec3::new(0.36, 0.22, 0.1), burn * 0.8);
    let ink = Vec3::new(0.12, 0.08, 0.05);
    let sea_ink = Vec3::new(0.30, 0.36, 0.38);
    let mut albedo = paper;
    let mut ink_k = 0.0f32;
    if h < sea {
        // Sea: a pale wash with ripple lines following the coast.
        let depth = sea - h;
        albedo = mix3(albedo, sea_ink * 1.6, 0.35 * sstep(0.0, 0.03, depth));
        let ripple = ((depth / 0.012) * TAU).cos();
        if depth < 0.05 {
            ink_k = ink_k.max(sstep(0.85, 0.97, ripple) * 0.45 * (1.0 - depth / 0.05));
        }
    } else {
        // Contours, heavier every fifth.
        let c = (h - sea) / contour;
        let d = (c - c.round()).abs() / (slope / contour * px * 2.0).max(1e-4);
        let major = (c.round() as i32).rem_euclid(5) == 0;
        if d < 1.2 {
            ink_k = ink_k.max((1.0 - d / 1.2) * if major { 0.7 } else { 0.35 });
        }
        // Hatching on steep, high ground.
        let high = sstep(0.12, 0.25, h - sea);
        let hatch = ((u + v) * 260.0 * TAU).sin();
        ink_k = ink_k.max(high * sstep(0.6, 0.95, hatch) * sstep(0.5, 2.0, slope) * 0.6);
    }
    // Coastline: a firm double line.
    let coast = (h - sea).abs() / (slope * px * 2.0).max(1e-4);
    ink_k = ink_k.max(sstep(1.6, 0.4, coast) * 0.95);
    // Frame: a ruled double border inside the edge.
    for (inset, w) in [(0.045f32, 0.0035f32), (0.055, 0.0015)] {
        let d = (edge_d - inset).abs();
        ink_k = ink_k.max(sstep(w, w * 0.4, d) * 0.85);
    }
    // Places: small inked rings.
    for m in marks {
        let d = Vec2::new(u, v).distance(*m);
        ink_k = ink_k.max(sstep(0.004, 0.0015, (d - 0.012).abs()) * 0.9);
        ink_k = ink_k.max(sstep(0.004, 0.002, d) * 0.9);
    }
    // Compass rose (lower right): a four-pointed star in a ring.
    let rose = Vec2::new(0.85, 0.8);
    let q = Vec2::new(u, v) - rose;
    let (r, a) = (q.length(), q.y.atan2(q.x));
    let star = 0.07 * (0.25 + 0.75 * (2.0 * a).cos().abs().powf(8.0));
    ink_k = ink_k.max(sstep(0.003, 0.0, (r - 0.055).abs()) * 0.8);
    if r < star {
        ink_k = ink_k.max(if (a * 4.0 / TAU).fract() < 0.5 {
            0.85
        } else {
            0.4
        });
    }
    // Ink soaks unevenly into the paper.
    ink_k *= 0.75 + 0.3 * noised(u * 300.0, v * 300.0, 0, 334)[0];
    albedo = mix3(albedo, ink, sat(ink_k));
    Texel {
        albedo,
        height: blotch * 0.0002 - burn * 0.0004,
        rough: 0.85 - 0.15 * ink_k,
        ao: 1.0 - 0.3 * burn,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How much `channel` jumps across the line `a` (texel pitch `e`, along u when `across_u`,
    /// else along v) beyond the steps beside it, averaged along the line. A seam is a line
    /// that jumps where its neighbours don't.
    fn jump(
        f: fn(f32, f32) -> Texel,
        channel: fn(Texel) -> f32,
        a: f32,
        e: f32,
        across_u: bool,
    ) -> f32 {
        let at = |x: f32, t: f32| {
            let x = x.rem_euclid(1.0);
            channel(if across_u { f(x, t) } else { f(t, x) })
        };
        let mut sum = 0.0;
        for k in 0..512 {
            let t = (k as f32 + 0.5) / 512.0;
            let (l3, l1, r1, r3) = (
                at(a - 3.0 * e, t),
                at(a - e, t),
                at(a + e, t),
                at(a + 3.0 * e, t),
            );
            sum += ((r1 - l1).abs() - (l1 - l3).abs().max((r3 - r1).abs())).max(0.0);
        }
        sum / 512.0
    }

    /// Tiled textures meet themselves across their edges: height and color jump no more across
    /// an edge than across the worst of 15 lines inside the tile.
    #[test]
    fn tiled_textures_have_no_seams() {
        fn height(t: Texel) -> f32 {
            t.height
        }
        fn albedo(t: Texel) -> f32 {
            t.albedo.element_sum() / 3.0
        }
        for (name, f, (w, h), range) in [
            (
                "cobble",
                cobble_texel as fn(f32, f32) -> Texel,
                (1024.0, 1024.0),
                0.012,
            ),
            ("wicker", wicker_texel, (512.0, 512.0), 0.004),
            ("linen", linen_texel, (512.0, 512.0), 0.0003),
            ("wood", wood_texel, (512.0, 1024.0), 0.002),
            ("metal", metal_texel, (512.0, 512.0), 0.0003),
            ("dirt", dirt_texel, (1024.0, 1024.0), 0.008),
            ("rock", rock_texel, (1024.0, 1024.0), 0.012),
        ] {
            // Heights span `range` meters; colors about half of [0, 1].
            for (channel, get, range) in [
                ("height", height as fn(Texel) -> f32, range),
                ("albedo", albedo, 0.5),
            ] {
                for (axis, e) in [("u", 0.5 / w), ("v", 0.5 / h)] {
                    let across_u = axis == "u";
                    let seam = jump(f, get, 0.0, e, across_u);
                    let inside = (1..16)
                        .map(|k| jump(f, get, k as f32 / 16.0, e, across_u))
                        .fold(0.0f32, f32::max);
                    assert!(
                        seam <= inside * 2.0 + range * 1e-3,
                        "{name}: a {channel} seam across {axis} (jump {seam}, inside up to {inside})"
                    );
                }
            }
        }
    }
}
