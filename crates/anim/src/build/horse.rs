//! A procedural horse: four hoofed legs on a [`LeggedGait`] with walk, trot, canter and
//! gallop, a nodding neck, a tail on springs, and muscles (hindquarters, shoulders, forearms,
//! gaskins, neck) that flex with each leg's load and the gait's intensity.

use engine_core::prelude::*;

use super::quadruped::{Leg, Rig, Stance, action_clip};
use super::sdf::{Body, Paint, Shape};
use super::{PoseBuilder, bone, cycle_sin};
use crate::clip::AnimationClip;
use crate::graph::{AnimGraph, NodeDef};
use crate::muscle::{Muscle, MuscleDriver};
use crate::procedural::gait::{FootKind, GaitLeg, GaitStage, GaitSway, LegStyle, LeggedGait};
use crate::procedural::spring::SpringChainDef;
use crate::skeleton::Skeleton;
use crate::skin::SkinnedMesh;

pub const SKELETON: &str = "horse";
pub const GRAPH: &str = "horse";
const SIDE: [&str; 2] = ["l", "r"];
const X: [f32; 2] = [-0.2, 0.2];

/// Joint positions (model space): shoulder/hip, elbow/stifle, knee/hock, fetlock.
fn front_leg(side: usize) -> [Vec3; 4] {
    let x = X[side];
    [
        Vec3::new(x, 1.2, -0.78),
        Vec3::new(x * 1.05, 0.93, -0.66),
        Vec3::new(x, 0.52, -0.72),
        Vec3::new(x, 0.18, -0.72),
    ]
}

fn hind_leg(side: usize) -> [Vec3; 4] {
    let x = X[side] * 1.05;
    [
        Vec3::new(x, 1.26, 0.7),
        Vec3::new(x * 1.08, 0.9, 0.5),
        Vec3::new(x, 0.56, 0.84),
        Vec3::new(x, 0.18, 0.8),
    ]
}

fn name(prefix: &str, part: &str, side: usize) -> String {
    format!("{prefix}_{part}_{}", SIDE[side])
}

pub fn skeleton() -> Skeleton {
    let i = Quat::IDENTITY;
    let body = Vec3::new(0.0, 1.25, 0.0);
    let neck = [
        Vec3::new(0.0, 1.45, -0.85),
        Vec3::new(0.0, 1.75, -1.08),
        Vec3::new(0.0, 1.98, -1.27),
        Vec3::new(0.0, 2.1, -1.42),
    ];
    let tail = [
        Vec3::new(0.0, 1.43, 1.0),
        Vec3::new(0.0, 1.3, 1.13),
        Vec3::new(0.0, 1.0, 1.2),
        Vec3::new(0.0, 0.68, 1.22),
    ];
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("body", Some("root"), body, i),
        bone("neck_1", Some("body"), neck[0] - body, i),
        bone("neck_2", Some("neck_1"), neck[1] - neck[0], i),
        bone("neck_3", Some("neck_2"), neck[2] - neck[1], i),
        bone("head", Some("neck_3"), neck[3] - neck[2], i),
        bone("tail_1", Some("body"), tail[0] - body, i),
        bone("tail_2", Some("tail_1"), tail[1] - tail[0], i),
        bone("tail_3", Some("tail_2"), tail[2] - tail[1], i),
        bone("tail_4", Some("tail_3"), tail[3] - tail[2], i),
    ];
    for side in 0..2 {
        for (prefix, j) in [("front", front_leg(side)), ("hind", hind_leg(side))] {
            let upper = name(prefix, "upper", side);
            let lower = name(prefix, "lower", side);
            let cannon = name(prefix, "cannon", side);
            bones.push(bone(&upper, Some("body"), j[0] - body, i));
            bones.push(bone(&lower, Some(&upper), j[1] - j[0], i));
            bones.push(bone(&cannon, Some(&lower), j[2] - j[1], i));
            bones.push(bone(
                &name(prefix, "hoof", side),
                Some(&cannon),
                j[3] - j[2],
                i,
            ));
        }
    }
    Skeleton::new(SKELETON, bones).expect("horse skeleton")
}

pub fn body() -> Body {
    let coat = Paint::hide([0.27, 0.11, 0.04]);
    // Veins show on the working legs, not across the barrel.
    let trunk = Paint {
        mask: Vec4::new(0.0, 0.6, 0.0, 1.0),
        ..coat
    };
    let dark = Paint::hide([0.018, 0.013, 0.011]);
    let muzzle = Paint::hide([0.06, 0.035, 0.025]);
    let hoof = Paint::hard([0.035, 0.03, 0.028]);
    let blaze = Paint::hide([0.75, 0.73, 0.7]);
    let eye = Paint::hard([0.008, 0.006, 0.005]);
    let mut b = Body::default();
    // Trunk.
    b.part(
        "body",
        Shape::ellipsoid(Vec3::new(0.0, 1.22, 0.0), Vec3::new(0.31, 0.35, 0.82)),
        trunk,
        0.12,
    )
    .part(
        "body",
        Shape::ellipsoid(Vec3::new(0.0, 1.18, -0.6), Vec3::new(0.29, 0.37, 0.36)),
        trunk,
        0.12,
    )
    .part(
        "body",
        Shape::ellipsoid(Vec3::new(0.0, 1.3, 0.62), Vec3::new(0.32, 0.32, 0.38)),
        trunk,
        0.12,
    )
    .part(
        "body",
        Shape::ellipsoid(Vec3::new(0.0, 1.49, -0.55), Vec3::new(0.13, 0.13, 0.32)),
        trunk,
        0.1,
    )
    // Neck, head.
    .part(
        "neck_1",
        Shape::capsule(
            Vec3::new(0.0, 1.38, -0.78),
            Vec3::new(0.0, 1.75, -1.08),
            0.25,
            0.17,
        ),
        trunk,
        0.1,
    )
    .part(
        "neck_2",
        Shape::capsule(
            Vec3::new(0.0, 1.75, -1.08),
            Vec3::new(0.0, 1.98, -1.27),
            0.17,
            0.13,
        ),
        trunk,
        0.06,
    )
    .part(
        "neck_3",
        Shape::capsule(
            Vec3::new(0.0, 1.98, -1.27),
            Vec3::new(0.0, 2.08, -1.4),
            0.13,
            0.12,
        ),
        trunk,
        0.05,
    )
    .part(
        "head",
        Shape::ellipsoid(Vec3::new(0.0, 2.08, -1.47), Vec3::new(0.115, 0.13, 0.16)),
        trunk,
        0.05,
    )
    .part(
        "head",
        Shape::capsule(
            Vec3::new(0.0, 2.0, -1.55),
            Vec3::new(0.0, 1.73, -1.84),
            0.105,
            0.075,
        ),
        trunk,
        0.06,
    )
    .part(
        "head",
        Shape::ellipsoid(Vec3::new(0.0, 1.72, -1.86), Vec3::new(0.08, 0.07, 0.085)),
        muzzle,
        0.03,
    )
    .part(
        "head",
        Shape::capsule(
            Vec3::new(0.0, 2.1, -1.6),
            Vec3::new(0.0, 1.88, -1.8),
            0.035,
            0.03,
        ),
        blaze,
        0.01,
    );
    // Mane: a crest of dark hair along the top of the neck.
    for (k, (bone_name, a, b2)) in [
        (
            "neck_1",
            Vec3::new(0.0, 1.62, -0.78),
            Vec3::new(0.0, 1.9, -1.02),
        ),
        (
            "neck_2",
            Vec3::new(0.0, 1.9, -1.02),
            Vec3::new(0.0, 2.1, -1.22),
        ),
        (
            "neck_3",
            Vec3::new(0.0, 2.1, -1.22),
            Vec3::new(0.0, 2.22, -1.38),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let r = [0.06, 0.05, 0.04][k];
        b.part(bone_name, Shape::capsule(a, b2, r, r * 0.9), dark, 0.02);
    }
    // Tail.
    let tail = [
        Vec3::new(0.0, 1.43, 1.0),
        Vec3::new(0.0, 1.3, 1.13),
        Vec3::new(0.0, 1.0, 1.2),
        Vec3::new(0.0, 0.68, 1.22),
        Vec3::new(0.0, 0.42, 1.22),
    ];
    for (k, r) in [
        (0usize, (0.07, 0.07)),
        (1, (0.07, 0.1)),
        (2, (0.1, 0.085)),
        (3, (0.085, 0.04)),
    ] {
        b.part(
            &format!("tail_{}", k + 1),
            Shape::capsule(tail[k], tail[k + 1], r.0, r.1),
            dark,
            0.03,
        );
    }
    for side in 0..2 {
        let x = X[side];
        b.part(
            "head",
            Shape::ellipsoid(Vec3::new(x * 0.55, 2.07, -1.56), Vec3::splat(0.026)),
            eye,
            0.008,
        )
        .part(
            "head",
            Shape::capsule(
                Vec3::new(x * 0.3, 2.2, -1.43),
                Vec3::new(x * 0.38, 2.34, -1.4),
                0.032,
                0.01,
            ),
            coat,
            0.02,
        )
        .part(
            "head",
            Shape::ellipsoid(Vec3::new(x * 0.2, 1.71, -1.93), Vec3::splat(0.018)),
            eye,
            0.006,
        );
        // Front leg.
        let [sh, el, kn, fe] = front_leg(side);
        let f = |part: &str| name("front", part, side);
        // The shoulder blade's mass rides on the trunk; the leg swings below it.
        b.part(
            "body",
            Shape::ellipsoid(
                sh + Vec3::new(x * 0.1, -0.05, 0.02),
                Vec3::new(0.12, 0.3, 0.19),
            ),
            trunk,
            0.1,
        )
        .part(&f("upper"), Shape::capsule(sh, el, 0.13, 0.1), coat, 0.06)
        .part(&f("lower"), Shape::capsule(el, kn, 0.085, 0.05), coat, 0.05)
        .part(
            &f("lower"),
            Shape::ellipsoid(
                el + Vec3::new(0.0, -0.14, -0.02),
                Vec3::new(0.075, 0.15, 0.08),
            ),
            coat,
            0.04,
        )
        .part(
            &f("cannon"),
            Shape::ellipsoid(kn, Vec3::new(0.055, 0.06, 0.055)),
            dark,
            0.03,
        )
        .part(
            &f("cannon"),
            Shape::capsule(kn, fe, 0.042, 0.038),
            dark,
            0.02,
        )
        .part(
            &f("hoof"),
            Shape::ellipsoid(fe, Vec3::new(0.05, 0.055, 0.055)),
            dark,
            0.02,
        )
        .part(
            &f("hoof"),
            Shape::capsule(fe, fe + Vec3::new(0.0, -0.1, -0.045), 0.042, 0.048),
            dark,
            0.02,
        )
        .part(
            &f("hoof"),
            Shape::cylinder(
                fe + Vec3::new(0.0, -0.135, -0.055),
                Vec3::Y,
                0.066,
                0.045,
                0.015,
            ),
            hoof,
            0.015,
        );
        // Hind leg.
        let [hip, st, ho, fe] = hind_leg(side);
        let h = |part: &str| name("hind", part, side);
        // Likewise the haunch: the pelvis carries it, only the thigh below swings.
        b.part(
            "body",
            Shape::ellipsoid(
                hip + Vec3::new(x * 0.05, -0.1, -0.02),
                Vec3::new(0.16, 0.3, 0.27),
            ),
            trunk,
            0.12,
        )
        .part(&h("upper"), Shape::capsule(hip, st, 0.16, 0.12), coat, 0.08)
        .part(&h("lower"), Shape::capsule(st, ho, 0.11, 0.055), coat, 0.05)
        .part(
            &h("lower"),
            Shape::ellipsoid(st + Vec3::new(0.0, -0.1, 0.07), Vec3::new(0.08, 0.15, 0.09)),
            coat,
            0.04,
        )
        .part(
            &h("cannon"),
            Shape::ellipsoid(ho, Vec3::new(0.05, 0.065, 0.075)),
            coat,
            0.03,
        )
        .part(
            &h("cannon"),
            Shape::capsule(ho + Vec3::new(0.0, -0.06, 0.0), fe, 0.043, 0.038),
            dark,
            0.02,
        )
        .part(
            &h("hoof"),
            Shape::ellipsoid(fe, Vec3::new(0.05, 0.055, 0.055)),
            dark,
            0.02,
        )
        .part(
            &h("hoof"),
            Shape::capsule(fe, fe + Vec3::new(0.0, -0.1, -0.045), 0.042, 0.048),
            dark,
            0.02,
        )
        .part(
            &h("hoof"),
            Shape::cylinder(
                fe + Vec3::new(0.0, -0.135, -0.055),
                Vec3::Y,
                0.066,
                0.045,
                0.015,
            ),
            hoof,
            0.015,
        );
        // Muscles.
        let n = |m: &str| format!("{m}_{}", SIDE[side]);
        b.muscle(
            &n("haunch"),
            Vec3::new(x * 1.15, 1.18, 0.64),
            Vec3::new(0.16, 0.3, 0.3),
            0.028,
            Vec3::new(0.0, -0.01, 0.0),
        )
        .muscle(
            &n("shoulder"),
            Vec3::new(x * 1.2, 1.2, -0.7),
            Vec3::new(0.13, 0.28, 0.2),
            0.02,
            Vec3::new(0.0, -0.006, 0.0),
        )
        .muscle(
            &n("forearm"),
            el + Vec3::new(x * 0.2, -0.14, -0.02),
            Vec3::new(0.08, 0.17, 0.09),
            0.014,
            Vec3::ZERO,
        )
        .muscle(
            &n("gaskin"),
            st + Vec3::new(x * 0.2, -0.12, 0.08),
            Vec3::new(0.09, 0.17, 0.1),
            0.014,
            Vec3::ZERO,
        );
    }
    b.muscle(
        "neck",
        Vec3::new(0.0, 1.66, -1.02),
        Vec3::new(0.25, 0.22, 0.22),
        0.014,
        Vec3::ZERO,
    )
    .muscle(
        "belly",
        Vec3::new(0.0, 0.92, 0.0),
        Vec3::new(0.3, 0.18, 0.55),
        0.004,
        Vec3::new(0.0, -0.03, 0.0),
    );
    b
}

pub fn mesh() -> SkinnedMesh {
    body().build(&skeleton(), 0.022, 0.04)
}

/// Legs in the order front left, front right, hind left, hind right.
pub fn gait() -> LeggedGait {
    let mut legs = Vec::new();
    for prefix in ["front", "hind"] {
        for side in 0..2 {
            legs.push(GaitLeg {
                bones: ["upper", "lower", "cannon", "hoof"]
                    .iter()
                    .map(|p| name(prefix, p, side))
                    .collect(),
                style: LegStyle::Hoofed,
                phase: 0.0,
                pole: if prefix == "front" {
                    Vec3::Z
                } else {
                    Vec3::NEG_Z
                },
                kind: FootKind::Hoof,
                foot_size: [0.13, 0.15],
                sole: 0.18,
                stance_offset: Vec3::ZERO,
                release: RIG.release[legs.len()].into(),
            });
        }
    }
    let stage = |name: &str, min_speed: f32, offsets: [f32; 4], duty: f32, bob: f32, pitch: f32| {
        GaitStage {
            name: name.into(),
            min_speed,
            offsets: offsets.to_vec(),
            duty,
            bob,
            pitch_deg: pitch,
        }
    };
    LeggedGait {
        body: "body".into(),
        legs,
        stages: vec![
            // Four-beat walk: LH, LF, RH, RF.
            stage("walk", 0.0, [0.25, 0.75, 0.0, 0.5], 0.64, 0.02, 1.5),
            // Two-beat trot: diagonal pairs.
            stage("trot", 2.4, [0.0, 0.5, 0.5, 0.0], 0.45, 0.05, 1.0),
            // Three-beat canter on the left lead: RH, then LH + RF, then LF.
            stage("canter", 5.0, [0.58, 0.32, 0.32, 0.0], 0.4, 0.07, 5.0),
            // Four-beat transverse gallop.
            stage("gallop", 8.5, [0.6, 0.48, 0.12, 0.0], 0.3, 0.06, 6.0),
        ],
        base_frequency: 0.75,
        frequency_per_speed: 0.13,
        step_height: 0.1,
        step_height_per_speed: 0.022,
        body_follow_ground: 0.8,
        body_tilt: 0.8,
        full_speed: 11.0,
        turn_radius: 1.2,
        mass_kg: 500.0,
        // The neck pumps once a stride, lowest as the leading foreleg takes the weight
        // (mid-stance at about 0.75 of a canter or gallop stride) and highest through the
        // hind push and suspension; the head counters part of it so the face stays steadier.
        sway: vec![
            GaitSway {
                bone: "neck_1".into(),
                axis: Vec3::X,
                amplitude_deg: 11.0,
                frequency: 1.0,
                phase: 0.0,
            },
            GaitSway {
                bone: "neck_2".into(),
                axis: Vec3::X,
                amplitude_deg: 4.0,
                frequency: 1.0,
                phase: 0.03,
            },
            GaitSway {
                bone: "head".into(),
                axis: Vec3::X,
                amplitude_deg: -7.0,
                frequency: 1.0,
                phase: 0.05,
            },
        ],
        ..Default::default()
    }
}

/// Muscles matching [`mesh`]'s targets. Leg loads come from the gait (`gait.load.N`).
pub fn muscles() -> Vec<Muscle> {
    let input = |name: &str, weight: f32| MuscleDriver::Input {
        name: name.into(),
        weight,
    };
    let muscle = |name: &str, bone: &str, drivers: Vec<MuscleDriver>| Muscle {
        name: name.into(),
        bulge: name.into(),
        sway: Some(format!("{name}.sway")),
        drivers,
        bone: bone.into(),
        rise_secs: 0.06,
        fall_secs: 0.35,
        ..Default::default()
    };
    let mut out = Vec::new();
    for (side, suffix) in SIDE.iter().enumerate() {
        let n = |m: &str| format!("{m}_{suffix}");
        let front_load = format!("gait.load.{side}");
        let hind_load = format!("gait.load.{}", side + 2);
        out.extend([
            muscle(
                &n("haunch"),
                &name("hind", "upper", side),
                vec![input(&hind_load, 0.55), input("gait.intensity", 0.55)],
            ),
            muscle(
                &n("shoulder"),
                &name("front", "upper", side),
                vec![input(&front_load, 0.45), input("gait.intensity", 0.45)],
            ),
            muscle(
                &n("forearm"),
                &name("front", "lower", side),
                vec![input(&front_load, 0.6), input("gait.intensity", 0.3)],
            ),
            muscle(
                &n("gaskin"),
                &name("hind", "lower", side),
                vec![input(&hind_load, 0.6), input("gait.intensity", 0.3)],
            ),
        ]);
    }
    out.push(muscle("neck", "neck_1", vec![input("gait.intensity", 0.7)]));
    out.push(Muscle {
        jiggle_hz: 2.5,
        jiggle_damping: 0.12,
        tone: 0.0,
        ..muscle("belly", "body", Vec::new())
    });
    out
}

pub fn tail() -> SpringChainDef {
    SpringChainDef {
        bones: vec![
            "tail_1".into(),
            "tail_2".into(),
            "tail_3".into(),
            "tail_4".into(),
        ],
        stiffness: 0.05,
        damping: 0.88,
        gravity: 0.7,
        wind: 0.6,
        tip_length: 0.26,
    }
}

/// Head and neck idle: breathing, looking around, ear-flick nod.
fn idle(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("horse_idle", 6.0, 61, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let look = cycle_sin(ph);
        let breath = cycle_sin(ph * 3.0);
        p.rot("neck_1", 1.5 * breath, 3.0 * look, 0.0)
            .rot("neck_2", 0.0, 4.0 * look, 1.0 * look)
            .rot("neck_3", -breath, 4.0 * look, 0.0)
            .rot("head", 2.0 * cycle_sin(ph * 2.0), 5.0 * look, -2.0 * look);
        p.take()
    })
}

pub fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    let mut out = vec![idle(s)];
    out.extend(actions(s));
    out
}

/// What the horse's actions move.
const RIG: Rig = Rig {
    legs: [
        &[
            "front_upper_l",
            "front_lower_l",
            "front_cannon_l",
            "front_hoof_l",
        ],
        &[
            "front_upper_r",
            "front_lower_r",
            "front_cannon_r",
            "front_hoof_r",
        ],
        &[
            "hind_upper_l",
            "hind_lower_l",
            "hind_cannon_l",
            "hind_hoof_l",
        ],
        &[
            "hind_upper_r",
            "hind_lower_r",
            "hind_cannon_r",
            "hind_hoof_r",
        ],
    ],
    release: [
        "release.front_l",
        "release.front_r",
        "release.hind_l",
        "release.hind_r",
    ],
    neck: &["neck_1", "neck_2", "neck_3", "head"],
    jaw: None,
    tail: &["tail_1", "tail_2", "tail_3", "tail_4"],
    loin: None,
};

/// The horse's actions ([`crate::actions`]): `horse_graze`, `horse_drink`, `horse_rear`,
/// `horse_shake` and `horse_paw`.
pub const ACTIONS: [&str; 5] = ["graze", "drink", "rear", "shake", "paw"];

/// Where the muzzle is in the head bone's frame (the head bone sits at the poll), m.
pub const MUZZLE: Vec3 = Vec3::new(0.0, -0.34, -0.42);

fn actions(s: &Skeleton) -> Vec<AnimationClip> {
    let rest = Stance::REST;
    // Head down to the grass, nibbling and moving on a little, then up.
    let down = Stance {
        body: Vec3::new(0.0, -0.03, 0.0),
        pitch: -4.0,
        neck: [-58.0, -22.0, -12.0, -18.0],
        tail: -4.0,
        ..rest
    };
    let nibble = |at: f32, yaw: f32, nod: f32| Stance {
        neck_yaw: [yaw, yaw * 0.6, yaw * 0.3, 0.0],
        neck: [-58.0, -22.0, -12.0, -18.0 + nod],
        ..down.at(at)
    };
    let graze = action_clip(
        &RIG,
        s,
        1.0,
        "horse_graze",
        &[
            rest.at(0.0),
            down.at(1.3),
            nibble(2.2, 4.0, -5.0),
            nibble(3.0, 6.0, 2.0),
            nibble(3.8, -3.0, -6.0),
            nibble(4.8, -5.0, 3.0),
            nibble(5.6, 0.0, -2.0),
            rest.at(7.0),
        ],
        &[(1.0, 0.0), (1.3, 1.0), (5.6, 1.0), (6.1, 0.0)],
    );
    // Head down to a trough's water, drinking in pulls, then up.
    let low = Stance {
        neck: [-38.0, -16.0, -8.0, -20.0],
        pitch: -2.0,
        ..rest
    };
    let pull = |at: f32, k: f32| Stance {
        neck: [-38.0 - 2.0 * k, -16.0, -8.0, -20.0 - 3.0 * k],
        ..low.at(at)
    };
    let drink = action_clip(
        &RIG,
        s,
        1.0,
        "horse_drink",
        &[
            rest.at(0.0),
            low.at(1.1),
            pull(1.9, 1.0),
            pull(2.5, 0.0),
            pull(3.1, 1.0),
            pull(3.7, 0.0),
            pull(4.3, 1.0),
            low.at(5.0),
            rest.at(6.4),
        ],
        &[(0.8, 0.0), (1.1, 1.0), (5.0, 1.0), (5.5, 0.0)],
    );
    // Gathering onto the haunches, up on the hind legs with the forelegs folded and
    // pawing the air, then down again.
    let folded = |lift: f32, reach: f32| Leg::Free {
        joint: Vec3::new(0.2, 1.25 + lift, -0.75 - reach),
        cannon: Vec3::new(0.0, -0.75, 0.6),
        end: 40.0,
        pole: Vec3::Z,
    };
    let mirror = |l: Leg| match l {
        Leg::Free {
            joint,
            cannon,
            end,
            pole,
        } => Leg::Free {
            joint: Vec3::new(-joint.x, joint.y, joint.z),
            cannon,
            end,
            pole,
        },
        l => l,
    };
    let gather = Stance {
        body: Vec3::new(0.0, -0.08, 0.12),
        pitch: 6.0,
        neck: [8.0, 4.0, 0.0, 6.0],
        ..rest
    };
    let up = |at: f32, a: f32, b: f32| Stance {
        at,
        body: Vec3::new(0.0, -0.05, 0.42),
        pitch: 42.0,
        neck: [14.0, 6.0, 4.0, 14.0],
        tail: -12.0,
        legs: [
            mirror(folded(a, 0.05 * a)),
            folded(b, 0.05 * b),
            Leg::Planted,
            Leg::Planted,
        ],
        ..rest
    };
    let rear = action_clip(
        &RIG,
        s,
        1.0,
        "horse_rear",
        &[
            rest.at(0.0),
            gather.at(0.45),
            up(1.0, 0.1, 0.0),
            up(1.35, -0.05, 0.12),
            up(1.7, 0.1, -0.02),
            gather.at(2.25),
            rest.at(2.8),
        ],
        &[(0.0, 0.0)],
    );
    // Shaking the head and neck from side to side.
    let shake_to = |at: f32, side: f32| Stance {
        neck_yaw: [6.0 * side, 14.0 * side, 10.0 * side, 8.0 * side],
        head_roll: 18.0 * side,
        roll: -2.0 * side,
        neck: [4.0, 0.0, 0.0, -6.0],
        ..rest.at(at)
    };
    let shake = action_clip(
        &RIG,
        s,
        1.0,
        "horse_shake",
        &[
            rest.at(0.0),
            shake_to(0.25, 1.0),
            shake_to(0.5, -1.0),
            shake_to(0.72, 1.0),
            shake_to(0.92, -0.8),
            shake_to(1.1, 0.4),
            rest.at(1.5),
        ],
        &[(0.0, 0.0)],
    );
    // Pawing the ground with the right foreleg: lifted, reached forward and scraped back,
    // twice, the head down watching.
    let watch = Stance {
        neck: [-14.0, -6.0, -4.0, -10.0],
        ..rest
    };
    let fore = |at: f32, joint: Vec3, cannon: Vec3| Stance {
        legs: [
            Leg::Planted,
            Leg::Free {
                joint,
                cannon,
                end: 20.0,
                pole: Vec3::Z,
            },
            Leg::Planted,
            Leg::Planted,
        ],
        ..watch.at(at)
    };
    let lift = (Vec3::new(0.2, 0.72, -0.86), Vec3::new(0.0, -0.8, 0.55));
    let ahead = (Vec3::new(0.2, 0.6, -1.02), Vec3::new(0.0, -0.95, -0.3));
    let scrape = (Vec3::new(0.2, 0.53, -0.7), Vec3::new(0.0, -1.0, 0.12));
    let paw = action_clip(
        &RIG,
        s,
        1.0,
        "horse_paw",
        &[
            rest.at(0.0),
            fore(0.35, lift.0, lift.1),
            fore(0.65, ahead.0, ahead.1),
            fore(0.95, scrape.0, scrape.1),
            fore(1.25, lift.0, lift.1),
            fore(1.55, ahead.0, ahead.1),
            fore(1.85, scrape.0, scrape.1),
            watch.at(2.2),
            rest.at(2.6),
        ],
        &[(0.0, 0.0)],
    );
    vec![graze, drink, rear, shake, paw]
}

pub fn graph() -> AnimGraph {
    AnimGraph {
        name: GRAPH.into(),
        root: NodeDef::clip("horse_idle"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::muscle::MorphWeights;

    #[test]
    fn horse_stands_on_its_hooves() {
        let s = skeleton();
        for side in 0..2 {
            for prefix in ["front", "hind"] {
                let f = s.rest_model[s.bone(&name(prefix, "hoof", side))].t;
                assert!((f.y - 0.18).abs() < 1e-4);
            }
        }
        let mesh = mesh();
        assert!(mesh.triangle_count() > 6000, "{}", mesh.triangle_count());
        let lowest = mesh
            .vertices
            .iter()
            .map(|v| v.position.y)
            .fold(f32::MAX, f32::min);
        assert!(lowest.abs() < 0.02, "hooves end at {lowest}");
        let morphs = MorphWeights::new(mesh.morph_names());
        for m in muscles() {
            assert!(morphs.index_of(&m.bulge).is_some(), "{}", m.bulge);
        }
    }
}
