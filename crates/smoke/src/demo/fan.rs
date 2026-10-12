//! The industrial drum fans along the side of the range: a rolled steel drum with domed wire
//! guards, a finned motor at the back, a yoke on a mast with splayed legs, and a separate
//! rotor of five twisted, cambered blades that spins up and down.
//!
//! Fan space: the hub is at the origin, the fan blows along −Z (the entity's forward), the
//! motor is at +Z, +Y is up.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Quat, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::kit::{self, at, tr};

/// Radius of the drum's inside, m.
pub const DRUM_R: f32 = 1.0;
/// Blade tip radius, m.
pub const TIP_R: f32 = 0.93;

/// Lathe around the fan axis (+Z).
fn zl(m: Mesh) -> Mesh {
    kit::placed(m, tr(Vec3::ZERO, Quat::from_rotation_x(FRAC_PI_2)))
}

/// Drum radius at axial position `a` (outlet at −0.24, back at +0.24): flared lips and a
/// stiffening bead in the middle.
fn drum_r(a: f32) -> f32 {
    let lip = |d: f32| (1.0 - (d / 0.05).clamp(0.0, 1.0)).powi(2) * 0.045;
    DRUM_R + lip(a + 0.24) + lip(0.24 - a) + (-(a / 0.018).powi(2)).exp() * 0.012
}

/// Axial position of a guard wire at radius r (domed out from the drum).
fn guard_a(r: f32, front: bool) -> f32 {
    let dome = 0.07 * (1.0 - (r / 1.06).powi(2)).max(0.0);
    if front {
        -0.255 - dome
    } else {
        0.255 + dome * 0.8
    }
}

/// The fan's fixed parts: drum, guards, motor, yoke and stand. `ground` is the floor's
/// height in fan space.
pub fn body(ground: f32) -> Parts {
    let mut p = Parts::default();
    // Drum shell: inner and outer skins 3 mm apart.
    let n = 40;
    let mut loop_pts = Vec::new();
    for i in 0..=n {
        let a = -0.24 + 0.48 * i as f32 / n as f32;
        loop_pts.push(Vec2::new(drum_r(a), a));
    }
    for i in (0..=n).rev() {
        let a = -0.24 + 0.48 * i as f32 / n as f32;
        loop_pts.push(Vec2::new(drum_r(a) + 0.003, a));
    }
    p.add(art::FAN, zl(kit::lathe_loop(&loop_pts, 96, 50.0)));
    // Rolled wire edges at both lips.
    for a in [-0.24f32, 0.24] {
        let ring = kit::ring(drum_r(a) + 0.006, 0.008, 96, 8);
        p.add(
            art::FAN,
            kit::placed(
                ring,
                tr(Vec3::new(0.0, 0.0, a), Quat::from_rotation_x(FRAC_PI_2)),
            ),
        );
    }
    // Guards: concentric wire rings and radial spokes, domed.
    for front in [true, false] {
        let mut k = 0;
        let mut r = 0.16;
        while r < 1.07 {
            let ring = kit::ring(r, 0.0042, ((r * 90.0) as u32).max(24), 6);
            p.add(
                art::GALV,
                kit::placed(
                    ring,
                    tr(
                        Vec3::new(0.0, 0.0, guard_a(r, front)),
                        Quat::from_rotation_x(FRAC_PI_2),
                    ),
                ),
            );
            k += 1;
            r = 0.16 + k as f32 * 0.1;
        }
        for s in 0..20 {
            let ang = s as f32 / 20.0 * TAU;
            let dir = Vec3::new(ang.cos(), ang.sin(), 0.0);
            let mut path: Vec<Vec3> = (0..=14)
                .map(|i| {
                    let r = 0.12 + (1.06 - 0.12) * i as f32 / 14.0;
                    dir * r + Vec3::Z * guard_a(r, front)
                })
                .collect();
            // Bend back onto the drum lip.
            let lip = if front { -0.24 } else { 0.24 };
            path.push(dir * 1.075 + Vec3::Z * lip);
            p.add(art::GALV, kit::tube(&path, 0.0048, 6, false));
        }
    }
    // Badge on the front guard.
    let badge = [
        Vec2::new(0.0, 0.0),
        Vec2::new(0.11, 0.0),
        Vec2::new(0.115, 0.006),
        Vec2::new(0.112, 0.014),
        Vec2::new(0.09, 0.018),
        Vec2::new(0.06, 0.024),
        Vec2::new(0.0, 0.026),
    ];
    p.add(
        art::FAN,
        kit::placed(
            kit::lathe(&badge, 32, 35.0, false),
            tr(
                Vec3::new(0.0, 0.0, guard_a(0.0, true) + 0.008),
                Quat::from_rotation_x(-FRAC_PI_2),
            ),
        ),
    );
    // Motor: finned body with an end bell, behind the back guard.
    let motor = [
        Vec2::new(0.0, 0.27),
        Vec2::new(0.06, 0.27),
        Vec2::new(0.07, 0.29),
        Vec2::new(0.15, 0.3),
        Vec2::new(0.163, 0.315),
        Vec2::new(0.163, 0.56),
        Vec2::new(0.155, 0.6),
        Vec2::new(0.12, 0.63),
        Vec2::new(0.06, 0.645),
        Vec2::new(0.0, 0.648),
    ];
    p.add(art::DARK, zl(kit::lathe(&motor, 40, 30.0, false)));
    for f in 0..28 {
        let ang = f as f32 / 28.0 * TAU;
        if (ang - FRAC_PI_2).abs() < 0.3 {
            continue; // terminal box
        }
        let fin = kit::rounded_box(Vec3::new(0.022, 0.005, 0.22), 0.002, 1, 0.3);
        let c = Vec3::new(ang.cos(), ang.sin(), 0.0) * 0.172 + Vec3::Z * 0.44;
        p.add(
            art::DARK,
            kit::placed(fin, tr(c, Quat::from_rotation_z(ang))),
        );
    }
    p.add(
        art::DARK,
        kit::placed(
            kit::rounded_box(Vec3::new(0.11, 0.08, 0.13), 0.01, 2, 0.1),
            at(Vec3::new(0.0, 0.2, 0.43)),
        ),
    );
    p.add(
        art::RUBBER,
        kit::placed(
            kit::lathe(
                &[
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.018, 0.0),
                    Vec2::new(0.018, 0.03),
                    Vec2::new(0.012, 0.045),
                    Vec2::new(0.0, 0.045),
                ],
                12,
                35.0,
                false,
            ),
            tr(Vec3::new(0.0, 0.2, 0.495), Quat::from_rotation_x(FRAC_PI_2)),
        ),
    );
    // Motor clamp band and three struts out to the drum.
    p.add(
        art::GALV,
        kit::placed(
            kit::ring(0.168, 0.009, 40, 6),
            tr(Vec3::new(0.0, 0.0, 0.45), Quat::from_rotation_x(FRAC_PI_2)),
        ),
    );
    for s in 0..3 {
        let ang = s as f32 / 3.0 * TAU + PI / 6.0 + PI;
        let dir = Vec3::new(ang.cos(), ang.sin(), 0.0);
        let a = dir * 0.17 + Vec3::Z * 0.45;
        let b = dir * (DRUM_R + 0.01) + Vec3::Z * 0.2;
        let d = b - a;
        let strut = kit::rounded_box(Vec3::new(0.045, d.length(), 0.008), 0.003, 1, 0.3);
        let rot = Quat::from_rotation_arc(Vec3::Y, d.normalize());
        p.add(art::GALV, kit::placed(strut, tr((a + b) * 0.5, rot)));
    }
    // Yoke: a bent channel from pivot to pivot under the drum.
    let yw = DRUM_R + 0.11;
    let drop = 1.12;
    let mut path = vec![Vec3::new(-yw, 0.0, 0.0), Vec3::new(-yw, -drop + 0.12, 0.0)];
    for i in 1..8 {
        let t = i as f32 / 8.0 * FRAC_PI_2;
        path.push(Vec3::new(
            -yw + 0.12 - 0.12 * t.cos(),
            -drop + 0.12 - 0.12 * t.sin(),
            0.0,
        ));
    }
    path.push(Vec3::new(-yw + 0.12, -drop, 0.0));
    path.push(Vec3::new(yw - 0.12, -drop, 0.0));
    for i in 1..8 {
        let t = i as f32 / 8.0 * FRAC_PI_2;
        path.push(Vec3::new(
            yw - 0.12 + 0.12 * t.sin(),
            -drop + 0.12 - 0.12 * t.cos(),
            0.0,
        ));
    }
    path.push(Vec3::new(yw, -drop + 0.12, 0.0));
    path.push(Vec3::new(yw, 0.0, 0.0));
    let section = kit::fillet(
        &[
            Vec2::new(-0.03, -0.018),
            Vec2::new(0.03, -0.018),
            Vec2::new(0.03, 0.018),
            Vec2::new(-0.03, 0.018),
        ],
        0.006,
        2,
    );
    p.add(art::DARK, kit::sweep(&path, &section, false, 40.0));
    // Pivot hubs and tension knobs.
    for side in [-1.0f32, 1.0] {
        let hub = kit::lathe(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.07, 0.0),
                Vec2::new(0.07, 0.03),
                Vec2::new(0.05, 0.05),
                Vec2::new(0.0, 0.05),
            ],
            24,
            35.0,
            false,
        );
        p.add(
            art::DARK,
            kit::placed(
                hub,
                tr(
                    Vec3::new(side * (DRUM_R + 0.04), 0.0, 0.0),
                    Quat::from_rotation_z(-side * FRAC_PI_2),
                ),
            ),
        );
        let knob = kit::lathe(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.02, 0.0),
                Vec2::new(0.02, 0.03),
                Vec2::new(0.055, 0.035),
                Vec2::new(0.058, 0.055),
                Vec2::new(0.045, 0.07),
                Vec2::new(0.0, 0.072),
            ],
            6,
            35.0,
            true,
        );
        p.add(
            art::RUBBER,
            kit::placed(
                knob,
                tr(
                    Vec3::new(side * (yw + 0.02), 0.0, 0.0),
                    Quat::from_rotation_z(-side * FRAC_PI_2),
                ),
            ),
        );
    }
    // Mast with a height clamp, and four splayed legs with rubber feet.
    let mast_top = -drop - 0.018;
    let mast_bottom = ground + 0.22;
    let mast = kit::rounded_box(Vec3::new(0.07, mast_top - mast_bottom, 0.07), 0.008, 2, 0.3);
    p.add(
        art::DARK,
        kit::placed(
            mast,
            at(Vec3::new(0.0, (mast_top + mast_bottom) * 0.5, 0.0)),
        ),
    );
    p.add(
        art::DARK,
        kit::placed(
            kit::rounded_box(Vec3::new(0.095, 0.12, 0.095), 0.01, 2, 0.1),
            at(Vec3::new(0.0, mast_top - 0.35, 0.0)),
        ),
    );
    p.add(
        art::GALV,
        kit::rod(
            Vec3::new(0.0, mast_top - 0.35, 0.045),
            Vec3::new(0.0, mast_top - 0.35, 0.12),
            0.007,
            8,
        ),
    );
    p.add(
        art::RUBBER,
        kit::placed(
            kit::rounded_box(Vec3::new(0.09, 0.022, 0.022), 0.009, 2, 0.1),
            at(Vec3::new(0.0, mast_top - 0.35, 0.125)),
        ),
    );
    for k in 0..4 {
        let ang = k as f32 * FRAC_PI_2 + PI / 4.0;
        let dir = Vec3::new(ang.cos(), 0.0, ang.sin());
        let a = Vec3::new(0.0, mast_bottom + 0.05, 0.0);
        let b = dir * 0.85 + Vec3::Y * (ground + 0.045);
        let d = b - a;
        let leg = kit::rounded_box(Vec3::new(0.055, d.length(), 0.055), 0.007, 2, 0.3);
        p.add(
            art::DARK,
            kit::placed(
                leg,
                tr(
                    (a + b) * 0.5,
                    Quat::from_rotation_arc(Vec3::Y, d.normalize()),
                ),
            ),
        );
        let foot = kit::lathe(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.065, 0.0),
                Vec2::new(0.068, 0.006),
                Vec2::new(0.06, 0.03),
                Vec2::new(0.0, 0.032),
            ],
            20,
            35.0,
            false,
        );
        p.add(
            art::RUBBER,
            kit::placed(foot, at(dir * 0.88 + Vec3::Y * ground)),
        );
    }
    p.add(
        art::DARK,
        kit::placed(
            kit::rounded_box(Vec3::new(0.12, 0.08, 0.12), 0.01, 2, 0.1),
            at(Vec3::new(0.0, mast_bottom + 0.04, 0.0)),
        ),
    );
    p.shade(|_, n| Vec3::splat(1.0 - n.y.min(0.0).abs() * 0.2));
    p
}

/// One blade along +X, root at the hub: a cambered, twisted plate that widens to a rounded
/// tip, as a closed surface.
fn blade() -> Mesh {
    let stations = 12;
    let chord_pts = 10;
    let r0 = 0.1;
    let mut rings: Vec<Vec<Vec3>> = Vec::new();
    for i in 0..=stations {
        let t = i as f32 / stations as f32;
        let r = r0 + (TIP_R - r0) * t;
        // Wider toward the tip, rounded off at the very end.
        let tip_round = (1.0 - ((t - 0.86).max(0.0) / 0.14).powi(2)).max(0.0).sqrt();
        let chord = (0.12 + 0.25 * t.powf(0.8)) * (0.25 + 0.75 * tip_round);
        let pitch = (40.0 - 22.0 * t).to_radians();
        let camber = 0.07 * chord;
        let thick = 0.006 * (1.0 - 0.5 * t);
        let chord_dir = Vec3::new(0.0, pitch.cos(), -pitch.sin());
        let thick_dir = Vec3::new(0.0, pitch.sin(), pitch.cos());
        let sweep = 0.07 * t * t;
        let mut ring = Vec::new();
        // Upper surface trailing edge to leading edge, then lower back.
        for k in 0..chord_pts {
            let s = 1.0 - (k as f32 / (chord_pts - 1) as f32);
            let s = 0.5 - 0.5 * (s * PI).cos();
            let c = camber * 4.0 * s * (1.0 - s);
            let th = thick * (s.sqrt() * (1.0 - s)).max(0.08) * 2.2;
            ring.push(Vec3::X * r + chord_dir * ((0.4 - s) * chord + sweep) + thick_dir * (c + th));
        }
        for k in 1..chord_pts - 1 {
            let s = k as f32 / (chord_pts - 1) as f32;
            let s = 0.5 - 0.5 * (s * PI).cos();
            let c = camber * 4.0 * s * (1.0 - s);
            let th = thick * (s.sqrt() * (1.0 - s)).max(0.08) * 2.2;
            ring.push(Vec3::X * r + chord_dir * ((0.4 - s) * chord + sweep) + thick_dir * (c - th));
        }
        rings.push(ring);
    }
    let per = rings[0].len() as u32;
    let mut m = Mesh::default();
    for (i, ring) in rings.iter().enumerate() {
        let v = i as f32 / stations as f32;
        for (k, p) in ring.iter().enumerate() {
            m.vertices.push(kit::vtx(
                *p,
                Vec3::Y,
                Vec2::new(k as f32 / per as f32 * 0.3, v * 0.83),
            ));
        }
    }
    for i in 0..stations as u32 {
        for k in 0..per {
            let a = i * per + k;
            let b = i * per + (k + 1) % per;
            let c = (i + 1) * per + (k + 1) % per;
            let d = (i + 1) * per + k;
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    // Cap the tip.
    let last = stations as u32 * per;
    for k in 1..per - 1 {
        m.indices.extend_from_slice(&[last, last + k, last + k + 1]);
    }
    kit::orient_outward(&mut m);
    kit::welded_normals(&mut m);
    m
}

/// The spinning parts: spinner, hub, five blades with root clamps, and the shaft.
pub fn rotor() -> Parts {
    let mut p = Parts::default();
    let spinner = [
        Vec2::new(0.0, -0.2),
        Vec2::new(0.03, -0.195),
        Vec2::new(0.07, -0.17),
        Vec2::new(0.105, -0.13),
        Vec2::new(0.125, -0.085),
        Vec2::new(0.13, -0.06),
        Vec2::new(0.13, 0.04),
        Vec2::new(0.12, 0.05),
        Vec2::new(0.03, 0.05),
        Vec2::new(0.03, 0.27),
        Vec2::new(0.0, 0.27),
    ];
    // The lathe runs along +Y; turn it so −Y (the nose) points out of the front (−Z).
    p.add(
        art::FAN,
        kit::placed(
            kit::lathe(&spinner, 40, 35.0, false),
            tr(Vec3::ZERO, Quat::from_rotation_x(FRAC_PI_2)),
        ),
    );
    let b = blade();
    for k in 0..5 {
        let ang = k as f32 / 5.0 * TAU;
        let rot = Quat::from_rotation_z(ang);
        p.add(art::ALU, kit::placed(b.clone(), tr(Vec3::ZERO, rot)));
        let clamp = kit::rounded_box(Vec3::new(0.08, 0.05, 0.035), 0.008, 2, 0.1);
        p.add(
            art::FAN,
            kit::placed(clamp, tr(rot * Vec3::new(0.14, 0.0, -0.01), rot)),
        );
        for d in [-0.015f32, 0.015] {
            p.add(
                art::GALV,
                kit::placed(
                    kit::hex_bolt(0.008),
                    tr(
                        rot * Vec3::new(0.14 + d, 0.0, -0.028),
                        rot * Quat::from_rotation_x(-FRAC_PI_2),
                    ),
                ),
            );
        }
    }
    p
}
