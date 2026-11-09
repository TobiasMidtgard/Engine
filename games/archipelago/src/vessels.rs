//! Ships and aircraft: hulls lofted from sections, rigs, funnels, airship envelopes.
//!
//! Every vessel's origin is at its waterline (airships: their envelope's axis) in the middle,
//! bow toward +X.

use std::f32::consts::{PI, TAU};

use engine::prelude::*;
use serde::{Deserialize, Serialize};

use crate::art::{Mat, Parts, hexc, shade, v3};

/// A kind of ship or aircraft.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Craft {
    /// Two-masted sailing cargo ship, 30 m.
    Schooner,
    /// Iron steam freighter, 56 m.
    Steamer,
    /// Small sailing fishing boat, 11 m.
    Smack,
    /// Three-masted barque of the free traders, 44 m.
    Barque,
    /// Rigid cargo airship, 96 m.
    Airship,
    /// Mail biplane, 9 m span.
    Biplane,
}

impl Craft {
    pub const ALL: [Craft; 6] = [
        Craft::Schooner,
        Craft::Steamer,
        Craft::Smack,
        Craft::Barque,
        Craft::Airship,
        Craft::Biplane,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Craft::Schooner => "Schooner",
            Craft::Steamer => "Steam freighter",
            Craft::Smack => "Fishing smack",
            Craft::Barque => "Trader's barque",
            Craft::Airship => "Cargo airship",
            Craft::Biplane => "Mail biplane",
        }
    }

    /// Cruising speed, m/s (time-lapsed: ships in the game sail about four times real speed).
    pub fn speed(self) -> f32 {
        match self {
            Craft::Schooner => 14.0,
            Craft::Steamer => 17.0,
            Craft::Smack => 8.0,
            Craft::Barque => 12.0,
            Craft::Airship => 30.0,
            Craft::Biplane => 55.0,
        }
    }

    /// Cargo slots and goods per slot.
    pub fn hold(self) -> (u32, u32) {
        match self {
            Craft::Schooner => (2, 30),
            Craft::Steamer => (4, 50),
            Craft::Smack => (0, 0),
            Craft::Barque => (3, 40),
            Craft::Airship => (2, 25),
            Craft::Biplane => (0, 0),
        }
    }

    /// Half length, meters.
    pub fn half_length(self) -> f32 {
        match self {
            Craft::Schooner => 15.0,
            Craft::Steamer => 28.0,
            Craft::Smack => 5.5,
            Craft::Barque => 22.0,
            Craft::Airship => 48.0,
            Craft::Biplane => 3.5,
        }
    }

    pub fn flies(self) -> bool {
        matches!(self, Craft::Airship | Craft::Biplane)
    }

    pub fn from_name(s: &str) -> Option<Craft> {
        Craft::ALL.into_iter().find(|c| {
            serde_json::to_value(c)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                == Some(s.to_ascii_lowercase())
        })
    }
}

/// A lofted hull: `len` long, `beam` wide, `draft` below and `free` above the waterline,
/// with a sheer that rises toward the ends. `bottom` paints below `boot` m (red lead),
/// `topside` above.
#[allow(clippy::too_many_arguments)]
fn hull(
    p: &mut Parts,
    mat: Mat,
    len: f32,
    beam: f32,
    draft: f32,
    free: f32,
    fine: f32,
    topside: Vec4,
    bottom: Vec4,
    boot: f32,
) {
    let n = 28;
    let k = 7;
    let mut rows: Vec<Vec<Vec3>> = Vec::new();
    for i in 0..=n {
        let t = i as f32 / n as f32; // 0 stern .. 1 bow
        let x = (t - 0.5) * len;
        // Plan shape: full amidships, a fine bow and a rounder stern.
        let s = if t > 0.5 {
            let u = (t - 0.5) * 2.0;
            1.0 - u.powf(fine)
        } else {
            let u = (0.5 - t) * 2.0;
            1.0 - u.powf(3.0) * 0.75
        };
        let half = beam * 0.5 * s.max(0.0).sqrt().max(0.02);
        let sheer = free + 0.5 * free * ((2.0 * t - 1.0).powi(2));
        let keel = -draft * (1.0 - (2.0 * t - 1.0).powi(6)).max(0.15);
        let mut row = Vec::new();
        for j in 0..=k {
            let a = j as f32 / k as f32 * PI * 0.5; // 0 at keel, 90° at the deck edge
            let y = keel + (sheer - keel) * (1.0 - a.cos());
            let z = half * a.sin().powf(0.6);
            row.push(v3(x, y, z));
        }
        rows.push(row);
    }
    let col = |y: f32| if y < boot { bottom } else { topside };
    for side in [1.0f32, -1.0] {
        for i in 0..n {
            for j in 0..k {
                let m = |q: Vec3| v3(q.x, q.y, q.z * side);
                let (a, b, c, d) = (
                    m(rows[i][j]),
                    m(rows[i + 1][j]),
                    m(rows[i + 1][j + 1]),
                    m(rows[i][j + 1]),
                );
                let cc = col((a.y + c.y) * 0.5);
                if side > 0.0 {
                    p.quad(mat, a, b, c, d, cc);
                } else {
                    p.quad(mat, b, a, d, c, cc);
                }
            }
        }
    }
    // Deck.
    for i in 0..n {
        let a = rows[i][k];
        let b = rows[i + 1][k];
        let deck = |q: Vec3, s: f32| v3(q.x, q.y - 0.25, q.z * s * 0.96);
        p.quad(
            Mat::Planks,
            deck(a, 1.0),
            deck(a, -1.0),
            deck(b, -1.0),
            deck(b, 1.0),
            hexc(0xc8a878),
        );
    }
    // Transom.
    let r = &rows[0];
    for j in 0..k {
        p.tri(
            mat,
            v3(r[0].x, r[j].y, 0.0),
            v3(r[j].x, r[j].y, r[j].z),
            v3(r[j + 1].x, r[j + 1].y, r[j + 1].z),
            topside,
        );
        p.tri(
            mat,
            v3(r[0].x, r[j].y, 0.0),
            v3(r[j + 1].x, r[j + 1].y, -r[j + 1].z),
            v3(r[j].x, r[j].y, -r[j].z),
            topside,
        );
    }
}

/// A gaff sail on a mast at `x`: luff from `y0` to `y1`, foot `foot` long aft.
fn gaff_sail(p: &mut Parts, mat: Mat, x: f32, y0: f32, y1: f32, foot: f32, belly: f32) {
    let a = v3(x, y0, 0.0);
    let b = v3(x - foot, y0 + 0.3, 0.0);
    let c = v3(x - foot * 0.95, y1 - 0.6, 0.0);
    let d = v3(x, y1, 0.0);
    // Two strips with a belly to leeward.
    let m1 = (a + d) * 0.5 + v3(-foot * 0.5, 0.0, belly);
    let m0 = (a + b) * 0.5 + v3(0.0, 0.0, belly * 0.6);
    let m2 = (d + c) * 0.5 + v3(0.0, 0.0, belly * 0.6);
    p.quad(mat, a, m0, m1, (a + d) * 0.5, Vec4::ONE);
    p.quad(
        mat,
        m0,
        b,
        (b + c) * 0.5 + v3(0.0, 0.0, belly * 0.5),
        m1,
        Vec4::ONE,
    );
    p.quad(mat, (a + d) * 0.5, m1, m2, d, Vec4::ONE);
    p.quad(
        mat,
        m1,
        (b + c) * 0.5 + v3(0.0, 0.0, belly * 0.5),
        c,
        m2,
        Vec4::ONE,
    );
    p.beam(Mat::Timber, a, b, 0.14, Vec4::ONE);
    p.beam(Mat::Timber, d, c, 0.12, Vec4::ONE);
}

fn jib(p: &mut Parts, mat: Mat, tack: Vec3, head: Vec3, clew: Vec3) {
    let mid = (tack + head + clew) / 3.0 + v3(0.0, 0.0, 0.5);
    p.tri(mat, tack, clew, mid, Vec4::ONE);
    p.tri(mat, clew, head, mid, Vec4::ONE);
    p.tri(mat, head, tack, mid, Vec4::ONE);
}

/// A square sail hung from a yard at `y` on a mast at `x`, `w` wide and `h` deep.
fn square_sail(p: &mut Parts, x: f32, y: f32, w: f32, h: f32) {
    let belly = 0.8;
    let rows = 3;
    for r in 0..rows {
        let y0 = y - h * r as f32 / rows as f32;
        let y1 = y - h * (r + 1) as f32 / rows as f32;
        let b0 = belly * (PI * r as f32 / rows as f32).sin();
        let b1 = belly * (PI * (r + 1) as f32 / rows as f32).sin();
        for s in [-1.0f32, 0.0] {
            let z0 = s * w * 0.5;
            let z1 = z0 + w * 0.5;
            let mid = |z: f32| if z.abs() < 0.01 { 1.0 } else { 0.4 };
            p.quad(
                Mat::Canvas,
                v3(x + b0 * mid(z1), y0, z1),
                v3(x + b0 * mid(z0), y0, z0),
                v3(x + b1 * mid(z0), y1, z0),
                v3(x + b1 * mid(z1), y1, z1),
                Vec4::ONE,
            );
        }
    }
    p.rod(
        Mat::Timber,
        v3(x, y, -w * 0.55),
        v3(x, y, w * 0.55),
        0.12,
        6,
        Vec4::ONE,
    );
}

fn mast(p: &mut Parts, x: f32, deck: f32, h: f32, r: f32) {
    p.cylinder(
        Mat::Timber,
        v3(x, deck, 0.0),
        r,
        r * 0.6,
        h,
        10,
        hexc(0xb08a5a),
        true,
    );
}

fn shrouds(p: &mut Parts, x: f32, deck: f32, h: f32, beam: f32) {
    for s in [1.0f32, -1.0] {
        for dx in [-0.6, 0.0, 0.6] {
            p.rod(
                Mat::Iron,
                v3(x + dx, deck, s * beam * 0.48),
                v3(x, deck + h * 0.88, 0.0),
                0.025,
                3,
                hexc(0x2a2420),
            );
        }
    }
}

pub fn schooner() -> Parts {
    let mut p = Parts::default();
    let (len, beam) = (30.0, 7.0);
    hull(
        &mut p,
        Mat::Paint,
        len,
        beam,
        2.6,
        1.6,
        2.2,
        hexc(0x1e3a32),
        hexc(0x6a2a1a),
        0.25,
    );
    // Bowsprit.
    p.rod(
        Mat::Timber,
        v3(13.0, 2.1, 0.0),
        v3(22.0, 3.2, 0.0),
        0.18,
        8,
        Vec4::ONE,
    );
    let deck = 1.5;
    mast(&mut p, 5.5, deck, 24.0, 0.32);
    mast(&mut p, -4.5, deck, 26.0, 0.34);
    shrouds(&mut p, 5.5, deck, 24.0, beam);
    shrouds(&mut p, -4.5, deck, 26.0, beam);
    gaff_sail(&mut p, Mat::Canvas, 5.3, deck + 2.0, deck + 19.0, 8.5, 1.2);
    gaff_sail(
        &mut p,
        Mat::Canvas,
        -4.7,
        deck + 2.0,
        deck + 21.0,
        10.5,
        1.4,
    );
    jib(
        &mut p,
        Mat::Canvas,
        v3(21.0, 3.1, 0.0),
        v3(5.6, deck + 22.0, 0.0),
        v3(9.5, deck + 2.6, 0.6),
    );
    jib(
        &mut p,
        Mat::Canvas,
        v3(15.0, 2.4, 0.0),
        v3(5.6, deck + 17.0, 0.0),
        v3(8.0, deck + 2.4, 0.5),
    );
    p.rod(
        Mat::Iron,
        v3(22.0, 3.2, 0.0),
        v3(5.5, deck + 23.5, 0.0),
        0.03,
        3,
        hexc(0x2a2420),
    );
    p.rod(
        Mat::Iron,
        v3(5.5, deck + 23.5, 0.0),
        v3(-4.5, deck + 25.5, 0.0),
        0.03,
        3,
        hexc(0x2a2420),
    );
    // Deckhouse, hatch, cargo.
    p.cuboid(
        Mat::Planks,
        v3(-12.0, deck - 0.2, -1.5),
        v3(-8.0, deck + 1.8, 1.5),
        hexc(0xd8d0b8),
        false,
    );
    p.cuboid(
        Mat::Timber,
        v3(-12.2, deck + 1.8, -1.7),
        v3(-7.8, deck + 2.0, 1.7),
        Vec4::ONE,
        true,
    );
    p.cuboid(
        Mat::Timber,
        v3(-1.5, deck - 0.2, -1.8),
        v3(2.5, deck + 0.6, 1.8),
        Vec4::ONE,
        true,
    );
    p.ground_occlusion(1.0, 1.0);
    p.finish()
}

pub fn steamer() -> Parts {
    let mut p = Parts::default();
    let (len, beam) = (56.0, 9.5);
    hull(
        &mut p,
        Mat::Paint,
        len,
        beam,
        3.6,
        2.6,
        3.0,
        hexc(0x141618),
        hexc(0x7a2418),
        0.4,
    );
    let deck = 2.4;
    // White boot line.
    p.cuboid(
        Mat::Paint,
        v3(-27.0, 0.38, -0.02),
        v3(27.0, 0.48, 0.02),
        Vec4::ONE,
        false,
    );
    // Bridge and engine house amidships.
    p.cuboid(
        Mat::Paint,
        v3(-6.0, deck, -3.6),
        v3(4.0, deck + 3.2, 3.6),
        hexc(0xe8e4d8),
        false,
    );
    p.cuboid(
        Mat::Paint,
        v3(-2.5, deck + 3.2, -3.0),
        v3(3.5, deck + 5.8, 3.0),
        hexc(0xe8e4d8),
        false,
    );
    p.cuboid(
        Mat::Timber,
        v3(-2.8, deck + 5.8, -3.6),
        v3(3.8, deck + 6.1, 3.6),
        Vec4::ONE,
        true,
    );
    for (n, z) in [(Vec3::Z, 3.0), (-Vec3::Z, -3.0)] {
        p.windows(v3(0.5, deck + 4.6, z), n, 4, 0.8, 0.9, 0.5, hexc(0x8a6a4a));
    }
    p.windows(
        v3(3.5, deck + 4.6, 0.0),
        Vec3::X,
        4,
        0.8,
        0.9,
        0.5,
        hexc(0x8a6a4a),
    );
    // Funnel with a company band.
    p.cylinder(
        Mat::Paint,
        v3(-4.5, deck + 3.2, 0.0),
        1.3,
        1.25,
        7.5,
        16,
        hexc(0x2a2a2a),
        false,
    );
    p.cylinder(
        Mat::Paint,
        v3(-4.5, deck + 8.0, 0.0),
        1.27,
        1.26,
        1.4,
        16,
        hexc(0xc8a030),
        false,
    );
    p.cylinder(
        Mat::Paint,
        v3(-4.5, deck + 10.7, 0.0),
        1.26,
        1.25,
        0.5,
        16,
        hexc(0x0a0a0a),
        false,
    );
    // Masts with derricks.
    for x in [16.0, -18.0] {
        p.cylinder(
            Mat::Iron,
            v3(x, deck, 0.0),
            0.3,
            0.2,
            16.0,
            10,
            hexc(0xb8a888),
            true,
        );
        p.rod(
            Mat::Iron,
            v3(x, deck + 1.5, 0.0),
            v3(x + if x > 0.0 { -8.0 } else { 8.0 }, deck + 6.0, 0.0),
            0.14,
            6,
            hexc(0xb8a888),
        );
        p.rod(
            Mat::Iron,
            v3(x, deck + 15.0, 0.0),
            v3(x + if x > 0.0 { -8.0 } else { 8.0 }, deck + 6.0, 0.0),
            0.03,
            3,
            hexc(0x2a2420),
        );
    }
    p.rod(
        Mat::Iron,
        v3(27.0, deck + 1.2, 0.0),
        v3(16.0, deck + 15.5, 0.0),
        0.03,
        3,
        hexc(0x2a2420),
    );
    p.rod(
        Mat::Iron,
        v3(16.0, deck + 15.5, 0.0),
        v3(-18.0, deck + 15.5, 0.0),
        0.03,
        3,
        hexc(0x2a2420),
    );
    // Hatches.
    for x in [10.0, 21.0, -12.0, -22.0] {
        p.cuboid(
            Mat::Timber,
            v3(x - 2.5, deck - 0.2, -2.6),
            v3(x + 2.5, deck + 0.8, 2.6),
            hexc(0x6a5a40),
            true,
        );
    }
    // Lifeboats.
    for z in [-3.9, 3.9] {
        p.add_parts(
            &crate::looks::small_boat(),
            Mat4::from_translation(v3(-1.0, deck + 3.4, z)),
        );
    }
    p.finish()
}

pub fn smack() -> Parts {
    let mut p = Parts::default();
    hull(
        &mut p,
        Mat::Planks,
        11.0,
        3.6,
        1.3,
        0.9,
        1.8,
        hexc(0x6a5a48),
        hexc(0x3a2a20),
        0.15,
    );
    let deck = 0.85;
    mast(&mut p, 2.0, deck, 10.0, 0.16);
    gaff_sail(&mut p, Mat::SailDark, 1.9, deck + 1.2, deck + 8.5, 5.5, 0.6);
    jib(
        &mut p,
        Mat::SailDark,
        v3(6.5, 1.2, 0.0),
        v3(2.0, deck + 9.0, 0.0),
        v3(3.2, deck + 1.4, 0.3),
    );
    p.rod(
        Mat::Timber,
        v3(5.0, 1.2, 0.0),
        v3(7.0, 1.4, 0.0),
        0.08,
        6,
        Vec4::ONE,
    );
    p.box_at(
        Mat::Planks,
        v3(-3.0, deck + 0.4, 0.0),
        v3(0.8, 0.4, 0.9),
        0.0,
        hexc(0xc8b890),
    );
    p.finish()
}

pub fn barque() -> Parts {
    let mut p = Parts::default();
    let (len, beam) = (44.0, 9.0);
    hull(
        &mut p,
        Mat::Paint,
        len,
        beam,
        3.4,
        2.2,
        2.4,
        hexc(0x2a2018),
        hexc(0x5a2014),
        0.3,
    );
    // Painted gunports band.
    p.cuboid(
        Mat::Paint,
        v3(-19.0, 1.4, -0.02),
        v3(19.0, 1.9, 0.02),
        hexc(0xe0d8b8),
        false,
    );
    let deck = 2.1;
    p.rod(
        Mat::Timber,
        v3(20.0, 2.8, 0.0),
        v3(31.0, 4.6, 0.0),
        0.24,
        8,
        Vec4::ONE,
    );
    for (x, h) in [(12.0, 30.0), (0.0, 33.0), (-12.0, 26.0)] {
        mast(&mut p, x, deck, h, 0.42);
        shrouds(&mut p, x, deck, h, beam);
        if x > -10.0 {
            square_sail(&mut p, x, deck + h * 0.42, 14.0, 7.0);
            square_sail(&mut p, x, deck + h * 0.66, 11.5, 6.0);
            square_sail(&mut p, x, deck + h * 0.86, 8.5, 4.8);
        } else {
            gaff_sail(
                &mut p,
                Mat::Canvas,
                x - 0.3,
                deck + 3.0,
                deck + h * 0.8,
                9.0,
                1.1,
            );
        }
    }
    jib(
        &mut p,
        Mat::Canvas,
        v3(30.0, 4.5, 0.0),
        v3(12.2, deck + 28.0, 0.0),
        v3(18.0, deck + 4.0, 0.6),
    );
    p.cuboid(
        Mat::Planks,
        v3(-21.0, deck - 0.2, -3.5),
        v3(-14.0, deck + 2.2, 3.5),
        hexc(0xd8c8a0),
        false,
    );
    p.finish()
}

/// Rigid airship: 96 m doped-linen envelope, four cruciform fins, a control car and four
/// engine cars. The propellers are a separate part ([`airship_props`]).
pub fn airship() -> Parts {
    let mut p = Parts::default();
    let len = 96.0;
    let r = 10.5;
    let n = 26;
    let prof: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let x = (t - 0.5) * len;
            // Rounded nose (+X), long taper aft.
            let s = if t > 0.62 {
                let u = (t - 0.62) / 0.38;
                (1.0 - u.powf(2.4)).max(0.0).sqrt()
            } else {
                let u = (0.62 - t) / 0.62;
                (1.0 - u.powf(1.8)).max(0.0).powf(0.7)
            };
            (x, r * s + 0.05)
        })
        .collect();
    // 16 sides: the girders show as gently faceted panels.
    p.lathe_x(Mat::Envelope, &prof, 16, |x, a| {
        let band = if (x - 30.0).abs() < 1.2 { 0.75 } else { 1.0 };
        let under = 0.9 + 0.1 * a.sin().abs();
        Vec4::new(band * under, band * under, band * under * 0.98, 1.0)
    });
    // Fins.
    for k in 0..4 {
        let a = k as f32 * PI * 0.5 + PI * 0.25 * 0.0;
        let d = v3(0.0, a.cos(), a.sin());
        let root0 = v3(-44.0, 0.0, 0.0) + d * 2.5;
        let root1 = v3(-30.0, 0.0, 0.0) + d * 6.5;
        let tip0 = v3(-46.5, 0.0, 0.0) + d * 10.5;
        let tip1 = v3(-38.0, 0.0, 0.0) + d * 10.0;
        let side = v3(0.0, -a.sin(), a.cos()) * 0.25;
        p.quad(
            Mat::Envelope,
            root0 + side,
            root1 + side,
            tip1 + side,
            tip0 + side,
            Vec4::splat(0.85).with_w(1.0),
        );
        p.quad(
            Mat::Envelope,
            root1 - side,
            root0 - side,
            tip0 - side,
            tip1 - side,
            Vec4::splat(0.85).with_w(1.0),
        );
        // Elevator / rudder in red.
        let aft = v3(-2.2, 0.0, 0.0);
        p.quad(
            Mat::Paint,
            root0 + side * 1.2 + aft,
            root0 + side * 1.2,
            tip0 + side * 1.2,
            tip0 + side * 1.2 + aft,
            hexc(0x8a1e1a),
        );
        p.quad(
            Mat::Paint,
            root0 - side * 1.2,
            root0 - side * 1.2 + aft,
            tip0 - side * 1.2 + aft,
            tip0 - side * 1.2,
            hexc(0x8a1e1a),
        );
    }
    // Control car under the nose.
    p.cuboid(
        Mat::Paint,
        v3(18.0, -r - 2.2, -1.6),
        v3(30.0, -r + 0.4, 1.6),
        hexc(0x3a3a3a),
        true,
    );
    p.windows(
        v3(24.0, -r - 1.0, 1.6),
        Vec3::Z,
        6,
        1.0,
        0.9,
        0.6,
        hexc(0x8a8a8a),
    );
    p.windows(
        v3(24.0, -r - 1.0, -1.6),
        -Vec3::Z,
        6,
        1.0,
        0.9,
        0.6,
        hexc(0x8a8a8a),
    );
    p.windows(
        v3(30.0, -r - 1.0, 0.0),
        Vec3::X,
        2,
        1.0,
        0.9,
        0.4,
        hexc(0x8a8a8a),
    );
    // Cargo gondola amidships.
    p.cuboid(
        Mat::Planks,
        v3(-8.0, -r - 3.0, -2.2),
        v3(8.0, -r + 0.6, 2.2),
        hexc(0x5a4a3a),
        true,
    );
    // Engine cars on struts.
    for (x, z) in [(-14.0, 7.5), (-14.0, -7.5), (5.0, 8.5), (5.0, -8.5)] {
        let y = -r * 0.65;
        let mut car = Parts::default();
        car.lathe_x(
            Mat::Paint,
            &[
                (-2.6, 0.05),
                (-2.0, 0.9),
                (1.2, 1.0),
                (2.2, 0.5),
                (2.5, 0.05),
            ],
            10,
            |_, _| hexc(0x4a4a48),
        );
        p.add_parts(&car, Mat4::from_translation(v3(x, y, z)));
        p.rod(
            Mat::Iron,
            v3(x, y + 0.5, z * 0.82),
            v3(x, y + 2.5, z * 0.6),
            0.1,
            4,
            Vec4::ONE,
        );
    }
    p.finish()
}

/// One airship propeller about its hub (turning about X).
pub fn airship_props() -> Parts {
    let mut p = Parts::default();
    for k in 0..4 {
        let a = k as f32 * TAU / 4.0;
        let d = v3(0.0, a.cos(), a.sin()) * 2.0;
        p.box_at(
            Mat::Timber,
            d * 0.5,
            v3(0.06, 0.12, 0.12) + d.abs() * 0.5,
            0.0,
            hexc(0x4a3424),
        );
    }
    p.cylinder(
        Mat::Iron,
        v3(0.0, -0.2, 0.0),
        0.2,
        0.2,
        0.4,
        8,
        Vec4::ONE,
        true,
    );
    p.finish()
}

pub fn biplane() -> Parts {
    let mut p = Parts::default();
    let body = hexc(0xb8a040);
    p.lathe_x(
        Mat::Paint,
        &[
            (-3.6, 0.05),
            (-3.0, 0.25),
            (0.5, 0.55),
            (1.6, 0.6),
            (2.2, 0.45),
            (2.4, 0.05),
        ],
        10,
        |_, _| body,
    );
    for y in [-0.45f32, 1.3] {
        p.cuboid(
            Mat::Canvas,
            v3(0.0, y, -4.5),
            v3(1.5, y + 0.12, 4.5),
            hexc(0xe8dcc0),
            true,
        );
    }
    for z in [-3.2f32, 3.2, -1.0, 1.0] {
        p.beam(
            Mat::Timber,
            v3(0.4, -0.4, z),
            v3(0.5, 1.3, z),
            0.06,
            Vec4::ONE,
        );
        p.beam(
            Mat::Timber,
            v3(1.2, -0.4, z),
            v3(1.1, 1.3, z),
            0.06,
            Vec4::ONE,
        );
    }
    p.cuboid(
        Mat::Canvas,
        v3(-3.7, 0.1, -1.6),
        v3(-2.8, 0.18, 1.6),
        hexc(0xe8dcc0),
        true,
    );
    p.quad(
        Mat::Paint,
        v3(-3.7, 0.15, 0.0),
        v3(-2.9, 0.15, 0.0),
        v3(-3.2, 1.2, 0.0),
        v3(-3.7, 1.25, 0.0),
        hexc(0x8a1e1a),
    );
    // Wheels.
    for z in [-0.8f32, 0.8] {
        p.beam(
            Mat::Iron,
            v3(1.0, -0.4, 0.0),
            v3(1.0, -1.2, z),
            0.05,
            Vec4::ONE,
        );
        p.cylinder(
            Mat::Iron,
            v3(1.0, -1.3, z - 0.06),
            0.35,
            0.35,
            0.12,
            10,
            hexc(0x1a1a1a),
            true,
        );
    }
    p.box_at(
        Mat::Timber,
        v3(2.5, 0.0, 0.0),
        v3(0.05, 1.2, 0.1),
        0.0,
        hexc(0x4a3424),
    );
    p.finish()
}

/// The model of a craft.
pub fn craft_model(c: Craft) -> Parts {
    match c {
        Craft::Schooner => schooner(),
        Craft::Steamer => steamer(),
        Craft::Smack => smack(),
        Craft::Barque => barque(),
        Craft::Airship => airship(),
        Craft::Biplane => biplane(),
    }
}

/// A two-wheeled horse cart with a load of crates (origin on the road, front +X).
pub fn cart() -> Parts {
    let mut p = Parts::default();
    let wood = hexc(0x7a5a3a);
    p.cuboid(
        Mat::Planks,
        v3(-1.3, 0.75, -0.7),
        v3(0.7, 0.85, 0.7),
        wood,
        true,
    );
    for z in [-0.7f32, 0.7] {
        p.cuboid(
            Mat::Planks,
            v3(-1.3, 0.85, z - 0.04),
            v3(0.7, 1.15, z + 0.04),
            wood,
            false,
        );
        let mut wheel = Parts::default();
        wheel.cylinder(
            Mat::Timber,
            v3(0.0, 0.0, -0.05),
            0.55,
            0.55,
            0.1,
            12,
            shade(wood, 0.6),
            true,
        );
        p.add_parts(
            &wheel,
            Mat4::from_rotation_translation(
                Quat::from_rotation_x(PI * 0.5),
                v3(-0.3, 0.55, z * 1.08),
            ),
        );
    }
    p.beam(
        Mat::Timber,
        v3(0.7, 0.8, -0.35),
        v3(2.8, 0.95, -0.35),
        0.07,
        wood,
    );
    p.beam(
        Mat::Timber,
        v3(0.7, 0.8, 0.35),
        v3(2.8, 0.95, 0.35),
        0.07,
        wood,
    );
    // A horse: a chunky body, neck and legs.
    let coat = hexc(0x4a3022);
    let mut body = Parts::default();
    body.lathe_x(
        Mat::Wool,
        &[(-0.8, 0.05), (-0.6, 0.35), (0.4, 0.38), (0.8, 0.05)],
        10,
        |_, _| coat,
    );
    p.add_parts(&body, Mat4::from_translation(v3(2.6, 1.25, 0.0)));
    p.beam(Mat::Wool, v3(3.2, 1.35, 0.0), v3(3.6, 2.0, 0.0), 0.28, coat);
    p.box_at(
        Mat::Wool,
        v3(3.75, 1.95, 0.0),
        v3(0.3, 0.15, 0.13),
        0.0,
        coat,
    );
    for (x, z) in [(2.1, -0.18), (2.1, 0.18), (3.1, -0.18), (3.1, 0.18)] {
        p.beam(
            Mat::Wool,
            v3(x, 0.0, z),
            v3(x, 1.0, z),
            0.12,
            shade(coat, 0.8),
        );
    }
    p.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crafts_build_within_budget() {
        for c in Craft::ALL {
            let t = craft_model(c).triangles();
            assert!(t > 50 && t < 60_000, "{c:?}: {t}");
        }
        assert!(cart().triangles() < 4_000);
        assert_eq!(Craft::from_name("airship"), Some(Craft::Airship));
    }
}
