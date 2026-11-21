//! The works of the far regions' own peoples, in the same art kit as `regional` and
//! `patrician`: plantain groves, cotton fields and calico looms for Sarrowmere's Riverfolk,
//! Freeholders and Planters; goat pens, robe looms, tea gardens and carpet looms for the
//! Amber Reach's Herders, Caravaners and Scholars.
//!
//! Models stand in a building's own space (origin at the middle of the footprint at floor
//! height, front toward +Z).

use std::f32::consts::TAU;

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::civic::{dome, sheet};
use crate::defs::Kind;
use crate::looks::{ModelKey, Piece, h01};
use crate::regional::{mud, parapet};

/// The models this module builds (they are listed in `looks::MODELS`; a test checks that).
#[cfg(test)]
pub const MODELS: &[ModelKey] = &[
    "plantain",
    "plantain_shed",
    "cotton_bush",
    "cotton_barn",
    "calico_loom",
    "dye_vat",
    "cloth_rack",
    "goat",
    "goat_shelter",
    "stone_wall_4",
    "trough",
    "robe_loom",
    "robe_line",
    "tea_row",
    "terrace_6",
    "tea_house",
    "carpet_loom",
    "carpet_frame",
];

pub fn build(key: ModelKey) -> Option<Parts> {
    Some(match key {
        "plantain" => plantain(),
        "plantain_shed" => plantain_shed(),
        "cotton_bush" => cotton_bush(),
        "cotton_barn" => cotton_barn(),
        "calico_loom" => calico_loom(),
        "dye_vat" => dye_vat(),
        "cloth_rack" => cloth_rack(),
        "goat" => goat(),
        "goat_shelter" => goat_shelter(),
        "stone_wall_4" => stone_wall_4(),
        "trough" => trough(),
        "robe_loom" => robe_loom(),
        "robe_line" => robe_line(),
        "tea_row" => tea_row(),
        "terrace_6" => terrace_6(),
        "tea_house" => tea_house(),
        "carpet_loom" => carpet_loom(),
        "carpet_frame" => carpet_frame(),
        _ => return None,
    })
}

/// Bright cloth: the dyes of calico, robes and carpets.
const DYES: [u32; 6] = [0x2c3f8a, 0xb0302a, 0xd9a23a, 0x2f7a5a, 0x7a2e6a, 0xe0d2b0];

fn dye(k: u64) -> Vec4 {
    hexc(DYES[k as usize % DYES.len()])
}

/// What a building of this module draws: false for kinds it does not know.
pub fn look(kind: Kind, seed: u64, hw: f32, hd: f32, out: &mut Vec<Piece>) -> bool {
    let r = |k: u64| h01(seed, 1300 + k);
    match kind {
        Kind::PlantainGrove => {
            // A shed for the bunches in the front left corner; plantains in loose rows on
            // the rest of the lot, taller and shorter.
            out.push(Piece::at("plantain_shed", v3(-hw + 4.2, 0.0, hd - 3.6)));
            out.push(Piece::at("hand_cart", v3(-hw + 9.4, 0.0, hd - 2.0)).yaw(0.4));
            let (nx, nz) = (6, 5);
            for i in 0..nx {
                for j in 0..nz {
                    let x = -hw + 2.2 + i as f32 * (hw * 2.0 - 4.4) / (nx - 1) as f32;
                    let z = -hd + 2.0 + j as f32 * (hd * 2.0 - 4.0) / (nz - 1) as f32;
                    if x < -hw + 11.0 && z > hd - 7.0 {
                        continue;
                    }
                    let k = (i * nz + j) as u64;
                    let at = v3(x + (r(k) - 0.5) * 1.6, 0.0, z + (r(40 + k) - 0.5) * 1.4);
                    out.push(
                        Piece::at("plantain", at)
                            .yaw(r(80 + k) * TAU)
                            .scale(1.15 + 0.5 * r(120 + k))
                            .tint([0.9 + 0.15 * r(160 + k), 1.0, 0.9 + 0.1 * r(200 + k)]),
                    );
                }
            }
        }
        Kind::CottonField => {
            out.push(Piece::at("cotton_barn", v3(-hw + 5.0, 0.0, hd - 4.0)));
            // Rows of bushes across the lot, white with open bolls.
            let rows = 9;
            for j in 0..rows {
                let z = -hd + 1.6 + j as f32 * (hd * 2.0 - 3.2) / (rows - 1) as f32;
                let mut x = -hw + 1.4;
                let mut k = 0u64;
                while x < hw - 1.0 {
                    if !(x < -hw + 11.0 && z > hd - 8.5) {
                        let kk = j as u64 * 40 + k;
                        out.push(
                            Piece::at("cotton_bush", v3(x, 0.0, z + (r(kk) - 0.5) * 0.4))
                                .yaw(r(500 + kk) * TAU)
                                .scale(0.85 + 0.3 * r(900 + kk)),
                        );
                    }
                    x += 1.9 + 0.4 * r(300 + k);
                    k += 1;
                }
            }
        }
        Kind::CalicoLoom => {
            out.push(Piece::at("calico_loom", v3(0.0, 0.0, -2.0)));
            for (k, x) in [-5.6f32, -3.6, -1.6].into_iter().enumerate() {
                out.push(Piece::at("dye_vat", v3(x, 0.0, 4.4)).tint({
                    let c = dye(k as u64);
                    [c.x * 2.0, c.y * 2.0, c.z * 2.0]
                }));
            }
            out.push(Piece::at("cloth_rack", v3(3.6, 0.0, 4.2)).yaw(r(0) * 0.2));
        }
        Kind::GoatPen => {
            // Dry-stone walls round most of the lot, a shelter at the back, goats about.
            let (x0, x1, z0, z1) = (-hw + 0.6, hw - 0.6, -hd + 0.6, hd - 0.6);
            let mut wall = |a: Vec3, b: Vec3| {
                let d = b - a;
                let n = (d.length() / 4.0).round().max(1.0) as usize;
                let yaw = -d.z.atan2(d.x);
                for s in 0..n {
                    let t = (s as f32 + 0.5) / n as f32;
                    out.push(
                        Piece::at("stone_wall_4", a + d * t)
                            .yaw(yaw)
                            .scale(d.length() / (4.0 * n as f32)),
                    );
                }
            };
            wall(v3(x0, 0.0, z0), v3(x1, 0.0, z0));
            wall(v3(x0, 0.0, z0), v3(x0, 0.0, z1));
            wall(v3(x1, 0.0, z0), v3(x1, 0.0, z1));
            // (A gap in the front wall for the herd to come and go.)
            wall(v3(x0, 0.0, z1), v3(-2.4, 0.0, z1));
            wall(v3(2.4, 0.0, z1), v3(x1, 0.0, z1));
            out.push(Piece::at("goat_shelter", v3(-2.0, 0.0, -hd + 4.2)));
            out.push(Piece::at("trough", v3(5.6, 0.0, 1.0)).yaw(0.3));
            for k in 0..11u64 {
                let a = r(10 + k) * TAU;
                let d = 2.0 + 7.0 * r(30 + k);
                let at = v3(a.cos() * d * 1.1, 0.0, 1.6 + a.sin() * d * 0.7);
                let coat = match (r(50 + k) * 4.0) as u32 {
                    0 => [0.32, 0.26, 0.22],
                    1 => [1.0, 0.98, 0.95],
                    2 => [0.62, 0.45, 0.3],
                    _ => [0.85, 0.78, 0.66],
                };
                out.push(
                    Piece::at("goat", at)
                        .yaw(r(70 + k) * TAU)
                        .scale(0.85 + 0.3 * r(90 + k))
                        .tint(coat),
                );
            }
        }
        Kind::RobeLoom => {
            out.push(Piece::at("robe_loom", v3(0.0, 0.0, -2.6)));
            out.push(Piece::at("robe_line", v3(0.0, 0.0, 5.4)));
            out.push(Piece::at("jar", v3(-4.6, 0.0, 5.8)).scale(1.1));
            out.push(Piece::at("jar", v3(-4.0, 0.0, 6.6)).scale(0.8));
        }
        Kind::TeaGarden => {
            // Terraces climbing from the front to the back, each a row of clipped bushes,
            // and a pavilion where the leaf is withered and rolled.
            let steps = 4;
            let depth = (hd * 2.0 - 2.0) / steps as f32;
            for s in 0..steps {
                let z = hd - 1.0 - depth * (s as f32 + 0.5);
                let y = 0.55 * s as f32;
                for x in [-hw * 0.5, hw * 0.5] {
                    out.push(Piece::at("terrace_6", v3(x, y, z)).scale(hw / 6.0).tint([
                        0.95 + 0.08 * r(s as u64),
                        1.0,
                        1.0,
                    ]));
                }
                for row in 0..2 {
                    let zz = z - depth * 0.22 + row as f32 * depth * 0.44;
                    for x in [-hw * 0.5, hw * 0.5] {
                        if s == 0 && x > 0.0 {
                            continue;
                        }
                        out.push(
                            Piece::at("tea_row", v3(x, y + 0.55, zz))
                                .scale(hw / 6.4)
                                .tint([0.92 + 0.12 * r(20 + (s * 2 + row) as u64), 1.0, 0.95]),
                        );
                    }
                }
            }
            out.push(Piece::at(
                "tea_house",
                v3(hw * 0.5, 0.0, hd - depth * 0.5 - 0.6),
            ));
        }
        Kind::CarpetLoom => {
            out.push(Piece::at("carpet_loom", v3(0.0, 0.0, -2.2)));
            out.push(Piece::at("carpet_frame", v3(-4.0, 0.0, 5.2)).yaw(0.1));
            out.push(
                Piece::at("carpet_frame", v3(3.8, 0.0, 5.6))
                    .yaw(-0.15)
                    .tint([0.85, 0.9, 1.15]),
            );
        }
        _ => return false,
    }
    true
}

// ----------------------------------------------------------------- Sarrowmere: plantains

/// A plantain: a soft green stem, a crown of long ragged leaves, and a bunch hanging under
/// them on its purple bell.
fn plantain() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Leaf,
        v3(0.0, 0.0, 0.0),
        0.24,
        0.15,
        2.6,
        8,
        hexc(0x6f8a3a),
        false,
    );
    let green = hexc(0x4f9030);
    for i in 0..8 {
        let a = i as f32 * 0.82 + 0.2;
        let dir = v3(a.cos(), 0.0, a.sin());
        let side = v3(-a.sin(), 0.0, a.cos());
        let up = 0.9 + 0.3 * ((i * 5 % 3) as f32);
        let top = v3(0.0, 2.45, 0.0);
        let pts = [
            top,
            top + dir * 0.8 + Vec3::Y * up,
            top + dir * 1.9 + Vec3::Y * (up + 0.3),
            top + dir * 2.9 + Vec3::Y * (up - 0.6),
        ];
        // (Old leaves split and brown at the tips.)
        let col = if i % 4 == 3 {
            hexc(0x8a8a40)
        } else {
            shade(green, 0.82 + 0.05 * i as f32)
        };
        for k in 0..pts.len() - 1 {
            let w = [0.16, 0.8, 0.86, 0.24];
            let (a0, b0) = (pts[k], pts[k + 1]);
            let (wa, wb) = (side * w[k] * 0.5, side * w[k + 1] * 0.5);
            sheet(&mut p, Mat::Leaf, a0 - wa, a0 + wa, b0 + wb, b0 - wb, col);
        }
    }
    // The bunch: rings of green fingers on a hanging stalk, the bell below.
    let stalk = v3(0.35, 2.3, 0.1);
    p.rod(
        Mat::Leaf,
        stalk,
        stalk + v3(0.25, -0.9, 0.0),
        0.04,
        5,
        hexc(0x5a7030),
    );
    for k in 0..4 {
        let c = stalk + v3(0.1 + 0.06 * k as f32, -0.2 - 0.18 * k as f32, 0.0);
        p.cylinder(
            Mat::Leaf,
            c - Vec3::Y * 0.1,
            0.2 - 0.02 * k as f32,
            0.22 - 0.02 * k as f32,
            0.16,
            8,
            hexc(0x8aa040),
            false,
        );
    }
    p.cylinder(
        Mat::Paint,
        stalk + v3(0.3, -1.05, 0.0),
        0.0,
        0.12,
        0.28,
        6,
        hexc(0x6a2a48),
        false,
    );
    p.finish()
}

/// Four poles and a thatched roof: green bunches hang from the beams, a table under them.
fn plantain_shed() -> Parts {
    let mut p = Parts::default();
    let pole = hexc(0x6a4a2e);
    for x in [-2.6f32, 2.6] {
        for z in [-1.8f32, 1.8] {
            p.rod(Mat::Timber, v3(x, 0.0, z), v3(x, 2.5, z), 0.12, 6, pole);
        }
    }
    for z in [-1.8f32, 1.8] {
        p.beam(Mat::Timber, v3(-2.8, 2.5, z), v3(2.8, 2.5, z), 0.16, pole);
    }
    p.gable_roof(
        Mat::Thatch,
        Mat::Thatch,
        v3(0.0, 2.5, 0.0),
        5.6,
        3.6,
        1.6,
        0.6,
        false,
        hexc(0xb59a5a),
        hexc(0xa58a4a),
    );
    for k in 0..6 {
        let x = -2.0 + k as f32 * 0.8;
        let z = if k % 2 == 0 { -0.6 } else { 0.6 };
        p.rod(
            Mat::Leaf,
            v3(x, 2.4, z),
            v3(x, 1.8, z),
            0.03,
            4,
            hexc(0x5a7030),
        );
        p.cylinder(
            Mat::Leaf,
            v3(x, 1.25, z),
            0.2,
            0.24,
            0.6,
            8,
            shade(hexc(0x8aa040), 0.9 + 0.04 * (k % 3) as f32),
            true,
        );
    }
    p.cuboid(
        Mat::Planks,
        v3(-1.6, 0.75, 1.0),
        v3(1.6, 0.85, 1.6),
        hexc(0x8a6a48),
        false,
    );
    for x in [-1.4f32, 1.4] {
        p.cuboid(
            Mat::Planks,
            v3(x - 0.06, 0.0, 1.05),
            v3(x + 0.06, 0.75, 1.55),
            hexc(0x7a5a38),
            false,
        );
    }
    p.ground_occlusion(0.4, 1.4);
    p.finish()
}

/// A cotton bush: woody stems, dark leaves and white bolls bursting open.
fn cotton_bush() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Timber,
        Vec3::ZERO,
        0.06,
        0.04,
        0.4,
        5,
        hexc(0x5a4030),
        false,
    );
    for (k, (y, r)) in [(0.3f32, 0.48f32), (0.62, 0.5), (0.92, 0.32)]
        .into_iter()
        .enumerate()
    {
        p.cylinder(
            Mat::Leaf,
            v3(0.0, y, 0.0),
            r,
            r * 0.8,
            0.34,
            8,
            shade(hexc(0x3c5e2c), 0.9 + 0.06 * k as f32),
            k == 2,
        );
    }
    for k in 0..12 {
        let a = k as f32 * 2.4;
        let rr = 0.3 + 0.18 * ((k * 7 % 5) as f32 / 4.0);
        let y = 0.45 + 0.6 * ((k * 3 % 7) as f32 / 6.0);
        p.box_at(
            Mat::Wool,
            v3(a.cos() * rr, y, a.sin() * rr),
            v3(0.09, 0.08, 0.09),
            a,
            hexc(0xfaf8f0),
        );
    }
    p.finish()
}

/// A plank barn with an open front, bales of ginned cotton stacked inside and out.
fn cotton_barn() -> Parts {
    let mut p = Parts::default();
    let wall = hexc(0x9a7650);
    p.cuboid(
        Mat::Planks,
        v3(-3.6, 0.0, -2.6),
        v3(3.6, 3.2, -2.3),
        wall,
        false,
    );
    for x in [-3.6f32, 3.3] {
        p.cuboid(
            Mat::Planks,
            v3(x, 0.0, -2.6),
            v3(x + 0.3, 3.2, 2.4),
            wall,
            false,
        );
    }
    for x in [-1.2f32, 1.2] {
        p.rod(
            Mat::Timber,
            v3(x, 0.0, 2.3),
            v3(x, 3.2, 2.3),
            0.13,
            6,
            hexc(0x6a4a2e),
        );
    }
    p.gable_roof(
        Mat::Thatch,
        Mat::Planks,
        v3(0.0, 3.2, -0.1),
        7.2,
        5.0,
        2.0,
        0.6,
        false,
        hexc(0xb89c5c),
        wall,
    );
    let bale = |p: &mut Parts, c: Vec3, yaw: f32| {
        p.box_at(Mat::Wool, c, v3(0.6, 0.45, 0.45), yaw, hexc(0xf4f0e4));
        for d in [-0.3f32, 0.3] {
            let off = v3(yaw.cos() * d, 0.0, -yaw.sin() * d);
            p.box_at(
                Mat::Canvas,
                c + off,
                v3(0.04, 0.47, 0.47),
                yaw,
                hexc(0x8a7a5a),
            );
        }
    };
    for (k, c) in [
        v3(-2.4, 0.45, -1.6),
        v3(-1.1, 0.45, -1.6),
        v3(0.2, 0.45, -1.6),
        v3(-1.8, 1.35, -1.6),
        v3(-0.5, 1.35, -1.6),
        v3(2.0, 0.45, 3.2),
        v3(3.0, 0.45, 3.6),
    ]
    .into_iter()
    .enumerate()
    {
        bale(&mut p, c, 0.1 * (k % 3) as f32);
    }
    p.ground_occlusion(0.6, 1.8);
    p.finish()
}

/// The calico works: a long whitewashed workshop under red tiles, with a printing table seen
/// through the open bays and a loft door.
fn calico_loom() -> Parts {
    let mut p = Parts::default();
    let wall = hexc(0xf2ead8);
    let timber = hexc(0x5a3e28);
    let (l, w, h) = (14.0f32, 7.0f32, 4.2f32);
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, 0.0, -w * 0.5),
        v3(l * 0.5, h, w * 0.5),
        wall,
        false,
    );
    // A timber frame on the plaster, and a row of open bays.
    for k in 0..=6 {
        let x = -l * 0.5 + k as f32 * l / 6.0;
        p.cuboid(
            Mat::Timber,
            v3(x - 0.14, 0.0, w * 0.5),
            v3(x + 0.14, h, w * 0.5 + 0.08),
            timber,
            false,
        );
    }
    p.cuboid(
        Mat::Timber,
        v3(-l * 0.5, h - 0.3, w * 0.5),
        v3(l * 0.5, h, w * 0.5 + 0.1),
        timber,
        false,
    );
    for k in 0..6 {
        let x = -l * 0.5 + (k as f32 + 0.5) * l / 6.0;
        if k % 2 == 0 {
            p.cuboid(
                Mat::Window,
                v3(x - 0.75, 0.6, w * 0.5 + 0.02),
                v3(x + 0.75, 3.2, w * 0.5 + 0.06),
                hexc(0x2a2620),
                false,
            );
            // Cloth hanging in the bay.
            sheet(
                &mut p,
                Mat::Canvas,
                v3(x - 0.6, 3.1, w * 0.5 + 0.12),
                v3(x + 0.6, 3.1, w * 0.5 + 0.12),
                v3(x + 0.6, 1.2, w * 0.5 + 0.12),
                v3(x - 0.6, 1.2, w * 0.5 + 0.12),
                dye(k as u64 / 2),
            );
        } else {
            p.windows(
                v3(x, 2.4, w * 0.5 + 0.02),
                Vec3::Z,
                1,
                0.9,
                1.1,
                1.0,
                timber,
            );
        }
    }
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        v3(0.0, h, 0.0),
        l,
        w,
        2.6,
        0.6,
        false,
        hexc(0xa8563a),
        wall,
    );
    // A loft door with a hoist beam at one gable.
    p.cuboid(
        Mat::Planks,
        v3(l * 0.5, 4.4, -0.6),
        v3(l * 0.5 + 0.06, 5.8, 0.6),
        hexc(0x6a4a30),
        false,
    );
    p.beam(
        Mat::Timber,
        v3(l * 0.5, 6.2, 0.0),
        v3(l * 0.5 + 1.2, 6.2, 0.0),
        0.18,
        timber,
    );
    p.door(
        v3(-l * 0.5 + 1.2, 0.0, w * 0.5 + 0.06),
        Vec3::Z,
        1.2,
        2.4,
        timber,
    );
    p.ground_occlusion(0.7, 2.2);
    p.finish()
}

/// A dye vat: a stave tub of coloured liquor (the tint gives the colour), a paddle across it.
fn dye_vat() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Planks,
        Vec3::ZERO,
        0.8,
        0.86,
        1.0,
        14,
        hexc(0x7a5a3a),
        false,
    );
    for y in [0.22f32, 0.78] {
        p.cylinder(
            Mat::Iron,
            v3(0.0, y, 0.0),
            0.88,
            0.88,
            0.08,
            14,
            hexc(0x3a3a3a),
            false,
        );
    }
    p.cylinder(
        Mat::Brine,
        v3(0.0, 0.88, 0.0),
        0.8,
        0.8,
        0.02,
        14,
        hexc(0x808080),
        true,
    );
    p.beam(
        Mat::Timber,
        v3(-0.9, 1.04, 0.2),
        v3(0.9, 1.04, -0.2),
        0.06,
        hexc(0x8a6a48),
    );
    p.finish()
}

/// A drying rack: two frames and three poles with long lengths of printed calico.
fn cloth_rack() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x6a4a2e);
    for x in [-2.8f32, 2.8] {
        p.rod(
            Mat::Timber,
            v3(x, 0.0, -0.8),
            v3(x, 3.0, -0.8),
            0.08,
            5,
            wood,
        );
        p.rod(Mat::Timber, v3(x, 0.0, 0.8), v3(x, 3.0, 0.8), 0.08, 5, wood);
    }
    for (k, z) in [-0.7f32, 0.0, 0.7].into_iter().enumerate() {
        p.rod(
            Mat::Timber,
            v3(-2.9, 2.9, z),
            v3(2.9, 2.9, z),
            0.05,
            5,
            wood,
        );
        for s in 0..2 {
            let x0 = -2.6 + s as f32 * 2.7;
            let c = dye((k * 2 + s) as u64);
            sheet(
                &mut p,
                Mat::Canvas,
                v3(x0, 2.88, z),
                v3(x0 + 2.4, 2.88, z),
                v3(x0 + 2.4, 0.6 + 0.2 * k as f32, z + 0.05),
                v3(x0, 0.6 + 0.2 * k as f32, z + 0.05),
                c,
            );
            // A printed band across each length.
            sheet(
                &mut p,
                Mat::Paint,
                v3(x0, 1.9, z + 0.02),
                v3(x0 + 2.4, 1.9, z + 0.02),
                v3(x0 + 2.4, 1.6, z + 0.03),
                v3(x0, 1.6, z + 0.03),
                shade(dye((k * 2 + s + 2) as u64), 1.1),
            );
        }
    }
    p.finish()
}

// ------------------------------------------------------- the Amber Reach: goats and robes

/// A long-haired goat, standing, its head up; the tint gives the coat.
fn goat() -> Parts {
    let mut p = Parts::default();
    let coat = hexc(0xf0ece4);
    let horn = hexc(0x4a3a2a);
    p.box_at(
        Mat::Wool,
        v3(0.0, 0.62, 0.0),
        v3(0.48, 0.22, 0.2),
        0.0,
        coat,
    );
    // The shaggy skirt of long hair under the belly.
    p.box_at(
        Mat::Wool,
        v3(0.0, 0.42, 0.0),
        v3(0.44, 0.08, 0.21),
        0.0,
        shade(coat, 0.9),
    );
    for (x, z) in [
        (-0.34f32, -0.12f32),
        (-0.34, 0.12),
        (0.34, -0.12),
        (0.34, 0.12),
    ] {
        p.rod(
            Mat::Wool,
            v3(x, 0.0, z),
            v3(x, 0.45, z),
            0.045,
            5,
            shade(coat, 0.75),
        );
    }
    // Neck, head, ears, beard and the swept-back horns.
    p.rod(
        Mat::Wool,
        v3(0.42, 0.7, 0.0),
        v3(0.58, 0.95, 0.0),
        0.1,
        6,
        coat,
    );
    p.box_at(
        Mat::Wool,
        v3(0.68, 0.98, 0.0),
        v3(0.16, 0.09, 0.08),
        -0.35,
        coat,
    );
    p.box_at(
        Mat::Wool,
        v3(0.7, 0.82, 0.0),
        v3(0.04, 0.08, 0.03),
        0.0,
        shade(coat, 0.8),
    );
    for z in [-0.08f32, 0.08] {
        p.rod(
            Mat::Wool,
            v3(0.6, 1.02, z),
            v3(0.56, 1.0, z * 2.6),
            0.03,
            4,
            coat,
        );
        p.rod(
            Mat::Iron,
            v3(0.62, 1.06, z * 0.6),
            v3(0.46, 1.3, z * 1.2),
            0.025,
            4,
            horn,
        );
        p.rod(
            Mat::Iron,
            v3(0.46, 1.3, z * 1.2),
            v3(0.34, 1.22, z * 1.6),
            0.02,
            4,
            horn,
        );
    }
    p.rod(
        Mat::Wool,
        v3(-0.46, 0.76, 0.0),
        v3(-0.56, 0.92, 0.0),
        0.035,
        4,
        coat,
    );
    p.finish()
}

/// A mud-brick shelter, open at the front under a pole-and-brush roof, with hay inside.
fn goat_shelter() -> Parts {
    let mut p = Parts::default();
    let col = mud(1);
    p.cuboid(
        Mat::Clay,
        v3(-4.0, 0.0, -1.8),
        v3(4.0, 2.4, -1.5),
        col,
        false,
    );
    for x in [-4.0f32, 3.7] {
        p.cuboid(
            Mat::Clay,
            v3(x, 0.0, -1.8),
            v3(x + 0.3, 2.2, 1.8),
            col,
            false,
        );
    }
    // Brush roof on poles.
    for x in [-3.0f32, -1.0, 1.0, 3.0] {
        p.rod(
            Mat::Timber,
            v3(x, 2.1, -1.9),
            v3(x, 2.1, 2.2),
            0.07,
            5,
            hexc(0x7a5a38),
        );
    }
    p.quad(
        Mat::Thatch,
        v3(-4.3, 2.18, 2.2),
        v3(4.3, 2.18, 2.2),
        v3(4.3, 2.5, -2.0),
        v3(-4.3, 2.5, -2.0),
        hexc(0xa88c58),
    );
    for x in [-2.8f32, 2.8] {
        p.rod(
            Mat::Timber,
            v3(x, 0.0, 1.9),
            v3(x, 2.1, 1.9),
            0.1,
            6,
            hexc(0x6a4a2e),
        );
    }
    p.cylinder(
        Mat::Thatch,
        v3(-2.2, 0.0, -0.6),
        0.9,
        0.5,
        0.8,
        9,
        hexc(0xc8aa66),
        true,
    );
    p.cylinder(
        Mat::Thatch,
        v3(1.6, 0.0, -0.8),
        0.7,
        0.4,
        0.6,
        9,
        hexc(0xbea060),
        true,
    );
    p.ground_occlusion(0.5, 1.6);
    p.finish()
}

/// Four meters of dry-stone wall, knee high, with a ragged top.
fn stone_wall_4() -> Parts {
    let mut p = Parts::default();
    let base = hexc(0xb8a68a);
    p.cuboid(
        Mat::Plaster,
        v3(-2.0, 0.0, -0.3),
        v3(2.0, 0.7, 0.3),
        base,
        false,
    );
    for k in 0..6 {
        let x = -1.7 + k as f32 * 0.68;
        p.box_at(
            Mat::Plaster,
            v3(x, 0.8 + 0.05 * (k % 2) as f32, 0.0),
            v3(0.3, 0.12, 0.26),
            0.15 * (k % 3) as f32,
            shade(base, 0.9 + 0.05 * (k % 3) as f32),
        );
    }
    p.finish()
}

/// A stone water trough.
fn trough() -> Parts {
    let mut p = Parts::default();
    let c = hexc(0xa8987c);
    p.cuboid(
        Mat::Plaster,
        v3(-1.2, 0.0, -0.4),
        v3(1.2, 0.6, 0.4),
        c,
        false,
    );
    p.cuboid(
        Mat::Brine,
        v3(-1.05, 0.5, -0.28),
        v3(1.05, 0.62, 0.28),
        hexc(0x6a8aa0),
        false,
    );
    p.finish()
}

/// The robe works: a flat-roofed mud-brick hall behind a parapet, a striped awning over the
/// looms at its door, and a wind tower on the roof.
fn robe_loom() -> Parts {
    let mut p = Parts::default();
    let col = mud(0);
    let (l, w, h) = (10.0f32, 8.0f32, 4.0f32);
    p.cuboid(
        Mat::Clay,
        v3(-l * 0.5, 0.0, -w * 0.5),
        v3(l * 0.5, h, w * 0.5),
        col,
        false,
    );
    parapet(&mut p, v3(0.0, h, 0.0), l, w, 0.7, shade(col, 1.04));
    // The wind tower.
    p.cuboid(
        Mat::Clay,
        v3(-4.2, h, -3.4),
        v3(-2.4, h + 3.4, -1.6),
        shade(col, 0.96),
        false,
    );
    for z in [-3.0f32, -2.0] {
        p.cuboid(
            Mat::Window,
            v3(-4.24, h + 2.2, z - 0.25),
            v3(-2.36, h + 3.0, z + 0.25),
            hexc(0x2a2018),
            false,
        );
    }
    // Arched door and small high windows.
    p.door(
        v3(1.6, 0.0, w * 0.5 + 0.02),
        Vec3::Z,
        1.6,
        2.6,
        hexc(0x5a3a22),
    );
    p.cylinder(
        Mat::Clay,
        v3(1.6, 2.6, w * 0.5 + 0.02),
        0.95,
        0.0,
        0.7,
        8,
        shade(col, 0.9),
        false,
    );
    p.windows(
        v3(-2.4, 3.0, w * 0.5 + 0.02),
        Vec3::Z,
        2,
        0.6,
        0.6,
        1.4,
        hexc(0x8a6a48),
    );
    // A striped awning on poles over the looms out front.
    let stripes = 6;
    for s in 0..stripes {
        let x0 = -l * 0.5 + 0.4 + s as f32 * (l - 0.8) / stripes as f32;
        let x1 = x0 + (l - 0.8) / stripes as f32;
        sheet(
            &mut p,
            Mat::Canvas,
            v3(x0, 3.2, w * 0.5),
            v3(x1, 3.2, w * 0.5),
            v3(x1, 2.6, w * 0.5 + 2.6),
            v3(x0, 2.6, w * 0.5 + 2.6),
            if s % 2 == 0 { hexc(0xe8dcc0) } else { dye(0) },
        );
    }
    for x in [-l * 0.5 + 0.4, l * 0.5 - 0.4] {
        p.rod(
            Mat::Timber,
            v3(x, 0.0, w * 0.5 + 2.6),
            v3(x, 2.6, w * 0.5 + 2.6),
            0.07,
            5,
            hexc(0x6a4a2e),
        );
    }
    // A ground loom under the awning: two beams and the warp between them.
    p.beam(
        Mat::Timber,
        v3(-3.6, 0.2, w * 0.5 + 0.6),
        v3(-0.6, 0.2, w * 0.5 + 0.6),
        0.12,
        hexc(0x7a5a38),
    );
    p.beam(
        Mat::Timber,
        v3(-3.6, 0.2, w * 0.5 + 2.2),
        v3(-0.6, 0.2, w * 0.5 + 2.2),
        0.12,
        hexc(0x7a5a38),
    );
    p.quad(
        Mat::Wool,
        v3(-3.5, 0.24, w * 0.5 + 2.2),
        v3(-0.7, 0.24, w * 0.5 + 2.2),
        v3(-0.7, 0.24, w * 0.5 + 0.6),
        v3(-3.5, 0.24, w * 0.5 + 0.6),
        hexc(0x3a2e26),
    );
    p.ground_occlusion(0.7, 2.0);
    p.finish()
}

/// A line of robes drying between two posts.
fn robe_line() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x6a4a2e);
    for x in [-4.4f32, 4.4] {
        p.rod(Mat::Timber, v3(x, 0.0, 0.0), v3(x, 2.6, 0.0), 0.08, 5, wood);
    }
    p.rod(
        Mat::Timber,
        v3(-4.4, 2.5, 0.0),
        v3(4.4, 2.5, 0.0),
        0.025,
        4,
        hexc(0x8a7a5a),
    );
    let cols = [0x3a2e26, 0xe8dcc0, 0x2c3f8a, 0x8a5a3a, 0xf0ece4];
    for (k, c) in cols.into_iter().enumerate() {
        let x = -3.4 + k as f32 * 1.7;
        let col = hexc(c);
        // Shoulders and sleeves, then the long skirt.
        sheet(
            &mut p,
            Mat::Wool,
            v3(x - 0.75, 2.48, 0.0),
            v3(x + 0.75, 2.48, 0.0),
            v3(x + 0.75, 2.1, 0.02),
            v3(x - 0.75, 2.1, 0.02),
            col,
        );
        sheet(
            &mut p,
            Mat::Wool,
            v3(x - 0.42, 2.12, 0.02),
            v3(x + 0.42, 2.12, 0.02),
            v3(x + 0.55, 0.7, 0.06),
            v3(x - 0.55, 0.7, 0.06),
            shade(col, 0.95),
        );
        // A woven border at the hem.
        sheet(
            &mut p,
            Mat::Paint,
            v3(x - 0.54, 0.95, 0.07),
            v3(x + 0.54, 0.95, 0.07),
            v3(x + 0.55, 0.78, 0.08),
            v3(x - 0.55, 0.78, 0.08),
            dye(k as u64 + 1),
        );
    }
    p.finish()
}

// ------------------------------------------------------- the Amber Reach: tea and carpets

/// Six meters of clipped tea bushes, a long rounded hedge with flat tops.
fn tea_row() -> Parts {
    let mut p = Parts::default();
    let green = hexc(0x2c5226);
    for k in 0..8 {
        let x = -2.6 + k as f32 * 0.75;
        p.cylinder(
            Mat::Leaf,
            v3(x, 0.0, 0.0),
            0.32,
            0.48,
            0.55,
            9,
            shade(green, 0.92 + 0.05 * (k % 3) as f32),
            false,
        );
        p.cylinder(
            Mat::Leaf,
            v3(x, 0.55, 0.0),
            0.48,
            0.44,
            0.14,
            9,
            shade(hexc(0x46742e), 0.92 + 0.08 * (k % 3) as f32),
            true,
        );
    }
    p.finish()
}

/// A terrace: six meters of stone retaining wall and the earth behind it.
fn terrace_6() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0xb09a7a);
    p.cuboid(
        Mat::Plaster,
        v3(-6.0, -0.6, 1.0),
        v3(6.0, 0.56, 1.3),
        stone,
        false,
    );
    for k in 0..10 {
        let x = -5.4 + k as f32 * 1.2;
        p.box_at(
            Mat::Plaster,
            v3(x, 0.0, 1.32),
            v3(0.5, 0.22, 0.04),
            0.0,
            shade(stone, 0.9 + 0.06 * (k % 3) as f32),
        );
    }
    p.cuboid(
        Mat::Clay,
        v3(-6.0, -0.6, -1.4),
        v3(6.0, 0.54, 1.0),
        hexc(0x8a6a48),
        false,
    );
    p.finish()
}

/// The withering house: a pavilion on a stone plinth under a sweeping hipped roof, trays of
/// leaf on racks inside.
fn tea_house() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0xc8b496);
    p.cuboid(
        Mat::Plaster,
        v3(-3.2, 0.0, -2.4),
        v3(3.2, 0.5, 2.4),
        stone,
        false,
    );
    for x in [-2.8f32, 0.0, 2.8] {
        for z in [-2.0f32, 2.0] {
            p.rod(
                Mat::Timber,
                v3(x, 0.5, z),
                v3(x, 3.2, z),
                0.14,
                8,
                hexc(0x7a3a2a),
            );
        }
    }
    p.hip_roof(
        Mat::RoofTile,
        v3(0.0, 3.2, 0.0),
        6.4,
        4.8,
        2.0,
        0.9,
        hexc(0xb06a48),
    );
    // Upturned corners.
    for (x, z) in [(-3.9f32, -3.2f32), (3.9, -3.2), (-3.9, 3.2), (3.9, 3.2)] {
        p.rod(
            Mat::RoofTile,
            v3(x * 0.85, 3.15, z * 0.85),
            v3(x, 3.5, z),
            0.08,
            5,
            hexc(0xb06a48),
        );
    }
    for k in 0..3 {
        let y = 0.9 + k as f32 * 0.6;
        p.cuboid(
            Mat::Planks,
            v3(-2.4, y, -1.4),
            v3(2.4, y + 0.06, 1.4),
            hexc(0x9a7a50),
            false,
        );
        p.cuboid(
            Mat::Leaf,
            v3(-2.2, y + 0.06, -1.2),
            v3(2.2, y + 0.12, 1.2),
            hexc(0x5a7a3a),
            false,
        );
    }
    p.ground_occlusion(0.5, 1.6);
    p.finish()
}

/// The carpet hall: a tall mud-brick hall with a dome over the middle, a pointed-arch door
/// and a row of arches along its front.
fn carpet_loom() -> Parts {
    let mut p = Parts::default();
    let col = mud(2);
    let (l, w, h) = (14.0f32, 9.0f32, 5.6f32);
    p.cuboid(
        Mat::Clay,
        v3(-l * 0.5, 0.0, -w * 0.5),
        v3(l * 0.5, h, w * 0.5),
        col,
        false,
    );
    parapet(&mut p, v3(0.0, h, 0.0), l, w, 0.6, shade(col, 1.05));
    p.cylinder(
        Mat::Clay,
        v3(0.0, h, 0.0),
        3.2,
        3.2,
        1.2,
        16,
        shade(col, 1.02),
        false,
    );
    dome(
        &mut p,
        Mat::Plaster,
        v3(0.0, h + 1.2, 0.0),
        3.2,
        3.0,
        hexc(0x3a7a8a),
    );
    p.cylinder(
        Mat::Copper,
        v3(0.0, h + 4.2, 0.0),
        0.12,
        0.0,
        1.0,
        6,
        hexc(0xc8a050),
        false,
    );
    // Front arcade: pointed arches, dark within, a carpet hung in the middle one.
    let n = 5;
    for k in 0..n {
        let x = -l * 0.5 + (k as f32 + 0.5) * l / n as f32;
        let (hw, top) = (0.9f32, 3.4f32);
        p.cuboid(
            Mat::Window,
            v3(x - hw, 0.0, w * 0.5 + 0.01),
            v3(x + hw, top - 0.6, w * 0.5 + 0.05),
            hexc(0x241c16),
            false,
        );
        p.tri(
            Mat::Window,
            v3(x - hw, top - 0.6, w * 0.5 + 0.05),
            v3(x + hw, top - 0.6, w * 0.5 + 0.05),
            v3(x, top, w * 0.5 + 0.05),
            hexc(0x241c16),
        );
        if k == n / 2 {
            carpet(&mut p, v3(x, 0.2, w * 0.5 + 0.1), 1.5, 2.4, 3);
        }
    }
    // A frieze of glazed tile under the parapet.
    p.cuboid(
        Mat::Paint,
        v3(-l * 0.5, h - 0.7, w * 0.5),
        v3(l * 0.5, h - 0.4, w * 0.5 + 0.05),
        hexc(0x2f6a8a),
        false,
    );
    p.ground_occlusion(0.8, 2.4);
    p.finish()
}

/// A carpet `w` wide and `h` tall hanging flat at `c` (its bottom middle), facing +Z: a
/// field, a border and a medallion.
fn carpet(p: &mut Parts, c: Vec3, w: f32, h: f32, k: u64) {
    let (a, b) = (dye(k + 1), dye(k));
    let z = c.z;
    let rect = |p: &mut Parts, x0: f32, y0: f32, x1: f32, y1: f32, dz: f32, col: Vec4| {
        sheet(
            p,
            Mat::Wool,
            v3(c.x + x0, c.y + y1, z + dz),
            v3(c.x + x1, c.y + y1, z + dz),
            v3(c.x + x1, c.y + y0, z + dz),
            v3(c.x + x0, c.y + y0, z + dz),
            col,
        );
    };
    let hw = w * 0.5;
    rect(p, -hw, 0.0, hw, h, 0.0, b);
    rect(p, -hw + 0.15, 0.15, hw - 0.15, h - 0.15, 0.01, a);
    rect(p, -hw + 0.3, 0.3, hw - 0.3, h - 0.3, 0.02, shade(b, 0.8));
    // The medallion: a diamond of the border colour.
    let m = v3(c.x, c.y + h * 0.5, z + 0.03);
    let (dx, dy) = (w * 0.24, h * 0.18);
    p.quad(
        Mat::Wool,
        m + v3(0.0, -dy, 0.0),
        m + v3(dx, 0.0, 0.0),
        m + v3(0.0, dy, 0.0),
        m + v3(-dx, 0.0, 0.0),
        a,
    );
    p.quad(
        Mat::Wool,
        m + v3(0.0, -dy, -0.06),
        m + v3(-dx, 0.0, -0.06),
        m + v3(0.0, dy, -0.06),
        m + v3(dx, 0.0, -0.06),
        a,
    );
}

/// An upright carpet loom in the yard: two posts, beams top and bottom, and a carpet half
/// knotted on its warp.
fn carpet_frame() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x6a4a2e);
    for x in [-1.6f32, 1.6] {
        p.cuboid(
            Mat::Timber,
            v3(x - 0.12, 0.0, -0.12),
            v3(x + 0.12, 3.2, 0.12),
            wood,
            false,
        );
        // Feet.
        p.cuboid(
            Mat::Timber,
            v3(x - 0.14, 0.0, -0.6),
            v3(x + 0.14, 0.18, 0.6),
            wood,
            false,
        );
    }
    for y in [0.35f32, 3.0] {
        p.rod(
            Mat::Timber,
            v3(-1.7, y, 0.0),
            v3(1.7, y, 0.0),
            0.09,
            6,
            wood,
        );
    }
    carpet(&mut p, v3(0.0, 0.42, 0.02), 2.8, 1.6, 4);
    // The bare warp above the knotted part.
    sheet(
        &mut p,
        Mat::Canvas,
        v3(-1.4, 2.95, 0.01),
        v3(1.4, 2.95, 0.01),
        v3(1.4, 2.02, 0.01),
        v3(-1.4, 2.02, 0.01),
        hexc(0xe8e0cc),
    );
    // A weaver's bench.
    p.cuboid(
        Mat::Planks,
        v3(-1.0, 0.4, 0.7),
        v3(1.0, 0.48, 1.1),
        hexc(0x8a6a48),
        false,
    );
    for x in [-0.8f32, 0.8] {
        p.cuboid(
            Mat::Planks,
            v3(x - 0.05, 0.0, 0.75),
            v3(x + 0.05, 0.4, 1.05),
            hexc(0x7a5a38),
            false,
        );
    }
    p.finish()
}
