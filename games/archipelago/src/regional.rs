//! Buildings of the far regions, in the same art kit as the home isles: stilt houses, cane
//! fields and a rum distillery for the jungle coast of Sarrowmere; adobe houses, date groves
//! and salt pans for the Amber Reach; turf cabins, trappers and an oil works for Frostholm.
//!
//! Models stand in a building's own space like those in `looks` (origin at the middle of the
//! footprint at floor height, front toward +Z).

use std::f32::consts::{PI, TAU};

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::defs::{CELL_M, Kind, Tier};
use crate::globe::Biome;
use crate::looks::{ModelKey, Piece, WHITE, h01, lump, shutters, small_boat, variant};

pub(crate) const SNOW: Vec4 = Vec4::new(0.95, 0.97, 1.0, 1.0);

/// The models this module builds (they are listed in `looks::MODELS`; this list checks that).
#[cfg(test)]
pub const MODELS: &[ModelKey] = &[
    "stilt_0",
    "stilt_1",
    "stilt_2",
    "gallery_0",
    "gallery_1",
    "gallery_2",
    "adobe_0",
    "adobe_1",
    "adobe_2",
    "riad_0",
    "riad_1",
    "riad_2",
    "cabin_0",
    "cabin_1",
    "cabin_2",
    "lodge_0",
    "lodge_1",
    "lodge_2",
    "cane_cell",
    "cane_shed",
    "distillery",
    "date_palm",
    "date_house",
    "mud_wall_4",
    "salt_pan",
    "salt_heap",
    "salt_house",
    "windpump",
    "trapper",
    "pelt_frame",
    "sled",
    "husky",
    "oil_works",
    "snow_drift",
    "fish_rack",
    "jar",
    "canoe",
    "banana",
];

/// Build one of this module's models.
pub fn build(key: ModelKey) -> Option<Parts> {
    Some(match key {
        "stilt_0" => stilt_house(0),
        "stilt_1" => stilt_house(1),
        "stilt_2" => stilt_house(2),
        "gallery_0" => gallery_house(0),
        "gallery_1" => gallery_house(1),
        "gallery_2" => gallery_house(2),
        "adobe_0" => adobe_house(0),
        "adobe_1" => adobe_house(1),
        "adobe_2" => adobe_house(2),
        "riad_0" => riad_house(0),
        "riad_1" => riad_house(1),
        "riad_2" => riad_house(2),
        "cabin_0" => turf_cabin(0),
        "cabin_1" => turf_cabin(1),
        "cabin_2" => turf_cabin(2),
        "lodge_0" => lodge_house(0),
        "lodge_1" => lodge_house(1),
        "lodge_2" => lodge_house(2),
        "cane_cell" => cane_cell(),
        "cane_shed" => cane_shed(),
        "distillery" => distillery(),
        "date_palm" => date_palm(),
        "date_house" => date_house(),
        "mud_wall_4" => mud_wall_4(),
        "salt_pan" => salt_pan(),
        "salt_heap" => salt_heap(),
        "salt_house" => salt_house(),
        "windpump" => windpump(),
        "trapper" => trapper(),
        "pelt_frame" => pelt_frame(),
        "sled" => sled(),
        "husky" => husky(),
        "oil_works" => oil_works(),
        "snow_drift" => snow_drift(),
        "fish_rack" => fish_rack(),
        "jar" => jar(),
        "canoe" => canoe(),
        "banana" => banana(),
        _ => return None,
    })
}

// ------------------------------------------------------------------------------- looks

/// The house model of a climate and tier (the home isles' own are in `looks`).
pub fn house_model(biome: Biome, tier: Tier, v: usize) -> Option<ModelKey> {
    let set: [ModelKey; 3] = match (biome, tier) {
        // (Burghers' houses of every climate are in `burgher`.)
        (Biome::Temperate, _) | (_, Tier::Burghers) => return None,
        (Biome::Tropical, Tier::Fisherfolk) => ["stilt_0", "stilt_1", "stilt_2"],
        (Biome::Tropical, Tier::Labourers) => ["gallery_0", "gallery_1", "gallery_2"],
        (Biome::Arid, Tier::Fisherfolk) => ["adobe_0", "adobe_1", "adobe_2"],
        (Biome::Arid, Tier::Labourers) => ["riad_0", "riad_1", "riad_2"],
        (Biome::Arctic, Tier::Fisherfolk) => ["cabin_0", "cabin_1", "cabin_2"],
        (Biome::Arctic, Tier::Labourers) => ["lodge_0", "lodge_1", "lodge_2"],
    };
    Some(set[v % 3])
}

/// A house of a far region and what stands on its lot.
pub fn house_look(biome: Biome, tier: Tier, seed: u64, out: &mut Vec<Piece>) {
    let v = variant(seed);
    let Some(key) = house_model(biome, tier, v) else {
        return;
    };
    let r = |k: u64| h01(seed, 140 + k);
    let t = 0.92 + 0.16 * h01(seed, 2);
    out.push(Piece::at(key, Vec3::ZERO).tint([t, t * (0.97 + 0.06 * h01(seed, 3)), t]));
    let big = tier == Tier::Labourers;
    match biome {
        Biome::Tropical => {
            let leaf = |k: u64| {
                let g = 0.85 + 0.3 * r(k);
                [g * (0.9 + 0.15 * r(k + 1)), g, g * 0.85]
            };
            for (k, (x, z)) in [(-5.0, 4.6), (5.1, -3.9), (-5.1, -4.4)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                if r(k) < 0.62 {
                    out.push(
                        Piece::at("banana", v3(x, 0.0, z))
                            .yaw(r(4 + k) * TAU)
                            .scale(0.8 + 0.5 * r(8 + k))
                            .tint(leaf(12 + k * 2)),
                    );
                }
            }
            if !big {
                if r(20) > 0.5 {
                    out.push(Piece::at("canoe", v3(5.0, 0.0, 1.6)).yaw(PI * 0.5 + r(21) * 0.3));
                }
                if r(22) > 0.4 {
                    out.push(Piece::at("barrel", v3(-2.2, 0.0, 0.6)));
                }
                if r(23) > 0.5 {
                    out.push(Piece::at("wash_line", v3(0.8, 0.0, -4.8)).tint([
                        1.0,
                        0.9 + 0.1 * r(24),
                        0.8 + 0.2 * r(25),
                    ]));
                }
            } else {
                out.push(Piece::at("tub_shrub", v3(-1.3, 0.0, 5.0)).tint(leaf(30)));
                out.push(Piece::at("tub_shrub", v3(1.3, 0.0, 5.0)).tint(leaf(32)));
            }
        }
        Biome::Arid => {
            let n = if big { 2 } else { 3 };
            for k in 0..n {
                if r(k) < 0.7 {
                    let x = [-4.6, 3.2, -3.6][k as usize] + r(4 + k) * 0.5;
                    let z = [3.6, 4.4, 4.6][k as usize];
                    out.push(
                        Piece::at("jar", v3(x, 0.0, z))
                            .scale(0.8 + 0.5 * r(8 + k))
                            .tint([1.0, 0.9 + 0.1 * r(12 + k), 0.85 + 0.15 * r(16 + k)]),
                    );
                }
            }
            if r(20) > 0.55 {
                out.push(Piece::at("bench", v3(2.2, 0.0, 2.6)));
            }
            if r(21) > 0.6 {
                out.push(
                    Piece::at("bush", v3(-5.0, 0.0, -4.4))
                        .scale(0.7)
                        .tint([0.95, 0.9, 0.7]),
                );
            }
        }
        Biome::Arctic => {
            for (k, (x, z)) in [(-4.4, 3.4), (4.6, -2.4), (-4.2, -4.2), (3.4, 4.6)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                if r(k) < 0.6 {
                    out.push(
                        Piece::at("snow_drift", v3(x, 0.0, z))
                            .yaw(r(4 + k) * TAU)
                            .scale(0.7 + 0.7 * r(8 + k)),
                    );
                }
            }
            if r(20) > 0.35 {
                out.push(
                    Piece::at("log_pile", v3(-4.9, 0.0, 0.2))
                        .yaw(PI * 0.5)
                        .scale(0.5),
                );
            }
            if !big && r(21) > 0.5 {
                out.push(Piece::at("sled", v3(4.9, 0.0, 2.6)).yaw(PI * 0.5 + r(22) * 0.5));
            }
            if !big && r(23) > 0.5 {
                out.push(Piece::at("fish_rack", v3(0.4, 0.0, -4.7)));
            }
        }
        Biome::Temperate => {}
    }
}

/// The pieces of the far regions' works. `hw`, `hd`: half the lot, meters.
pub fn works_look(kind: Kind, seed: u64, hw: f32, hd: f32, out: &mut Vec<Piece>) {
    let cell = CELL_M as f32;
    // The farm kinds keep a yard in the front left corner (three cells square); every other
    // cell of the lot is planted like the fields outside it.
    let yard = v3(-hw + 6.0, 0.0, hd - 6.0);
    let lot_cells = |out: &mut Vec<Piece>, f: &dyn Fn(u64, Vec3) -> Vec<Piece>| {
        let (w, d) = ((hw * 2.0 / cell) as i32, (hd * 2.0 / cell) as i32);
        for j in 0..d {
            for i in 0..w {
                if i < 3 && j >= d - 3 {
                    continue;
                }
                let at = v3(
                    -hw + cell * (i as f32 + 0.5),
                    0.0,
                    -hd + cell * (j as f32 + 0.5),
                );
                out.extend(f((j * w + i) as u64, at));
            }
        }
    };
    match kind {
        Kind::CanePlantation => {
            out.push(Piece::at("cane_shed", yard + v3(0.0, 0.0, -1.0)));
            out.push(Piece::at("hand_cart", yard + v3(3.6, 0.0, 3.6)).yaw(0.5));
            out.push(Piece::at("barrel", yard + v3(-4.6, 0.0, 3.8)));
            lot_cells(out, &|k, at| vec![cane_piece(seed, 6000 + k, at, 0.5)]);
        }
        Kind::DateGrove => {
            out.push(Piece::at("date_house", yard));
            lot_cells(out, &|k, at| vec![palm_piece(seed, 6000 + k, at)]);
        }
        Kind::SaltWorks => {
            out.push(Piece::at("salt_house", yard + v3(0.0, 0.0, -0.5)));
            out.push(Piece::at("windpump", yard + v3(4.4, 0.0, 4.2)));
            out.push(Piece::at("hand_cart", yard + v3(-3.8, 0.0, 4.2)).yaw(1.2));
            lot_cells(out, &|k, at| {
                pan_pieces(seed, 6000 + k, at, h01(seed, 9100 + k / 2))
            });
        }
        Kind::Distillery => {
            out.push(Piece::at("distillery", Vec3::ZERO));
            for i in 0..8 {
                out.push(Piece::at(
                    "barrel",
                    v3(
                        0.4 + (i % 4) as f32 * 0.85,
                        0.0,
                        6.6 - (i / 4) as f32 * 0.85,
                    ),
                ));
            }
        }
        Kind::TrapperLodge => {
            out.push(Piece::at("trapper", Vec3::ZERO));
            let pelts = [
                [0.62, 0.42, 0.28],
                [0.95, 0.93, 0.9],
                [0.75, 0.42, 0.2],
                [0.55, 0.55, 0.56],
            ];
            for k in 0..3u64 {
                let c = pelts[(h01(seed, 60 + k) * 4.0) as usize % 4];
                out.push(
                    Piece::at("pelt_frame", v3(1.6 + k as f32 * 1.5, 0.0, -3.9))
                        .yaw(0.2 * (h01(seed, 64 + k) - 0.5))
                        .tint(c),
                );
            }
            out.push(Piece::at("sled", v3(3.6, 0.0, 3.4)).yaw(0.3));
            out.push(
                Piece::at("husky", v3(1.6, 0.0, 4.2))
                    .yaw(2.8)
                    .tint([0.9, 0.9, 0.95]),
            );
            out.push(
                Piece::at("husky", v3(2.4, 0.0, 2.2))
                    .yaw(0.6)
                    .tint([0.55, 0.55, 0.6]),
            );
            out.push(Piece::at("snow_drift", v3(-4.6, 0.0, 3.6)).scale(1.2));
            out.push(Piece::at("snow_drift", v3(4.4, 0.0, -1.2)).yaw(1.9));
        }
        Kind::OilWorks => {
            out.push(Piece::at("oil_works", Vec3::ZERO));
            for i in 0..10 {
                out.push(Piece::at(
                    "barrel",
                    v3(
                        2.6 + (i % 5) as f32 * 0.85,
                        0.0,
                        1.0 + (i / 5) as f32 * 0.85,
                    ),
                ));
            }
            out.push(Piece::at("snow_drift", v3(-6.6, 0.0, -6.4)).scale(1.3));
        }
        _ => {}
    }
}

/// One 4 m square of sugar cane (`strip_t` 0..1: how far along its strip is).
pub fn cane_piece(seed: u64, k: u64, at: Vec3, strip_t: f32) -> Piece {
    // Young strips stand lower and paler.
    let young = strip_t < 0.16;
    let g = 0.86 + 0.24 * h01(seed, k);
    Piece::at("cane_cell", at)
        .yaw(if h01(seed, k + 1) < 0.5 { 0.0 } else { PI })
        .scale(if young { 0.55 } else { 0.9 + 0.2 * strip_t })
        .tint(if young {
            [g * 1.05, g * 1.1, g * 0.8]
        } else {
            [g * (0.92 + 0.12 * strip_t), g, g * 0.86]
        })
}

/// A date palm for one cell of a grove.
pub fn palm_piece(seed: u64, k: u64, at: Vec3) -> Piece {
    let off = v3(
        h01(seed, k) * 1.2 - 0.6,
        0.0,
        h01(seed, k + 1000) * 1.2 - 0.6,
    );
    let g = 0.88 + 0.2 * h01(seed, k + 2000);
    Piece::at("date_palm", at + off)
        .yaw(h01(seed, k + 3000) * TAU)
        .scale(0.8 + 0.4 * h01(seed, k + 4000))
        .tint([g, g, g * 0.9])
}

/// A salt pan for one cell: brine from sea green through rose to white as it dries
/// (`strip_t` 0..1), and now and then a raked heap.
pub fn pan_pieces(seed: u64, k: u64, at: Vec3, strip_t: f32) -> Vec<Piece> {
    let tint = if strip_t < 0.34 {
        [0.62, 0.86, 0.84]
    } else if strip_t < 0.6 {
        [0.9, 0.74, 0.74]
    } else if strip_t < 0.8 {
        [0.84, 0.9, 0.92]
    } else {
        [1.0, 1.0, 1.0]
    };
    let mut out = vec![Piece::at("salt_pan", at).tint(tint)];
    if strip_t >= 0.6 && h01(seed, k + 500) < 0.3 {
        out.push(
            Piece::at(
                "salt_heap",
                at + v3(
                    h01(seed, k + 600) * 1.4 - 0.7,
                    0.05,
                    h01(seed, k + 700) * 1.4 - 0.7,
                ),
            )
            .scale(0.6 + 0.5 * h01(seed, k + 800)),
        );
    }
    out
}

// ------------------------------------------------------------------------------ helpers

/// A flat quad facing up, from `(x0, z0)` to `(x1, z1)` at height `y`.
pub(crate) fn up_quad(p: &mut Parts, m: Mat, x: (f32, f32), z: (f32, f32), y: f32, col: Vec4) {
    p.quad(
        m,
        v3(x.0, y, z.1),
        v3(x.1, y, z.1),
        v3(x.1, y, z.0),
        v3(x.0, y, z.0),
        col,
    );
}

/// Snow lying on a gabled roof built by `gable_roof` with the same numbers, down the upper
/// `share` of both slopes, its lower edge uneven.
#[allow(clippy::too_many_arguments)]
pub(crate) fn roof_snow(
    p: &mut Parts,
    c: Vec3,
    l: f32,
    w: f32,
    h: f32,
    over: f32,
    ridge_z: bool,
    share: f32,
) {
    let mut s = Parts::default();
    let (hl, hw) = (l * 0.5 + over, w * 0.5 + over);
    let drop = h / (w * 0.5) * over;
    let n = 7;
    let lift = 0.07;
    for i in 0..n {
        let x0 = -hl + 2.0 * hl * i as f32 / n as f32;
        let x1 = -hl + 2.0 * hl * (i + 1) as f32 / n as f32;
        for side in [1.0f32, -1.0] {
            let jag = h01(91, (i * 2) as u64 + (side > 0.0) as u64);
            let k = (share * (0.8 + 0.2 * jag)).min(1.0);
            let (ez, ey) = (hw * k * side, h - (h + drop) * k + lift);
            let (a, b) = (v3(x0, ey, ez), v3(x1, ey, ez));
            let (r0, r1) = (v3(x0, h + lift, 0.0), v3(x1, h + lift, 0.0));
            if side > 0.0 {
                s.quad(Mat::Snow, a, b, r1, r0, SNOW);
            } else {
                s.quad(Mat::Snow, b, a, r0, r1, SNOW);
            }
        }
    }
    s.beam(
        Mat::Snow,
        v3(-hl, h - 0.04, 0.0),
        v3(hl, h - 0.04, 0.0),
        0.34,
        SNOW,
    );
    let rot = if ridge_z {
        Quat::from_rotation_y(PI * 0.5)
    } else {
        Quat::IDENTITY
    };
    p.add_parts(&s, Mat4::from_rotation_translation(rot, c));
}

/// A leaf blade along `pts`, `widths` wide at each point, lying across `side`.
fn blade(p: &mut Parts, m: Mat, pts: &[Vec3], widths: &[f32], side: Vec3, col: Vec4) {
    for k in 0..pts.len() - 1 {
        let (a, b) = (pts[k], pts[k + 1]);
        let (wa, wb) = (side * widths[k] * 0.5, side * widths[k + 1] * 0.5);
        p.quad(m, a - wa, a + wa, b + wb, b - wb, col);
    }
}

/// A wooden vat standing on `base`, with two iron hoops.
fn vat(p: &mut Parts, base: Vec3, r: f32, h: f32, col: Vec4) {
    p.cylinder(Mat::Planks, base, r, r * 1.04, h, 14, col, true);
    for y in [0.22, 0.78] {
        p.cylinder(
            Mat::Iron,
            base + Vec3::Y * (h * y),
            r * 1.05,
            r * 1.05,
            0.07,
            14,
            shade(Vec4::ONE, 0.5),
            false,
        );
    }
}

/// A round brick chimney stack with a collar.
fn stack(p: &mut Parts, base: Vec3, r: f32, h: f32) {
    p.cylinder(Mat::Brick, base, r, r * 0.68, h, 14, Vec4::ONE, false);
    p.cylinder(
        Mat::Brick,
        base + Vec3::Y * h,
        r * 0.8,
        r * 0.8,
        0.5,
        14,
        shade(Vec4::ONE, 0.7),
        false,
    );
}

/// A railing from `a` to `b` (posts about every 1.1 m, a top and a middle rail).
pub(crate) fn railing(p: &mut Parts, a: Vec3, b: Vec3, h: f32, col: Vec4) {
    let len = (b - a).length();
    let n = (len / 1.1).ceil().max(1.0) as usize;
    for i in 0..=n {
        let at = a + (b - a) * (i as f32 / n as f32);
        p.beam(Mat::Timber, at, at + Vec3::Y * h, 0.08, col);
    }
    p.beam(Mat::Timber, a + Vec3::Y * h, b + Vec3::Y * h, 0.09, col);
    p.beam(
        Mat::Timber,
        a + Vec3::Y * (h * 0.5),
        b + Vec3::Y * (h * 0.5),
        0.05,
        col,
    );
}

/// A low parapet round a flat roof `l` × `w` centred on `c` (the roof's height).
pub(crate) fn parapet(p: &mut Parts, c: Vec3, l: f32, w: f32, h: f32, col: Vec4) {
    let t = 0.28;
    let (hl, hw) = (l * 0.5, w * 0.5);
    p.cuboid(
        Mat::Plaster,
        c + v3(-hl, 0.0, hw - t),
        c + v3(hl, h, hw),
        col,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        c + v3(-hl, 0.0, -hw),
        c + v3(hl, h, -hw + t),
        col,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        c + v3(-hl, 0.0, -hw + t),
        c + v3(-hl + t, h, hw - t),
        col,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        c + v3(hl - t, 0.0, -hw + t),
        c + v3(hl, h, hw - t),
        col,
        false,
    );
    // Corner horns.
    for x in [-hl + 0.2, hl - 0.2] {
        for z in [-hw + 0.2, hw - 0.2] {
            p.cylinder(
                Mat::Plaster,
                c + v3(x, h, z),
                0.24,
                0.07,
                0.55,
                6,
                shade(col, 1.05),
                true,
            );
        }
    }
}

/// Roof beam ends poking through a mud wall that faces `normal`, along its head.
pub(crate) fn beam_ends(p: &mut Parts, mid: Vec3, normal: Vec3, len: f32) {
    let along = Vec3::Y.cross(normal).normalize();
    let n = (len / 0.85) as i32;
    for i in 0..n {
        let off = (i as f32 - (n as f32 - 1.0) * 0.5) * 0.85;
        let a = mid + along * off;
        p.rod(
            Mat::Timber,
            a - normal * 0.1,
            a + normal * 0.38,
            0.07,
            5,
            hexc(0x4a3626),
        );
    }
}

// ------------------------------------------------------------------- Sarrowmere: jungle

/// A fisher's house on stilts: painted boards, a veranda and a palm-thatch roof.
fn stilt_house(v: usize) -> Parts {
    let mut p = Parts::default();
    let deck = 1.35;
    let (l, w, wall) = (6.0, 4.2, 2.5);
    let cz = -0.7;
    let c = v3(0.0, deck, cz);
    let wood = hexc(0x8a6a48);
    for x in [-3.3, 0.0, 3.3] {
        for z in [-2.9, 0.1, 3.05] {
            p.rod(
                Mat::Timber,
                v3(x, -2.6, z),
                v3(x, deck, z),
                0.15,
                6,
                shade(wood, 0.65),
            );
        }
    }
    // Cross braces under the floor.
    for z in [-2.9, 3.05] {
        p.beam(
            Mat::Timber,
            v3(-3.3, 0.1, z),
            v3(0.0, deck - 0.2, z),
            0.09,
            shade(wood, 0.6),
        );
        p.beam(
            Mat::Timber,
            v3(3.3, 0.1, z),
            v3(0.0, deck - 0.2, z),
            0.09,
            shade(wood, 0.6),
        );
    }
    p.cuboid(
        Mat::Planks,
        v3(-3.6, deck - 0.2, -3.1),
        v3(3.6, deck, 3.3),
        shade(Vec4::ONE, 0.9),
        true,
    );
    let paint = [hexc(0x4aa6a0), hexc(0xe6b450), hexc(0xd9755a)][v % 3];
    p.cuboid(
        Mat::Planks,
        c + v3(-l * 0.5, 0.0, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        paint,
        false,
    );
    for x in [-l * 0.5, l * 0.5] {
        for z in [-w * 0.5, w * 0.5] {
            p.beam(
                Mat::Paint,
                c + v3(x, 0.0, z),
                c + v3(x, wall, z),
                0.16,
                WHITE,
            );
        }
    }
    let thatch = hexc(0xb7a070);
    let tin = hexc(0xb86a48);
    let (roof, rcol) = if v == 1 {
        (Mat::Iron, tin)
    } else {
        (Mat::Thatch, thatch)
    };
    if v == 1 {
        p.gable_roof(
            roof,
            Mat::Planks,
            c + v3(0.0, wall, 0.0),
            l,
            w,
            1.7,
            0.6,
            false,
            rcol,
            paint,
        );
    } else {
        p.hip_roof(roof, c + v3(0.0, wall, 0.0), l, w, 2.9, 0.85, rcol);
    }
    // The veranda: posts at the deck's edge carry a lean-to off the front wall.
    let (z0, z1) = (cz + w * 0.5, 3.15);
    for x in [-3.4, -1.15, 1.15, 3.4] {
        p.beam(
            Mat::Timber,
            v3(x, deck, z1),
            v3(x, deck + 2.0, z1),
            0.12,
            WHITE,
        );
    }
    let (ya, yb) = (deck + 1.98, deck + wall + 0.12);
    p.quad(
        roof,
        v3(-3.8, ya, z1 + 0.35),
        v3(3.8, ya, z1 + 0.35),
        v3(3.8, yb, z0),
        v3(-3.8, yb, z0),
        rcol,
    );
    p.quad(
        roof,
        v3(3.8, ya - 0.08, z1 + 0.35),
        v3(-3.8, ya - 0.08, z1 + 0.35),
        v3(-3.8, yb - 0.08, z0),
        v3(3.8, yb - 0.08, z0),
        shade(rcol, 0.5),
    );
    // Railing, open at the steps.
    railing(&mut p, v3(-3.5, deck, z1), v3(-0.8, deck, z1), 0.9, WHITE);
    railing(&mut p, v3(0.8, deck, z1), v3(3.5, deck, z1), 0.9, WHITE);
    railing(&mut p, v3(-3.5, deck, z0), v3(-3.5, deck, z1), 0.9, WHITE);
    railing(&mut p, v3(3.5, deck, z0), v3(3.5, deck, z1), 0.9, WHITE);
    // Steps down to the path.
    for k in 0..4 {
        let y = deck - 0.32 * (k as f32 + 1.0);
        let z = 3.3 + 0.36 * k as f32;
        p.cuboid(
            Mat::Planks,
            v3(-0.7, y, z),
            v3(0.7, y + 0.08, z + 0.38),
            shade(Vec4::ONE, 0.85),
            true,
        );
    }
    for x in [-0.72, 0.72] {
        p.beam(
            Mat::Timber,
            v3(x, deck - 0.1, 3.3),
            v3(x, -0.1, 4.85),
            0.1,
            shade(wood, 0.7),
        );
    }
    let door = [hexc(0xf2efe0), hexc(0x2f6a66), hexc(0x2f4a5a)][v % 3];
    p.door(c + v3(0.0, 0.0, w * 0.5), Vec3::Z, 0.95, 2.0, door);
    let shut = [hexc(0xf2efe0), hexc(0x3f7a52), hexc(0x35607a)][v % 3];
    for x in [-1.95, 1.95] {
        p.windows(c + v3(x, 1.35, w * 0.5), Vec3::Z, 1, 0.8, 1.0, 0.0, WHITE);
        shutters(&mut p, c + v3(x, 1.35, w * 0.5), 0.8, 1.0, shut);
    }
    p.windows(c + v3(l * 0.5, 1.35, 0.0), Vec3::X, 1, 0.8, 1.0, 0.0, WHITE);
    p.windows(
        c + v3(-l * 0.5, 1.35, 0.0),
        -Vec3::X,
        1,
        0.8,
        1.0,
        0.0,
        WHITE,
    );
    p.windows(
        c + v3(0.0, 1.35, -w * 0.5),
        -Vec3::Z,
        2,
        0.8,
        1.0,
        1.6,
        WHITE,
    );
    // A hammock slung between two stilts.
    if v != 1 {
        let (a, b) = (v3(-3.2, 0.95, 0.1), v3(-0.1, 0.95, 0.1));
        let n = 6;
        for i in 0..n {
            let t0 = i as f32 / n as f32;
            let t1 = (i + 1) as f32 / n as f32;
            let sag = |t: f32| -0.5 * (1.0 - (2.0 * t - 1.0).powi(2));
            let (p0, p1) = (
                a.lerp(b, t0) + Vec3::Y * sag(t0),
                a.lerp(b, t1) + Vec3::Y * sag(t1),
            );
            p.quad(
                Mat::Canvas,
                p0 - Vec3::Z * 0.35,
                p0 + Vec3::Z * 0.35,
                p1 + Vec3::Z * 0.35,
                p1 - Vec3::Z * 0.35,
                hexc(0xd9c9a0),
            );
        }
    }
    p.ground_occlusion(0.62, 1.2);
    p.finish()
}

/// A planter's house: whitewashed ground floor, a boarded upper floor behind a gallery.
fn gallery_house(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (8.2, 5.6);
    let c = v3(0.0, 0.0, -0.9);
    let (f0, f1, top) = (0.45, 3.5, 6.5);
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.2, -3.0, -3.9),
        v3(l * 0.5 + 0.2, f0, 3.9),
        Vec4::splat(0.82).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, f0, -w * 0.5),
        c + v3(l * 0.5, f1, w * 0.5),
        hexc(0xefe6d2),
        false,
    );
    let boards = [hexc(0xf0e2c0), hexc(0x9ec7bd), hexc(0xe8b3a2)][v % 3];
    p.cuboid(
        Mat::Planks,
        c + v3(-l * 0.5, f1, -w * 0.5),
        c + v3(l * 0.5, top, w * 0.5),
        boards,
        false,
    );
    // The gallery across the front, on posts, under the same roof.
    let (z0, z1) = (c.z + w * 0.5, 3.6);
    p.cuboid(
        Mat::Planks,
        v3(-l * 0.5, f1 - 0.18, z0),
        v3(l * 0.5, f1, z1),
        shade(Vec4::ONE, 0.9),
        true,
    );
    for x in [-3.95, -1.35, 1.35, 3.95] {
        p.beam(
            Mat::Timber,
            v3(x, f0, z1 - 0.12),
            v3(x, top, z1 - 0.12),
            0.16,
            WHITE,
        );
    }
    railing(
        &mut p,
        v3(-4.0, f1, z1 - 0.1),
        v3(4.0, f1, z1 - 0.1),
        0.95,
        WHITE,
    );
    let tile = Vec4::ONE;
    let rc = v3(0.0, top, (c.z - w * 0.5 + z1) * 0.5);
    p.hip_roof(Mat::RoofTile, rc, l, z1 - (c.z - w * 0.5), 2.5, 0.55, tile);
    let shut = [hexc(0x2f6a5a), hexc(0xf2efe0), hexc(0x4a6a8a)][v % 3];
    p.door(v3(0.0, f0, z0), Vec3::Z, 1.5, 2.3, shut);
    for x in [-2.7, 2.7] {
        p.windows(v3(x, 2.1, z0), Vec3::Z, 1, 0.95, 1.5, 0.0, WHITE);
        shutters(&mut p, v3(x, 2.1, z0), 0.95, 1.5, shut);
    }
    for x in [-2.7, 0.0, 2.7] {
        p.windows(v3(x, 5.0, z0), Vec3::Z, 1, 0.95, 1.7, 0.0, WHITE);
        shutters(&mut p, v3(x, 5.0, z0), 0.95, 1.7, shut);
    }
    for (n, side) in [(Vec3::X, l * 0.5), (-Vec3::X, -l * 0.5)] {
        p.windows(c + v3(side, 2.1, 0.0), n, 2, 0.9, 1.4, 1.2, WHITE);
        p.windows(c + v3(side, 5.0, 0.0), n, 2, 0.9, 1.5, 1.2, WHITE);
    }
    p.windows(
        c + v3(0.0, 5.0, -w * 0.5),
        -Vec3::Z,
        3,
        0.9,
        1.5,
        1.4,
        WHITE,
    );
    // Steps to the door.
    for k in 0..2 {
        p.cuboid(
            Mat::Stone,
            v3(-1.2, -0.3, z1 + 0.3 * k as f32 - 0.3),
            v3(
                1.2,
                f0 - 0.22 * (k as f32 + 0.0),
                z1 + 0.3 * (k as f32 + 1.0),
            ),
            Vec4::splat(0.85).with_w(1.0),
            false,
        );
    }
    p.ground_occlusion(0.6, 1.6);
    p.finish()
}

/// A 4 m square of sugar cane: tall crossed cards, stalks yellow-green, leaves arching.
fn cane_cell() -> Parts {
    let mut p = Parts::default();
    let stalk = hexc(0xa7b356);
    let leaf = hexc(0x5c9838);
    for r in 0..4 {
        let z = -1.5 + r as f32 * 1.0;
        let mut x = -1.75 + (r % 2) as f32 * 0.3;
        let mut k = r * 7 + 1;
        while x < 1.9 {
            let h = 2.5 + 0.8 * ((k * 37 % 13) as f32 / 13.0);
            let lean = v3(
                ((k * 13 % 7) as f32 / 7.0 - 0.5) * 0.5,
                0.0,
                ((k * 29 % 5) as f32 / 5.0 - 0.5) * 0.5,
            );
            for a in [0.6f32, -0.6] {
                let d = v3(a.sin(), 0.0, a.cos()) * 0.34;
                let b = v3(x, 0.0, z);
                let m = Vec3::Y * h * 0.55 + lean * 0.5;
                p.quad(
                    Mat::Crop,
                    b - d * 0.5,
                    b + d * 0.5,
                    b + d * 0.6 + m,
                    b - d * 0.6 + m,
                    stalk,
                );
                let t = Vec3::Y * h + lean;
                p.quad(
                    Mat::Crop,
                    b - d * 0.6 + m,
                    b + d * 0.6 + m,
                    b + d * 2.1 + t,
                    b - d * 2.1 + t,
                    leaf,
                );
            }
            x += 0.62;
            k += 1;
        }
    }
    p.finish()
}

/// The plantation's yard: an open thatched shed over a cane press with its sweep.
fn cane_shed() -> Parts {
    let mut p = Parts::default();
    let (l, w) = (8.0, 5.6);
    let wood = hexc(0x8a6a48);
    for x in [-l * 0.5, 0.0, l * 0.5] {
        for z in [-w * 0.5, w * 0.5] {
            p.rod(
                Mat::Timber,
                v3(x, -2.0, z),
                v3(x, 3.0, z),
                0.15,
                6,
                shade(wood, 0.8),
            );
        }
    }
    p.hip_roof(
        Mat::Thatch,
        v3(0.0, 3.0, 0.0),
        l,
        w,
        2.6,
        0.9,
        hexc(0xb7a070),
    );
    // The press: three upright rollers in a frame, turned by a long sweep.
    p.cuboid(
        Mat::Timber,
        v3(-1.3, 0.0, -0.6),
        v3(1.3, 0.3, 0.6),
        wood,
        false,
    );
    p.cuboid(
        Mat::Timber,
        v3(-1.3, 1.5, -0.6),
        v3(1.3, 1.75, 0.6),
        wood,
        false,
    );
    for x in [-0.75, 0.0, 0.75] {
        p.cylinder(
            Mat::Iron,
            v3(x, 0.3, 0.0),
            0.33,
            0.33,
            1.2,
            12,
            shade(Vec4::ONE, 0.8),
            false,
        );
    }
    p.rod(
        Mat::Timber,
        v3(0.0, 1.75, 0.0),
        v3(0.0, 2.3, 0.0),
        0.1,
        6,
        wood,
    );
    p.beam(
        Mat::Timber,
        v3(0.0, 2.25, 0.0),
        v3(3.6, 1.2, 2.2),
        0.14,
        wood,
    );
    // Cut cane in bundles, and a trough for the juice.
    let cane = hexc(0xb7b25a);
    for (k, (x, z, yaw)) in [(-2.9, -1.6, 0.2f32), (-2.7, 1.2, -0.3), (2.6, -1.8, 1.4)]
        .into_iter()
        .enumerate()
    {
        let dir = v3(yaw.cos(), 0.0, yaw.sin());
        for i in 0..7 {
            let off = v3(-dir.z, 0.0, dir.x) * ((i % 4) as f32 * 0.16 - 0.24);
            let y = 0.1 + (i / 4) as f32 * 0.16;
            let c = v3(x, y, z) + off;
            p.rod(
                Mat::Crop,
                c - dir * 1.1,
                c + dir * 1.1,
                0.07,
                5,
                shade(cane, 0.9 + 0.05 * (k + i) as f32 % 0.2),
            );
        }
    }
    p.cuboid(
        Mat::Planks,
        v3(-0.5, 0.0, 0.8),
        v3(0.5, 0.35, 2.0),
        shade(Vec4::ONE, 0.7),
        true,
    );
    p.ground_occlusion(0.6, 1.2);
    p.finish()
}

/// A pot still: brick hearth, copper pot and head, the neck bending down to a worm tub.
fn pot_still(p: &mut Parts, at: Vec3, toward: Vec3) {
    let copper = hexc(0xc47a45);
    p.cylinder(Mat::Brick, at, 1.0, 1.0, 0.9, 12, Vec4::ONE, true);
    p.cylinder(
        Mat::Paint,
        at + Vec3::Y * 0.9,
        0.82,
        0.9,
        0.7,
        14,
        copper,
        false,
    );
    p.cylinder(
        Mat::Paint,
        at + Vec3::Y * 1.6,
        0.9,
        0.2,
        1.0,
        14,
        copper,
        false,
    );
    let top = at + Vec3::Y * 2.6;
    p.rod(
        Mat::Paint,
        at + Vec3::Y * 2.55,
        top + Vec3::Y * 0.3,
        0.14,
        8,
        copper,
    );
    let bend = top + Vec3::Y * 0.3;
    let tub = at + toward * 2.3;
    p.rod(Mat::Paint, bend, tub + Vec3::Y * 1.5, 0.1, 8, copper);
    vat(p, tub, 0.62, 1.25, hexc(0x8a6a48));
}

/// The rum distillery: a tiled still house, two pot stills under a lean-to, vats and a
/// tall stack.
fn distillery() -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (9.0, 6.4, 4.2);
    let c = v3(-2.6, 0.0, -3.8);
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5 - 0.2, -3.0, -w * 0.5 - 0.2),
        c + v3(l * 0.5 + 0.2, 0.4, w * 0.5 + 0.2),
        Vec4::splat(0.8).with_w(1.0),
        false,
    );
    let lime = hexc(0xe9dcbc);
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, 0.4, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        lime,
        false,
    );
    p.hip_roof(
        Mat::RoofTile,
        c + v3(0.0, wall, 0.0),
        l,
        w,
        2.5,
        0.5,
        Vec4::ONE,
    );
    // Louvred lantern on the ridge.
    p.cuboid(
        Mat::Planks,
        c + v3(-1.4, wall + 2.0, -0.7),
        c + v3(1.4, wall + 3.0, 0.7),
        hexc(0x3f7a52),
        false,
    );
    p.hip_roof(
        Mat::RoofTile,
        c + v3(0.0, wall + 3.0, 0.0),
        2.8,
        1.4,
        0.7,
        0.25,
        Vec4::ONE,
    );
    let green = hexc(0x2f6a5a);
    p.door(c + v3(-1.6, 0.4, w * 0.5), Vec3::Z, 2.2, 2.9, green);
    for x in [1.2, 3.1] {
        p.windows(c + v3(x, 2.4, w * 0.5), Vec3::Z, 1, 0.95, 1.5, 0.0, WHITE);
        shutters(&mut p, c + v3(x, 2.4, w * 0.5), 0.95, 1.5, green);
    }
    p.windows(
        c + v3(-l * 0.5, 2.4, 0.0),
        -Vec3::X,
        2,
        0.9,
        1.4,
        1.4,
        WHITE,
    );
    // The still bay on the right: a tiled lean-to on posts.
    let (bx0, bx1, bz0, bz1) = (2.1, 7.6, -6.8, -1.2);
    for x in [bx1 - 0.1] {
        for z in [bz0, (bz0 + bz1) * 0.5, bz1] {
            p.beam(Mat::Timber, v3(x, 0.0, z), v3(x, 3.0, z), 0.2, Vec4::ONE);
        }
    }
    p.quad(
        Mat::RoofTile,
        v3(bx1 + 0.4, 2.95, bz1 + 0.3),
        v3(bx1 + 0.4, 2.95, bz0 - 0.3),
        v3(bx0 - 0.2, 4.15, bz0 - 0.3),
        v3(bx0 - 0.2, 4.15, bz1 + 0.3),
        Vec4::ONE,
    );
    p.quad(
        Mat::RoofTile,
        v3(bx1 + 0.4, 2.85, bz0 - 0.3),
        v3(bx1 + 0.4, 2.85, bz1 + 0.3),
        v3(bx0 - 0.2, 4.05, bz1 + 0.3),
        v3(bx0 - 0.2, 4.05, bz0 - 0.3),
        shade(Vec4::ONE, 0.5),
    );
    p.cuboid(
        Mat::Cobble,
        v3(bx0, -0.5, bz0),
        v3(bx1, 0.06, bz1),
        Vec4::ONE,
        false,
    );
    pot_still(&mut p, v3(3.6, 0.06, -5.2), v3(0.9, 0.0, 0.45).normalize());
    pot_still(&mut p, v3(3.6, 0.06, -2.7), v3(0.9, 0.0, 0.3).normalize());
    // The stack behind the bay.
    stack(&mut p, v3(6.6, -1.0, -7.0), 1.05, 14.0);
    // Fermenting vats in the yard, and a heap of cane.
    for (k, x) in [-6.4f32, -4.0, -1.6].into_iter().enumerate() {
        vat(
            &mut p,
            v3(x, 0.0, 3.6),
            1.05,
            1.35,
            shade(hexc(0x8a6a48), 0.85 + 0.1 * k as f32),
        );
    }
    let cane = hexc(0xb7b25a);
    for i in 0..14 {
        let (row, col) = (i / 5, i % 5);
        let c = v3(
            5.4,
            0.12 + row as f32 * 0.17,
            3.2 + col as f32 * 0.2 + row as f32 * 0.1,
        );
        p.rod(
            Mat::Crop,
            c - Vec3::X * 1.3,
            c + Vec3::X * 1.3,
            0.08,
            5,
            shade(cane, 0.85 + 0.03 * i as f32),
        );
    }
    p.ground_occlusion(0.55, 2.0);
    p.finish()
}

/// A banana plant: broad leaves arching from a short stem.
fn banana() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Leaf,
        v3(0.0, -0.2, 0.0),
        0.16,
        0.11,
        1.5,
        7,
        hexc(0x7a8a3a),
        false,
    );
    let green = hexc(0x4f8a2e);
    for i in 0..7 {
        let a = i as f32 * 0.9 + 0.3;
        let dir = v3(a.cos(), 0.0, a.sin());
        let side = v3(-a.sin(), 0.0, a.cos());
        let up = 0.7 + 0.25 * ((i * 5 % 3) as f32);
        let top = v3(0.0, 1.25, 0.0);
        let pts = [
            top,
            top + dir * 0.6 + Vec3::Y * up,
            top + dir * 1.4 + Vec3::Y * (up + 0.25),
            top + dir * 2.2 + Vec3::Y * (up - 0.45),
        ];
        blade(
            &mut p,
            Mat::Leaf,
            &pts,
            &[0.12, 0.62, 0.7, 0.2],
            side,
            shade(green, 0.85 + 0.05 * i as f32),
        );
    }
    p.finish()
}

/// A dugout canoe drawn up on the ground.
fn canoe() -> Parts {
    let mut p = Parts::default();
    p.add_parts(
        &small_boat(),
        Mat4::from_scale_rotation_translation(
            v3(1.15, 0.8, 0.7),
            Quat::from_rotation_z(0.06),
            v3(0.0, 0.02, 0.0),
        ),
    );
    for m in p.meshes.values_mut() {
        for v in &mut m.vertices {
            v.color = [0.52, 0.36, 0.22, 1.0];
        }
    }
    p.finish()
}

// ------------------------------------------------------------------ the Amber Reach: dry

pub(crate) fn mud(v: usize) -> Vec4 {
    [hexc(0xc79f6d), hexc(0xd9bb8c), hexc(0xb98659)][v % 3]
}

/// A mud-brick house: flat roof behind a parapet, beam ends, a stair up the side and a
/// walled yard with an oven.
fn adobe_house(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w, hgt) = (6.8, 5.4, 3.2);
    let c = v3(-0.5, 0.0, -0.9);
    let col = mud(v);
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, -2.5, -w * 0.5),
        c + v3(l * 0.5, hgt, w * 0.5),
        col,
        false,
    );
    parapet(&mut p, c + v3(0.0, hgt, 0.0), l, w, 0.45, col);
    beam_ends(&mut p, c + v3(0.0, hgt - 0.45, w * 0.5), Vec3::Z, l - 0.8);
    beam_ends(&mut p, c + v3(0.0, hgt - 0.45, -w * 0.5), -Vec3::Z, l - 0.8);
    let blue = [hexc(0x2a5f8a), hexc(0x2f7a6a), hexc(0x7a3a2a)][v % 3];
    let z = c.z + w * 0.5;
    p.door(v3(c.x - 0.6, 0.0, z), Vec3::Z, 1.0, 2.05, blue);
    // A whitewashed surround to the door.
    p.cuboid(
        Mat::Plaster,
        v3(c.x - 1.3, 0.0, z),
        v3(c.x - 1.15, 2.3, z + 0.05),
        hexc(0xefe6d2),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(c.x - 0.05, 0.0, z),
        v3(c.x + 0.1, 2.3, z + 0.05),
        hexc(0xefe6d2),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(c.x - 1.3, 2.1, z),
        v3(c.x + 0.1, 2.3, z + 0.05),
        hexc(0xefe6d2),
        false,
    );
    p.windows(
        v3(c.x + 1.9, 1.9, z),
        Vec3::Z,
        1,
        0.6,
        0.8,
        0.0,
        hexc(0x5a4632),
    );
    shutters(&mut p, v3(c.x + 1.9, 1.9, z), 0.6, 0.8, blue);
    p.windows(
        c + v3(-l * 0.5, 1.9, 0.0),
        -Vec3::X,
        1,
        0.55,
        0.7,
        0.0,
        hexc(0x5a4632),
    );
    p.windows(
        c + v3(0.0, 1.9, -w * 0.5),
        -Vec3::Z,
        2,
        0.55,
        0.7,
        1.6,
        hexc(0x5a4632),
    );
    // The stair up the right side to the roof.
    let sx = c.x + l * 0.5;
    let steps = 8;
    for k in 0..steps {
        let y1 = hgt * (k as f32 + 1.0) / steps as f32;
        let z1 = c.z + w * 0.5 - 0.6 * k as f32;
        p.cuboid(
            Mat::Plaster,
            v3(sx, -1.5, z1 - 0.6),
            v3(sx + 0.95, y1, z1),
            shade(col, 0.94),
            false,
        );
    }
    // On the roof.
    let roof = c + v3(0.0, hgt, 0.0);
    match v % 3 {
        0 => {
            // An awning on poles and something drying in the sun.
            for (x, z) in [(-2.4, -1.8), (0.4, -1.8), (-2.4, 0.8), (0.4, 0.8)] {
                p.beam(
                    Mat::Timber,
                    roof + v3(x, 0.0, z),
                    roof + v3(x, 1.9, z),
                    0.08,
                    Vec4::ONE,
                );
            }
            up_quad(
                &mut p,
                Mat::Canvas,
                (roof.x - 2.6, roof.x + 0.6),
                (roof.z - 2.0, roof.z + 1.0),
                roof.y + 1.92,
                hexc(0xc9583a),
            );
            p.quad(
                Mat::Canvas,
                v3(roof.x - 2.6, roof.y + 1.9, roof.z - 2.0),
                v3(roof.x + 0.6, roof.y + 1.9, roof.z - 2.0),
                v3(roof.x + 0.6, roof.y + 1.9, roof.z + 1.0),
                v3(roof.x - 2.6, roof.y + 1.9, roof.z + 1.0),
                shade(hexc(0xc9583a), 0.6),
            );
            up_quad(
                &mut p,
                Mat::Canvas,
                (roof.x + 1.2, roof.x + 2.8),
                (roof.z - 1.0, roof.z + 1.4),
                roof.y + 0.03,
                hexc(0x9a4a22),
            );
        }
        1 => {
            // A roof room.
            let r = roof + v3(-1.6, 0.0, -1.0);
            p.cuboid(
                Mat::Plaster,
                r + v3(-1.5, 0.0, -1.3),
                r + v3(1.5, 2.3, 1.3),
                shade(col, 1.05),
                false,
            );
            parapet(
                &mut p,
                r + v3(0.0, 2.3, 0.0),
                3.0,
                2.6,
                0.3,
                shade(col, 1.05),
            );
            p.door(r + v3(0.2, 0.0, 1.3), Vec3::Z, 0.8, 1.8, blue);
        }
        _ => {
            // A whitewashed dome.
            lump(
                &mut p,
                Mat::Plaster,
                roof + v3(-0.6, 0.0, -0.2),
                3.4,
                1.7,
                hexc(0xf2ead8),
            );
        }
    }
    // The yard wall along the lot's front and sides, open before the door.
    let wall = shade(col, 0.9);
    let (wy0, wy1) = (-1.5, 1.05);
    p.cuboid(
        Mat::Plaster,
        v3(-5.8, wy0, 5.3),
        v3(-2.0, wy1, 5.6),
        wall,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-0.2, wy0, 5.3),
        v3(5.8, wy1, 5.6),
        wall,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-5.8, wy0, 1.8),
        v3(-5.5, wy1, 5.3),
        wall,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(5.5, wy0, 2.4),
        v3(5.8, wy1, 5.3),
        wall,
        false,
    );
    for x in [-2.0, -0.2] {
        p.cuboid(
            Mat::Plaster,
            v3(x - 0.22, wy0, 5.2),
            v3(x + 0.22, 1.5, 5.7),
            wall,
            false,
        );
    }
    // A clay oven in the yard.
    lump(
        &mut p,
        Mat::Clay,
        v3(3.9, -0.1, 3.9),
        1.8,
        1.05,
        shade(Vec4::ONE, 1.5),
    );
    p.quad(
        Mat::Window,
        v3(3.6, 0.1, 3.02),
        v3(4.2, 0.1, 3.02),
        v3(4.2, 0.55, 3.12),
        v3(3.6, 0.55, 3.12),
        Vec4::ONE,
    );
    p.ground_occlusion(0.7, 1.4);
    p.finish()
}

/// A merchant's house of the dry coast: two whitewashed storeys, a latticed bay and a wind
/// tower.
fn riad_house(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w, hgt) = (8.0, 6.2, 6.3);
    let c = v3(0.0, 0.0, -0.6);
    let col = [hexc(0xeadfc6), hexc(0xd9b98a), hexc(0xe2c0a8)][v % 3];
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, -2.5, -w * 0.5),
        c + v3(l * 0.5, hgt, w * 0.5),
        col,
        false,
    );
    // A darker plinth band.
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5 - 0.06, -2.5, -w * 0.5 - 0.06),
        c + v3(l * 0.5 + 0.06, 0.9, w * 0.5 + 0.06),
        shade(mud(2), 0.95),
        false,
    );
    parapet(&mut p, c + v3(0.0, hgt, 0.0), l, w, 0.5, col);
    let z = c.z + w * 0.5;
    beam_ends(&mut p, v3(0.0, hgt - 0.5, z), Vec3::Z, l - 1.0);
    beam_ends(&mut p, v3(0.0, 3.2, z), Vec3::Z, l - 1.0);
    let paint = [hexc(0x2a5f8a), hexc(0x2f7a6a), hexc(0x6a3a5a)][v % 3];
    p.door(v3(0.0, 0.0, z), Vec3::Z, 1.5, 2.5, paint);
    for x in [-2.8, 2.8] {
        p.windows(v3(x, 1.9, z), Vec3::Z, 1, 0.7, 1.1, 0.0, hexc(0x5a4632));
        shutters(&mut p, v3(x, 1.9, z), 0.7, 1.1, paint);
        p.windows(v3(x, 4.8, z), Vec3::Z, 1, 0.8, 1.3, 0.0, hexc(0x5a4632));
        shutters(&mut p, v3(x, 4.8, z), 0.8, 1.3, paint);
    }
    // The latticed bay over the door, on brackets.
    let dark = hexc(0x4a3626);
    p.cuboid(
        Mat::Planks,
        v3(-1.2, 3.7, z),
        v3(1.2, 5.7, z + 0.75),
        dark,
        true,
    );
    for x in [-1.0, 1.0] {
        p.beam(Mat::Timber, v3(x, 3.0, z), v3(x, 3.7, z + 0.7), 0.12, dark);
    }
    for k in 0..5 {
        let x = -0.96 + k as f32 * 0.48;
        p.cuboid(
            Mat::Window,
            v3(x - 0.14, 4.2, z + 0.76),
            v3(x + 0.14, 5.3, z + 0.78),
            Vec4::ONE,
            false,
        );
    }
    up_quad(
        &mut p,
        Mat::Planks,
        (-1.35, 1.35),
        (z, z + 0.9),
        5.75,
        shade(dark, 1.3),
    );
    for (n, side) in [(Vec3::X, l * 0.5), (-Vec3::X, -l * 0.5)] {
        p.windows(c + v3(side, 4.8, 0.0), n, 2, 0.7, 1.2, 1.4, hexc(0x5a4632));
        p.windows(c + v3(side, 1.9, 0.0), n, 2, 0.6, 0.9, 1.6, hexc(0x5a4632));
    }
    p.windows(
        c + v3(0.0, 4.8, -w * 0.5),
        -Vec3::Z,
        3,
        0.7,
        1.2,
        1.3,
        hexc(0x5a4632),
    );
    // The wind tower at the back.
    let t = c + v3(if v == 1 { -2.6 } else { 2.6 }, hgt, -1.7);
    p.cuboid(
        Mat::Plaster,
        t + v3(-1.0, -0.2, -1.0),
        t + v3(1.0, 3.6, 1.0),
        shade(col, 0.97),
        false,
    );
    for (n, o) in [
        (Vec3::Z, v3(0.0, 0.0, 1.01)),
        (-Vec3::Z, v3(0.0, 0.0, -1.01)),
        (Vec3::X, v3(1.01, 0.0, 0.0)),
        (-Vec3::X, v3(-1.01, 0.0, 0.0)),
    ] {
        let along = Vec3::Y.cross(n);
        for k in [-0.5f32, 0.0, 0.5] {
            let m = t + o + along * k + Vec3::Y * 2.5;
            p.quad(
                Mat::Window,
                m - along * 0.16 - Vec3::Y * 0.8,
                m + along * 0.16 - Vec3::Y * 0.8,
                m + along * 0.16 + Vec3::Y * 0.8,
                m - along * 0.16 + Vec3::Y * 0.8,
                Vec4::ONE,
            );
        }
    }
    parapet(&mut p, t + v3(0.0, 3.6, 0.0), 2.0, 2.0, 0.25, col);
    if v == 2 {
        lump(
            &mut p,
            Mat::Plaster,
            c + v3(-1.6, hgt, 0.4),
            3.6,
            1.9,
            hexc(0xf2ead8),
        );
    }
    p.ground_occlusion(0.7, 1.6);
    p.finish()
}

/// A date palm, 7 m: a ringed trunk, a crown of long fronds and bunches of dates.
fn date_palm() -> Parts {
    let mut p = Parts::default();
    let bark = hexc(0x7d6a52);
    let n = 6;
    let at = |k: usize| {
        let t = k as f32 / n as f32;
        v3(0.5 * t * t, 6.2 * t - 0.3, 0.12 * t)
    };
    for k in 0..n {
        let (a, b) = (at(k), at(k + 1));
        let r = 0.3 - 0.018 * k as f32;
        p.rod(
            Mat::Timber,
            a,
            b + (b - a) * 0.04,
            r,
            7,
            shade(bark, 0.85 + 0.06 * (k % 2) as f32),
        );
    }
    let top = at(n);
    let green = hexc(0x5c7d38);
    let fronds = 15;
    for i in 0..fronds {
        let a = i as f32 * TAU / fronds as f32 + (i % 3) as f32 * 0.13;
        let dir = v3(a.cos(), 0.0, a.sin());
        let side = v3(-a.sin(), 0.0, a.cos());
        let lift = 0.5 + 0.9 * ((i * 7 % 5) as f32 / 5.0);
        let reach = 3.3 + 0.7 * ((i * 3 % 4) as f32 / 4.0);
        let pts = [
            top,
            top + dir * reach * 0.3 + Vec3::Y * lift,
            top + dir * reach * 0.65 + Vec3::Y * (lift + 0.1),
            top + dir * reach + Vec3::Y * (lift - 1.3),
        ];
        blade(
            &mut p,
            Mat::Leaf,
            &pts,
            &[0.2, 0.95, 0.85, 0.12],
            side,
            shade(green, 0.82 + 0.03 * (i % 6) as f32),
        );
    }
    // Young fronds standing in the heart.
    for i in 0..5 {
        let a = i as f32 * TAU / 5.0 + 0.4;
        let dir = v3(a.cos(), 0.0, a.sin());
        let side = v3(-a.sin(), 0.0, a.cos());
        let pts = [
            top,
            top + dir * 0.5 + Vec3::Y * 1.2,
            top + dir * 1.1 + Vec3::Y * 2.0,
        ];
        blade(
            &mut p,
            Mat::Leaf,
            &pts,
            &[0.2, 0.6, 0.1],
            side,
            shade(green, 1.15),
        );
    }
    let dates = hexc(0xc67a2c);
    for i in 0..4 {
        let a = i as f32 * TAU / 4.0 + 0.8;
        let d = v3(a.cos(), 0.0, a.sin());
        lump(
            &mut p,
            Mat::Paint,
            top + d * 0.45 - Vec3::Y * 0.95,
            0.55,
            0.8,
            dates,
        );
    }
    p.finish()
}

/// The grove keeper's house: a small adobe with dates drying on the roof, and a well with a
/// sweep.
fn date_house() -> Parts {
    let mut p = Parts::default();
    let (l, w, hgt) = (6.2, 4.6, 3.0);
    let c = v3(-1.2, 0.0, -1.8);
    let col = mud(0);
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, -2.5, -w * 0.5),
        c + v3(l * 0.5, hgt, w * 0.5),
        col,
        false,
    );
    parapet(&mut p, c + v3(0.0, hgt, 0.0), l, w, 0.4, col);
    beam_ends(&mut p, c + v3(0.0, hgt - 0.4, w * 0.5), Vec3::Z, l - 0.8);
    p.door(
        c + v3(-0.8, 0.0, w * 0.5),
        Vec3::Z,
        1.0,
        2.0,
        hexc(0x2f7a6a),
    );
    p.windows(
        c + v3(1.6, 1.8, w * 0.5),
        Vec3::Z,
        1,
        0.6,
        0.8,
        0.0,
        hexc(0x5a4632),
    );
    up_quad(
        &mut p,
        Mat::Canvas,
        (c.x - 2.4, c.x + 2.4),
        (c.z - 1.6, c.z + 1.6),
        hgt + 0.03,
        hexc(0xa85a22),
    );
    // Mats of dates drying in the yard.
    for (x, z) in [(2.6, 2.6), (2.6, 4.4), (-0.4, 3.6)] {
        up_quad(
            &mut p,
            Mat::Canvas,
            (x - 1.2, x + 1.2),
            (z - 0.7, z + 0.7),
            0.04,
            hexc(0xd9c9a0),
        );
        up_quad(
            &mut p,
            Mat::Canvas,
            (x - 1.0, x + 1.0),
            (z - 0.5, z + 0.5),
            0.07,
            hexc(0x9a4a1e),
        );
    }
    // The well and its sweep: a long pole on a forked post, a stone on the short end.
    let well = v3(-4.0, 0.0, 3.4);
    p.cylinder(
        Mat::Stone,
        well + v3(0.0, -0.4, 0.0),
        0.9,
        0.9,
        1.1,
        12,
        Vec4::ONE,
        false,
    );
    p.cylinder(
        Mat::Window,
        well + v3(0.0, 0.3, 0.0),
        0.78,
        0.78,
        0.02,
        12,
        Vec4::ONE,
        true,
    );
    let post = well + v3(1.9, 0.0, -0.4);
    p.beam(Mat::Timber, post, post + Vec3::Y * 3.0, 0.18, Vec4::ONE);
    let (tip, tail) = (well + v3(0.0, 4.2, 0.0), post + v3(1.9, 1.9, -0.4));
    p.rod(Mat::Timber, tail, tip, 0.07, 6, Vec4::ONE);
    p.box_at(Mat::Stone, tail, v3(0.3, 0.25, 0.25), 0.4, Vec4::ONE);
    p.rod(
        Mat::Timber,
        tip,
        well + v3(0.0, 1.0, 0.0),
        0.02,
        4,
        hexc(0x8a7a5a),
    );
    p.ground_occlusion(0.7, 1.4);
    p.finish()
}

/// A 4 m run of mud wall round a grove.
fn mud_wall_4() -> Parts {
    let mut p = Parts::default();
    let col = shade(mud(0), 0.92);
    p.cuboid(
        Mat::Plaster,
        v3(-2.05, -0.6, -0.17),
        v3(2.05, 0.85, 0.17),
        col,
        false,
    );
    p.beam(
        Mat::Plaster,
        v3(-2.05, 0.82, 0.0),
        v3(2.05, 0.82, 0.0),
        0.26,
        shade(col, 1.06),
    );
    p.ground_occlusion(0.7, 0.7);
    p.finish()
}

/// A 4 m salt pan: brine inside a clay bund. (White; a piece's tint makes its brine sea
/// green, rose or dry.)
fn salt_pan() -> Parts {
    let mut p = Parts::default();
    let bund = hexc(0xcbb89a);
    let (o, i) = (2.0, 1.72);
    p.cuboid(Mat::Plaster, v3(-o, -0.4, i), v3(o, 0.16, o), bund, false);
    p.cuboid(Mat::Plaster, v3(-o, -0.4, -o), v3(o, 0.16, -i), bund, false);
    p.cuboid(Mat::Plaster, v3(-o, -0.4, -i), v3(-i, 0.16, i), bund, false);
    p.cuboid(Mat::Plaster, v3(i, -0.4, -i), v3(o, 0.16, i), bund, false);
    up_quad(&mut p, Mat::Brine, (-i, i), (-i, i), 0.08, Vec4::ONE);
    // A crust of salt along the edges.
    for (x, z) in [((-i, i), (i - 0.22, i)), ((-i, i), (-i, -i + 0.22))] {
        up_quad(&mut p, Mat::Snow, x, z, 0.1, SNOW);
    }
    p.finish()
}

/// A raked heap of salt.
fn salt_heap() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Snow,
        v3(0.0, -0.05, 0.0),
        1.25,
        0.12,
        1.15,
        12,
        SNOW,
        true,
    );
    p.finish()
}

/// The salt store: a mud-brick store with two arched doors, sacks and heaps.
fn salt_house() -> Parts {
    let mut p = Parts::default();
    let (l, w, hgt) = (7.6, 4.8, 3.4);
    let c = v3(-0.6, 0.0, -2.6);
    let col = mud(1);
    p.cuboid(
        Mat::Plaster,
        c + v3(-l * 0.5, -2.5, -w * 0.5),
        c + v3(l * 0.5, hgt, w * 0.5),
        col,
        false,
    );
    parapet(&mut p, c + v3(0.0, hgt, 0.0), l, w, 0.4, col);
    beam_ends(&mut p, c + v3(0.0, hgt - 0.45, w * 0.5), Vec3::Z, l - 0.8);
    let z = c.z + w * 0.5 + 0.02;
    for x in [c.x - 1.9, c.x + 1.9] {
        p.quad(
            Mat::Window,
            v3(x - 0.9, 0.0, z),
            v3(x + 0.9, 0.0, z),
            v3(x + 0.9, 2.1, z),
            v3(x - 0.9, 2.1, z),
            Vec4::ONE,
        );
        p.tri(
            Mat::Window,
            v3(x - 0.9, 2.1, z),
            v3(x + 0.9, 2.1, z),
            v3(x, 2.75, z),
            Vec4::ONE,
        );
    }
    // Salt in heaps and sacks before the doors.
    for (x, zz, r) in [(-3.6, 1.6, 1.3), (-1.4, 2.4, 1.0), (2.9, 1.2, 1.15)] {
        p.cylinder(
            Mat::Snow,
            v3(x, -0.05, zz),
            r,
            0.1,
            r * 0.85,
            12,
            SNOW,
            true,
        );
    }
    for k in 0..6 {
        let (row, i) = (k / 3, k % 3);
        p.box_at(
            Mat::Canvas,
            v3(
                0.6 + i as f32 * 0.75,
                0.22 + row as f32 * 0.4,
                1.0 + row as f32 * 0.12,
            ),
            v3(0.34, 0.2, 0.5),
            0.1 * i as f32,
            hexc(0xd9c9a0),
        );
    }
    // Rakes leaning on the wall.
    for x in [c.x + 3.3, c.x + 3.55] {
        p.rod(
            Mat::Timber,
            v3(x, 0.0, c.z + w * 0.5 + 0.7),
            v3(x, 2.3, c.z + w * 0.5 + 0.08),
            0.03,
            5,
            Vec4::ONE,
        );
    }
    p.ground_occlusion(0.7, 1.4);
    p.finish()
}

/// A wind pump: a lattice tower, a wheel of blades and a vane, lifting sea water to pans.
fn windpump() -> Parts {
    let mut p = Parts::default();
    let iron = shade(Vec4::ONE, 0.7);
    let (foot, head, h) = (1.3, 0.28, 8.0);
    let corner = |sx: f32, sz: f32, t: f32| {
        let r = foot + (head - foot) * t;
        v3(sx * r, h * t, sz * r)
    };
    let c4 = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for (sx, sz) in c4 {
        p.beam(
            Mat::Timber,
            corner(sx, sz, -0.1),
            corner(sx, sz, 1.0),
            0.12,
            iron,
        );
    }
    for k in 0..4 {
        let (t0, t1) = (k as f32 * 0.25, (k + 1) as f32 * 0.25);
        for i in 0..4 {
            let (a, b) = (c4[i], c4[(i + 1) % 4]);
            p.beam(
                Mat::Timber,
                corner(a.0, a.1, t1),
                corner(b.0, b.1, t1),
                0.07,
                iron,
            );
            p.beam(
                Mat::Timber,
                corner(a.0, a.1, t0),
                corner(b.0, b.1, t1),
                0.05,
                iron,
            );
        }
    }
    let hub = v3(0.0, h + 0.3, 0.45);
    p.rod(Mat::Iron, v3(0.0, h + 0.3, -0.2), hub, 0.1, 6, Vec4::ONE);
    let pale = hexc(0xe6e2d6);
    let blades = 14;
    for i in 0..blades {
        let a = i as f32 * TAU / blades as f32;
        let d = v3(a.cos(), a.sin(), 0.0);
        let s = v3(-a.sin(), a.cos(), 0.12);
        let (r0, r1) = (0.45, 1.75);
        p.quad(
            Mat::Paint,
            hub + d * r0 - s * 0.08,
            hub + d * r0 + s * 0.08,
            hub + d * r1 + s * 0.3,
            hub + d * r1 - s * 0.3,
            pale,
        );
        p.quad(
            Mat::Paint,
            hub + d * r0 + s * 0.08,
            hub + d * r0 - s * 0.08,
            hub + d * r1 - s * 0.3,
            hub + d * r1 + s * 0.3,
            shade(pale, 0.7),
        );
    }
    // The tail vane.
    p.beam(
        Mat::Iron,
        v3(0.0, h + 0.3, -0.2),
        v3(0.0, h + 0.3, -2.2),
        0.06,
        Vec4::ONE,
    );
    for flip in [false, true] {
        let (a, b, c, d) = (
            v3(0.0, h - 0.25, -1.5),
            v3(0.0, h - 0.4, -2.6),
            v3(0.0, h + 1.0, -2.6),
            v3(0.0, h + 0.85, -1.5),
        );
        if flip {
            p.quad(Mat::Paint, d, c, b, a, hexc(0x9a3a2a));
        } else {
            p.quad(Mat::Paint, a, b, c, d, hexc(0x9a3a2a));
        }
    }
    // The pump rod and a trough.
    p.rod(
        Mat::Iron,
        v3(0.0, 0.0, 0.0),
        v3(0.0, h, 0.0),
        0.03,
        4,
        Vec4::ONE,
    );
    p.cuboid(
        Mat::Planks,
        v3(-0.4, 0.0, 0.5),
        v3(0.4, 0.4, 2.2),
        shade(Vec4::ONE, 0.7),
        true,
    );
    p.ground_occlusion(0.7, 1.0);
    p.finish()
}

/// A clay storage jar.
fn jar() -> Parts {
    let mut p = Parts::default();
    let clay = shade(Vec4::ONE, 1.7);
    p.cylinder(
        Mat::Clay,
        v3(0.0, 0.0, 0.0),
        0.2,
        0.42,
        0.45,
        10,
        clay,
        false,
    );
    p.cylinder(
        Mat::Clay,
        v3(0.0, 0.45, 0.0),
        0.42,
        0.3,
        0.35,
        10,
        clay,
        false,
    );
    p.cylinder(
        Mat::Clay,
        v3(0.0, 0.8, 0.0),
        0.3,
        0.17,
        0.14,
        10,
        clay,
        false,
    );
    p.cylinder(
        Mat::Clay,
        v3(0.0, 0.94, 0.0),
        0.17,
        0.21,
        0.1,
        10,
        shade(clay, 0.8),
        true,
    );
    p.finish()
}

// --------------------------------------------------------------------- Frostholm: ice

/// A fisher's cabin of the north: boarded walls on stone, a steep turf roof under snow, a
/// stone chimney and a porch against the wind.
fn turf_cabin(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (6.6, 4.8, 2.3);
    let c = v3(0.0, 0.0, -0.2);
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5 - 0.15, -3.0, -w * 0.5 - 0.15),
        c + v3(l * 0.5 + 0.15, 0.35, w * 0.5 + 0.15),
        Vec4::splat(0.78).with_w(1.0),
        false,
    );
    let paint = [hexc(0x7d2a22), hexc(0x3a2c24), hexc(0xb98a3a)][v % 3];
    p.cuboid(
        Mat::Planks,
        c + v3(-l * 0.5, 0.35, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        paint,
        false,
    );
    for x in [-l * 0.5, l * 0.5] {
        for z in [-w * 0.5, w * 0.5] {
            p.beam(
                Mat::Paint,
                c + v3(x, 0.35, z),
                c + v3(x, wall, z),
                0.16,
                WHITE,
            );
        }
    }
    let turf = hexc(0x6b7438);
    let (roof, rcol, share) = match v % 3 {
        0 => (Mat::Thatch, turf, 0.92),
        1 => (Mat::Thatch, shade(turf, 0.9), 0.6),
        _ => (Mat::Slate, Vec4::ONE, 0.78),
    };
    let rc = c + v3(0.0, wall, 0.0);
    p.gable_roof(roof, Mat::Planks, rc, l, w, 3.0, 0.5, false, rcol, paint);
    roof_snow(&mut p, rc, l, w, 3.0, 0.5, false, share);
    // The chimney at one end.
    let cx = if v == 2 { -l * 0.3 } else { l * 0.3 };
    p.cuboid(
        Mat::Stone,
        c + v3(cx - 0.45, wall, -0.45),
        c + v3(cx + 0.45, wall + 3.7, 0.45),
        Vec4::splat(0.85).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Snow,
        c + v3(cx - 0.52, wall + 3.7, -0.52),
        c + v3(cx + 0.52, wall + 3.82, 0.52),
        SNOW,
        false,
    );
    // The porch.
    let px = -1.2;
    let z = c.z + w * 0.5;
    p.cuboid(
        Mat::Planks,
        v3(px - 0.85, 0.2, z),
        v3(px + 0.85, 2.05, z + 1.25),
        shade(paint, 0.92),
        false,
    );
    let pc = v3(px, 2.05, z + 0.62);
    p.gable_roof(
        roof,
        Mat::Planks,
        pc,
        1.25,
        1.7,
        0.85,
        0.2,
        true,
        rcol,
        paint,
    );
    roof_snow(&mut p, pc, 1.25, 1.7, 0.85, 0.2, true, 0.95);
    let door = [hexc(0x2f4a3a), hexc(0x9a3a2a), hexc(0x2f4a5a)][v % 3];
    p.door(v3(px, 0.2, z + 1.25), Vec3::Z, 0.9, 1.8, door);
    p.windows(v3(1.7, 1.5, z), Vec3::Z, 1, 0.95, 0.9, 0.0, WHITE);
    p.windows(
        c + v3(0.0, 1.5, -w * 0.5),
        -Vec3::Z,
        2,
        0.8,
        0.8,
        1.6,
        WHITE,
    );
    p.windows(c + v3(l * 0.5, 1.5, 0.0), Vec3::X, 1, 0.8, 0.8, 0.0, WHITE);
    // Snow banked against the weather wall, firewood under the eaves.
    lump(
        &mut p,
        Mat::Snow,
        c + v3(-l * 0.5 - 0.5, -0.1, -0.6),
        2.6,
        0.75,
        SNOW,
    );
    lump(
        &mut p,
        Mat::Snow,
        c + v3(1.4, -0.1, -w * 0.5 - 0.5),
        3.0,
        0.6,
        SNOW,
    );
    let logs = hexc(0x9a7a56);
    for k in 0..10 {
        let (row, i) = (k / 5, k % 5);
        let x = c.x + l * 0.5 + 0.35;
        let z = c.z - 1.3 + i as f32 * 0.3 + row as f32 * 0.15;
        p.rod(
            Mat::Timber,
            v3(x - 0.3, 0.15 + row as f32 * 0.28, z),
            v3(x + 0.3, 0.15 + row as f32 * 0.28, z),
            0.14,
            6,
            shade(logs, 0.85 + 0.04 * i as f32),
        );
    }
    p.ground_occlusion(0.7, 1.2);
    p.finish()
}

/// A northern town house: two painted timber storeys with white trim under a snowy slate
/// roof.
fn lodge_house(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w, wall) = (8.0, 6.0, 5.4);
    let c = v3(0.0, 0.0, -0.4);
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5 - 0.15, -3.0, -w * 0.5 - 0.15),
        c + v3(l * 0.5 + 0.15, 0.6, w * 0.5 + 0.15),
        Vec4::splat(0.78).with_w(1.0),
        false,
    );
    let paint = [hexc(0x8a2a22), hexc(0xd2aa48), hexc(0x5a7a8a)][v % 3];
    p.cuboid(
        Mat::Planks,
        c + v3(-l * 0.5, 0.6, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        paint,
        false,
    );
    for x in [-l * 0.5, l * 0.5] {
        for z in [-w * 0.5, w * 0.5] {
            p.beam(
                Mat::Paint,
                c + v3(x, 0.6, z),
                c + v3(x, wall, z),
                0.2,
                WHITE,
            );
        }
    }
    for z in [-w * 0.5, w * 0.5] {
        p.beam(
            Mat::Paint,
            c + v3(-l * 0.5, 3.0, z),
            c + v3(l * 0.5, 3.0, z),
            0.14,
            WHITE,
        );
    }
    let rc = c + v3(0.0, wall, 0.0);
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        rc,
        l,
        w,
        3.3,
        0.45,
        false,
        Vec4::ONE,
        paint,
    );
    roof_snow(&mut p, rc, l, w, 3.3, 0.45, false, 0.82);
    p.cuboid(
        Mat::Brick,
        c + v3(2.0, wall + 1.0, -0.4),
        c + v3(2.8, wall + 4.3, 0.4),
        Vec4::ONE,
        false,
    );
    p.cuboid(
        Mat::Snow,
        c + v3(1.94, wall + 4.3, -0.46),
        c + v3(2.86, wall + 4.4, 0.46),
        SNOW,
        false,
    );
    let z = c.z + w * 0.5;
    // A porch with its own little roof.
    for x in [-2.0, -0.4] {
        p.beam(
            Mat::Paint,
            v3(x, 0.0, z + 1.1),
            v3(x, 2.7, z + 1.1),
            0.14,
            WHITE,
        );
    }
    p.cuboid(
        Mat::Planks,
        v3(-2.1, 0.3, z),
        v3(-0.3, 0.6, z + 1.2),
        shade(Vec4::ONE, 0.8),
        false,
    );
    let pc = v3(-1.2, 2.7, z + 0.6);
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        pc,
        1.2,
        2.0,
        0.8,
        0.2,
        true,
        Vec4::ONE,
        WHITE,
    );
    roof_snow(&mut p, pc, 1.2, 2.0, 0.8, 0.2, true, 0.95);
    p.door(v3(-1.2, 0.6, z), Vec3::Z, 1.0, 2.0, hexc(0x2f4a3a));
    p.windows(v3(1.9, 2.0, z), Vec3::Z, 2, 0.9, 1.2, 0.5, WHITE);
    p.windows(v3(0.0, 4.2, z), Vec3::Z, 4, 0.85, 1.1, 0.9, WHITE);
    for (n, side) in [(Vec3::X, l * 0.5), (-Vec3::X, -l * 0.5)] {
        p.windows(c + v3(side, 2.0, 0.0), n, 2, 0.85, 1.2, 1.2, WHITE);
        p.windows(c + v3(side, 4.2, 0.0), n, 2, 0.85, 1.1, 1.2, WHITE);
        p.windows(c + v3(side, 6.5, 0.0), n, 1, 0.7, 0.8, 0.0, WHITE);
    }
    p.windows(
        c + v3(0.0, 4.2, -w * 0.5),
        -Vec3::Z,
        3,
        0.85,
        1.1,
        1.4,
        WHITE,
    );
    lump(
        &mut p,
        Mat::Snow,
        c + v3(-l * 0.5 - 0.5, -0.1, 0.8),
        2.8,
        0.8,
        SNOW,
    );
    lump(
        &mut p,
        Mat::Snow,
        c + v3(2.2, -0.1, -w * 0.5 - 0.5),
        3.2,
        0.7,
        SNOW,
    );
    p.ground_occlusion(0.7, 1.6);
    p.finish()
}

/// A trapper's log cabin with a stone chimney and snowshoes by the door.
fn trapper() -> Parts {
    let mut p = Parts::default();
    let (l, w) = (5.6, 4.2);
    let c = v3(-1.6, 0.0, -1.0);
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5 - 0.1, -3.0, -w * 0.5 - 0.1),
        c + v3(l * 0.5 + 0.1, 0.3, w * 0.5 + 0.1),
        Vec4::splat(0.78).with_w(1.0),
        false,
    );
    let col = hexc(0x6b4a30);
    let mut y = 0.45;
    while y < 2.6 {
        for z in [w * 0.5, -w * 0.5] {
            p.rod(
                Mat::Timber,
                c + v3(-l * 0.5 - 0.25, y, z),
                c + v3(l * 0.5 + 0.25, y, z),
                0.17,
                6,
                col,
            );
        }
        for x in [l * 0.5, -l * 0.5] {
            p.rod(
                Mat::Timber,
                c + v3(x, y + 0.17, -w * 0.5 - 0.25),
                c + v3(x, y + 0.17, w * 0.5 + 0.25),
                0.17,
                6,
                col,
            );
        }
        y += 0.34;
    }
    p.cuboid(
        Mat::Timber,
        c + v3(-l * 0.5 + 0.1, 0.3, -w * 0.5 + 0.1),
        c + v3(l * 0.5 - 0.1, 2.6, w * 0.5 - 0.1),
        shade(col, 0.6),
        false,
    );
    let rc = c + v3(0.0, 2.6, 0.0);
    p.gable_roof(
        Mat::Planks,
        Mat::Timber,
        rc,
        l,
        w,
        2.3,
        0.6,
        false,
        shade(Vec4::ONE, 0.6),
        col,
    );
    roof_snow(&mut p, rc, l, w, 2.3, 0.6, false, 0.9);
    p.cuboid(
        Mat::Stone,
        c + v3(-l * 0.5 - 0.9, -1.0, -0.6),
        c + v3(-l * 0.5 + 0.05, 5.6, 0.6),
        Vec4::splat(0.85).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Snow,
        c + v3(-l * 0.5 - 0.96, 5.6, -0.66),
        c + v3(-l * 0.5 + 0.1, 5.7, 0.66),
        SNOW,
        false,
    );
    p.door(
        c + v3(0.7, 0.3, w * 0.5 + 0.17),
        Vec3::Z,
        0.9,
        1.85,
        hexc(0x4a3a2a),
    );
    p.windows(
        c + v3(-1.2, 1.6, w * 0.5 + 0.15),
        Vec3::Z,
        1,
        0.7,
        0.7,
        0.0,
        WHITE,
    );
    // Antlers over the door and snowshoes on the wall.
    let bone = hexc(0xd9d0b8);
    for s in [-1.0f32, 1.0] {
        let b = c + v3(0.7, 2.35, w * 0.5 + 0.3);
        p.rod(Mat::Paint, b, b + v3(s * 0.5, 0.35, 0.0), 0.03, 4, bone);
        p.rod(
            Mat::Paint,
            b + v3(s * 0.3, 0.2, 0.0),
            b + v3(s * 0.35, 0.55, 0.0),
            0.025,
            4,
            bone,
        );
    }
    for x in [1.75, 2.15] {
        let b = c + v3(x, 0.4, w * 0.5 + 0.35);
        p.cylinder(Mat::Timber, b, 0.02, 0.02, 0.01, 4, Vec4::ONE, false);
        p.box_at(
            Mat::Timber,
            b + v3(0.0, 0.55, 0.0),
            v3(0.16, 0.5, 0.02),
            0.0,
            hexc(0xb8926a),
        );
    }
    lump(
        &mut p,
        Mat::Snow,
        c + v3(l * 0.5 + 0.6, -0.1, -1.0),
        2.4,
        0.7,
        SNOW,
    );
    p.ground_occlusion(0.7, 1.2);
    p.finish()
}

/// A pelt stretched on a frame. (Pale; a piece's tint makes it the fur's colour.)
fn pelt_frame() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x9a7a56);
    let (hw, h) = (0.62, 1.75);
    let lean = 0.3;
    let at = |x: f32, y: f32| v3(x, y, -lean * y / h);
    p.beam(Mat::Timber, at(-hw, -0.2), at(-hw, h), 0.07, wood);
    p.beam(Mat::Timber, at(hw, -0.2), at(hw, h), 0.07, wood);
    p.beam(Mat::Timber, at(-hw, h), at(hw, h), 0.07, wood);
    p.beam(Mat::Timber, at(-hw, 0.3), at(hw, 0.3), 0.07, wood);
    p.beam(Mat::Timber, v3(0.0, 0.0, -1.0), at(0.0, h), 0.06, wood);
    let o = Vec3::Z * 0.05;
    let fur = Vec4::new(1.25, 1.25, 1.25, 1.0);
    p.quad(
        Mat::Wool,
        at(-0.38, 0.45) + o,
        at(0.38, 0.45) + o,
        at(0.48, 1.6) + o,
        at(-0.48, 1.6) + o,
        fur,
    );
    p.quad(
        Mat::Wool,
        at(0.38, 0.45) - o,
        at(-0.38, 0.45) - o,
        at(-0.48, 1.6) - o,
        at(0.48, 1.6) - o,
        shade(fur, 0.8),
    );
    p.finish()
}

/// A freight sled with a lashed load.
fn sled() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x9a7a56);
    for z in [-0.42, 0.42] {
        p.beam(Mat::Timber, v3(-1.3, 0.06, z), v3(1.0, 0.06, z), 0.08, wood);
        p.beam(Mat::Timber, v3(1.0, 0.06, z), v3(1.45, 0.5, z), 0.08, wood);
        for x in [-0.9, 0.0, 0.8] {
            p.beam(Mat::Timber, v3(x, 0.06, z), v3(x, 0.36, z), 0.05, wood);
        }
        p.beam(Mat::Timber, v3(-1.3, 0.36, z), v3(1.45, 0.5, z), 0.05, wood);
    }
    for x in [-1.1, -0.5, 0.1, 0.7] {
        p.beam(
            Mat::Timber,
            v3(x, 0.36, -0.45),
            v3(x, 0.36, 0.45),
            0.06,
            wood,
        );
    }
    p.box_at(
        Mat::Canvas,
        v3(-0.3, 0.62, 0.0),
        v3(0.8, 0.24, 0.36),
        0.0,
        hexc(0xa89a7a),
    );
    p.box_at(
        Mat::Wool,
        v3(-0.5, 0.92, 0.0),
        v3(0.45, 0.1, 0.3),
        0.2,
        Vec4::new(0.8, 0.6, 0.45, 1.0),
    );
    // The handle bars at the back.
    for z in [-0.42, 0.42] {
        p.beam(
            Mat::Timber,
            v3(-1.25, 0.36, z),
            v3(-1.5, 1.05, z),
            0.05,
            wood,
        );
    }
    p.beam(
        Mat::Timber,
        v3(-1.5, 1.05, -0.42),
        v3(-1.5, 1.05, 0.42),
        0.05,
        wood,
    );
    p.finish()
}

/// A sled dog, sitting up. (Pale; a piece's tint gives its coat.)
fn husky() -> Parts {
    let mut p = Parts::default();
    let coat = Vec4::new(1.2, 1.2, 1.2, 1.0);
    let prof: Vec<(f32, f32)> = (0..=7)
        .map(|i| {
            let t = i as f32 / 7.0;
            (
                -0.42 + t * 0.84,
                0.2 * (1.0 - (2.0 * t - 1.0).powi(2)).max(0.0).sqrt() + 0.002,
            )
        })
        .collect();
    let mut body = Parts::default();
    body.lathe_x(Mat::Wool, &prof, 8, |_, _| coat);
    p.add_parts(&body, Mat4::from_translation(v3(0.0, 0.5, 0.0)));
    p.box_at(
        Mat::Wool,
        v3(0.48, 0.72, 0.0),
        v3(0.13, 0.11, 0.1),
        0.0,
        coat,
    );
    p.box_at(
        Mat::Wool,
        v3(0.64, 0.68, 0.0),
        v3(0.08, 0.05, 0.05),
        0.0,
        shade(coat, 0.5),
    );
    for z in [-0.06, 0.06] {
        p.cylinder(
            Mat::Wool,
            v3(0.44, 0.82, z),
            0.04,
            0.005,
            0.12,
            4,
            shade(coat, 0.7),
            false,
        );
    }
    for (x, z) in [(-0.28, -0.1), (-0.28, 0.1), (0.28, -0.1), (0.28, 0.1)] {
        p.beam(
            Mat::Wool,
            v3(x, 0.0, z),
            v3(x, 0.4, z),
            0.07,
            shade(coat, 0.9),
        );
    }
    p.rod(
        Mat::Wool,
        v3(-0.4, 0.55, 0.0),
        v3(-0.52, 0.85, 0.0),
        0.05,
        5,
        coat,
    );
    p.finish()
}

/// Snow blown into a drift.
fn snow_drift() -> Parts {
    let mut p = Parts::default();
    // Low mounds run together: wide at the foot, rounded over the top.
    for (x, z, r, h) in [
        (0.0, 0.0, 1.7, 0.62),
        (1.25, 0.35, 1.15, 0.44),
        (-1.1, -0.45, 1.0, 0.38),
        (0.3, -1.0, 0.85, 0.3),
        (-0.5, 0.95, 0.75, 0.26),
    ] {
        let rings = [(0.0, 1.0), (0.4, 0.86), (0.75, 0.58), (1.0, 0.18)];
        for k in 0..3 {
            let (y0, r0) = rings[k];
            let (y1, r1) = rings[k + 1];
            p.cylinder(
                Mat::Snow,
                v3(x, -0.1 + y0 * h, z),
                r * r0,
                r * r1,
                (y1 - y0) * h,
                11,
                shade(SNOW, 0.93 + 0.035 * k as f32),
                k == 2,
            );
        }
    }
    p.finish()
}

/// A rack of split fish drying in the wind.
fn fish_rack() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x8a7a66);
    for x in [-1.9, 1.9] {
        p.beam(Mat::Timber, v3(x, -0.3, -0.7), v3(x, 2.4, 0.0), 0.1, wood);
        p.beam(Mat::Timber, v3(x, -0.3, 0.7), v3(x, 2.4, 0.0), 0.1, wood);
    }
    for (y, z) in [(2.35, 0.0), (1.6, -0.2), (1.6, 0.2)] {
        p.beam(Mat::Timber, v3(-2.1, y, z), v3(2.1, y, z), 0.07, wood);
    }
    let fish = hexc(0xb9b4a4);
    for (y, z) in [(2.35, 0.0), (1.6, -0.2), (1.6, 0.2)] {
        for k in 0..12 {
            let x = -1.7 + k as f32 * 0.31;
            p.box_at(
                Mat::Canvas,
                v3(x, y - 0.3, z),
                v3(0.07, 0.26, 0.02),
                0.3 * ((k % 3) as f32 - 1.0),
                shade(fish, 0.85 + 0.05 * (k % 4) as f32),
            );
        }
    }
    p.finish()
}

/// The oil works on its quay: a brick boiling house under snow, two try-pots on their
/// furnace, a stack, an oil vat, a derrick and whaleboats.
fn oil_works() -> Parts {
    let mut p = Parts::default();
    // The quay: stone under the whole yard, planks out over the water at the front.
    p.cuboid(
        Mat::Stone,
        v3(-8.0, -5.0, -8.0),
        v3(8.0, 0.0, 4.6),
        Vec4::splat(0.75).with_w(1.0),
        false,
    );
    p.cuboid(
        Mat::Planks,
        v3(-7.6, -0.3, 4.6),
        v3(7.6, 0.0, 8.0),
        shade(Vec4::ONE, 0.85),
        true,
    );
    for x in [-7.2, -3.6, 0.0, 3.6, 7.2] {
        for z in [5.4, 7.7] {
            p.rod(
                Mat::Timber,
                v3(x, -6.0, z),
                v3(x, -0.25, z),
                0.16,
                6,
                shade(Vec4::ONE, 0.55),
            );
        }
    }
    // The boiling house.
    let (l, w, wall) = (8.4, 5.0, 3.6);
    let c = v3(-3.2, 0.0, -5.0);
    p.cuboid(
        Mat::Brick,
        c + v3(-l * 0.5, 0.0, -w * 0.5),
        c + v3(l * 0.5, wall, w * 0.5),
        shade(Vec4::ONE, 0.85),
        false,
    );
    let rc = c + v3(0.0, wall, 0.0);
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        rc,
        l,
        w,
        2.4,
        0.4,
        false,
        Vec4::ONE,
        shade(Vec4::ONE, 0.85),
    );
    roof_snow(&mut p, rc, l, w, 2.4, 0.4, false, 0.85);
    p.door(c + v3(1.6, 0.0, w * 0.5), Vec3::Z, 2.0, 2.6, hexc(0x2a3a3a));
    p.windows(c + v3(-1.8, 1.9, w * 0.5), Vec3::Z, 2, 0.9, 1.1, 0.8, WHITE);
    p.windows(
        c + v3(-l * 0.5, 1.9, 0.0),
        -Vec3::X,
        2,
        0.9,
        1.1,
        0.8,
        WHITE,
    );
    // The try-works: a brick furnace with two iron pots, and its stack.
    p.cuboid(
        Mat::Brick,
        v3(2.0, 0.0, -6.4),
        v3(6.2, 1.25, -3.8),
        Vec4::ONE,
        false,
    );
    for x in [3.05, 5.15] {
        p.cylinder(
            Mat::Iron,
            v3(x, 1.25, -5.1),
            0.85,
            0.95,
            0.45,
            14,
            shade(Vec4::ONE, 0.45),
            false,
        );
        p.cylinder(
            Mat::Window,
            v3(x, 1.6, -5.1),
            0.86,
            0.86,
            0.02,
            14,
            Vec4::ONE,
            true,
        );
        p.quad(
            Mat::Lamp,
            v3(x - 0.3, 0.2, -3.78),
            v3(x + 0.3, 0.2, -3.78),
            v3(x + 0.3, 0.6, -3.78),
            v3(x - 0.3, 0.6, -3.78),
            Vec4::ONE,
        );
    }
    stack(&mut p, v3(6.9, 0.0, -6.9), 0.85, 11.5);
    // The oil vat.
    vat(&mut p, v3(-5.6, 0.0, 0.6), 1.9, 2.7, hexc(0x5a4632));
    p.cylinder(
        Mat::Snow,
        v3(-5.6, 2.7, 0.6),
        1.85,
        1.2,
        0.18,
        14,
        SNOW,
        true,
    );
    // A derrick at the quay's edge.
    let foot = v3(-1.4, 0.0, 3.8);
    p.beam(Mat::Timber, foot, foot + Vec3::Y * 6.2, 0.26, Vec4::ONE);
    let tip = foot + v3(0.4, 5.2, 4.2);
    p.beam(Mat::Timber, foot + Vec3::Y * 1.2, tip, 0.18, Vec4::ONE);
    p.rod(
        Mat::Iron,
        foot + Vec3::Y * 6.1,
        tip,
        0.025,
        4,
        shade(Vec4::ONE, 0.4),
    );
    p.rod(
        Mat::Iron,
        tip,
        tip - Vec3::Y * 3.4,
        0.025,
        4,
        shade(Vec4::ONE, 0.4),
    );
    // Whaleboats: one on trestles, one keel-up under snow.
    let mut boat = small_boat();
    for m in boat.meshes.values_mut() {
        for v in &mut m.vertices {
            v.color = [0.86, 0.86, 0.82, 1.0];
        }
    }
    p.add_parts(
        &boat,
        Mat4::from_scale_rotation_translation(
            Vec3::splat(1.7),
            Quat::from_rotation_y(0.15),
            v3(3.4, 0.45, 5.9),
        ),
    );
    for x in [2.0, 4.8] {
        p.beam(
            Mat::Timber,
            v3(x, 0.0, 5.3),
            v3(x, 0.5, 5.9),
            0.1,
            Vec4::ONE,
        );
        p.beam(
            Mat::Timber,
            v3(x, 0.0, 6.5),
            v3(x, 0.5, 5.9),
            0.1,
            Vec4::ONE,
        );
    }
    p.add_parts(
        &boat,
        Mat4::from_scale_rotation_translation(
            Vec3::splat(1.6),
            Quat::from_rotation_y(-1.3) * Quat::from_rotation_x(PI),
            v3(-6.3, 0.95, 5.6),
        ),
    );
    p.ground_occlusion(0.7, 1.8);
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_far_house_has_a_model_and_the_home_isles_keep_their_own() {
        for biome in [Biome::Tropical, Biome::Arid, Biome::Arctic] {
            for tier in [Tier::Fisherfolk, Tier::Labourers] {
                for v in 0..3 {
                    let key = house_model(biome, tier, v).unwrap();
                    assert!(MODELS.contains(&key));
                    assert!(build(key).is_some(), "{key}");
                }
            }
        }
        assert!(house_model(Biome::Temperate, Tier::Fisherfolk, 0).is_none());
    }

    #[test]
    fn snow_lies_on_top_of_its_roof() {
        let mut p = Parts::default();
        roof_snow(&mut p, v3(0.0, 2.0, 0.0), 6.0, 4.0, 3.0, 0.5, false, 0.9);
        let m = &p.meshes[&Mat::Snow];
        // Facing up, and never below the eaves.
        for v in &m.vertices {
            assert!(v.position[1] > 1.2, "{:?}", v.position);
        }
        let up = m.vertices.iter().filter(|v| v.normal[1] > 0.2).count();
        assert!(
            up * 10 >= m.vertices.len() * 7,
            "{up} of {}",
            m.vertices.len()
        );
    }
}
