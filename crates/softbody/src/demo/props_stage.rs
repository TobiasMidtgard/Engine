//! Stage props: the curtain stage (timber frame, brass rod with rings and finials), the
//! pedestal gun that shoots the velvet curtain, the gas burner that lights the linen one,
//! the bead-curtain doorframe and the swing's A-frame and seat.

use std::f32::consts::{FRAC_PI_2, TAU};

use engine_core::prelude::*;
use engine_render::Mesh;
use engine_world::physics::ColliderShape;

use super::kit::{Mat, Model, ModelBuilder, cuboid as box_shape, cylinder, part, part_rot};
use super::meshkit::*;
use super::props::*;
use crate::cloth::{Cloth, ClothPins};

/// X positions (relative to the curtain's anchor) of the rings a curtain hangs from: its
/// pinned top-edge particles, laid out as the cloth builder gathers them.
pub(super) fn curtain_ring_xs(spec: &Cloth) -> Vec<f32> {
    let nu = spec.resolution[0].max(2) as usize;
    let every = match spec.pins {
        ClothPins::TopEdge { every } => every.max(1) as usize,
        _ => return Vec::new(),
    };
    let w = spec.size[0];
    let gather = spec.gather.clamp(0.2, 1.0);
    let x = |i: usize| (i as f32 / (nu - 1) as f32 - spec.anchor[0]) * w * gather;
    let mut xs: Vec<f32> = (0..nu).step_by(every).map(x).collect();
    if !(nu - 1).is_multiple_of(every) {
        xs.push(x(nu - 1));
    }
    xs
}

/// A brass acorn finial along +Y, base at the origin.
fn finial() -> Mesh {
    turned(
        &[
            (0.0, 0.0),
            (0.036, 0.0),
            (0.036, 0.012),
            (0.026, 0.022),
            (0.03, 0.03),
            (0.045, 0.055),
            (0.05, 0.08),
            (0.044, 0.11),
            (0.026, 0.14),
            (0.012, 0.158),
            (0.012, 0.165),
            (0.0, 0.172),
        ],
        20,
        Vec3::ZERO,
    )
}

/// A peg (wooden dowel end) standing out of a face along `normal`.
fn peg(at: Vec3, normal: Vec3, seed: u32) -> Mesh {
    let m = turned_wood(
        &[
            (0.0, -0.03),
            (0.017, -0.03),
            (0.017, 0.004),
            (0.014, 0.009),
            (0.0, 0.01),
        ],
        10,
        Vec3::ZERO,
        seed,
    );
    place(m, at, Quat::from_rotation_arc(Vec3::Y, normal))
}

/// Post base: a concrete pad with a galvanized U-shoe and through-bolts. Post bottom at the
/// returned height.
fn post_shoe(mb: &mut ModelBuilder, at: Vec3, post: f32) -> f32 {
    mb.add(
        Mat::Concrete,
        place(
            rounded_box(
                Vec3::new(post + 0.28, 0.16, post + 0.28),
                0.025,
                2,
                0.5,
                BoxUv::PLANAR,
            ),
            at + Vec3::Y * 0.02,
            Quat::IDENTITY,
        ),
    );
    let top = 0.1;
    mb.add(
        Mat::Galvanized,
        place(
            rounded_box(
                Vec3::new(post + 0.06, 0.012, post + 0.06),
                0.004,
                1,
                0.5,
                BoxUv::PLANAR,
            ),
            at + Vec3::Y * (top + 0.006),
            Quat::IDENTITY,
        ),
    );
    for side in [-1.0f32, 1.0] {
        let x = side * (post * 0.5 + 0.005);
        mb.add(
            Mat::Galvanized,
            place(
                rounded_box(
                    Vec3::new(0.01, 0.32, post - 0.02),
                    0.003,
                    1,
                    0.5,
                    BoxUv::PLANAR,
                ),
                at + Vec3::new(x, top + 0.17, 0.0),
                Quat::IDENTITY,
            ),
        );
        let pts: Vec<Vec3> = [0.2f32, 0.34]
            .iter()
            .map(|&y| at + Vec3::new(x + side * 0.005, y, 0.0))
            .collect();
        mb.add_small(Mat::BlackSteel, nuts(&pts, Vec3::X * side, 0.014));
    }
    top + 0.012
}

/// The curtain stage: three dark-stained posts on steel shoes, a lintel with knee braces,
/// pegged joints and black T-straps, and a brass rod on brackets in front of it with
/// finials and a ring at every pinned point of the two curtains (`ring_xs`, stage-relative).
pub(super) fn curtain_stage(commands: &mut Commands, ring_xs: &[f32]) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("curtain_stage").grounded();
    let w = Mat::TimberDark;
    let posts = [-5.2f32, 0.0, 5.2];
    let mut colliders = Vec::new();
    for (i, &px) in posts.iter().enumerate() {
        let base = post_shoe(&mut mb, Vec3::new(px, 0.0, 0.0), 0.28);
        let h = 6.2 - base;
        mb.add(
            w,
            board_at(
                Vec3::new(px, base + h * 0.5, 0.0),
                Vec3::new(0.28, h, 0.28),
                10 + i as u32,
            ),
        );
        colliders.push(part(
            Vec3::new(px, 3.1, 0.0),
            box_shape(Vec3::new(0.28, 6.2, 0.28)),
        ));
    }
    mb.add(
        w,
        board_at(Vec3::new(0.0, 6.37, 0.0), Vec3::new(11.4, 0.34, 0.3), 20),
    );
    colliders.push(part(
        Vec3::new(0.0, 6.37, 0.0),
        box_shape(Vec3::new(11.4, 0.34, 0.3)),
    ));
    // Knee braces with pegs at both ends.
    for (k, (px, d)) in [(-5.2f32, 1.0f32), (0.0, -1.0), (0.0, 1.0), (5.2, -1.0)]
        .into_iter()
        .enumerate()
    {
        let a = Vec3::new(px + d * 0.08, 5.22, 0.0);
        let b = Vec3::new(px + d * 1.08, 6.26, 0.0);
        mb.add(w, beam(a, b, 0.13, 0.13, Vec3::Z, 30 + k as u32));
        let rot = frame_rot(b - a, Vec3::Z);
        colliders.push(part_rot(
            (a + b) * 0.5,
            rot,
            box_shape(Vec3::new(0.13, a.distance(b), 0.13)),
        ));
        mb.add_small(
            Mat::Timber,
            peg(Vec3::new(px + d * 0.22, 5.4, 0.14), Vec3::Z, 40 + k as u32),
        );
        mb.add_small(
            Mat::Timber,
            peg(Vec3::new(px + d * 0.9, 6.3, 0.15), Vec3::Z, 50 + k as u32),
        );
    }
    // Pegged post-to-lintel joints and black T-straps on the front.
    for (i, &px) in posts.iter().enumerate() {
        for dx in [-0.07f32, 0.07] {
            mb.add_small(
                Mat::Timber,
                peg(Vec3::new(px + dx, 6.45, 0.15), Vec3::Z, 60 + i as u32),
            );
        }
        let z_post = 0.14 + 0.005;
        let z_beam = 0.15 + 0.005;
        mb.add(
            Mat::BlackSteel,
            place(
                rounded_box(Vec3::new(0.09, 0.62, 0.008), 0.003, 1, 0.3, BoxUv::PLANAR),
                Vec3::new(px, 5.89, z_post),
                Quat::IDENTITY,
            ),
        );
        let half = if i == 1 { 0.5 } else { 0.42 };
        let cx = if i == 1 {
            px
        } else {
            px - px.signum() * (half - 0.14)
        };
        mb.add(
            Mat::BlackSteel,
            place(
                rounded_box(
                    Vec3::new(half * 2.0, 0.09, 0.008),
                    0.003,
                    1,
                    0.3,
                    BoxUv::PLANAR,
                ),
                Vec3::new(cx, 6.3, z_beam),
                Quat::IDENTITY,
            ),
        );
        let mut heads: Vec<Vec3> = [5.68f32, 5.88, 6.08]
            .iter()
            .map(|&y| Vec3::new(px, y, z_post + 0.004))
            .collect();
        for k in 0..4 {
            let x = cx - half + 0.08 + k as f32 * (half * 2.0 - 0.16) / 3.0;
            heads.push(Vec3::new(x, 6.3, z_beam + 0.004));
        }
        mb.add_small(Mat::BlackSteel, bolt_heads(&heads, Vec3::Z, 0.013));
    }
    // Brass rod on three brackets, with finials.
    let (rod_y, rod_z, rod_x) = (6.05f32, 0.25f32, 5.32f32);
    mb.add(
        Mat::Brass,
        place(
            turned(
                &[(0.0, -rod_x), (0.028, -rod_x), (0.028, rod_x), (0.0, rod_x)],
                16,
                Vec3::ZERO,
            ),
            Vec3::new(0.0, rod_y, rod_z),
            Quat::from_rotation_z(-FRAC_PI_2),
        ),
    );
    for side in [-1.0f32, 1.0] {
        let rot = Quat::from_rotation_arc(Vec3::Y, Vec3::X * side);
        mb.add(
            Mat::Brass,
            place(finial(), Vec3::new(side * rod_x, rod_y, rod_z), rot),
        );
    }
    for x in [-4.6f32, 0.0, 4.6] {
        mb.add(
            Mat::Brass,
            place(
                turned(
                    &[
                        (0.0, 0.0),
                        (0.042, 0.0),
                        (0.042, 0.008),
                        (0.03, 0.014),
                        (0.0, 0.014),
                    ],
                    16,
                    Vec3::ZERO,
                ),
                Vec3::new(x, 6.33, 0.15),
                Quat::from_rotation_x(FRAC_PI_2),
            ),
        );
        let arm = smooth_path(
            &[
                Vec3::new(x, 6.33, 0.16),
                Vec3::new(x, 6.32, 0.215),
                Vec3::new(x, 6.25, 0.25),
                Vec3::new(x, 6.15, rod_z),
                Vec3::new(x, 6.088, rod_z),
            ],
            4,
        );
        mb.add(
            Mat::Brass,
            tube(&arm, |t| 0.012 - 0.003 * t, 10, false, true, 1.0),
        );
        mb.add(
            Mat::Brass,
            place(
                torus(0.036, 0.008, 20, 8),
                Vec3::new(x, rod_y, rod_z),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        );
    }
    let ring = torus(0.042, 0.0055, 20, 8);
    let eye = torus(0.008, 0.0022, 10, 6);
    for &x in ring_xs {
        mb.add_small(
            Mat::Brass,
            place(
                ring.clone(),
                Vec3::new(x, rod_y, rod_z),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        );
        mb.add_small(
            Mat::Brass,
            place(
                eye.clone(),
                Vec3::new(x, rod_y - 0.052, rod_z),
                Quat::from_rotation_x(FRAC_PI_2),
            ),
        );
    }
    (mb.build(commands), ColliderShape::Compound(colliders))
}

// ---------------------------------------------------------------------------------------
// Pedestal gun

/// Height of the gun's pivot (trunnions) above the ground, meters.
pub(super) const GUN_PIVOT: f32 = 1.2;
/// Height of the turntable on the pedestal, meters.
pub(super) const GUN_TURNTABLE: f32 = 0.92;

/// The fixed part of the pedestal gun: concrete slab, bolted flange, tapered pedestal and
/// slewing ring.
pub(super) fn gun_pedestal(commands: &mut Commands) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("gun_pedestal").grounded();
    mb.add(
        Mat::Concrete,
        place(
            rounded_box(Vec3::new(1.1, 0.14, 1.1), 0.03, 2, 0.6, BoxUv::PLANAR),
            Vec3::Y * 0.05,
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::PaintGreen,
        turned(
            &[
                (0.0, 0.12),
                (0.32, 0.12),
                (0.32, 0.12),
                (0.32, 0.15),
                (0.3, 0.165),
                (0.19, 0.17),
                (0.15, 0.22),
                (0.12, 0.4),
                (0.105, 0.82),
                (0.12, 0.84),
                (0.12, 0.86),
                (0.0, 0.86),
            ],
            32,
            Vec3::ZERO,
        ),
    );
    let bolts: Vec<Vec3> = (0..8)
        .map(|k| {
            let a = k as f32 / 8.0 * TAU + 0.2;
            Vec3::new(a.cos() * 0.26, 0.165, a.sin() * 0.26)
        })
        .collect();
    mb.add_small(Mat::BlackSteel, nuts(&bolts, Vec3::Y, 0.016));
    mb.add(
        Mat::BlackSteel,
        turned(
            &[
                (0.0, 0.855),
                (0.15, 0.855),
                (0.155, 0.87),
                (0.155, 0.905),
                (0.15, 0.92),
                (0.0, 0.92),
            ],
            32,
            Vec3::ZERO,
        ),
    );
    // Ammunition can next to the pedestal.
    mb.add(
        Mat::PaintGreen,
        place(
            rounded_box(Vec3::new(0.3, 0.18, 0.15), 0.012, 2, 0.4, BoxUv::PLANAR),
            Vec3::new(0.42, 0.21, 0.3),
            Quat::from_rotation_y(0.4),
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            tube(
                &smooth_path(
                    &[
                        Vec3::new(-0.06, 0.0, 0.0),
                        Vec3::new(-0.05, 0.03, 0.0),
                        Vec3::new(0.05, 0.03, 0.0),
                        Vec3::new(0.06, 0.0, 0.0),
                    ],
                    4,
                ),
                |_| 0.006,
                6,
                false,
                true,
                1.0,
            ),
            Vec3::new(0.42, 0.3, 0.3),
            Quat::from_rotation_y(0.4),
        ),
    );
    (
        mb.build(commands),
        ColliderShape::Compound(vec![
            part(Vec3::Y * 0.05, box_shape(Vec3::new(1.1, 0.14, 1.1))),
            part(Vec3::Y * 0.5, cylinder(0.16, 0.84)),
        ]),
    )
}

/// The turning part of the gun mount (yawing about +Y): turntable and yoke with trunnions.
/// Origin at the turntable; the trunnions are `GUN_PIVOT - GUN_TURNTABLE` above it.
pub(super) fn gun_yoke(commands: &mut Commands) -> Model {
    let mut mb = ModelBuilder::new("gun_yoke");
    let pivot = GUN_PIVOT - GUN_TURNTABLE;
    mb.add(
        Mat::PaintGreen,
        turned(
            &[
                (0.0, 0.0),
                (0.15, 0.0),
                (0.15, 0.035),
                (0.13, 0.05),
                (0.0, 0.05),
            ],
            28,
            Vec3::ZERO,
        ),
    );
    for side in [-1.0f32, 1.0] {
        let arm = extrude(
            &round_corners(
                &[
                    Vec2::new(-0.07, 0.03),
                    Vec2::new(0.07, 0.03),
                    Vec2::new(0.05, pivot + 0.05),
                    Vec2::new(-0.05, pivot + 0.05),
                ],
                |_| 0.02,
                3,
            ),
            0.035,
            0.006,
            30.0,
        );
        mb.add(
            Mat::PaintGreen,
            place(
                arm,
                Vec3::new(side * 0.125, 0.0, 0.0),
                Quat::from_rotation_y(FRAC_PI_2),
            ),
        );
        mb.add(
            Mat::BlackSteel,
            place(
                turned(
                    &[
                        (0.0, 0.0),
                        (0.042, 0.0),
                        (0.042, 0.02),
                        (0.03, 0.03),
                        (0.0, 0.03),
                    ],
                    16,
                    Vec3::ZERO,
                ),
                Vec3::new(side * 0.142, pivot, 0.0),
                Quat::from_rotation_arc(Vec3::Y, Vec3::X * side),
            ),
        );
    }
    mb.build(commands)
}

/// The gun itself (pitching about +X at the trunnions, firing along −Z): receiver with top
/// cover, finned barrel jacket, barrel and muzzle brake, sights, spade grips, ammunition box
/// and feed chute.
pub(super) fn gun_body(commands: &mut Commands) -> Model {
    let mut mb = ModelBuilder::new("gun_body");
    let back = Quat::from_rotation_x(-FRAC_PI_2);
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.13, 0.15, 0.6), 0.012, 2, 0.2, BoxUv::PLANAR),
            Vec3::new(0.0, 0.03, 0.12),
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.122, 0.022, 0.42), 0.008, 2, 0.2, BoxUv::PLANAR),
            Vec3::new(0.0, 0.115, 0.12),
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.2, 0.075, 0.1), 0.01, 2, 0.2, BoxUv::PLANAR),
            Vec3::ZERO,
            Quat::IDENTITY,
        ),
    );
    // Finned barrel jacket, barrel, muzzle brake.
    let mut jacket = vec![(0.0, 0.0), (0.048, 0.0), (0.048, 0.0)];
    for k in 0..12 {
        let y = 0.04 + k as f32 * 0.045;
        jacket.extend_from_slice(&[
            (0.044, y),
            (0.054, y + 0.006),
            (0.054, y + 0.016),
            (0.044, y + 0.022),
        ]);
    }
    jacket.extend_from_slice(&[(0.044, 0.6), (0.03, 0.61), (0.0, 0.61)]);
    mb.add(
        Mat::BlackSteel,
        place(
            turned(&jacket, 20, Vec3::ZERO),
            Vec3::new(0.0, 0.03, -0.17),
            back,
        ),
    );
    mb.add(
        Mat::DarkSteel,
        place(
            turned(
                &[(0.0, 0.0), (0.019, 0.0), (0.017, 0.28), (0.0, 0.28)],
                14,
                Vec3::ZERO,
            ),
            Vec3::new(0.0, 0.03, -0.76),
            back,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            turned(
                &[
                    (0.0, 0.0),
                    (0.024, 0.0),
                    (0.03, 0.01),
                    (0.03, 0.03),
                    (0.024, 0.036),
                    (0.03, 0.042),
                    (0.03, 0.07),
                    (0.022, 0.08),
                    (0.0, 0.08),
                ],
                16,
                Vec3::ZERO,
            ),
            Vec3::new(0.0, 0.03, -1.03),
            back,
        ),
    );
    // Sights.
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.012, 0.06, 0.02), 0.003, 1, 0.1, BoxUv::PLANAR),
            Vec3::new(0.0, 0.1, -0.74),
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.05, 0.05, 0.025), 0.004, 1, 0.1, BoxUv::PLANAR),
            Vec3::new(0.0, 0.15, 0.3),
            Quat::IDENTITY,
        ),
    );
    // Spade grips behind the receiver.
    for side in [-1.0f32, 1.0] {
        let x = side * 0.07;
        mb.add(
            Mat::BlackSteel,
            tube(
                &smooth_path(
                    &[
                        Vec3::new(side * 0.04, 0.06, 0.42),
                        Vec3::new(x, 0.06, 0.48),
                        Vec3::new(x, 0.03, 0.5),
                    ],
                    4,
                ),
                |_| 0.012,
                8,
                false,
                true,
                1.0,
            ),
        );
        mb.add(
            Mat::Rubber,
            place(
                turned(
                    &[
                        (0.0, 0.0),
                        (0.017, 0.0),
                        (0.019, 0.02),
                        (0.019, 0.1),
                        (0.016, 0.12),
                        (0.0, 0.12),
                    ],
                    12,
                    Vec3::ZERO,
                ),
                Vec3::new(x, -0.1, 0.5),
                Quat::IDENTITY,
            ),
        );
    }
    // Ammunition box outboard of the left trunnion on a bracket, with a feed chute arcing
    // over the trunnion into the receiver.
    let can = Vec3::new(-0.255, -0.04, 0.12);
    mb.add(
        Mat::PaintGreen,
        place(
            rounded_box(Vec3::new(0.11, 0.17, 0.26), 0.01, 2, 0.2, BoxUv::PLANAR),
            can,
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.115, 0.012, 0.2), 0.004, 1, 0.2, BoxUv::PLANAR),
            can + Vec3::Y * 0.09,
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.2, 0.012, 0.1), 0.004, 1, 0.2, BoxUv::PLANAR),
            Vec3::new(-0.15, -0.115, 0.2),
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            rounded_box(Vec3::new(0.012, 0.08, 0.1), 0.004, 1, 0.2, BoxUv::PLANAR),
            Vec3::new(-0.06, -0.08, 0.2),
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        tube(
            &smooth_path(
                &[
                    can + Vec3::new(0.0, 0.09, -0.08),
                    Vec3::new(-0.24, 0.13, -0.04),
                    Vec3::new(-0.15, 0.16, 0.0),
                    Vec3::new(-0.06, 0.11, 0.02),
                ],
                6,
            ),
            |_| 0.02,
            8,
            false,
            true,
            1.0,
        ),
    );
    mb.build(commands)
}

// ---------------------------------------------------------------------------------------
// Burner

/// A propane bottle with a valve and regulator, a hose along the ground and a burner on a
/// small tripod aimed at the curtain's hem. Positions are relative to the stage center;
/// `nozzle` is where the flame should lick (the igniter point), `stand` the tripod's foot.
pub(super) fn burner(commands: &mut Commands, nozzle: Vec3, stand: Vec3, bottle: Vec3) -> Model {
    let mut mb = ModelBuilder::new("burner").grounded();
    mb.add(
        Mat::PaintRed,
        turned(
            &[
                (0.0, 0.0),
                (0.14, 0.0),
                (0.14, 0.0),
                (0.14, 0.055),
                (0.128, 0.062),
                (0.128, 0.062),
                (0.146, 0.075),
                (0.153, 0.11),
                (0.153, 0.47),
                (0.145, 0.51),
                (0.12, 0.545),
                (0.075, 0.565),
                (0.036, 0.57),
                (0.036, 0.6),
                (0.0, 0.6),
            ],
            32,
            bottle,
        ),
    );
    // Guard ring on three stays.
    mb.add(
        Mat::PaintRed,
        place(
            torus(0.095, 0.011, 28, 8),
            bottle + Vec3::Y * 0.68,
            Quat::IDENTITY,
        ),
    );
    for k in 0..3 {
        let a = k as f32 / 3.0 * TAU + 0.5;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        mb.add(
            Mat::PaintRed,
            tube(
                &[
                    bottle + d * 0.075 + Vec3::Y * 0.55,
                    bottle + d * 0.095 + Vec3::Y * 0.68,
                ],
                |_| 0.009,
                6,
                false,
                true,
                1.0,
            ),
        );
    }
    let valve = bottle + Vec3::Y * 0.6;
    mb.add(
        Mat::Brass,
        turned(
            &[
                (0.0, 0.0),
                (0.022, 0.0),
                (0.022, 0.045),
                (0.016, 0.055),
                (0.0, 0.055),
            ],
            12,
            valve,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        place(
            torus(0.026, 0.005, 16, 6),
            valve + Vec3::Y * 0.065,
            Quat::IDENTITY,
        ),
    );
    mb.add(
        Mat::BlackSteel,
        turned(
            &[(0.0, 0.055), (0.006, 0.055), (0.006, 0.07), (0.0, 0.07)],
            6,
            valve,
        ),
    );
    let reg = valve + Vec3::new(0.05, 0.03, 0.0);
    mb.add(
        Mat::Brass,
        place(
            turned(
                &[(0.0, -0.04), (0.02, -0.04), (0.02, 0.04), (0.0, 0.04)],
                12,
                Vec3::ZERO,
            ),
            reg,
            Quat::from_rotation_z(FRAC_PI_2),
        ),
    );
    mb.add(
        Mat::Brass,
        place(
            turned(
                &[(0.0, 0.0), (0.03, 0.0), (0.03, 0.025), (0.0, 0.03)],
                14,
                Vec3::ZERO,
            ),
            reg + Vec3::Y * 0.015,
            Quat::IDENTITY,
        ),
    );
    // Burner on a tripod, aimed at the hem.
    let head = stand + Vec3::Y * 0.2;
    let aim = (nozzle - head).normalize();
    for k in 0..3 {
        let a = k as f32 / 3.0 * TAU + 0.3;
        let foot = stand + Vec3::new(a.cos() * 0.14, 0.0, a.sin() * 0.14);
        mb.add(
            Mat::BlackSteel,
            tube(
                &[foot, head - Vec3::Y * 0.02 + (foot - stand) * 0.15],
                |_| 0.006,
                6,
                false,
                true,
                1.0,
            ),
        );
        mb.add_small(
            Mat::Rubber,
            place(sphere(0.012, 8), foot + Vec3::Y * 0.006, Quat::IDENTITY),
        );
    }
    mb.add(
        Mat::BlackSteel,
        place(
            torus(0.03, 0.006, 16, 6),
            head - Vec3::Y * 0.02,
            Quat::IDENTITY,
        ),
    );
    let rot = Quat::from_rotation_arc(Vec3::Y, aim);
    mb.add(
        Mat::BrightSteel,
        place(
            turned(
                &[
                    (0.0, -0.03),
                    (0.016, -0.03),
                    (0.016, 0.16),
                    (0.02, 0.18),
                    (0.03, 0.235),
                    (0.028, 0.24),
                    (0.0, 0.24),
                ],
                16,
                Vec3::ZERO,
            ),
            head,
            rot,
        ),
    );
    mb.add(
        Mat::Brass,
        place(
            turned(
                &[(0.0, -0.06), (0.02, -0.06), (0.02, -0.02), (0.0, -0.02)],
                12,
                Vec3::ZERO,
            ),
            head,
            rot,
        ),
    );
    mb.add(
        Mat::PaintRed,
        place(
            turned(
                &[(0.0, 0.0), (0.016, 0.0), (0.016, 0.02), (0.0, 0.022)],
                10,
                Vec3::ZERO,
            ),
            head + rot * Vec3::new(0.03, -0.04, 0.0),
            rot * Quat::from_rotation_z(-FRAC_PI_2),
        ),
    );
    // Hose from the regulator along the ground to the burner.
    let inlet = head + rot * Vec3::new(0.0, -0.06, 0.0);
    let hose = smooth_path(
        &[
            reg + Vec3::new(0.04, 0.0, 0.0),
            reg + Vec3::new(0.12, -0.08, 0.0),
            bottle + Vec3::new(0.2, 0.012, -0.12),
            bottle.lerp(stand, 0.45) + Vec3::new(0.05, 0.012, 0.15),
            stand.lerp(bottle, 0.15) + Vec3::new(0.0, 0.012, 0.0),
            inlet - aim * 0.12 - Vec3::Y * 0.1,
            inlet,
        ],
        8,
    );
    mb.add(Mat::Rubber, tube(&hose, |_| 0.009, 8, false, true, 1.0));
    mb.build(commands)
}

// ---------------------------------------------------------------------------------------
// Bead curtain

/// The bead-curtain doorframe: posts on sole plates with braces, a lintel with steel corner
/// plates, and a brass screw eye for every string (`string_xs`, at `string_y`).
pub(super) fn bead_frame(
    commands: &mut Commands,
    string_xs: &[f32],
    string_y: f32,
) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("bead_frame").grounded();
    let w = Mat::Timber;
    let mut colliders = Vec::new();
    for (i, x) in [-1.8f32, 1.8].into_iter().enumerate() {
        let s = 100 + i as u32 * 10;
        mb.add(
            w,
            board_at(Vec3::new(x, 0.07, 0.0), Vec3::new(0.2, 0.14, 1.3), s),
        );
        mb.add(
            w,
            board_at(
                Vec3::new(x, 0.14 + 1.33, 0.0),
                Vec3::new(0.18, 2.66, 0.18),
                s + 1,
            ),
        );
        for (k, z) in [-1.0f32, 1.0].into_iter().enumerate() {
            mb.add(
                w,
                beam(
                    Vec3::new(x, 0.1, z * 0.56),
                    Vec3::new(x, 0.98, z * 0.06),
                    0.09,
                    0.09,
                    Vec3::X,
                    s + 2 + k as u32,
                ),
            );
            mb.add_small(
                Mat::BlackSteel,
                bolt_heads(&[Vec3::new(x + 0.09, 0.85, z * 0.12)], Vec3::X, 0.012),
            );
        }
        colliders.push(part(
            Vec3::new(x, 1.4, 0.0),
            box_shape(Vec3::new(0.18, 2.8, 0.18)),
        ));
        colliders.push(part(
            Vec3::new(x, 0.07, 0.0),
            box_shape(Vec3::new(0.2, 0.14, 1.3)),
        ));
        // Corner plates on both faces.
        for zs in [-1.0f32, 1.0] {
            let z = zs * 0.093;
            let plate = |size: Vec3, at: Vec3| {
                place(
                    rounded_box(size, 0.003, 1, 0.2, BoxUv::PLANAR),
                    at,
                    Quat::IDENTITY,
                )
            };
            mb.add(
                Mat::BlackSteel,
                plate(Vec3::new(0.08, 0.3, 0.006), Vec3::new(x, 2.62, z)),
            );
            let hx = x - x.signum() * 0.16;
            mb.add(
                Mat::BlackSteel,
                plate(Vec3::new(0.4, 0.08, 0.006), Vec3::new(hx, 2.86, zs * 0.103)),
            );
            let heads = [
                Vec3::new(x, 2.52, z + zs * 0.003),
                Vec3::new(x, 2.7, z + zs * 0.003),
                Vec3::new(hx - 0.12, 2.86, zs * 0.106),
                Vec3::new(hx + 0.12, 2.86, zs * 0.106),
            ];
            mb.add_small(Mat::BlackSteel, bolt_heads(&heads, Vec3::Z * zs, 0.011));
        }
    }
    mb.add(
        w,
        board_at(Vec3::new(0.0, 2.91, 0.0), Vec3::new(4.1, 0.22, 0.2), 130),
    );
    colliders.push(part(
        Vec3::new(0.0, 2.91, 0.0),
        box_shape(Vec3::new(4.1, 0.22, 0.2)),
    ));
    let ring = torus(0.008, 0.0019, 10, 6);
    for &x in string_xs {
        mb.add_small(
            Mat::Brass,
            turned(
                &[(0.0, 0.0), (0.003, 0.0), (0.003, 0.012), (0.0, 0.012)],
                6,
                Vec3::new(x, 2.788, 0.0),
            ),
        );
        mb.add_small(
            Mat::Brass,
            place(
                ring.clone(),
                Vec3::new(x, string_y + 0.008, 0.0),
                Quat::from_rotation_x(FRAC_PI_2),
            ),
        );
    }
    (mb.build(commands), ColliderShape::Compound(colliders))
}

/// The ball that rolls through the bead curtain: a carved sandstone sphere with two crossed
/// grooves (so its rolling reads), slightly irregular like hand-dressed stone.
pub(super) fn stone_ball(commands: &mut Commands, radius: f32) -> Model {
    let mut mb = ModelBuilder::new("stone_ball");
    let groove = |v: Vec3, n: Vec3| {
        let d = v.dot(n) / radius;
        let g = (-(d / 0.035).powi(2)).exp();
        1.0 - 0.018 * g
    };
    let mut m = sphere(radius, 56);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let s = groove(p, Vec3::X) * groove(p, Vec3::Z);
        v.position = (p * s).to_array();
    }
    let m = smooth_normals(jitter(m, 0.004, 3.0, 77));
    mb.add(Mat::Stone, m);
    mb.build(commands)
}

// ---------------------------------------------------------------------------------------
// Swing

/// The swing's A-frame: a top beam on two splayed timber A's with cross ties, galvanized apex
/// plates, U-bolt hangers with swivel eyes at `hanger_xs` (eyes end at `pivot_y`) and ground
/// anchors.
pub(super) fn swing_frame(
    commands: &mut Commands,
    hanger_xs: &[f32],
    pivot_y: f32,
) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("swing_frame").grounded();
    let w = Mat::Timber;
    let beam_y = pivot_y + 0.2;
    mb.add(
        w,
        board_at(Vec3::new(0.0, beam_y, 0.0), Vec3::new(3.4, 0.2, 0.2), 200),
    );
    let mut colliders = vec![part(
        Vec3::new(0.0, beam_y, 0.0),
        box_shape(Vec3::new(3.4, 0.2, 0.2)),
    )];
    for (i, sx) in [-1.0f32, 1.0].into_iter().enumerate() {
        let top_x = sx * 1.42;
        for (k, sz) in [-1.0f32, 1.0].into_iter().enumerate() {
            let foot = Vec3::new(sx * 1.48, 0.0, sz * 1.3);
            let top = Vec3::new(top_x, beam_y - 0.02, sz * 0.07);
            mb.add(
                w,
                beam(
                    foot - (top - foot).normalize() * 0.05,
                    top,
                    0.13,
                    0.13,
                    Vec3::X,
                    210 + (i * 2 + k) as u32,
                ),
            );
            let rot = frame_rot(top - foot, Vec3::X);
            colliders.push(part_rot(
                (foot + top) * 0.5,
                rot,
                box_shape(Vec3::new(0.13, foot.distance(top), 0.13)),
            ));
            mb.add(
                Mat::Galvanized,
                place(
                    rounded_box(Vec3::new(0.22, 0.012, 0.22), 0.004, 1, 0.3, BoxUv::PLANAR),
                    foot + Vec3::Y * 0.006,
                    Quat::IDENTITY,
                ),
            );
            mb.add(
                Mat::Galvanized,
                place(
                    rounded_box(Vec3::new(0.008, 0.3, 0.12), 0.003, 1, 0.3, BoxUv::PLANAR),
                    foot + Vec3::new(sx * 0.072, 0.15, -sz * 0.04),
                    Quat::from_rotation_x(sz * 0.29),
                ),
            );
            mb.add_small(
                Mat::Galvanized,
                nuts(
                    &[
                        foot + Vec3::new(sx * 0.08, 0.1, -sz * 0.02),
                        foot + Vec3::new(sx * 0.08, 0.2, -sz * 0.05),
                    ],
                    Vec3::X * sx,
                    0.012,
                ),
            );
        }
        // Cross tie at 1.5 m.
        let y = 1.5;
        let t = y / (beam_y - 0.02);
        let z = 1.3 + (0.07 - 1.3) * t;
        let x = sx * (1.48 + (1.42 - 1.48) * t) + sx * 0.09;
        mb.add(
            w,
            board_at(
                Vec3::new(x, y, 0.0),
                Vec3::new(0.05, 0.15, 2.0 * z + 0.3),
                220 + i as u32,
            ),
        );
        mb.add_small(
            Mat::BlackSteel,
            bolt_heads(
                &[
                    Vec3::new(x + sx * 0.025, y, z),
                    Vec3::new(x + sx * 0.025, y, -z),
                ],
                Vec3::X * sx,
                0.013,
            ),
        );
        // Apex plate on the outside of each A.
        let plate = extrude(
            &round_corners(
                &[
                    Vec2::new(-0.36, beam_y - 0.75),
                    Vec2::new(0.36, beam_y - 0.75),
                    Vec2::new(0.12, beam_y + 0.1),
                    Vec2::new(-0.12, beam_y + 0.1),
                ],
                |_| 0.03,
                3,
            ),
            0.01,
            0.002,
            30.0,
        );
        let px = sx * (1.42 + 0.075);
        mb.add(
            Mat::Galvanized,
            place(
                plate,
                Vec3::new(px, 0.0, 0.0),
                Quat::from_rotation_y(FRAC_PI_2),
            ),
        );
        let pts = [
            Vec3::new(px + sx * 0.005, beam_y, 0.0),
            Vec3::new(px + sx * 0.005, beam_y - 0.5, 0.2),
            Vec3::new(px + sx * 0.005, beam_y - 0.5, -0.2),
        ];
        mb.add_small(Mat::Galvanized, nuts(&pts, Vec3::X * sx, 0.016));
    }
    // Hangers: U-bolt over the beam, plate under it, swivel eye below.
    for &x in hanger_xs {
        let u = smooth_path(
            &[
                Vec3::new(x, beam_y - 0.1, -0.112),
                Vec3::new(x, beam_y + 0.07, -0.112),
                Vec3::new(x, beam_y + 0.112, 0.0),
                Vec3::new(x, beam_y + 0.07, 0.112),
                Vec3::new(x, beam_y - 0.1, 0.112),
            ],
            6,
        );
        mb.add(Mat::Galvanized, tube(&u, |_| 0.008, 8, false, true, 1.0));
        mb.add(
            Mat::Galvanized,
            place(
                rounded_box(Vec3::new(0.06, 0.016, 0.28), 0.004, 1, 0.3, BoxUv::PLANAR),
                Vec3::new(x, beam_y - 0.108, 0.0),
                Quat::IDENTITY,
            ),
        );
        mb.add_small(
            Mat::Galvanized,
            nuts(
                &[
                    Vec3::new(x, beam_y - 0.116, -0.112),
                    Vec3::new(x, beam_y - 0.116, 0.112),
                ],
                Vec3::NEG_Y,
                0.012,
            ),
        );
        mb.add(
            Mat::Galvanized,
            place(
                torus(0.016, 0.005, 14, 6),
                Vec3::new(x, pivot_y + 0.068, 0.0),
                Quat::from_rotation_x(FRAC_PI_2),
            ),
        );
        mb.add(
            Mat::Galvanized,
            place(
                torus(0.026, 0.0055, 16, 6),
                Vec3::new(x, pivot_y + 0.026, 0.0),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        );
    }
    (mb.build(commands), ColliderShape::Compound(colliders))
}

/// The swing seat (kinematic, origin at its center, top at +0.03): two slats on cleats with
/// galvanized eye bolts where the ropes tie on (`rope_xs`).
pub(super) fn swing_seat(commands: &mut Commands, rope_xs: &[f32]) -> Model {
    let mut mb = ModelBuilder::new("swing_seat");
    for (k, z) in [-0.0925f32, 0.0925].into_iter().enumerate() {
        mb.add(
            Mat::Pine,
            board_at(
                Vec3::new(0.0, 0.0, z),
                Vec3::new(0.7, 0.06, 0.17),
                300 + k as u32,
            ),
        );
    }
    for (k, &x) in rope_xs.iter().enumerate() {
        mb.add(
            Mat::Pine,
            board_at(
                Vec3::new(x, -0.045, 0.0),
                Vec3::new(0.07, 0.03, 0.36),
                310 + k as u32,
            ),
        );
        mb.add_small(
            Mat::Galvanized,
            place(
                dome(0.02, 0.006, 10),
                Vec3::new(x, 0.03, 0.0),
                Quat::IDENTITY,
            ),
        );
        mb.add(
            Mat::Galvanized,
            place(
                torus(0.02, 0.005, 14, 6),
                Vec3::new(x, 0.03 + 0.024, 0.0),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        );
        mb.add_small(
            Mat::Galvanized,
            nuts(&[Vec3::new(x, -0.06, 0.0)], Vec3::NEG_Y, 0.012),
        );
    }
    mb.build(commands)
}
