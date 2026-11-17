//! Public buildings of the far regions: each climate's own quay, fishery, market square and
//! house of worship, in the same art kit as `looks` and `regional`.
//!
//! - Sarrowmere: a thatched warehouse on stilts over a timber quay, a fisher's hut with canoes
//!   and nets, a round thatched market pavilion, a whitewashed mission chapel with a bell gable.
//! - Amber Reach: a mud-brick quay house with a watch tower and awnings on a stone quay, a
//!   fisher's hut with a reed shade on a stone mole, a market under shade sails round a domed
//!   well, a domed shrine with a tapering tower.
//! - Frostholm: three painted wharf houses gable-on to a stone quay, a boathouse with a
//!   slipway and fish racks, a market round a roofed brazier, a tarred stave church.
//!
//! Models stand in a building's own space (origin at the middle of the footprint at floor
//! height, front toward +Z), on the same footprints as the home isles' buildings.

use std::f32::consts::{PI, TAU};

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::defs::Kind;
use crate::globe::Biome;
use crate::looks::{ModelKey, Piece, WHITE, h01, small_boat};
use crate::regional::{SNOW, beam_ends, mud, parapet, railing, roof_snow, up_quad};

/// The models this module builds (they are listed in `looks::MODELS`; a test checks that).
#[cfg(test)]
pub const MODELS: &[ModelKey] = &[
    "quay_tropic",
    "quay_arid",
    "quay_arctic",
    "fishery_tropic",
    "fishery_arid",
    "fishery_arctic",
    "market_tropic",
    "market_arid",
    "market_arctic",
    "chapel_tropic",
    "chapel_arid",
    "chapel_arctic",
    "sacks",
    "torch",
];

pub fn build(key: ModelKey) -> Option<Parts> {
    Some(match key {
        "quay_tropic" => quay_tropic(),
        "quay_arid" => quay_arid(),
        "quay_arctic" => quay_arctic(),
        "fishery_tropic" => fishery_tropic(),
        "fishery_arid" => fishery_arid(),
        "fishery_arctic" => fishery_arctic(),
        "market_tropic" => market_tropic(),
        "market_arid" => market_arid(),
        "market_arctic" => market_arctic(),
        "chapel_tropic" => chapel_tropic(),
        "chapel_arid" => chapel_arid(),
        "chapel_arctic" => chapel_arctic(),
        "sacks" => sacks(),
        "torch" => torch(),
        _ => return None,
    })
}

/// The pieces of a public building in a far region's style. False where the region keeps the
/// home isles' look for `kind`.
pub fn look(kind: Kind, biome: Biome, seed: u64, out: &mut Vec<Piece>) -> bool {
    // `ARCH_HOME_STYLE=1` keeps the home isles' buildings everywhere (for "before" pictures).
    static HOME_STYLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *HOME_STYLE.get_or_init(|| std::env::var_os("ARCH_HOME_STYLE").is_some()) {
        return false;
    }
    let r = |k: u64| h01(seed, 300 + k);
    let leaf = |k: u64| {
        let g = 0.85 + 0.3 * r(k);
        [g * (0.9 + 0.15 * r(k + 1)), g, g * 0.85]
    };
    match (kind, biome) {
        (Kind::Harbour, Biome::Tropical) => {
            out.push(Piece::at("quay_tropic", Vec3::ZERO));
            for i in 0..5 {
                let x = -10.4 + i as f32 * 1.3 + r(10 + i) * 0.4;
                out.push(
                    Piece::at("crate", v3(x, 0.0, 3.4 + r(20 + i) * 2.0)).yaw(r(30 + i) * 0.4),
                );
            }
            for i in 0..6 {
                out.push(Piece::at(
                    "barrel",
                    v3(
                        4.2 + (i % 3) as f32 * 0.85,
                        0.0,
                        3.0 + (i / 3) as f32 * 0.85,
                    ),
                ));
            }
            out.push(Piece::at("sacks", v3(-3.4, 0.0, 4.6)).yaw(0.3));
            out.push(Piece::at("sacks", v3(0.6, 0.0, 5.6)).yaw(-0.5).scale(0.85));
            out.push(Piece::at("canoe", v3(-10.6, 0.0, -4.0)).yaw(0.2));
            out.push(
                Piece::at("banana", v3(10.4, 0.0, -6.4))
                    .scale(1.2)
                    .tint(leaf(40)),
            );
        }
        (Kind::Harbour, Biome::Arid) => {
            out.push(Piece::at("quay_arid", Vec3::ZERO));
            for i in 0..6u64 {
                out.push(
                    Piece::at(
                        "jar",
                        v3(-6.4 + i as f32 * 0.95, 0.0, 0.2 + (i % 2) as f32 * 0.7),
                    )
                    .yaw(r(10 + i) * TAU)
                    .scale(0.85 + 0.4 * r(20 + i)),
                );
            }
            out.push(Piece::at("sacks", v3(1.4, 0.0, 4.4)).yaw(0.4));
            out.push(Piece::at("sacks", v3(3.6, 0.0, 5.4)).yaw(-0.3).scale(0.9));
            for i in 0..3 {
                out.push(
                    Piece::at("crate", v3(-9.6 + i as f32 * 1.3, 0.0, 2.2 + r(30 + i)))
                        .yaw(r(34 + i)),
                );
            }
        }
        (Kind::Harbour, Biome::Arctic) => {
            out.push(Piece::at("quay_arctic", Vec3::ZERO));
            for i in 0..8 {
                out.push(Piece::at(
                    "barrel",
                    v3(
                        -10.6 + (i % 4) as f32 * 0.85,
                        0.0,
                        2.4 + (i / 4) as f32 * 0.85,
                    ),
                ));
            }
            for i in 0..3 {
                out.push(
                    Piece::at("crate", v3(-2.4 + i as f32 * 1.4, 0.0, 4.6 + r(30 + i)))
                        .yaw(r(34 + i) * 0.5),
                );
            }
            out.push(Piece::at("sled", v3(3.6, 0.0, 1.2)).yaw(PI * 0.5));
            out.push(Piece::at("snow_drift", v3(-10.6, 0.0, -6.4)).scale(1.3));
            out.push(
                Piece::at("snow_drift", v3(10.8, 0.0, -6.6))
                    .yaw(2.0)
                    .scale(1.1),
            );
        }
        (Kind::Fishery, Biome::Tropical) => {
            out.push(Piece::at("fishery_tropic", Vec3::ZERO));
            out.push(Piece::at("canoe", v3(3.9, 0.0, 1.6)).yaw(PI * 0.5 + 0.25));
            out.push(Piece::at("canoe", v3(4.9, 0.0, -1.6)).yaw(PI * 0.5 - 0.2));
            out.push(
                Piece::at("banana", v3(-4.8, 0.0, -4.6))
                    .yaw(r(1) * TAU)
                    .tint(leaf(4)),
            );
        }
        (Kind::Fishery, Biome::Arid) => {
            out.push(Piece::at("fishery_arid", Vec3::ZERO));
            for i in 0..3u64 {
                out.push(
                    Piece::at(
                        "jar",
                        v3(2.9 + i as f32 * 0.8, 0.0, -1.6 + (i % 2) as f32 * 0.6),
                    )
                    .yaw(r(i) * TAU)
                    .scale(0.8 + 0.3 * r(4 + i)),
                );
            }
        }
        (Kind::Fishery, Biome::Arctic) => {
            out.push(Piece::at("fishery_arctic", Vec3::ZERO));
            out.push(Piece::at("fish_rack", v3(-4.3, 0.0, 2.6)).yaw(PI * 0.5));
            out.push(Piece::at("fish_rack", v3(-4.3, 0.0, -2.2)).yaw(PI * 0.5));
            out.push(Piece::at("barrel", v3(3.4, 0.0, -0.6)));
            out.push(Piece::at("barrel", v3(4.2, 0.0, -0.9)));
        }
        (Kind::Market, Biome::Tropical) => {
            out.push(Piece::at("market_tropic", Vec3::ZERO));
            for (x, z) in [(-8.7, -8.7), (8.7, -8.7), (-8.7, 8.7), (8.7, 8.7)] {
                out.push(Piece::at("torch", v3(x, 0.08, z)));
            }
            for (k, (x, z)) in [(-8.6, 0.0), (8.6, 0.0), (0.0, -8.8)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                out.push(
                    Piece::at("banana", v3(x, 0.08, z))
                        .yaw(r(k) * TAU)
                        .scale(0.9 + 0.4 * r(4 + k))
                        .tint(leaf(8 + k * 2)),
                );
            }
            out.push(Piece::at("sacks", v3(-1.0, 0.08, 8.0)).yaw(0.2).scale(0.8));
        }
        (Kind::Market, Biome::Arid) => {
            out.push(Piece::at("market_arid", Vec3::ZERO));
            for (k, (x, z)) in [
                (-8.4, -3.0),
                (-8.6, -1.8),
                (8.5, 2.4),
                (8.3, 3.6),
                (2.6, -8.6),
            ]
            .into_iter()
            .enumerate()
            {
                let k = k as u64;
                out.push(
                    Piece::at("jar", v3(x, 0.08, z))
                        .yaw(r(k) * TAU)
                        .scale(0.9 + 0.5 * r(8 + k)),
                );
            }
            out.push(Piece::at("sacks", v3(-2.8, 0.08, 8.2)).yaw(-0.3).scale(0.8));
        }
        (Kind::Market, Biome::Arctic) => {
            out.push(Piece::at("market_arctic", Vec3::ZERO));
            for (x, z) in [(-8.5, -8.5), (8.5, -8.5), (-8.5, 8.5), (8.5, 8.5)] {
                out.push(Piece::at("lamp_post", v3(x, 0.0, z)));
            }
            out.push(Piece::at("sled", v3(8.2, 0.08, 0.6)).yaw(0.2));
            out.push(Piece::at("fish_rack", v3(-8.4, 0.08, 0.0)).yaw(PI * 0.5));
            out.push(Piece::at("barrel", v3(0.8, 0.08, -8.4)));
            out.push(Piece::at("barrel", v3(1.7, 0.08, -8.6)));
        }
        (Kind::Chapel, Biome::Tropical) => {
            out.push(Piece::at("chapel_tropic", Vec3::ZERO));
            out.push(Piece::at("tub_shrub", v3(-1.9, 0.0, 6.2)).tint(leaf(1)));
            out.push(Piece::at("tub_shrub", v3(1.9, 0.0, 6.2)).tint(leaf(3)));
            for (k, (x, z)) in [(-6.4, 3.0), (6.3, -2.0), (-6.2, -6.4), (6.4, 6.6)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                out.push(
                    Piece::at("banana", v3(x, 0.0, z))
                        .yaw(r(10 + k) * TAU)
                        .scale(0.9 + 0.5 * r(14 + k))
                        .tint(leaf(18 + k * 2)),
                );
            }
        }
        (Kind::Chapel, Biome::Arid) => {
            out.push(Piece::at("chapel_arid", Vec3::ZERO));
            for (k, (x, z)) in [(4.6, 5.0), (5.4, 5.6), (5.0, 4.0)].into_iter().enumerate() {
                let k = k as u64;
                out.push(
                    Piece::at("jar", v3(x, 0.0, z))
                        .yaw(r(k) * TAU)
                        .scale(0.9 + 0.5 * r(4 + k)),
                );
            }
            out.push(Piece::at("bush", v3(-2.6, 0.0, 7.4)).scale(0.8));
            out.push(Piece::at("bush", v3(2.8, 0.0, 7.6)).scale(0.7));
        }
        (Kind::Chapel, Biome::Arctic) => {
            out.push(Piece::at("chapel_arctic", Vec3::ZERO));
            out.push(Piece::at("snow_drift", v3(-5.6, 0.0, 6.4)).scale(1.2));
            out.push(Piece::at("snow_drift", v3(5.8, 0.0, 7.0)).yaw(1.4));
            out.push(
                Piece::at("snow_drift", v3(5.9, 0.0, -7.4))
                    .yaw(3.0)
                    .scale(1.4),
            );
        }
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------------------ helpers

/// A dome standing on `base`: `r` across at its foot, `h` high.
fn dome(p: &mut Parts, m: Mat, base: Vec3, r: f32, h: f32, col: Vec4) {
    let rings = 5;
    for k in 0..rings {
        let a0 = k as f32 / rings as f32 * PI * 0.5;
        let a1 = (k + 1) as f32 / rings as f32 * PI * 0.5;
        p.cylinder(
            m,
            base + Vec3::Y * (h * a0.sin()),
            r * a0.cos(),
            (r * a1.cos()).max(0.03),
            h * (a1.sin() - a0.sin()),
            14,
            shade(col, 0.9 + 0.03 * k as f32),
            k == rings - 1,
        );
    }
}

/// A sheet seen from both sides (a sail, an awning, a net).
fn sheet(p: &mut Parts, m: Mat, a: Vec3, b: Vec3, c: Vec3, d: Vec3, col: Vec4) {
    p.quad(m, a, b, c, d, col);
    p.quad(m, d, c, b, a, shade(col, 0.85));
}

/// A low mound of snow on `at`.
fn snow_mound(p: &mut Parts, at: Vec3, r: f32, h: f32) {
    let rings = [(0.0, 1.0), (0.4, 0.86), (0.75, 0.58), (1.0, 0.18)];
    for k in 0..3 {
        let (y0, r0) = rings[k];
        let (y1, r1) = rings[k + 1];
        p.cylinder(
            Mat::Snow,
            at + Vec3::Y * (y0 * h - 0.05),
            r * r0,
            r * r1,
            (y1 - y0) * h,
            10,
            shade(SNOW, 0.93 + 0.035 * k as f32),
            k == 2,
        );
    }
}

/// A quay of planks on piles over the water half of a harbour's footprint.
fn timber_quay(p: &mut Parts, wood: Vec4) {
    p.cuboid(
        Mat::Planks,
        v3(-12.0, -0.35, -1.0),
        v3(12.0, 0.0, 8.0),
        shade(Vec4::ONE, 0.95),
        true,
    );
    let mut x = -11.5;
    while x <= 11.6 {
        for z in [1.5, 4.5, 7.7] {
            p.rod(
                Mat::Timber,
                v3(x, -8.0, z),
                v3(x, -0.3, z),
                0.16,
                6,
                shade(wood, 0.6),
            );
        }
        x += 2.3;
    }
    p.beam(
        Mat::Timber,
        v3(-12.0, 0.0, 7.85),
        v3(12.0, 0.0, 7.85),
        0.22,
        shade(wood, 0.7),
    );
    for x in [-10.0, -4.0, 2.0, 8.0] {
        p.cylinder(
            Mat::Timber,
            v3(x, 0.0, 7.3),
            0.2,
            0.17,
            0.9,
            8,
            shade(wood, 0.6),
            true,
        );
    }
}

/// A quay of dressed stone from the sea bed up, paved, with a coping and iron bollards. Call
/// it after `ground_occlusion`, which would darken the whole deck.
fn stone_quay(p: &mut Parts, col: Vec4) {
    p.cuboid(
        Mat::Cobble,
        v3(-12.0, -8.0, -1.4),
        v3(12.0, 0.0, 8.0),
        col,
        false,
    );
    // Paving on top.
    up_quad(
        p,
        Mat::Cobble,
        (-11.8, 11.8),
        (-1.3, 7.4),
        0.015,
        shade(col, 1.08),
    );
    p.cuboid(
        Mat::Stone,
        v3(-12.1, -0.3, 7.45),
        v3(12.1, 0.2, 8.1),
        shade(col, 0.82),
        false,
    );
    for x in [-10.0, -4.0, 2.0, 8.0] {
        p.cylinder(
            Mat::Iron,
            v3(x, 0.2, 7.75),
            0.2,
            0.16,
            0.6,
            10,
            Vec4::ONE,
            true,
        );
    }
}

/// The home isles' jib crane, standing on `base`.
fn jib_crane(p: &mut Parts, base: Vec3) {
    p.cylinder(Mat::Timber, base, 0.5, 0.35, 7.0, 8, Vec4::ONE, true);
    p.beam(
        Mat::Timber,
        base + v3(0.0, 6.5, 0.0),
        base + v3(-1.5, 9.5, 4.5),
        0.3,
        Vec4::ONE,
    );
    p.beam(
        Mat::Timber,
        base + v3(0.0, 2.0, 0.0),
        base + v3(-1.2, 8.8, 3.6),
        0.2,
        Vec4::ONE,
    );
    p.rod(
        Mat::Iron,
        base + v3(-1.5, 9.5, 4.5),
        base + v3(-1.5, 3.0, 4.5),
        0.03,
        4,
        Vec4::ONE,
    );
}

/// A heap of five sacks.
fn sacks() -> Parts {
    let mut p = Parts::default();
    let jute = hexc(0xb9a070);
    for (k, (x, y, z, yaw)) in [
        (-0.5, 0.17, 0.0, 0.1),
        (0.5, 0.17, 0.1, -0.2),
        (0.0, 0.17, 0.7, 1.5),
        (0.0, 0.5, 0.1, 0.4),
        (0.1, 0.5, 0.6, 1.2),
    ]
    .into_iter()
    .enumerate()
    {
        p.box_at(
            Mat::Canvas,
            v3(x, y, z),
            v3(0.48, 0.17, 0.3),
            yaw,
            shade(jute, 0.88 + 0.06 * k as f32),
        );
    }
    p.finish()
}

/// A torch on a bamboo pole.
fn torch() -> Parts {
    let mut p = Parts::default();
    let cane = hexc(0xb59a5a);
    p.cylinder(
        Mat::Timber,
        v3(0.0, -0.3, 0.0),
        0.06,
        0.05,
        2.7,
        6,
        cane,
        true,
    );
    for y in [0.7, 1.5, 2.2] {
        p.cylinder(
            Mat::Timber,
            v3(0.0, y, 0.0),
            0.075,
            0.075,
            0.05,
            6,
            shade(cane, 0.7),
            false,
        );
    }
    p.cylinder(
        Mat::Clay,
        v3(0.0, 2.38, 0.0),
        0.07,
        0.19,
        0.26,
        8,
        hexc(0x8a4a2a),
        false,
    );
    p.cylinder(
        Mat::Lamp,
        v3(0.0, 2.6, 0.0),
        0.13,
        0.02,
        0.36,
        6,
        Vec4::ONE,
        true,
    );
    p.finish()
}

// ------------------------------------------------------------------- Sarrowmere: jungle

const THATCH: u32 = 0xb7a070;

/// A timber quay with a thatched warehouse on stilts, a veranda over the quay and sheer legs.
fn quay_tropic() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x8a6a48);
    let thatch = hexc(THATCH);
    timber_quay(&mut p, wood);
    // The warehouse stands on posts over the beach; its floor runs out as a veranda.
    let floor = 1.1;
    let (l, w, wall) = (15.0, 6.2, 3.6);
    let c = v3(0.0, floor, -4.6);
    for x in [-7.2, -3.6, 0.0, 3.6, 7.2] {
        for z in [-7.5, -4.6, -1.7, 1.0] {
            p.rod(
                Mat::Timber,
                v3(x, -2.4, z),
                v3(x, floor, z),
                0.17,
                6,
                shade(wood, 0.6),
            );
        }
    }
    p.cuboid(
        Mat::Planks,
        v3(-7.9, floor - 0.22, -7.9),
        v3(7.9, floor, 1.3),
        shade(Vec4::ONE, 0.9),
        true,
    );
    let paint = hexc(0xe8dcc0);
    p.cuboid(
        Mat::Planks,
        c + v3(-l * 0.5, 0.0, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        paint,
        false,
    );
    for x in [-l * 0.5, -l * 0.17, l * 0.17, l * 0.5] {
        for z in [-w * 0.5, w * 0.5] {
            p.beam(
                Mat::Paint,
                c + v3(x, 0.0, z),
                c + v3(x, wall, z),
                0.18,
                hexc(0x2f7f86),
            );
        }
    }
    p.hip_roof(Mat::Thatch, c + v3(0.0, wall, 0.0), l, w, 4.0, 1.0, thatch);
    // Ridge poles crossed at the ends, as on the houses.
    for s in [-1.0, 1.0] {
        let x = s * (l - w) * 0.5;
        p.beam(
            Mat::Timber,
            c + v3(x, wall + 3.7, -0.5),
            c + v3(x, wall + 4.7, 0.5),
            0.09,
            shade(wood, 0.7),
        );
        p.beam(
            Mat::Timber,
            c + v3(x, wall + 3.7, 0.5),
            c + v3(x, wall + 4.7, -0.5),
            0.09,
            shade(wood, 0.7),
        );
    }
    // Veranda posts and its lean-to of thatch.
    let (z0, z1) = (c.z + w * 0.5, 1.15);
    for x in [-7.5, -4.5, -1.5, 1.5, 4.5, 7.5] {
        p.beam(
            Mat::Timber,
            v3(x, floor, z1),
            v3(x, floor + 2.5, z1),
            0.14,
            WHITE,
        );
    }
    let (ya, yb) = (floor + 2.45, floor + wall + 0.15);
    p.quad(
        Mat::Thatch,
        v3(-8.3, ya, z1 + 0.45),
        v3(8.3, ya, z1 + 0.45),
        v3(8.3, yb, z0),
        v3(-8.3, yb, z0),
        shade(thatch, 0.92),
    );
    railing(
        &mut p,
        v3(-7.8, floor, 1.2),
        v3(-2.2, floor, 1.2),
        0.9,
        WHITE,
    );
    railing(&mut p, v3(2.2, floor, 1.2), v3(7.8, floor, 1.2), 0.9, WHITE);
    // Wide doors, shuttered windows, a lettered board.
    let teal = hexc(0x2f7f86);
    for x in [-4.9, 4.9] {
        p.door(c + v3(x, 0.0, w * 0.5), Vec3::Z, 2.4, 2.8, teal);
    }
    p.windows(c + v3(0.0, 1.8, w * 0.5), Vec3::Z, 2, 1.0, 1.2, 1.4, WHITE);
    p.windows(c + v3(l * 0.5, 1.8, 0.0), Vec3::X, 2, 0.9, 1.1, 1.6, WHITE);
    p.windows(
        c + v3(-l * 0.5, 1.8, 0.0),
        -Vec3::X,
        2,
        0.9,
        1.1,
        1.6,
        WHITE,
    );
    p.cuboid(
        Mat::Paint,
        c + v3(-2.6, wall - 0.75, w * 0.5 + 0.03),
        c + v3(2.6, wall - 0.15, w * 0.5 + 0.1),
        hexc(0x1c2e3e),
        true,
    );
    // Steps from the veranda down to the quay.
    for (k, y) in [0.74, 0.37].into_iter().enumerate() {
        let z = 1.3 + k as f32 * 0.45;
        p.cuboid(
            Mat::Planks,
            v3(-1.9, 0.0, z),
            v3(1.9, y, z + 0.45),
            shade(Vec4::ONE, 0.85),
            false,
        );
    }
    // Sheer legs with a block and a sling of sacks.
    let top = v3(9.6, 8.2, 6.6);
    for s in [-1.0, 1.0] {
        p.beam(Mat::Timber, v3(9.6 + s * 1.5, 0.0, 3.6), top, 0.24, wood);
    }
    p.rod(Mat::Iron, top, v3(9.6, 0.0, -0.6), 0.03, 4, Vec4::ONE);
    p.rod(Mat::Iron, top, top - Vec3::Y * 5.2, 0.03, 4, Vec4::ONE);
    p.box_at(
        Mat::Canvas,
        top - Vec3::Y * 5.6,
        v3(0.5, 0.36, 0.34),
        0.5,
        hexc(0xb9a070),
    );
    // A palm-leaf shade on four poles at the quay's other end.
    for x in [-11.2, -7.6] {
        for z in [1.6, 5.2] {
            p.beam(
                Mat::Timber,
                v3(x, 0.0, z),
                v3(x, 2.5, z),
                0.1,
                shade(wood, 0.8),
            );
        }
    }
    p.quad(
        Mat::Thatch,
        v3(-11.7, 2.35, 5.7),
        v3(-7.1, 2.35, 5.7),
        v3(-7.1, 2.75, 1.1),
        v3(-11.7, 2.75, 1.1),
        thatch,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// A fisher's thatched hut on stilts with a jetty, nets on poles and fish baskets.
fn fishery_tropic() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x8a6a48);
    let thatch = hexc(THATCH);
    let deck = 1.0;
    let (l, w, wall) = (5.0, 3.4, 2.2);
    let c = v3(0.0, deck, -3.5);
    for x in [-2.7, 0.0, 2.7] {
        for z in [-5.3, -3.5, -1.2] {
            p.rod(
                Mat::Timber,
                v3(x, -2.4, z),
                v3(x, deck, z),
                0.13,
                6,
                shade(wood, 0.65),
            );
        }
    }
    p.cuboid(
        Mat::Planks,
        v3(-3.0, deck - 0.18, -5.5),
        v3(3.0, deck, -1.0),
        shade(Vec4::ONE, 0.9),
        true,
    );
    p.cuboid(
        Mat::Planks,
        c + v3(-l * 0.5, 0.0, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        hexc(0x4aa6a0),
        false,
    );
    p.hip_roof(Mat::Thatch, c + v3(0.0, wall, 0.0), l, w, 2.5, 0.75, thatch);
    p.door(
        c + v3(-1.0, 0.0, w * 0.5),
        Vec3::Z,
        1.0,
        1.9,
        hexc(0x5a3a24),
    );
    p.windows(c + v3(1.2, 1.2, w * 0.5), Vec3::Z, 1, 0.8, 0.8, 0.0, WHITE);
    // A ladder down to the jetty.
    for x in [-1.4, -0.6] {
        p.beam(Mat::Timber, v3(x, 0.0, -0.3), v3(x, deck, -1.0), 0.07, wood);
    }
    for k in 0..3 {
        let t = (k as f32 + 0.6) / 3.6;
        p.beam(
            Mat::Timber,
            v3(-1.4, deck * t, -0.3 - 0.7 * t),
            v3(-0.6, deck * t, -0.3 - 0.7 * t),
            0.05,
            wood,
        );
    }
    // The jetty.
    p.cuboid(
        Mat::Planks,
        v3(-1.2, -0.25, -0.9),
        v3(1.2, 0.0, 6.0),
        Vec4::ONE,
        true,
    );
    for z in [0.5, 3.0, 5.6] {
        for x in [-1.0, 1.0] {
            p.rod(
                Mat::Timber,
                v3(x, -6.0, z),
                v3(x, 0.55, z),
                0.11,
                6,
                shade(wood, 0.6),
            );
        }
    }
    // Nets hung between poles to dry.
    for (x, z0, z1) in [(-4.4, -3.6, 0.4), (-3.4, 1.4, 4.6)] {
        for z in [z0, z1] {
            p.beam(
                Mat::Timber,
                v3(x, 0.0, z),
                v3(x, 2.4, z),
                0.08,
                shade(wood, 0.8),
            );
        }
        p.beam(Mat::Timber, v3(x, 2.3, z0), v3(x, 2.3, z1), 0.05, wood);
        sheet(
            &mut p,
            Mat::Canvas,
            v3(x, 0.5, z0 + 0.15),
            v3(x, 0.7, z1 - 0.15),
            v3(x, 2.25, z1 - 0.1),
            v3(x, 2.25, z0 + 0.1),
            hexc(0x4a5a52),
        );
        // Floats along its foot.
        for k in 0..5 {
            let z = z0 + 0.4 + (z1 - z0 - 0.8) * k as f32 / 4.0;
            p.cylinder(
                Mat::Paint,
                v3(x, 0.52, z),
                0.09,
                0.09,
                0.12,
                6,
                hexc(0xd9755a),
                true,
            );
        }
    }
    // Baskets of fish on the jetty.
    for (x, z, s) in [(0.5, 1.2, 1.0), (-0.4, 2.0, 0.8), (0.4, 4.4, 0.9)] {
        p.cylinder(
            Mat::Thatch,
            v3(x, 0.0, z),
            0.26 * s,
            0.34 * s,
            0.36 * s,
            9,
            hexc(0xa88a4a),
            false,
        );
        p.cylinder(
            Mat::Slate,
            v3(x, 0.3 * s, z),
            0.3 * s,
            0.2 * s,
            0.1,
            9,
            hexc(0xaab4ba),
            true,
        );
    }
    p.ground_occlusion(0.6, 1.4);
    p.finish()
}

/// A market square of pale flags with a round thatched pavilion and heaps of fruit.
fn market_tropic() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x8a6a48);
    let thatch = hexc(THATCH);
    p.cuboid(
        Mat::Cobble,
        v3(-9.8, -0.6, -9.8),
        v3(9.8, 0.08, 9.8),
        hexc(0xe2d3ab),
        false,
    );
    // A kerb of boards.
    for (lo, hi) in [
        (v3(-10.0, -0.6, -10.0), v3(10.0, 0.14, -9.6)),
        (v3(-10.0, -0.6, 9.6), v3(10.0, 0.14, 10.0)),
        (v3(-10.0, -0.6, -9.6), v3(-9.6, 0.14, 9.6)),
        (v3(9.6, -0.6, -9.6), v3(10.0, 0.14, 9.6)),
    ] {
        p.cuboid(Mat::Planks, lo, hi, shade(Vec4::ONE, 0.8), false);
    }
    // The pavilion: a ring of posts under a cone of thatch.
    p.cylinder(
        Mat::Stone,
        v3(0.0, 0.08, 0.0),
        3.4,
        3.4,
        0.3,
        18,
        hexc(0xcfc3a6),
        true,
    );
    let n = 8;
    let post = |k: usize| {
        let a = k as f32 / n as f32 * TAU + 0.39;
        v3(a.cos() * 2.9, 0.0, a.sin() * 2.9)
    };
    for k in 0..n {
        let (a, b) = (post(k), post((k + 1) % n));
        p.rod(
            Mat::Timber,
            a + Vec3::Y * 0.38,
            a + Vec3::Y * 3.2,
            0.13,
            7,
            wood,
        );
        p.beam(
            Mat::Timber,
            a + Vec3::Y * 3.1,
            b + Vec3::Y * 3.1,
            0.14,
            shade(wood, 0.8),
        );
    }
    p.cylinder(
        Mat::Thatch,
        v3(0.0, 2.75, 0.0),
        4.3,
        4.0,
        0.22,
        18,
        shade(thatch, 0.78),
        false,
    );
    p.cylinder(
        Mat::Thatch,
        v3(0.0, 2.95, 0.0),
        4.05,
        0.3,
        3.1,
        18,
        thatch,
        true,
    );
    p.cylinder(
        Mat::Thatch,
        v3(0.0, 5.8, 0.0),
        0.55,
        0.4,
        0.5,
        10,
        shade(thatch, 0.8),
        true,
    );
    p.rod(
        Mat::Timber,
        v3(0.0, 6.2, 0.0),
        v3(0.0, 7.0, 0.0),
        0.05,
        5,
        wood,
    );
    // A round table of fruit under it.
    p.cylinder(
        Mat::Planks,
        v3(0.0, 0.38, 0.0),
        1.7,
        1.7,
        0.75,
        14,
        shade(Vec4::ONE, 0.9),
        true,
    );
    let fruit = [
        (0.0, 0.0, 0.62, 0xe0b83a),
        (0.9, 0.3, 0.42, 0x5f9a3a),
        (-0.8, 0.5, 0.45, 0xd9762a),
        (-0.3, -0.9, 0.4, 0xb8402c),
        (0.6, -0.8, 0.36, 0x8ab04a),
    ];
    for (x, z, r, col) in fruit {
        p.cylinder(
            Mat::Paint,
            v3(x, 1.13, z),
            r,
            r * 0.75,
            r * 0.45,
            9,
            shade(hexc(col), 0.85),
            false,
        );
        p.cylinder(
            Mat::Paint,
            v3(x, 1.13 + r * 0.45, z),
            r * 0.75,
            r * 0.15,
            r * 0.4,
            9,
            hexc(col),
            true,
        );
    }
    // Woven mats on the flags.
    for (x, z, a) in [(-6.6, 0.4, 0.0), (6.4, -0.6, 0.0), (0.2, 6.9, PI * 0.5)] {
        p.box_at(
            Mat::Thatch,
            v3(x, 0.1, z),
            v3(1.0, 0.025, 1.7),
            a,
            hexc(0xc9b27a),
        );
    }
    p.finish()
}

/// A whitewashed mission chapel: a tiled nave, a bell gable over the door, blue shutters.
fn chapel_tropic() -> Parts {
    let mut p = Parts::default();
    let white = hexc(0xf0e9d8);
    let blue = hexc(0x2f6a8a);
    let (l, w, wall) = (6.6, 11.0, 4.6);
    let c = v3(0.0, 0.0, -1.6);
    let (zb, zf) = (c.z - w * 0.5, c.z + w * 0.5);
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.4, -3.0, zb - 0.4),
        v3(l * 0.5 + 0.4, 0.5, zf + 1.4),
        hexc(0xcfc3a6),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, 0.5, zb),
        v3(l * 0.5, wall, zf),
        white,
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        c + v3(0.0, wall, 0.0),
        w,
        l,
        3.0,
        0.45,
        true,
        Vec4::ONE,
        white,
    );
    // Buttresses and tall shuttered windows down both sides.
    for k in 0..4 {
        let z = zb + 1.2 + k as f32 * 2.9;
        for s in [-1.0f32, 1.0] {
            let x = s * l * 0.5;
            let (x0, x1) = if s > 0.0 {
                (x, x + 0.55)
            } else {
                (x - 0.55, x)
            };
            p.cuboid(
                Mat::Plaster,
                v3(x0, 0.5, z - 0.3),
                v3(x1, wall - 1.0, z + 0.3),
                white,
                false,
            );
            if k < 3 {
                p.windows(
                    v3(x, 2.9, z + 1.45),
                    v3(s, 0.0, 0.0),
                    1,
                    0.9,
                    2.0,
                    0.0,
                    blue,
                );
            }
        }
    }
    // The front wall rises in steps to a bell gable.
    let f = zf;
    p.cuboid(
        Mat::Plaster,
        v3(-3.8, 0.5, f),
        v3(3.8, wall + 1.3, f + 0.6),
        white,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-2.7, wall + 1.3, f),
        v3(2.7, wall + 2.6, f + 0.6),
        white,
        false,
    );
    for s in [-1.0, 1.0] {
        p.cuboid(
            Mat::Plaster,
            v3(s * 1.1 - 0.45, wall + 2.6, f),
            v3(s * 1.1 + 0.45, wall + 4.6, f + 0.6),
            white,
            false,
        );
        // Scrolls on the shoulders.
        p.cylinder(
            Mat::Plaster,
            v3(s * 3.3, wall + 1.3, f + 0.3),
            0.45,
            0.12,
            0.8,
            8,
            white,
            true,
        );
        p.cylinder(
            Mat::Plaster,
            v3(s * 2.2, wall + 2.6, f + 0.3),
            0.4,
            0.1,
            0.7,
            8,
            white,
            true,
        );
    }
    p.cuboid(
        Mat::Plaster,
        v3(-1.75, wall + 4.6, f - 0.05),
        v3(1.75, wall + 5.0, f + 0.65),
        white,
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        v3(0.0, wall + 5.0, f + 0.3),
        3.5,
        0.7,
        0.75,
        0.2,
        false,
        Vec4::ONE,
        white,
    );
    // The bell on its yoke.
    p.beam(
        Mat::Timber,
        v3(-0.7, wall + 4.3, f + 0.3),
        v3(0.7, wall + 4.3, f + 0.3),
        0.14,
        hexc(0x4a3626),
    );
    p.cylinder(
        Mat::Copper,
        v3(0.0, wall + 3.3, f + 0.3),
        0.5,
        0.26,
        0.85,
        12,
        hexc(0xc8a04a),
        true,
    );
    // A cross.
    let top = v3(0.0, wall + 5.75, f + 0.3);
    p.beam(Mat::Iron, top, top + Vec3::Y * 1.5, 0.09, hexc(0x3a2c20));
    p.beam(
        Mat::Iron,
        top + v3(-0.45, 1.0, 0.0),
        top + v3(0.45, 1.0, 0.0),
        0.09,
        hexc(0x3a2c20),
    );
    // Door, a window over it, steps.
    p.cuboid(
        Mat::Plaster,
        v3(-1.45, 0.5, f + 0.6),
        v3(1.45, 3.95, f + 0.72),
        shade(white, 0.9),
        false,
    );
    p.door(v3(0.0, 0.5, f + 0.72), Vec3::Z, 1.9, 3.0, blue);
    p.windows(
        v3(0.0, wall + 0.55, f + 0.6),
        Vec3::Z,
        1,
        1.1,
        1.1,
        0.0,
        blue,
    );
    for (k, y) in [0.34, 0.17].into_iter().enumerate() {
        let z = f + 0.72 + k as f32 * 0.4;
        p.cuboid(
            Mat::Stone,
            v3(-2.1, -0.4, z),
            v3(2.1, y, z + 0.4 + 0.3),
            hexc(0xcfc3a6),
            false,
        );
    }
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

// --------------------------------------------------------------------- Amber Reach: arid

const SANDSTONE: u32 = 0xe2cfa4;

/// A stone quay with a mud-brick quay house, a watch tower with a pennant, and awnings.
fn quay_arid() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(SANDSTONE);
    let wall_col = mud(0);
    // The store: one long room behind three doors.
    let (l, w, wall) = (13.0, 6.2, 5.4);
    let c = v3(-2.4, 0.0, -4.6);
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, -3.0, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        wall_col,
        false,
    );
    parapet(&mut p, c + Vec3::Y * wall, l, w, 0.75, wall_col);
    beam_ends(&mut p, c + v3(0.0, wall - 0.55, w * 0.5), Vec3::Z, l - 1.0);
    let wood = hexc(0x5a3a24);
    for x in [-4.3, 0.0, 4.3] {
        p.cuboid(
            Mat::Plaster,
            c + v3(x - 1.45, 0.0, w * 0.5),
            c + v3(x + 1.45, 3.6, w * 0.5 + 0.14),
            shade(wall_col, 1.12),
            false,
        );
        p.door(c + v3(x, 0.0, w * 0.5 + 0.14), Vec3::Z, 2.1, 3.0, wood);
    }
    p.windows(
        c + v3(0.0, 4.25, w * 0.5),
        Vec3::Z,
        6,
        0.35,
        0.8,
        1.75,
        hexc(0x3a2a1c),
    );
    // The tower at its seaward corner.
    let t = v3(7.4, 0.0, -4.6);
    let (tw, th) = (4.4, 9.4);
    p.cuboid(
        Mat::Plaster,
        t + v3(-tw * 0.5, -3.0, -tw * 0.5),
        t + v3(tw * 0.5, th, tw * 0.5),
        shade(wall_col, 0.96),
        false,
    );
    parapet(&mut p, t + Vec3::Y * th, tw, tw, 0.8, shade(wall_col, 0.96));
    for n in [Vec3::Z, Vec3::X] {
        beam_ends(
            &mut p,
            t + n * (tw * 0.5) + Vec3::Y * (th - 0.6),
            n,
            tw - 0.6,
        );
        beam_ends(&mut p, t + n * (tw * 0.5) + Vec3::Y * 5.6, n, tw - 0.6);
        p.windows(
            t + n * (tw * 0.5) + Vec3::Y * 7.4,
            n,
            2,
            0.4,
            1.0,
            0.9,
            hexc(0x3a2a1c),
        );
    }
    p.door(t + v3(0.0, 0.0, tw * 0.5), Vec3::Z, 1.3, 2.4, wood);
    let pole = t + v3(0.0, th, 0.0);
    p.rod(Mat::Timber, pole, pole + Vec3::Y * 4.6, 0.06, 6, wood);
    sheet(
        &mut p,
        Mat::Canvas,
        pole + v3(0.0, 3.3, 0.0),
        pole + v3(2.3, 3.75, 0.25),
        pole + v3(2.3, 3.95, 0.25),
        pole + v3(0.0, 4.5, 0.0),
        hexc(0xb8402c),
    );
    // Striped awnings on poles along the front.
    let (za, zb) = (c.z + w * 0.5 + 0.1, 2.2);
    let stripes = [0xd9a441, 0xe9dcc0, 0xb8502c, 0xe9dcc0];
    for (k, stripe) in stripes.into_iter().enumerate() {
        let x0 = c.x - l * 0.5 + 0.3 + k as f32 * (l - 0.6) / 4.0;
        let x1 = x0 + (l - 0.6) / 4.0 - 0.15;
        sheet(
            &mut p,
            Mat::Canvas,
            v3(x0, 2.85, zb),
            v3(x1, 2.85, zb),
            v3(x1, 3.75, za),
            v3(x0, 3.75, za),
            hexc(stripe),
        );
    }
    for k in 0..5 {
        let x = c.x - l * 0.5 + 0.3 + k as f32 * (l - 0.6) / 4.0 - 0.07;
        p.rod(Mat::Timber, v3(x, 0.0, zb), v3(x, 2.9, zb), 0.06, 6, wood);
    }
    // A mast derrick with a long yard.
    let base = v3(-9.8, 0.0, 5.4);
    p.cylinder(Mat::Timber, base, 0.28, 0.2, 8.0, 8, wood, true);
    p.beam(
        Mat::Timber,
        base + v3(-1.6, 3.2, -1.2),
        base + v3(2.6, 9.4, 2.2),
        0.18,
        shade(wood, 1.2),
    );
    p.rod(
        Mat::Iron,
        base + v3(2.6, 9.4, 2.2),
        base + v3(2.6, 2.4, 2.2),
        0.03,
        4,
        Vec4::ONE,
    );
    p.rod(
        Mat::Iron,
        base + Vec3::Y * 7.9,
        base + v3(-1.6, 3.2, -1.2),
        0.025,
        4,
        Vec4::ONE,
    );
    // Steps down to the water at the quay's end.
    for k in 0..4 {
        let y = -0.5 * (k as f32 + 1.0);
        p.cuboid(
            Mat::Stone,
            v3(10.4, -8.0, 8.0 + k as f32 * 0.5),
            v3(12.0, y, 8.5 + k as f32 * 0.5),
            shade(stone, 0.9),
            false,
        );
    }
    p.ground_occlusion(0.6, 2.0);
    stone_quay(&mut p, stone);
    p.finish()
}

/// A fisher's mud hut with a reed shade, a stone mole, drying racks and a boat with a mast.
fn fishery_arid() -> Parts {
    let mut p = Parts::default();
    let wall_col = mud(1);
    let wood = hexc(0x5a3a24);
    let (l, w, wall) = (5.2, 3.8, 2.8);
    let c = v3(-0.4, 0.0, -3.7);
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, -3.0, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        wall_col,
        false,
    );
    parapet(&mut p, c + Vec3::Y * wall, l, w, 0.5, wall_col);
    beam_ends(&mut p, c + v3(0.0, wall - 0.4, w * 0.5), Vec3::Z, l - 0.8);
    p.door(c + v3(1.1, 0.0, w * 0.5), Vec3::Z, 1.1, 2.0, wood);
    p.windows(
        c + v3(-1.2, 1.6, w * 0.5),
        Vec3::Z,
        1,
        0.5,
        0.7,
        0.0,
        hexc(0x3a2a1c),
    );
    // A shade of reeds in front of the door.
    let (z0, z1) = (c.z + w * 0.5, 0.4);
    for x in [-2.9, 2.1] {
        p.rod(Mat::Timber, v3(x, 0.0, z1), v3(x, 2.2, z1), 0.07, 6, wood);
    }
    p.quad(
        Mat::Thatch,
        v3(-3.2, 2.15, z1 + 0.3),
        v3(2.4, 2.15, z1 + 0.3),
        v3(2.4, 2.45, z0),
        v3(-3.2, 2.45, z0),
        hexc(0xb9a26a),
    );
    // The mole.
    let stone = hexc(SANDSTONE);
    // Racks of split fish.
    let x = 4.5;
    for z in [-3.0, 0.0] {
        p.beam(Mat::Timber, v3(x, 0.0, z), v3(x, 1.8, z), 0.1, wood);
    }
    p.beam(Mat::Timber, v3(x, 1.7, -3.0), v3(x, 1.7, 0.0), 0.08, wood);
    for k in 0..6 {
        p.box_at(
            Mat::Canvas,
            v3(x, 1.35, -2.8 + k as f32 * 0.5),
            v3(0.03, 0.3, 0.08),
            0.0,
            hexc(0xb9b4a4),
        );
    }
    // A boat drawn up, her mast stepped and the sail furled along the yard.
    let mut boat = small_boat();
    boat.rod(
        Mat::Timber,
        v3(0.4, 0.2, 0.0),
        v3(0.4, 3.4, 0.0),
        0.05,
        6,
        wood,
    );
    boat.beam(
        Mat::Timber,
        v3(-1.5, 2.2, 0.0),
        v3(1.9, 3.6, 0.0),
        0.06,
        wood,
    );
    boat.rod(
        Mat::Canvas,
        v3(-1.2, 2.2, 0.0),
        v3(1.6, 3.35, 0.0),
        0.1,
        6,
        hexc(0xe2d6b8),
    );
    p.add_parts(
        &boat,
        Mat4::from_rotation_translation(Quat::from_rotation_y(1.2), v3(-4.0, 0.05, 2.6)),
    );
    p.ground_occlusion(0.6, 1.4);
    p.cuboid(
        Mat::Stone,
        v3(-1.3, -8.0, -0.4),
        v3(1.3, 0.0, 6.0),
        stone,
        false,
    );
    p.cuboid(
        Mat::Stone,
        v3(-1.4, -0.2, 5.5),
        v3(1.4, 0.18, 6.1),
        shade(stone, 0.82),
        false,
    );
    p.finish()
}

/// A market of pale flags under four shade sails strung from a domed well.
fn market_arid() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0xe6d6b0);
    let wall_col = mud(1);
    p.cuboid(
        Mat::Cobble,
        v3(-9.8, -0.6, -9.8),
        v3(9.8, 0.08, 9.8),
        stone,
        false,
    );
    for (lo, hi) in [
        (v3(-10.0, -0.6, -10.0), v3(10.0, 0.16, -9.65)),
        (v3(-10.0, -0.6, 9.65), v3(10.0, 0.16, 10.0)),
        (v3(-10.0, -0.6, -9.65), v3(-9.65, 0.16, 9.65)),
        (v3(9.65, -0.6, -9.65), v3(10.0, 0.16, 9.65)),
    ] {
        p.cuboid(Mat::Plaster, lo, hi, wall_col, false);
    }
    // The well under a dome on four pillars.
    p.cuboid(
        Mat::Stone,
        v3(-2.4, 0.08, -2.4),
        v3(2.4, 0.3, 2.4),
        shade(stone, 0.9),
        false,
    );
    p.cylinder(
        Mat::Stone,
        v3(0.0, 0.3, 0.0),
        1.05,
        1.05,
        0.75,
        14,
        shade(stone, 0.95),
        true,
    );
    p.cylinder(
        Mat::Window,
        v3(0.0, 0.3, 0.0),
        0.8,
        0.8,
        0.77,
        14,
        Vec4::ONE,
        true,
    );
    for x in [-1.75, 1.75] {
        for z in [-1.75, 1.75] {
            p.cuboid(
                Mat::Plaster,
                v3(x - 0.3, 0.3, z - 0.3),
                v3(x + 0.3, 3.3, z + 0.3),
                wall_col,
                false,
            );
        }
    }
    p.cuboid(
        Mat::Plaster,
        v3(-2.2, 3.3, -2.2),
        v3(2.2, 3.95, 2.2),
        wall_col,
        true,
    );
    for x in [-2.0, 2.0] {
        for z in [-2.0, 2.0] {
            p.cylinder(
                Mat::Plaster,
                v3(x, 3.95, z),
                0.22,
                0.06,
                0.5,
                6,
                shade(wall_col, 1.05),
                true,
            );
        }
    }
    dome(
        &mut p,
        Mat::Plaster,
        v3(0.0, 3.95, 0.0),
        1.85,
        1.6,
        hexc(0xf0e9d8),
    );
    p.rod(
        Mat::Copper,
        v3(0.0, 5.5, 0.0),
        v3(0.0, 6.3, 0.0),
        0.05,
        6,
        hexc(0xc8a04a),
    );
    // Shade sails from the dome's corners out to poles at the square's edge.
    let wood = hexc(0x5a3a24);
    let cloth = [0xd9a441, 0xb8502c, 0x3f6a8a, 0xe9dcc0];
    for (k, (sx, sz)) in [(1.0f32, 1.0f32), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
        .into_iter()
        .enumerate()
    {
        let a = v3(2.1 * sx, 3.75, 2.1 * sz);
        let b = v3(9.0 * sx, 4.3, 3.0 * sz);
        let c = v3(9.0 * sx, 4.9, 9.0 * sz);
        let d = v3(3.0 * sx, 4.3, 9.0 * sz);
        // (Wound so the first face looks up in every quarter.)
        if sx * sz > 0.0 {
            sheet(&mut p, Mat::Canvas, a, d, c, b, hexc(cloth[k]));
        } else {
            sheet(&mut p, Mat::Canvas, a, b, c, d, hexc(cloth[k]));
        }
        for q in [b, c, d] {
            p.rod(
                Mat::Timber,
                v3(q.x, 0.0, q.z),
                q + Vec3::Y * 0.25,
                0.07,
                6,
                wood,
            );
        }
    }
    // Carpets laid out for sale.
    for (x, z, hx, hz, col, band) in [
        (-6.8, 0.2, 1.1, 1.8, 0x9a3a2c, 0xd9a441),
        (6.6, -0.4, 1.0, 1.6, 0x2f5a7a, 0xe9dcc0),
        (0.4, 6.9, 1.7, 1.0, 0x7a4a8a, 0xd9a441),
    ] {
        up_quad(
            &mut p,
            Mat::Canvas,
            (x - hx, x + hx),
            (z - hz, z + hz),
            0.1,
            hexc(col),
        );
        up_quad(
            &mut p,
            Mat::Canvas,
            (x - hx * 0.6, x + hx * 0.6),
            (z - hz * 0.6, z + hz * 0.6),
            0.105,
            hexc(band),
        );
        up_quad(
            &mut p,
            Mat::Canvas,
            (x - hx * 0.35, x + hx * 0.35),
            (z - hz * 0.35, z + hz * 0.35),
            0.11,
            hexc(col),
        );
    }
    p.finish()
}

/// A mud-brick shrine: a domed hall behind a walled court, and a tapering tower stuck with
/// timbers.
fn chapel_arid() -> Parts {
    let mut p = Parts::default();
    let wall_col = mud(0);
    let pale = hexc(0xf0e9d8);
    let wood = hexc(0x4a3626);
    // The hall.
    let (l, w, wall) = (9.0, 8.4, 5.2);
    let c = v3(0.6, 0.0, -4.4);
    let f = c.z + w * 0.5;
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, -3.0, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        wall_col,
        false,
    );
    parapet(&mut p, c + Vec3::Y * wall, l, w, 0.8, wall_col);
    for (n, len) in [(Vec3::Z, l), (Vec3::X, w), (-Vec3::X, w)] {
        let half = if n.z != 0.0 { w } else { l } * 0.5;
        beam_ends(&mut p, c + n * half + Vec3::Y * (wall - 0.6), n, len - 1.0);
        beam_ends(&mut p, c + n * half + Vec3::Y * 2.9, n, len - 1.0);
    }
    // Pilasters either side of the door, and the door in a pale frame.
    for x in [-3.2, -1.7, 1.7, 3.2] {
        p.cuboid(
            Mat::Plaster,
            v3(c.x + x - 0.3, 0.0, f),
            v3(c.x + x + 0.3, wall + 0.8, f + 0.28),
            shade(wall_col, 1.06),
            false,
        );
        p.cylinder(
            Mat::Plaster,
            v3(c.x + x, wall + 0.8, f + 0.14),
            0.3,
            0.08,
            0.8,
            6,
            shade(wall_col, 1.1),
            true,
        );
    }
    p.cuboid(
        Mat::Plaster,
        v3(c.x - 1.25, 0.0, f),
        v3(c.x + 1.25, 3.5, f + 0.16),
        pale,
        false,
    );
    p.door(v3(c.x, 0.0, f + 0.16), Vec3::Z, 1.7, 2.9, wood);
    for (n, half) in [(Vec3::X, l * 0.5), (-Vec3::X, l * 0.5)] {
        p.windows(
            c + n * half + Vec3::Y * 1.9,
            n,
            3,
            0.35,
            1.3,
            1.7,
            hexc(0x3a2a1c),
        );
    }
    // The dome on its drum.
    let top = c + Vec3::Y * wall;
    p.cylinder(
        Mat::Plaster,
        top,
        3.2,
        3.1,
        0.9,
        16,
        shade(wall_col, 1.05),
        false,
    );
    dome(&mut p, Mat::Plaster, top + Vec3::Y * 0.9, 3.05, 2.7, pale);
    p.rod(
        Mat::Copper,
        top + Vec3::Y * 3.55,
        top + Vec3::Y * 4.7,
        0.06,
        6,
        hexc(0xc8a04a),
    );
    p.cylinder(
        Mat::Copper,
        top + Vec3::Y * 4.0,
        0.2,
        0.2,
        0.2,
        8,
        hexc(0xc8a04a),
        true,
    );
    // The tower: round, tapering, stuck with timbers for the plasterers to climb.
    let t = v3(-5.6, 0.0, 1.6);
    let (r0, r1, th) = (1.65, 1.05, 12.0);
    p.cylinder(
        Mat::Plaster,
        t - Vec3::Y * 3.0,
        r0,
        r0,
        3.0,
        12,
        wall_col,
        false,
    );
    p.cylinder(Mat::Plaster, t, r0, r1, th, 12, wall_col, false);
    for k in 0..5 {
        let y = 2.2 + k as f32 * 2.1;
        let r = r0 + (r1 - r0) * y / th;
        for j in 0..6 {
            let a = j as f32 / 6.0 * TAU + k as f32 * 0.5;
            let dir = v3(a.cos(), 0.0, a.sin());
            p.rod(
                Mat::Timber,
                t + dir * (r - 0.15) + Vec3::Y * y,
                t + dir * (r + 0.55) + Vec3::Y * y,
                0.05,
                5,
                wood,
            );
        }
    }
    p.cylinder(
        Mat::Plaster,
        t + Vec3::Y * th,
        r1 + 0.4,
        r1 + 0.4,
        0.4,
        12,
        shade(wall_col, 1.06),
        true,
    );
    for j in 0..6 {
        let a = j as f32 / 6.0 * TAU;
        p.cylinder(
            Mat::Plaster,
            t + v3(a.cos() * (r1 + 0.25), th + 0.4, a.sin() * (r1 + 0.25)),
            0.16,
            0.05,
            0.5,
            6,
            shade(wall_col, 1.1),
            true,
        );
    }
    p.cylinder(
        Mat::Plaster,
        t + Vec3::Y * (th + 0.4),
        0.75,
        0.7,
        1.7,
        10,
        wall_col,
        false,
    );
    dome(
        &mut p,
        Mat::Plaster,
        t + Vec3::Y * (th + 2.1),
        0.85,
        0.85,
        pale,
    );
    p.rod(
        Mat::Copper,
        t + Vec3::Y * (th + 2.9),
        t + Vec3::Y * (th + 3.7),
        0.04,
        5,
        hexc(0xc8a04a),
    );
    // The court: low walls with a gate.
    let (cx0, cx1, cz) = (-7.6, 7.4, 9.4);
    let low = |p: &mut Parts, lo: Vec3, hi: Vec3| {
        p.cuboid(Mat::Plaster, lo, hi, shade(wall_col, 0.97), false);
    };
    low(&mut p, v3(cx0, -1.0, cz - 0.4), v3(-1.5, 1.2, cz));
    low(&mut p, v3(1.5, -1.0, cz - 0.4), v3(cx1, 1.2, cz));
    low(&mut p, v3(cx0, -1.0, 3.6), v3(cx0 + 0.4, 1.2, cz - 0.4));
    low(&mut p, v3(cx1 - 0.4, -1.0, f), v3(cx1, 1.2, cz - 0.4));
    for x in [-1.9, 1.9] {
        p.cuboid(
            Mat::Plaster,
            v3(x - 0.45, -1.0, cz - 0.6),
            v3(x + 0.45, 2.3, cz + 0.2),
            wall_col,
            false,
        );
        p.cylinder(
            Mat::Plaster,
            v3(x, 2.3, cz - 0.2),
            0.4,
            0.1,
            0.8,
            6,
            shade(wall_col, 1.08),
            true,
        );
    }
    // A basin for washing.
    p.cylinder(
        Mat::Stone,
        v3(3.6, 0.0, 6.6),
        0.9,
        0.9,
        0.5,
        12,
        hexc(SANDSTONE),
        true,
    );
    p.cylinder(
        Mat::Window,
        v3(3.6, 0.0, 6.6),
        0.7,
        0.7,
        0.52,
        12,
        Vec4::ONE,
        true,
    );
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

// ----------------------------------------------------------------------- Frostholm: arctic

/// A stone quay with three painted wharf houses gable-on to the water, each with its hoist.
fn quay_arctic() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0xc6c8ca);
    let (hw, depth, wall) = (4.7, 6.3, 5.2);
    let cz = -4.75;
    let front = cz + depth * 0.5;
    p.cuboid(
        Mat::Stone,
        v3(-7.6, -3.0, cz - depth * 0.5 - 0.2),
        v3(7.6, 0.5, front + 0.2),
        shade(stone, 0.85),
        false,
    );
    let paints = [0x9a2f25, 0xc9963a, 0xd9d6cc];
    let slate = hexc(0x4a4f55);
    for (k, cx) in [-4.9f32, 0.0, 4.9].into_iter().enumerate() {
        let col = hexc(paints[k]);
        let wall = wall + [0.0, 0.5, -0.3][k];
        p.cuboid(
            Mat::Planks,
            v3(cx - hw * 0.5, 0.5, cz - depth * 0.5),
            v3(cx + hw * 0.5, wall, front),
            col,
            false,
        );
        let rc = v3(cx, wall, cz);
        p.gable_roof(
            Mat::Slate,
            Mat::Planks,
            rc,
            depth,
            hw,
            2.7,
            0.3,
            true,
            slate,
            col,
        );
        roof_snow(&mut p, rc, depth, hw, 2.7, 0.3, true, 0.62);
        for s in [-1.0, 1.0] {
            p.beam(
                Mat::Paint,
                v3(cx + s * hw * 0.5, 0.5, front),
                v3(cx + s * hw * 0.5, wall, front),
                0.16,
                WHITE,
            );
        }
        let green = hexc(0x2f4a3a);
        p.door(v3(cx, 0.5, front), Vec3::Z, 1.9, 2.3, green);
        p.door(v3(cx, 3.2, front), Vec3::Z, 1.2, 1.5, green);
        p.windows(v3(cx, 4.0, front), Vec3::Z, 2, 0.6, 0.8, 2.0, WHITE);
        // The hoist beam under the gable, its rope and hook.
        let y = wall + 1.75;
        p.beam(
            Mat::Timber,
            v3(cx, y, front - 0.4),
            v3(cx, y, front + 1.3),
            0.16,
            hexc(0x4a3626),
        );
        p.rod(
            Mat::Iron,
            v3(cx, y, front + 1.15),
            v3(cx, 3.4, front + 1.15),
            0.025,
            4,
            Vec4::ONE,
        );
        p.box_at(
            Mat::Iron,
            v3(cx, 3.3, front + 1.15),
            v3(0.07, 0.12, 0.07),
            0.0,
            Vec4::ONE,
        );
    }
    // A stone chimney on the middle house.
    p.cuboid(
        Mat::Stone,
        v3(0.9, 6.4, cz - 1.6),
        v3(1.7, 9.0, cz - 0.8),
        shade(stone, 0.9),
        false,
    );
    jib_crane(&mut p, v3(9.8, 0.0, 4.8));
    // A lantern on a post, and snow shovelled into heaps.
    let post = v3(-8.6, 0.0, 6.6);
    p.beam(
        Mat::Timber,
        post,
        post + Vec3::Y * 3.2,
        0.14,
        hexc(0x4a3626),
    );
    p.box_at(
        Mat::Lamp,
        post + Vec3::Y * 3.45,
        v3(0.16, 0.22, 0.16),
        0.0,
        Vec4::ONE,
    );
    p.cylinder(
        Mat::Iron,
        post + Vec3::Y * 3.67,
        0.28,
        0.04,
        0.22,
        6,
        Vec4::ONE,
        true,
    );
    for (x, z, r, h) in [
        (-11.0, 0.4, 1.3, 0.7),
        (-9.6, -0.4, 1.0, 0.5),
        (11.0, 0.2, 1.2, 0.65),
        (8.4, -0.3, 0.9, 0.4),
        (6.0, 6.9, 0.8, 0.35),
        (-6.4, 7.0, 0.9, 0.4),
    ] {
        snow_mound(&mut p, v3(x, 0.0, z), r, h);
    }
    p.ground_occlusion(0.6, 2.0);
    stone_quay(&mut p, stone);
    p.finish()
}

/// A red boathouse with a slipway of logs and a boat hauled up beside it.
fn fishery_arctic() -> Parts {
    let mut p = Parts::default();
    let red = hexc(0x9a2f25);
    let wood = hexc(0x4a3626);
    let (span, depth, wall) = (5.2, 5.8, 2.7);
    let c = v3(0.6, 0.0, -2.9);
    let front = c.z + depth * 0.5;
    p.cuboid(
        Mat::Stone,
        c + v3(-span * 0.5 - 0.2, -3.0, -depth * 0.5 - 0.2),
        c + v3(span * 0.5 + 0.2, 0.3, depth * 0.5 + 0.2),
        hexc(0x9a9a98),
        false,
    );
    p.cuboid(
        Mat::Planks,
        c + v3(-span * 0.5, 0.3, -depth * 0.5),
        c + v3(span * 0.5, wall, depth * 0.5),
        red,
        false,
    );
    let rc = c + Vec3::Y * wall;
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        rc,
        depth,
        span,
        2.3,
        0.35,
        true,
        hexc(0x4a4f55),
        red,
    );
    roof_snow(&mut p, rc, depth, span, 2.3, 0.35, true, 0.7);
    for s in [-1.0, 1.0] {
        p.beam(
            Mat::Paint,
            v3(c.x + s * span * 0.5, 0.3, front),
            v3(c.x + s * span * 0.5, wall, front),
            0.14,
            WHITE,
        );
    }
    // Double doors to the sea, one standing open.
    p.door(
        v3(c.x - 0.75, 0.3, front),
        Vec3::Z,
        1.5,
        2.2,
        shade(red, 0.7),
    );
    p.box_at(
        Mat::Planks,
        v3(c.x + 1.75, 1.4, front + 0.7),
        v3(0.05, 1.1, 0.75),
        0.25,
        shade(red, 0.75),
    );
    p.cuboid(
        Mat::Paint,
        v3(c.x + 0.05, 0.3, front - 0.3),
        v3(c.x + 1.5, 2.5, front - 0.25),
        hexc(0x15181c),
        false,
    );
    p.windows(v3(c.x, 3.6, front), Vec3::Z, 1, 0.6, 0.6, 0.0, WHITE);
    // The slip: two rails of logs running down into the water, with rollers across.
    for x in [-0.3, 1.5] {
        p.rod(
            Mat::Timber,
            v3(x, 0.25, front + 0.1),
            v3(x, -1.1, 6.0),
            0.13,
            6,
            wood,
        );
    }
    for k in 0..6 {
        let t = (k as f32 + 0.5) / 6.0;
        let z = front + 0.3 + (5.7 - front) * t;
        let y = 0.25 - 1.35 * t + 0.12;
        p.rod(
            Mat::Timber,
            v3(-0.6, y, z),
            v3(1.8, y, z),
            0.09,
            6,
            shade(wood, 1.3),
        );
    }
    p.add_parts(
        &small_boat(),
        Mat4::from_rotation_translation(Quat::from_rotation_y(1.35), v3(4.3, 0.05, 2.6)),
    );
    // Oars against the wall, a coil of rope, snow in the lee.
    for k in 0..2 {
        let x = c.x + span * 0.5 + 0.12 + k as f32 * 0.12;
        p.beam(
            Mat::Timber,
            v3(x, 0.0, -1.4 + k as f32 * 0.5),
            v3(x - 0.1, 3.0, -1.6 + k as f32 * 0.5),
            0.05,
            hexc(0xb9a070),
        );
    }
    p.cylinder(
        Mat::Canvas,
        v3(-2.6, 0.0, 1.2),
        0.4,
        0.36,
        0.22,
        10,
        hexc(0x8a7a52),
        true,
    );
    snow_mound(&mut p, v3(c.x - span * 0.5 - 0.6, 0.0, -3.4), 1.2, 0.6);
    snow_mound(&mut p, v3(c.x + span * 0.5 + 0.9, 0.0, -4.4), 1.0, 0.5);
    p.ground_occlusion(0.6, 1.4);
    p.finish()
}

/// A market of boards with a brazier under a snow-capped roof, benches, and snow swept to
/// the edges.
fn market_arctic() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x4a3626);
    p.cuboid(
        Mat::Planks,
        v3(-9.8, -0.6, -9.8),
        v3(9.8, 0.08, 9.8),
        hexc(0xe0d0b4),
        false,
    );
    p.cuboid(
        Mat::Stone,
        v3(-10.0, -0.6, -10.0),
        v3(10.0, 0.04, -9.7),
        Vec4::ONE,
        false,
    );
    // The shelter: four stout posts, braces, a pyramid of shingles under snow.
    let s = 2.1;
    for x in [-s, s] {
        for z in [-s, s] {
            p.cuboid(
                Mat::Timber,
                v3(x - 0.17, 0.08, z - 0.17),
                v3(x + 0.17, 3.3, z + 0.17),
                wood,
                false,
            );
            p.beam(
                Mat::Timber,
                v3(x, 2.3, z),
                v3(x - x.signum() * 0.9, 3.25, z),
                0.1,
                wood,
            );
            p.beam(
                Mat::Timber,
                v3(x, 2.3, z),
                v3(x, 3.25, z - z.signum() * 0.9),
                0.1,
                wood,
            );
        }
    }
    for (a, b) in [
        (v3(-s, 3.3, -s), v3(s, 3.3, -s)),
        (v3(-s, 3.3, s), v3(s, 3.3, s)),
        (v3(-s, 3.3, -s), v3(-s, 3.3, s)),
        (v3(s, 3.3, -s), v3(s, 3.3, s)),
    ] {
        p.beam(Mat::Timber, a, b, 0.2, wood);
    }
    let (span, rise) = (2.0 * s + 0.4, 2.5);
    p.hip_roof(
        Mat::Slate,
        v3(0.0, 3.4, 0.0),
        span,
        span,
        rise,
        0.7,
        hexc(0x4a4038),
    );
    let k = 0.66;
    p.hip_roof(
        Mat::Snow,
        v3(0.0, 3.4 + rise * (1.0 - k) + 0.07, 0.0),
        span * k,
        span * k,
        rise * k,
        0.0,
        SNOW,
    );
    // The brazier and a kettle on its tripod.
    p.cylinder(
        Mat::Iron,
        v3(0.0, 0.08, 0.0),
        0.5,
        0.62,
        0.7,
        10,
        Vec4::ONE,
        false,
    );
    for j in 0..3 {
        let a = j as f32 / 3.0 * TAU;
        p.beam(
            Mat::Iron,
            v3(a.cos() * 0.75, 0.08, a.sin() * 0.75),
            v3(0.0, 2.0, 0.0),
            0.04,
            Vec4::ONE,
        );
    }
    p.cylinder(
        Mat::Lamp,
        v3(0.0, 0.7, 0.0),
        0.56,
        0.4,
        0.14,
        10,
        Vec4::ONE,
        true,
    );
    p.cylinder(
        Mat::Iron,
        v3(0.0, 1.15, 0.0),
        0.22,
        0.28,
        0.32,
        8,
        shade(Vec4::ONE, 0.6),
        true,
    );
    // Benches on two sides.
    for z in [-1.55, 1.55] {
        p.cuboid(
            Mat::Planks,
            v3(-1.3, 0.5, z - 0.2),
            v3(1.3, 0.58, z + 0.2),
            Vec4::ONE,
            true,
        );
        for x in [-1.1, 1.1] {
            p.cuboid(
                Mat::Timber,
                v3(x - 0.06, 0.08, z - 0.16),
                v3(x + 0.06, 0.5, z + 0.16),
                wood,
                false,
            );
        }
    }
    // Snow swept to the edges of the square.
    for (x, z, r, h) in [
        (-9.0, -6.2, 1.4, 0.7),
        (-9.2, -3.6, 1.0, 0.5),
        (-8.8, 5.6, 1.3, 0.65),
        (9.0, -5.4, 1.2, 0.6),
        (9.1, 6.4, 1.5, 0.75),
        (8.9, 3.9, 0.9, 0.45),
        (-4.6, -9.0, 1.2, 0.55),
        (5.2, -9.1, 1.4, 0.65),
        (-2.4, 9.0, 1.0, 0.5),
        (4.4, 9.1, 1.2, 0.55),
    ] {
        snow_mound(&mut p, v3(x, 0.08, z), r, h);
    }
    p.finish()
}

/// A tarred stave church: a tall nave above a low aisle, shingled roofs in tiers under snow,
/// a spire on the ridge and carved heads on the gables.
fn chapel_arctic() -> Parts {
    let mut p = Parts::default();
    let tar = hexc(0x8a5c3a);
    let shingle = hexc(0x6a5a4e);
    let c = v3(0.0, 0.0, -1.2);
    // A footing of field stones.
    p.cuboid(
        Mat::Stone,
        c + v3(-4.6, -3.0, -6.1),
        c + v3(4.6, 0.4, 6.1),
        hexc(0x9a9a98),
        false,
    );
    // The aisle all round, under its own roof.
    let (al, aw, awall) = (11.6, 8.6, 2.4);
    p.cuboid(
        Mat::Planks,
        c + v3(-aw * 0.5, 0.4, -al * 0.5),
        c + v3(aw * 0.5, awall, al * 0.5),
        tar,
        false,
    );
    let mut skirt = Parts::default();
    skirt.hip_roof(Mat::Slate, Vec3::ZERO, al, aw, 3.2, 0.45, shingle);
    p.add_parts(
        &skirt,
        Mat4::from_rotation_translation(Quat::from_rotation_y(PI * 0.5), c + Vec3::Y * awall),
    );
    // Posts of the aisle's gallery.
    for k in 0..6 {
        let z = c.z - al * 0.5 + 0.3 + k as f32 * (al - 0.6) / 5.0;
        for s in [-1.0, 1.0] {
            p.beam(
                Mat::Timber,
                v3(s * (aw * 0.5 + 0.02), 0.4, z),
                v3(s * (aw * 0.5 + 0.02), awall, z),
                0.2,
                shade(tar, 1.35),
            );
        }
    }
    // The nave rising through it.
    let (nl, nw, nwall) = (8.8, 5.6, 6.8);
    p.cuboid(
        Mat::Planks,
        c + v3(-nw * 0.5, 0.4, -nl * 0.5),
        c + v3(nw * 0.5, nwall, nl * 0.5),
        tar,
        false,
    );
    for s in [-1.0, 1.0] {
        for z in [-nl * 0.5, nl * 0.5] {
            p.beam(
                Mat::Timber,
                c + v3(s * nw * 0.5, awall, z),
                c + v3(s * nw * 0.5, nwall, z),
                0.26,
                shade(tar, 1.35),
            );
        }
        p.windows(
            c + v3(s * nw * 0.5, 5.6, 0.0),
            v3(s, 0.0, 0.0),
            4,
            0.4,
            0.5,
            1.5,
            hexc(0x2a1c14),
        );
    }
    let rc = c + Vec3::Y * nwall;
    let rise = 4.8;
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        rc,
        nl,
        nw,
        rise,
        0.5,
        true,
        shingle,
        tar,
    );
    roof_snow(&mut p, rc, nl, nw, rise, 0.5, true, 0.5);
    // Carved heads rearing from both ends of the ridge.
    let ridge = nwall + rise;
    for s in [-1.0, 1.0] {
        let end = c + v3(0.0, ridge - 0.1, s * (nl * 0.5 + 0.4));
        p.beam(
            Mat::Timber,
            end,
            end + v3(0.0, 1.2, s * 0.9),
            0.16,
            shade(tar, 1.3),
        );
        p.beam(
            Mat::Timber,
            end + v3(0.0, 1.2, s * 0.9),
            end + v3(0.0, 1.25, s * 1.5),
            0.2,
            shade(tar, 1.3),
        );
    }
    // The ridge turret and its spire.
    let t = c + Vec3::Y * (ridge - 1.5);
    p.cuboid(
        Mat::Planks,
        t + v3(-0.95, 0.0, -0.95),
        t + v3(0.95, 3.6, 0.95),
        tar,
        false,
    );
    p.cylinder(
        Mat::Slate,
        t + Vec3::Y * 3.6,
        1.55,
        1.1,
        0.5,
        8,
        shingle,
        false,
    );
    p.cylinder(
        Mat::Slate,
        t + Vec3::Y * 4.1,
        1.1,
        0.05,
        4.6,
        8,
        shingle,
        true,
    );
    snow_mound(&mut p, t + Vec3::Y * 3.72, 1.5, 0.3);
    let top = t + Vec3::Y * 8.6;
    p.beam(Mat::Iron, top, top + Vec3::Y * 1.3, 0.07, hexc(0x8a7a3a));
    p.beam(
        Mat::Iron,
        top + v3(-0.4, 0.85, 0.0),
        top + v3(0.4, 0.85, 0.0),
        0.07,
        hexc(0x8a7a3a),
    );
    // The porch and its door.
    let f = c.z + al * 0.5;
    for x in [-1.2, 1.2] {
        p.beam(
            Mat::Timber,
            v3(x, 0.0, f + 1.7),
            v3(x, 2.6, f + 1.7),
            0.2,
            shade(tar, 1.35),
        );
    }
    p.cuboid(
        Mat::Stone,
        v3(-1.5, -1.0, f),
        v3(1.5, 0.2, f + 2.0),
        hexc(0x9a9a98),
        false,
    );
    let pc = v3(0.0, 2.6, f + 0.95);
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        pc,
        2.1,
        2.9,
        1.7,
        0.3,
        true,
        shingle,
        tar,
    );
    roof_snow(&mut p, pc, 2.1, 2.9, 1.7, 0.3, true, 0.6);
    p.door(v3(0.0, 0.4, f), Vec3::Z, 1.3, 2.1, hexc(0x7a2a20));
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_far_region_has_its_own_public_buildings() {
        for biome in [Biome::Tropical, Biome::Arid, Biome::Arctic] {
            for kind in [Kind::Harbour, Kind::Fishery, Kind::Market, Kind::Chapel] {
                let mut out = Vec::new();
                assert!(look(kind, biome, 7, &mut out), "{kind:?} in {biome:?}");
                // The first piece is the building itself, and one of this module's models.
                assert!(MODELS.contains(&out[0].model), "{}", out[0].model);
                for piece in &out {
                    assert!(
                        crate::looks::MODELS.contains(&piece.model),
                        "{} is not listed",
                        piece.model
                    );
                }
            }
        }
        // The home isles keep theirs.
        assert!(!look(Kind::Harbour, Biome::Temperate, 7, &mut Vec::new()));
        for key in MODELS {
            assert!(build(key).is_some(), "{key}");
        }
    }
}
