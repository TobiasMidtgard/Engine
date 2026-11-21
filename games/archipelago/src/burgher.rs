//! The third tier's buildings: Burghers' houses in each climate's style, the manufactories
//! that join two regions' produce (glassworks, saltery, confectioner, furrier, lamp maker)
//! and the two public buildings Burghers ask for (school, tavern).
//!
//! Models stand in a building's own space (origin at the middle of the footprint at floor
//! height, front toward +Z), in the same art kit as `looks`, `regional` and `civic`.

use std::f32::consts::{PI, TAU};

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::civic::{dome, sheet};
use crate::defs::Kind;
use crate::globe::Biome;
use crate::looks::{ModelKey, Piece, WHITE, h01};
use crate::regional::{SNOW, beam_ends, mud, parapet, railing, roof_snow, up_quad};

/// The models this module builds (they are listed in `looks::MODELS`; a test checks that).
#[cfg(test)]
pub const MODELS: &[ModelKey] = &[
    "burgher_0",
    "burgher_1",
    "burgher_2",
    "villa_0",
    "villa_1",
    "villa_2",
    "mansion_0",
    "mansion_1",
    "mansion_2",
    "merchant_0",
    "merchant_1",
    "merchant_2",
    "glassworks",
    "saltery",
    "confectioner",
    "furrier",
    "lamp_maker",
    "school",
    "tavern",
    "sand_heap",
    "glass_rack",
];

pub fn build(key: ModelKey) -> Option<Parts> {
    Some(match key {
        "burgher_0" => burgher(0),
        "burgher_1" => burgher(1),
        "burgher_2" => burgher(2),
        "villa_0" => villa(0),
        "villa_1" => villa(1),
        "villa_2" => villa(2),
        "mansion_0" => mansion(0),
        "mansion_1" => mansion(1),
        "mansion_2" => mansion(2),
        "merchant_0" => merchant(0),
        "merchant_1" => merchant(1),
        "merchant_2" => merchant(2),
        "glassworks" => glassworks(),
        "saltery" => saltery(),
        "confectioner" => confectioner(),
        "furrier" => furrier(),
        "lamp_maker" => lamp_maker(),
        "school" => school(),
        "tavern" => tavern(),
        "sand_heap" => sand_heap(),
        "glass_rack" => glass_rack(),
        _ => return None,
    })
}

/// The Burghers' house model of a climate.
pub fn house_model(biome: Biome, v: usize) -> ModelKey {
    let set: [ModelKey; 3] = match biome {
        Biome::Temperate => ["burgher_0", "burgher_1", "burgher_2"],
        Biome::Tropical => ["villa_0", "villa_1", "villa_2"],
        Biome::Arid => ["mansion_0", "mansion_1", "mansion_2"],
        Biome::Arctic => ["merchant_0", "merchant_1", "merchant_2"],
    };
    set[v % 3]
}

/// Where a Burghers' house smokes from (none in the heat).
pub fn house_chimney(biome: Biome) -> Option<Vec3> {
    match biome {
        Biome::Temperate => Some(v3(3.5, 14.1, 0.4)),
        Biome::Arctic => Some(v3(2.6, 13.4, 0.2)),
        Biome::Tropical | Biome::Arid => None,
    }
}

/// A Burghers' house and what stands on its lot.
pub fn house_look(biome: Biome, seed: u64, out: &mut Vec<Piece>) {
    let v = crate::looks::variant(seed);
    let r = |k: u64| h01(seed, 500 + k);
    let t = 0.94 + 0.12 * h01(seed, 2);
    out.push(Piece::at(house_model(biome, v), Vec3::ZERO).tint([
        t,
        t * (0.97 + 0.06 * h01(seed, 3)),
        t,
    ]));
    let leaf = |k: u64| {
        let g = 0.85 + 0.3 * r(k);
        [g * (0.9 + 0.15 * r(k + 1)), g, g * 0.88]
    };
    match biome {
        Biome::Temperate => {
            out.push(Piece::at("tub_shrub", v3(-3.9, 0.0, 5.0)).tint(leaf(0)));
            out.push(Piece::at("tub_shrub", v3(-1.3, 0.0, 5.0)).tint(leaf(2)));
            if r(4) > 0.4 {
                out.push(Piece::at("lamp_post", v3(4.9, 0.0, 5.2)));
            }
            if r(5) > 0.5 {
                out.push(Piece::at("bench", v3(2.4, 0.0, 5.0)));
            }
            if r(6) > 0.5 {
                out.push(
                    Piece::at("bush", v3(-5.0, 0.0, -4.6))
                        .yaw(r(7) * TAU)
                        .tint(leaf(8)),
                );
            }
        }
        Biome::Tropical => {
            for (k, (x, z)) in [(-5.2, 3.8), (5.2, 3.6), (-5.1, -4.6)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                if r(k) < 0.75 {
                    out.push(
                        Piece::at("banana", v3(x, 0.0, z))
                            .yaw(r(4 + k) * TAU)
                            .scale(0.9 + 0.5 * r(8 + k))
                            .tint(leaf(12 + k * 2)),
                    );
                }
            }
            out.push(Piece::at("tub_shrub", v3(-1.9, 0.0, 5.4)).tint(leaf(30)));
            out.push(Piece::at("tub_shrub", v3(1.9, 0.0, 5.4)).tint(leaf(32)));
        }
        Biome::Arid => {
            for k in 0..3u64 {
                if r(k) < 0.7 {
                    let x = [-4.9, 4.8, 3.4][k as usize];
                    let z = [4.4, 4.6, 5.0][k as usize];
                    out.push(
                        Piece::at("jar", v3(x, 0.0, z))
                            .scale(0.9 + 0.5 * r(8 + k))
                            .tint([1.0, 0.9 + 0.1 * r(12 + k), 0.85 + 0.15 * r(16 + k)]),
                    );
                }
            }
            out.push(Piece::at("tub_shrub", v3(-2.2, 0.0, 4.6)).tint([0.95, 0.95, 0.7]));
        }
        Biome::Arctic => {
            for (k, (x, z)) in [(-4.8, 4.0), (4.9, -3.0), (-4.6, -4.4), (4.6, 4.6)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                if r(k) < 0.65 {
                    out.push(
                        Piece::at("snow_drift", v3(x, 0.0, z))
                            .yaw(r(4 + k) * TAU)
                            .scale(0.7 + 0.7 * r(8 + k)),
                    );
                }
            }
            if r(20) > 0.5 {
                out.push(Piece::at("lamp_post", v3(-4.9, 0.0, 5.2)));
            }
        }
    }
}

/// The pieces of the manufactories, the school and the tavern (false: not one of them).
pub fn look(kind: Kind, seed: u64, out: &mut Vec<Piece>) -> bool {
    let r = |k: u64| h01(seed, 700 + k);
    match kind {
        Kind::Glassworks => {
            out.push(Piece::at("glassworks", Vec3::ZERO));
            out.push(Piece::at("sand_heap", v3(5.4, 0.0, 5.2)).yaw(r(0) * TAU));
            out.push(
                Piece::at("sand_heap", v3(3.2, 0.0, 6.2))
                    .yaw(r(1) * TAU)
                    .scale(0.7),
            );
            out.push(Piece::at("glass_rack", v3(-5.6, 0.0, 4.6)).yaw(0.2));
            out.push(Piece::at("glass_rack", v3(-3.4, 0.0, 5.8)).yaw(-0.3));
            for i in 0..3u64 {
                out.push(
                    Piece::at(
                        "crate",
                        v3(-1.2 + i as f32 * 1.2, 0.0, 5.6 + r(4 + i) * 0.6),
                    )
                    .yaw(r(8 + i) * 0.5),
                );
            }
        }
        Kind::Saltery => {
            out.push(Piece::at("saltery", Vec3::ZERO));
            out.push(Piece::at("fish_rack", v3(3.9, 0.0, 1.0)).yaw(PI * 0.5));
            out.push(Piece::at("fish_rack", v3(3.9, 0.0, -3.2)).yaw(PI * 0.5));
            out.push(Piece::at("salt_heap", v3(3.6, 0.0, 5.6)).scale(0.8));
            for i in 0..6u64 {
                out.push(Piece::at(
                    "barrel",
                    v3(
                        -4.4 + (i % 3) as f32 * 0.85,
                        0.0,
                        6.6 - (i / 3) as f32 * 0.85,
                    ),
                ));
            }
        }
        Kind::Confectioner => {
            out.push(Piece::at("confectioner", Vec3::ZERO));
            out.push(Piece::at("tub_shrub", v3(-4.4, 0.0, 4.4)).tint([1.0, 0.9, 0.95]));
            out.push(Piece::at("sacks", v3(4.6, 0.0, 1.0)).yaw(1.2).scale(0.8));
            out.push(Piece::at("jar", v3(4.7, 0.0, 3.4)).scale(0.9));
            out.push(Piece::at("bench", v3(-1.4, 0.0, 4.9)));
        }
        Kind::Furrier => {
            out.push(Piece::at("furrier", Vec3::ZERO));
            for (i, x) in [-3.8, -0.6, 2.6].into_iter().enumerate() {
                out.push(
                    Piece::at("pelt_frame", v3(x, 0.0, 5.6 + r(i as u64) * 0.8))
                        .yaw(r(4 + i as u64) * 0.5 - 0.25),
                );
            }
            out.push(Piece::at("sacks", v3(4.8, 0.0, 3.2)).yaw(0.4));
        }
        Kind::LampMaker => {
            out.push(Piece::at("lamp_maker", Vec3::ZERO));
            out.push(Piece::at("lamp_post", v3(-4.9, 0.0, 4.9)));
            out.push(Piece::at("lamp_post", v3(4.9, 0.0, 4.9)));
            out.push(Piece::at("crate", v3(4.4, 0.0, 1.2)).yaw(0.3));
            out.push(Piece::at("crate", v3(4.6, 0.0, 2.6)).yaw(-0.2));
        }
        Kind::School => {
            out.push(Piece::at("school", Vec3::ZERO));
            for x in [-6.0, -2.0, 2.0] {
                out.push(Piece::at("fence_4", v3(x, 0.0, 7.4)));
            }
            out.push(Piece::at("bench", v3(-4.6, 0.0, 3.4)));
            out.push(Piece::at("bench", v3(4.6, 0.0, 3.4)));
            out.push(Piece::at("bush", v3(-6.4, 0.0, 5.4)).tint([0.9, 1.0, 0.85]));
        }
        Kind::Tavern => {
            out.push(Piece::at("tavern", Vec3::ZERO));
            out.push(Piece::at("bench", v3(-3.4, 0.0, 4.4)));
            out.push(Piece::at("bench", v3(-0.4, 0.0, 4.4)));
            for i in 0..4u64 {
                out.push(Piece::at(
                    "barrel",
                    v3(
                        5.6 + (i % 2) as f32 * 0.85,
                        0.0,
                        2.2 + (i / 2) as f32 * 0.85,
                    ),
                ));
            }
            out.push(Piece::at("hand_cart", v3(6.2, 0.0, -2.6)).yaw(1.9));
            out.push(Piece::at("lamp_post", v3(-6.8, 0.0, 4.6)));
        }
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------------------ helpers

/// A quad seen from the side `out` points to.
pub(crate) fn face(p: &mut Parts, m: Mat, q: [Vec3; 4], col: Vec4, out: Vec3) {
    if (q[1] - q[0]).cross(q[3] - q[0]).dot(out) >= 0.0 {
        p.quad(m, q[0], q[1], q[2], q[3], col);
    } else {
        p.quad(m, q[3], q[2], q[1], q[0], col);
    }
}

/// A mansard roof: four steep slopes from the eaves of an `l × w` rectangle at `c` up `h` to
/// a deck `inset` smaller all round.
pub(crate) fn mansard(p: &mut Parts, c: Vec3, l: f32, w: f32, h: f32, inset: f32, col: Vec4) {
    let (hl, hw) = (l * 0.5 + 0.25, w * 0.5 + 0.25);
    let (tl, tw) = (l * 0.5 - inset, w * 0.5 - inset);
    let lo = |x: f32, z: f32| c + v3(x, 0.0, z);
    let hi = |x: f32, z: f32| c + v3(x, h, z);
    let m = Mat::Slate;
    face(
        p,
        m,
        [lo(-hl, hw), lo(hl, hw), hi(tl, tw), hi(-tl, tw)],
        col,
        Vec3::Z,
    );
    face(
        p,
        m,
        [lo(hl, -hw), lo(-hl, -hw), hi(-tl, -tw), hi(tl, -tw)],
        col,
        Vec3::NEG_Z,
    );
    face(
        p,
        m,
        [lo(hl, hw), lo(hl, -hw), hi(tl, -tw), hi(tl, tw)],
        col,
        Vec3::X,
    );
    face(
        p,
        m,
        [lo(-hl, -hw), lo(-hl, hw), hi(-tl, tw), hi(-tl, -tw)],
        col,
        Vec3::NEG_X,
    );
    up_quad(
        p,
        Mat::Slate,
        (c.x - tl, c.x + tl),
        (c.z - tw, c.z + tw),
        c.y + h,
        shade(col, 0.8),
    );
}

/// A dormer looking out along +Z: its front face at `at` (bottom middle), `w` wide, the
/// window `h` high, running `depth` back into the roof.
pub(crate) fn dormer(p: &mut Parts, at: Vec3, w: f32, h: f32, depth: f32, roof: Mat) {
    p.cuboid(
        Mat::Paint,
        at + v3(-w * 0.5, 0.0, -depth),
        at + v3(w * 0.5, h, 0.0),
        WHITE,
        false,
    );
    p.windows(
        at + v3(0.0, h * 0.52, 0.02),
        Vec3::Z,
        1,
        w * 0.6,
        h * 0.66,
        0.0,
        WHITE,
    );
    p.gable_roof(
        roof,
        Mat::Paint,
        at + v3(0.0, h, -depth * 0.5),
        depth,
        w,
        w * 0.4,
        0.12,
        true,
        Vec4::ONE,
        WHITE,
    );
}

/// A chimney from `base` up to `top`, with a cap and two pots.
pub(crate) fn chimney(p: &mut Parts, m: Mat, base: Vec3, top: f32, col: Vec4) {
    p.cuboid(
        m,
        base + v3(-0.42, 0.0, -0.36),
        v3(base.x + 0.42, top, base.z + 0.36),
        col,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(base.x - 0.5, top, base.z - 0.44),
        v3(base.x + 0.5, top + 0.14, base.z + 0.44),
        WHITE,
        true,
    );
    for dx in [-0.2, 0.2] {
        p.cylinder(
            Mat::Clay,
            v3(base.x + dx, top + 0.14, base.z),
            0.13,
            0.1,
            0.4,
            8,
            Vec4::ONE,
            false,
        );
    }
}

/// A signboard hanging from an iron bracket on a wall facing +Z at `at` (the bracket's
/// root), `w × h`, seen from both sides along the street.
pub(crate) fn hanging_sign(p: &mut Parts, at: Vec3, w: f32, h: f32, col: Vec4, mark: Vec4) {
    let out = at + v3(0.0, 0.0, w + 0.35);
    p.beam(Mat::Iron, at, out, 0.06, Vec4::ONE);
    p.beam(
        Mat::Iron,
        at + v3(0.0, -0.5, 0.0),
        at + v3(0.0, 0.0, 0.6),
        0.04,
        Vec4::ONE,
    );
    let c = at + v3(0.0, -0.12 - h * 0.5, 0.25 + w * 0.5);
    p.cuboid(
        Mat::Paint,
        c - v3(0.04, h * 0.5, w * 0.5),
        c + v3(0.04, h * 0.5, w * 0.5),
        col,
        true,
    );
    for s in [-1.0f32, 1.0] {
        face(
            p,
            Mat::Paint,
            [
                c + v3(s * 0.05, -h * 0.25, -w * 0.25),
                c + v3(s * 0.05, -h * 0.25, w * 0.25),
                c + v3(s * 0.05, h * 0.25, w * 0.25),
                c + v3(s * 0.05, h * 0.25, -w * 0.25),
            ],
            mark,
            v3(s, 0.0, 0.0),
        );
    }
}

/// A striped awning over a shop front: from the wall at `at` (left end, top edge) `w` along
/// +X, hanging out `d` and down `drop`.
pub(crate) fn awning(p: &mut Parts, at: Vec3, w: f32, d: f32, drop: f32, cols: [Vec4; 2]) {
    let n = (w / 0.55).round().max(2.0) as usize;
    let sw = w / n as f32;
    for i in 0..n {
        let x0 = at.x + i as f32 * sw;
        let a = v3(x0, at.y - drop, at.z + d);
        let b = v3(x0 + sw, at.y - drop, at.z + d);
        let c = v3(x0 + sw, at.y, at.z);
        let e = v3(x0, at.y, at.z);
        sheet(p, Mat::Canvas, a, b, c, e, cols[i % 2]);
        // The valance.
        sheet(
            p,
            Mat::Canvas,
            a - Vec3::Y * 0.28,
            b - Vec3::Y * 0.28,
            b,
            a,
            cols[i % 2],
        );
    }
    for x in [at.x, at.x + w] {
        p.beam(
            Mat::Iron,
            v3(x, at.y - drop, at.z + d),
            v3(x, at.y - drop - 0.9, at.z),
            0.04,
            Vec4::ONE,
        );
    }
}

/// Timber framing on a wall facing +Z or −Z: posts about every 1.1 m between `x0` and `x1`,
/// a sill and a head rail, and a brace at each end.
fn framing_z(p: &mut Parts, x0: f32, x1: f32, y0: f32, y1: f32, z: f32, col: Vec4) {
    let n = ((x1 - x0) / 1.1).round().max(1.0) as usize;
    for i in 0..=n {
        let x = x0 + (x1 - x0) * i as f32 / n as f32;
        p.beam(Mat::Timber, v3(x, y0, z), v3(x, y1, z), 0.16, col);
    }
    for y in [y0, y1] {
        p.beam(Mat::Timber, v3(x0, y, z), v3(x1, y, z), 0.18, col);
    }
    let step = (x1 - x0) / n as f32;
    p.beam(Mat::Timber, v3(x0, y0, z), v3(x0 + step, y1, z), 0.13, col);
    p.beam(Mat::Timber, v3(x1, y0, z), v3(x1 - step, y1, z), 0.13, col);
}

/// The same on a wall facing ±X.
fn framing_x(p: &mut Parts, z0: f32, z1: f32, y0: f32, y1: f32, x: f32, col: Vec4) {
    let n = ((z1 - z0) / 1.1).round().max(1.0) as usize;
    for i in 0..=n {
        let z = z0 + (z1 - z0) * i as f32 / n as f32;
        p.beam(Mat::Timber, v3(x, y0, z), v3(x, y1, z), 0.16, col);
    }
    for y in [y0, y1] {
        p.beam(Mat::Timber, v3(x, y, z0), v3(x, y, z1), 0.18, col);
    }
}

/// Steps up to a door on a wall facing +Z: `n` treads `w` wide from the ground at
/// `z0 + n·0.32` back to `z0`, rising to `top`.
pub(crate) fn steps(p: &mut Parts, x: f32, z0: f32, w: f32, n: usize, top: f32, col: Vec4) {
    for k in 0..n {
        let y = top * (n - k) as f32 / n as f32;
        p.cuboid(
            Mat::Plaster,
            v3(x - w * 0.5, -0.4, z0 + k as f32 * 0.32),
            v3(x + w * 0.5, y, z0 + (k + 1) as f32 * 0.32),
            col,
            false,
        );
    }
}

/// Windows on all four walls of a box `l × w` centred on `c`, at height `y`: `nf` on the
/// front and back, `ns` on each side.
#[allow(clippy::too_many_arguments)]
pub(crate) fn window_band(
    p: &mut Parts,
    c: Vec3,
    l: f32,
    w: f32,
    y: f32,
    nf: u32,
    ns: u32,
    size: (f32, f32),
    frame: Vec4,
) {
    let gap_f = (l - 1.2) / nf as f32 - size.0;
    let gap_s = (w - 1.2) / ns as f32 - size.0;
    for s in [-1.0f32, 1.0] {
        p.windows(
            v3(c.x, y, c.z + s * (w * 0.5 + 0.02)),
            v3(0.0, 0.0, s),
            nf,
            size.0,
            size.1,
            gap_f,
            frame,
        );
        p.windows(
            v3(c.x + s * (l * 0.5 + 0.02), y, c.z),
            v3(s, 0.0, 0.0),
            ns,
            size.0,
            size.1,
            gap_s,
            frame,
        );
    }
}

// ------------------------------------------------------------------------ Burghers' houses

/// The home isles: a three-storey town house on a rusticated ground floor, under a slate
/// mansard with dormers. 0: brick with an iron balcony; 1: cream render over a shop front
/// with an awning; 2: grey-blue render with a bay window through two floors.
fn burgher(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (9.0, 7.6);
    let c = v3(0.0, 0.0, 0.4);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (g, wall) = (3.9, 10.2);
    let stone = hexc(0xd8d2c4);
    let (mat, col) = match v {
        0 => (Mat::Brick, Vec4::ONE),
        1 => (Mat::Plaster, hexc(0xeadfc2)),
        _ => (Mat::Plaster, hexc(0xc3ced2)),
    };
    // Ground floor of dressed stone, on a plinth.
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.15, -3.0, zb - 0.15),
        v3(l * 0.5 + 0.15, 0.5, zf + 0.15),
        shade(stone, 0.85),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, 0.5, zb),
        v3(l * 0.5, g, zf),
        stone,
        false,
    );
    // Courses cut into the stone: thin dark bands.
    for k in 1..5 {
        let y = 0.5 + k as f32 * 0.68;
        p.cuboid(
            Mat::Plaster,
            v3(-l * 0.5 - 0.02, y, zb - 0.02),
            v3(l * 0.5 + 0.02, y + 0.05, zf + 0.02),
            shade(stone, 0.6),
            true,
        );
    }
    // Two floors above.
    p.cuboid(mat, v3(-l * 0.5, g, zb), v3(l * 0.5, wall, zf), col, false);
    for (y, d, h) in [(g, 0.12, 0.22), (wall - 0.3, 0.28, 0.3)] {
        p.cuboid(
            Mat::Plaster,
            v3(-l * 0.5 - d, y, zb - d),
            v3(l * 0.5 + d, y + h, zf + d),
            WHITE,
            true,
        );
    }
    // Quoins at the corners.
    for sx in [-1.0f32, 1.0] {
        for sz in [-1.0f32, 1.0] {
            for k in 0..8 {
                let y = g + 0.3 + k as f32 * 0.74;
                let d = if k % 2 == 0 { 0.5 } else { 0.32 };
                let (x, z) = (sx * l * 0.5, if sz > 0.0 { zf } else { zb });
                p.cuboid(
                    Mat::Plaster,
                    v3(x - d * 0.5 - 0.03, y, z - d * 0.5 - 0.03),
                    v3(x + d * 0.5 + 0.03, y + 0.42, z + d * 0.5 + 0.03),
                    WHITE,
                    true,
                );
            }
        }
    }
    window_band(&mut p, c, l, w, 5.75, 4, 2, (0.95, 1.8), WHITE);
    window_band(&mut p, c, l, w, 8.55, 4, 2, (0.95, 1.45), WHITE);
    // Ground floor: the door on the left under a stone hood, with steps.
    let dx = -2.9;
    p.door(v3(dx, 0.5, zf), Vec3::Z, 1.25, 2.5, hexc(0x24402e));
    p.cuboid(
        Mat::Plaster,
        v3(dx - 1.0, 3.05, zf),
        v3(dx + 1.0, 3.3, zf + 0.55),
        WHITE,
        true,
    );
    for s in [-0.85f32, 0.85] {
        p.cylinder(
            Mat::Stone,
            v3(dx + s, 0.5, zf + 0.35),
            0.13,
            0.11,
            2.55,
            10,
            WHITE,
            false,
        );
    }
    steps(&mut p, dx, zf, 2.2, 2, 0.5, stone);
    if v == 1 {
        // A shop: one wide glazed front under a striped awning.
        p.cuboid(
            Mat::Paint,
            v3(-1.3, 0.75, zf),
            v3(4.0, 3.35, zf + 0.12),
            hexc(0x2f4a3a),
            false,
        );
        p.windows(v3(1.35, 2.0, zf + 0.13), Vec3::Z, 3, 1.45, 1.9, 0.2, WHITE);
        awning(
            &mut p,
            v3(-1.4, 3.45, zf + 0.1),
            5.5,
            1.5,
            0.7,
            [hexc(0xf2ead8), hexc(0x9a2f25)],
        );
    } else {
        p.windows(v3(1.4, 2.25, zf + 0.02), Vec3::Z, 2, 1.1, 2.0, 1.2, WHITE);
    }
    p.windows(
        v3(c.x, 2.25, zb - 0.02),
        Vec3::NEG_Z,
        3,
        1.0,
        1.7,
        1.6,
        WHITE,
    );
    if v == 0 {
        // An iron balcony before the middle windows of the first floor.
        p.cuboid(
            Mat::Plaster,
            v3(-2.3, 4.55, zf),
            v3(2.3, 4.72, zf + 0.9),
            WHITE,
            true,
        );
        let iron = hexc(0x2a2c30);
        railing(
            &mut p,
            v3(-2.25, 4.72, zf + 0.85),
            v3(2.25, 4.72, zf + 0.85),
            0.95,
            iron,
        );
        for x in [-2.25, 2.25] {
            railing(&mut p, v3(x, 4.72, zf), v3(x, 4.72, zf + 0.85), 0.95, iron);
        }
    }
    if v == 2 {
        // A bay through both upper floors, under its own little roof.
        p.cuboid(
            Mat::Paint,
            v3(-1.5, g + 0.3, zf),
            v3(1.5, wall - 0.5, zf + 0.8),
            WHITE,
            true,
        );
        for y in [5.75, 8.5] {
            p.windows(v3(0.0, y, zf + 0.82), Vec3::Z, 2, 1.0, 1.6, 0.25, WHITE);
        }
        face(
            &mut p,
            Mat::Slate,
            [
                v3(-1.65, wall - 0.5, zf + 0.95),
                v3(1.65, wall - 0.5, zf + 0.95),
                v3(1.5, wall + 0.1, zf),
                v3(-1.5, wall + 0.1, zf),
            ],
            Vec4::ONE,
            Vec3::Z,
        );
    }
    // The mansard, three dormers to the street, a chimney at each end.
    mansard(&mut p, c + v3(0.0, wall, 0.0), l, w, 2.6, 1.5, Vec4::ONE);
    for x in [-2.7, 0.0, 2.7] {
        dormer(
            &mut p,
            v3(x, wall + 0.35, zf - 0.15),
            1.3,
            1.35,
            1.3,
            Mat::Slate,
        );
    }
    for x in [-3.5, 3.5] {
        chimney(
            &mut p,
            Mat::Brick,
            v3(x, wall + 1.0, c.z),
            wall + 3.8,
            Vec4::ONE,
        );
    }
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// Sarrowmere: a planter's town villa. Rendered masonry in a strong colour on a stone
/// basement, verandas on both floors across the front, louvred doors, a tiled hip roof with
/// a look-out on top.
fn villa(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (9.4, 6.4);
    let c = v3(0.0, 0.0, -0.9);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (base, mid, wall) = (1.2, 4.5, 7.8);
    let ver = 2.3;
    let col = [hexc(0xe6906e), hexc(0xe9c468), hexc(0x9fd0c6)][v % 3];
    let louvre = [hexc(0x2f5d4a), hexc(0x2d5578), hexc(0x7a3326)][v % 3];
    let white = hexc(0xf4eee0);
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - 0.2, -3.0, zb - 0.2),
        v3(l * 0.5 + 0.2, base, zf + ver),
        hexc(0xcfc3a6),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, base, zb),
        v3(l * 0.5, wall, zf),
        col,
        false,
    );
    // White bands at the floors and corners.
    for y in [mid - 0.12, wall - 0.3] {
        p.cuboid(
            Mat::Plaster,
            v3(-l * 0.5 - 0.06, y, zb - 0.06),
            v3(l * 0.5 + 0.06, y + 0.3, zf + 0.06),
            white,
            true,
        );
    }
    for sx in [-1.0f32, 1.0] {
        for z in [zb, zf] {
            p.cuboid(
                Mat::Plaster,
                v3(sx * l * 0.5 - 0.2, base, z - 0.2),
                v3(sx * l * 0.5 + 0.2, wall, z + 0.2),
                white,
                false,
            );
        }
    }
    // Louvred doors and windows on both floors.
    for (y0, top) in [(base, 2.5), (mid + 0.18, 2.4)] {
        p.door(v3(0.0, y0, zf), Vec3::Z, 1.3, top, louvre);
        for x in [-3.2, -1.7, 1.7, 3.2] {
            p.windows(
                v3(x, y0 + 1.35, zf + 0.02),
                Vec3::Z,
                1,
                0.85,
                1.9,
                0.0,
                louvre,
            );
        }
        for s in [-1.0f32, 1.0] {
            p.windows(
                v3(s * (l * 0.5 + 0.02), y0 + 1.35, c.z),
                v3(s, 0.0, 0.0),
                2,
                0.85,
                1.9,
                1.6,
                louvre,
            );
        }
        p.windows(
            v3(0.0, y0 + 1.35, zb - 0.02),
            Vec3::NEG_Z,
            4,
            0.85,
            1.9,
            1.2,
            louvre,
        );
    }
    // The verandas: an upper floor of boards, posts through both floors, railings.
    p.cuboid(
        Mat::Planks,
        v3(-l * 0.5, mid - 0.08, zf),
        v3(l * 0.5, mid + 0.08, zf + ver),
        hexc(0xb99a72),
        true,
    );
    let zp = zf + ver - 0.15;
    for k in 0..6 {
        let x = -l * 0.5 + 0.15 + k as f32 * (l - 0.3) / 5.0;
        p.cylinder(
            Mat::Paint,
            v3(x, base, zp),
            0.11,
            0.09,
            wall - base,
            8,
            white,
            false,
        );
        // A little bracket fan under the eaves.
        for s in [-1.0f32, 1.0] {
            if (k == 0 && s < 0.0) || (k == 5 && s > 0.0) {
                continue;
            }
            p.beam(
                Mat::Paint,
                v3(x, wall - 0.7, zp),
                v3(x + s * 0.6, wall - 0.05, zp),
                0.06,
                white,
            );
        }
    }
    for y in [base, mid + 0.08] {
        for (x0, x1) in [(-l * 0.5 + 0.15, -1.0), (1.0, l * 0.5 - 0.15)] {
            // (The ground floor's railing leaves the stair free; upstairs it runs through.)
            railing(&mut p, v3(x0, y, zp), v3(x1, y, zp), 0.9, white);
        }
        for x in [-l * 0.5 + 0.15, l * 0.5 - 0.15] {
            railing(&mut p, v3(x, y, zf), v3(x, y, zp), 0.9, white);
        }
    }
    railing(
        &mut p,
        v3(-1.0, mid + 0.08, zp),
        v3(1.0, mid + 0.08, zp),
        0.9,
        white,
    );
    steps(&mut p, 0.0, zf + ver, 2.2, 4, base, hexc(0xcfc3a6));
    // One roof over house and verandas, and the look-out on its ridge.
    let rc = v3(0.0, wall, (zb + zf + ver) * 0.5);
    p.hip_roof(Mat::RoofTile, rc, l, w + ver, 2.7, 0.55, Vec4::ONE);
    p.cuboid(
        Mat::Paint,
        rc + v3(-1.0, 1.7, -1.0),
        rc + v3(1.0, 3.3, 1.0),
        white,
        false,
    );
    for (n, at) in [
        (Vec3::Z, v3(0.0, 2.55, 1.02)),
        (Vec3::NEG_Z, v3(0.0, 2.55, -1.02)),
        (Vec3::X, v3(1.02, 2.55, 0.0)),
        (Vec3::NEG_X, v3(-1.02, 2.55, 0.0)),
    ] {
        p.windows(rc + at, n, 2, 0.6, 0.9, 0.2, louvre);
    }
    p.hip_roof(
        Mat::RoofTile,
        rc + v3(0.0, 3.3, 0.0),
        2.0,
        2.0,
        0.9,
        0.35,
        Vec4::ONE,
    );
    p.beam(
        Mat::Iron,
        rc + v3(0.0, 4.2, 0.0),
        rc + v3(0.0, 5.3, 0.0),
        0.05,
        Vec4::ONE,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The Amber Reach: a merchant's tower house. Three floors of mud brick, the top one
/// whitewashed, white surrounds to every window, a latticed bay, crenels and corner
/// horns, and a domed room with an awning on the roof.
fn mansion(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (8.6, 7.6);
    let c = v3(0.0, 0.0, -0.2);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (f1, f2, wall) = (3.6, 7.0, 10.4);
    let col = mud(v);
    let white = hexc(0xf4ecd8);
    let dark = hexc(0x4a3222);
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, -3.0, zb),
        v3(l * 0.5, f2, zf),
        col,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, f2, zb),
        v3(l * 0.5, wall, zf),
        white,
        false,
    );
    // A battered foot.
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - 0.25, -3.0, zb - 0.25),
        v3(l * 0.5 + 0.25, 0.9, zf + 0.25),
        shade(col, 0.9),
        true,
    );
    // Beam ends under each floor.
    for y in [f1, f2] {
        beam_ends(&mut p, v3(0.0, y - 0.15, zf), Vec3::Z, l - 1.0);
        beam_ends(&mut p, v3(0.0, y - 0.15, zb), Vec3::NEG_Z, l - 1.0);
        beam_ends(&mut p, v3(l * 0.5, y - 0.15, c.z), Vec3::X, w - 1.0);
        beam_ends(&mut p, v3(-l * 0.5, y - 0.15, c.z), Vec3::NEG_X, w - 1.0);
    }
    // Windows in white surrounds; small and high on the ground floor.
    let surround = |p: &mut Parts, at: Vec3, n: Vec3, ww: f32, wh: f32, tone: Vec4| {
        let along = Vec3::Y.cross(n).normalize();
        let h = along * (ww * 0.5 + 0.18) + Vec3::Y * (wh * 0.5 + 0.2) + n.abs() * 0.05;
        let (a, b) = (at - h, at + h);
        p.cuboid(Mat::Plaster, a.min(b), a.max(b), tone, true);
        p.windows(at + n * 0.07, n, 1, ww, wh, 0.0, dark);
    };
    for (y, wh, tone) in [
        (2.5, 0.8, white),
        (5.3, 1.6, white),
        (8.7, 1.6, shade(col, 1.05)),
    ] {
        for x in [-2.9, 2.9] {
            surround(&mut p, v3(x, y, zf), Vec3::Z, 0.9, wh, tone);
            surround(&mut p, v3(x, y, zb), Vec3::NEG_Z, 0.9, wh, tone);
        }
        for z in [c.z - 1.9, c.z + 1.9] {
            surround(&mut p, v3(l * 0.5, y, z), Vec3::X, 0.9, wh, tone);
            surround(&mut p, v3(-l * 0.5, y, z), Vec3::NEG_X, 0.9, wh, tone);
        }
    }
    surround(
        &mut p,
        v3(0.0, 8.7, zf),
        Vec3::Z,
        0.9,
        1.6,
        shade(col, 1.05),
    );
    // The carved door in a tall white frame.
    p.cuboid(
        Mat::Plaster,
        v3(-1.25, 0.0, zf),
        v3(1.25, 3.3, zf + 0.12),
        white,
        false,
    );
    p.door(v3(0.0, 0.2, zf + 0.12), Vec3::Z, 1.5, 2.6, dark);
    steps(&mut p, 0.0, zf + 0.12, 2.4, 1, 0.2, shade(col, 0.9));
    // The latticed bay over the door.
    p.cuboid(
        Mat::Timber,
        v3(-1.3, 4.3, zf),
        v3(1.3, 6.5, zf + 0.75),
        dark,
        true,
    );
    p.windows(v3(0.0, 5.45, zf + 0.77), Vec3::Z, 3, 0.55, 1.3, 0.12, dark);
    for x in [-1.0, 1.0] {
        p.beam(Mat::Timber, v3(x, 3.7, zf), v3(x, 4.3, zf + 0.6), 0.1, dark);
    }
    face(
        &mut p,
        Mat::Timber,
        [
            v3(-1.4, 6.5, zf + 0.85),
            v3(1.4, 6.5, zf + 0.85),
            v3(1.3, 6.85, zf),
            v3(-1.3, 6.85, zf),
        ],
        shade(dark, 1.2),
        Vec3::Z,
    );
    // The roof: crenels, a horn at each corner, a domed room and an awning.
    up_quad(
        &mut p,
        Mat::Plaster,
        (-l * 0.5, l * 0.5),
        (zb, zf),
        wall,
        shade(white, 0.92),
    );
    parapet(&mut p, v3(0.0, wall, c.z), l, w, 0.7, white);
    for sx in [-1.0f32, 1.0] {
        for z in [zb, zf] {
            p.cylinder(
                Mat::Plaster,
                v3(
                    sx * (l * 0.5 - 0.2),
                    wall + 0.6,
                    z - (z - c.z).signum() * 0.2,
                ),
                0.3,
                0.06,
                1.1,
                8,
                white,
                false,
            );
        }
    }
    let rc = v3(-1.9, wall, c.z - 1.6);
    p.cuboid(
        Mat::Plaster,
        rc + v3(-1.5, 0.0, -1.5),
        rc + v3(1.5, 2.3, 1.5),
        white,
        false,
    );
    p.door(rc + v3(0.0, 0.0, 1.5), Vec3::Z, 0.9, 1.9, dark);
    dome(
        &mut p,
        Mat::Plaster,
        rc + v3(0.0, 2.3, 0.0),
        1.45,
        1.3,
        white,
    );
    let cloth = [hexc(0xb5532f), hexc(0x2f6a8a), hexc(0xd9a441)][v % 3];
    let (ax, az) = (2.0, c.z + 1.4);
    for (dx, dz) in [(-1.5, -1.4), (1.5, -1.4), (-1.5, 1.4), (1.5, 1.4)] {
        p.beam(
            Mat::Timber,
            v3(ax + dx, wall, az + dz),
            v3(ax + dx, wall + 2.2, az + dz),
            0.08,
            dark,
        );
    }
    sheet(
        &mut p,
        Mat::Canvas,
        v3(ax - 1.7, wall + 2.2, az + 1.6),
        v3(ax + 1.7, wall + 2.2, az + 1.6),
        v3(ax + 1.7, wall + 2.3, az - 1.6),
        v3(ax - 1.7, wall + 2.3, az - 1.6),
        cloth,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// Frostholm: a merchant's house. Three boarded floors in one strong colour with white
/// corner boards and frames, a cross gable over a glazed porch, snow on a steep slate roof.
fn merchant(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (9.0, 7.4);
    let c = v3(0.0, 0.0, 0.2);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (base, wall) = (0.6, 9.0);
    let col = [hexc(0xc9962e), hexc(0x8f2f25), hexc(0x4f6b78)][v % 3];
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.1, -3.0, zb - 0.1),
        v3(l * 0.5 + 0.1, base, zf + 0.1),
        hexc(0x9a9a98),
        false,
    );
    p.cuboid(
        Mat::Planks,
        v3(-l * 0.5, base, zb),
        v3(l * 0.5, wall, zf),
        col,
        false,
    );
    // White corner boards and a board at each floor.
    for sx in [-1.0f32, 1.0] {
        for z in [zb, zf] {
            p.cuboid(
                Mat::Paint,
                v3(sx * l * 0.5 - 0.14, base, z - 0.14),
                v3(sx * l * 0.5 + 0.14, wall, z + 0.14),
                WHITE,
                false,
            );
        }
    }
    for y in [3.4, 6.2] {
        p.cuboid(
            Mat::Paint,
            v3(-l * 0.5 - 0.04, y, zb - 0.04),
            v3(l * 0.5 + 0.04, y + 0.16, zf + 0.04),
            WHITE,
            true,
        );
    }
    window_band(&mut p, c, l, w, 4.95, 5, 3, (0.85, 1.4), WHITE);
    window_band(&mut p, c, l, w, 7.7, 5, 3, (0.85, 1.3), WHITE);
    p.windows(v3(-2.9, 2.2, zf + 0.02), Vec3::Z, 2, 0.9, 1.5, 0.6, WHITE);
    p.windows(v3(2.9, 2.2, zf + 0.02), Vec3::Z, 2, 0.9, 1.5, 0.6, WHITE);
    p.windows(
        v3(0.0, 2.2, zb - 0.02),
        Vec3::NEG_Z,
        4,
        0.9,
        1.5,
        1.0,
        WHITE,
    );
    for s in [-1.0f32, 1.0] {
        p.windows(
            v3(s * (l * 0.5 + 0.02), 2.2, c.z),
            v3(s, 0.0, 0.0),
            2,
            0.9,
            1.5,
            1.6,
            WHITE,
        );
    }
    // The roof, with snow down most of it.
    let rc = v3(0.0, wall, c.z);
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        rc,
        l,
        w,
        3.4,
        0.5,
        false,
        Vec4::ONE,
        col,
    );
    roof_snow(&mut p, rc, l, w, 3.4, 0.5, false, 0.72);
    // A cross gable to the street.
    p.cuboid(
        Mat::Planks,
        v3(-1.7, wall, zf - 2.2),
        v3(1.7, wall + 1.5, zf),
        col,
        false,
    );
    p.windows(
        v3(0.0, wall + 0.8, zf + 0.02),
        Vec3::Z,
        2,
        0.75,
        1.0,
        0.3,
        WHITE,
    );
    let gc = v3(0.0, wall + 1.5, zf - 1.7);
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        gc,
        4.0,
        3.4,
        1.5,
        0.4,
        true,
        Vec4::ONE,
        col,
    );
    roof_snow(&mut p, gc, 4.0, 3.4, 1.5, 0.4, true, 0.8);
    // The glazed porch and its steps.
    let pz = zf + 1.5;
    p.cuboid(
        Mat::Paint,
        v3(-1.3, base, zf),
        v3(1.3, 3.2, pz),
        WHITE,
        false,
    );
    p.door(v3(0.0, base, pz), Vec3::Z, 1.0, 2.1, shade(col, 0.6));
    for s in [-1.0f32, 1.0] {
        p.windows(
            v3(s * 1.32, 2.0, zf + 0.75),
            v3(s, 0.0, 0.0),
            1,
            0.9,
            1.3,
            0.0,
            WHITE,
        );
        p.windows(
            v3(s * 0.9, 2.0, pz + 0.02),
            Vec3::Z,
            1,
            0.42,
            1.3,
            0.0,
            WHITE,
        );
    }
    let pc = v3(0.0, 3.2, zf + 0.75);
    p.gable_roof(
        Mat::Slate,
        Mat::Paint,
        pc,
        1.7,
        2.8,
        0.9,
        0.25,
        true,
        Vec4::ONE,
        WHITE,
    );
    roof_snow(&mut p, pc, 1.7, 2.8, 0.9, 0.25, true, 0.9);
    steps(&mut p, 0.0, pz, 1.8, 2, base, hexc(0x9a9a98));
    for x in [-2.6, 2.6] {
        chimney(
            &mut p,
            Mat::Brick,
            v3(x, wall + 2.0, c.z),
            wall + 4.3,
            Vec4::ONE,
        );
        p.cuboid(
            Mat::Snow,
            v3(x - 0.5, wall + 4.44, c.z - 0.44),
            v3(x + 0.5, wall + 4.56, c.z + 0.44),
            SNOW,
            true,
        );
    }
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

// --------------------------------------------------------------------------- manufactories

/// A brick hall beside the glass cone: the tall brick kiln whose mouth smokes.
fn glassworks() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (8.4, 8.0, 5.0);
    let c = v3(-3.2, 0.0, -2.6);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    p.cuboid(
        Mat::Stone,
        v3(c.x - l * 0.5 - 0.15, -3.0, zb - 0.15),
        v3(c.x + l * 0.5 + 0.15, 0.4, zf + 0.15),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Brick,
        v3(c.x - l * 0.5, 0.4, zb),
        v3(c.x + l * 0.5, wall, zf),
        Vec4::ONE,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(c.x, wall, c.z),
        l,
        w,
        3.0,
        0.4,
        false,
        Vec4::ONE,
        Vec4::ONE,
    );
    // Tall windows and a cart door.
    p.windows(
        v3(c.x - 1.6, 2.9, zf + 0.02),
        Vec3::Z,
        2,
        1.0,
        2.6,
        0.9,
        WHITE,
    );
    p.door(v3(c.x + 2.4, 0.4, zf), Vec3::Z, 2.2, 3.0, hexc(0x3a4a3a));
    p.windows(
        v3(c.x - l * 0.5 - 0.02, 2.9, c.z),
        Vec3::NEG_X,
        3,
        1.0,
        2.6,
        1.0,
        WHITE,
    );
    // A louvred vent along the ridge.
    p.cuboid(
        Mat::Planks,
        v3(c.x - 2.6, wall + 2.6, c.z - 0.5),
        v3(c.x + 2.6, wall + 3.4, c.z + 0.5),
        hexc(0x5a4a3a),
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3(c.x, wall + 3.4, c.z),
        5.2,
        1.0,
        0.5,
        0.25,
        false,
        Vec4::ONE,
        hexc(0x5a4a3a),
    );
    // The cone.
    let k = v3(4.0, 0.0, -2.5);
    p.cylinder(
        Mat::Stone,
        k - Vec3::Y * 3.0,
        3.7,
        3.7,
        3.6,
        24,
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    p.cylinder(
        Mat::Brick,
        k + Vec3::Y * 0.6,
        3.5,
        2.5,
        4.4,
        24,
        Vec4::ONE,
        false,
    );
    p.cylinder(
        Mat::Brick,
        k + Vec3::Y * 5.0,
        2.5,
        1.15,
        6.0,
        24,
        shade(Vec4::ONE, 0.92),
        false,
    );
    p.cylinder(
        Mat::Brick,
        k + Vec3::Y * 11.0,
        1.3,
        1.3,
        0.35,
        24,
        shade(Vec4::ONE, 0.7),
        false,
    );
    // Iron bands and the arched mouths at its foot, glowing.
    for y in [1.6, 5.0, 8.2] {
        let r = if y < 5.0 {
            3.5 - (y - 0.6) / 4.4
        } else {
            2.5 - (y - 5.0) * 1.35 / 6.0
        };
        p.cylinder(
            Mat::Iron,
            k + Vec3::Y * y,
            r + 0.04,
            r + 0.02,
            0.12,
            24,
            Vec4::ONE,
            false,
        );
    }
    for a in [0.4f32, 1.5, 2.7] {
        let n = v3(a.cos(), 0.0, a.sin());
        let at = k + n * 3.38 + Vec3::Y * 0.6;
        let along = Vec3::Y.cross(n).normalize();
        face(
            &mut p,
            Mat::Lamp,
            [
                at - along * 0.55,
                at + along * 0.55,
                at + along * 0.55 + Vec3::Y * 1.5 - n * 0.3,
                at - along * 0.55 + Vec3::Y * 1.5 - n * 0.3,
            ],
            hexc(0xffa040),
            n,
        );
    }
    // A covered way between hall and cone.
    p.cuboid(
        Mat::Brick,
        v3(c.x + l * 0.5, 0.4, c.z - 1.2),
        v3(k.x - 2.9, 3.0, c.z + 1.2),
        Vec4::ONE,
        false,
    );
    face(
        &mut p,
        Mat::Slate,
        [
            v3(c.x + l * 0.5, 3.0, c.z + 1.35),
            v3(k.x - 2.8, 3.0, c.z + 1.35),
            v3(k.x - 2.8, 3.5, c.z),
            v3(c.x + l * 0.5, 3.5, c.z),
        ],
        Vec4::ONE,
        Vec3::Z,
    );
    face(
        &mut p,
        Mat::Slate,
        [
            v3(k.x - 2.8, 3.0, c.z - 1.35),
            v3(c.x + l * 0.5, 3.0, c.z - 1.35),
            v3(c.x + l * 0.5, 3.5, c.z),
            v3(k.x - 2.8, 3.5, c.z),
        ],
        Vec4::ONE,
        Vec3::NEG_Z,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// A heap of pale sand.
fn sand_heap() -> Parts {
    let mut p = Parts::default();
    let sand = hexc(0xe6d6a4);
    p.cylinder(
        Mat::Snow,
        v3(0.0, -0.3, 0.0),
        1.9,
        1.1,
        0.9,
        12,
        sand,
        false,
    );
    p.cylinder(
        Mat::Snow,
        v3(0.1, 0.6, 0.0),
        1.1,
        0.15,
        0.75,
        12,
        shade(sand, 1.05),
        true,
    );
    // A shovel stuck in it.
    p.beam(
        Mat::Timber,
        v3(0.9, 0.3, 0.5),
        v3(1.2, 1.5, 0.7),
        0.05,
        Vec4::ONE,
    );
    p.finish()
}

/// Panes of window glass leaning on an A-frame.
fn glass_rack() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x7a5a3a);
    for x in [-0.8, 0.8] {
        p.beam(Mat::Timber, v3(x, 0.0, -0.5), v3(x, 1.5, 0.0), 0.08, wood);
        p.beam(Mat::Timber, v3(x, 0.0, 0.5), v3(x, 1.5, 0.0), 0.08, wood);
    }
    p.beam(
        Mat::Timber,
        v3(-0.9, 1.5, 0.0),
        v3(0.9, 1.5, 0.0),
        0.08,
        wood,
    );
    p.beam(
        Mat::Timber,
        v3(-0.9, 0.1, -0.5),
        v3(0.9, 0.1, -0.5),
        0.07,
        wood,
    );
    p.beam(
        Mat::Timber,
        v3(-0.9, 0.1, 0.5),
        v3(0.9, 0.1, 0.5),
        0.07,
        wood,
    );
    for (s, k) in [(1.0f32, 0.0f32), (1.0, 0.1), (-1.0, 0.0), (-1.0, 0.1)] {
        let z0 = s * (0.56 + k);
        let z1 = s * (0.1 + k);
        sheet(
            &mut p,
            Mat::Window,
            v3(-0.7, 0.12, z0),
            v3(0.7, 0.12, z0),
            v3(0.7, 1.38, z1),
            v3(-0.7, 1.38, z1),
            hexc(0x9fd8d0),
        );
    }
    p.finish()
}

/// A long tarred shed with brine vats under a lean-to along its front.
fn saltery() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (7.2, 9.6, 3.3);
    let c = v3(-1.6, 0.0, -2.4);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let tar = hexc(0x6e4e36);
    p.cuboid(
        Mat::Stone,
        v3(c.x - l * 0.5 - 0.1, -3.0, zb - 0.1),
        v3(c.x + l * 0.5 + 0.1, 0.4, zf + 0.1),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Planks,
        v3(c.x - l * 0.5, 0.4, zb),
        v3(c.x + l * 0.5, wall, zf),
        tar,
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Planks,
        v3(c.x, wall, c.z),
        w,
        l,
        2.6,
        0.45,
        true,
        Vec4::ONE,
        tar,
    );
    p.door(v3(c.x, 0.4, zf), Vec3::Z, 2.2, 2.5, hexc(0x3a2a1e));
    p.windows(
        v3(c.x, wall + 0.9, zf + 0.02),
        Vec3::Z,
        1,
        0.9,
        0.9,
        0.0,
        WHITE,
    );
    p.windows(
        v3(c.x - l * 0.5 - 0.02, 2.1, c.z),
        Vec3::NEG_X,
        3,
        0.9,
        1.0,
        1.6,
        WHITE,
    );
    // The lean-to on the right, open to the yard, over three brine vats.
    let (x0, x1) = (c.x + l * 0.5, c.x + l * 0.5 + 3.0);
    for z in [zb + 0.4, c.z, zf - 0.4] {
        p.beam(
            Mat::Timber,
            v3(x1 - 0.15, 0.0, z),
            v3(x1 - 0.15, 2.3, z),
            0.16,
            tar,
        );
    }
    face(
        &mut p,
        Mat::RoofTile,
        [
            v3(x1 + 0.25, 2.25, zf),
            v3(x1 + 0.25, 2.25, zb),
            v3(x0, 3.1, zb),
            v3(x0, 3.1, zf),
        ],
        Vec4::ONE,
        Vec3::X,
    );
    for z in [zb + 1.6, c.z, zf - 1.6] {
        let at = v3(x0 + 1.4, 0.0, z);
        p.cylinder(Mat::Timber, at, 1.0, 1.08, 0.95, 16, hexc(0x8a6a48), false);
        for y in [0.2, 0.7] {
            p.cylinder(
                Mat::Iron,
                at + Vec3::Y * y,
                1.1,
                1.1,
                0.07,
                16,
                Vec4::ONE,
                false,
            );
        }
        p.cylinder(
            Mat::Brine,
            at + Vec3::Y * 0.8,
            0.98,
            0.98,
            0.02,
            16,
            hexc(0xcfe0dc),
            true,
        );
    }
    // A chimney for the smoking loft.
    chimney(
        &mut p,
        Mat::Brick,
        v3(c.x - 1.6, wall + 1.2, zb + 1.4),
        wall + 3.6,
        Vec4::ONE,
    );
    p.ground_occlusion(0.6, 1.6);
    p.finish()
}

/// A pink-and-white shop with a bow window under a striped awning, and the oven's chimney.
fn confectioner() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (8.0, 6.4, 5.6);
    let c = v3(0.0, 0.0, -1.0);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let pink = hexc(0xf0c4c0);
    let cream = hexc(0xf6eede);
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.1, -3.0, zb - 0.1),
        v3(l * 0.5 + 0.1, 0.4, zf + 0.1),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, 0.4, zb),
        v3(l * 0.5, wall, zf),
        pink,
        false,
    );
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.1, 3.05, zb - 0.1),
        v3(l * 0.5 + 0.1, 3.25, zf + 0.1),
        cream,
        true,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        v3(0.0, wall, c.z),
        l,
        w,
        2.8,
        0.4,
        false,
        Vec4::ONE,
        pink,
    );
    // The shop front: a bow window and the door.
    p.cuboid(
        Mat::Paint,
        v3(-3.4, 0.6, zf),
        v3(1.2, 2.95, zf + 0.5),
        cream,
        true,
    );
    p.windows(v3(-1.1, 1.8, zf + 0.52), Vec3::Z, 3, 1.25, 1.7, 0.15, cream);
    p.door(v3(2.5, 0.4, zf), Vec3::Z, 1.1, 2.3, hexc(0x8a3a4a));
    steps(&mut p, 2.5, zf, 1.6, 1, 0.4, hexc(0xcfc8bc));
    awning(
        &mut p,
        v3(-3.6, 3.15, zf + 0.06),
        7.2,
        1.5,
        0.65,
        [cream, hexc(0xe08a9a)],
    );
    window_band(&mut p, c, l, w, 4.4, 3, 2, (0.9, 1.3), cream);
    dormer(
        &mut p,
        v3(0.0, wall + 0.3, zf - 0.2),
        1.3,
        1.2,
        1.2,
        Mat::RoofTile,
    );
    // A cake for a sign: three tiers on an iron bracket.
    let s = v3(-3.9, 4.6, zf);
    p.beam(Mat::Iron, s, s + v3(0.0, 0.0, 1.3), 0.06, Vec4::ONE);
    for (k, (r, col)) in [(0.42, cream), (0.32, hexc(0xe08a9a)), (0.2, cream)]
        .into_iter()
        .enumerate()
    {
        p.cylinder(
            Mat::Paint,
            s + v3(0.0, -0.95 + k as f32 * 0.26, 1.0),
            r,
            r,
            0.26,
            14,
            col,
            true,
        );
    }
    // The oven's stack at the back.
    chimney(
        &mut p,
        Mat::Brick,
        v3(-3.0, wall + 0.6, zb + 0.9),
        wall + 3.4,
        Vec4::ONE,
    );
    p.ground_occlusion(0.6, 1.6);
    p.finish()
}

/// A workshop of brick below and timber framing above, with a pelt on its sign.
fn furrier() -> Parts {
    let mut p = Parts::default();
    let (l, w) = (8.0, 7.6);
    let c = v3(0.0, 0.0, -2.8);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (g, wall) = (3.2, 6.2);
    let jet = 0.35;
    let cream = hexc(0xeadfc6);
    let oak = hexc(0x4a3626);
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.1, -3.0, zb - 0.1),
        v3(l * 0.5 + 0.1, 0.4, zf + 0.1),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5, 0.4, zb),
        v3(l * 0.5, g, zf),
        Vec4::ONE,
        false,
    );
    // The upper floor juts out over the street.
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - jet, g, zb),
        v3(l * 0.5 + jet, wall, zf + jet),
        cream,
        true,
    );
    framing_z(
        &mut p,
        -l * 0.5 - jet,
        l * 0.5 + jet,
        g + 0.1,
        wall - 0.1,
        zf + jet + 0.02,
        oak,
    );
    for s in [-1.0f32, 1.0] {
        framing_x(
            &mut p,
            zb,
            zf + jet,
            g + 0.1,
            wall - 0.1,
            s * (l * 0.5 + jet + 0.02),
            oak,
        );
    }
    for x in [-3.0, -1.0, 1.0, 3.0] {
        p.beam(
            Mat::Timber,
            v3(x, g - 0.5, zf),
            v3(x, g, zf + jet),
            0.14,
            oak,
        );
    }
    p.windows(
        v3(0.0, 4.75, zf + jet + 0.04),
        Vec3::Z,
        3,
        0.95,
        1.3,
        1.25,
        WHITE,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Plaster,
        v3(0.0, wall, c.z + jet * 0.5),
        l + jet * 2.0,
        w + jet,
        3.2,
        0.4,
        false,
        Vec4::ONE,
        cream,
    );
    p.door(v3(-2.4, 0.4, zf), Vec3::Z, 1.2, 2.3, hexc(0x5a2f22));
    p.windows(v3(1.5, 1.9, zf + 0.02), Vec3::Z, 2, 1.4, 1.6, 0.3, WHITE);
    steps(&mut p, -2.4, zf, 1.8, 1, 0.4, hexc(0xcfc8bc));
    hanging_sign(
        &mut p,
        v3(3.9, 4.4, zf + jet),
        0.9,
        0.8,
        hexc(0x2a3a52),
        hexc(0xc9a060),
    );
    chimney(
        &mut p,
        Mat::Brick,
        v3(3.0, wall + 1.0, c.z - 1.0),
        wall + 4.0,
        Vec4::ONE,
    );
    // A drying loft door in the gable with a hoist beam.
    p.beam(
        Mat::Timber,
        v3(-l * 0.5 - jet, wall + 1.9, c.z),
        v3(-l * 0.5 - jet - 1.2, wall + 1.9, c.z),
        0.14,
        oak,
    );
    p.ground_occlusion(0.6, 1.6);
    p.finish()
}

/// A brick workshop with a wall of glass to work by, skylights, and a great brass lantern
/// on its corner.
fn lamp_maker() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (7.6, 6.2, 4.6);
    let c = v3(0.0, 0.0, -1.2);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.1, -3.0, zb - 0.1),
        v3(l * 0.5 + 0.1, 0.4, zf + 0.1),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5, 0.4, zb),
        v3(l * 0.5, wall, zf),
        hexc(0xe8d8c8),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - 0.12, wall - 0.25, zb - 0.12),
        v3(l * 0.5 + 0.12, wall, zf + 0.12),
        WHITE,
        true,
    );
    // The glass wall: three tall lights and the door.
    p.cuboid(
        Mat::Paint,
        v3(-3.3, 0.7, zf),
        v3(1.5, 4.0, zf + 0.1),
        hexc(0x23333c),
        false,
    );
    p.windows(v3(-0.9, 2.4, zf + 0.11), Vec3::Z, 3, 1.3, 2.9, 0.2, WHITE);
    p.door(v3(2.6, 0.4, zf), Vec3::Z, 1.1, 2.3, hexc(0x23333c));
    steps(&mut p, 2.6, zf, 1.6, 1, 0.4, hexc(0xcfc8bc));
    p.windows(
        v3(l * 0.5 + 0.02, 2.6, c.z),
        Vec3::X,
        2,
        1.0,
        1.8,
        1.2,
        WHITE,
    );
    p.windows(
        v3(-l * 0.5 - 0.02, 2.6, c.z),
        Vec3::NEG_X,
        2,
        1.0,
        1.8,
        1.2,
        WHITE,
    );
    // The roof, with skylights down the street side.
    let (rh, over) = (2.4, 0.4);
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(0.0, wall, c.z),
        l,
        w,
        rh,
        over,
        false,
        Vec4::ONE,
        hexc(0xe8d8c8),
    );
    let slope = rh / (w * 0.5);
    for x in [-2.4, 0.0, 2.4] {
        let (z0, z1) = (zf - 0.5, zf - 2.0);
        let y = |z: f32| wall + (zf - z) * slope + 0.06;
        face(
            &mut p,
            Mat::Window,
            [
                v3(x - 0.7, y(z0), z0),
                v3(x + 0.7, y(z0), z0),
                v3(x + 0.7, y(z1), z1),
                v3(x - 0.7, y(z1), z1),
            ],
            hexc(0x9fc8d8),
            Vec3::Y,
        );
    }
    chimney(
        &mut p,
        Mat::Brick,
        v3(-2.8, wall + 0.8, zb + 1.0),
        wall + 3.2,
        Vec4::ONE,
    );
    // The lantern: a brass cage with a glowing light on an iron arm.
    let a = v3(l * 0.5, 3.9, zf);
    p.beam(Mat::Iron, a, a + v3(0.9, 0.3, 0.9), 0.07, Vec4::ONE);
    let lc = a + v3(0.9, -0.55, 0.9);
    p.cuboid(
        Mat::Lamp,
        lc - v3(0.26, 0.0, 0.26),
        lc + v3(0.26, 0.7, 0.26),
        hexc(0xffd890),
        true,
    );
    for (dx, dz) in [(-0.28, -0.28), (0.28, -0.28), (-0.28, 0.28), (0.28, 0.28)] {
        p.beam(
            Mat::Copper,
            lc + v3(dx, -0.05, dz),
            lc + v3(dx, 0.75, dz),
            0.05,
            Vec4::ONE,
        );
    }
    p.cylinder(
        Mat::Copper,
        lc + Vec3::Y * 0.7,
        0.45,
        0.08,
        0.4,
        4,
        Vec4::ONE,
        true,
    );
    p.cuboid(
        Mat::Copper,
        lc - v3(0.32, 0.08, 0.32),
        lc + v3(0.32, 0.0, 0.32),
        Vec4::ONE,
        true,
    );
    p.ground_occlusion(0.6, 1.6);
    p.finish()
}

// ------------------------------------------------------------------------ school and tavern

/// A brick schoolhouse: tall windows either side of a gabled porch, a bell under a little
/// spire on the ridge.
fn school() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (11.0, 7.0, 5.2);
    let c = v3(0.0, 0.0, -3.0);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.15, -3.0, zb - 0.15),
        v3(l * 0.5 + 0.15, 0.6, zf + 0.15),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5, 0.6, zb),
        v3(l * 0.5, wall, zf),
        Vec4::ONE,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - 0.12, wall - 0.25, zb - 0.12),
        v3(l * 0.5 + 0.12, wall, zf + 0.12),
        WHITE,
        true,
    );
    let rh = 3.2;
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(0.0, wall, c.z),
        l,
        w,
        rh,
        0.4,
        false,
        Vec4::ONE,
        Vec4::ONE,
    );
    // Tall windows with stone heads.
    for x in [-4.1, -2.5, 2.5, 4.1] {
        p.windows(v3(x, 3.0, zf + 0.02), Vec3::Z, 1, 1.0, 2.8, 0.0, WHITE);
        p.cuboid(
            Mat::Plaster,
            v3(x - 0.65, 4.45, zf),
            v3(x + 0.65, 4.65, zf + 0.08),
            WHITE,
            true,
        );
    }
    p.windows(
        v3(0.0, 3.0, zb - 0.02),
        Vec3::NEG_Z,
        5,
        1.0,
        2.8,
        0.9,
        WHITE,
    );
    for s in [-1.0f32, 1.0] {
        p.windows(
            v3(s * (l * 0.5 + 0.02), 3.0, c.z),
            v3(s, 0.0, 0.0),
            2,
            1.0,
            2.8,
            1.2,
            WHITE,
        );
        p.windows(
            v3(s * (l * 0.5 + 0.02), wall + 1.2, c.z),
            v3(s, 0.0, 0.0),
            1,
            0.8,
            0.9,
            0.0,
            WHITE,
        );
    }
    // The porch.
    let pz = zf + 1.8;
    p.cuboid(
        Mat::Brick,
        v3(-1.6, 0.6, zf),
        v3(1.6, 4.3, pz),
        Vec4::ONE,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(0.0, 4.3, zf + 0.9),
        2.2,
        3.2,
        1.5,
        0.3,
        true,
        Vec4::ONE,
        Vec4::ONE,
    );
    p.door(v3(0.0, 0.6, pz), Vec3::Z, 1.4, 2.5, hexc(0x24402e));
    p.cuboid(
        Mat::Plaster,
        v3(-1.0, 3.2, pz),
        v3(1.0, 3.45, pz + 0.1),
        WHITE,
        true,
    );
    p.windows(v3(0.0, 4.75, pz + 0.02), Vec3::Z, 1, 0.6, 0.6, 0.0, WHITE);
    steps(&mut p, 0.0, pz, 2.4, 2, 0.6, hexc(0xcfc8bc));
    // The bell turret.
    let t = v3(0.0, wall + rh - 0.3, c.z);
    for (dx, dz) in [(-0.45, -0.45), (0.45, -0.45), (-0.45, 0.45), (0.45, 0.45)] {
        p.beam(
            Mat::Paint,
            t + v3(dx, 0.0, dz),
            t + v3(dx, 1.7, dz),
            0.12,
            WHITE,
        );
    }
    p.cuboid(
        Mat::Paint,
        t + v3(-0.6, 0.0, -0.6),
        t + v3(0.6, 0.45, 0.6),
        WHITE,
        true,
    );
    p.cylinder(
        Mat::Copper,
        t + v3(0.0, 0.85, 0.0),
        0.32,
        0.14,
        0.5,
        10,
        Vec4::ONE,
        true,
    );
    p.cylinder(
        Mat::Slate,
        t + v3(0.0, 1.7, 0.0),
        0.95,
        0.02,
        1.9,
        4,
        Vec4::ONE,
        true,
    );
    chimney(
        &mut p,
        Mat::Brick,
        v3(4.2, wall + 1.4, c.z),
        wall + 4.4,
        Vec4::ONE,
    );
    // A flagpole in the yard.
    let f = v3(5.6, 0.0, 4.6);
    p.cylinder(Mat::Paint, f, 0.07, 0.04, 7.0, 8, WHITE, true);
    sheet(
        &mut p,
        Mat::Canvas,
        f + v3(0.0, 5.7, 0.0),
        f + v3(1.5, 5.9, 0.2),
        f + v3(1.5, 6.7, 0.2),
        f + v3(0.0, 6.9, 0.0),
        hexc(0x2d5578),
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// An inn: a stone ground floor, a timber-framed floor jutting out above it, a steep tiled
/// roof with dormers, a great chimney and a painted sign.
fn tavern() -> Parts {
    let mut p = Parts::default();
    let (l, w) = (10.4, 6.4);
    let c = v3(-1.6, 0.0, -1.4);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (x0, x1) = (c.x - l * 0.5, c.x + l * 0.5);
    let (g, wall) = (3.1, 5.9);
    let jet = 0.4;
    let cream = hexc(0xf0e4c4);
    let oak = hexc(0x3f2e20);
    p.cuboid(
        Mat::Plaster,
        v3(x0, -3.0, zb),
        v3(x1, g, zf),
        hexc(0xd6cfbf),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(x0 - jet, g, zb - jet),
        v3(x1 + jet, wall, zf + jet),
        cream,
        true,
    );
    for z in [zf + jet + 0.02, zb - jet - 0.02] {
        framing_z(&mut p, x0 - jet, x1 + jet, g + 0.1, wall - 0.1, z, oak);
    }
    for x in [x0 - jet - 0.02, x1 + jet + 0.02] {
        framing_x(&mut p, zb - jet, zf + jet, g + 0.1, wall - 0.1, x, oak);
    }
    for k in 0..7 {
        let x = x0 + 0.4 + k as f32 * (l - 0.8) / 6.0;
        p.beam(
            Mat::Timber,
            v3(x, g - 0.55, zf),
            v3(x, g, zf + jet),
            0.15,
            oak,
        );
    }
    p.windows(
        v3(c.x, 4.5, zf + jet + 0.04),
        Vec3::Z,
        4,
        0.95,
        1.2,
        1.35,
        WHITE,
    );
    p.windows(
        v3(c.x, 4.5, zb - jet - 0.04),
        Vec3::NEG_Z,
        4,
        0.95,
        1.2,
        1.35,
        WHITE,
    );
    // The ground floor: a wide door, leaded windows.
    p.door(v3(c.x - 0.6, 0.0, zf), Vec3::Z, 1.5, 2.3, hexc(0x4a2a1c));
    p.windows(
        v3(c.x - 3.4, 1.7, zf + 0.02),
        Vec3::Z,
        2,
        1.1,
        1.3,
        0.3,
        oak,
    );
    p.windows(
        v3(c.x + 2.9, 1.7, zf + 0.02),
        Vec3::Z,
        2,
        1.1,
        1.3,
        0.3,
        oak,
    );
    p.cuboid(
        Mat::Lamp,
        v3(c.x + 0.5, 2.1, zf + 0.02),
        v3(c.x + 0.8, 2.55, zf + 0.3),
        hexc(0xffd890),
        true,
    );
    // The roof.
    let rc = v3(c.x, wall, c.z);
    let (rl, rw, rh) = (l + jet * 2.0, w + jet * 2.0, 3.8);
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        rc,
        rl,
        rw,
        rh,
        0.45,
        false,
        Vec4::ONE,
        cream,
    );
    for x in [c.x - 2.6, c.x + 2.6] {
        dormer(
            &mut p,
            v3(x, wall + 0.5, zf + jet - 0.3),
            1.4,
            1.25,
            1.4,
            Mat::RoofTile,
        );
    }
    // The great chimney on the right gable.
    p.cuboid(
        Mat::Plaster,
        v3(x1 + jet, -3.0, c.z - 1.0),
        v3(x1 + jet + 0.9, wall + 1.2, c.z + 1.0),
        hexc(0xd6cfbf),
        false,
    );
    chimney(
        &mut p,
        Mat::Brick,
        v3(x1 + jet + 0.45, wall + 1.2, c.z),
        wall + rh + 1.6,
        Vec4::ONE,
    );
    hanging_sign(
        &mut p,
        v3(x0 + 0.6, 5.3, zf + jet),
        1.0,
        0.9,
        hexc(0x23513a),
        hexc(0xe0b84a),
    );
    // A shed for the casks on the right.
    let (sx0, sx1) = (x1 + jet + 0.9, 7.4);
    for z in [zb + 0.6, zf - 0.6] {
        p.beam(
            Mat::Timber,
            v3(sx1 - 0.2, 0.0, z),
            v3(sx1 - 0.2, 2.2, z),
            0.15,
            oak,
        );
    }
    face(
        &mut p,
        Mat::RoofTile,
        [
            v3(sx1 + 0.2, 2.15, zf - 0.3),
            v3(sx1 + 0.2, 2.15, zb + 0.3),
            v3(sx0, 2.9, zb + 0.3),
            v3(sx0, 2.9, zf - 0.3),
        ],
        Vec4::ONE,
        Vec3::X,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_climate_has_three_burgher_houses_and_every_new_kind_a_look() {
        for biome in [
            Biome::Temperate,
            Biome::Tropical,
            Biome::Arid,
            Biome::Arctic,
        ] {
            let mut keys = std::collections::HashSet::new();
            for k in 0..64u64 {
                let seed = k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let mut out = Vec::new();
                house_look(biome, seed, &mut out);
                assert!(MODELS.contains(&out[0].model));
                keys.insert(out[0].model);
            }
            assert_eq!(keys.len(), 3, "{biome:?}");
        }
        for kind in [
            Kind::Glassworks,
            Kind::Saltery,
            Kind::Confectioner,
            Kind::Furrier,
            Kind::LampMaker,
            Kind::School,
            Kind::Tavern,
        ] {
            let mut out = Vec::new();
            assert!(look(kind, 7, &mut out), "{kind:?}");
            assert!(build(out[0].model).is_some());
        }
        for key in MODELS {
            assert!(build(key).is_some(), "{key}");
        }
    }
}
