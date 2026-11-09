//! What each building looks like: models built from the art kit, in the building's own space
//! (origin at the middle of its footprint at floor height, front toward +Z).
//!
//! Every model is built once and drawn as instanced rows: forty houses of one variant are
//! one draw per material.

use std::f32::consts::{PI, TAU};

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::defs::{CELL_M, Kind, Tier};

/// A model's name in the art registry.
pub type ModelKey = &'static str;

/// One placed copy of a model in a building's space.
#[derive(Clone, Copy, Debug)]
pub struct Piece {
    pub model: ModelKey,
    pub pos: Vec3,
    pub yaw: f32,
    pub scale: f32,
    pub tint: [f32; 3],
}

impl Piece {
    pub fn at(model: ModelKey, pos: Vec3) -> Self {
        Self {
            model,
            pos,
            yaw: 0.0,
            scale: 1.0,
            tint: [1.0; 3],
        }
    }

    pub fn yaw(mut self, y: f32) -> Self {
        self.yaw = y;
        self
    }

    pub fn scale(mut self, s: f32) -> Self {
        self.scale = s;
        self
    }

    pub fn tint(mut self, t: [f32; 3]) -> Self {
        self.tint = t;
        self
    }
}

fn h01(seed: u64, k: u64) -> f32 {
    let mut x = seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Every model the game draws, by key, built on demand by [`build_model`].
pub const MODELS: &[ModelKey] = &[
    "cottage_0",
    "cottage_1",
    "cottage_2",
    "townhouse_0",
    "townhouse_1",
    "townhouse_2",
    "harbour",
    "fishery",
    "woodcutter",
    "sawmill",
    "weaver",
    "farmstead",
    "barn",
    "fence_6",
    "sheep",
    "wheat_row",
    "hop_pole",
    "oast",
    "mill_tower",
    "bakery",
    "brewery",
    "clay_pit",
    "brickworks",
    "market_plate",
    "well",
    "chapel",
    "airship_mast",
    "log_pile",
    "plank_stack",
    "crate",
    "barrel",
    "lamp_post",
    "plot",
];

/// The pieces a building is drawn with (without its moving parts).
pub fn look(kind: Kind, tier: Tier, seed: u64) -> Vec<Piece> {
    let v = (h01(seed, 1) * 3.0) as usize % 3;
    let (w, d) = kind.def().size;
    let (hw, hd) = (
        w as f32 * CELL_M as f32 * 0.5,
        d as f32 * CELL_M as f32 * 0.5,
    );
    let mut out = Vec::new();
    match kind {
        Kind::Road => {}
        Kind::House => {
            let key: ModelKey = match (tier, v) {
                (Tier::Fisherfolk, 0) => "cottage_0",
                (Tier::Fisherfolk, 1) => "cottage_1",
                (Tier::Fisherfolk, _) => "cottage_2",
                (Tier::Labourers, 0) => "townhouse_0",
                (Tier::Labourers, 1) => "townhouse_1",
                (Tier::Labourers, _) => "townhouse_2",
            };
            let t = 0.9 + 0.2 * h01(seed, 2);
            out.push(Piece::at(key, Vec3::ZERO).tint([t, t * (0.97 + 0.06 * h01(seed, 3)), t]));
            if tier == Tier::Fisherfolk {
                out.push(Piece::at("plot", v3(0.0, 0.0, -3.6)).yaw(0.0));
            }
        }
        Kind::Harbour => {
            out.push(Piece::at("harbour", Vec3::ZERO));
            for i in 0..5 {
                let x = -9.0 + i as f32 * 1.3 + h01(seed, 10 + i) * 0.4;
                out.push(
                    Piece::at("crate", v3(x, 0.0, 3.0 + h01(seed, 20 + i) * 2.0))
                        .yaw(h01(seed, 30 + i) * 0.4),
                );
            }
            for i in 0..4 {
                out.push(Piece::at(
                    "barrel",
                    v3(6.0 + i as f32 * 0.8, 0.0, 1.5 + (i % 2) as f32 * 0.8),
                ));
            }
        }
        Kind::Fishery => out.push(Piece::at("fishery", Vec3::ZERO)),
        Kind::Woodcutter => {
            out.push(Piece::at("woodcutter", v3(-1.5, 0.0, -1.5)));
            out.push(Piece::at("log_pile", v3(3.0, 0.0, 2.5)).yaw(0.3));
            out.push(
                Piece::at("log_pile", v3(-2.5, 0.0, 3.5))
                    .yaw(1.4)
                    .scale(0.8),
            );
        }
        Kind::Sawmill => {
            out.push(Piece::at("sawmill", Vec3::ZERO));
            out.push(Piece::at("plank_stack", v3(3.6, 0.0, 6.0)).yaw(PI * 0.5));
            out.push(Piece::at("plank_stack", v3(-3.6, 0.0, 6.2)).yaw(PI * 0.5));
            out.push(Piece::at("log_pile", v3(0.0, 0.0, -6.6)));
        }
        Kind::Weaver => out.push(Piece::at("weaver", Vec3::ZERO)),
        Kind::Bakery => out.push(Piece::at("bakery", Vec3::ZERO)),
        Kind::Brewery => {
            out.push(Piece::at("brewery", Vec3::ZERO));
            for i in 0..6 {
                out.push(Piece::at(
                    "barrel",
                    v3(-5.5 + (i % 3) as f32 * 0.8, 0.0, 6.4 - (i / 3) as f32 * 0.8),
                ));
            }
        }
        Kind::ClayPit => out.push(Piece::at("clay_pit", Vec3::ZERO)),
        Kind::Brickworks => out.push(Piece::at("brickworks", Vec3::ZERO)),
        Kind::Mill => out.push(Piece::at("mill_tower", Vec3::ZERO)),
        Kind::Market => {
            out.push(Piece::at("market_plate", Vec3::ZERO));
            out.push(Piece::at("well", Vec3::ZERO));
            for (x, z) in [(-8.5, -8.5), (8.5, -8.5), (-8.5, 8.5), (8.5, 8.5)] {
                out.push(Piece::at("lamp_post", v3(x, 0.0, z)));
            }
        }
        Kind::Chapel => out.push(Piece::at("chapel", Vec3::ZERO)),
        Kind::AirshipMast => out.push(Piece::at("airship_mast", Vec3::ZERO)),
        Kind::SheepFarm | Kind::GrainFarm | Kind::HopFarm => {
            // Farmyard at the front left, the rest is paddock or field.
            let yard = v3(-hw + 6.0, 0.0, hd - 6.0);
            out.push(Piece::at("farmstead", yard + v3(0.0, 0.0, 1.0)));
            if kind == Kind::HopFarm {
                out.push(Piece::at("oast", yard + v3(6.5, 0.0, 1.5)));
            } else {
                out.push(Piece::at("barn", yard + v3(0.5, 0.0, -6.5)).yaw(PI * 0.5));
            }
            // Fence round the whole lot.
            let mut x = -hw + 0.3;
            while x < hw - 3.0 {
                out.push(Piece::at("fence_6", v3(x + 3.0, 0.0, -hd + 0.3)));
                out.push(Piece::at("fence_6", v3(x + 3.0, 0.0, hd - 0.3)));
                x += 6.0;
            }
            let mut z = -hd + 0.3;
            while z < hd - 3.0 {
                out.push(Piece::at("fence_6", v3(-hw + 0.3, 0.0, z + 3.0)).yaw(PI * 0.5));
                out.push(Piece::at("fence_6", v3(hw - 0.3, 0.0, z + 3.0)).yaw(PI * 0.5));
                z += 6.0;
            }
            let in_yard = |p: Vec3| p.x < yard.x + 7.5 && p.z > yard.z - 11.0;
            match kind {
                Kind::SheepFarm => {
                    for i in 0..22 {
                        let p = v3(
                            (h01(seed, 100 + i) * 2.0 - 1.0) * (hw - 2.0),
                            0.0,
                            (h01(seed, 200 + i) * 2.0 - 1.0) * (hd - 2.0),
                        );
                        if in_yard(p) {
                            continue;
                        }
                        let t = 0.85 + 0.25 * h01(seed, 300 + i);
                        out.push(
                            Piece::at("sheep", p)
                                .yaw(h01(seed, 400 + i) * TAU)
                                .scale(0.9 + 0.2 * h01(seed, 500 + i))
                                .tint([t, t, t * 0.97]),
                        );
                    }
                }
                Kind::GrainFarm => {
                    // Rows 0.9 m apart, in 6 m lengths.
                    let mut z = -hd + 1.5;
                    let mut k = 0u64;
                    while z < hd - 1.5 {
                        let mut x = -hw + 1.5;
                        while x + 6.0 < hw - 1.0 {
                            let p = v3(x + 3.0, 0.0, z);
                            if !in_yard(p) {
                                let ripe = 0.92 + 0.12 * h01(seed, 600 + k);
                                out.push(Piece::at("wheat_row", p).tint([ripe, ripe * 0.98, 0.9]));
                            }
                            x += 6.0;
                            k += 1;
                        }
                        z += 0.9;
                    }
                }
                _ => {
                    let mut z = -hd + 2.0;
                    let mut k = 0u64;
                    while z < hd - 2.0 {
                        let mut x = -hw + 2.0;
                        while x < hw - 2.0 {
                            let p = v3(x, 0.0, z);
                            if !in_yard(p) {
                                out.push(
                                    Piece::at("hop_pole", p)
                                        .yaw(h01(seed, 700 + k) * TAU)
                                        .scale(0.9 + 0.2 * h01(seed, 800 + k)),
                                );
                            }
                            x += 2.4;
                            k += 1;
                        }
                        z += 2.4;
                    }
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------ models

const WHITE: Vec4 = Vec4::new(0.92, 0.9, 0.85, 1.0);

fn plaster_col(v: usize) -> Vec4 {
    match v {
        0 => hexc(0xe8dcc0),
        1 => hexc(0xd9b98a),
        _ => hexc(0xc9d0cf),
    }
}

/// Stone foundation under a footprint `l × w`, from 3 m below the floor to just above it.
fn plinth(p: &mut Parts, l: f32, w: f32, top: f32) {
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5, -3.0, -w * 0.5),
        v3(l * 0.5, top, w * 0.5),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
}

/// Half-timbered cottage of a fisher family, on a 12 m lot.
fn cottage(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (7.0, 5.2, 3.0);
    let z0 = 1.2;
    let c = v3(0.0, 0.0, z0);
    plinth(&mut p, l + 0.2, w + 0.2, 0.35);
    let pc = plaster_col(v);
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, 0.35, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        pc,
        false,
    );
    // Timber frame: posts, a sill and a head beam, braces.
    let tc = hexc(0x3b2a1e);
    for x in [-l * 0.5, -l * 0.17, l * 0.17, l * 0.5] {
        for z in [-w * 0.5, w * 0.5] {
            p.beam(Mat::Timber, c + v3(x, 0.35, z), c + v3(x, wall, z), 0.2, tc);
        }
    }
    for z in [-w * 0.5, w * 0.5] {
        p.beam(
            Mat::Timber,
            c + v3(-l * 0.5, wall - 0.1, z),
            c + v3(l * 0.5, wall - 0.1, z),
            0.22,
            tc,
        );
        p.beam(
            Mat::Timber,
            c + v3(-l * 0.5, 0.45, z),
            c + v3(l * 0.5, 0.45, z),
            0.2,
            tc,
        );
        p.beam(
            Mat::Timber,
            c + v3(-l * 0.5, 0.5, z),
            c + v3(-l * 0.17, wall - 0.2, z),
            0.14,
            tc,
        );
        p.beam(
            Mat::Timber,
            c + v3(l * 0.5, 0.5, z),
            c + v3(l * 0.17, wall - 0.2, z),
            0.14,
            tc,
        );
    }
    for x in [-l * 0.5, l * 0.5] {
        p.beam(
            Mat::Timber,
            c + v3(x, wall - 0.1, -w * 0.5),
            c + v3(x, wall - 0.1, w * 0.5),
            0.22,
            tc,
        );
    }
    let roof = if v == 1 { Mat::RoofTile } else { Mat::Thatch };
    let over = if roof == Mat::Thatch { 0.7 } else { 0.45 };
    p.gable_roof(
        roof,
        Mat::Plaster,
        c + v3(0.0, wall, 0.0),
        l,
        w,
        3.4,
        over,
        false,
        Vec4::ONE,
        pc,
    );
    // Chimney.
    let cx = if v == 2 { -l * 0.32 } else { l * 0.32 };
    p.cuboid(
        Mat::Brick,
        c + v3(cx - 0.4, wall, -0.4),
        c + v3(cx + 0.4, wall + 4.1, 0.4),
        Vec4::ONE,
        false,
    );
    p.cuboid(
        Mat::Brick,
        c + v3(cx - 0.5, wall + 4.0, -0.5),
        c + v3(cx + 0.5, wall + 4.25, 0.5),
        shade(Vec4::ONE, 0.8),
        false,
    );
    // Door and windows.
    p.door(
        c + v3(-0.9, 0.35, w * 0.5),
        Vec3::Z,
        0.95,
        2.0,
        hexc(0x2f4a5a),
    );
    p.windows(c + v3(1.6, 1.7, w * 0.5), Vec3::Z, 1, 0.9, 1.0, 0.0, WHITE);
    p.windows(c + v3(-2.6, 1.7, w * 0.5), Vec3::Z, 1, 0.8, 1.0, 0.0, WHITE);
    p.windows(
        c + v3(0.0, 1.7, -w * 0.5),
        -Vec3::Z,
        2,
        0.8,
        1.0,
        1.4,
        WHITE,
    );
    p.windows(c + v3(l * 0.5, 1.7, 0.0), Vec3::X, 1, 0.7, 0.9, 0.0, WHITE);
    // Lean-to shed.
    let sx = if v == 2 {
        l * 0.5 + 1.1
    } else {
        -l * 0.5 - 1.1
    };
    p.cuboid(
        Mat::Planks,
        c + v3(sx - 1.0, 0.0, -1.6),
        c + v3(sx + 1.0, 2.0, 1.2),
        shade(Vec4::ONE, 0.8),
        false,
    );
    p.quad(
        Mat::RoofTile,
        c + v3(sx - 1.2, 2.3, 1.4),
        c + v3(sx + 1.2, 2.3, 1.4),
        c + v3(sx + 1.2, 2.0, -1.8),
        c + v3(sx - 1.2, 2.0, -1.8),
        shade(Vec4::ONE, 0.85),
    );
    // Front fence along the lot's edge.
    for i in 0..6 {
        let x = -5.5 + i as f32 * 2.2;
        if (x + 0.9).abs() < 1.2 {
            continue;
        }
        p.beam(Mat::Timber, v3(x, 0.0, 5.8), v3(x, 1.0, 5.8), 0.1, WHITE);
        p.beam(
            Mat::Timber,
            v3(x, 0.8, 5.8),
            v3(x + 2.2, 0.8, 5.8),
            0.07,
            WHITE,
        );
        p.beam(
            Mat::Timber,
            v3(x, 0.45, 5.8),
            v3(x + 2.2, 0.45, 5.8),
            0.07,
            WHITE,
        );
    }
    p.ground_occlusion(0.55, 1.6);
    p.finish()
}

/// Brick terrace house of a labourer's family: two storeys and an attic.
fn townhouse(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (8.4, 7.0, 6.2);
    let c = v3(0.0, 0.0, 0.8);
    plinth(&mut p, l + 0.3, w + 0.3, 0.5);
    let bc = match v {
        0 => Vec4::ONE,
        1 => hexc(0xd8c8b8),
        _ => hexc(0xf0d0c0),
    };
    p.cuboid(
        Mat::Brick,
        c + v3(-l * 0.5, 0.5, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        bc,
        false,
    );
    // String course and cornice.
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5 - 0.06, 3.2, -w * 0.5 - 0.06),
        c + v3(l * 0.5 + 0.06, 3.4, w * 0.5 + 0.06),
        WHITE,
        true,
    );
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5 - 0.15, wall - 0.25, -w * 0.5 - 0.15),
        c + v3(l * 0.5 + 0.15, wall, w * 0.5 + 0.15),
        WHITE,
        true,
    );
    let roof = if v == 1 { Mat::RoofTile } else { Mat::Slate };
    p.gable_roof(
        roof,
        Mat::Brick,
        c + v3(0.0, wall, 0.0),
        l,
        w,
        3.0,
        0.35,
        false,
        Vec4::ONE,
        bc,
    );
    // Dormer.
    p.cuboid(
        Mat::Paint,
        c + v3(-0.8, wall + 0.3, 1.2),
        c + v3(0.8, wall + 1.8, 2.4),
        WHITE,
        false,
    );
    p.windows(
        c + v3(0.0, wall + 1.05, 2.42),
        Vec3::Z,
        1,
        0.8,
        0.9,
        0.0,
        WHITE,
    );
    p.gable_roof(
        roof,
        Mat::Paint,
        c + v3(0.0, wall + 1.8, 1.8),
        1.6,
        1.3,
        0.8,
        0.15,
        true,
        Vec4::ONE,
        WHITE,
    );
    // Chimney stacks on both gables.
    for x in [-l * 0.5 + 0.5, l * 0.5 - 0.5] {
        p.cuboid(
            Mat::Brick,
            c + v3(x - 0.45, wall, -0.6),
            c + v3(x + 0.45, wall + 4.0, 0.6),
            shade(bc, 0.9),
            false,
        );
        for k in 0..3 {
            let z = -0.35 + k as f32 * 0.35;
            p.cylinder(
                Mat::Clay,
                c + v3(x, wall + 4.0, z),
                0.12,
                0.1,
                0.5,
                8,
                Vec4::ONE,
                true,
            );
        }
    }
    p.door(
        c + v3(-2.6, 0.5, w * 0.5),
        Vec3::Z,
        1.1,
        2.3,
        hexc(0x6b1e1e),
    );
    p.windows(c + v3(0.8, 1.9, w * 0.5), Vec3::Z, 2, 1.0, 1.5, 1.2, WHITE);
    p.windows(c + v3(0.0, 4.6, w * 0.5), Vec3::Z, 3, 1.0, 1.4, 1.2, WHITE);
    p.windows(
        c + v3(0.0, 1.9, -w * 0.5),
        -Vec3::Z,
        3,
        1.0,
        1.4,
        1.2,
        WHITE,
    );
    p.windows(
        c + v3(0.0, 4.6, -w * 0.5),
        -Vec3::Z,
        3,
        1.0,
        1.4,
        1.2,
        WHITE,
    );
    // Yard wall at the back.
    p.cuboid(
        Mat::Brick,
        v3(-5.8, 0.0, -5.8),
        v3(5.8, 1.6, -5.5),
        shade(bc, 0.85),
        false,
    );
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

fn harbour() -> Parts {
    let mut p = Parts::default();
    // Warehouse on the land half (z from -8 to -1).
    let (l, w, wall) = (15.0, 7.0, 6.5);
    let c = v3(0.0, 0.0, -4.4);
    plinth(&mut p, l + 0.4, w + 0.4, 0.6);
    p.cuboid(
        Mat::Brick,
        c + v3(-l * 0.5, 0.6, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        Vec4::ONE,
        false,
    );
    for x in [-l * 0.5, -l * 0.25, 0.0, l * 0.25, l * 0.5] {
        p.cuboid(
            Mat::Brick,
            c + v3(x - 0.35, 0.6, w * 0.5 - 0.05),
            c + v3(x + 0.35, wall, w * 0.5 + 0.25),
            shade(Vec4::ONE, 0.85),
            false,
        );
    }
    p.hip_roof(
        Mat::Slate,
        c + v3(0.0, wall, 0.0),
        l,
        w,
        3.2,
        0.45,
        Vec4::ONE,
    );
    for x in [-l * 0.375, l * 0.125] {
        p.door(c + v3(x, 0.6, w * 0.5), Vec3::Z, 2.6, 3.4, hexc(0x3f5a3a));
    }
    p.windows(c + v3(0.0, 4.6, w * 0.5), Vec3::Z, 4, 1.1, 1.1, 2.6, WHITE);
    p.windows(c + v3(l * 0.5, 3.0, 0.0), Vec3::X, 2, 1.0, 1.2, 1.8, WHITE);
    p.windows(
        c + v3(-l * 0.5, 3.0, 0.0),
        -Vec3::X,
        2,
        1.0,
        1.2,
        1.8,
        WHITE,
    );
    // Lettered board.
    p.cuboid(
        Mat::Paint,
        c + v3(-3.0, wall - 1.2, w * 0.5 + 0.3),
        c + v3(3.0, wall - 0.4, w * 0.5 + 0.38),
        hexc(0x1c2e3e),
        true,
    );
    // Quay: planks on piles from z = -1 to the front edge.
    let deck_y = 0.0;
    p.cuboid(
        Mat::Planks,
        v3(-12.0, deck_y - 0.35, -1.0),
        v3(12.0, deck_y, 8.0),
        Vec4::ONE,
        true,
    );
    p.cuboid(
        Mat::Stone,
        v3(-12.0, -3.0, -1.4),
        v3(12.0, deck_y, -0.6),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    let mut x = -11.5;
    while x <= 11.6 {
        for z in [1.5, 4.5, 7.7] {
            p.rod(
                Mat::Timber,
                v3(x, -8.0, z),
                v3(x, deck_y - 0.3, z),
                0.16,
                6,
                shade(Vec4::ONE, 0.6),
            );
        }
        x += 2.3;
    }
    // Bollards and a timber kerb.
    p.beam(
        Mat::Timber,
        v3(-12.0, deck_y, 7.85),
        v3(12.0, deck_y, 7.85),
        0.22,
        shade(Vec4::ONE, 0.7),
    );
    for x in [-10.0, -4.0, 2.0, 8.0] {
        p.cylinder(
            Mat::Iron,
            v3(x, deck_y, 7.3),
            0.18,
            0.15,
            0.6,
            10,
            Vec4::ONE,
            true,
        );
    }
    // Jib crane.
    let base = v3(9.5, deck_y, 5.0);
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
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

fn fishery() -> Parts {
    let mut p = Parts::default();
    // Shed at the back on the beach.
    let c = v3(0.0, 0.0, -3.0);
    plinth(&mut p, 6.4, 4.4, 0.3);
    p.cuboid(
        Mat::Planks,
        c + v3(-3.0, 0.3, -2.0),
        c + v3(3.0, 2.8, 2.0),
        hexc(0x9a3b2a),
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Planks,
        c + v3(0.0, 2.8, 0.0),
        6.0,
        4.0,
        2.0,
        0.35,
        false,
        Vec4::ONE,
        hexc(0x9a3b2a),
    );
    p.door(c + v3(1.2, 0.3, 2.0), Vec3::Z, 1.4, 2.0, hexc(0x2a3a3a));
    p.windows(c + v3(-1.4, 1.6, 2.0), Vec3::Z, 1, 0.8, 0.7, 0.0, WHITE);
    // Jetty out to the front.
    p.cuboid(
        Mat::Planks,
        v3(-1.2, -0.25, -0.5),
        v3(1.2, 0.0, 6.0),
        Vec4::ONE,
        true,
    );
    for z in [0.5, 3.0, 5.6] {
        for x in [-1.0, 1.0] {
            p.rod(
                Mat::Timber,
                v3(x, -6.0, z),
                v3(x, -0.2, z),
                0.12,
                6,
                shade(Vec4::ONE, 0.6),
            );
        }
    }
    // Drying racks.
    for i in 0..2 {
        let x = -4.2 + i as f32 * 8.4;
        for z in [-1.0, 2.0] {
            p.beam(Mat::Timber, v3(x, 0.0, z), v3(x, 1.8, z), 0.1, Vec4::ONE);
        }
        p.beam(
            Mat::Timber,
            v3(x, 1.7, -1.0),
            v3(x, 1.7, 2.0),
            0.08,
            Vec4::ONE,
        );
        for k in 0..6 {
            let z = -0.8 + k as f32 * 0.5;
            p.box_at(
                Mat::Canvas,
                v3(x, 1.35, z),
                v3(0.03, 0.3, 0.08),
                0.0,
                hexc(0x8a8f8a),
            );
        }
    }
    // A rowboat on the beach.
    p.add_parts(
        &small_boat(),
        Mat4::from_rotation_translation(Quat::from_rotation_y(0.4), v3(3.5, 0.05, 2.8)),
    );
    p.ground_occlusion(0.6, 1.4);
    p.finish()
}

/// A 4 m rowboat (open hull from three planks a side).
pub fn small_boat() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0x5a6b72);
    let n = 8;
    let mut prev: Option<[Vec3; 3]> = None;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = (t - 0.5) * 4.0;
        let half = 0.75 * (1.0 - (2.0 * t - 1.0).powi(2)).max(0.02).sqrt();
        let sec = [v3(x, 0.55, half), v3(x, 0.0, 0.0), v3(x, 0.55, -half)];
        if let Some(pr) = prev {
            p.quad(Mat::Planks, pr[0], sec[0], sec[1], pr[1], col);
            p.quad(Mat::Planks, pr[1], sec[1], sec[2], pr[2], col);
        }
        prev = Some(sec);
    }
    p.box_at(
        Mat::Planks,
        v3(0.0, 0.35, 0.0),
        v3(0.15, 0.03, 0.6),
        0.0,
        Vec4::ONE,
    );
    p
}

fn woodcutter() -> Parts {
    let mut p = Parts::default();
    let (l, w) = (5.0, 4.0);
    plinth(&mut p, l, w, 0.25);
    // Log walls: stacked rods.
    let col = hexc(0x6b4a30);
    let mut y = 0.4;
    while y < 2.6 {
        p.rod(
            Mat::Timber,
            v3(-l * 0.5 - 0.2, y, w * 0.5),
            v3(l * 0.5 + 0.2, y, w * 0.5),
            0.17,
            6,
            col,
        );
        p.rod(
            Mat::Timber,
            v3(-l * 0.5 - 0.2, y, -w * 0.5),
            v3(l * 0.5 + 0.2, y, -w * 0.5),
            0.17,
            6,
            col,
        );
        p.rod(
            Mat::Timber,
            v3(l * 0.5, y + 0.17, -w * 0.5 - 0.2),
            v3(l * 0.5, y + 0.17, w * 0.5 + 0.2),
            0.17,
            6,
            col,
        );
        p.rod(
            Mat::Timber,
            v3(-l * 0.5, y + 0.17, -w * 0.5 - 0.2),
            v3(-l * 0.5, y + 0.17, w * 0.5 + 0.2),
            0.17,
            6,
            col,
        );
        y += 0.34;
    }
    p.cuboid(
        Mat::Timber,
        v3(-l * 0.5 + 0.1, 0.25, -w * 0.5 + 0.1),
        v3(l * 0.5 - 0.1, 2.6, w * 0.5 - 0.1),
        shade(col, 0.6),
        false,
    );
    p.gable_roof(
        Mat::Planks,
        Mat::Timber,
        v3(0.0, 2.6, 0.0),
        l,
        w,
        1.8,
        0.5,
        false,
        shade(Vec4::ONE, 0.75),
        col,
    );
    p.door(
        v3(0.6, 0.25, w * 0.5 + 0.15),
        Vec3::Z,
        0.9,
        1.9,
        hexc(0x4a3a2a),
    );
    // Chopping block with an axe.
    p.cylinder(
        Mat::Timber,
        v3(-2.0, 0.0, 3.0),
        0.35,
        0.33,
        0.6,
        10,
        col,
        true,
    );
    p.ground_occlusion(0.6, 1.2);
    p.finish()
}

fn log_pile() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0x7a5a3c);
    for row in 0..3 {
        let n = 4 - row;
        for i in 0..n {
            let z = (i as f32 - (n as f32 - 1.0) * 0.5) * 0.5;
            let y = 0.25 + row as f32 * 0.42;
            p.rod(Mat::Timber, v3(-2.0, y, z), v3(2.0, y, z), 0.24, 7, col);
        }
    }
    p.finish()
}

fn plank_stack() -> Parts {
    let mut p = Parts::default();
    for k in 0..6 {
        let y = k as f32 * 0.16;
        p.cuboid(
            Mat::Planks,
            v3(-2.2, y + 0.1, -0.6),
            v3(2.2, y + 0.24, 0.6),
            hexc(0xd8b890),
            true,
        );
    }
    p.cuboid(
        Mat::Timber,
        v3(-2.0, 0.0, -0.7),
        v3(-1.8, 0.1, 0.7),
        Vec4::ONE,
        false,
    );
    p.cuboid(
        Mat::Timber,
        v3(1.8, 0.0, -0.7),
        v3(2.0, 0.1, 0.7),
        Vec4::ONE,
        false,
    );
    p.finish()
}

fn sawmill() -> Parts {
    let mut p = Parts::default();
    // Open hall on posts.
    let (l, w) = (7.0, 10.0);
    plinth(&mut p, l, w, 0.2);
    for x in [-l * 0.5, 0.0, l * 0.5] {
        for z in [-w * 0.5, -w * 0.17, w * 0.17, w * 0.5] {
            p.beam(Mat::Timber, v3(x, 0.2, z), v3(x, 4.0, z), 0.3, Vec4::ONE);
        }
    }
    p.cuboid(
        Mat::Planks,
        v3(-l * 0.5, 0.2, -w * 0.5),
        v3(l * 0.5, 4.0, -w * 0.5 + 0.12),
        shade(Vec4::ONE, 0.8),
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Planks,
        v3(0.0, 4.0, 0.0),
        w,
        l,
        2.6,
        0.5,
        true,
        Vec4::ONE,
        Vec4::ONE,
    );
    // Saw bench and a log on it.
    p.cuboid(
        Mat::Timber,
        v3(-0.6, 0.2, -3.5),
        v3(0.6, 1.0, 3.5),
        Vec4::ONE,
        false,
    );
    p.rod(
        Mat::Timber,
        v3(0.0, 1.3, -3.4),
        v3(0.0, 1.3, 3.4),
        0.3,
        8,
        hexc(0x7a5a3c),
    );
    p.cylinder(
        Mat::Iron,
        v3(0.0, 0.6, 0.0),
        0.7,
        0.7,
        0.04,
        16,
        Vec4::ONE,
        true,
    );
    // Office with a chimney.
    p.cuboid(
        Mat::Planks,
        v3(-l * 0.5 - 2.6, 0.2, 2.0),
        v3(-l * 0.5, 2.8, 5.0),
        hexc(0x9a6a40),
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Planks,
        v3(-l * 0.5 - 1.3, 2.8, 3.5),
        2.6,
        3.0,
        1.4,
        0.3,
        true,
        Vec4::ONE,
        hexc(0x9a6a40),
    );
    p.ground_occlusion(0.6, 1.5);
    p.finish()
}

fn weaver() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (9.0, 12.0, 6.0);
    plinth(&mut p, l, w, 0.5);
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5, 0.5, -w * 0.5),
        v3(l * 0.5, wall, w * 0.5),
        hexc(0xe0c8b0),
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(0.0, wall, 0.0),
        w,
        l,
        3.2,
        0.4,
        true,
        Vec4::ONE,
        hexc(0xe0c8b0),
    );
    for (n, side) in [(Vec3::X, l * 0.5), (-Vec3::X, -l * 0.5)] {
        p.windows(v3(side, 2.2, 0.0), n, 4, 1.1, 1.6, 1.6, WHITE);
        p.windows(v3(side, 4.6, 0.0), n, 4, 1.1, 1.4, 1.6, WHITE);
    }
    p.door(v3(0.0, 0.5, w * 0.5), Vec3::Z, 2.0, 2.8, hexc(0x2a4a6a));
    p.windows(v3(0.0, 4.6, w * 0.5), Vec3::Z, 2, 1.0, 1.4, 2.0, WHITE);
    // Dye vats.
    for k in 0..3 {
        let col = [hexc(0x2a3a7a), hexc(0x7a2a2a), hexc(0x8a7a2a)][k];
        p.cylinder(
            Mat::Timber,
            v3(-3.0 + k as f32 * 2.2, 0.0, w * 0.5 + 2.6),
            0.8,
            0.8,
            1.0,
            12,
            col,
            true,
        );
    }
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

fn farmstead() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (8.0, 5.5, 3.2);
    plinth(&mut p, l, w, 0.4);
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, 0.4, -w * 0.5),
        v3(l * 0.5, wall, w * 0.5),
        plaster_col(0),
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        v3(0.0, wall, 0.0),
        l,
        w,
        3.2,
        0.45,
        false,
        Vec4::ONE,
        plaster_col(0),
    );
    p.cuboid(
        Mat::Brick,
        v3(2.4, wall, -0.4),
        v3(3.2, wall + 3.8, 0.4),
        Vec4::ONE,
        false,
    );
    p.door(v3(0.0, 0.4, w * 0.5), Vec3::Z, 1.0, 2.0, hexc(0x3a5a3a));
    p.windows(v3(0.0, 1.8, w * 0.5), Vec3::Z, 2, 0.9, 1.0, 2.6, WHITE);
    p.ground_occlusion(0.55, 1.6);
    p.finish()
}

fn barn() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (10.0, 7.0, 4.0);
    plinth(&mut p, l, w, 0.3);
    let red = hexc(0x8a2a1e);
    p.cuboid(
        Mat::Planks,
        v3(-l * 0.5, 0.3, -w * 0.5),
        v3(l * 0.5, wall, w * 0.5),
        red,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3(0.0, wall, 0.0),
        l,
        w,
        3.6,
        0.5,
        false,
        Vec4::ONE,
        red,
    );
    p.cuboid(
        Mat::Planks,
        v3(-1.6, 0.3, w * 0.5),
        v3(1.6, 3.4, w * 0.5 + 0.08),
        shade(red, 0.7),
        true,
    );
    p.beam(
        Mat::Paint,
        v3(-1.6, 0.4, w * 0.5 + 0.1),
        v3(1.6, 3.3, w * 0.5 + 0.1),
        0.12,
        WHITE,
    );
    p.beam(
        Mat::Paint,
        v3(1.6, 0.4, w * 0.5 + 0.1),
        v3(-1.6, 3.3, w * 0.5 + 0.1),
        0.12,
        WHITE,
    );
    // Hay bales.
    for k in 0..3 {
        p.box_at(
            Mat::Thatch,
            v3(-3.5 + k as f32 * 1.3, 0.45, w * 0.5 + 1.5),
            v3(0.6, 0.45, 0.45),
            0.1 * k as f32,
            hexc(0xc8a860),
        );
    }
    p.ground_occlusion(0.55, 1.6);
    p.finish()
}

fn fence_6() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0x8a7a66);
    for x in [-3.0, 0.0] {
        p.beam(
            Mat::Timber,
            v3(x, -0.3, 0.0),
            v3(x + 0.05, 1.1, 0.0),
            0.12,
            col,
        );
    }
    p.beam(
        Mat::Timber,
        v3(-3.0, 0.95, 0.0),
        v3(3.0, 0.9, 0.0),
        0.08,
        col,
    );
    p.beam(
        Mat::Timber,
        v3(-3.0, 0.5, 0.0),
        v3(3.0, 0.52, 0.0),
        0.08,
        col,
    );
    p.finish()
}

fn sheep() -> Parts {
    let mut p = Parts::default();
    // Body: a fluffy ellipsoid from a lathe along X; head and legs.
    let prof: Vec<(f32, f32)> = (0..=8)
        .map(|i| {
            let t = i as f32 / 8.0;
            (
                -0.55 + t * 1.1,
                0.36 * (1.0 - (2.0 * t - 1.0).powi(2)).max(0.0).sqrt() + 0.001,
            )
        })
        .collect();
    let mut body = Parts::default();
    body.lathe_x(Mat::Wool, &prof, 10, |_, _| Vec4::ONE);
    p.add_parts(&body, Mat4::from_translation(v3(0.0, 0.75, 0.0)));
    let dark = hexc(0x2a2420);
    p.box_at(
        Mat::Iron,
        v3(0.68, 0.85, 0.0),
        v3(0.17, 0.13, 0.11),
        0.0,
        dark,
    );
    for (x, z) in [(-0.35, -0.15), (-0.35, 0.15), (0.35, -0.15), (0.35, 0.15)] {
        p.beam(Mat::Iron, v3(x, 0.0, z), v3(x, 0.5, z), 0.07, dark);
    }
    p.finish()
}

fn wheat_row() -> Parts {
    let mut p = Parts::default();
    // Crossed blade cards along a 6 m row, 0.9 m tall, golden.
    let gold = hexc(0xd8b060);
    let mut x = -3.0;
    let mut k = 0;
    while x < 3.0 {
        let h = 0.85 + 0.15 * ((k * 37 % 11) as f32 / 11.0);
        for a in [0.4f32, -0.4] {
            let d = v3(a.sin(), 0.0, a.cos()) * 0.35;
            let base = v3(x, 0.0, 0.0);
            p.quad(
                Mat::Crop,
                base - d,
                base + d,
                base + d + Vec3::Y * h,
                base - d + Vec3::Y * h,
                gold.with_w(1.0),
            );
        }
        x += 0.35;
        k += 1;
    }
    p.finish()
}

fn hop_pole() -> Parts {
    let mut p = Parts::default();
    p.rod(
        Mat::Timber,
        v3(0.0, 0.0, 0.0),
        v3(0.0, 5.2, 0.0),
        0.06,
        5,
        Vec4::ONE,
    );
    // Bines climbing it: leafy cards spiralling up.
    let green = hexc(0x4a7a2a);
    for k in 0..10 {
        let y = 0.6 + k as f32 * 0.45;
        let a = k as f32 * 1.7;
        let d = v3(a.cos(), 0.0, a.sin()) * 0.4;
        let n = v3(-a.sin(), 0.0, a.cos()) * 0.4;
        p.quad(
            Mat::Leaf,
            v3(0.0, y, 0.0) - n,
            v3(0.0, y, 0.0) + n,
            v3(0.0, y + 0.7, 0.0) + n + d * 0.3,
            v3(0.0, y + 0.7, 0.0) - n + d * 0.3,
            shade(green, 0.85 + 0.03 * k as f32),
        );
    }
    p.finish()
}

fn oast() -> Parts {
    let mut p = Parts::default();
    // Round kiln with a conical roof and a white cowl, beside a drying barn.
    p.cylinder(
        Mat::Brick,
        v3(0.0, -2.0, 0.0),
        2.4,
        2.4,
        6.0,
        20,
        Vec4::ONE,
        false,
    );
    p.cylinder(
        Mat::RoofTile,
        v3(0.0, 4.0, 0.0),
        2.6,
        0.45,
        4.2,
        20,
        Vec4::ONE,
        false,
    );
    p.cylinder(
        Mat::Paint,
        v3(0.0, 8.1, 0.0),
        0.45,
        0.4,
        1.0,
        10,
        WHITE,
        true,
    );
    p.box_at(
        Mat::Paint,
        v3(0.2, 8.9, 0.35),
        v3(0.42, 0.35, 0.05),
        0.0,
        WHITE,
    );
    p.beam(
        Mat::Paint,
        v3(0.0, 9.1, 0.0),
        v3(0.9, 9.3, 0.0),
        0.08,
        WHITE,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-7.0, 0.0, -2.0),
        v3(-2.2, 4.0, 2.0),
        plaster_col(0),
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        v3(-4.6, 4.0, 0.0),
        4.8,
        4.0,
        2.4,
        0.3,
        false,
        Vec4::ONE,
        plaster_col(0),
    );
    p.windows(v3(-4.6, 2.0, 2.0), Vec3::Z, 2, 0.8, 0.9, 1.0, WHITE);
    p.ground_occlusion(0.55, 1.6);
    p.finish()
}

fn mill_tower() -> Parts {
    let mut p = Parts::default();
    plinth(&mut p, 6.0, 6.0, 0.3);
    // Tapered octagonal tower, tarred boarding over a brick base.
    p.cylinder(
        Mat::Brick,
        v3(0.0, 0.3, 0.0),
        2.9,
        2.7,
        2.5,
        8,
        Vec4::ONE,
        false,
    );
    p.cylinder(
        Mat::Planks,
        v3(0.0, 2.8, 0.0),
        2.7,
        1.8,
        8.5,
        8,
        hexc(0x3a3a38),
        false,
    );
    // Stage (gallery) round the base of the boarding.
    p.cylinder(
        Mat::Planks,
        v3(0.0, 2.7, 0.0),
        3.8,
        3.8,
        0.15,
        8,
        Vec4::ONE,
        true,
    );
    // Boat-shaped cap.
    let mut cap = Parts::default();
    cap.gable_roof(
        Mat::Planks,
        Mat::Planks,
        v3(0.0, 0.0, 0.0),
        4.2,
        3.6,
        1.8,
        0.1,
        true,
        hexc(0xdad6c8),
        hexc(0xdad6c8),
    );
    p.add_parts(&cap, Mat4::from_translation(v3(0.0, 11.3, 0.0)));
    p.door(v3(0.0, 0.3, 2.85), Vec3::Z, 1.0, 1.9, hexc(0x5a3a2a));
    p.windows(v3(0.0, 6.5, 2.35), Vec3::Z, 1, 0.6, 0.8, 0.0, WHITE);
    // Sacks at the door.
    for k in 0..3 {
        p.box_at(
            Mat::Canvas,
            v3(-2.0 + k as f32 * 0.7, 0.35, 3.6),
            v3(0.3, 0.35, 0.22),
            0.3 * k as f32,
            hexc(0xd8c8a0),
        );
    }
    p.ground_occlusion(0.55, 1.6);
    p.finish()
}

/// The four sails of a windmill, about their hub (turning about local Z).
pub fn mill_sails() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Iron,
        v3(0.0, 0.0, -0.3),
        0.35,
        0.3,
        0.6,
        10,
        Vec4::ONE,
        true,
    );
    for k in 0..4 {
        let a = k as f32 * PI * 0.5;
        let dir = v3(a.cos(), a.sin(), 0.0);
        let side = v3(-a.sin(), a.cos(), 0.0);
        p.beam(Mat::Timber, Vec3::ZERO, dir * 9.0, 0.22, Vec4::ONE);
        // Lattice and canvas.
        for i in 1..9 {
            let r = i as f32 + 0.5;
            p.beam(Mat::Timber, dir * r, dir * r + side * 1.6, 0.06, Vec4::ONE);
        }
        p.quad(
            Mat::Canvas,
            dir * 1.5 + v3(0.0, 0.0, 0.05),
            dir * 9.0 + v3(0.0, 0.0, 0.05),
            dir * 9.0 + side * 1.6 + v3(0.0, 0.0, 0.05),
            dir * 1.5 + side * 1.6 + v3(0.0, 0.0, 0.05),
            Vec4::ONE,
        );
    }
    p.finish()
}

fn bakery() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (8.0, 6.5, 4.0);
    plinth(&mut p, l, w, 0.4);
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5, 0.4, -w * 0.5),
        v3(l * 0.5, wall, w * 0.5),
        hexc(0xf0d8c8),
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Brick,
        v3(0.0, wall, 0.0),
        l,
        w,
        3.0,
        0.4,
        false,
        Vec4::ONE,
        hexc(0xf0d8c8),
    );
    // Oven bulge and its stack.
    p.cylinder(
        Mat::Brick,
        v3(-l * 0.5 - 0.2, 0.0, 0.0),
        1.6,
        1.2,
        2.4,
        12,
        Vec4::ONE,
        true,
    );
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5 - 0.6, 2.0, -0.5),
        v3(-l * 0.5 + 0.4, wall + 5.0, 0.5),
        Vec4::ONE,
        false,
    );
    p.door(v3(1.4, 0.4, w * 0.5), Vec3::Z, 1.1, 2.1, hexc(0x8a5a2a));
    p.windows(v3(-1.6, 1.8, w * 0.5), Vec3::Z, 1, 1.6, 1.2, 0.0, WHITE);
    // Awning over the shop window.
    p.quad(
        Mat::Canvas,
        v3(-2.8, 2.9, w * 0.5),
        v3(-0.4, 2.9, w * 0.5),
        v3(-0.4, 2.4, w * 0.5 + 1.2),
        v3(-2.8, 2.4, w * 0.5 + 1.2),
        hexc(0xb03a2a),
    );
    p.ground_occlusion(0.55, 1.6);
    p.finish()
}

fn brewery() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (11.0, 9.0, 6.5);
    plinth(&mut p, l, w, 0.5);
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5, 0.5, -w * 0.5),
        v3(l * 0.5, wall, w * 0.5),
        Vec4::ONE,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(0.0, wall, 0.0),
        l,
        w,
        3.6,
        0.4,
        false,
        Vec4::ONE,
        Vec4::ONE,
    );
    // Louvred vent on the ridge.
    p.cuboid(
        Mat::Planks,
        v3(-1.5, wall + 3.0, -0.8),
        v3(1.5, wall + 4.4, 0.8),
        WHITE,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3(0.0, wall + 4.4, 0.0),
        3.0,
        1.6,
        0.8,
        0.2,
        false,
        Vec4::ONE,
        WHITE,
    );
    // Tall square chimney.
    p.cuboid(
        Mat::Brick,
        v3(l * 0.5 - 1.6, 0.5, -w * 0.5 + 0.2),
        v3(l * 0.5 - 0.2, 16.0, -w * 0.5 + 1.6),
        shade(Vec4::ONE, 0.9),
        false,
    );
    p.cuboid(
        Mat::Brick,
        v3(l * 0.5 - 1.8, 15.6, -w * 0.5),
        v3(l * 0.5, 16.2, -w * 0.5 + 1.8),
        shade(Vec4::ONE, 0.75),
        false,
    );
    p.door(v3(-2.0, 0.5, w * 0.5), Vec3::Z, 2.4, 3.0, hexc(0x2a3a2a));
    p.windows(v3(2.5, 2.4, w * 0.5), Vec3::Z, 2, 1.0, 1.6, 1.4, WHITE);
    p.windows(v3(0.0, 4.9, w * 0.5), Vec3::Z, 4, 0.9, 1.2, 1.4, WHITE);
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

fn clay_pit() -> Parts {
    let mut p = Parts::default();
    // A shallow excavation, two heaps and a drying shed.
    p.cuboid(
        Mat::Clay,
        v3(-6.5, -0.5, -6.5),
        v3(2.5, -0.15, 3.0),
        Vec4::ONE,
        false,
    );
    for (x, z, r) in [(-3.0, -2.0, 2.0), (0.0, 1.5, 1.5)] {
        p.cylinder(
            Mat::Clay,
            v3(x, -0.2, z),
            r,
            0.2,
            r * 0.8,
            12,
            shade(Vec4::ONE, 1.1),
            true,
        );
    }
    for x in [3.6, 7.0] {
        for z in [-6.0, -1.0, 4.0] {
            p.beam(Mat::Timber, v3(x, 0.0, z), v3(x, 2.4, z), 0.16, Vec4::ONE);
        }
    }
    p.gable_roof(
        Mat::Thatch,
        Mat::Planks,
        v3(5.3, 2.4, -1.0),
        10.5,
        3.4,
        1.4,
        0.3,
        true,
        Vec4::ONE,
        Vec4::ONE,
    );
    for k in 0..4 {
        p.cuboid(
            Mat::Clay,
            v3(4.0, 0.2 + k as f32 * 0.5, -5.5),
            v3(6.6, 0.3 + k as f32 * 0.5, 3.5),
            shade(Vec4::ONE, 0.9),
            true,
        );
    }
    p.finish()
}

fn brickworks() -> Parts {
    let mut p = Parts::default();
    plinth(&mut p, 14.0, 8.0, 0.3);
    // Long kiln with arched firing holes, a tall round stack.
    p.cuboid(
        Mat::Brick,
        v3(-7.0, 0.3, -3.0),
        v3(4.0, 3.6, 3.0),
        shade(Vec4::ONE, 0.85),
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(-1.5, 3.6, 0.0),
        11.0,
        6.0,
        1.6,
        0.3,
        false,
        Vec4::ONE,
        Vec4::ONE,
    );
    for k in 0..5 {
        let x = -6.0 + k as f32 * 2.2;
        p.quad(
            Mat::Window,
            v3(x - 0.4, 0.4, 3.03),
            v3(x + 0.4, 0.4, 3.03),
            v3(x + 0.4, 1.4, 3.03),
            v3(x - 0.4, 1.4, 3.03),
            Vec4::ONE,
        );
    }
    p.cylinder(
        Mat::Brick,
        v3(5.5, 0.3, -1.5),
        1.2,
        0.8,
        19.0,
        14,
        Vec4::ONE,
        false,
    );
    p.cylinder(
        Mat::Brick,
        v3(5.5, 19.0, -1.5),
        0.95,
        0.95,
        0.6,
        14,
        shade(Vec4::ONE, 0.7),
        false,
    );
    // Stacks of finished bricks.
    for k in 0..4 {
        let x = -6.0 + k as f32 * 2.6;
        p.cuboid(
            Mat::Brick,
            v3(x, 0.0, 4.2),
            v3(x + 1.8, 1.2, 5.6),
            hexc(0xf0b0a0),
            true,
        );
    }
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

fn market_plate() -> Parts {
    let mut p = Parts::default();
    p.cuboid(
        Mat::Cobble,
        v3(-9.8, -0.6, -9.8),
        v3(9.8, 0.08, 9.8),
        Vec4::ONE,
        false,
    );
    p.cuboid(
        Mat::Stone,
        v3(-10.0, -0.6, -10.0),
        v3(10.0, 0.04, -9.7),
        Vec4::ONE,
        false,
    );
    p.finish()
}

fn well() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Stone,
        v3(0.0, 0.0, 0.0),
        1.1,
        1.1,
        0.9,
        16,
        Vec4::ONE,
        true,
    );
    p.cylinder(
        Mat::Window,
        v3(0.0, 0.0, 0.0),
        0.85,
        0.85,
        0.92,
        16,
        Vec4::ONE,
        true,
    );
    for x in [-0.9, 0.9] {
        p.beam(
            Mat::Timber,
            v3(x, 0.9, 0.0),
            v3(x, 2.6, 0.0),
            0.14,
            Vec4::ONE,
        );
    }
    p.gable_roof(
        Mat::RoofTile,
        Mat::Timber,
        v3(0.0, 2.6, 0.0),
        2.4,
        1.6,
        0.9,
        0.15,
        false,
        Vec4::ONE,
        Vec4::ONE,
    );
    p.finish()
}

fn lamp_post() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Iron,
        v3(0.0, 0.0, 0.0),
        0.09,
        0.06,
        3.6,
        8,
        hexc(0x202424),
        true,
    );
    p.cylinder(
        Mat::Lamp,
        v3(0.0, 3.6, 0.0),
        0.18,
        0.22,
        0.45,
        6,
        Vec4::ONE,
        true,
    );
    p.cylinder(
        Mat::Iron,
        v3(0.0, 4.05, 0.0),
        0.28,
        0.0,
        0.25,
        6,
        hexc(0x202424),
        false,
    );
    p.finish()
}

fn chapel() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (7.5, 13.0, 6.5);
    let c = v3(0.0, 0.0, -1.5);
    plinth(&mut p, l + 0.5, w + 0.5, 0.6);
    let stone = hexc(0xc8c0b0);
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5, 0.6, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        stone,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Stone,
        c + v3(0.0, wall, 0.0),
        w,
        l,
        4.6,
        0.35,
        true,
        Vec4::ONE,
        stone,
    );
    // Buttresses and tall windows.
    for k in 0..4 {
        let z = -w * 0.5 + 1.6 + k as f32 * 3.2;
        for (x, n) in [(l * 0.5, 1.0f32), (-l * 0.5, -1.0)] {
            let (x0, x1) = if n > 0.0 { (x, x + 0.6) } else { (x - 0.6, x) };
            p.cuboid(
                Mat::Stone,
                c + v3(x0, 0.6, z - 0.35),
                c + v3(x1, wall - 1.5, z + 0.35),
                stone,
                false,
            );
            if k < 3 {
                p.windows(
                    c + v3(x, 3.6, z + 1.6),
                    v3(n, 0.0, 0.0),
                    1,
                    1.0,
                    3.0,
                    0.0,
                    hexc(0x8a8070),
                );
            }
        }
    }
    // Tower with a spire at the front.
    let t = c + v3(0.0, 0.0, w * 0.5 + 2.0);
    p.cuboid(
        Mat::Stone,
        t + v3(-2.2, 0.6, -2.2),
        t + v3(2.2, 15.0, 2.2),
        stone,
        false,
    );
    p.cuboid(
        Mat::Stone,
        t + v3(-2.4, 14.6, -2.4),
        t + v3(2.4, 15.3, 2.4),
        shade(stone, 0.9),
        true,
    );
    for n in [Vec3::X, -Vec3::X, Vec3::Z, -Vec3::Z] {
        p.windows(
            t + n * 2.2 + Vec3::Y * 12.5,
            n,
            1,
            1.0,
            1.8,
            0.0,
            hexc(0x8a8070),
        );
    }
    p.cylinder(
        Mat::Copper,
        t + Vec3::Y * 15.3,
        2.3,
        0.05,
        9.0,
        8,
        Vec4::ONE,
        false,
    );
    p.beam(
        Mat::Iron,
        t + Vec3::Y * 24.0,
        t + Vec3::Y * 25.6,
        0.08,
        hexc(0x8a7a3a),
    );
    p.beam(
        Mat::Iron,
        t + v3(-0.5, 25.0, 0.0),
        t + v3(0.5, 25.0, 0.0),
        0.08,
        hexc(0x8a7a3a),
    );
    p.door(t + v3(0.0, 0.6, 2.2), Vec3::Z, 1.6, 3.0, hexc(0x4a2a1a));
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

fn airship_mast() -> Parts {
    let mut p = Parts::default();
    let steel = hexc(0x4a5258);
    plinth(&mut p, 8.0, 8.0, 0.3);
    // Lattice tower, 36 m, square, tapering.
    let levels = 9;
    let h = 36.0;
    let corner = |y: f32, k: usize| {
        let half = 3.2 - 2.0 * (y / h);
        let (sx, sz) = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)][k];
        v3(sx * half, y, sz * half)
    };
    for k in 0..4 {
        p.beam(Mat::Iron, corner(0.3, k), corner(h, k), 0.3, steel);
    }
    for i in 0..levels {
        let y0 = 0.3 + i as f32 * (h - 0.3) / levels as f32;
        let y1 = y0 + (h - 0.3) / levels as f32;
        for k in 0..4 {
            let k2 = (k + 1) % 4;
            p.beam(Mat::Iron, corner(y1, k), corner(y1, k2), 0.14, steel);
            p.beam(Mat::Iron, corner(y0, k), corner(y1, k2), 0.08, steel);
            p.beam(Mat::Iron, corner(y0, k2), corner(y1, k), 0.08, steel);
        }
    }
    // Mooring cup and a beacon.
    p.cylinder(Mat::Iron, v3(0.0, h, 0.0), 1.4, 1.6, 1.6, 12, steel, true);
    p.cylinder(
        Mat::Iron,
        v3(0.0, h + 1.6, 0.0),
        0.3,
        0.1,
        1.6,
        8,
        steel,
        false,
    );
    p.cylinder(
        Mat::Lamp,
        v3(0.0, h + 3.2, 0.0),
        0.25,
        0.25,
        0.35,
        8,
        Vec4::ONE,
        true,
    );
    // Terminal shed and gas tanks.
    p.cuboid(
        Mat::Brick,
        v3(-9.0, 0.0, 4.0),
        v3(-3.5, 3.6, 9.0),
        Vec4::ONE,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(-6.25, 3.6, 6.5),
        5.5,
        5.0,
        2.0,
        0.3,
        false,
        Vec4::ONE,
        Vec4::ONE,
    );
    p.windows(v3(-6.25, 1.9, 9.0), Vec3::Z, 2, 1.0, 1.2, 1.2, WHITE);
    for x in [4.5, 7.5] {
        p.cylinder(
            Mat::Paint,
            v3(x, 0.0, 6.5),
            1.3,
            1.3,
            4.5,
            16,
            hexc(0x9aa49a),
            true,
        );
    }
    p.ground_occlusion(0.6, 1.5);
    p.finish()
}

fn crate_model() -> Parts {
    let mut p = Parts::default();
    p.cuboid(
        Mat::Planks,
        v3(-0.5, 0.0, -0.5),
        v3(0.5, 0.9, 0.5),
        hexc(0xc8a070),
        false,
    );
    for y in [0.05, 0.85] {
        p.cuboid(
            Mat::Timber,
            v3(-0.52, y - 0.05, -0.52),
            v3(0.52, y + 0.05, 0.52),
            Vec4::ONE,
            false,
        );
    }
    p.finish()
}

fn barrel() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Timber,
        v3(0.0, 0.0, 0.0),
        0.3,
        0.36,
        0.45,
        12,
        hexc(0x8a6040),
        false,
    );
    p.cylinder(
        Mat::Timber,
        v3(0.0, 0.45, 0.0),
        0.36,
        0.3,
        0.45,
        12,
        hexc(0x8a6040),
        true,
    );
    for y in [0.1, 0.8] {
        p.cylinder(
            Mat::Iron,
            v3(0.0, y, 0.0),
            0.335,
            0.335,
            0.06,
            12,
            Vec4::ONE,
            false,
        );
    }
    p.finish()
}

fn plot() -> Parts {
    // A kitchen garden behind a cottage: rows of greens.
    let mut p = Parts::default();
    p.cuboid(
        Mat::Clay,
        v3(-4.5, -0.3, -1.8),
        v3(4.5, 0.05, 1.8),
        shade(Vec4::ONE, 0.6),
        false,
    );
    for k in 0..5 {
        let z = -1.4 + k as f32 * 0.7;
        let mut x = -4.0;
        while x < 4.0 {
            let c = v3(x, 0.05, z);
            p.quad(
                Mat::Leaf,
                c + v3(-0.25, 0.0, 0.0),
                c + v3(0.25, 0.0, 0.0),
                c + v3(0.2, 0.4, 0.1),
                c + v3(-0.2, 0.4, 0.1),
                hexc(0x3a6a2a),
            );
            p.quad(
                Mat::Leaf,
                c + v3(0.0, 0.0, -0.25),
                c + v3(0.0, 0.0, 0.25),
                c + v3(0.1, 0.4, 0.2),
                c + v3(0.1, 0.4, -0.2),
                hexc(0x4a7a2a),
            );
            x += 0.6;
        }
    }
    p.finish()
}

/// Build one model by key.
pub fn build_model(key: ModelKey) -> Parts {
    match key {
        "cottage_0" => cottage(0),
        "cottage_1" => cottage(1),
        "cottage_2" => cottage(2),
        "townhouse_0" => townhouse(0),
        "townhouse_1" => townhouse(1),
        "townhouse_2" => townhouse(2),
        "harbour" => harbour(),
        "fishery" => fishery(),
        "woodcutter" => woodcutter(),
        "sawmill" => sawmill(),
        "weaver" => weaver(),
        "farmstead" => farmstead(),
        "barn" => barn(),
        "fence_6" => fence_6(),
        "sheep" => sheep(),
        "wheat_row" => wheat_row(),
        "hop_pole" => hop_pole(),
        "oast" => oast(),
        "mill_tower" => mill_tower(),
        "bakery" => bakery(),
        "brewery" => brewery(),
        "clay_pit" => clay_pit(),
        "brickworks" => brickworks(),
        "market_plate" => market_plate(),
        "well" => well(),
        "chapel" => chapel(),
        "airship_mast" => airship_mast(),
        "log_pile" => log_pile(),
        "plank_stack" => plank_stack(),
        "crate" => crate_model(),
        "barrel" => barrel(),
        "lamp_post" => lamp_post(),
        "plot" => plot(),
        _ => Parts::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_builds_within_budget() {
        for key in MODELS {
            let p = build_model(key);
            let tris = p.triangles();
            assert!(tris < 40_000, "{key}: {tris} triangles");
            for m in p.meshes.values() {
                for v in &m.vertices {
                    assert!(v.position.iter().all(|c| c.is_finite()), "{key}");
                }
            }
        }
    }

    #[test]
    fn every_kind_has_a_look() {
        for d in crate::defs::DEFS {
            if d.kind == Kind::Road {
                continue;
            }
            let l = look(d.kind, Tier::Fisherfolk, 7);
            assert!(!l.is_empty(), "{:?}", d.kind);
            for piece in &l {
                assert!(MODELS.contains(&piece.model), "{}", piece.model);
            }
        }
    }
}
