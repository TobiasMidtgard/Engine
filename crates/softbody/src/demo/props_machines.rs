//! Machine props: plank crates with steel corners (the jelly stands), the painted steel
//! gantry with its pad eye and shackle, the wrecking ball, and the cast-iron screw press.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::prelude::*;
use engine_world::physics::ColliderShape;

use super::kit::{Mat, Model, ModelBuilder, cuboid as box_shape, cylinder, part};
use super::meshkit::*;
use super::props::*;

// ---------------------------------------------------------------------------------------
// Crate

/// A 1.2 m plank crate on skids: five boards per side with gaps (a dark liner behind them),
/// edge cleats and a diagonal brace on every face, top cleats, and riveted steel corners.
/// Origin at the bottom center; the top is flat at 1.2 m.
pub(super) fn plank_crate(commands: &mut Commands, seed: u32) -> Model {
    let mut mb = ModelBuilder::new(format!("crate_{seed}")).grounded();
    let mut n = seed * 100;
    let mut next = || {
        n += 1;
        n
    };
    let (s, t) = (1.2f32, 0.022f32);
    let h = s * 0.5;
    let y0 = 0.06;
    for x in [-0.42f32, 0.42] {
        mb.add(
            Mat::Timber,
            board_at(Vec3::new(x, y0 * 0.5, 0.0), Vec3::new(0.09, y0, s), next()),
        );
    }
    mb.add(
        Mat::TimberDark,
        board_at(
            Vec3::new(0.0, (y0 + s) * 0.5, 0.0),
            Vec3::new(s - 2.0 * t, s - y0 - 0.02, s - 2.0 * t),
            next(),
        ),
    );
    let side_h = s - t - y0;
    let rows = 5;
    let gap = 0.012;
    let bh = (side_h - gap * (rows - 1) as f32) / rows as f32;
    for r in 0..rows {
        let y = y0 + bh * 0.5 + r as f32 * (bh + gap);
        for zs in [-1.0f32, 1.0] {
            mb.add(
                Mat::Pine,
                board_at(
                    Vec3::new(0.0, y, zs * (h - t * 0.5)),
                    Vec3::new(s, bh, t),
                    next(),
                ),
            );
        }
        for xs in [-1.0f32, 1.0] {
            mb.add(
                Mat::Pine,
                board_at(
                    Vec3::new(xs * (h - t * 0.5), y, 0.0),
                    Vec3::new(t, bh, s - 2.0 * t),
                    next(),
                ),
            );
        }
    }
    let tw = (s - gap * (rows - 1) as f32) / rows as f32;
    for r in 0..rows {
        let z = -h + tw * 0.5 + r as f32 * (tw + gap);
        mb.add(
            Mat::Pine,
            board_at(Vec3::new(0.0, s - t * 0.5, z), Vec3::new(s, t, tw), next()),
        );
    }
    // Edge cleats and a brace on every side face.
    let cw = 0.08;
    let ch = side_h;
    let cy = y0 + ch * 0.5;
    for (face, (normal, along)) in [
        (Vec3::Z, Vec3::X),
        (Vec3::NEG_Z, Vec3::NEG_X),
        (Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_X, Vec3::Z),
    ]
    .into_iter()
    .enumerate()
    {
        let out = normal * (h + t * 0.5);
        for e in [-1.0f32, 1.0] {
            let c = out + along * e * (h - cw * 0.5) + Vec3::Y * cy;
            let size = Vec3::new(cw, ch, t);
            mb.add(
                Mat::Pine,
                place(
                    board(size, next()),
                    c,
                    Quat::from_rotation_arc(Vec3::Z, normal),
                ),
            );
        }
        let d = if face % 2 == 0 { 1.0 } else { -1.0 };
        let a = out + along * (-d * (h - cw)) + Vec3::Y * (y0 + 0.07);
        let b = out + along * (d * (h - cw)) + Vec3::Y * (s - t - 0.07);
        mb.add(Mat::Pine, beam(a, b, cw, t, along, next()));
        // Nail heads along the cleats.
        let mut nails = Vec::new();
        for e in [-1.0f32, 1.0] {
            for r in 0..rows {
                let y = y0 + bh * 0.5 + r as f32 * (bh + gap);
                nails.push(normal * (h + t) + along * e * (h - cw * 0.5) + Vec3::Y * y);
            }
        }
        mb.add_small(Mat::DarkSteel, bolt_heads(&nails, normal, 0.006));
    }
    for x in [-1.0f32, 1.0] {
        mb.add(
            Mat::Pine,
            board_at(
                Vec3::new(x * (h - cw * 0.5 - 0.02), s + t * 0.5, 0.0),
                Vec3::new(cw, t, s),
                next(),
            ),
        );
    }
    // Steel corners: three riveted plates folded over each corner (two at the bottom).
    let plate = rounded_box(Vec3::new(0.13, 0.13, 0.004), 0.0018, 1, 0.2, BoxUv::PLANAR);
    let off = h + t + 0.002;
    for sy in [-1.0f32, 1.0] {
        for sx in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                let yc = if sy > 0.0 { s + t + 0.002 } else { y0 };
                let ye = if sy > 0.0 { s + t - 0.063 } else { y0 + 0.065 };
                let ex = sx * (off - 0.063);
                let ez = sz * (off - 0.063);
                let mut plates = vec![
                    (Vec3::new(ex, ye, sz * off), Vec3::Z * sz),
                    (Vec3::new(sx * off, ye, ez), Vec3::X * sx),
                ];
                if sy > 0.0 {
                    plates.push((Vec3::new(ex, yc, ez), Vec3::Y));
                }
                for (c, nrm) in plates {
                    mb.add(
                        Mat::PaintRed,
                        place(plate.clone(), c, Quat::from_rotation_arc(Vec3::Z, nrm)),
                    );
                    let (u, v) = if nrm.y.abs() > 0.5 {
                        (Vec3::X * sx, Vec3::Z * sz)
                    } else if nrm.x.abs() > 0.5 {
                        (Vec3::Z * sz, Vec3::Y * sy)
                    } else {
                        (Vec3::X * sx, Vec3::Y * sy)
                    };
                    let rivets = [
                        c + u * 0.035 + v * 0.035,
                        c - u * 0.03 + v * 0.035,
                        c + u * 0.035 - v * 0.03,
                    ];
                    let rivets: Vec<Vec3> = rivets.iter().map(|&p| p + nrm * 0.002).collect();
                    mb.add_small(Mat::DarkSteel, bolt_heads(&rivets, nrm, 0.007));
                }
            }
        }
    }
    mb.build(commands)
}

// ---------------------------------------------------------------------------------------
// Gantry and wrecking ball

/// An I-section profile (flanges top and bottom, `depth` tall, `width` wide) with filleted
/// web joints and softened outer edges.
fn i_section(depth: f32, width: f32, tf: f32, tw: f32) -> Vec<Vec2> {
    let (d, b) = (depth * 0.5, width * 0.5);
    let p = [
        Vec2::new(-b, -d),
        Vec2::new(b, -d),
        Vec2::new(b, -d + tf),
        Vec2::new(tw, -d + tf),
        Vec2::new(tw, d - tf),
        Vec2::new(b, d - tf),
        Vec2::new(b, d),
        Vec2::new(-b, d),
        Vec2::new(-b, d - tf),
        Vec2::new(-tw, d - tf),
        Vec2::new(-tw, -d + tf),
        Vec2::new(-b, -d + tf),
    ];
    // Indices 3, 4, 9, 10 are the web fillets.
    round_corners(
        &p,
        |i| {
            if matches!(i, 3 | 4 | 9 | 10) {
                0.012
            } else {
                0.003
            }
        },
        3,
    )
}

/// Pivot of the wrecking ball's cable below the gantry beam, relative to the gantry origin.
pub(super) const GANTRY_PIVOT: f32 = 6.85;

/// A painted steel portal: H-section columns on base plates and concrete plinths, an
/// I-beam with end plates, gusseted knee braces, and a pad eye with a shackle and cable
/// thimble at the pivot (`GANTRY_PIVOT` m up, swinging along `swing`). Columns at x = ±5.
pub(super) fn gantry(commands: &mut Commands, swing: Vec3) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("gantry").grounded();
    let paint = Mat::PaintYellow;
    let col_top = 7.08f32;
    let base = 0.275f32;
    let beam_y = 7.28f32;
    let mut colliders = Vec::new();
    for x in [-5.0f32, 5.0] {
        mb.add(
            Mat::Concrete,
            place(
                rounded_box(Vec3::new(0.9, 0.3, 0.9), 0.03, 2, 0.5, BoxUv::PLANAR),
                Vec3::new(x, 0.1, 0.0),
                Quat::IDENTITY,
            ),
        );
        mb.add(
            paint,
            place(
                rounded_box(Vec3::new(0.55, 0.025, 0.55), 0.005, 1, 0.5, BoxUv::PLANAR),
                Vec3::new(x, 0.2625, 0.0),
                Quat::IDENTITY,
            ),
        );
        let anchors: Vec<Vec3> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .iter()
            .map(|&(a, b)| Vec3::new(x + a * 0.21, base, b * 0.21))
            .collect();
        mb.add_small(Mat::Galvanized, nuts(&anchors, Vec3::Y, 0.022));
        let len = col_top - base;
        let column = extrude(&i_section(0.3, 0.3, 0.02, 0.007), len, 0.002, 30.0);
        let rot = Quat::from_rotation_x(-FRAC_PI_2) * Quat::from_rotation_z(FRAC_PI_2);
        mb.add(
            paint,
            place(column, Vec3::new(x, base + len * 0.5, 0.0), rot),
        );
        mb.add(
            paint,
            place(
                rounded_box(Vec3::new(0.34, 0.02, 0.34), 0.004, 1, 0.5, BoxUv::PLANAR),
                Vec3::new(x, col_top + 0.01, 0.0),
                Quat::IDENTITY,
            ),
        );
        colliders.push(part(
            Vec3::new(x, (base + col_top) * 0.5, 0.0),
            box_shape(Vec3::new(0.3, len, 0.3)),
        ));
        // Knee brace (square tube) with gusset plates in the web plane.
        let sx = x.signum();
        let a = Vec3::new(x - sx * 0.15, 6.05, 0.0);
        let b = Vec3::new(x - sx * 1.1, beam_y - 0.18, 0.0);
        mb.add(
            paint,
            place(
                rounded_box(
                    Vec3::new(0.1, a.distance(b) + 0.1, 0.1),
                    0.008,
                    2,
                    0.5,
                    BoxUv::PLANAR,
                ),
                (a + b) * 0.5,
                frame_rot(b - a, Vec3::Z),
            ),
        );
        let gusset = extrude(
            &round_corners(
                &[
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.0, -0.45),
                    Vec2::new(0.06, -0.45),
                    Vec2::new(0.45, -0.06),
                    Vec2::new(0.45, 0.0),
                ],
                |_| 0.008,
                2,
            ),
            0.012,
            0.002,
            30.0,
        );
        let g = if sx > 0.0 { mirror_x(gusset) } else { gusset };
        mb.add(
            paint,
            place(
                g,
                Vec3::new(x - sx * 0.15, beam_y - 0.18, 0.0),
                Quat::IDENTITY,
            ),
        );
        // Bolted beam-to-column cap.
        let bolts: Vec<Vec3> = [(-0.1f32, -0.08f32), (0.1, -0.08), (-0.1, 0.08), (0.1, 0.08)]
            .iter()
            .map(|&(dx, dz)| Vec3::new(x + dx, beam_y - 0.18 - 0.004, dz))
            .collect();
        mb.add_small(Mat::Galvanized, nuts(&bolts, Vec3::NEG_Y, 0.016));
    }
    let blen = 10.7;
    let ib = extrude(&i_section(0.36, 0.22, 0.022, 0.008), blen, 0.002, 30.0);
    mb.add(
        paint,
        place(
            ib,
            Vec3::new(0.0, beam_y, 0.0),
            Quat::from_rotation_y(FRAC_PI_2),
        ),
    );
    for x in [-1.0f32, 1.0] {
        mb.add(
            paint,
            place(
                rounded_box(Vec3::new(0.02, 0.38, 0.24), 0.004, 1, 0.5, BoxUv::PLANAR),
                Vec3::new(x * (blen * 0.5 + 0.01), beam_y, 0.0),
                Quat::IDENTITY,
            ),
        );
    }
    colliders.push(part(
        Vec3::new(0.0, beam_y, 0.0),
        box_shape(Vec3::new(blen, 0.36, 0.22)),
    ));
    // Pad eye in the swing plane, shackle, thimble.
    let pivot = GANTRY_PIVOT;
    let flat = Vec3::new(swing.x, 0.0, swing.z).normalize_or(Vec3::X);
    let yaw = Quat::from_rotation_arc(Vec3::X, flat);
    let mut eye_profile = vec![Vec2::new(-0.075, 0.27), Vec2::new(-0.075, 0.19)];
    for k in 0..=10 {
        let a = PI + PI * k as f32 / 10.0;
        eye_profile.push(Vec2::new(a.cos() * 0.055, 0.13 + a.sin() * 0.055));
    }
    eye_profile.extend_from_slice(&[Vec2::new(0.075, 0.19), Vec2::new(0.075, 0.27)]);
    let eye = extrude(
        &round_corners(&eye_profile, |_| 0.006, 2),
        0.03,
        0.005,
        35.0,
    );
    mb.add(paint, place(eye, Vec3::Y * pivot, yaw));
    let n = yaw * Vec3::Z;
    let pin_y = pivot + 0.13;
    mb.add(
        Mat::DarkSteel,
        place(
            turned(
                &[
                    (0.0, -0.06),
                    (0.024, -0.06),
                    (0.024, -0.045),
                    (0.013, -0.045),
                    (0.013, 0.06),
                    (0.0, 0.06),
                ],
                12,
                Vec3::ZERO,
            ),
            Vec3::Y * pin_y,
            Quat::from_rotation_arc(Vec3::Y, n),
        ),
    );
    mb.add_small(
        Mat::DarkSteel,
        place(
            hex_nut(0.034, 0.02),
            Vec3::Y * pin_y + n * 0.05,
            Quat::from_rotation_arc(Vec3::Y, n),
        ),
    );
    let bow = smooth_path(
        &[
            Vec3::Y * (pin_y + 0.01) + n * 0.04,
            Vec3::Y * (pin_y - 0.06) + n * 0.042,
            Vec3::Y * (pivot + 0.015),
            Vec3::Y * (pin_y - 0.06) - n * 0.042,
            Vec3::Y * (pin_y + 0.01) - n * 0.04,
        ],
        6,
    );
    mb.add(Mat::DarkSteel, tube(&bow, |_| 0.011, 10, false, true, 1.0));
    mb.add(
        Mat::DarkSteel,
        place(
            torus(0.02, 0.006, 16, 6),
            Vec3::Y * (pivot + 0.012),
            yaw * Quat::from_rotation_x(FRAC_PI_2),
        ),
    );
    (mb.build(commands), ColliderShape::Compound(colliders))
}

/// The wrecking ball (local +Y toward the cable): a forged ball with a parting line, a
/// conical neck and collar, a clevis with its pin, and a spelter socket the cable runs into.
pub(super) fn wrecking_ball(commands: &mut Commands, radius: f32) -> Model {
    let mut mb = ModelBuilder::new("wrecking_ball");
    let r = radius;
    let mut p = Vec::new();
    for k in 0..=18 {
        let a = -FRAC_PI_2 + (FRAC_PI_2 + 0.87) * k as f32 / 18.0;
        p.push((a.cos().max(0.0) * r, a.sin() * r));
    }
    p.extend_from_slice(&[
        (r * 0.46, r * 0.92),
        (r * 0.3, r * 1.1),
        (r * 0.3, r * 1.1),
        (r * 0.34, r * 1.11),
        (r * 0.34, r * 1.24),
        (r * 0.26, r * 1.28),
        (0.0, r * 1.29),
    ]);
    mb.add(Mat::DarkSteel, turned(&p, 40, Vec3::ZERO));
    mb.add_small(
        Mat::DarkSteel,
        place(torus(r * 1.003, 0.006, 48, 6), Vec3::ZERO, Quat::IDENTITY),
    );
    let top = r * 1.28;
    for side in [-1.0f32, 1.0] {
        mb.add(
            Mat::DarkSteel,
            place(
                rounded_box(Vec3::new(0.026, 0.17, 0.11), 0.008, 2, 0.2, BoxUv::PLANAR),
                Vec3::new(side * 0.046, top + 0.07, 0.0),
                Quat::IDENTITY,
            ),
        );
    }
    mb.add(
        Mat::DarkSteel,
        place(
            turned(
                &[
                    (0.0, -0.075),
                    (0.026, -0.075),
                    (0.026, -0.062),
                    (0.017, -0.062),
                    (0.017, 0.075),
                    (0.0, 0.075),
                ],
                12,
                Vec3::ZERO,
            ),
            Vec3::Y * (top + 0.11),
            Quat::from_rotation_z(-FRAC_PI_2),
        ),
    );
    mb.add(
        Mat::DarkSteel,
        turned(
            &[
                (0.0, 0.0),
                (0.03, 0.0),
                (0.034, 0.02),
                (0.034, 0.05),
                (0.024, 0.2),
                (0.022, 0.22),
                (0.0, 0.22),
            ],
            14,
            Vec3::Y * (top + 0.08),
        ),
    );
    // The socket's tongue between the clevis plates, around the pin.
    mb.add(
        Mat::DarkSteel,
        place(
            rounded_box(Vec3::new(0.056, 0.1, 0.075), 0.012, 2, 0.2, BoxUv::PLANAR),
            Vec3::Y * (top + 0.11),
            Quat::IDENTITY,
        ),
    );
    mb.build(commands)
}

// ---------------------------------------------------------------------------------------
// Press

/// Height of the press's crosshead underside, meters.
const CROSSHEAD_Y: f32 = 2.62;

/// The fixed frame of the screw press: a ribbed cast-iron bed with column bosses and a
/// slotted steel table (top at 1 m), two turned steel columns with nuts, and an arched
/// crosshead with a bronze nut in the middle. Columns at x = ±0.9.
pub(super) fn press_frame(commands: &mut Commands) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("press_frame").grounded();
    let iron = Mat::CastIron;
    mb.add(
        iron,
        place(
            rounded_box(Vec3::new(2.5, 0.12, 1.4), 0.03, 2, 0.5, BoxUv::PLANAR),
            Vec3::Y * 0.06,
            Quat::IDENTITY,
        ),
    );
    let body = round_corners(
        &[
            Vec2::new(-1.0, 0.12),
            Vec2::new(1.0, 0.12),
            Vec2::new(1.0, 0.3),
            Vec2::new(0.78, 0.52),
            Vec2::new(0.75, 0.92),
            Vec2::new(-0.75, 0.92),
            Vec2::new(-0.78, 0.52),
            Vec2::new(-1.0, 0.3),
        ],
        |i| if i < 2 { 0.0 } else { 0.06 },
        4,
    );
    mb.add(iron, extrude(&body, 1.2, 0.025, 35.0));
    for z in [-1.0f32, 1.0] {
        for x in [-0.42f32, 0.0, 0.42] {
            mb.add(
                iron,
                place(
                    rounded_box(Vec3::new(0.06, 0.62, 0.05), 0.018, 2, 0.3, BoxUv::PLANAR),
                    Vec3::new(x, 0.5, z * 0.615),
                    Quat::IDENTITY,
                ),
            );
        }
    }
    for k in 0..4 {
        let z = -0.45 + k as f32 * 0.3;
        mb.add(
            Mat::BrightSteel,
            place(
                rounded_box(Vec3::new(1.5, 0.08, 0.27), 0.01, 2, 0.4, BoxUv::PLANAR),
                Vec3::new(0.0, 0.96, z),
                Quat::IDENTITY,
            ),
        );
    }
    let mut colliders = vec![
        part(Vec3::Y * 0.06, box_shape(Vec3::new(2.5, 0.12, 1.4))),
        part(Vec3::Y * 0.52, box_shape(Vec3::new(1.56, 0.8, 1.2))),
        part(Vec3::Y * 0.96, box_shape(Vec3::new(1.5, 0.08, 1.25))),
    ];
    for x in [-0.9f32, 0.9] {
        mb.add(
            iron,
            turned(
                &[
                    (0.0, 0.12),
                    (0.2, 0.12),
                    (0.2, 0.12),
                    (0.2, 0.14),
                    (0.185, 0.17),
                    (0.165, 0.3),
                    (0.16, 0.9),
                    (0.172, 0.94),
                    (0.172, 0.99),
                    (0.16, 1.0),
                    (0.0, 1.0),
                ],
                28,
                Vec3::new(x, 0.0, 0.0),
            ),
        );
        let mut col = vec![(0.0, 1.0), (0.075, 1.0), (0.075, 1.0), (0.075, 2.93)];
        for k in 0..8 {
            let y = 2.94 + k as f32 * 0.02;
            col.extend_from_slice(&[(0.075, y), (0.069, y + 0.01)]);
        }
        col.extend_from_slice(&[(0.072, 3.11), (0.065, 3.12), (0.0, 3.12)]);
        mb.add(Mat::BrightSteel, turned(&col, 20, Vec3::new(x, 0.0, 0.0)));
        mb.add(
            Mat::BlackSteel,
            place(hex_nut(0.2, 0.08), Vec3::new(x, 1.0, 0.0), Quat::IDENTITY),
        );
        mb.add(
            iron,
            turned(
                &[
                    (0.0, CROSSHEAD_Y - 0.02),
                    (0.16, CROSSHEAD_Y - 0.02),
                    (0.165, CROSSHEAD_Y),
                    (0.165, 2.93),
                    (0.15, 2.95),
                    (0.0, 2.95),
                ],
                28,
                Vec3::new(x, 0.0, 0.0),
            ),
        );
        mb.add(
            Mat::BlackSteel,
            place(hex_nut(0.2, 0.08), Vec3::new(x, 2.95, 0.0), Quat::IDENTITY),
        );
        colliders.push(part(Vec3::new(x, 0.5, 0.0), cylinder(0.2, 1.0)));
        colliders.push(part(Vec3::new(x, 1.8, 0.0), cylinder(0.075, 1.6)));
    }
    let mut arch = vec![Vec2::new(-1.12, CROSSHEAD_Y), Vec2::new(1.12, CROSSHEAD_Y)];
    for k in 0..=16 {
        let x = 1.12 - 2.24 * k as f32 / 16.0;
        arch.push(Vec2::new(x, 2.9 + 0.11 * (FRAC_PI_2 * x / 1.12).cos()));
    }
    let arch = round_corners(
        &arch,
        |i| {
            if i <= 2 || i == 18 { 0.03 } else { 0.0 }
        },
        3,
    );
    let crosshead = extrude(&arch, 0.34, 0.022, 35.0);
    mb.add(iron, crosshead);
    colliders.push(part(Vec3::Y * 2.78, box_shape(Vec3::new(2.24, 0.34, 0.34))));
    mb.add(
        iron,
        turned(
            &[
                (0.0, 2.6),
                (0.2, 2.6),
                (0.21, 2.62),
                (0.21, 3.04),
                (0.19, 3.06),
                (0.0, 3.06),
            ],
            32,
            Vec3::ZERO,
        ),
    );
    mb.add(
        Mat::Brass,
        turned(
            &[
                (0.0, 3.06),
                (0.15, 3.06),
                (0.15, 3.09),
                (0.14, 3.1),
                (0.0, 3.1),
            ],
            28,
            Vec3::ZERO,
        ),
    );
    let heads: Vec<Vec3> = (0..6)
        .map(|k| {
            let a = k as f32 / 6.0 * TAU;
            Vec3::new(a.cos() * 0.115, 3.1, a.sin() * 0.115)
        })
        .collect();
    mb.add_small(Mat::BlackSteel, nuts(&heads, Vec3::Y, 0.012));
    (mb.build(commands), ColliderShape::Compound(colliders))
}

/// Screw length above the ram's crossbar, meters.
pub(super) const PRESS_SCREW: f32 = 1.55;

/// The moving ram of the press (origin at the ram's center; its collider is the 0.5 × 0.4 ×
/// 0.5 block): cast platen with a hardened face plate, a crossbar with bronze bushings that
/// slide on the columns, and the threaded screw rising through the crosshead.
pub(super) fn press_ram(commands: &mut Commands) -> Model {
    let mut mb = ModelBuilder::new("press_ram");
    mb.add(
        Mat::CastIron,
        place(
            rounded_box(Vec3::new(0.5, 0.32, 0.5), 0.02, 2, 0.3, BoxUv::PLANAR),
            Vec3::Y * 0.04,
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BrightSteel,
        place(
            rounded_box(Vec3::new(0.48, 0.08, 0.48), 0.008, 2, 0.3, BoxUv::PLANAR),
            Vec3::Y * -0.16,
            Quat::IDENTITY,
        ),
    );
    let bar = round_corners(
        &[
            Vec2::new(-0.95, 0.2),
            Vec2::new(0.95, 0.2),
            Vec2::new(0.95, 0.36),
            Vec2::new(0.25, 0.4),
            Vec2::new(-0.25, 0.4),
            Vec2::new(-0.95, 0.36),
        ],
        |_| 0.03,
        3,
    );
    mb.add(Mat::CastIron, extrude(&bar, 0.22, 0.015, 35.0));
    for x in [-0.9f32, 0.9] {
        mb.add(
            Mat::Brass,
            turned(
                &[
                    (0.0, 0.13),
                    (0.11, 0.13),
                    (0.12, 0.145),
                    (0.12, 0.415),
                    (0.11, 0.43),
                    (0.0, 0.43),
                ],
                24,
                Vec3::new(x, 0.0, 0.0),
            ),
        );
    }
    mb.add(
        Mat::BlackSteel,
        turned(
            &[
                (0.0, 0.4),
                (0.11, 0.4),
                (0.11, 0.44),
                (0.09, 0.46),
                (0.0, 0.46),
            ],
            24,
            Vec3::ZERO,
        ),
    );
    let mut screw = vec![(0.0, 0.46), (0.06, 0.46), (0.06, 0.46)];
    let pitch = 0.03;
    let turns = ((PRESS_SCREW - 0.5) / pitch) as u32;
    for k in 0..turns {
        let y = 0.5 + k as f32 * pitch;
        screw.extend_from_slice(&[
            (0.061, y),
            (0.061, y + pitch * 0.3),
            (0.052, y + pitch * 0.55),
            (0.052, y + pitch * 0.75),
        ]);
    }
    screw.extend_from_slice(&[
        (0.055, PRESS_SCREW + 0.4),
        (0.04, PRESS_SCREW + 0.42),
        (0.0, PRESS_SCREW + 0.42),
    ]);
    mb.add(Mat::BrightSteel, turned(&screw, 18, Vec3::ZERO));
    mb.build(commands)
}

/// The press's handwheel (spins about +Y on top of the screw): hub, five dished spokes, a
/// rim and a turned wooden handle.
pub(super) fn press_wheel(commands: &mut Commands) -> Model {
    let mut mb = ModelBuilder::new("press_wheel");
    let iron = Mat::CastIron;
    mb.add(
        iron,
        turned(
            &[
                (0.0, -0.07),
                (0.085, -0.07),
                (0.095, -0.05),
                (0.095, 0.04),
                (0.08, 0.06),
                (0.0, 0.06),
            ],
            24,
            Vec3::ZERO,
        ),
    );
    let rim_r = 0.5;
    let dish = -0.06;
    for k in 0..5 {
        let a = k as f32 / 5.0 * TAU;
        let d = Vec3::new(a.cos(), 0.0, -a.sin());
        let path = smooth_path(
            &[
                d * 0.08,
                d * 0.25 + Vec3::Y * dish * 0.6,
                d * (rim_r - 0.02) + Vec3::Y * dish,
            ],
            6,
        );
        mb.add(
            iron,
            tube(&path, |t| 0.024 - 0.008 * t, 10, false, true, 1.0),
        );
    }
    mb.add(
        Mat::BrightSteel,
        place(torus(rim_r, 0.03, 64, 10), Vec3::Y * dish, Quat::IDENTITY),
    );
    mb.add(
        Mat::TimberDark,
        turned_wood(
            &[
                (0.0, 0.0),
                (0.016, 0.0),
                (0.024, 0.03),
                (0.022, 0.12),
                (0.016, 0.16),
                (0.0, 0.168),
            ],
            14,
            Vec3::new(rim_r, dish + 0.02, 0.0),
            700,
        ),
    );
    mb.build(commands)
}
