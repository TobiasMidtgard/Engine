//! The fourth and fifth tiers' buildings: Masters' and Patricians' houses in each climate's
//! style, the works of their goods (ore and gold mines, ironworks, coffee and cacao groves,
//! roastery, clockmaker, chocolatier, silk farm, couturier, goldsmith) and their two public
//! buildings (theatre, bank).
//!
//! Models stand in a building's own space (origin at the middle of the footprint at floor
//! height, front toward +Z), in the same art kit as `looks`, `regional`, `civic` and
//! `burgher`.

use std::f32::consts::{PI, TAU};

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::burgher::{awning, chimney, dormer, hanging_sign, mansard, steps, window_band};
use crate::civic::dome;
use crate::defs::{Kind, Tier};
use crate::globe::Biome;
use crate::looks::{ModelKey, Piece, WHITE, h01};
use crate::regional::{parapet, roof_snow, up_quad};

/// The models this module builds (they are listed in `looks::MODELS`; a test checks that).
#[cfg(test)]
pub const MODELS: &[ModelKey] = &[
    "guild_0",
    "guild_1",
    "guild_2",
    "palazzo_0",
    "palazzo_1",
    "palazzo_2",
    "veranda_0",
    "veranda_1",
    "veranda_2",
    "plantation_0",
    "plantation_1",
    "plantation_2",
    "tower_house_0",
    "tower_house_1",
    "tower_house_2",
    "palace_0",
    "palace_1",
    "palace_2",
    "stave_hall_0",
    "stave_hall_1",
    "stave_hall_2",
    "copper_manor_0",
    "copper_manor_1",
    "copper_manor_2",
    "ore_mine",
    "ore_heap",
    "ironworks",
    "coffee_bush",
    "shade_tree",
    "drying_floor",
    "roastery",
    "clockmaker",
    "cacao_tree",
    "chocolatier",
    "mulberry_row",
    "silk_shed",
    "couturier",
    "gold_mine",
    "goldsmith",
    "theatre",
    "bank",
    "obelisk",
];

pub fn build(key: ModelKey) -> Option<Parts> {
    let v = |s: &str| s.as_bytes()[s.len() - 1].wrapping_sub(b'0') as usize % 3;
    Some(match key {
        k if k.starts_with("guild_") => guild(v(k)),
        k if k.starts_with("palazzo_") => palazzo(v(k)),
        k if k.starts_with("veranda_") => veranda(v(k)),
        k if k.starts_with("plantation_") => plantation(v(k)),
        k if k.starts_with("tower_house_") => tower_house(v(k)),
        k if k.starts_with("palace_") => palace(v(k)),
        k if k.starts_with("stave_hall_") => stave_hall(v(k)),
        k if k.starts_with("copper_manor_") => copper_manor(v(k)),
        "ore_mine" => ore_mine(),
        "ore_heap" => ore_heap(),
        "ironworks" => ironworks(),
        "coffee_bush" => coffee_bush(),
        "shade_tree" => shade_tree(),
        "drying_floor" => drying_floor(),
        "roastery" => roastery(),
        "clockmaker" => clockmaker(),
        "cacao_tree" => cacao_tree(),
        "chocolatier" => chocolatier(),
        "mulberry_row" => mulberry_row(),
        "silk_shed" => silk_shed(),
        "couturier" => couturier(),
        "gold_mine" => gold_mine(),
        "goldsmith" => goldsmith(),
        "theatre" => theatre(),
        "bank" => bank(),
        "obelisk" => obelisk(),
        _ => return None,
    })
}

/// The house model of a tier (Masters or Patricians) in a climate.
pub fn house_model(tier: Tier, biome: Biome, v: usize) -> ModelKey {
    let set: [ModelKey; 3] = match (biome, tier == Tier::Patricians) {
        (Biome::Temperate, false) => ["guild_0", "guild_1", "guild_2"],
        (Biome::Temperate, true) => ["palazzo_0", "palazzo_1", "palazzo_2"],
        (Biome::Tropical, false) => ["veranda_0", "veranda_1", "veranda_2"],
        (Biome::Tropical, true) => ["plantation_0", "plantation_1", "plantation_2"],
        (Biome::Arid, false) => ["tower_house_0", "tower_house_1", "tower_house_2"],
        (Biome::Arid, true) => ["palace_0", "palace_1", "palace_2"],
        (Biome::Arctic, false) => ["stave_hall_0", "stave_hall_1", "stave_hall_2"],
        (Biome::Arctic, true) => ["copper_manor_0", "copper_manor_1", "copper_manor_2"],
    };
    set[v % 3]
}

/// Where a Masters' or Patricians' house smokes from (none in the heat).
pub fn house_smoke(tier: Tier, biome: Biome) -> Vec<(Vec3, &'static str)> {
    let top = tier == Tier::Patricians;
    match biome {
        Biome::Temperate if top => vec![
            (v3(-4.0, 17.6, -2.6), "isle.chimney"),
            (v3(4.0, 17.6, -2.6), "isle.chimney"),
        ],
        Biome::Temperate => vec![(v3(3.2, 17.4, -1.6), "isle.chimney")],
        Biome::Arctic if top => vec![(v3(-3.6, 16.4, -2.4), "isle.chimney")],
        Biome::Arctic => vec![(v3(2.8, 17.2, -1.0), "isle.chimney")],
        Biome::Tropical | Biome::Arid => vec![],
    }
}

/// A Masters' or Patricians' house and what stands on its lot.
pub fn house_look(tier: Tier, biome: Biome, seed: u64, out: &mut Vec<Piece>) {
    let v = crate::looks::variant(seed);
    let r = |k: u64| h01(seed, 900 + k);
    let t = 0.95 + 0.1 * h01(seed, 2);
    out.push(Piece::at(house_model(tier, biome, v), Vec3::ZERO).tint([
        t,
        t * (0.97 + 0.05 * h01(seed, 3)),
        t,
    ]));
    let leaf = |k: u64| {
        let g = 0.85 + 0.3 * r(k);
        [g * (0.9 + 0.15 * r(k + 1)), g, g * 0.88]
    };
    let top = tier == Tier::Patricians;
    match biome {
        Biome::Temperate => {
            out.push(Piece::at("lamp_post", v3(-5.2, 0.0, 5.4)));
            out.push(Piece::at("lamp_post", v3(5.2, 0.0, 5.4)));
            if top {
                out.push(Piece::at("tub_shrub", v3(-2.6, 0.0, 5.6)).tint(leaf(0)));
                out.push(Piece::at("tub_shrub", v3(2.6, 0.0, 5.6)).tint(leaf(2)));
            } else if r(4) > 0.4 {
                out.push(Piece::at("bench", v3(2.6, 0.0, 5.4)));
            }
        }
        Biome::Tropical => {
            for (k, (x, z)) in [(-5.3, 4.6), (5.3, 4.6), (-5.3, -5.0), (5.3, -5.0)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                if r(k) < 0.8 {
                    out.push(
                        Piece::at("banana", v3(x, 0.0, z))
                            .yaw(r(6 + k) * TAU)
                            .scale(1.0 + 0.5 * r(10 + k))
                            .tint(leaf(14 + k * 2)),
                    );
                }
            }
        }
        Biome::Arid => {
            for (k, x) in [-5.0f32, 5.0].into_iter().enumerate() {
                let k = k as u64;
                out.push(
                    Piece::at("jar", v3(x, 0.0, 5.4))
                        .scale(1.1 + 0.4 * r(20 + k))
                        .tint([1.0, 0.9 + 0.1 * r(24 + k), 0.85]),
                );
            }
            if top {
                out.push(Piece::at("date_palm", v3(-5.2, 0.0, -5.2)).scale(0.8));
            }
        }
        Biome::Arctic => {
            for (k, (x, z)) in [(-5.0, 4.6), (5.0, -4.6), (4.8, 4.8)]
                .into_iter()
                .enumerate()
            {
                let k = k as u64;
                if r(30 + k) < 0.65 {
                    out.push(
                        Piece::at("snow_drift", v3(x, 0.0, z))
                            .yaw(r(34 + k) * TAU)
                            .scale(0.7 + 0.6 * r(38 + k)),
                    );
                }
            }
            out.push(Piece::at("lamp_post", v3(-5.2, 0.0, 5.4)));
        }
    }
}

/// The pieces of the new works, the theatre and the bank (false: not one of them).
pub fn look(kind: Kind, seed: u64, out: &mut Vec<Piece>) -> bool {
    let r = |k: u64| h01(seed, 950 + k);
    match kind {
        Kind::OreMine => {
            out.push(Piece::at("ore_mine", Vec3::ZERO));
            out.push(Piece::at("ore_heap", v3(5.0, 0.0, 5.0)).yaw(r(0) * TAU));
            out.push(
                Piece::at("ore_heap", v3(-5.2, 0.0, 5.4))
                    .yaw(r(1) * TAU)
                    .scale(0.7),
            );
            out.push(Piece::at("hand_cart", v3(2.0, 0.0, 6.2)).yaw(1.2));
        }
        Kind::GoldMine => {
            out.push(Piece::at("gold_mine", Vec3::ZERO));
            out.push(
                Piece::at("ore_heap", v3(5.4, 0.0, 5.6))
                    .yaw(r(2) * TAU)
                    .tint([1.0, 0.9, 0.7]),
            );
            out.push(Piece::at("hand_cart", v3(-4.6, 0.0, 6.0)).yaw(-0.8));
        }
        Kind::Ironworks => {
            out.push(Piece::at("ironworks", Vec3::ZERO));
            out.push(Piece::at("ore_heap", v3(7.4, 0.0, 6.0)).yaw(r(3) * TAU));
            out.push(Piece::at("log_pile", v3(-7.2, 0.0, 6.2)).yaw(0.1));
        }
        Kind::CoffeeGrove | Kind::CacaoGrove => {
            let coffee = kind == Kind::CoffeeGrove;
            out.push(Piece::at("drying_floor", v3(-6.4, 0.0, 6.4)));
            out.push(Piece::at("sacks", v3(-3.6, 0.0, 9.6)).yaw(0.5));
            let plant = if coffee { "coffee_bush" } else { "cacao_tree" };
            for i in 0..5 {
                for j in 0..5 {
                    if i < 2 && j >= 3 {
                        continue;
                    }
                    let k = (i * 5 + j) as u64;
                    let x = -9.0 + i as f32 * 4.4 + (r(10 + k) - 0.5) * 0.8;
                    let z = 9.0 - j as f32 * 4.4 + (r(40 + k) - 0.5) * 0.8;
                    if coffee && (i + j) % 4 == 1 {
                        out.push(
                            Piece::at("shade_tree", v3(x, 0.0, z))
                                .yaw(r(70 + k) * TAU)
                                .scale(0.85 + 0.3 * r(90 + k)),
                        );
                    } else {
                        out.push(
                            Piece::at(plant, v3(x, 0.0, z))
                                .yaw(r(70 + k) * TAU)
                                .scale(0.8 + 0.35 * r(90 + k)),
                        );
                    }
                }
            }
        }
        Kind::SilkFarm => {
            out.push(Piece::at("silk_shed", v3(-4.0, 0.0, 4.4)));
            for (i, x) in [-9.2f32, -4.8, -0.4, 4.0, 8.4].into_iter().enumerate() {
                out.push(Piece::at("mulberry_row", v3(x, 0.0, -5.0)).tint([
                    0.92 + 0.1 * r(i as u64),
                    1.0,
                    0.9,
                ]));
            }
            for (i, x) in [5.2f32, 9.0].into_iter().enumerate() {
                out.push(Piece::at("mulberry_row", v3(x, 0.0, 5.0)).tint([
                    0.95 + 0.08 * r(8 + i as u64),
                    1.0,
                    0.9,
                ]));
            }
        }
        Kind::Roastery => {
            out.push(Piece::at("roastery", Vec3::ZERO));
            out.push(Piece::at("sacks", v3(4.6, 0.0, 6.0)).yaw(0.4));
            out.push(Piece::at("sacks", v3(-4.4, 0.0, 6.4)).yaw(-0.6).scale(0.8));
        }
        Kind::Clockmaker => {
            out.push(Piece::at("clockmaker", Vec3::ZERO));
            out.push(Piece::at("lamp_post", v3(-5.0, 0.0, 5.0)));
            out.push(Piece::at("crate", v3(4.6, 0.0, 4.4)).yaw(0.3));
        }
        Kind::Chocolatier => {
            out.push(Piece::at("chocolatier", Vec3::ZERO));
            out.push(Piece::at("bench", v3(-5.6, 0.0, 4.6)));
            out.push(Piece::at("tub_shrub", v3(5.8, 0.0, 4.6)).tint([1.0, 0.9, 0.95]));
        }
        Kind::Couturier => {
            out.push(Piece::at("couturier", Vec3::ZERO));
            out.push(Piece::at("lamp_post", v3(-6.6, 0.0, 6.6)));
            out.push(Piece::at("lamp_post", v3(6.6, 0.0, 6.6)));
            out.push(Piece::at("tub_shrub", v3(-3.0, 0.0, 7.0)).tint([0.95, 1.0, 0.9]));
            out.push(Piece::at("tub_shrub", v3(3.0, 0.0, 7.0)).tint([0.95, 1.0, 0.9]));
        }
        Kind::Goldsmith => {
            out.push(Piece::at("goldsmith", Vec3::ZERO));
            out.push(Piece::at("lamp_post", v3(5.0, 0.0, 5.0)));
        }
        Kind::Theatre => {
            out.push(Piece::at("theatre", Vec3::ZERO));
            for x in [-11.0f32, 11.0] {
                out.push(Piece::at("lamp_post", v3(x, 0.0, 9.0)));
                out.push(Piece::at("obelisk", v3(x * 0.7, 0.0, 9.4)));
            }
            out.push(Piece::at("bench", v3(-7.0, 0.0, 9.2)));
            out.push(Piece::at("bench", v3(7.0, 0.0, 9.2)));
        }
        Kind::Bank => {
            out.push(Piece::at("bank", Vec3::ZERO));
            for x in [-8.6f32, 8.6] {
                out.push(Piece::at("lamp_post", v3(x, 0.0, 8.6)));
            }
            out.push(Piece::at("tub_shrub", v3(-5.0, 0.0, 9.0)).tint([0.9, 1.0, 0.85]));
            out.push(Piece::at("tub_shrub", v3(5.0, 0.0, 9.0)).tint([0.9, 1.0, 0.85]));
        }
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------------------ helpers

const STONE: u32 = 0xddd6c6;
const GOLD: u32 = 0xd8b04a;
const COPPER: u32 = 0x5f9e86;

/// A row of `n` round columns from `x0` to `x1` at depth `z`, standing on `y0`, `h` tall,
/// with a square base and capital.
#[allow(clippy::too_many_arguments)]
fn columns(p: &mut Parts, x0: f32, x1: f32, z: f32, n: usize, y0: f32, h: f32, r: f32, col: Vec4) {
    for i in 0..n {
        let x = if n == 1 {
            (x0 + x1) * 0.5
        } else {
            x0 + (x1 - x0) * i as f32 / (n - 1) as f32
        };
        let b = r * 1.45;
        p.cuboid(
            Mat::Stone,
            v3(x - b, y0, z - b),
            v3(x + b, y0 + 0.35, z + b),
            col,
            false,
        );
        p.cylinder(
            Mat::Stone,
            v3(x, y0 + 0.35, z),
            r,
            r * 0.86,
            h - 0.7,
            12,
            col,
            false,
        );
        p.cuboid(
            Mat::Stone,
            v3(x - b, y0 + h - 0.35, z - b),
            v3(x + b, y0 + h, z + b),
            col,
            false,
        );
    }
}

/// A triangular pediment over `x0..x1` at the front `z`, its foot at `y`, `h` high and
/// `d` deep, with a moulded cornice under it.
#[allow(clippy::too_many_arguments)]
fn pediment(p: &mut Parts, x0: f32, x1: f32, z: f32, y: f32, h: f32, d: f32, col: Vec4) {
    let m = (x0 + x1) * 0.5;
    p.cuboid(
        Mat::Stone,
        v3(x0 - 0.25, y - 0.45, z - d),
        v3(x1 + 0.25, y, z + 0.25),
        col,
        false,
    );
    let (a, b, c) = (v3(x0, y, z), v3(x1, y, z), v3(m, y + h, z));
    p.tri(Mat::Stone, a, b, c, shade(col, 0.95));
    let (ab, bb, cb) = (v3(x0, y, z - d), v3(x1, y, z - d), v3(m, y + h, z - d));
    p.tri(Mat::Stone, bb, ab, cb, col);
    // The raking cornices, a roof over the triangle.
    p.quad(
        Mat::Slate,
        v3(x0 - 0.3, y - 0.05, z + 0.3),
        v3(m, y + h + 0.2, z + 0.3),
        v3(m, y + h + 0.2, z - d),
        v3(x0 - 0.3, y - 0.05, z - d),
        shade(col, 0.7),
    );
    p.quad(
        Mat::Slate,
        v3(m, y + h + 0.2, z + 0.3),
        v3(x1 + 0.3, y - 0.05, z + 0.3),
        v3(x1 + 0.3, y - 0.05, z - d),
        v3(m, y + h + 0.2, z - d),
        shade(col, 0.7),
    );
}

/// A balustrade along a straight line from `a` to `b`: a plinth, turned balusters and a
/// rail, `h` high.
fn balustrade(p: &mut Parts, a: Vec3, b: Vec3, h: f32, col: Vec4) {
    let len = (b - a).length();
    let n = (len / 0.45).ceil().max(2.0) as usize;
    let dir = (b - a) / len.max(1e-4);
    let side = Vec3::Y.cross(dir) * 0.16;
    p.cuboid(
        Mat::Stone,
        a.min(b) - side.abs() - Vec3::Y * 0.0,
        a.max(b) + side.abs() + Vec3::Y * 0.16,
        col,
        false,
    );
    for i in 0..=n {
        let at = a + (b - a) * (i as f32 / n as f32);
        p.cylinder(
            Mat::Stone,
            at + Vec3::Y * 0.16,
            0.07,
            0.05,
            h - 0.32,
            6,
            col,
            false,
        );
    }
    p.cuboid(
        Mat::Stone,
        a.min(b) - side.abs() * 1.2 + Vec3::Y * (h - 0.16),
        a.max(b) + side.abs() * 1.2 + Vec3::Y * h,
        col,
        false,
    );
}

/// A balustrade round a flat roof `l` × `w` centred on `c` (the roof's height).
fn balustrade_round(p: &mut Parts, c: Vec3, l: f32, w: f32, h: f32, col: Vec4) {
    let (hl, hw) = (l * 0.5, w * 0.5);
    let k = |x: f32, z: f32| c + v3(x, 0.0, z);
    balustrade(p, k(-hl, hw), k(hl, hw), h, col);
    balustrade(p, k(-hl, -hw), k(hl, -hw), h, col);
    balustrade(p, k(-hl, -hw), k(-hl, hw), h, col);
    balustrade(p, k(hl, -hw), k(hl, hw), h, col);
}

/// A moulded band all round a box `l × w` at `c`, from `y` up `h`, standing out `d`.
#[allow(clippy::too_many_arguments)]
fn band(p: &mut Parts, c: Vec3, l: f32, w: f32, y: f32, h: f32, d: f32, col: Vec4) {
    p.cuboid(
        Mat::Plaster,
        v3(c.x - l * 0.5 - d, y, c.z - w * 0.5 - d),
        v3(c.x + l * 0.5 + d, y + h, c.z + w * 0.5 + d),
        col,
        true,
    );
}

/// Quoins up the four corners of a box `l × w` at `c` from `y0` to `y1`.
#[allow(clippy::too_many_arguments)]
fn quoins(p: &mut Parts, c: Vec3, l: f32, w: f32, y0: f32, y1: f32, col: Vec4) {
    let n = ((y1 - y0) / 0.75) as usize;
    for sx in [-1.0f32, 1.0] {
        for sz in [-1.0f32, 1.0] {
            for k in 0..n {
                let y = y0 + 0.1 + k as f32 * 0.75;
                let d = if k % 2 == 0 { 0.55 } else { 0.34 };
                let (x, z) = (c.x + sx * l * 0.5, c.z + sz * w * 0.5);
                p.cuboid(
                    Mat::Stone,
                    v3(x - d * 0.5 - 0.04, y, z - d * 0.5 - 0.04),
                    v3(x + d * 0.5 + 0.04, y + 0.45, z + d * 0.5 + 0.04),
                    col,
                    true,
                );
            }
        }
    }
}

/// A plain wall box on a stone plinth, `l × w` centred on `c` at floor height, from the
/// plinth's top to `top`.
#[allow(clippy::too_many_arguments)]
fn body(p: &mut Parts, m: Mat, c: Vec3, l: f32, w: f32, top: f32, col: Vec4, plinth: Vec4) {
    p.cuboid(
        Mat::Stone,
        v3(c.x - l * 0.5 - 0.2, -3.0, c.z - w * 0.5 - 0.2),
        v3(c.x + l * 0.5 + 0.2, 0.6, c.z + w * 0.5 + 0.2),
        plinth,
        false,
    );
    p.cuboid(
        m,
        v3(c.x - l * 0.5, 0.6, c.z - w * 0.5),
        v3(c.x + l * 0.5, top, c.z + w * 0.5),
        col,
        false,
    );
}

/// A small domed lantern (a cupola) standing on a roof at `at`.
fn cupola(p: &mut Parts, at: Vec3, r: f32, dome_col: Vec4, wall: Vec4) {
    p.cylinder(Mat::Plaster, at, r, r, r * 1.3, 12, wall, false);
    for k in 0..6 {
        let a = k as f32 / 6.0 * TAU;
        p.windows(
            at + v3(a.sin() * (r + 0.02), r * 0.65, a.cos() * (r + 0.02)),
            v3(a.sin(), 0.0, a.cos()),
            1,
            r * 0.5,
            r * 0.8,
            0.0,
            WHITE,
        );
    }
    p.cuboid(
        Mat::Plaster,
        at + v3(-r - 0.12, r * 1.3, -r - 0.12),
        at + v3(r + 0.12, r * 1.3 + 0.18, r + 0.12),
        WHITE,
        false,
    );
    dome(
        p,
        Mat::Copper,
        at + Vec3::Y * (r * 1.3 + 0.18),
        r,
        r * 0.95,
        dome_col,
    );
    p.rod(
        Mat::Iron,
        at + Vec3::Y * (r * 2.2 + 0.1),
        at + Vec3::Y * (r * 2.2 + 1.4),
        0.05,
        6,
        hexc(GOLD),
    );
}

/// A flag on a pole from `base`, `h` tall.
fn flag(p: &mut Parts, base: Vec3, h: f32, col: Vec4) {
    p.rod(Mat::Iron, base, base + Vec3::Y * h, 0.06, 6, WHITE);
    let t = base + Vec3::Y * (h - 0.2);
    crate::civic::sheet(
        p,
        Mat::Canvas,
        t,
        t + v3(1.6, -0.15, 0.1),
        t + v3(1.6, -1.15, 0.1),
        t + v3(0.0, -1.0, 0.0),
        col,
    );
}

// --------------------------------------------------------------------- Masters' houses

/// The home isles: a guild master's house of four storeys under a stepped gable to the
/// street, an arcaded shop floor, a hoist beam in the gable. 0: red brick; 1: dark brick
/// with white bands; 2: ochre render.
fn guild(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (9.6, 9.0);
    let c = v3(0.0, 0.0, 0.0);
    let (zf, eave) = (w * 0.5, 13.2);
    let (m, col) = match v {
        0 => (Mat::Brick, Vec4::ONE),
        1 => (Mat::Brick, hexc(0x9a8a84)),
        _ => (Mat::Plaster, hexc(0xe2c27e)),
    };
    let stone = hexc(STONE);
    body(&mut p, m, c, l, w, eave, col, shade(stone, 0.85));
    // A stone shop floor of three arches.
    p.cuboid(
        Mat::Stone,
        v3(-l * 0.5 - 0.05, 0.6, zf - 0.6),
        v3(l * 0.5 + 0.05, 4.2, zf + 0.12),
        stone,
        false,
    );
    for k in 0..3 {
        let x = -3.0 + k as f32 * 3.0;
        p.windows(v3(x, 2.35, zf + 0.14), Vec3::Z, 1, 2.0, 2.7, 0.0, WHITE);
        p.cylinder(
            Mat::Stone,
            v3(x, 3.8, zf + 0.1),
            1.05,
            0.2,
            0.4,
            10,
            shade(stone, 0.92),
            true,
        );
    }
    p.door(v3(-3.0, 0.6, zf + 0.15), Vec3::Z, 1.2, 2.4, hexc(0x2c3f5a));
    for (y, h) in [(4.2, 0.28), (7.2, 0.2), (10.2, 0.2), (eave - 0.25, 0.3)] {
        band(
            &mut p,
            c,
            l,
            w,
            y,
            h,
            0.12,
            if v == 1 { WHITE } else { stone },
        );
    }
    window_band(&mut p, c, l, w, 5.75, 4, 3, (0.95, 1.8), WHITE);
    window_band(&mut p, c, l, w, 8.7, 4, 3, (0.95, 1.7), WHITE);
    window_band(&mut p, c, l, w, 11.6, 4, 3, (0.9, 1.5), WHITE);
    // The roof, its ridge front to back, under a stepped gable to the street.
    let rh = 5.6;
    p.gable_roof(
        Mat::RoofTile,
        m,
        v3(0.0, eave, 0.0),
        l,
        w,
        rh,
        0.1,
        true,
        hexc(0x8f4f3c),
        col,
    );
    let steps_n = 5;
    for k in 0..steps_n {
        let f = k as f32 / steps_n as f32;
        let hw = l * 0.5 * (1.0 - f) + 0.15;
        let y0 = eave + rh * f;
        p.cuboid(
            m,
            v3(-hw, y0, zf - 0.1),
            v3(hw, y0 + rh / steps_n as f32 + 0.25, zf + 0.35),
            col,
            false,
        );
        p.cuboid(
            Mat::Stone,
            v3(-hw - 0.08, y0 + rh / steps_n as f32 + 0.25, zf - 0.15),
            v3(-hw + 0.7, y0 + rh / steps_n as f32 + 0.4, zf + 0.4),
            stone,
            false,
        );
        p.cuboid(
            Mat::Stone,
            v3(hw - 0.7, y0 + rh / steps_n as f32 + 0.25, zf - 0.15),
            v3(hw + 0.08, y0 + rh / steps_n as f32 + 0.4, zf + 0.4),
            stone,
            false,
        );
    }
    p.windows(
        v3(0.0, eave + 1.6, zf + 0.37),
        Vec3::Z,
        2,
        0.85,
        1.4,
        0.7,
        WHITE,
    );
    p.windows(
        v3(0.0, eave + 3.6, zf + 0.37),
        Vec3::Z,
        1,
        0.8,
        1.1,
        0.0,
        WHITE,
    );
    // The hoist beam over the top window.
    p.beam(
        Mat::Timber,
        v3(0.0, eave + 4.9, zf + 0.3),
        v3(0.0, eave + 4.9, zf + 1.5),
        0.22,
        hexc(0x3a2a1e),
    );
    chimney(
        &mut p,
        Mat::Brick,
        v3(3.2, eave + 1.0, -1.6),
        eave + 4.2,
        Vec4::ONE,
    );
    hanging_sign(
        &mut p,
        v3(l * 0.5 - 0.6, 4.9, zf + 0.12),
        0.9,
        0.7,
        hexc(0x23304a),
        hexc(GOLD),
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// Sarrowmere: a ship's master's house. Two tall floors on a stone basement, wrapped in
/// verandas on both floors under a wide tiled hip roof, louvred shutters. 0: white;
/// 1: pale green; 2: yellow.
fn veranda(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (8.2, 7.4);
    let col = match v {
        0 => hexc(0xf2efe6),
        1 => hexc(0xc6dcc0),
        _ => hexc(0xeed58e),
    };
    let wood = hexc(0xf6f4ee);
    let stone = hexc(0xb8ad98);
    let c = v3(0.0, 0.0, 0.0);
    body(&mut p, Mat::Plaster, c, l, w, 10.6, col, stone);
    // Basement raised 1.6 m: steps up to the lower veranda.
    p.cuboid(
        Mat::Stone,
        v3(-5.6, -3.0, -5.2),
        v3(5.6, 1.6, 5.2),
        stone,
        false,
    );
    steps(&mut p, 0.0, 5.2, 2.6, 4, 1.6, stone);
    window_band(&mut p, c, l, w, 3.9, 3, 3, (1.1, 2.3), hexc(0x3f6e4a));
    window_band(&mut p, c, l, w, 8.2, 3, 3, (1.1, 2.1), hexc(0x3f6e4a));
    p.door(
        v3(0.0, 1.6, w * 0.5 + 0.02),
        Vec3::Z,
        1.4,
        2.6,
        hexc(0x3f6e4a),
    );
    // Two verandas all round on slim posts, with railings.
    for (y, top) in [(1.6f32, 6.1f32), (6.3, 10.6)] {
        up_quad(
            &mut p,
            Mat::Planks,
            (-5.5, 5.5),
            (-5.1, 5.1),
            y + 0.05,
            hexc(0xb08a62),
        );
        for (x, z) in [
            (-5.3, 4.9),
            (-1.8, 4.9),
            (1.8, 4.9),
            (5.3, 4.9),
            (-5.3, -4.9),
            (5.3, -4.9),
            (-5.3, 0.0),
            (5.3, 0.0),
            (-1.8, -4.9),
            (1.8, -4.9),
        ] {
            p.beam(Mat::Timber, v3(x, y, z), v3(x, top, z), 0.2, wood);
        }
        if y > 2.0 {
            for (a, b) in [
                (v3(-5.3, y, 4.9), v3(5.3, y, 4.9)),
                (v3(-5.3, y, -4.9), v3(5.3, y, -4.9)),
                (v3(-5.3, y, -4.9), v3(-5.3, y, 4.9)),
                (v3(5.3, y, -4.9), v3(5.3, y, 4.9)),
            ] {
                crate::regional::railing(&mut p, a, b, 1.0, wood);
            }
        }
        p.cuboid(
            Mat::Planks,
            v3(-5.5, top - 0.3, -5.1),
            v3(5.5, top, 5.1),
            wood,
            true,
        );
    }
    p.hip_roof(
        Mat::RoofTile,
        v3(0.0, 10.6, 0.0),
        11.2,
        10.4,
        3.6,
        0.6,
        hexc(0xa8583e),
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The Amber Reach: a master's tower house. Four storeys of thick render, small deep
/// windows with carved screens, a wind-catcher tower and crenellations. 0: sand; 1: white;
/// 2: rose.
fn tower_house(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (8.8, 8.4);
    let col = match v {
        0 => hexc(0xe0c69a),
        1 => hexc(0xf1ece2),
        _ => hexc(0xe7bba6),
    };
    let c = v3(0.0, 0.0, 0.0);
    let top = 14.2;
    body(&mut p, Mat::Clay, c, l, w, top, col, shade(col, 0.8));
    for y in [4.0f32, 7.4, 10.8] {
        band(&mut p, c, l, w, y, 0.18, 0.06, shade(col, 0.85));
    }
    window_band(&mut p, c, l, w, 2.2, 2, 2, (0.8, 1.2), hexc(0x6b4a30));
    window_band(&mut p, c, l, w, 5.6, 3, 2, (0.7, 1.4), hexc(0x6b4a30));
    window_band(&mut p, c, l, w, 9.0, 3, 2, (0.7, 1.4), hexc(0x6b4a30));
    window_band(&mut p, c, l, w, 12.4, 3, 2, (0.7, 1.3), hexc(0x6b4a30));
    // A carved wooden bay over the door.
    p.cuboid(
        Mat::Timber,
        v3(-1.6, 4.6, w * 0.5),
        v3(1.6, 7.0, w * 0.5 + 0.8),
        hexc(0x8a5a36),
        true,
    );
    p.windows(
        v3(0.0, 5.8, w * 0.5 + 0.82),
        Vec3::Z,
        3,
        0.7,
        1.4,
        0.2,
        hexc(0x5a3a22),
    );
    p.door(
        v3(0.0, 0.6, w * 0.5 + 0.02),
        Vec3::Z,
        1.4,
        2.6,
        hexc(0x5c3a22),
    );
    // Crenellations.
    for (a, b) in [
        (v3(-l * 0.5, top, w * 0.5 - 0.3), v3(l * 0.5, top, w * 0.5)),
        (
            v3(-l * 0.5, top, -w * 0.5),
            v3(l * 0.5, top, -w * 0.5 + 0.3),
        ),
    ] {
        let n = 8;
        for k in 0..n {
            if k % 2 == 0 {
                let x0 = a.x + (b.x - a.x) * k as f32 / n as f32;
                let x1 = a.x + (b.x - a.x) * (k + 1) as f32 / n as f32;
                p.cuboid(
                    Mat::Clay,
                    v3(x0, top, a.z),
                    v3(x1, top + 0.9, b.z),
                    col,
                    false,
                );
            }
        }
    }
    parapet(&mut p, v3(0.0, top, 0.0), l, w, 0.45, col);
    // The wind-catcher: a tall square tower with slots at the top.
    let (tx, tz) = (-2.6, -2.2);
    p.cuboid(
        Mat::Clay,
        v3(tx - 1.0, top, tz - 1.0),
        v3(tx + 1.0, top + 4.6, tz + 1.0),
        col,
        false,
    );
    for (nx, nz) in [(0.0f32, 1.0f32), (0.0, -1.0), (1.0, 0.0), (-1.0, 0.0)] {
        p.windows(
            v3(tx + nx * 1.02, top + 3.6, tz + nz * 1.02),
            v3(nx, 0.0, nz),
            2,
            0.3,
            1.3,
            0.25,
            hexc(0x4a3220),
        );
    }
    p.cuboid(
        Mat::Clay,
        v3(tx - 1.2, top + 4.6, tz - 1.2),
        v3(tx + 1.2, top + 4.9, tz + 1.2),
        shade(col, 0.9),
        false,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// Frostholm: a master's hall. A stone ground floor, three boarded floors in a strong
/// colour, a steep slate roof with dormers, a turret on one corner. 0: red; 1: blue;
/// 2: ochre.
fn stave_hall(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (9.2, 8.2);
    let col = match v {
        0 => hexc(0x9c3a2c),
        1 => hexc(0x3e5a7a),
        _ => hexc(0xc98f3a),
    };
    let stone = hexc(0x9a968c);
    let c = v3(0.0, 0.0, 0.0);
    let eave = 12.6;
    body(&mut p, Mat::Stone, c, l, w, 3.6, stone, shade(stone, 0.85));
    p.cuboid(
        Mat::Planks,
        v3(-l * 0.5, 3.6, -w * 0.5),
        v3(l * 0.5, eave, w * 0.5),
        col,
        false,
    );
    for y in [3.6f32, 6.6, 9.6] {
        band(&mut p, c, l, w, y, 0.22, 0.08, WHITE);
    }
    window_band(&mut p, c, l, w, 2.0, 3, 2, (0.9, 1.3), WHITE);
    window_band(&mut p, c, l, w, 5.1, 4, 3, (0.85, 1.6), WHITE);
    window_band(&mut p, c, l, w, 8.1, 4, 3, (0.85, 1.6), WHITE);
    window_band(&mut p, c, l, w, 11.0, 4, 3, (0.8, 1.4), WHITE);
    p.door(
        v3(0.0, 0.6, w * 0.5 + 0.02),
        Vec3::Z,
        1.4,
        2.6,
        hexc(0x2e2a26),
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3(0.0, eave, 0.0),
        l,
        w,
        6.2,
        0.35,
        false,
        hexc(0x4a4e56),
        col,
    );
    roof_snow(&mut p, v3(0.0, eave, 0.0), l, w, 6.2, 0.35, false, 0.7);
    for x in [-2.6f32, 0.0, 2.6] {
        dormer(
            &mut p,
            v3(x, eave + 0.9, w * 0.5 - 0.9),
            1.2,
            1.3,
            1.6,
            Mat::Slate,
        );
    }
    // A round turret on the front corner under a cone.
    let t = v3(l * 0.5 - 0.3, 3.6, w * 0.5 - 0.3);
    p.cylinder(Mat::Planks, t, 1.5, 1.5, eave + 1.6 - 3.6, 12, col, false);
    p.windows(t + v3(0.0, 4.6, 1.52), Vec3::Z, 1, 0.7, 1.3, 0.0, WHITE);
    p.windows(t + v3(0.0, 7.6, 1.52), Vec3::Z, 1, 0.7, 1.3, 0.0, WHITE);
    p.cylinder(
        Mat::Slate,
        t + Vec3::Y * (eave + 1.6 - 3.6),
        1.8,
        0.05,
        4.4,
        12,
        hexc(0x4a4e56),
        false,
    );
    chimney(
        &mut p,
        Mat::Stone,
        v3(2.8, eave + 1.0, -1.0),
        eave + 5.0,
        stone,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

// ------------------------------------------------------------------- Patricians' houses

/// The home isles: a shipowner's palazzo. A rusticated ground floor, a piano nobile of tall
/// windows behind a two-storey portico of columns under a pediment, an attic, a balustrade
/// and a belvedere dome. 0: cream stone; 1: pale grey; 2: rose.
fn palazzo(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (10.6, 9.6);
    let col = match v {
        0 => hexc(0xeadfc6),
        1 => hexc(0xd6d8d6),
        _ => hexc(0xe6c4b4),
    };
    let stone = hexc(STONE);
    let c = v3(0.0, 0.0, -0.4);
    let (zf, top) = (c.z + w * 0.5, 15.0);
    body(
        &mut p,
        Mat::Stone,
        c,
        l,
        w,
        4.4,
        shade(stone, 0.95),
        shade(stone, 0.8),
    );
    // Rustication: deep joints across the ground floor.
    for k in 1..6 {
        let y = 0.6 + k as f32 * 0.66;
        band(&mut p, c, l, w, y, 0.06, 0.02, shade(stone, 0.6));
    }
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, 4.4, c.z - w * 0.5),
        v3(l * 0.5, top, zf),
        col,
        false,
    );
    quoins(&mut p, c, l, w, 4.4, top, stone);
    for (y, h, d) in [
        (4.4f32, 0.35f32, 0.2f32),
        (11.4, 0.22, 0.12),
        (top - 0.4, 0.5, 0.35),
    ] {
        band(&mut p, c, l, w, y, h, d, WHITE);
    }
    window_band(&mut p, c, l, w, 2.5, 4, 3, (1.0, 1.6), WHITE);
    window_band(&mut p, c, l, w, 7.6, 5, 3, (1.0, 2.6), WHITE);
    window_band(&mut p, c, l, w, 12.8, 5, 3, (0.9, 1.2), WHITE);
    // The portico: four columns through two floors, a pediment, a balcony behind.
    let pz = zf + 1.1;
    p.cuboid(
        Mat::Stone,
        v3(-3.6, -1.0, zf),
        v3(3.6, 0.9, pz + 0.4),
        stone,
        false,
    );
    steps(&mut p, 0.0, pz + 0.4, 4.4, 3, 0.9, stone);
    columns(&mut p, -3.0, 3.0, pz, 4, 0.9, 10.2, 0.34, WHITE);
    p.cuboid(
        Mat::Stone,
        v3(-3.5, 11.1, zf),
        v3(3.5, 11.6, pz + 0.45),
        WHITE,
        true,
    );
    pediment(&mut p, -3.6, 3.6, pz + 0.45, 11.6, 2.2, 1.55, WHITE);
    balustrade(&mut p, v3(-3.0, 4.6, pz), v3(3.0, 4.6, pz), 1.0, WHITE);
    p.door(v3(0.0, 0.9, zf + 0.02), Vec3::Z, 1.6, 3.0, hexc(0x1d2d40));
    // The roof: a low hip behind a balustrade, a belvedere dome over the middle.
    p.hip_roof(
        Mat::Slate,
        v3(0.0, top, c.z),
        l - 0.8,
        w - 0.8,
        2.2,
        0.0,
        hexc(0x5a6068),
    );
    balustrade_round(
        &mut p,
        v3(0.0, top + 0.1, c.z),
        l + 0.5,
        w + 0.5,
        1.0,
        WHITE,
    );
    cupola(
        &mut p,
        v3(0.0, top + 1.6, c.z - 0.6),
        1.3,
        hexc(COPPER),
        WHITE,
    );
    for x in [-4.0f32, 4.0] {
        chimney(&mut p, Mat::Stone, v3(x, top, c.z - 2.2), top + 2.6, stone);
    }
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// Sarrowmere: a planter-patrician's hall. Three white floors behind a portico of giant
/// columns the full height, galleries behind them, a hip roof and a lantern on top. 0: white
/// with green shutters; 1: cream with blue; 2: pale pink with white.
fn plantation(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (10.0, 8.0);
    let (col, shut) = match v {
        0 => (hexc(0xf6f4ee), hexc(0x356a48)),
        1 => (hexc(0xf0e6cc), hexc(0x2e5a86)),
        _ => (hexc(0xf0d6d0), WHITE),
    };
    let stone = hexc(0xb8ad98);
    let c = v3(0.0, 0.0, -0.8);
    let (zf, top) = (c.z + w * 0.5, 13.6);
    body(&mut p, Mat::Plaster, c, l, w, top, col, stone);
    for y in [4.8f32, 9.2] {
        band(&mut p, c, l, w, y, 0.25, 0.1, WHITE);
    }
    window_band(&mut p, c, l, w, 2.8, 5, 3, (1.0, 2.3), shut);
    window_band(&mut p, c, l, w, 7.1, 5, 3, (1.0, 2.3), shut);
    window_band(&mut p, c, l, w, 11.4, 5, 3, (1.0, 2.0), shut);
    p.door(v3(0.0, 0.6, zf + 0.02), Vec3::Z, 1.8, 3.0, shut);
    // Galleries on two floors behind giant columns.
    let pz = zf + 2.0;
    for y in [4.8f32, 9.2] {
        up_quad(
            &mut p,
            Mat::Planks,
            (-5.0, 5.0),
            (zf, pz + 0.3),
            y,
            hexc(0xc8b08a),
        );
        crate::regional::railing(&mut p, v3(-4.6, y, pz), v3(4.6, y, pz), 1.0, WHITE);
    }
    up_quad(&mut p, Mat::Stone, (-5.2, 5.2), (zf, pz + 0.6), 0.6, stone);
    steps(&mut p, 0.0, pz + 0.6, 4.0, 2, 0.6, stone);
    columns(&mut p, -4.6, 4.6, pz, 6, 0.6, top - 0.6, 0.36, WHITE);
    p.cuboid(
        Mat::Plaster,
        v3(-5.2, top - 0.1, zf),
        v3(5.2, top + 0.5, pz + 0.5),
        WHITE,
        true,
    );
    pediment(&mut p, -5.2, 5.2, pz + 0.5, top + 0.5, 2.4, 2.5, WHITE);
    p.hip_roof(
        Mat::RoofTile,
        v3(0.0, top, c.z),
        l,
        w,
        3.6,
        0.8,
        hexc(0x7a4a3a),
    );
    cupola(&mut p, v3(0.0, top + 3.0, c.z), 1.1, hexc(0x7a4a3a), WHITE);
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The Amber Reach: a patrician's palace. Two broad floors over an arcade, a great dome on
/// a drum in the middle, a slim tower with a small dome on one corner. 0: white with a gilt
/// dome; 1: sand with a blue tiled dome; 2: rose with a green dome.
fn palace(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (10.8, 9.6);
    let (col, dome_col) = match v {
        0 => (hexc(0xf3eee4), hexc(GOLD)),
        1 => (hexc(0xe2caa0), hexc(0x2f6aa8)),
        _ => (hexc(0xe8c0ac), hexc(COPPER)),
    };
    let c = v3(0.0, 0.0, -0.3);
    let (zf, top) = (c.z + w * 0.5, 10.4);
    body(&mut p, Mat::Clay, c, l, w, top, col, shade(col, 0.8));
    // The arcade: seven arches across the front.
    p.cuboid(
        Mat::Clay,
        v3(-l * 0.5 - 0.1, 0.6, zf),
        v3(l * 0.5 + 0.1, 4.6, zf + 1.8),
        col,
        false,
    );
    for k in 0..6 {
        let x = -l * 0.5 + 0.9 + k as f32 * (l - 1.8) / 5.0;
        p.windows(
            v3(x, 2.4, zf + 1.82),
            Vec3::Z,
            1,
            1.1,
            3.0,
            0.0,
            hexc(0x3a2a1a),
        );
        p.cylinder(
            Mat::Clay,
            v3(x, 3.85, zf + 1.81),
            0.6,
            0.12,
            0.3,
            8,
            shade(col, 0.85),
            true,
        );
    }
    band(&mut p, c, l, w, 4.6, 0.25, 0.08, shade(col, 0.85));
    window_band(&mut p, c, l, w, 7.2, 5, 4, (0.9, 2.2), hexc(0x4a3420));
    parapet(&mut p, v3(0.0, top, c.z), l, w, 0.7, col);
    // The great dome on its drum.
    let d = v3(0.0, top, c.z - 0.4);
    p.cylinder(Mat::Clay, d, 3.4, 3.4, 2.2, 16, col, false);
    for k in 0..8 {
        let a = k as f32 / 8.0 * TAU;
        p.windows(
            d + v3(a.sin() * 3.42, 1.1, a.cos() * 3.42),
            v3(a.sin(), 0.0, a.cos()),
            1,
            0.5,
            1.2,
            0.0,
            hexc(0x4a3420),
        );
    }
    dome(&mut p, Mat::Copper, d + Vec3::Y * 2.2, 3.6, 4.0, dome_col);
    p.rod(
        Mat::Iron,
        d + Vec3::Y * 6.1,
        d + Vec3::Y * 7.6,
        0.07,
        6,
        hexc(GOLD),
    );
    // A slim corner tower.
    let t = v3(-l * 0.5 + 0.9, top, c.z - w * 0.5 + 0.9);
    p.cylinder(Mat::Clay, t, 0.9, 0.8, 6.4, 12, col, false);
    p.cuboid(
        Mat::Clay,
        t + v3(-1.2, 6.4, -1.2),
        t + v3(1.2, 6.7, 1.2),
        shade(col, 0.9),
        false,
    );
    p.cylinder(Mat::Clay, t + Vec3::Y * 6.7, 0.9, 0.9, 1.4, 12, col, false);
    dome(&mut p, Mat::Copper, t + Vec3::Y * 8.1, 1.0, 1.3, dome_col);
    p.door(v3(0.0, 0.6, zf + 0.02), Vec3::Z, 1.6, 2.8, hexc(0x5c3a22));
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// Frostholm: a patrician's manor. Four floors of dressed stone, a slate mansard with
/// dormers, a copper-spired clock tower over the door. 0: grey granite; 1: buff; 2: dark.
fn copper_manor(v: usize) -> Parts {
    let mut p = Parts::default();
    let (l, w) = (10.0, 9.0);
    let col = match v {
        0 => hexc(0xb8b6ae),
        1 => hexc(0xd2c2a2),
        _ => hexc(0x8e8a84),
    };
    let c = v3(0.0, 0.0, -0.2);
    let (zf, eave) = (c.z + w * 0.5, 12.8);
    body(&mut p, Mat::Stone, c, l, w, eave, col, shade(col, 0.8));
    quoins(&mut p, c, l, w, 0.6, eave, WHITE);
    for (y, h, d) in [(3.8f32, 0.3f32, 0.12f32), (eave - 0.4, 0.45, 0.3)] {
        band(&mut p, c, l, w, y, h, d, WHITE);
    }
    window_band(&mut p, c, l, w, 2.2, 4, 3, (0.95, 1.5), WHITE);
    window_band(&mut p, c, l, w, 5.4, 5, 3, (0.95, 2.0), WHITE);
    window_band(&mut p, c, l, w, 8.4, 5, 3, (0.95, 1.8), WHITE);
    window_band(&mut p, c, l, w, 11.0, 5, 3, (0.9, 1.4), WHITE);
    mansard(&mut p, v3(0.0, eave, c.z), l, w, 3.0, 1.4, hexc(0x4a4e56));
    roof_snow(
        &mut p,
        v3(0.0, eave, c.z),
        l - 1.0,
        w - 1.0,
        3.0,
        0.2,
        false,
        0.5,
    );
    for x in [-3.4f32, 3.4] {
        dormer(
            &mut p,
            v3(x, eave + 0.5, zf - 0.6),
            1.2,
            1.3,
            1.4,
            Mat::Slate,
        );
    }
    // The clock tower over the door, through the roof, under a copper spire.
    let t = v3(0.0, 0.6, zf - 1.0);
    p.cuboid(
        Mat::Stone,
        t + v3(-1.6, 0.0, -0.4),
        t + v3(1.6, eave + 4.6 - 0.6, 1.4),
        shade(col, 1.05),
        false,
    );
    p.door(t + v3(0.0, 0.0, 1.42), Vec3::Z, 1.5, 2.8, hexc(0x24302a));
    p.windows(t + v3(0.0, 5.6, 1.42), Vec3::Z, 1, 1.0, 2.0, 0.0, WHITE);
    p.windows(t + v3(0.0, 9.0, 1.42), Vec3::Z, 1, 1.0, 1.8, 0.0, WHITE);
    let face_at = t + v3(0.0, eave + 2.6 - 0.6, 1.44);
    p.cylinder(
        Mat::Paint,
        face_at - Vec3::Z * 0.04,
        0.95,
        0.95,
        0.06,
        16,
        WHITE,
        true,
    );
    p.beam(
        Mat::Iron,
        face_at,
        face_at + v3(0.0, 0.7, 0.05),
        0.07,
        hexc(0x202020),
    );
    p.beam(
        Mat::Iron,
        face_at,
        face_at + v3(0.5, 0.0, 0.05),
        0.07,
        hexc(0x202020),
    );
    let st = t + v3(0.0, eave + 4.6 - 0.6, 0.5);
    p.cylinder(Mat::Copper, st, 1.9, 0.04, 6.2, 4, hexc(COPPER), false);
    p.rod(
        Mat::Iron,
        st + Vec3::Y * 6.0,
        st + Vec3::Y * 7.2,
        0.05,
        6,
        hexc(GOLD),
    );
    chimney(
        &mut p,
        Mat::Stone,
        v3(-3.6, eave + 1.2, -2.4),
        eave + 4.0,
        col,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

// ------------------------------------------------------------------------------- works

/// A heap of ore (or gold-bearing rock: the look tints it).
fn ore_heap() -> Parts {
    let mut p = Parts::default();
    for (k, (x, z, r, h)) in [
        (0.0, 0.0, 1.8, 1.4),
        (1.2, 0.8, 1.2, 0.9),
        (-1.1, 0.7, 1.0, 0.8),
        (0.4, -1.1, 1.1, 0.8),
    ]
    .into_iter()
    .enumerate()
    {
        p.cylinder(
            Mat::Stone,
            v3(x, -0.2, z),
            r,
            r * 0.25,
            h,
            9,
            shade(hexc(0x6b4a3e), 0.9 + 0.08 * k as f32),
            true,
        );
    }
    p.finish()
}

/// The ore mine: a timber headframe with its sheave wheel over the shaft, a winding house
/// and a loading chute.
fn ore_mine() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x5a4030);
    // Winding house.
    p.cuboid(
        Mat::Planks,
        v3(-7.0, 0.0, -6.6),
        v3(-1.0, 4.4, -1.6),
        hexc(0x7a5a40),
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3(-4.0, 4.4, -4.1),
        6.0,
        5.0,
        2.0,
        0.3,
        false,
        hexc(0x4a4e56),
        hexc(0x7a5a40),
    );
    p.windows(v3(-4.0, 2.4, -1.58), Vec3::Z, 2, 0.8, 1.0, 1.4, WHITE);
    chimney(&mut p, Mat::Brick, v3(-6.0, 4.0, -5.6), 9.6, Vec4::ONE);
    // Headframe: four legs leaning in over the shaft, braces, the wheel.
    let s = v3(3.0, 0.0, -1.0);
    p.cuboid(
        Mat::Stone,
        s + v3(-2.2, -0.5, -2.2),
        s + v3(2.2, 0.4, 2.2),
        hexc(0x8a847a),
        false,
    );
    let top = s + Vec3::Y * 12.0;
    for (dx, dz) in [(-2.0f32, -2.0f32), (2.0, -2.0), (-2.0, 2.0), (2.0, 2.0)] {
        p.beam(
            Mat::Timber,
            s + v3(dx, 0.4, dz),
            top + v3(dx * 0.3, 0.0, dz * 0.3),
            0.3,
            wood,
        );
    }
    for y in [3.5f32, 7.0, 10.0] {
        let k = 1.0 - y / 12.0 * 0.7;
        for (a, b) in [
            (v3(-2.0, y, -2.0), v3(2.0, y, -2.0)),
            (v3(-2.0, y, 2.0), v3(2.0, y, 2.0)),
            (v3(-2.0, y, -2.0), v3(-2.0, y, 2.0)),
            (v3(2.0, y, -2.0), v3(2.0, y, 2.0)),
        ] {
            p.beam(
                Mat::Timber,
                s + a * v3(k, 1.0, k),
                s + b * v3(k, 1.0, k),
                0.18,
                wood,
            );
        }
    }
    // The sheave wheel and the rope down to the winding house.
    let wc = top + Vec3::Y * 1.1;
    let n = 16;
    for k in 0..n {
        let a0 = k as f32 / n as f32 * TAU;
        let a1 = (k + 1) as f32 / n as f32 * TAU;
        p.beam(
            Mat::Iron,
            wc + v3(a0.cos() * 1.4, a0.sin() * 1.4, 0.0),
            wc + v3(a1.cos() * 1.4, a1.sin() * 1.4, 0.0),
            0.12,
            hexc(0x3a3a3a),
        );
        if k % 4 == 0 {
            p.beam(
                Mat::Iron,
                wc,
                wc + v3(a0.cos() * 1.4, a0.sin() * 1.4, 0.0),
                0.07,
                hexc(0x3a3a3a),
            );
        }
    }
    p.rod(
        Mat::Iron,
        wc + v3(-1.4, 0.0, 0.0),
        v3(-1.6, 3.2, -2.2),
        0.04,
        4,
        hexc(0x202020),
    );
    // A chute out to a wagon on rails.
    p.beam(
        Mat::Timber,
        s + v3(1.5, 4.5, 1.5),
        s + v3(3.6, 1.6, 4.6),
        0.6,
        wood,
    );
    for x in [-4.0f32, -1.0, 2.0, 5.0] {
        p.cuboid(
            Mat::Timber,
            v3(x, 0.0, 4.0),
            v3(x + 0.4, 0.12, 6.0),
            wood,
            false,
        );
    }
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The gold mine: a stone engine house with its tall round chimney beside an iron
/// headframe.
fn gold_mine() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0xa39a88);
    p.cuboid(
        Mat::Stone,
        v3(-6.6, -1.0, -6.0),
        v3(-0.6, 9.6, -0.6),
        stone,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Stone,
        v3(-3.6, 9.6, -3.3),
        6.0,
        5.4,
        2.6,
        0.3,
        true,
        hexc(0x44484e),
        stone,
    );
    p.windows(v3(-3.6, 4.0, -0.58), Vec3::Z, 1, 1.4, 3.6, 0.0, WHITE);
    p.windows(v3(-0.58, 6.4, -3.3), Vec3::X, 2, 0.9, 1.8, 1.2, WHITE);
    p.cylinder(
        Mat::Brick,
        v3(-6.0, 0.0, -6.0),
        1.1,
        0.8,
        16.0,
        12,
        Vec4::ONE,
        true,
    );
    // An iron headframe over the shaft.
    let s = v3(3.4, 0.0, -1.2);
    let iron = hexc(0x2e3236);
    let top = s + Vec3::Y * 13.0;
    for (dx, dz) in [(-1.8f32, -1.8f32), (1.8, -1.8), (-1.8, 1.8), (1.8, 1.8)] {
        p.beam(
            Mat::Iron,
            s + v3(dx, 0.0, dz),
            top + v3(dx * 0.35, 0.0, dz * 0.35),
            0.25,
            iron,
        );
    }
    p.beam(Mat::Iron, s + v3(-1.8, 0.0, 2.6), top, 0.3, iron);
    for y in [4.0f32, 8.0, 12.0] {
        let k = 1.0 - y / 13.0 * 0.65;
        p.cuboid(
            Mat::Iron,
            s + v3(-1.8 * k, y, -1.8 * k),
            s + v3(1.8 * k, y + 0.2, 1.8 * k),
            iron,
            true,
        );
    }
    let wc = top + Vec3::Y * 1.0;
    p.cylinder(
        Mat::Iron,
        wc + v3(0.0, 0.0, -0.15),
        1.3,
        1.3,
        0.3,
        16,
        hexc(GOLD),
        true,
    );
    p.cuboid(
        Mat::Stone,
        s + v3(-2.2, -0.5, -2.2),
        s + v3(2.2, 0.4, 2.2),
        stone,
        false,
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The ironworks: a tall stone blast furnace with its charging bridge, a brick casting
/// hall, two stacks.
fn ironworks() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0x8e867a);
    // Casting hall.
    p.cuboid(
        Mat::Brick,
        v3(-9.4, 0.0, -7.0),
        v3(1.0, 6.0, 1.6),
        Vec4::ONE,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Brick,
        v3(-4.2, 6.0, -2.7),
        10.4,
        8.6,
        3.2,
        0.3,
        false,
        hexc(0x3e4248),
        Vec4::ONE,
    );
    p.windows(v3(-4.2, 3.4, 1.62), Vec3::Z, 4, 1.2, 2.6, 1.1, WHITE);
    // Roof lantern for the heat.
    p.cuboid(
        Mat::Planks,
        v3(-7.6, 9.2, -3.4),
        v3(-0.8, 10.2, -2.0),
        hexc(0x3e4248),
        false,
    );
    // The furnace: a tapering stone stack with iron bands.
    let f = v3(5.6, 0.0, -2.4);
    p.cuboid(
        Mat::Stone,
        f + v3(-3.2, 0.0, -3.2),
        f + v3(3.2, 4.0, 3.2),
        stone,
        false,
    );
    p.cylinder(
        Mat::Stone,
        f + Vec3::Y * 4.0,
        2.8,
        2.0,
        9.0,
        14,
        stone,
        false,
    );
    for y in [6.0f32, 9.0, 12.0] {
        p.cylinder(
            Mat::Iron,
            f + Vec3::Y * y,
            2.82 - (y - 4.0) * 0.088,
            2.8 - (y - 4.0) * 0.088,
            0.3,
            14,
            hexc(0x2a2a2a),
            false,
        );
    }
    p.cylinder(
        Mat::Iron,
        f + Vec3::Y * 13.0,
        2.1,
        1.7,
        1.6,
        14,
        hexc(0x3a3a3a),
        true,
    );
    // The glowing tap hole.
    p.cuboid(
        Mat::Lamp,
        f + v3(-0.6, 0.6, 3.2),
        f + v3(0.6, 1.8, 3.3),
        hexc(0xff8a30),
        false,
    );
    // Charging bridge from the hill side.
    p.beam(
        Mat::Timber,
        f + v3(-1.0, 13.0, -2.0),
        f + v3(-1.0, 3.0, -7.0),
        1.2,
        hexc(0x5a4030),
    );
    // Two stacks.
    for (x, z, h) in [(-8.2f32, -5.8f32, 16.0f32), (2.0, -6.2, 18.0)] {
        p.cylinder(Mat::Brick, v3(x, 0.0, z), 0.9, 0.65, h, 10, Vec4::ONE, true);
    }
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// A coffee bush in a row: a rounded green shrub with red cherries.
fn coffee_bush() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Timber,
        Vec3::ZERO,
        0.1,
        0.08,
        0.5,
        5,
        hexc(0x5a4030),
        false,
    );
    for (k, (y, r)) in [(0.5f32, 0.9f32), (1.1, 1.0), (1.7, 0.75), (2.2, 0.4)]
        .into_iter()
        .enumerate()
    {
        p.cylinder(
            Mat::Leaf,
            v3(0.0, y, 0.0),
            r,
            r * 0.85,
            0.65,
            9,
            shade(hexc(0x2e5e2a), 0.9 + 0.06 * k as f32),
            k == 3,
        );
    }
    for k in 0..8 {
        let a = k as f32 / 8.0 * TAU + 0.3;
        p.box_at(
            Mat::Paint,
            v3(a.cos() * 0.92, 1.0 + (k % 3) as f32 * 0.4, a.sin() * 0.92),
            v3(0.09, 0.09, 0.09),
            a,
            hexc(0xb22a22),
        );
    }
    p.finish()
}

/// A tall shade tree over the coffee: a straight trunk, a wide flat crown.
fn shade_tree() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Timber,
        Vec3::ZERO,
        0.28,
        0.2,
        7.0,
        7,
        hexc(0x6a5844),
        false,
    );
    for (k, (y, r, h)) in [(6.2f32, 3.0f32, 1.0f32), (7.0, 2.6, 0.8), (7.6, 1.6, 0.6)]
        .into_iter()
        .enumerate()
    {
        p.cylinder(
            Mat::Leaf,
            v3(0.0, y, 0.0),
            r,
            r * 0.8,
            h,
            10,
            shade(hexc(0x3c6e32), 0.9 + 0.07 * k as f32),
            true,
        );
    }
    p.finish()
}

/// A drying floor: a paved square with beans raked in rows, a low wall and a little shed.
fn drying_floor() -> Parts {
    let mut p = Parts::default();
    p.cuboid(
        Mat::Stone,
        v3(-3.6, -0.5, -3.6),
        v3(3.6, 0.15, 3.6),
        hexc(0xc8bca8),
        false,
    );
    for k in 0..6 {
        let z = -3.0 + k as f32 * 1.2;
        p.cuboid(
            Mat::Crop,
            v3(-3.0, 0.15, z),
            v3(3.0, 0.25, z + 0.6),
            hexc(0x6a3a22),
            false,
        );
    }
    p.cuboid(
        Mat::Planks,
        v3(-3.6, 0.0, -5.6),
        v3(-0.6, 2.8, -3.8),
        hexc(0x8a6a48),
        false,
    );
    p.gable_roof(
        Mat::Thatch,
        Mat::Planks,
        v3(-2.1, 2.8, -4.7),
        3.0,
        1.8,
        1.2,
        0.4,
        false,
        hexc(0xb8a070),
        hexc(0x8a6a48),
    );
    p.finish()
}

/// A cacao tree: a short twisted trunk, a dense crown, yellow and red pods on the trunk.
fn cacao_tree() -> Parts {
    let mut p = Parts::default();
    p.cylinder(
        Mat::Timber,
        Vec3::ZERO,
        0.22,
        0.16,
        2.6,
        6,
        hexc(0x5e4a38),
        false,
    );
    for (k, (y, r)) in [(2.2f32, 1.7f32), (3.1, 1.5), (3.9, 0.9)]
        .into_iter()
        .enumerate()
    {
        p.cylinder(
            Mat::Leaf,
            v3(0.0, y, 0.0),
            r,
            r * 0.8,
            0.9,
            10,
            shade(hexc(0x335a26), 0.9 + 0.07 * k as f32),
            k == 2,
        );
    }
    for k in 0..6 {
        let a = k as f32 / 6.0 * TAU;
        let col = if k % 2 == 0 { 0xd8a428 } else { 0xb04a26 };
        p.box_at(
            Mat::Paint,
            v3(a.cos() * 0.26, 0.9 + (k % 3) as f32 * 0.45, a.sin() * 0.26),
            v3(0.11, 0.2, 0.11),
            a,
            hexc(col),
        );
    }
    p.finish()
}

/// A row of pollarded mulberry bushes, 4 m long, on a low earth bank.
fn mulberry_row() -> Parts {
    let mut p = Parts::default();
    p.cuboid(
        Mat::Clay,
        v3(-0.8, -0.3, -4.6),
        v3(0.8, 0.2, 4.6),
        hexc(0x8a6a48),
        false,
    );
    for k in 0..5 {
        let z = -3.6 + k as f32 * 1.8;
        p.cylinder(
            Mat::Timber,
            v3(0.0, 0.2, z),
            0.12,
            0.1,
            1.0,
            5,
            hexc(0x6a5440),
            false,
        );
        p.cylinder(
            Mat::Leaf,
            v3(0.0, 1.1, z),
            0.85,
            0.55,
            1.1,
            8,
            shade(hexc(0x4a7a30), 0.92 + 0.04 * (k % 3) as f32),
            true,
        );
    }
    p.finish()
}

/// The silk farm's worm shed: long, low and white, louvred vents along its ridge, a
/// reeling room at one end.
fn silk_shed() -> Parts {
    let mut p = Parts::default();
    let wall = hexc(0xf0e8da);
    p.cuboid(
        Mat::Plaster,
        v3(-6.0, 0.0, -2.4),
        v3(6.0, 3.6, 2.4),
        wall,
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Plaster,
        v3(0.0, 3.6, 0.0),
        12.0,
        4.8,
        2.0,
        0.4,
        false,
        hexc(0x9a5a40),
        wall,
    );
    for k in 0..5 {
        let x = -4.8 + k as f32 * 2.4;
        p.cuboid(
            Mat::Planks,
            v3(x - 0.5, 5.4, -0.5),
            v3(x + 0.5, 6.2, 0.5),
            hexc(0x8a6a48),
            false,
        );
        p.cuboid(
            Mat::RoofTile,
            v3(x - 0.7, 6.2, -0.7),
            v3(x + 0.7, 6.4, 0.7),
            hexc(0x9a5a40),
            false,
        );
    }
    p.windows(
        v3(0.0, 2.0, 2.42),
        Vec3::Z,
        6,
        0.7,
        0.9,
        1.2,
        hexc(0x6a5440),
    );
    p.door(v3(-5.0, 0.0, 2.42), Vec3::Z, 1.1, 2.3, hexc(0x6a5440));
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The roastery: a brick shop with a big iron roasting drum in the open front and a tall
/// iron flue.
fn roastery() -> Parts {
    let mut p = Parts::default();
    let c = v3(0.0, 0.0, -1.0);
    p.cuboid(
        Mat::Brick,
        v3(-5.4, 0.0, -7.0),
        v3(5.4, 7.0, 4.0),
        Vec4::ONE,
        false,
    );
    p.gable_roof(
        Mat::RoofTile,
        Mat::Brick,
        v3(0.0, 7.0, -1.5),
        10.8,
        11.0,
        3.2,
        0.3,
        false,
        hexc(0x8a4c38),
        Vec4::ONE,
    );
    window_band(&mut p, c, 10.8, 11.0, 5.0, 4, 4, (1.0, 1.6), WHITE);
    awning(
        &mut p,
        v3(-4.6, 3.6, 4.0),
        9.2,
        1.6,
        0.6,
        [hexc(0x6a3a22), hexc(0xf0e6d2)],
    );
    // The drum, on its brick stove, under the awning.
    p.cuboid(
        Mat::Brick,
        v3(-1.6, 0.0, 4.2),
        v3(1.6, 1.2, 6.2),
        hexc(0x9a7a6a),
        false,
    );
    p.rod(
        Mat::Iron,
        v3(-1.3, 2.0, 5.2),
        v3(1.3, 2.0, 5.2),
        0.75,
        12,
        hexc(0x3a3a3a),
    );
    p.rod(
        Mat::Iron,
        v3(0.0, 2.6, 5.0),
        v3(0.0, 11.0, 4.2),
        0.25,
        8,
        hexc(0x2a2a2a),
    );
    p.door(v3(-3.6, 0.0, 4.02), Vec3::Z, 1.2, 2.4, hexc(0x3a2a1e));
    hanging_sign(
        &mut p,
        v3(5.0, 4.6, 4.0),
        0.9,
        0.7,
        hexc(0x6a3a22),
        hexc(GOLD),
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The clockmaker: a narrow three-storey shop with a big clock in a gilt frame on its gable
/// and a bay window full of clocks.
fn clockmaker() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0x2e4a62);
    let c = v3(0.0, 0.0, -0.6);
    body(&mut p, Mat::Plaster, c, 8.6, 8.6, 11.0, col, hexc(STONE));
    band(&mut p, c, 8.6, 8.6, 4.0, 0.25, 0.1, WHITE);
    band(&mut p, c, 8.6, 8.6, 7.6, 0.2, 0.08, WHITE);
    window_band(&mut p, c, 8.6, 8.6, 5.8, 3, 3, (0.9, 1.7), WHITE);
    window_band(&mut p, c, 8.6, 8.6, 9.3, 3, 3, (0.85, 1.4), WHITE);
    // A shop bay.
    p.cuboid(
        Mat::Paint,
        v3(-3.2, 0.6, 3.7),
        v3(1.4, 3.6, 4.6),
        WHITE,
        true,
    );
    p.windows(v3(-0.9, 2.1, 4.62), Vec3::Z, 2, 1.6, 2.2, 0.4, hexc(GOLD));
    p.door(v3(2.6, 0.6, 3.62), Vec3::Z, 1.1, 2.4, hexc(0x1e2630));
    p.gable_roof(
        Mat::Slate,
        Mat::Plaster,
        v3(0.0, 11.0, -0.6),
        8.6,
        8.6,
        4.4,
        0.3,
        true,
        hexc(0x3e4248),
        col,
    );
    // The clock in the gable.
    let f = v3(0.0, 12.8, 3.72);
    p.cylinder(
        Mat::Copper,
        f - Vec3::Z * 0.1,
        1.35,
        1.35,
        0.12,
        20,
        hexc(GOLD),
        true,
    );
    p.cylinder(
        Mat::Paint,
        f - Vec3::Z * 0.02,
        1.15,
        1.15,
        0.08,
        20,
        WHITE,
        true,
    );
    p.beam(
        Mat::Iron,
        f + v3(0.0, 0.0, 0.08),
        f + v3(0.0, 0.85, 0.08),
        0.07,
        hexc(0x202020),
    );
    p.beam(
        Mat::Iron,
        f + v3(0.0, 0.0, 0.08),
        f + v3(0.55, -0.2, 0.08),
        0.07,
        hexc(0x202020),
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The chocolatier: a pretty two-storey shop in cream and chocolate brown, a striped
/// awning, a chimney for the roasting.
fn chocolatier() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0xf0e2c8);
    let brown = hexc(0x5a3424);
    let c = v3(0.0, 0.0, -0.6);
    body(&mut p, Mat::Plaster, c, 12.4, 8.6, 8.8, col, hexc(STONE));
    p.cuboid(
        Mat::Paint,
        v3(-6.25, 0.6, 3.6),
        v3(6.25, 4.2, 3.7),
        brown,
        false,
    );
    p.windows(v3(-2.0, 2.3, 3.72), Vec3::Z, 3, 1.8, 2.4, 0.5, hexc(GOLD));
    p.door(v3(4.4, 0.6, 3.72), Vec3::Z, 1.3, 2.6, brown);
    awning(
        &mut p,
        v3(-6.0, 4.4, 3.7),
        12.0,
        1.5,
        0.7,
        [brown, hexc(0xf6eee0)],
    );
    band(&mut p, c, 12.4, 8.6, 4.4, 0.25, 0.08, WHITE);
    window_band(&mut p, c, 12.4, 8.6, 6.5, 5, 3, (0.9, 1.6), brown);
    mansard(
        &mut p,
        v3(0.0, 8.8, -0.6),
        12.4,
        8.6,
        2.6,
        1.2,
        hexc(0x4a3a36),
    );
    for x in [-3.6f32, 0.0, 3.6] {
        dormer(&mut p, v3(x, 9.2, 3.0), 1.1, 1.1, 1.2, Mat::Slate);
    }
    chimney(&mut p, Mat::Brick, v3(-4.6, 9.0, -3.2), 13.6, Vec4::ONE);
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The couturier: a fashion house with tall plate-glass windows on two floors, a canopy
/// on iron columns, a mansard with an oval window.
fn couturier() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0xe8e4ea);
    let c = v3(0.0, 0.0, -0.8);
    body(&mut p, Mat::Plaster, c, 13.0, 12.4, 10.4, col, hexc(STONE));
    quoins(&mut p, c, 13.0, 12.4, 0.6, 10.4, WHITE);
    for (y, h) in [(5.0f32, 0.3f32), (10.0, 0.4)] {
        band(&mut p, c, 13.0, 12.4, y, h, 0.15, WHITE);
    }
    p.windows(v3(0.0, 2.8, 5.42), Vec3::Z, 3, 2.6, 3.6, 0.8, hexc(GOLD));
    window_band(&mut p, c, 13.0, 12.4, 7.6, 4, 4, (1.6, 2.8), WHITE);
    // A glass canopy on slim iron columns.
    for x in [-5.6f32, -1.9, 1.9, 5.6] {
        p.rod(
            Mat::Iron,
            v3(x, 0.6, 7.6),
            v3(x, 4.6, 7.6),
            0.09,
            6,
            hexc(0x2a2a2a),
        );
    }
    p.cuboid(
        Mat::Window,
        v3(-6.2, 4.6, 5.4),
        v3(6.2, 4.8, 7.9),
        hexc(0xb8d0dc),
        true,
    );
    p.cuboid(
        Mat::Iron,
        v3(-6.2, 4.8, 7.7),
        v3(6.2, 5.0, 7.9),
        hexc(0x2a2a2a),
        false,
    );
    p.door(v3(0.0, 0.6, 5.42), Vec3::Z, 1.6, 2.8, hexc(0x2a2032));
    mansard(
        &mut p,
        v3(0.0, 10.4, -0.8),
        13.0,
        12.4,
        3.2,
        1.6,
        hexc(0x3e4248),
    );
    p.cylinder(
        Mat::Window,
        v3(0.0, 12.0, 4.0),
        0.8,
        0.8,
        0.1,
        16,
        hexc(0xb8d0dc),
        true,
    );
    flag(&mut p, v3(5.6, 13.6, -0.8), 3.2, hexc(0x7a2a5a));
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The goldsmith: a small stone shop with barred windows, a gilt sign and a strongroom
/// behind.
fn goldsmith() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(0xc8c0ae);
    let c = v3(0.0, 0.0, -0.4);
    body(
        &mut p,
        Mat::Stone,
        c,
        9.0,
        9.4,
        7.6,
        stone,
        shade(stone, 0.8),
    );
    quoins(&mut p, c, 9.0, 9.4, 0.6, 7.6, WHITE);
    band(&mut p, c, 9.0, 9.4, 7.3, 0.4, 0.25, WHITE);
    p.windows(v3(-1.6, 2.4, 4.32), Vec3::Z, 2, 1.4, 2.0, 0.6, hexc(GOLD));
    for k in 0..7 {
        let x = -3.0 + k as f32 * 0.45;
        p.rod(
            Mat::Iron,
            v3(x, 1.4, 4.4),
            v3(x, 3.4, 4.4),
            0.03,
            4,
            hexc(0x202020),
        );
    }
    window_band(&mut p, c, 9.0, 9.4, 5.6, 3, 3, (0.85, 1.4), WHITE);
    p.door(v3(2.4, 0.6, 4.32), Vec3::Z, 1.1, 2.4, hexc(0x1a1a1a));
    p.hip_roof(
        Mat::Slate,
        v3(0.0, 7.6, -0.4),
        9.0,
        9.4,
        2.4,
        0.2,
        hexc(0x3e4248),
    );
    hanging_sign(
        &mut p,
        v3(-4.4, 4.0, 4.3),
        0.9,
        0.9,
        hexc(GOLD),
        hexc(0x1e2630),
    );
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// A stone obelisk on a plinth, gilded at the tip.
fn obelisk() -> Parts {
    let mut p = Parts::default();
    let stone = hexc(STONE);
    p.cuboid(
        Mat::Stone,
        v3(-0.7, -0.3, -0.7),
        v3(0.7, 0.9, 0.7),
        stone,
        false,
    );
    p.cylinder(
        Mat::Stone,
        v3(0.0, 0.9, 0.0),
        0.5,
        0.28,
        4.2,
        4,
        stone,
        false,
    );
    p.cylinder(
        Mat::Copper,
        v3(0.0, 5.1, 0.0),
        0.28,
        0.02,
        0.6,
        4,
        hexc(GOLD),
        false,
    );
    p.finish()
}

// ------------------------------------------------------------------------------ public

/// The theatre: a great classical house on a broad stair, six columns under a pediment,
/// a fly tower behind, a copper dome over the auditorium and flags on the roof.
fn theatre() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0xefe6d2);
    let stone = hexc(STONE);
    // The auditorium block.
    let c = v3(0.0, 0.0, -2.4);
    body(
        &mut p,
        Mat::Plaster,
        c,
        20.0,
        15.6,
        13.0,
        col,
        shade(stone, 0.8),
    );
    quoins(&mut p, c, 20.0, 15.6, 0.6, 13.0, WHITE);
    for (y, h, d) in [(5.4f32, 0.35f32, 0.15f32), (12.6, 0.5, 0.4)] {
        band(&mut p, c, 20.0, 15.6, y, h, d, WHITE);
    }
    window_band(&mut p, c, 20.0, 15.6, 3.0, 7, 5, (1.2, 2.4), WHITE);
    window_band(&mut p, c, 20.0, 15.6, 9.0, 7, 5, (1.3, 3.4), WHITE);
    balustrade_round(&mut p, v3(0.0, 13.1, c.z), 20.6, 16.2, 1.0, WHITE);
    // The fly tower over the stage at the back.
    p.cuboid(
        Mat::Plaster,
        v3(-7.0, 13.0, -10.0),
        v3(7.0, 19.0, -4.4),
        shade(col, 0.96),
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Plaster,
        v3(0.0, 19.0, -7.2),
        14.0,
        5.6,
        2.2,
        0.3,
        false,
        hexc(0x4a5058),
        col,
    );
    // The dome over the house.
    let d = v3(0.0, 13.0, -0.2);
    p.cylinder(Mat::Plaster, d, 4.4, 4.4, 2.4, 18, col, false);
    for k in 0..10 {
        let a = k as f32 / 10.0 * TAU;
        p.windows(
            d + v3(a.sin() * 4.42, 1.2, a.cos() * 4.42),
            v3(a.sin(), 0.0, a.cos()),
            1,
            0.6,
            1.4,
            0.0,
            WHITE,
        );
    }
    dome(
        &mut p,
        Mat::Copper,
        d + Vec3::Y * 2.4,
        4.7,
        4.6,
        hexc(COPPER),
    );
    cupola(&mut p, d + Vec3::Y * 6.9, 0.8, hexc(GOLD), WHITE);
    // The portico on its stair.
    let zf = c.z + 7.8;
    let pz = zf + 3.0;
    p.cuboid(
        Mat::Stone,
        v3(-8.0, -1.0, zf),
        v3(8.0, 1.4, pz + 0.6),
        stone,
        false,
    );
    steps(&mut p, 0.0, pz + 0.6, 14.0, 4, 1.4, stone);
    columns(&mut p, -6.6, 6.6, pz, 6, 1.4, 10.6, 0.5, WHITE);
    p.cuboid(
        Mat::Stone,
        v3(-7.4, 12.0, zf),
        v3(7.4, 12.8, pz + 0.65),
        WHITE,
        true,
    );
    pediment(&mut p, -7.6, 7.6, pz + 0.65, 12.8, 3.4, 3.7, WHITE);
    for x in [-3.6f32, 0.0, 3.6] {
        p.door(v3(x, 1.4, zf + 0.02), Vec3::Z, 1.8, 3.6, hexc(0x5a1e26));
    }
    for (x, colr) in [(-9.0f32, 0x9a1e2a), (9.0, 0x22406a)] {
        flag(&mut p, v3(x, 14.1, c.z + 6.6), 4.0, hexc(colr));
    }
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}

/// The bank: a block of dressed stone on a high base, four giant columns between pilasters
/// under a pediment, an attic storey and a shallow dome.
fn bank() -> Parts {
    let mut p = Parts::default();
    let col = hexc(0xd8d2c2);
    let stone = shade(col, 0.85);
    let c = v3(0.0, 0.0, -1.4);
    body(&mut p, Mat::Stone, c, 17.0, 14.0, 14.0, col, stone);
    for k in 1..5 {
        let y = 0.6 + k as f32 * 0.7;
        band(&mut p, c, 17.0, 14.0, y, 0.06, 0.02, shade(col, 0.6));
    }
    for (y, h, d) in [
        (3.8f32, 0.4f32, 0.2f32),
        (11.0, 0.6, 0.45),
        (14.0, 0.4, 0.2),
    ] {
        band(&mut p, c, 17.0, 14.0, y, h, d, WHITE);
    }
    window_band(&mut p, c, 17.0, 14.0, 2.2, 6, 5, (0.9, 1.3), hexc(0x202020));
    window_band(&mut p, c, 17.0, 14.0, 7.4, 6, 5, (1.2, 3.4), WHITE);
    window_band(&mut p, c, 17.0, 14.0, 12.6, 6, 5, (0.9, 1.0), WHITE);
    let zf = c.z + 7.0;
    let pz = zf + 2.4;
    p.cuboid(
        Mat::Stone,
        v3(-6.0, -1.0, zf),
        v3(6.0, 2.0, pz + 0.7),
        stone,
        false,
    );
    steps(&mut p, 0.0, pz + 0.7, 10.0, 5, 2.0, stone);
    columns(&mut p, -4.8, 4.8, pz, 4, 2.0, 9.0, 0.55, WHITE);
    p.cuboid(
        Mat::Stone,
        v3(-5.8, 11.0, zf),
        v3(5.8, 11.8, pz + 0.75),
        WHITE,
        true,
    );
    pediment(&mut p, -6.0, 6.0, pz + 0.75, 11.8, 2.6, 3.15, WHITE);
    p.door(v3(0.0, 2.0, zf + 0.02), Vec3::Z, 2.2, 4.4, hexc(0x3a2a1a));
    // Gilt name band over the door.
    p.cuboid(
        Mat::Copper,
        v3(-2.6, 7.0, zf + 0.02),
        v3(2.6, 7.6, zf + 0.12),
        hexc(GOLD),
        false,
    );
    balustrade_round(&mut p, v3(0.0, 14.4, c.z), 17.4, 14.4, 1.0, WHITE);
    let d = v3(0.0, 14.4, c.z - 0.6);
    p.cylinder(Mat::Stone, d, 4.2, 4.2, 1.4, 18, col, false);
    dome(
        &mut p,
        Mat::Copper,
        d + Vec3::Y * 1.4,
        4.4,
        3.0,
        hexc(COPPER),
    );
    p.rod(
        Mat::Iron,
        d + Vec3::Y * 4.3,
        d + Vec3::Y * 6.0,
        0.08,
        6,
        hexc(GOLD),
    );
    for x in [-7.8f32, 7.8] {
        p.cylinder(
            Mat::Copper,
            v3(x, 14.4, zf - 0.6),
            0.35,
            0.35,
            1.3,
            8,
            hexc(GOLD),
            true,
        );
    }
    let _ = PI;
    p.ground_occlusion(0.6, 2.0);
    p.finish()
}
