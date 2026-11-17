//! The looks of the three buildings that hold items: the Trading Union (a weighhouse with
//! stepped gables, a hoist and guild banners), the Town Hall (an arcaded hall with a clock
//! tower and a square) and the Harbour Office (a white office with a lookout tower, a signal
//! mast and a time ball).
//!
//! Models stand in a building's own space (origin at the middle of the footprint at floor
//! height, front toward +Z), in the same art kit as `looks`, `burgher` and `civic`.

use std::f32::consts::TAU;

use engine::prelude::*;

use crate::art::{Mat, Parts, hexc, shade, v3};
use crate::burgher::{chimney, dormer, face, steps};
use crate::civic::{dome, sheet};
use crate::defs::Kind;
use crate::looks::{ModelKey, Piece, WHITE, h01};
use crate::regional::up_quad;

/// The models this module builds (they are listed in `looks::MODELS`; a test checks that).
#[cfg(test)]
pub const MODELS: &[ModelKey] = &["trading_union", "town_hall", "harbour_office"];

pub fn build(key: ModelKey) -> Option<Parts> {
    Some(match key {
        "trading_union" => trading_union(),
        "town_hall" => town_hall(),
        "harbour_office" => harbour_office(),
        _ => return None,
    })
}

/// The pieces of one of the three buildings.
pub fn look(kind: Kind, seed: u64, out: &mut Vec<Piece>) -> bool {
    let r = |k: u64| h01(seed, 900 + k);
    match kind {
        Kind::TradingUnion => {
            out.push(Piece::at("trading_union", Vec3::ZERO));
            // Goods waiting for the scales.
            for i in 0..5u64 {
                out.push(
                    Piece::at(
                        "crate",
                        v3(
                            -8.6 + (i % 3) as f32 * 1.15,
                            0.0,
                            5.0 + (i / 3) as f32 * 1.15,
                        ),
                    )
                    .yaw(r(i) * 0.4 - 0.2),
                );
            }
            for i in 0..6u64 {
                out.push(Piece::at(
                    "barrel",
                    v3(
                        6.9 + (i % 3) as f32 * 0.85,
                        0.0,
                        2.0 + (i / 3) as f32 * 0.85,
                    ),
                ));
            }
            out.push(Piece::at("sacks", v3(-2.2, 0.0, 6.4)).yaw(0.3));
            out.push(Piece::at("sacks", v3(1.8, 0.0, 6.6)).yaw(-0.5).scale(0.85));
            out.push(Piece::at("hand_cart", v3(5.4, 0.0, 6.2)).yaw(2.2));
            out.push(Piece::at("lamp_post", v3(-9.2, 0.0, 7.4)));
            out.push(Piece::at("lamp_post", v3(9.2, 0.0, 7.4)));
        }
        Kind::TownHall => {
            out.push(Piece::at("town_hall", Vec3::ZERO));
            for x in [-8.8, -3.4, 3.4, 8.8] {
                out.push(Piece::at("lamp_post", v3(x, 0.0, 9.2)));
            }
            for x in [-6.6, 6.6] {
                out.push(Piece::at("bench", v3(x, 0.0, 7.6)));
                out.push(Piece::at("tub_shrub", v3(x, 0.0, 3.2)));
            }
            out.push(Piece::at("tub_shrub", v3(-3.6, 0.0, 3.2)).tint([0.95, 1.0, 0.9]));
            out.push(Piece::at("tub_shrub", v3(3.6, 0.0, 3.2)).tint([0.95, 1.0, 0.9]));
        }
        Kind::HarbourOffice => {
            out.push(Piece::at("harbour_office", Vec3::ZERO));
            for i in 0..4u64 {
                out.push(
                    Piece::at(
                        "crate",
                        v3(
                            -7.0 + (i % 2) as f32 * 1.15,
                            0.0,
                            3.0 + (i / 2) as f32 * 1.15,
                        ),
                    )
                    .yaw(r(i) * 0.3),
                );
            }
            for i in 0..3u64 {
                out.push(Piece::at("barrel", v3(-7.2 + i as f32 * 0.85, 0.0, 5.6)));
            }
            out.push(Piece::at("lamp_post", v3(-7.2, 0.0, 7.2)));
            out.push(Piece::at("lamp_post", v3(7.2, 0.0, 7.2)));
            out.push(Piece::at("bench", v3(-2.4, 0.0, 2.4)));
        }
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------------------ helpers

/// Dressed stone, pale.
const ASHLAR: u32 = 0xdcd6c8;
/// The company's red and gold.
const RED: u32 = 0xa02c2c;
const GOLD: u32 = 0xd9a520;
const NAVY: u32 = 0x24476b;

/// A flat disc facing `n` (a clock face, a sign).
fn disc(p: &mut Parts, m: Mat, c: Vec3, n: Vec3, r: f32, col: Vec4) {
    let along = Vec3::Y.cross(n).normalize();
    let segs = 20;
    for i in 0..segs {
        let a0 = i as f32 / segs as f32 * TAU;
        let a1 = (i + 1) as f32 / segs as f32 * TAU;
        let at = |a: f32| c + (along * a.cos() + Vec3::Y * a.sin()) * r;
        p.tri(m, c, at(a0), at(a1), col);
    }
}

/// A clock face on a wall facing `n`, with its rim and hands (ten past ten).
fn clock(p: &mut Parts, c: Vec3, n: Vec3, r: f32) {
    let along = Vec3::Y.cross(n).normalize();
    disc(p, Mat::Paint, c + n * 0.02, n, r + 0.12, hexc(0x2a2a2e));
    disc(p, Mat::Paint, c + n * 0.05, n, r, hexc(0xf2eee2));
    let hub = c + n * 0.09;
    for i in 0..12 {
        let a = i as f32 / 12.0 * TAU;
        let d = along * a.cos() + Vec3::Y * a.sin();
        p.beam(
            Mat::Paint,
            hub + d * (r * 0.78),
            hub + d * (r * 0.94),
            0.07,
            hexc(0x2a2a2e),
        );
    }
    let hand = |p: &mut Parts, a: f32, len: f32, w: f32| {
        let d = along * a.sin() + Vec3::Y * a.cos();
        p.beam(
            Mat::Paint,
            hub + n * 0.02,
            hub + n * 0.02 + d * len,
            w,
            hexc(0x1c1c20),
        );
    };
    hand(p, -1.0, r * 0.5, 0.1);
    hand(p, 1.05, r * 0.78, 0.07);
    disc(p, Mat::Copper, hub + n * 0.07, n, 0.1, Vec4::ONE);
}

/// A ball (a time ball, a finial) centred on `c`.
fn ball(p: &mut Parts, m: Mat, c: Vec3, r: f32, col: Vec4) {
    let rings = 6;
    for k in 0..rings {
        let a0 = (k as f32 / rings as f32 - 0.5) * std::f32::consts::PI;
        let a1 = ((k + 1) as f32 / rings as f32 - 0.5) * std::f32::consts::PI;
        p.cylinder(
            m,
            c + v3(0.0, a0.sin() * r, 0.0),
            a0.cos() * r,
            a1.cos() * r,
            (a1.sin() - a0.sin()) * r,
            12,
            col,
            false,
        );
    }
}

/// A banner hanging from a pole that juts out of a wall facing +Z at `at` (the pole's
/// root): `w` wide, `h` long, ending in a swallowtail, with a band of `mark` across it.
fn banner(p: &mut Parts, at: Vec3, w: f32, h: f32, col: Vec4, mark: Vec4) {
    let z = at.z + 0.55;
    p.beam(
        Mat::Iron,
        at,
        v3(at.x, at.y + 0.25, z + 0.15),
        0.06,
        Vec4::ONE,
    );
    p.beam(
        Mat::Timber,
        v3(at.x - w * 0.5 - 0.1, at.y + 0.25, z),
        v3(at.x + w * 0.5 + 0.1, at.y + 0.25, z),
        0.07,
        Vec4::ONE,
    );
    let top = at.y + 0.2;
    let (x0, x1) = (at.x - w * 0.5, at.x + w * 0.5);
    sheet(
        p,
        Mat::Canvas,
        v3(x0, top - h, z),
        v3(x1, top - h, z),
        v3(x1, top, z),
        v3(x0, top, z),
        col,
    );
    // The swallowtail.
    for (a, b) in [(x0, at.x), (x1, at.x)] {
        let (q0, q1, q2) = (
            v3(a, top - h, z),
            v3(a, top - h - w * 0.6, z),
            v3(b, top - h, z),
        );
        p.tri(Mat::Canvas, q0, q1, q2, col);
        p.tri(Mat::Canvas, q2, q1, q0, shade(col, 0.85));
    }
    // The device: a band and a roundel, on both sides.
    for s in [-1.0f32, 1.0] {
        let zz = z + s * 0.012;
        face(
            p,
            Mat::Paint,
            [
                v3(x0, top - h * 0.3, zz),
                v3(x1, top - h * 0.3, zz),
                v3(x1, top - h * 0.3 + 0.16, zz),
                v3(x0, top - h * 0.3 + 0.16, zz),
            ],
            mark,
            v3(0.0, 0.0, s),
        );
        disc(
            p,
            Mat::Paint,
            v3(at.x, top - h * 0.62, zz),
            v3(0.0, 0.0, s),
            w * 0.26,
            mark,
        );
    }
}

/// The company's pennant on a staff: the staff stands on `foot` and is `h` high; the pennant
/// flies toward +X, tapering, with a roundel at the hoist.
fn flag(p: &mut Parts, foot: Vec3, h: f32, size: (f32, f32), col: Vec4, mark: Vec4) {
    p.cylinder(Mat::Paint, foot, 0.07, 0.04, h, 8, WHITE, true);
    ball(
        p,
        Mat::Copper,
        foot + v3(0.0, h + 0.08, 0.0),
        0.1,
        hexc(GOLD),
    );
    let top = foot + v3(0.0, h - 0.15, 0.0);
    let (w, d) = size;
    // Flat at the hoist, then two folds, so it does not hang like a board.
    let pts = |k: f32| {
        let sag = 0.3 * ((k - 0.4).max(0.0) * 5.0).sin();
        let half = d * 0.5 * (1.0 - 0.72 * k);
        let mid = top + v3(w * k, -d * 0.5 - 0.12 * k, sag);
        (mid - Vec3::Y * half, mid + Vec3::Y * half)
    };
    let n = 6;
    for i in 0..n {
        let (a0, a1) = pts(i as f32 / n as f32);
        let (b0, b1) = pts((i + 1) as f32 / n as f32);
        sheet(p, Mat::Canvas, a0, b0, b1, a1, col);
    }
    let (lo, hi) = pts(0.2);
    for s in [-1.0f32, 1.0] {
        disc(
            p,
            Mat::Paint,
            (lo + hi) * 0.5 + v3(0.0, 0.0, 0.02 * s),
            v3(0.0, 0.0, s),
            d * 0.24,
            mark,
        );
    }
}

/// Corner stones up the four corners of a box.
fn quoins(p: &mut Parts, x: (f32, f32), z: (f32, f32), y: (f32, f32), col: Vec4) {
    let n = ((y.1 - y.0) / 0.55).floor() as usize;
    for k in 0..n {
        let y0 = y.0 + k as f32 * 0.55;
        let long = if k % 2 == 0 { 0.55 } else { 0.32 };
        for (cx, sx) in [(x.0, 1.0f32), (x.1, -1.0)] {
            for (cz, sz) in [(z.0, 1.0f32), (z.1, -1.0)] {
                let (xa, xb) = (cx - sx * 0.05, cx + sx * long);
                let (za, zb) = (cz - sz * 0.05, cz + sz * (0.87 - long));
                p.cuboid(
                    Mat::Plaster,
                    v3(xa.min(xb), y0, za.min(zb)),
                    v3(xa.max(xb), y0 + 0.5, za.max(zb)),
                    col,
                    false,
                );
            }
        }
    }
}

/// A paved apron in front of a building, a little proud of the ground.
fn apron(p: &mut Parts, x: (f32, f32), z: (f32, f32)) {
    p.cuboid(
        Mat::Cobble,
        v3(x.0, -1.2, z.0),
        v3(x.1, 0.1, z.1),
        hexc(0xe4e0d8),
        false,
    );
    // A kerb round it.
    let k = hexc(0xd6d2c8);
    for (a, b) in [
        (v3(x.0, 0.12, z.1), v3(x.1, 0.12, z.1)),
        (v3(x.0, 0.12, z.0), v3(x.0, 0.12, z.1)),
        (v3(x.1, 0.12, z.0), v3(x.1, 0.12, z.1)),
    ] {
        p.beam(Mat::Plaster, a, b, 0.24, k);
    }
}

// ---------------------------------------------------------------------------- Town Hall

/// A brick hall over a stone arcade, a clock tower rising through its front under a copper
/// cupola, a balcony for proclamations, and a paved square with the founder's statue.
fn town_hall() -> Parts {
    let mut p = Parts::default();
    let ashlar = hexc(ASHLAR);
    let (l, w) = (15.0, 9.6);
    let c = v3(0.0, 0.0, -3.6);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (floor, mid, wall) = (0.7, 4.5, 8.7);
    // Plinth.
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - 0.2, -3.0, zb - 0.2),
        v3(l * 0.5 + 0.2, floor, zf + 0.2),
        shade(ashlar, 0.9),
        false,
    );
    // Ground floor: stone, set back behind the arcade.
    let za = zf - 1.7;
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, floor, zb),
        v3(l * 0.5, mid, za),
        ashlar,
        false,
    );
    // The arcade: piers, chamfered heads and a lintel band.
    let bays = 6;
    let bay = l / bays as f32;
    for i in 0..=bays {
        let x = -l * 0.5 + i as f32 * bay;
        let (x0, x1) = ((x - 0.4).max(-l * 0.5), (x + 0.4).min(l * 0.5));
        p.cuboid(
            Mat::Plaster,
            v3(x0, floor, zf - 0.8),
            v3(x1, mid - 0.9, zf),
            ashlar,
            false,
        );
        p.cuboid(
            Mat::Plaster,
            v3(x0 - 0.08, floor, zf - 0.88),
            v3(x1 + 0.08, floor + 0.35, zf + 0.08),
            shade(ashlar, 0.92),
            false,
        );
        // Haunches: the arch springs from each pier.
        for s in [-1.0f32, 1.0] {
            if (i == 0 && s < 0.0) || (i == bays && s > 0.0) {
                continue;
            }
            let xe = if s > 0.0 { x1 } else { x0 };
            for z in [zf, zf - 0.8] {
                let out = if z == zf { Vec3::Z } else { Vec3::NEG_Z };
                let (a, b, cc) = (
                    v3(xe, mid - 1.9, z),
                    v3(xe, mid - 0.9, z),
                    v3(xe + s * 0.95, mid - 0.9, z),
                );
                if (b - a).cross(cc - a).dot(out) >= 0.0 {
                    p.tri(Mat::Plaster, a, b, cc, ashlar);
                } else {
                    p.tri(Mat::Plaster, cc, b, a, ashlar);
                }
            }
        }
    }
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5, mid - 0.9, zf - 0.8),
        v3(l * 0.5, mid, zf),
        ashlar,
        true,
    );
    // The arcade's ceiling and its back wall's doors and windows.
    up_quad(
        &mut p,
        Mat::Cobble,
        (-l * 0.5, l * 0.5),
        (za, zf),
        floor + 0.01,
        hexc(0xd8d2c4),
    );
    for x in [-5.0, 5.0] {
        p.door(v3(x, floor, za), Vec3::Z, 1.3, 2.5, hexc(0x3a2a1c));
    }
    for x in [-2.6, 2.6] {
        p.windows(v3(x, 2.6, za + 0.02), Vec3::Z, 1, 1.1, 1.9, 0.0, WHITE);
    }
    // Upper floor: brick with stone dressings.
    p.cuboid(
        Mat::Brick,
        v3(-l * 0.5, mid, zb),
        v3(l * 0.5, wall, zf),
        Vec4::ONE,
        true,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - 0.12, mid - 0.02, zb - 0.12),
        v3(l * 0.5 + 0.12, mid + 0.28, zf + 0.12),
        ashlar,
        true,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-l * 0.5 - 0.25, wall - 0.35, zb - 0.25),
        v3(l * 0.5 + 0.25, wall, zf + 0.25),
        ashlar,
        true,
    );
    quoins(
        &mut p,
        (-l * 0.5, l * 0.5),
        (zb, zf),
        (mid + 0.3, wall - 0.4),
        ashlar,
    );
    // Tall council-chamber windows with stone heads.
    for x in [-5.9, -3.9, 3.9, 5.9] {
        p.windows(v3(x, 6.5, zf + 0.02), Vec3::Z, 1, 1.05, 2.6, 0.0, WHITE);
        p.cuboid(
            Mat::Plaster,
            v3(x - 0.7, 7.85, zf),
            v3(x + 0.7, 8.08, zf + 0.1),
            ashlar,
            true,
        );
    }
    p.windows(
        v3(0.0, 6.5, zb - 0.02),
        Vec3::NEG_Z,
        6,
        1.05,
        2.6,
        1.2,
        WHITE,
    );
    p.windows(
        v3(0.0, 2.6, zb - 0.02),
        Vec3::NEG_Z,
        6,
        1.05,
        1.9,
        1.2,
        WHITE,
    );
    for s in [-1.0f32, 1.0] {
        for (y, h) in [(6.5, 2.6), (2.6, 1.9)] {
            p.windows(
                v3(s * (l * 0.5 + 0.02), y, c.z - 0.6),
                v3(s, 0.0, 0.0),
                3,
                1.05,
                h,
                1.3,
                WHITE,
            );
        }
    }
    // The roof, its dormers and chimneys.
    let rh = 3.4;
    p.hip_roof(Mat::Slate, v3(0.0, wall, c.z), l, w, rh, 0.45, Vec4::ONE);
    for x in [-4.9, 4.9] {
        dormer(
            &mut p,
            v3(x, wall + 0.75, zf - 1.3),
            1.2,
            1.1,
            1.5,
            Mat::Slate,
        );
    }
    for x in [-6.2, 6.2] {
        chimney(
            &mut p,
            Mat::Brick,
            v3(x, wall + 0.6, c.z - 1.0),
            wall + 4.3,
            Vec4::ONE,
        );
    }
    // The clock tower, standing forward of the front.
    let (tw, tz) = (3.8, zf - 1.3);
    let (tx0, tx1, tz0, tz1) = (-tw * 0.5, tw * 0.5, tz - tw * 0.5, tz + tw * 0.5);
    let (stage, belfry, top) = (12.6, 16.0, 16.5);
    p.cuboid(
        Mat::Plaster,
        v3(tx0, -0.5, tz0),
        v3(tx1, mid, tz1),
        ashlar,
        false,
    );
    p.cuboid(
        Mat::Brick,
        v3(tx0, mid, tz0),
        v3(tx1, stage, tz1),
        Vec4::ONE,
        false,
    );
    quoins(
        &mut p,
        (tx0, tx1),
        (tz0, tz1),
        (mid + 0.3, stage - 0.1),
        ashlar,
    );
    p.cuboid(
        Mat::Plaster,
        v3(tx0 - 0.15, stage, tz0 - 0.15),
        v3(tx1 + 0.15, stage + 0.3, tz1 + 0.15),
        ashlar,
        true,
    );
    p.cuboid(
        Mat::Plaster,
        v3(tx0, stage + 0.3, tz0),
        v3(tx1, belfry, tz1),
        ashlar,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(tx0 - 0.3, belfry, tz0 - 0.3),
        v3(tx1 + 0.3, top, tz1 + 0.3),
        shade(ashlar, 0.95),
        true,
    );
    // The great door, its fanlight and steps.
    p.door(v3(0.0, floor, tz1), Vec3::Z, 2.0, 3.0, hexc(0x24402e));
    disc(
        &mut p,
        Mat::Window,
        v3(0.0, floor + 3.0, tz1 + 0.03),
        Vec3::Z,
        1.0,
        Vec4::ONE,
    );
    p.cuboid(
        Mat::Plaster,
        v3(-1.25, floor + 2.92, tz1),
        v3(1.25, floor + 3.08, tz1 + 0.12),
        ashlar,
        true,
    );
    steps(&mut p, 0.0, tz1 + 0.02, 3.4, 3, floor, shade(ashlar, 0.95));
    for x in [-5.0, 5.0] {
        steps(&mut p, x, zf + 0.02, 3.0, 3, floor, shade(ashlar, 0.95));
    }
    // The balcony for proclamations.
    p.cuboid(
        Mat::Plaster,
        v3(tx0 - 0.3, mid + 0.25, tz1),
        v3(tx1 + 0.3, mid + 0.5, tz1 + 1.1),
        ashlar,
        true,
    );
    for x in [tx0 - 0.1, tx1 + 0.1] {
        p.beam(
            Mat::Plaster,
            v3(x, mid - 0.5, tz1),
            v3(x, mid + 0.25, tz1 + 0.8),
            0.22,
            ashlar,
        );
    }
    let rail = hexc(0x26262a);
    let by = mid + 0.5;
    for (a, b) in [
        (
            v3(tx0 - 0.25, by, tz1 + 1.05),
            v3(tx1 + 0.25, by, tz1 + 1.05),
        ),
        (v3(tx0 - 0.25, by, tz1), v3(tx0 - 0.25, by, tz1 + 1.05)),
        (v3(tx1 + 0.25, by, tz1), v3(tx1 + 0.25, by, tz1 + 1.05)),
    ] {
        let n = ((b - a).length() / 0.3).round() as usize;
        for i in 0..=n {
            let at = a + (b - a) * (i as f32 / n as f32);
            p.beam(Mat::Iron, at, at + Vec3::Y * 0.95, 0.04, rail);
        }
        p.beam(
            Mat::Iron,
            a + Vec3::Y * 0.95,
            b + Vec3::Y * 0.95,
            0.07,
            rail,
        );
    }
    p.door(v3(0.0, mid + 0.5, tz1), Vec3::Z, 1.3, 2.5, hexc(0x24402e));
    // The company's banners either side of the balcony door.
    for x in [-1.35, 1.35] {
        banner(
            &mut p,
            v3(x, mid + 4.3, tz1),
            0.75,
            2.6,
            hexc(RED),
            hexc(GOLD),
        );
    }
    // Belfry louvres below the clocks, on every side.
    for (n, at) in [
        (Vec3::Z, v3(0.0, 0.0, tz1 + 0.02)),
        (Vec3::NEG_Z, v3(0.0, 0.0, tz0 - 0.02)),
        (Vec3::X, v3(tx1 + 0.02, 0.0, tz)),
        (Vec3::NEG_X, v3(tx0 - 0.02, 0.0, tz)),
    ] {
        if n != Vec3::NEG_Z {
            p.windows(at + v3(0.0, 10.6, 0.0), n, 2, 0.6, 2.0, 0.5, ashlar);
        }
        clock(&mut p, at + v3(0.0, stage + 1.95, 0.0), n, 1.25);
    }
    // The cupola: an open lantern under a copper dome, and the vane.
    let cup = v3(0.0, top, tz);
    p.cylinder(Mat::Plaster, cup, 1.5, 1.4, 0.4, 8, ashlar, true);
    for i in 0..8 {
        let a = (i as f32 + 0.5) / 8.0 * TAU;
        let d = v3(a.cos(), 0.0, a.sin()) * 1.15;
        p.beam(
            Mat::Paint,
            cup + d + v3(0.0, 0.4, 0.0),
            cup + d + v3(0.0, 2.3, 0.0),
            0.2,
            WHITE,
        );
    }
    p.cylinder(
        Mat::Copper,
        cup + v3(0.0, 0.9, 0.0),
        0.5,
        0.26,
        0.75,
        10,
        hexc(0xc8a24a),
        true,
    );
    p.cylinder(
        Mat::Paint,
        cup + v3(0.0, 2.3, 0.0),
        1.5,
        1.5,
        0.25,
        8,
        WHITE,
        true,
    );
    let verdigris = hexc(0x6fae9a);
    dome(
        &mut p,
        Mat::Copper,
        cup + v3(0.0, 2.55, 0.0),
        1.4,
        1.5,
        verdigris,
    );
    p.cylinder(
        Mat::Copper,
        cup + v3(0.0, 4.0, 0.0),
        0.16,
        0.03,
        1.9,
        8,
        verdigris,
        false,
    );
    ball(
        &mut p,
        Mat::Copper,
        cup + v3(0.0, 4.75, 0.0),
        0.2,
        hexc(GOLD),
    );
    // The vane: an arrow and a pennon.
    let vy = cup.y + 5.6;
    p.beam(
        Mat::Iron,
        v3(-0.8, vy, tz),
        v3(0.9, vy, tz),
        0.05,
        Vec4::ONE,
    );
    sheet(
        &mut p,
        Mat::Copper,
        v3(-0.8, vy - 0.22, tz),
        v3(-0.25, vy, tz),
        v3(-0.8, vy + 0.22, tz),
        v3(-0.8, vy, tz),
        hexc(GOLD),
    );
    // The square: the founder on a pedestal, and two flags.
    let s = v3(0.0, 0.1, 7.0);
    p.cuboid(
        Mat::Plaster,
        s + v3(-0.9, 0.0, -0.9),
        s + v3(0.9, 0.3, 0.9),
        shade(ashlar, 0.9),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        s + v3(-0.6, 0.3, -0.6),
        s + v3(0.6, 1.7, 0.6),
        ashlar,
        false,
    );
    p.cuboid(
        Mat::Plaster,
        s + v3(-0.7, 1.7, -0.7),
        s + v3(0.7, 1.85, 0.7),
        ashlar,
        true,
    );
    let bronze = hexc(0x5d8a78);
    let f = s + v3(0.0, 1.85, 0.0);
    for dx in [-0.14, 0.14] {
        p.cylinder(
            Mat::Copper,
            f + v3(dx, 0.0, 0.0),
            0.11,
            0.13,
            0.9,
            8,
            bronze,
            false,
        );
    }
    p.cylinder(
        Mat::Copper,
        f + v3(0.0, 0.85, 0.0),
        0.3,
        0.24,
        0.85,
        10,
        bronze,
        true,
    );
    ball(&mut p, Mat::Copper, f + v3(0.0, 1.92, 0.0), 0.17, bronze);
    // One arm on a cane, one raised with a rolled charter.
    p.beam(
        Mat::Copper,
        f + v3(-0.27, 1.6, 0.0),
        f + v3(-0.42, 0.95, 0.12),
        0.1,
        bronze,
    );
    p.beam(
        Mat::Copper,
        f + v3(-0.42, 0.95, 0.12),
        f + v3(-0.42, 0.0, 0.2),
        0.04,
        bronze,
    );
    p.beam(
        Mat::Copper,
        f + v3(0.27, 1.6, 0.0),
        f + v3(0.62, 1.95, 0.22),
        0.1,
        bronze,
    );
    p.rod(
        Mat::Copper,
        f + v3(0.56, 1.9, 0.2),
        f + v3(0.72, 2.25, 0.3),
        0.05,
        8,
        hexc(GOLD),
    );
    // A cocked hat.
    p.cylinder(
        Mat::Copper,
        f + v3(0.0, 2.05, 0.0),
        0.26,
        0.1,
        0.12,
        3,
        bronze,
        true,
    );
    for x in [-8.9, 7.2] {
        flag(
            &mut p,
            v3(x, 0.1, 5.0),
            8.5,
            (1.9, 1.2),
            hexc(RED),
            hexc(GOLD),
        );
    }
    p.ground_occlusion(0.62, 2.2);
    // (Pale paving goes on after the ground shading, or it reads as mud.)
    apron(&mut p, (-9.6, 9.6), (zf, 9.7));
    p.finish()
}

// -------------------------------------------------------------------------- Trading Union

/// The scales the guilds weigh by: a brass beam, two pans on chains and a pointer.
fn great_scales(p: &mut Parts, at: Vec3, span: f32, col: Vec4) {
    p.beam(
        Mat::Copper,
        at + v3(-span, 0.0, 0.0),
        at + v3(span, 0.0, 0.0),
        0.09,
        col,
    );
    p.beam(
        Mat::Copper,
        at + v3(0.0, -0.1, 0.0),
        at + v3(0.0, 0.45, 0.0),
        0.08,
        col,
    );
    ball(p, Mat::Copper, at + v3(0.0, 0.5, 0.0), 0.1, col);
    for s in [-1.0f32, 1.0] {
        let end = at + v3(s * span, 0.0, 0.0);
        let pan = end + v3(0.0, -span * 0.75, 0.0);
        for (dx, dz) in [(-0.3, 0.0), (0.3, 0.0), (0.0, 0.3)] {
            p.beam(
                Mat::Iron,
                end,
                pan + v3(dx * span, 0.0, dz * span),
                0.025,
                Vec4::ONE,
            );
        }
        p.cylinder(
            Mat::Copper,
            pan + v3(0.0, -0.08, 0.0),
            span * 0.22,
            span * 0.36,
            0.08,
            12,
            col,
            true,
        );
    }
}

/// A weighhouse: a brick hall with stepped gables to the street, a cart door under a hoist,
/// the guilds' banners between its windows, a boarded warehouse wing and the public scales
/// in the yard.
fn trading_union() -> Parts {
    let mut p = Parts::default();
    let ashlar = hexc(ASHLAR);
    let (l, w) = (11.0, 9.0);
    let c = v3(-1.6, 0.0, -2.6);
    let (x0, x1) = (c.x - l * 0.5, c.x + l * 0.5);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (floor, wall) = (0.5, 7.2);
    p.cuboid(
        Mat::Plaster,
        v3(x0 - 0.18, -3.0, zb - 0.18),
        v3(x1 + 0.18, floor + 0.5, zf + 0.18),
        shade(ashlar, 0.9),
        false,
    );
    p.cuboid(
        Mat::Brick,
        v3(x0, floor + 0.5, zb),
        v3(x1, wall, zf),
        Vec4::ONE,
        false,
    );
    quoins(&mut p, (x0, x1), (zb, zf), (floor + 0.5, wall), ashlar);
    p.cuboid(
        Mat::Plaster,
        v3(x0 - 0.1, 3.95, zb - 0.1),
        v3(x1 + 0.1, 4.2, zf + 0.1),
        ashlar,
        true,
    );
    // The roof, its ridge running back from the street.
    let rh = 4.6;
    p.gable_roof(
        Mat::RoofTile,
        Mat::Brick,
        v3(c.x, wall, c.z),
        w,
        l,
        rh,
        0.0,
        true,
        Vec4::ONE,
        Vec4::ONE,
    );
    // Stepped gables front and back, each step capped in stone.
    let n = 5;
    let sh = rh / n as f32;
    for (z, dz) in [(zf, 0.32f32), (zb, -0.32)] {
        let (za, zc) = (z.min(z + dz), z.max(z + dz));
        for k in 0..n {
            let half = l * 0.5 * (1.0 - k as f32 / n as f32) + 0.3;
            let y = wall + k as f32 * sh;
            p.cuboid(
                Mat::Brick,
                v3(c.x - half, y - 0.02, za),
                v3(c.x + half, y + sh + 0.2, zc),
                Vec4::ONE,
                false,
            );
            let inner = l * 0.5 * (1.0 - (k + 1) as f32 / n as f32) + 0.3;
            for s in [-1.0f32, 1.0] {
                let (a, b) = (c.x + s * inner, c.x + s * (half + 0.06));
                p.cuboid(
                    Mat::Plaster,
                    v3(a.min(b), y + sh + 0.2, za - 0.05),
                    v3(a.max(b), y + sh + 0.36, zc + 0.05),
                    ashlar,
                    true,
                );
            }
        }
        // The crowning block and its finial.
        let y = wall + rh + 0.2;
        p.cuboid(
            Mat::Brick,
            v3(c.x - 0.6, y, za),
            v3(c.x + 0.6, y + 1.0, zc),
            Vec4::ONE,
            false,
        );
        p.cuboid(
            Mat::Plaster,
            v3(c.x - 0.72, y + 1.0, za - 0.06),
            v3(c.x + 0.72, y + 1.18, zc + 0.06),
            ashlar,
            true,
        );
        ball(
            &mut p,
            Mat::Copper,
            v3(c.x, y + 1.42, (za + zc) * 0.5),
            0.24,
            hexc(GOLD),
        );
    }
    let front = zf + 0.32;
    // The cart door, wide enough for a wagon, under a stone arch.
    p.door(v3(c.x, floor, zf), Vec3::Z, 2.8, 3.1, hexc(0x3a2a1c));
    disc(
        &mut p,
        Mat::Planks,
        v3(c.x, floor + 3.1, zf + 0.05),
        Vec3::Z,
        1.4,
        hexc(0x3a2a1c),
    );
    for k in 0..7 {
        let a = (k as f32 + 0.5) / 7.0 * std::f32::consts::PI;
        let d = v3(a.cos(), a.sin(), 0.0);
        p.box_at(
            Mat::Plaster,
            v3(c.x, floor + 3.1, zf + 0.08) + d * 1.62,
            v3(0.3, 0.22, 0.1),
            0.0,
            if k == 3 { hexc(GOLD) } else { ashlar },
        );
    }
    p.cuboid(
        Mat::Cobble,
        v3(c.x - 1.9, -0.4, zf),
        v3(c.x + 1.9, floor, zf + 1.6),
        hexc(0xd8d2c4),
        false,
    );
    // Windows: counting-house below, the guild hall's tall ones above.
    for dx in [-4.1, -2.6, 2.6, 4.1] {
        p.windows(
            v3(c.x + dx, 2.5, zf + 0.02),
            Vec3::Z,
            1,
            0.95,
            1.7,
            0.0,
            WHITE,
        );
    }
    for dx in [-4.2, -1.4, 1.4, 4.2] {
        p.windows(
            v3(c.x + dx, 5.6, zf + 0.02),
            Vec3::Z,
            1,
            0.95,
            2.2,
            0.0,
            WHITE,
        );
        p.cuboid(
            Mat::Plaster,
            v3(c.x + dx - 0.62, 6.75, zf),
            v3(c.x + dx + 0.62, 6.95, zf + 0.1),
            ashlar,
            true,
        );
    }
    for s in [-1.0f32, 1.0] {
        if s < 0.0 {
            for (y, h) in [(2.5, 1.7), (5.6, 2.2)] {
                p.windows(
                    v3(c.x + s * (l * 0.5 + 0.02), y, c.z),
                    v3(s, 0.0, 0.0),
                    4,
                    0.95,
                    h,
                    1.1,
                    WHITE,
                );
            }
        } else {
            p.windows(
                v3(c.x + s * (l * 0.5 + 0.02), 5.9, c.z),
                v3(s, 0.0, 0.0),
                4,
                0.95,
                1.7,
                1.1,
                WHITE,
            );
        }
    }
    p.windows(
        v3(c.x, 5.6, zb - 0.02),
        Vec3::NEG_Z,
        4,
        0.95,
        2.2,
        1.4,
        WHITE,
    );
    // The guilds' banners between the upper windows.
    for (dx, col, mark) in [
        (-2.8, hexc(0xc8961e), hexc(0x3a2a1c)),
        (0.0, hexc(0x2d5578), hexc(0xe8e2d0)),
        (2.8, hexc(RED), hexc(GOLD)),
    ] {
        banner(&mut p, v3(c.x + dx, 6.9, zf), 0.8, 2.4, col, mark);
    }
    // In the gable: the loft door, the hoist over it and a bale on the rope.
    p.door(
        v3(c.x, wall + 0.5, front),
        Vec3::Z,
        1.2,
        1.7,
        hexc(0x24402e),
    );
    let hook = v3(c.x, wall + 3.1, front);
    p.beam(Mat::Timber, hook, hook + v3(0.0, 0.0, 1.9), 0.22, Vec4::ONE);
    p.beam(
        Mat::Timber,
        hook + v3(0.0, -0.9, 0.0),
        hook + v3(0.0, -0.05, 1.2),
        0.14,
        Vec4::ONE,
    );
    p.rod(
        Mat::Iron,
        hook + v3(-0.14, -0.2, 1.65),
        hook + v3(0.14, -0.2, 1.65),
        0.2,
        10,
        Vec4::ONE,
    );
    let rope = hexc(0xb89c6a);
    p.beam(
        Mat::Canvas,
        hook + v3(0.0, -0.3, 1.65),
        hook + v3(0.0, -4.3, 1.65),
        0.045,
        rope,
    );
    p.cuboid(
        Mat::Canvas,
        hook + v3(-0.45, -5.1, 1.3),
        hook + v3(0.45, -4.3, 2.0),
        hexc(0xcbb890),
        true,
    );
    for dx in [-0.25, 0.25] {
        p.beam(
            Mat::Canvas,
            hook + v3(dx, -5.12, 1.28),
            hook + v3(dx, -4.28, 1.28),
            0.05,
            rope,
        );
    }
    // The union's sign: gilded scales on the crowning gable.
    great_scales(&mut p, v3(c.x, wall + 4.1, front + 0.14), 0.62, hexc(GOLD));
    p.cuboid(
        Mat::Paint,
        v3(c.x - 1.5, wall + 2.35, front),
        v3(c.x + 1.5, wall + 2.75, front + 0.08),
        hexc(0x24402e),
        true,
    );
    for k in 0..5 {
        disc(
            &mut p,
            Mat::Copper,
            v3(c.x - 1.1 + k as f32 * 0.55, wall + 2.55, front + 0.1),
            Vec3::Z,
            0.12,
            hexc(GOLD),
        );
    }
    chimney(
        &mut p,
        Mat::Brick,
        v3(x0 + 0.9, wall + 0.4, c.z - 1.5),
        wall + 4.4,
        Vec4::ONE,
    );
    // The warehouse wing: tarred boards under slate, a wide door and a loading stage.
    let (wx0, wx1) = (x1, x1 + 5.6);
    let (wzf, wzb) = (zf - 1.6, zb + 0.6);
    let wwall = 4.4;
    let tar = hexc(0x4a3c30);
    p.cuboid(
        Mat::Plaster,
        v3(wx0, -3.0, wzb - 0.1),
        v3(wx1 + 0.1, floor + 0.3, wzf + 0.1),
        shade(ashlar, 0.88),
        false,
    );
    p.cuboid(
        Mat::Planks,
        v3(wx0, floor + 0.3, wzb),
        v3(wx1, wwall, wzf),
        tar,
        false,
    );
    p.gable_roof(
        Mat::Slate,
        Mat::Planks,
        v3((wx0 + wx1) * 0.5, wwall, (wzf + wzb) * 0.5),
        wzf - wzb,
        wx1 - wx0,
        2.3,
        0.35,
        true,
        Vec4::ONE,
        tar,
    );
    p.door(
        v3((wx0 + wx1) * 0.5, floor + 0.3, wzf),
        Vec3::Z,
        2.6,
        2.9,
        hexc(0x6e4a2a),
    );
    p.windows(
        v3((wx0 + wx1) * 0.5, wwall + 0.9, wzf + 0.02),
        Vec3::Z,
        1,
        0.8,
        0.8,
        0.0,
        WHITE,
    );
    p.windows(
        v3(wx1 + 0.02, 2.8, (wzf + wzb) * 0.5),
        Vec3::X,
        3,
        0.9,
        1.2,
        1.0,
        WHITE,
    );
    p.cuboid(
        Mat::Planks,
        v3(wx0 + 0.4, -0.4, wzf),
        v3(wx1 - 0.4, floor + 0.3, wzf + 1.5),
        hexc(0x8a6a46),
        false,
    );
    // The public scales in the yard: a gallows frame, the beam, pans with weights and sacks.
    let g = v3(-4.6, 0.1, 5.3);
    let oak = hexc(0x6e5234);
    for dx in [-1.5, 1.5] {
        p.beam(
            Mat::Timber,
            g + v3(dx, 0.0, 0.0),
            g + v3(dx, 3.3, 0.0),
            0.24,
            oak,
        );
        p.beam(
            Mat::Timber,
            g + v3(dx, 2.5, 0.0),
            g + v3(dx * 0.55, 3.3, 0.0),
            0.14,
            oak,
        );
        p.cuboid(
            Mat::Plaster,
            g + v3(dx - 0.3, -0.5, -0.3),
            g + v3(dx + 0.3, 0.25, 0.3),
            ashlar,
            false,
        );
    }
    p.beam(
        Mat::Timber,
        g + v3(-1.8, 3.3, 0.0),
        g + v3(1.8, 3.3, 0.0),
        0.26,
        oak,
    );
    p.beam(
        Mat::Iron,
        g + v3(0.0, 3.2, 0.0),
        g + v3(0.0, 2.75, 0.0),
        0.05,
        Vec4::ONE,
    );
    great_scales(&mut p, g + v3(0.0, 2.3, 0.0), 1.05, hexc(0xc8a24a));
    // Weights on one pan, a sack on the other.
    let pan = g + v3(0.0, 2.3 - 1.05 * 0.75, 0.0);
    for (dx, r, h) in [(-1.15, 0.13, 0.26), (-0.92, 0.1, 0.2), (-1.02, 0.07, 0.34)] {
        p.cylinder(
            Mat::Iron,
            pan + v3(dx, 0.0, if h > 0.3 { 0.14 } else { -0.05 }),
            r,
            r * 0.8,
            h,
            8,
            Vec4::ONE,
            true,
        );
    }
    p.cylinder(
        Mat::Canvas,
        pan + v3(1.05, 0.0, 0.0),
        0.26,
        0.2,
        0.5,
        8,
        hexc(0xcbb890),
        true,
    );
    p.ground_occlusion(0.62, 2.0);
    // (Pale paving goes on after the ground shading, or it reads as mud.)
    apron(&mut p, (-9.6, 9.6), (zf, 7.7));
    p.finish()
}

// ------------------------------------------------------------------------- Harbour Office

/// A white harbour office with navy doors, a lookout tower with a glazed lantern, and over
/// it the signal mast: a yard hung with flags and the red time ball that drops at noon.
fn harbour_office() -> Parts {
    let mut p = Parts::default();
    let ashlar = hexc(ASHLAR);
    let white = hexc(0xf0ece2);
    let navy = hexc(NAVY);
    let (l, w) = (8.6, 7.0);
    let c = v3(-1.7, 0.0, -2.5);
    let (x0, x1) = (c.x - l * 0.5, c.x + l * 0.5);
    let (zf, zb) = (c.z + w * 0.5, c.z - w * 0.5);
    let (floor, mid, wall) = (0.5, 3.6, 6.5);
    p.cuboid(
        Mat::Plaster,
        v3(x0 - 0.15, -3.0, zb - 0.15),
        v3(x1 + 0.15, floor + 0.4, zf + 0.15),
        shade(ashlar, 0.85),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(x0, floor + 0.4, zb),
        v3(x1, wall, zf),
        white,
        false,
    );
    p.cuboid(
        Mat::Paint,
        v3(x0 - 0.06, mid - 0.1, zb - 0.06),
        v3(x1 + 0.06, mid + 0.12, zf + 0.06),
        navy,
        true,
    );
    p.cuboid(
        Mat::Plaster,
        v3(x0 - 0.18, wall - 0.25, zb - 0.18),
        v3(x1 + 0.18, wall, zf + 0.18),
        ashlar,
        true,
    );
    p.hip_roof(Mat::Slate, v3(c.x, wall, c.z), l, w, 2.5, 0.4, Vec4::ONE);
    // Door, windows and navy shutters.
    p.door(v3(c.x, floor, zf), Vec3::Z, 1.4, 2.5, navy);
    steps(&mut p, c.x, zf + 0.02, 2.2, 2, floor, shade(ashlar, 0.95));
    let shutters = |p: &mut Parts, at: Vec3, n: Vec3, ww: f32, h: f32| {
        let along = Vec3::Y.cross(n).normalize();
        for s in [-1.0f32, 1.0] {
            let m = at + along * s * (ww * 0.5 + 0.3) + n * 0.04;
            let hw = along * 0.24;
            face(
                p,
                Mat::Paint,
                [
                    m - hw - Vec3::Y * h * 0.5,
                    m + hw - Vec3::Y * h * 0.5,
                    m + hw + Vec3::Y * h * 0.5,
                    m - hw + Vec3::Y * h * 0.5,
                ],
                navy,
                n,
            );
        }
    };
    for dx in [-2.9, 2.9] {
        p.windows(
            v3(c.x + dx, 2.2, zf + 0.02),
            Vec3::Z,
            1,
            0.95,
            1.5,
            0.0,
            WHITE,
        );
        shutters(&mut p, v3(c.x + dx, 2.2, zf), Vec3::Z, 0.95, 1.5);
    }
    for dx in [-2.9, 0.0, 2.9] {
        p.windows(
            v3(c.x + dx, 5.0, zf + 0.02),
            Vec3::Z,
            1,
            0.95,
            1.5,
            0.0,
            WHITE,
        );
        shutters(&mut p, v3(c.x + dx, 5.0, zf), Vec3::Z, 0.95, 1.5);
    }
    for y in [2.2, 5.0] {
        p.windows(v3(x0 - 0.02, y, c.z), Vec3::NEG_X, 3, 0.95, 1.5, 1.0, WHITE);
        p.windows(v3(c.x, y, zb - 0.02), Vec3::NEG_Z, 4, 0.95, 1.5, 0.9, WHITE);
    }
    // A porch roof on brackets, with the office's brass plate.
    p.cuboid(
        Mat::Slate,
        v3(c.x - 1.4, floor + 2.75, zf),
        v3(c.x + 1.4, floor + 2.9, zf + 1.1),
        Vec4::ONE,
        true,
    );
    for dx in [-1.2, 1.2] {
        p.beam(
            Mat::Paint,
            v3(c.x + dx, floor + 2.0, zf),
            v3(c.x + dx, floor + 2.75, zf + 0.9),
            0.1,
            WHITE,
        );
    }
    p.cuboid(
        Mat::Copper,
        v3(c.x + 0.95, 1.7, zf),
        v3(c.x + 1.45, 2.05, zf + 0.05),
        hexc(0xc8a24a),
        true,
    );
    chimney(
        &mut p,
        Mat::Brick,
        v3(x0 + 1.2, wall + 0.6, c.z),
        wall + 3.6,
        Vec4::ONE,
    );
    // The lookout tower at the seaward corner.
    let (tw, tc) = (3.4, v3(x1 + 1.2, 0.0, zf - 1.9));
    let (tx0, tx1, tz0, tz1) = (
        tc.x - tw * 0.5,
        tc.x + tw * 0.5,
        tc.z - tw * 0.5,
        tc.z + tw * 0.5,
    );
    let deck = 11.4;
    p.cuboid(
        Mat::Plaster,
        v3(tx0 - 0.12, -3.0, tz0 - 0.12),
        v3(tx1 + 0.12, floor + 0.4, tz1 + 0.12),
        shade(ashlar, 0.85),
        false,
    );
    p.cuboid(
        Mat::Plaster,
        v3(tx0, floor + 0.4, tz0),
        v3(tx1, deck, tz1),
        white,
        false,
    );
    quoins(
        &mut p,
        (tx0, tx1),
        (tz0, tz1),
        (floor + 0.4, deck - 0.2),
        ashlar,
    );
    for (y, h) in [(2.4, 1.5), (5.2, 1.5), (8.4, 1.7)] {
        p.windows(v3(tc.x, y, tz1 + 0.02), Vec3::Z, 1, 0.9, h, 0.0, WHITE);
        p.windows(v3(tx1 + 0.02, y, tc.z), Vec3::X, 1, 0.9, h, 0.0, WHITE);
    }
    // A clock toward the quay: ships set their chronometers by it.
    clock(&mut p, v3(tc.x, 10.1, tz1 + 0.02), Vec3::Z, 0.62);
    // The gallery: a deck on brackets with an iron rail.
    p.cuboid(
        Mat::Plaster,
        v3(tx0 - 0.8, deck, tz0 - 0.8),
        v3(tx1 + 0.8, deck + 0.22, tz1 + 0.8),
        ashlar,
        true,
    );
    for (dx, dz) in [(-1.0, 1.0), (1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
        let corner = v3(tc.x + dx * tw * 0.5, deck, tc.z + dz * tw * 0.5);
        p.beam(
            Mat::Plaster,
            corner + v3(0.0, -0.9, 0.0),
            corner + v3(dx * 0.7, 0.0, dz * 0.7),
            0.2,
            ashlar,
        );
    }
    let rail = hexc(0x26262a);
    let gy = deck + 0.22;
    let (g0, g1, h0, h1) = (tx0 - 0.7, tx1 + 0.7, tz0 - 0.7, tz1 + 0.7);
    for (a, b) in [
        (v3(g0, gy, h1), v3(g1, gy, h1)),
        (v3(g0, gy, h0), v3(g1, gy, h0)),
        (v3(g0, gy, h0), v3(g0, gy, h1)),
        (v3(g1, gy, h0), v3(g1, gy, h1)),
    ] {
        let n = ((b - a).length() / 0.45).round() as usize;
        for i in 0..=n {
            let at = a + (b - a) * (i as f32 / n as f32);
            p.beam(Mat::Iron, at, at + Vec3::Y * 1.0, 0.04, rail);
        }
        p.beam(Mat::Iron, a + Vec3::Y * 1.0, b + Vec3::Y * 1.0, 0.07, rail);
        p.beam(Mat::Iron, a + Vec3::Y * 0.5, b + Vec3::Y * 0.5, 0.04, rail);
    }
    // A brass telescope on a tripod at the rail.
    let t = v3(tc.x - 0.6, gy, h1 - 0.45);
    for (dx, dz) in [(-0.25, 0.2), (0.25, 0.2), (0.0, -0.28)] {
        p.beam(
            Mat::Timber,
            t + v3(dx, 0.0, dz),
            t + v3(0.0, 1.15, 0.0),
            0.045,
            Vec4::ONE,
        );
    }
    p.rod(
        Mat::Copper,
        t + v3(0.0, 1.1, -0.45),
        t + v3(0.0, 1.32, 0.6),
        0.06,
        8,
        hexc(0xc8a24a),
    );
    // The lantern room: glazed all round under a copper cap.
    let lan = v3(tc.x, gy, tc.z);
    p.cylinder(Mat::Paint, lan, 1.25, 1.25, 0.7, 8, white, false);
    p.cylinder(
        Mat::Window,
        lan + v3(0.0, 0.7, 0.0),
        1.2,
        1.2,
        1.5,
        8,
        Vec4::ONE,
        false,
    );
    for i in 0..8 {
        let a = i as f32 / 8.0 * TAU;
        let d = v3(a.cos(), 0.0, a.sin()) * 1.22;
        p.beam(
            Mat::Paint,
            lan + d + v3(0.0, 0.7, 0.0),
            lan + d + v3(0.0, 2.2, 0.0),
            0.1,
            WHITE,
        );
    }
    p.cylinder(
        Mat::Paint,
        lan + v3(0.0, 2.2, 0.0),
        1.4,
        1.4,
        0.18,
        8,
        WHITE,
        true,
    );
    let verdigris = hexc(0x6fae9a);
    p.cylinder(
        Mat::Copper,
        lan + v3(0.0, 2.38, 0.0),
        1.45,
        0.2,
        1.3,
        8,
        verdigris,
        true,
    );
    // The signal mast: topmast, yard, gaff, shrouds.
    let foot = lan + v3(0.0, 3.6, 0.0);
    let mast_h = 8.2;
    p.cylinder(Mat::Paint, foot, 0.13, 0.07, mast_h, 8, WHITE, true);
    ball(
        &mut p,
        Mat::Copper,
        foot + v3(0.0, mast_h + 0.12, 0.0),
        0.13,
        hexc(GOLD),
    );
    let yard = foot + v3(0.0, 4.6, 0.0);
    p.rod(
        Mat::Paint,
        yard + v3(-2.6, 0.0, 0.0),
        yard + v3(2.6, 0.0, 0.0),
        0.06,
        8,
        WHITE,
    );
    for s in [-1.0f32, 1.0] {
        p.beam(
            Mat::Iron,
            yard + v3(s * 2.5, 0.0, 0.0),
            foot + v3(0.0, mast_h - 0.5, 0.0),
            0.02,
            Vec4::ONE,
        );
        // Halyards down to the gallery rail, hung with signal flags.
        let (top, bot) = (
            yard + v3(s * 2.3, 0.0, 0.0),
            v3(tc.x + s * (tw * 0.5 + 0.7), gy + 1.0, tc.z),
        );
        p.beam(Mat::Canvas, top, bot, 0.025, hexc(0xb89c6a));
        let cols: [(u32, u32); 4] = [
            (0xc23a2e, 0xf2eee2),
            (0xe8c22e, 0x24476b),
            (0x24476b, 0xf2eee2),
            (0xf2eee2, 0xc23a2e),
        ];
        for (k, (a, b)) in cols.iter().enumerate() {
            let at = top + (bot - top) * (0.1 + k as f32 * 0.17);
            let (fw, fh) = (0.62, 0.46);
            let q = |dx: f32, dy: f32| at + v3(0.0, dy, dx * -s.signum());
            sheet(
                &mut p,
                Mat::Canvas,
                q(0.0, -fh),
                q(fw, -fh),
                q(fw, 0.0),
                q(0.0, 0.0),
                hexc(*a),
            );
            for side in [-1.0f32, 1.0] {
                let off = v3(0.012 * side, 0.0, 0.0);
                face(
                    &mut p,
                    Mat::Paint,
                    [
                        q(fw * 0.3, -fh * 0.7) + off,
                        q(fw * 0.7, -fh * 0.7) + off,
                        q(fw * 0.7, -fh * 0.3) + off,
                        q(fw * 0.3, -fh * 0.3) + off,
                    ],
                    hexc(*b),
                    v3(side, 0.0, 0.0),
                );
            }
        }
    }
    // The time ball, hoisted halfway, and the cross-trees it drops to.
    ball(
        &mut p,
        Mat::Paint,
        foot + v3(0.0, 6.4, 0.0),
        0.55,
        hexc(0xc23a2e),
    );
    p.cylinder(
        Mat::Iron,
        foot + v3(0.0, 5.0, 0.0),
        0.3,
        0.3,
        0.08,
        10,
        Vec4::ONE,
        true,
    );
    // The company's flag on a gaff.
    let gaff = foot + v3(0.0, 2.6, 0.0);
    p.beam(Mat::Paint, gaff, gaff + v3(0.0, 1.5, -2.0), 0.07, WHITE);
    let (fa, fb) = (gaff + v3(0.0, 1.45, -1.95), gaff + v3(0.0, 0.4, -0.5));
    sheet(
        &mut p,
        Mat::Canvas,
        fb + v3(0.0, -0.9, 0.0),
        fa + v3(0.0, -1.2, 0.0),
        fa,
        fb,
        hexc(RED),
    );
    // The yard: an anchor against the wall, the noon gun, bollards and the tide board.
    let a = v3(x0 + 1.6, 0.1, zf + 0.5);
    let iron = hexc(0x2a2c30);
    p.beam(Mat::Iron, a, a + v3(0.25, 2.5, -0.35), 0.16, iron);
    p.beam(
        Mat::Timber,
        a + v3(-0.75, 2.2, -0.3),
        a + v3(1.2, 2.25, -0.32),
        0.16,
        Vec4::ONE,
    );
    for s in [-1.0f32, 1.0] {
        p.beam(Mat::Iron, a, a + v3(s * 0.85, 0.55, -0.05), 0.15, iron);
        p.beam(
            Mat::Iron,
            a + v3(s * 0.85, 0.55, -0.05),
            a + v3(s * 0.95, 0.95, -0.1),
            0.26,
            iron,
        );
    }
    p.cylinder(
        Mat::Iron,
        a + v3(0.25, 2.5, -0.35),
        0.2,
        0.2,
        0.08,
        10,
        iron,
        true,
    );
    // The noon gun on its carriage.
    let gun = v3(3.2, 0.1, 5.6);
    let oak = hexc(0x6e5234);
    p.cuboid(
        Mat::Timber,
        gun + v3(-0.32, 0.22, -0.7),
        gun + v3(0.32, 0.5, 0.55),
        oak,
        true,
    );
    for (dz, r) in [(-0.45, 0.2), (0.35, 0.24)] {
        p.rod(
            Mat::Timber,
            gun + v3(-0.42, r, dz),
            gun + v3(0.42, r, dz),
            r,
            10,
            oak,
        );
    }
    p.rod(
        Mat::Iron,
        gun + v3(0.0, 0.62, -0.75),
        gun + v3(0.0, 0.8, 1.15),
        0.14,
        10,
        iron,
    );
    ball(
        &mut p,
        Mat::Copper,
        gun + v3(0.0, 0.6, -0.85),
        0.12,
        hexc(0xc8a24a),
    );
    for k in 0..3 {
        ball(
            &mut p,
            Mat::Iron,
            gun + v3(0.75, 0.1, -0.3 + k as f32 * 0.24),
            0.11,
            iron,
        );
    }
    // Bollards along the front of the apron, with a hawser between two.
    for k in 0..5 {
        let b = v3(-5.6 + k as f32 * 2.8, 0.1, 7.2);
        p.cylinder(Mat::Iron, b, 0.17, 0.14, 0.6, 10, iron, true);
        p.cylinder(
            Mat::Iron,
            b + v3(0.0, 0.6, 0.0),
            0.22,
            0.2,
            0.1,
            10,
            iron,
            true,
        );
    }
    // The tide board: white, ruled, on two posts.
    let tb = v3(-4.9, 0.1, 2.2);
    for dx in [-0.75, 0.75] {
        p.beam(
            Mat::Timber,
            tb + v3(dx, 0.0, 0.0),
            tb + v3(dx, 2.3, 0.0),
            0.12,
            navy,
        );
    }
    p.cuboid(
        Mat::Paint,
        tb + v3(-0.8, 1.0, -0.04),
        tb + v3(0.8, 2.2, 0.04),
        hexc(0xf2eee2),
        true,
    );
    for k in 0..5 {
        let y = 1.2 + k as f32 * 0.2;
        p.cuboid(
            Mat::Paint,
            tb + v3(-0.65, y, 0.04),
            tb + v3(0.65 - (k % 3) as f32 * 0.25, y + 0.05, 0.05),
            hexc(0x2a2a2e),
            true,
        );
    }
    // A coil of rope and the ship's bell by the door.
    p.cylinder(
        Mat::Canvas,
        v3(c.x + 3.0, 0.1, zf + 1.3),
        0.5,
        0.5,
        0.22,
        12,
        hexc(0xb89c6a),
        true,
    );
    let bell = v3(c.x - 1.3, 2.3, zf);
    p.beam(Mat::Iron, bell, bell + v3(0.0, 0.0, 0.5), 0.05, iron);
    p.cylinder(
        Mat::Copper,
        bell + v3(0.0, -0.42, 0.45),
        0.2,
        0.09,
        0.36,
        10,
        hexc(0xc8a24a),
        true,
    );
    p.ground_occlusion(0.62, 2.0);
    // (Pale paving goes on after the ground shading, or it reads as mud.)
    apron(&mut p, (-7.7, 7.7), (zf - 0.2, 7.7));
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_stand_on_their_lots() {
        for (key, kind) in [
            ("trading_union", Kind::TradingUnion),
            ("town_hall", Kind::TownHall),
            ("harbour_office", Kind::HarbourOffice),
        ] {
            let p = build(key).unwrap();
            let (w, d) = kind.def().size;
            let (hw, hd) = (w as f32 * 2.0 + 0.05, d as f32 * 2.0 + 0.05);
            let mut top = 0.0f32;
            for m in p.meshes.values() {
                for v in &m.vertices {
                    let [x, y, z] = v.position;
                    assert!(x.abs() <= hw && z.abs() <= hd, "{key}: ({x}, {y}, {z})");
                    top = top.max(y);
                }
            }
            // Landmarks: each stands well above the houses.
            assert!(top > 12.5, "{key} is {top} m high");
            assert!(p.triangles() < 40_000, "{key}");
        }
    }
}
