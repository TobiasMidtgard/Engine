//! A procedural human: skeleton, two outfits, muscles, foot IK, a ponytail on springs, and
//! clips for idle, walk, run, leaning into turns, a heavy overhead swing, a block, a two-arm
//! reach, a double-biceps flex and getting up off the floor (from the back and from the front),
//! wired into one locomotion graph.
//!
//! Conventions: Y up, the character faces −Z, its right side is +X. Arm bones are rotated in
//! their rest pose so that, for every arm bone, rotating about local X bends it forward
//! (shoulder flexion, elbow flexion) and about local Z swings it outward on the right side.
//! Leg bones have no rest rotation: +X lifts the thigh forward, −X bends the knee.

use engine_core::prelude::*;

use super::sdf::{Body, Paint, Shape};
use super::{PoseBuilder, bone, bump, cycle, cycle_sin, keyed};
use crate::clip::AnimationClip;
use crate::graph::{
    AnimGraph, Blend1dChild, BoneMaskDef, Cmp, Condition, NodeDef, StateDef, TransitionDef, Weight,
};
use crate::ik::{FootIk, LegIk};
use crate::muscle::{Muscle, MuscleDriver};
use crate::procedural::spring::SpringChainDef;
use crate::skeleton::Skeleton;
use crate::skin::SkinnedMesh;
use crate::xform::Xf;

pub const SKELETON: &str = "humanoid";
pub const GRAPH: &str = "humanoid";

/// Walk and run speeds the clips are authored for, m/s.
pub const WALK_SPEED: f32 = 1.4;
pub const SLOW_WALK_SPEED: f32 = 0.65;
/// Speeds of the sidestep and backpedal clips, m/s.
pub const SIDESTEP_SPEED: f32 = 0.6;
pub const BACKPEDAL_SPEED: f32 = 1.0;
/// Speeds of the jogging sidesteps and backward jog, between the steps and the runs, m/s.
pub const SIDE_JOG_SPEED: f32 = 1.4;
pub const BACK_JOG_SPEED: f32 = 1.7;
/// Speeds of the running sidesteps and the backward run, m/s.
pub const SIDE_RUN_SPEED: f32 = 3.0;
pub const BACK_RUN_SPEED: f32 = 2.6;
pub const RUN_SPEED: f32 = 3.8;
pub const SPRINT_SPEED: f32 = 7.0;
/// Height of the hips at rest, m (the clips' positions are for this size of body and scale
/// with the hips of a fitted skeleton).
pub const HIPS_HEIGHT: f32 = 0.98;

pub const CLAVICLE: [&str; 2] = ["clavicle_l", "clavicle_r"];
pub const UPPER_ARM: [&str; 2] = ["upper_arm_l", "upper_arm_r"];
pub const LOWER_ARM: [&str; 2] = ["lower_arm_l", "lower_arm_r"];
pub const HAND: [&str; 2] = ["hand_l", "hand_r"];
pub const UPPER_LEG: [&str; 2] = ["upper_leg_l", "upper_leg_r"];
pub const LOWER_LEG: [&str; 2] = ["lower_leg_l", "lower_leg_r"];
pub const FOOT: [&str; 2] = ["foot_l", "foot_r"];
pub const TOE: [&str; 2] = ["toe_l", "toe_r"];
/// Side sign: left is −X, right is +X.
const S: [f32; 2] = [-1.0, 1.0];
const SIDE: [&str; 2] = ["l", "r"];

pub fn skeleton() -> Skeleton {
    let i = Quat::IDENTITY;
    let mut bones = vec![
        bone("root", None, Vec3::ZERO, i),
        bone("hips", Some("root"), Vec3::new(0.0, HIPS_HEIGHT, 0.0), i),
        bone("spine", Some("hips"), Vec3::new(0.0, 0.12, 0.0), i),
        bone("chest", Some("spine"), Vec3::new(0.0, 0.18, 0.0), i),
        bone("neck", Some("chest"), Vec3::new(0.0, 0.22, 0.0), i),
        bone("head", Some("neck"), Vec3::new(0.0, 0.10, 0.0), i),
        bone("ponytail_1", Some("head"), Vec3::new(0.0, 0.14, 0.095), i),
        bone(
            "ponytail_2",
            Some("ponytail_1"),
            Vec3::new(0.0, -0.08, 0.035),
            i,
        ),
        bone(
            "ponytail_3",
            Some("ponytail_2"),
            Vec3::new(0.0, -0.09, 0.015),
            i,
        ),
    ];
    for side in 0..2 {
        let s = S[side];
        bones.extend([
            bone(
                CLAVICLE[side],
                Some("chest"),
                Vec3::new(0.03 * s, 0.17, 0.0),
                i,
            ),
            bone(
                UPPER_ARM[side],
                Some(CLAVICLE[side]),
                Vec3::new(0.15 * s, 0.0, 0.0),
                Quat::from_rotation_z(30f32.to_radians() * s),
            ),
            bone(
                LOWER_ARM[side],
                Some(UPPER_ARM[side]),
                Vec3::new(0.0, -0.29, 0.0),
                i,
            ),
            bone(
                HAND[side],
                Some(LOWER_ARM[side]),
                Vec3::new(0.0, -0.25, 0.0),
                i,
            ),
            bone(
                UPPER_LEG[side],
                Some("hips"),
                Vec3::new(0.095 * s, -0.07, 0.0),
                i,
            ),
            bone(
                LOWER_LEG[side],
                Some(UPPER_LEG[side]),
                Vec3::new(0.0, -0.43, 0.0),
                i,
            ),
            bone(
                FOOT[side],
                Some(LOWER_LEG[side]),
                Vec3::new(0.0, -0.40, 0.0),
                i,
            ),
            bone(TOE[side], Some(FOOT[side]), Vec3::new(0.0, -0.05, -0.14), i),
        ]);
    }
    Skeleton::new(SKELETON, bones).expect("humanoid skeleton")
}

/// The humanoid skeleton sized to another character, for driving it through
/// [`Retarget`](crate::retarget::Retarget). `joint(bone)` says where a humanoid joint sits on
/// that character in its bind pose (model space, meters, facing −Z); bones it doesn't know
/// keep their offsets from their parents.
///
/// Rest orientations stay the humanoid's, so every humanoid clip, IK rig and muscle works
/// unchanged: arm and leg chains stay straight along their bones with the character's bone
/// lengths, the rest of the joints move to the character's positions. Clips key the hips
/// relative to their rest offset, so the hips keep it and the root takes up the difference.
pub fn fit_skeleton(name: &str, joint: impl Fn(&str) -> Option<Vec3>) -> Skeleton {
    let base = skeleton();
    let straight = [LOWER_ARM, HAND, LOWER_LEG, FOOT].concat();
    let n = base.len();
    let mut rest = base.rest.clone();
    let mut model = base.rest_model.clone();
    let hips = base.bone("hips");
    for b in 0..n {
        let bone_name = base.names[b].as_str();
        let parent = base.parents[b].map(usize::from);
        let parent_model = parent.map_or(Xf::IDENTITY, |p| model[p]);
        if b == base.bone("root") {
            if let Some(h) = joint("hips") {
                rest[b].t = h - base.rest[hips].t;
            }
        } else if b != hips
            && let Some(p) = parent
            && let Some(at) = joint(bone_name)
        {
            rest[b].t = if straight.contains(&bone_name) {
                // Keep the chain straight; take the character's bone length.
                let from = joint(&base.names[p]).unwrap_or(parent_model.t);
                base.rest[b].t.normalize_or(Vec3::NEG_Y) * (at - from).length()
            } else {
                parent_model.inverse().transform_point(at)
            };
        }
        model[b] = match parent {
            Some(p) => model[p].mul(&rest[b]),
            None => rest[b],
        };
    }
    Skeleton::from_parts(name, base.names.to_vec(), base.parents.to_vec(), rest, None)
}

/// What the character wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outfit {
    /// Tank top and shorts: bare arms and legs show muscles and veins. Has a ponytail.
    Athlete,
    /// Long-sleeved shirt and trousers: shows cloth bunching and stretching.
    Shirt,
    /// Bare-chested, in shorts: chest, shoulder and back muscles show when flexing.
    Lifter,
}

struct Look {
    skin: Paint,
    top: Paint,
    arms: Paint,
    forearms: Paint,
    bottom: Paint,
    shins: Paint,
    calves: Paint,
    shoes: Paint,
    hair: Paint,
    ponytail: bool,
}

fn look(outfit: Outfit) -> Look {
    match outfit {
        Outfit::Athlete => {
            let skin = [0.62, 0.38, 0.25];
            Look {
                skin: Paint::skin(skin),
                top: Paint::cloth([0.42, 0.045, 0.035]),
                arms: Paint::veined(skin),
                forearms: Paint::veined(skin),
                bottom: Paint::cloth([0.025, 0.035, 0.085]),
                shins: Paint::skin(skin),
                calves: Paint::veined(skin),
                shoes: Paint::hard([0.75, 0.75, 0.78]),
                hair: Paint::hide([0.06, 0.035, 0.02]),
                ponytail: true,
            }
        }
        Outfit::Shirt => {
            let skin = [0.34, 0.19, 0.11];
            let shirt = Paint::cloth([0.22, 0.38, 0.62]);
            let trousers = Paint::cloth([0.33, 0.27, 0.16]);
            Look {
                skin: Paint::skin(skin),
                top: shirt,
                arms: shirt,
                forearms: shirt,
                bottom: trousers,
                shins: trousers,
                calves: trousers,
                shoes: Paint::hard([0.11, 0.055, 0.03]),
                hair: Paint::hide([0.015, 0.012, 0.01]),
                ponytail: false,
            }
        }
        Outfit::Lifter => {
            let skin = [0.2, 0.105, 0.06];
            Look {
                skin: Paint::skin(skin),
                top: Paint::skin(skin),
                arms: Paint::veined(skin),
                forearms: Paint::veined(skin),
                bottom: Paint::cloth([0.03, 0.03, 0.033]),
                shins: Paint::skin(skin),
                calves: Paint::veined(skin),
                shoes: Paint::hard([0.05, 0.05, 0.055]),
                hair: Paint::hide([0.012, 0.01, 0.009]),
                ponytail: false,
            }
        }
    }
}

fn joint(s: &Skeleton, name: &str) -> Vec3 {
    s.rest_model[s.bone(name)].t
}

/// The body shape for an outfit (bind pose).
pub fn body(outfit: Outfit) -> Body {
    let sk = skeleton();
    let l = look(outfit);
    let mut b = Body::default();
    // Torso.
    b.part(
        "hips",
        Shape::ellipsoid(Vec3::new(0.0, 0.95, 0.005), Vec3::new(0.165, 0.115, 0.115)),
        l.bottom,
        0.06,
    )
    .part(
        "spine",
        Shape::ellipsoid(Vec3::new(0.0, 1.11, -0.01), Vec3::new(0.145, 0.13, 0.105)),
        l.top,
        0.07,
    )
    .part(
        "chest",
        Shape::ellipsoid(Vec3::new(0.0, 1.29, 0.0), Vec3::new(0.16, 0.165, 0.11)),
        l.top,
        0.07,
    )
    .part(
        "chest",
        Shape::ellipsoid(Vec3::new(0.0, 1.40, 0.005), Vec3::new(0.185, 0.085, 0.10)),
        l.top,
        0.05,
    )
    // Neck and head.
    .part(
        "neck",
        Shape::capsule(
            Vec3::new(0.0, 1.44, 0.012),
            Vec3::new(0.0, 1.62, 0.0),
            0.056,
            0.05,
        ),
        l.skin,
        0.04,
    )
    .part(
        "head",
        Shape::ellipsoid(Vec3::new(0.0, 1.715, 0.005), Vec3::new(0.087, 0.112, 0.10)),
        l.skin,
        0.03,
    )
    .part(
        "head",
        Shape::ellipsoid(Vec3::new(0.0, 1.655, -0.035), Vec3::new(0.066, 0.05, 0.062)),
        l.skin,
        0.04,
    )
    .part(
        "head",
        Shape::capsule(
            Vec3::new(0.0, 1.715, -0.097),
            Vec3::new(0.0, 1.686, -0.112),
            0.011,
            0.016,
        ),
        l.skin,
        0.015,
    )
    .part(
        "head",
        Shape::ellipsoid(Vec3::new(0.0, 1.746, -0.083), Vec3::new(0.07, 0.016, 0.024)),
        l.skin,
        0.02,
    )
    .part(
        "head",
        Shape::ellipsoid(Vec3::new(0.0, 1.758, 0.016), Vec3::new(0.093, 0.09, 0.104)),
        l.hair,
        0.01,
    );
    if l.ponytail {
        let p1 = joint(&sk, "ponytail_1");
        let p2 = joint(&sk, "ponytail_2");
        let p3 = joint(&sk, "ponytail_3");
        let tip = p3 + Vec3::new(0.0, -0.09, 0.0);
        b.part(
            "head",
            Shape::capsule(p1 + Vec3::new(0.0, 0.0, -0.02), p1, 0.028, 0.026),
            l.hair,
            0.02,
        )
        .part(
            "ponytail_1",
            Shape::capsule(p1, p2, 0.026, 0.024),
            l.hair,
            0.012,
        )
        .part(
            "ponytail_2",
            Shape::capsule(p2, p3, 0.024, 0.02),
            l.hair,
            0.012,
        )
        .part(
            "ponytail_3",
            Shape::capsule(p3, tip, 0.02, 0.01),
            l.hair,
            0.012,
        );
    }
    for side in 0..2 {
        let s = S[side];
        let rot = Quat::from_rotation_z(30f32.to_radians() * s);
        let shoulder = joint(&sk, UPPER_ARM[side]);
        let elbow = joint(&sk, LOWER_ARM[side]);
        let wrist = joint(&sk, HAND[side]);
        let d = (elbow - shoulder).normalize();
        let hip = joint(&sk, UPPER_LEG[side]);
        let knee = joint(&sk, LOWER_LEG[side]);
        let ankle = joint(&sk, FOOT[side]);
        let x = |v: f32| v * s;
        // Chest muscles, shoulder blades, trapezius.
        b.part(
            "chest",
            Shape::ellipsoid(
                Vec3::new(x(0.075), 1.345, -0.07),
                Vec3::new(0.075, 0.06, 0.045),
            ),
            l.top,
            0.04,
        )
        .part(
            "chest",
            Shape::ellipsoid(Vec3::new(x(0.09), 1.36, 0.075), Vec3::new(0.08, 0.09, 0.04)),
            l.top,
            0.04,
        )
        .part(
            CLAVICLE[side],
            Shape::capsule(
                Vec3::new(0.0, 1.50, 0.025),
                Vec3::new(x(0.15), 1.455, 0.01),
                0.05,
                0.035,
            ),
            l.top,
            0.04,
        )
        .part(
            "hips",
            Shape::ellipsoid(
                Vec3::new(x(0.075), 0.90, 0.075),
                Vec3::new(0.085, 0.095, 0.07),
            ),
            l.bottom,
            0.05,
        )
        .part(
            "head",
            Shape::ellipsoid(
                Vec3::new(x(0.087), 1.705, 0.012),
                Vec3::new(0.012, 0.03, 0.02),
            ),
            l.skin,
            0.012,
        )
        .part(
            "head",
            Shape::ellipsoid(
                Vec3::new(x(0.034), 1.722, -0.088),
                Vec3::new(0.014, 0.012, 0.01),
            ),
            Paint::hard([0.02, 0.018, 0.016]),
            0.004,
        );
        // Arm.
        b.part(
            UPPER_ARM[side],
            Shape::ellipsoid(
                shoulder + Vec3::new(x(0.025), 0.0, 0.0),
                Vec3::new(0.062, 0.07, 0.065),
            ),
            l.arms,
            0.05,
        )
        .part(
            UPPER_ARM[side],
            Shape::capsule(shoulder, elbow, 0.05, 0.04),
            l.arms,
            0.04,
        )
        .part(
            UPPER_ARM[side],
            Shape::ellipsoid(
                shoulder + d * 0.15 + Vec3::new(0.0, 0.0, -0.022),
                Vec3::new(0.034, 0.075, 0.034),
            )
            .rotated(rot),
            l.arms,
            0.03,
        )
        .part(
            UPPER_ARM[side],
            Shape::ellipsoid(
                shoulder + d * 0.13 + Vec3::new(0.0, 0.0, 0.026),
                Vec3::new(0.036, 0.085, 0.032),
            )
            .rotated(rot),
            l.arms,
            0.03,
        )
        .part(
            LOWER_ARM[side],
            Shape::capsule(elbow, wrist, 0.042, 0.03),
            l.forearms,
            0.04,
        )
        .part(
            LOWER_ARM[side],
            Shape::ellipsoid(
                elbow + d * 0.075 + Vec3::new(0.0, 0.0, -0.008),
                Vec3::new(0.042, 0.075, 0.038),
            )
            .rotated(rot),
            l.forearms,
            0.03,
        )
        .part(
            HAND[side],
            Shape::round_box(wrist + d * 0.07, Vec3::new(0.017, 0.05, 0.043), 0.014).rotated(rot),
            l.skin,
            0.025,
        )
        .part(
            HAND[side],
            Shape::round_box(wrist + d * 0.145, Vec3::new(0.013, 0.04, 0.04), 0.012).rotated(rot),
            l.skin,
            0.015,
        )
        .part(
            HAND[side],
            Shape::capsule(
                wrist + d * 0.03 + Vec3::new(0.0, 0.0, -0.035),
                wrist + d * 0.085 + Vec3::new(x(-0.012), 0.0, -0.06),
                0.012,
                0.01,
            ),
            l.skin,
            0.015,
        );
        // Leg.
        b.part(
            UPPER_LEG[side],
            Shape::capsule(hip + Vec3::new(0.0, 0.03, 0.0), knee, 0.085, 0.052),
            l.bottom,
            0.06,
        )
        .part(
            UPPER_LEG[side],
            Shape::ellipsoid(
                Vec3::new(x(0.095), 0.69, -0.04),
                Vec3::new(0.055, 0.15, 0.045),
            ),
            l.bottom,
            0.04,
        )
        .part(
            LOWER_LEG[side],
            Shape::ellipsoid(
                knee + Vec3::new(0.0, 0.0, -0.035),
                Vec3::new(0.035, 0.035, 0.025),
            ),
            l.shins,
            0.03,
        )
        .part(
            LOWER_LEG[side],
            Shape::capsule(knee, ankle, 0.05, 0.03),
            l.shins,
            0.04,
        )
        .part(
            LOWER_LEG[side],
            Shape::ellipsoid(
                Vec3::new(x(0.095), 0.34, 0.035),
                Vec3::new(0.046, 0.10, 0.045),
            ),
            l.calves,
            0.04,
        )
        .part(
            FOOT[side],
            Shape::round_box(
                Vec3::new(x(0.095), 0.045, -0.055),
                Vec3::new(0.045, 0.045, 0.115),
                0.035,
            ),
            l.shoes,
            0.03,
        )
        .part(
            TOE[side],
            Shape::round_box(
                Vec3::new(x(0.095), 0.032, -0.18),
                Vec3::new(0.043, 0.03, 0.05),
                0.025,
            ),
            l.shoes,
            0.02,
        );
        // Muscle shapes (morph targets).
        let n = |m: &str| format!("{m}_{}", SIDE[side]);
        b.muscle(
            &n("biceps"),
            shoulder + d * 0.15 + Vec3::new(0.0, 0.0, -0.035),
            Vec3::new(0.06, 0.1, 0.06),
            0.02,
            Vec3::new(0.0, -0.004, 0.004),
        )
        .muscle(
            &n("triceps"),
            shoulder + d * 0.13 + Vec3::new(0.0, 0.0, 0.04),
            Vec3::new(0.06, 0.11, 0.055),
            0.012,
            Vec3::new(0.0, -0.007, 0.0),
        )
        .muscle(
            &n("forearm"),
            elbow + d * 0.075 + Vec3::new(0.0, 0.0, -0.02),
            Vec3::new(0.06, 0.095, 0.055),
            0.011,
            Vec3::ZERO,
        )
        .muscle(
            &n("deltoid"),
            shoulder + Vec3::new(x(0.04), 0.01, 0.0),
            Vec3::new(0.07, 0.08, 0.08),
            0.012,
            Vec3::ZERO,
        )
        .muscle(
            &n("pec"),
            Vec3::new(x(0.08), 1.34, -0.1),
            Vec3::new(0.085, 0.07, 0.06),
            0.01,
            Vec3::new(0.0, -0.004, 0.0),
        )
        .muscle(
            &n("trap"),
            Vec3::new(x(0.08), 1.48, 0.03),
            Vec3::new(0.08, 0.05, 0.06),
            0.012,
            Vec3::ZERO,
        )
        .muscle(
            &n("lat"),
            Vec3::new(x(0.14), 1.25, 0.05),
            Vec3::new(0.06, 0.12, 0.07),
            0.01,
            Vec3::ZERO,
        )
        .muscle(
            &n("quad"),
            Vec3::new(x(0.095), 0.70, -0.07),
            Vec3::new(0.07, 0.16, 0.06),
            0.013,
            Vec3::new(0.0, 0.0, -0.004),
        )
        .muscle(
            &n("calf"),
            Vec3::new(x(0.095), 0.35, 0.06),
            Vec3::new(0.06, 0.1, 0.05),
            0.012,
            Vec3::new(0.0, -0.003, 0.003),
        )
        .muscle(
            &n("glute"),
            Vec3::new(x(0.085), 0.88, 0.12),
            Vec3::new(0.09, 0.09, 0.07),
            0.008,
            Vec3::new(0.0, -0.007, 0.0),
        );
    }
    b
}

/// The skinned mesh for an outfit.
pub fn mesh(outfit: Outfit) -> SkinnedMesh {
    body(outfit).build(&skeleton(), 0.0135, 0.028)
}

/// Muscles for [`MuscleRig`](crate::muscle::MuscleRig), matching the morph targets of
/// [`mesh`].
pub fn muscles() -> Vec<Muscle> {
    let mut out = Vec::new();
    let input = |name: &str, weight: f32| MuscleDriver::Input {
        name: name.into(),
        weight,
    };
    for side in 0..2 {
        let s = S[side];
        let n = |m: &str| format!("{m}_{}", SIDE[side]);
        let muscle = |name: &str, bone: &str, drivers: Vec<MuscleDriver>| Muscle {
            name: name.into(),
            bulge: name.into(),
            sway: Some(format!("{name}.sway")),
            drivers,
            bone: bone.into(),
            ..Default::default()
        };
        out.extend([
            muscle(
                &n("biceps"),
                UPPER_ARM[side],
                vec![
                    MuscleDriver::Joint {
                        bone: LOWER_ARM[side].into(),
                        axis: Vec3::X,
                        from_deg: 20.0,
                        to_deg: 125.0,
                        weight: 0.5,
                    },
                    input("effort", 0.6),
                    input("flex", 1.0),
                ],
            ),
            muscle(
                &n("triceps"),
                UPPER_ARM[side],
                vec![
                    input("effort", 0.65),
                    input("brace", 0.5),
                    input("flex", 0.6),
                ],
            ),
            muscle(
                &n("forearm"),
                LOWER_ARM[side],
                vec![
                    input("effort", 0.75),
                    input("brace", 0.4),
                    input("flex", 0.7),
                ],
            ),
            muscle(
                &n("deltoid"),
                UPPER_ARM[side],
                vec![
                    MuscleDriver::Joint {
                        bone: UPPER_ARM[side].into(),
                        axis: Vec3::Z,
                        from_deg: 0.0,
                        to_deg: 60.0 * s,
                        weight: 0.4,
                    },
                    input("effort", 0.55),
                    input("brace", 0.4),
                    input("flex", 0.8),
                ],
            ),
            muscle(
                &n("pec"),
                "chest",
                vec![
                    input("effort", 0.55),
                    input("brace", 0.5),
                    input("flex", 0.5),
                ],
            ),
            muscle(
                &n("trap"),
                CLAVICLE[side],
                vec![input("effort", 0.45), input("flex", 0.7)],
            ),
            muscle(
                &n("lat"),
                "chest",
                vec![input("effort", 0.5), input("flex", 0.9)],
            ),
            muscle(
                &n("quad"),
                UPPER_LEG[side],
                vec![
                    MuscleDriver::Joint {
                        bone: LOWER_LEG[side].into(),
                        axis: Vec3::X,
                        from_deg: -5.0,
                        to_deg: -70.0,
                        weight: 0.45,
                    },
                    input("effort", 0.3),
                    input("move_effort", 0.35),
                ],
            ),
            muscle(
                &n("calf"),
                LOWER_LEG[side],
                vec![
                    MuscleDriver::Joint {
                        bone: FOOT[side].into(),
                        axis: Vec3::X,
                        from_deg: 0.0,
                        to_deg: -25.0,
                        weight: 0.6,
                    },
                    input("move_effort", 0.3),
                ],
            ),
            muscle(
                &n("glute"),
                "hips",
                vec![input("effort", 0.3), input("move_effort", 0.2)],
            ),
        ]);
    }
    out
}

pub fn foot_ik() -> FootIk {
    FootIk {
        legs: (0..2)
            .map(|side| LegIk {
                upper: UPPER_LEG[side].into(),
                lower: LOWER_LEG[side].into(),
                foot: FOOT[side].into(),
                toe: Some(TOE[side].into()),
                knee_direction: Vec3::NEG_Z,
            })
            .collect(),
        ..Default::default()
    }
}

pub fn ponytail() -> SpringChainDef {
    SpringChainDef {
        bones: vec![
            "ponytail_1".into(),
            "ponytail_2".into(),
            "ponytail_3".into(),
        ],
        stiffness: 0.06,
        damping: 0.86,
        gravity: 0.6,
        wind: 0.4,
        tip_length: 0.09,
    }
}

// ----------------------------------------------------------------------------------- clips

/// Arm pose: shoulder flexion (forward), twist (internal), abduction (outward), elbow, wrist.
#[derive(Clone, Copy)]
struct Arm {
    flex: f32,
    twist: f32,
    abduct: f32,
    elbow: f32,
    wrist: f32,
    /// Clavicle protraction (shoulder forward) and elevation, degrees.
    protract: f32,
    elevate: f32,
}

impl Arm {
    const HANG: Arm = Arm {
        flex: 4.0,
        twist: 0.0,
        abduct: -15.0,
        elbow: 12.0,
        wrist: 0.0,
        protract: 0.0,
        elevate: 0.0,
    };
}

fn arm(p: &mut PoseBuilder, side: usize, a: Arm) {
    let s = S[side];
    p.rot(CLAVICLE[side], 0.0, a.protract * s, a.elevate * s)
        .rot(UPPER_ARM[side], a.flex, a.twist * s, a.abduct * s)
        .rot(LOWER_ARM[side], a.elbow, 0.0, 0.0)
        .rot(HAND[side], a.wrist, 0.0, 0.0);
}

/// Leg pose: hip flexion (forward), abduction, knee (positive = bent), ankle (positive =
/// toes up), toe bend.
fn leg(p: &mut PoseBuilder, side: usize, hip: f32, abduct: f32, knee: f32, ankle: f32, toe: f32) {
    let s = S[side];
    p.rot(UPPER_LEG[side], hip, 0.0, abduct * s)
        .rot(LOWER_LEG[side], -knee, 0.0, 0.0)
        .rot(FOOT[side], ankle, 0.0, 0.0)
        .rot(TOE[side], toe, 0.0, 0.0);
}

/// A crouch that keeps the feet flat under the hips: returns how far the hips drop.
fn crouch(p: &mut PoseBuilder, side: usize, hip: f32) -> f32 {
    leg(p, side, hip, 0.0, 2.0 * hip, hip, 0.0);
    0.83 * (1.0 - hip.to_radians().cos())
}

fn spine(p: &mut PoseBuilder, x: [f32; 4], yaw: [f32; 4], roll: [f32; 4]) {
    for (i, b) in ["spine", "chest", "neck", "head"].into_iter().enumerate() {
        p.rot(b, x[i], yaw[i], roll[i]);
    }
}

/// Arms let hang while standing about: close to the sides with the elbows soft and the palms
/// turned in toward the thighs.
const RELAXED: Arm = Arm {
    flex: 4.0,
    twist: -12.0,
    abduct: -21.0,
    elbow: 15.0,
    wrist: 6.0,
    protract: 0.0,
    elevate: 0.0,
};

/// Standing about: the weight settles onto the left leg, the right hip dropping and its knee
/// easing forward with the shoulders tilted against the hips, then shifts across onto the
/// right leg and back through the middle, breathing three times a loop. Starts with the
/// weight even ([`Key::STAND`]).
fn idle(s: &Skeleton) -> AnimationClip {
    // Weight on the left leg at +1, on the right at -1.
    let weight = [
        (0.0, 0.0),
        (0.1, 1.0),
        (0.45, 0.85),
        (0.6, -1.0),
        (0.9, -0.85),
        (1.0, 0.0),
    ];
    AnimationClip::from_fn("idle", 9.6, 97, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let breath = cycle_sin(ph * 3.0);
        let w = keyed(ph, &weight);
        p.rot_move(
            "hips",
            // Over the standing foot (the left is −X), sinking a touch as it settles.
            Vec3::new(-0.06 * w, -0.008 - 0.012 * w.abs(), 0.0),
            0.0,
            4.0 * w,
            -5.0 * w,
        );
        spine(
            &mut p,
            [-1.0, 1.2 * breath, 1.0, -1.0 - 0.6 * breath],
            [-w, -w, 0.5 * w, w],
            [3.0 * w, 4.0 * w, -1.5 * w, -0.5 * w],
        );
        for (side, sign) in S.iter().enumerate() {
            // How free this leg is: 1 with the weight on the other, -1 standing on it.
            let free = w * sign;
            arm(
                &mut p,
                side,
                Arm {
                    flex: RELAXED.flex + 1.0 * breath + 2.0 * free,
                    elevate: 1.2 * breath,
                    ..RELAXED
                },
            );
            let (hip, knee) = if free > 0.0 {
                (4.0 + 8.0 * free, 4.0 + 20.0 * free)
            } else {
                (4.0 + 2.0 * free, 4.0 + 3.0 * free)
            };
            leg(
                &mut p,
                side,
                hip,
                1.5 + 2.5 * free,
                knee,
                0.5 * knee - 2.0,
                0.0,
            );
        }
        p.take()
    })
}

fn walk(s: &Skeleton) -> AnimationClip {
    walk_with(s, "walk", 1.0, 1.0)
}

fn sidestep_left(s: &Skeleton) -> AnimationClip {
    step_cycle(
        s,
        "sidestep_left",
        Gait::walk(Vec2::X, SIDESTEP_SPEED, 0.65),
    )
}

fn sidestep_right(s: &Skeleton) -> AnimationClip {
    step_cycle(
        s,
        "sidestep_right",
        Gait::walk(-Vec2::X, SIDESTEP_SPEED, 0.65),
    )
}

fn backpedal(s: &Skeleton) -> AnimationClip {
    step_cycle(s, "backpedal", Gait::walk(-Vec2::Y, BACKPEDAL_SPEED, 0.9))
}

fn jog_left(s: &Skeleton) -> AnimationClip {
    step_cycle(s, "jog_left", Gait::jog(Vec2::X, SIDE_JOG_SPEED, 0.66))
}

fn jog_right(s: &Skeleton) -> AnimationClip {
    step_cycle(s, "jog_right", Gait::jog(-Vec2::X, SIDE_JOG_SPEED, 0.66))
}

fn jog_back(s: &Skeleton) -> AnimationClip {
    step_cycle(s, "jog_back", Gait::jog(-Vec2::Y, BACK_JOG_SPEED, 0.78))
}

fn run_left(s: &Skeleton) -> AnimationClip {
    step_cycle(s, "run_left", Gait::run(Vec2::X, SIDE_RUN_SPEED, 0.68))
}

fn run_right(s: &Skeleton) -> AnimationClip {
    step_cycle(s, "run_right", Gait::run(-Vec2::X, SIDE_RUN_SPEED, 0.68))
}

fn run_back(s: &Skeleton) -> AnimationClip {
    step_cycle(s, "run_back", Gait::run(-Vec2::Y, BACK_RUN_SPEED, 0.66))
}

/// A shorter, slower walk for strolling: whole steps with the feet clear of the ground,
/// rather than a walk faded into standing.
fn walk_slow(s: &Skeleton) -> AnimationClip {
    walk_with(s, "walk_slow", 1.15, 0.6)
}

/// A walk taking `secs` per stride, its steps `stride` times as long as the full walk's.
fn walk_with(s: &Skeleton, name: &str, secs: f32, stride: f32) -> AnimationClip {
    let swing = 0.5 + 0.5 * stride;
    AnimationClip::from_fn(name, secs, 45, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let c = cycle(ph) * swing;
        p.rot_move(
            "hips",
            Vec3::new(
                0.018 * cycle_sin(ph),
                -0.025 - 0.022 * stride * stride * cycle((ph - 0.05) * 2.0),
                0.0,
            ),
            3.0,
            5.0 * c,
            3.0 * cycle_sin(ph),
        );
        spine(
            &mut p,
            [-2.0, 1.0, 1.0, -2.0],
            [-3.0 * c, -5.0 * c, 1.5 * c, 1.5 * c],
            [
                -1.5 * cycle_sin(ph),
                -cycle_sin(ph),
                1.0 * cycle_sin(ph),
                1.0 * cycle_sin(ph),
            ],
        );
        for side in 0..2 {
            // Right foot strikes at phase 0, left at 0.5.
            let q = (ph + if side == 1 { 0.0 } else { 0.5 }).rem_euclid(1.0);
            let cq = cycle(q);
            let hip = 23.0 * stride * cq + 4.0;
            let knee = 4.0 + 14.0 * bump(q, 0.12, 0.09) + 60.0 * bump(q, 0.78, 0.14);
            let ankle =
                4.0 * bump(q, 0.0, 0.06) - 14.0 * bump(q, 0.55, 0.08) + 6.0 * bump(q, 0.8, 0.1);
            let toe = 24.0 * bump(q, 0.53, 0.07);
            leg(&mut p, side, hip, 1.0, knee, ankle, toe);
            // The arm swings against its leg, further forward than back; the forearm lags
            // the upper arm a little and bends most as the arm comes forward, the hand
            // trailing at the wrist.
            let fore = -cq * swing;
            let lag = -cycle(q - 0.07) * swing;
            arm(
                &mut p,
                side,
                Arm {
                    flex: 2.0 + if fore > 0.0 { 24.0 } else { 22.0 } * fore,
                    abduct: -13.0,
                    elbow: 14.0 + 22.0 * lag.max(0.0) + 4.0 * fore.abs(),
                    wrist: 8.0 * lag,
                    protract: 4.0 * fore,
                    ..Arm::HANG
                },
            );
        }
        p.take()
    })
    .with_event(0.02, "footstep", Some("foot_r"))
    .with_event(0.52, "footstep", Some("foot_l"))
}

/// How a [`step_cycle`] moves: `dir` is (left, ahead) relative to facing, at `speed` m/s,
/// one stride per `secs`; each foot stands for `stance` of the cycle (under a half: a run,
/// with both feet off the ground between steps) and lifts `lift` m as it swings.
#[derive(Debug, Clone, Copy)]
struct Gait {
    dir: Vec2,
    speed: f32,
    secs: f32,
    stance: f32,
    lift: f32,
    /// How far the hips turn towards a sideways move (the shoulders stay facing), degrees.
    turn_in: f32,
    /// Forward lean of the trunk, degrees.
    lean: f32,
}

impl Gait {
    /// Walking steps: long stances, low swings, sidesteps that shuffle without crossing.
    fn walk(dir: Vec2, speed: f32, secs: f32) -> Self {
        Self {
            dir,
            speed,
            secs,
            stance: 0.62,
            lift: 0.08,
            turn_in: 0.0,
            lean: 2.0,
        }
    }

    /// Between a walk and a run, so neighbouring speeds blend steps that land and lift
    /// nearly together.
    fn jog(dir: Vec2, speed: f32, secs: f32) -> Self {
        Self {
            dir,
            speed,
            secs,
            stance: 0.5,
            lift: 0.12,
            turn_in: 20.0,
            lean: 4.0,
        }
    }

    /// Running: short stances with a flight between them, high knees, and the hips turned
    /// into a sideways run so the legs run more ahead than across.
    fn run(dir: Vec2, speed: f32, secs: f32) -> Self {
        Self {
            dir,
            speed,
            secs,
            stance: 0.36,
            lift: 0.2,
            turn_in: 50.0,
            lean: 7.0,
        }
    }
}

/// A walk in any direction relative to facing, solved by leg IK rather than keyed angles:
/// `dir` is (left, ahead), at `speed` m/s, one stride per `secs`. Each planted foot moves
/// back under the body at exactly `speed` (so it stays put on the ground), swings over in an
/// arc, and plants like the walk does: the right foot at phase 0, the left at 0.5, so it
/// blends in step with the walk. Stepping sideways widens the stance so the feet never
/// cross; the hips sink as far as the stance legs need.
fn step_cycle(s: &Skeleton, name: &str, gait: Gait) -> AnimationClip {
    let Gait {
        dir,
        speed,
        secs,
        stance,
        lift,
        ..
    } = gait;
    let rest = |b: &str| {
        let mut model = Vec::new();
        s.to_model(&s.rest, &mut model);
        model[s.bone(b)].t
    };
    let hip = rest(UPPER_LEG[1]);
    let thigh = (rest(LOWER_LEG[1]) - hip).length();
    let shin = (rest(FOOT[1]) - rest(LOWER_LEG[1])).length();
    let ankle_y = rest(FOOT[1]).y;
    // Movement in model space (forward is −Z, left is −X).
    let dir = dir.normalize_or_zero();
    let m = Vec3::new(-dir.x, 0.0, -dir.y);
    // The hips turn into a sideways run (left is a positive yaw), and the feet are placed
    // about the turned hips.
    let yaw = (gait.turn_in * dir.x).to_radians();
    let turn = Quat::from_rotation_y(yaw);
    // Each foot travels 2·half while it stands.
    let half = speed * stance * secs * 0.5;
    // Walking sidesteps widen the stance so the feet never cross; a run has one foot down
    // at a time, so its feet may.
    let across = (turn.conjugate() * m).x.abs();
    let width = if stance > 0.5 {
        hip.x.abs() + across * (half + 0.06 - hip.x.abs()).max(0.0)
    } else {
        hip.x.abs() * 0.8
    };
    let reach = 0.985 * (thigh + shin);
    // Walking sideways is an athlete's shuffle: knees bent, trunk a little forward, hands up
    // and out, and the hips carried over whichever foot holds the weight.
    let athletic = if stance > 0.5 { dir.x.abs() } else { 0.0 };
    // Where each foot is (model space, relative to the hips' rest place) at `q` phase since
    // it planted, and how far it is through its swing (0 while it stands).
    let foot = |side: usize, q: f32| {
        let base = turn * Vec3::new(S[side] * width, 0.0, 0.0) + Vec3::Y * ankle_y;
        if q < stance {
            let u = q / stance;
            (base + m * half * (1.0 - 2.0 * u), 0.0)
        } else {
            let u = (q - stance) / (1.0 - stance);
            let e = u * u * (3.0 - 2.0 * u);
            let at = base + m * half * (2.0 * e - 1.0);
            (at + Vec3::Y * lift * (std::f32::consts::PI * u).sin(), u)
        }
    };
    AnimationClip::from_fn(name, secs, 41, true, move |ph| {
        let mut p = PoseBuilder::new(s);
        let feet = [0, 1].map(|side| {
            let q = (ph + if side == 1 { 0.0 } else { 0.5 }).rem_euclid(1.0);
            foot(side, q)
        });
        // The hips sink until both feet are in reach (knees soft), and dip a little as each
        // foot takes the weight.
        let mut drop: f32 = 0.015;
        let over: f32 = feet
            .iter()
            .enumerate()
            .map(|(side, (_, swing))| -S[side] * (std::f32::consts::PI * swing).sin())
            .sum();
        let shift = Vec3::X * 0.035 * athletic * over;
        let joint_at = |side: usize, drop: f32| {
            turn * Vec3::new(S[side] * hip.x.abs(), 0.0, hip.z) + shift + Vec3::Y * (hip.y - drop)
        };
        for (side, (at, _)) in feet.iter().enumerate() {
            let joint = joint_at(side, 0.0);
            let flat = (*at - joint).with_y(0.0).length();
            let below = (reach * reach - flat * flat).max(0.0).sqrt();
            drop = drop.max(joint.y - at.y - below);
        }
        drop += 0.012 * (1.0 - cycle(ph * 2.0)) * 0.5 + 0.06 * athletic;
        // A stoop (more running), leaning into a sidestep with the head kept level; the
        // spine turns the shoulders back to face ahead over hips turned into a run.
        let roll = 3.0 * dir.x;
        let lean = gait.lean + 6.0 * athletic;
        let yaw_deg = yaw.to_degrees();
        p.rot_move("hips", shift - Vec3::Y * drop, 0.0, yaw_deg, 0.0);
        spine(
            &mut p,
            [-lean * 0.6, -lean * 0.4, 1.5 + lean * 0.4, 0.5 + lean * 0.3],
            [
                -yaw_deg * 0.4,
                -yaw_deg * 0.35,
                -yaw_deg * 0.15,
                -yaw_deg * 0.1,
            ],
            [roll, roll * 0.5, -roll * 0.7, -roll * 0.8],
        );
        for (side, &(at, swing)) in feet.iter().enumerate() {
            let sign = S[side];
            let joint = joint_at(side, drop);
            // The leg is solved in the hips' turned frame.
            let d = turn.conjugate() * (at - joint);
            // Abduction swings the leg's plane out about the forward axis; flexion and the
            // knee reach within it.
            let out = d.x * sign;
            let down = (d.y * d.y + out * out).sqrt();
            let abduct = out.atan2(-d.y);
            let ahead = -d.z;
            let len = (ahead * ahead + down * down)
                .sqrt()
                .clamp(0.05, thigh + shin - 1e-3);
            let cos_knee = (thigh * thigh + shin * shin - len * len) / (2.0 * thigh * shin);
            let knee = std::f32::consts::PI - cos_knee.clamp(-1.0, 1.0).acos();
            let cos_hip = (thigh * thigh + len * len - shin * shin) / (2.0 * thigh * len);
            let flex = ahead.atan2(down) + cos_hip.clamp(-1.0, 1.0).acos();
            let (flex, knee, abduct) = (flex.to_degrees(), knee.to_degrees(), abduct.to_degrees());
            // The foot stays level, toes lifting a little through the swing.
            let toe_up = 12.0 * (std::f32::consts::PI * swing).sin();
            p.rot(UPPER_LEG[side], flex, 0.0, abduct * sign)
                .rot(LOWER_LEG[side], -knee, 0.0, 0.0)
                .rot(FOOT[side], knee - flex + toe_up, 0.0, -abduct * sign)
                .rot(TOE[side], 0.0, 0.0, 0.0);
            // Arms swing against the opposite leg's travel (pumping, elbows bent, in a run),
            // and open a little stepping sideways.
            let other = feet[1 - side].0;
            let ahead_of_hips = turn * Vec3::NEG_Z;
            let swing_arm = (other.dot(ahead_of_hips) / half.max(0.05)).clamp(-1.0, 1.0);
            let walk_swing = swing_arm * dir.y;
            let running = if stance < 0.5 { 1.0 } else { 0.0 };
            arm(
                &mut p,
                side,
                Arm {
                    flex: 2.0 + 12.0 * walk_swing * (1.0 - running) - 30.0 * swing_arm * running
                        + 14.0 * athletic,
                    elbow: 18.0 + 6.0 * walk_swing.abs() + 62.0 * running + 30.0 * athletic,
                    abduct: -15.0 - 6.0 * dir.x.abs() * (1.0 - running) + 12.0 * athletic,
                    wrist: 6.0 * athletic,
                    ..Arm::HANG
                },
            );
        }
        p.take()
    })
    .with_event(0.0, "footstep", Some("foot_r"))
    .with_event(0.5, "footstep", Some("foot_l"))
}

fn run(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("run", 0.66, 41, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let c = cycle(ph);
        p.rot_move(
            "hips",
            Vec3::new(
                0.012 * cycle_sin(ph),
                -0.05 - 0.035 * cycle((ph - 0.15) * 2.0),
                0.0,
            ),
            6.0,
            8.0 * c,
            4.0 * cycle_sin(ph),
        );
        spine(
            &mut p,
            [-9.0, -4.0, 6.0, 1.0],
            [-4.0 * c, -9.0 * c, 3.0 * c, 2.0 * c],
            [
                -2.0 * cycle_sin(ph),
                -cycle_sin(ph),
                1.5 * cycle_sin(ph),
                1.0 * cycle_sin(ph),
            ],
        );
        for side in 0..2 {
            let q = (ph + if side == 1 { 0.0 } else { 0.5 }).rem_euclid(1.0);
            let hip = 36.0 * cycle(q - 0.05) + 12.0;
            let knee = 22.0 + 18.0 * bump(q, 0.15, 0.1) + 85.0 * bump(q, 0.68, 0.15);
            let ankle = -18.0 * bump(q, 0.35, 0.08) + 10.0 * bump(q, 0.85, 0.12) + 4.0;
            let toe = 20.0 * bump(q, 0.33, 0.07);
            leg(&mut p, side, hip, 1.0, knee, ankle, toe);
            let cq = cycle(q);
            arm(
                &mut p,
                side,
                Arm {
                    flex: -32.0 * cq + 6.0,
                    abduct: -11.0,
                    elbow: 82.0 - 12.0 * cq,
                    wrist: 5.0,
                    protract: -6.0 * cq,
                    ..Arm::HANG
                },
            );
        }
        p.take()
    })
    .with_event(0.02, "footstep", Some("foot_r"))
    .with_event(0.52, "footstep", Some("foot_l"))
}

/// Flat-out running: the trunk tipped forward, knees driven high and heels kicked up under
/// the hips, arms pumping hard from the hip to the chin with the elbows held near a right
/// angle. Each foot is down for about a fifth of the cycle.
fn sprint(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("sprint", 0.56, 41, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let c = cycle(ph);
        p.rot_move(
            "hips",
            Vec3::new(
                0.01 * cycle_sin(ph),
                -0.06 - 0.03 * cycle((ph - 0.12) * 2.0),
                0.0,
            ),
            -3.0,
            9.0 * c,
            5.0 * cycle_sin(ph),
        );
        spine(
            &mut p,
            [-15.0, -9.0, 10.0, 6.0],
            [-5.0 * c, -10.0 * c, 4.0 * c, 2.0 * c],
            [
                -2.0 * cycle_sin(ph),
                -cycle_sin(ph),
                1.5 * cycle_sin(ph),
                cycle_sin(ph),
            ],
        );
        for side in 0..2 {
            let q = (ph + if side == 1 { 0.0 } else { 0.5 }).rem_euclid(1.0);
            // Thigh furthest back just after toe-off, highest late in the swing. The heel
            // folds up to the seat as the thigh passes under the hips, the shin hangs under
            // the raised thigh and only reaches out just before the foot lands under the
            // hips.
            let hip = 22.0 + 50.0 * cycle(q - 0.75);
            let knee = 24.0
                + 14.0 * bump(q, 0.1, 0.08)
                + 100.0 * bump(q, 0.42, 0.15)
                + 60.0 * bump(q, 0.66, 0.14);
            let ankle = -6.0 - 24.0 * bump(q, 0.24, 0.07) + 18.0 * bump(q, 0.8, 0.1);
            let toe = 25.0 * bump(q, 0.22, 0.06);
            leg(&mut p, side, hip, 1.0, knee, ankle, toe);
            let cq = cycle(q - 0.02);
            arm(
                &mut p,
                side,
                Arm {
                    flex: 20.0 - 60.0 * cq,
                    abduct: -9.0,
                    elbow: 88.0 - 28.0 * cq,
                    wrist: 4.0,
                    protract: -10.0 * cq,
                    ..Arm::HANG
                },
            );
        }
        p.take()
    })
    .with_event(0.02, "footstep", Some("foot_r"))
    .with_event(0.52, "footstep", Some("foot_l"))
}

/// Arms folded across the chest, the left forearm over the right with each hand tucked by
/// the other arm, rising a little with each breath. Laid over the arms by `arms_folded`.
fn arms_folded(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("arms_folded", 4.0, 17, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let breath = cycle_sin(ph);
        for (side, flex, elbow) in [(1, 30.0, 84.0), (0, 35.0, 95.0)] {
            arm(
                &mut p,
                side,
                Arm {
                    flex: flex + 1.5 * breath,
                    twist: 80.0,
                    abduct: -30.0,
                    elbow,
                    wrist: 4.0,
                    protract: 2.0,
                    elevate: 1.0 + breath,
                },
            );
        }
        p.take()
    })
}

/// Upper-body lean into a turn (additive). `dir`: +1 left, −1 right.
fn lean(s: &Skeleton, name: &str, dir: f32) -> AnimationClip {
    AnimationClip::from_fn(name, 1.0, 2, true, |_| {
        let mut p = PoseBuilder::new(s);
        spine(
            &mut p,
            [0.0; 4],
            [2.0 * dir, 3.0 * dir, 2.0 * dir, 3.0 * dir],
            [7.0 * dir, 5.0 * dir, -4.0 * dir, -6.0 * dir],
        );
        p.take()
    })
}

/// Leaning into a change of speed (added on top of moving, by `accel`): `dir` 1 tips the
/// trunk forward to drive off, −1 sits back on bent knees to dig in and stop. The head stays
/// nearer level than the trunk.
fn lean_pitch(s: &Skeleton, name: &str, dir: f32) -> AnimationClip {
    AnimationClip::from_fn(name, 1.0, 2, true, |_| {
        let mut p = PoseBuilder::new(s);
        let (trunk, drop) = if dir > 0.0 {
            (-16.0, 0.02)
        } else {
            (10.0, 0.05)
        };
        p.rot_move("hips", Vec3::new(0.0, -drop, 0.0), trunk * 0.3, 0.0, 0.0);
        spine(
            &mut p,
            [trunk * 0.45, trunk * 0.4, -trunk * 0.35, -trunk * 0.3],
            [0.0; 4],
            [0.0; 4],
        );
        p.take()
    })
}

/// Additive posture for the ground's slope (`climb` up a flight of stairs, `descend` down
/// one): leaning into a climb with the head up to see the top; going down, the knees softer,
/// the chin down to watch the steps and the arms a little out for balance.
fn grade_posture(s: &Skeleton, name: &str, dir: f32) -> AnimationClip {
    AnimationClip::from_fn(name, 1.0, 2, true, |_| {
        let mut p = PoseBuilder::new(s);
        if dir > 0.0 {
            p.rot_move("hips", Vec3::new(0.0, -0.02, 0.0), -5.0, 0.0, 0.0);
            spine(&mut p, [-5.0, -4.0, 3.0, 4.0], [0.0; 4], [0.0; 4]);
            for side in 0..2 {
                arm(
                    &mut p,
                    side,
                    Arm {
                        flex: 4.0,
                        twist: 0.0,
                        abduct: 0.0,
                        elbow: 10.0,
                        wrist: 0.0,
                        protract: 0.0,
                        elevate: 0.0,
                    },
                );
            }
        } else {
            p.rot_move("hips", Vec3::new(0.0, -0.035, 0.0), -1.0, 0.0, 0.0);
            spine(&mut p, [-1.0, -4.0, -8.0, -16.0], [0.0; 4], [0.0; 4]);
            for side in 0..2 {
                arm(
                    &mut p,
                    side,
                    Arm {
                        flex: 6.0,
                        twist: 0.0,
                        abduct: 12.0,
                        elbow: 16.0,
                        wrist: 0.0,
                        protract: 0.0,
                        elevate: 2.0,
                    },
                );
            }
        }
        p.take()
    })
}

fn heavy_swing(s: &Skeleton) -> AnimationClip {
    let effort = [
        (0.0, 0.0),
        (0.15, 0.25),
        (0.32, 0.55),
        (0.45, 1.0),
        (0.58, 1.0),
        (0.75, 0.15),
        (1.0, 0.0),
    ];
    AnimationClip::from_fn("heavy_swing", 2.0, 61, false, |ph| {
        let mut p = PoseBuilder::new(s);
        let k = |keys: &[(f32, f32)]| keyed(ph, keys);
        let bend = k(&[
            (0.0, 2.0),
            (0.32, 8.0),
            (0.55, 27.0),
            (0.65, 28.0),
            (0.88, 6.0),
            (1.0, 2.0),
        ]);
        let mut drop = 0.0;
        for side in 0..2 {
            drop = crouch(&mut p, side, bend);
        }
        p.rot_move(
            "hips",
            Vec3::new(0.0, -drop, 0.0),
            k(&[(0.0, 0.0), (0.55, 10.0), (1.0, 0.0)]),
            k(&[(0.0, 0.0), (0.32, -10.0), (0.55, 8.0), (1.0, 0.0)]),
            0.0,
        );
        let chest_yaw = k(&[
            (0.0, 0.0),
            (0.32, -22.0),
            (0.53, 4.0),
            (0.65, 6.0),
            (1.0, 0.0),
        ]);
        spine(
            &mut p,
            [
                k(&[
                    (0.0, -1.0),
                    (0.32, 10.0),
                    (0.42, 12.0),
                    (0.55, -22.0),
                    (0.65, -26.0),
                    (0.85, -6.0),
                    (1.0, -1.0),
                ]),
                k(&[
                    (0.0, 0.0),
                    (0.32, 8.0),
                    (0.55, -12.0),
                    (0.65, -14.0),
                    (1.0, 0.0),
                ]),
                k(&[(0.0, 0.0), (0.55, 10.0), (1.0, 0.0)]),
                k(&[(0.0, 0.0), (0.32, 6.0), (0.55, -2.0), (1.0, 0.0)]),
            ],
            [
                chest_yaw * 0.4,
                chest_yaw * 0.6,
                -chest_yaw * 0.3,
                -chest_yaw * 0.4,
            ],
            [0.0, 0.0, 0.0, 0.0],
        );
        // Right arm swings the hammer (the left hand follows it on the handle through
        // ReachIk while the `grip` curve is on). The wrists cock the hammer back on the lift
        // and snap it in line with the arms for the strike.
        arm(
            &mut p,
            1,
            Arm {
                flex: k(&[
                    (0.0, 4.0),
                    (0.12, 30.0),
                    (0.32, 165.0),
                    (0.42, 160.0),
                    (0.53, 62.0),
                    (0.6, 58.0),
                    (0.68, 50.0),
                    (0.85, 10.0),
                    (1.0, 4.0),
                ]),
                twist: k(&[(0.0, 0.0), (0.32, -20.0), (0.53, 0.0), (1.0, 0.0)]),
                abduct: k(&[
                    (0.0, -15.0),
                    (0.32, 8.0),
                    (0.53, -10.0),
                    (0.7, -10.0),
                    (1.0, -15.0),
                ]),
                elbow: k(&[
                    (0.0, 12.0),
                    (0.32, 115.0),
                    (0.42, 120.0),
                    (0.5, 15.0),
                    (0.53, 8.0),
                    (0.65, 10.0),
                    (1.0, 12.0),
                ]),
                wrist: k(&[
                    (0.0, 0.0),
                    (0.32, -30.0),
                    (0.45, -20.0),
                    (0.53, -70.0),
                    (0.65, -60.0),
                    (0.85, -20.0),
                    (1.0, 0.0),
                ]),
                protract: k(&[
                    (0.0, 0.0),
                    (0.32, -8.0),
                    (0.53, 12.0),
                    (0.7, 8.0),
                    (1.0, 0.0),
                ]),
                elevate: k(&[(0.0, 0.0), (0.32, 10.0), (0.55, 0.0)]),
            },
        );
        arm(
            &mut p,
            0,
            Arm {
                flex: k(&[
                    (0.0, 4.0),
                    (0.32, 40.0),
                    (0.52, -30.0),
                    (0.8, 0.0),
                    (1.0, 4.0),
                ]),
                abduct: k(&[(0.0, -15.0), (0.32, 10.0), (0.52, 5.0), (1.0, -15.0)]),
                elbow: k(&[(0.0, 12.0), (0.32, 60.0), (0.52, 35.0), (1.0, 12.0)]),
                ..Arm::HANG
            },
        );
        p.take()
    })
    .with_curve("effort", 61, |ph| keyed(ph, &effort))
    // Two hands on the handle from the lift to the recovery (drives the off hand's ReachIk).
    .with_curve("grip", 61, |ph| {
        keyed(ph, &[(0.0, 0.0), (0.1, 1.0), (0.8, 1.0), (0.95, 0.0)])
    })
    .with_event(0.53, "impact", Some("hand_r"))
}

fn block(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("block", 1.2, 25, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let tremble = (ph * 6.0 * std::f32::consts::TAU).sin();
        let mut drop = 0.0;
        for side in 0..2 {
            drop = crouch(&mut p, side, 14.0);
        }
        p.rot_move("hips", Vec3::new(0.0, -drop, 0.0), 4.0, 0.0, 0.0);
        spine(&mut p, [-8.0, -6.0, 2.0, -12.0], [0.0; 4], [0.0; 4]);
        for side in 0..2 {
            arm(
                &mut p,
                side,
                Arm {
                    flex: 72.0 + 0.6 * tremble,
                    twist: 30.0,
                    abduct: 16.0,
                    elbow: 122.0 + 0.8 * tremble,
                    wrist: -10.0,
                    protract: 6.0,
                    elevate: 6.0,
                },
            );
        }
        p.take()
    })
    .with_curve("effort", 25, |ph| {
        0.85 + 0.05 * (ph * 3.0 * std::f32::consts::TAU).sin()
    })
    .with_curve("brace", 2, |_| 1.0)
}

/// A boxer's guard, held loose: left foot forward, knees soft, the trunk turned to show the
/// left shoulder, chin tucked, the lead fist out at chin height and the rear fist by the
/// cheek, bouncing lightly on the balls of the feet with the hands working a little.
fn guard(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("guard", 1.6, 49, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let tau = std::f32::consts::TAU;
        // Two bounces a loop, and a slow weave from side to side once.
        let bounce = (ph * 2.0 * tau).sin();
        let weave = (ph * tau).sin();
        p.rot_move(
            "hips",
            Vec3::new(0.012 * weave, -0.045 + 0.008 * bounce, 0.0),
            2.0,
            -24.0,
            0.0,
        );
        spine(
            &mut p,
            [-2.0, -3.0 + 0.8 * bounce, 2.0, -8.0],
            [2.0, 3.0, 6.0, 8.0 + 2.0 * weave],
            [-1.5 * weave, -2.0 * weave, 0.0, 1.5 * weave],
        );
        // Lead (left) fist out in front of the chin, rear (right) fist by the cheek.
        arm(
            &mut p,
            0,
            Arm {
                flex: 50.0 + 3.0 * weave,
                twist: 0.0,
                abduct: -16.0,
                elbow: 110.0 - 4.0 * weave,
                wrist: -6.0,
                protract: 8.0,
                elevate: 7.0,
            },
        );
        arm(
            &mut p,
            1,
            Arm {
                flex: 55.0 - 2.0 * weave,
                twist: 10.0,
                abduct: -28.0,
                elbow: 145.0 + 3.0 * weave,
                wrist: -8.0,
                protract: 5.0,
                elevate: 9.0,
            },
        );
        // Stance: the lead leg ahead, the rear heel up off the ground.
        leg(
            &mut p,
            0,
            20.0,
            4.0,
            26.0 + 3.0 * bounce,
            8.0 + 1.5 * bounce,
            0.0,
        );
        leg(&mut p, 1, -6.0, 6.0, 20.0 + 3.0 * bounce, -6.0, 8.0);
        p.take()
    })
    .with_curve("effort", 49, |_| 0.15)
    .with_curve("fist", 2, |_| 0.9)
}

/// A boxer's guard arms (lead left fist before the chin, rear right fist by the cheek), as in
/// [`guard`] without its weave.
const GUARD_ARMS: [Arm; 2] = [
    Arm {
        flex: 50.0,
        twist: 0.0,
        abduct: -16.0,
        elbow: 110.0,
        wrist: -6.0,
        protract: 8.0,
        elevate: 7.0,
    },
    Arm {
        flex: 55.0,
        twist: 10.0,
        abduct: -28.0,
        elbow: 145.0,
        wrist: -8.0,
        protract: 5.0,
        elevate: 9.0,
    },
];

/// The punches of the paddock's combo ([`crate::combat`]), all from and back to the guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Punch {
    /// Lead (left) straight: quick, the shoulder turned in.
    Jab,
    /// Rear (right) straight: the hips and chest turn through it.
    Cross,
    /// Lead hook: the elbow up level with the fist, the body turning hard behind it.
    Hook,
    /// Rear uppercut: dip, then drive up through the legs.
    Uppercut,
    /// Lead hook to the body (the liver): knees bent deep and the trunk leaning over to the
    /// left, the fist driving up under the ribs.
    BodyHook,
}

impl Punch {
    pub const ALL: [Punch; 5] = [
        Punch::Jab,
        Punch::Cross,
        Punch::Hook,
        Punch::Uppercut,
        Punch::BodyHook,
    ];

    pub fn clip_name(self) -> &'static str {
        match self {
            Punch::Jab => "jab",
            Punch::Cross => "cross",
            Punch::Hook => "hook",
            Punch::Uppercut => "uppercut",
            Punch::BodyHook => "body_hook",
        }
    }

    /// The punching side (0 left, 1 right).
    pub fn side(self) -> usize {
        match self {
            Punch::Jab | Punch::Hook | Punch::BodyHook => 0,
            Punch::Cross | Punch::Uppercut => 1,
        }
    }

    /// Length, and the phase where the fist lands.
    pub fn timing(self) -> (f32, f32) {
        match self {
            Punch::Jab => (0.5, 0.32),
            Punch::Cross => (0.62, 0.36),
            Punch::Hook => (0.72, 0.42),
            Punch::Uppercut => (0.75, 0.45),
            Punch::BodyHook => (0.74, 0.43),
        }
    }
}

/// A punch from the guard: wind-up, the fist out on the landing phase, back to the guard.
/// `impact` fires on the punching hand where it lands; `effort` peaks there.
fn punch(s: &Skeleton, kind: Punch) -> AnimationClip {
    let (secs, land) = kind.timing();
    let side = kind.side();
    let keys = 31;
    // Phases: start (guard), load, land, hold a moment, back.
    let load = land * 0.45;
    let back = (land + 0.12).min(0.75);
    let out = match kind {
        Punch::Jab => Arm {
            flex: 88.0,
            twist: -55.0,
            abduct: -4.0,
            elbow: 6.0,
            wrist: 0.0,
            protract: 22.0,
            elevate: 6.0,
        },
        Punch::Cross => Arm {
            flex: 86.0,
            twist: -60.0,
            abduct: -2.0,
            elbow: 5.0,
            wrist: 0.0,
            protract: 26.0,
            elevate: 5.0,
        },
        Punch::Hook => Arm {
            flex: 72.0,
            twist: 30.0,
            abduct: 62.0,
            elbow: 95.0,
            wrist: 0.0,
            protract: 18.0,
            elevate: 12.0,
        },
        Punch::Uppercut => Arm {
            flex: 70.0,
            twist: 20.0,
            abduct: -18.0,
            elbow: 85.0,
            wrist: -10.0,
            protract: 14.0,
            elevate: 4.0,
        },
        // Lower and shorter than the head hook, the forearm angled up under the ribs.
        Punch::BodyHook => Arm {
            flex: 40.0,
            twist: 24.0,
            abduct: 44.0,
            elbow: 100.0,
            wrist: 0.0,
            protract: 16.0,
            elevate: 2.0,
        },
    };
    // The punching arm draws back a little on the load (a hook swings out first).
    let guard = GUARD_ARMS[side];
    let loaded = match kind {
        Punch::Hook | Punch::BodyHook => Arm {
            abduct: 35.0,
            elbow: 100.0,
            ..guard
        },
        Punch::Uppercut => Arm {
            flex: 30.0,
            elbow: 100.0,
            abduct: -24.0,
            ..guard
        },
        _ => Arm {
            flex: guard.flex - 6.0,
            ..guard
        },
    };
    // Chest yaw (+ turns left) at the landing: the punching shoulder comes through.
    let turn = match kind {
        Punch::Jab => -10.0,
        Punch::Cross => 28.0,
        Punch::Hook => -34.0,
        Punch::Uppercut => 20.0,
        Punch::BodyHook => -30.0,
    };
    let wind = match kind {
        Punch::Hook | Punch::BodyHook => 10.0,
        Punch::Uppercut => -8.0,
        _ => -turn * 0.15,
    };
    let dip = if kind == Punch::Uppercut { 14.0 } else { 0.0 };
    // A body shot is thrown from bent knees, the trunk leaning over the punching side.
    let (body_sink, body_lean) = if kind == Punch::BodyHook {
        (16.0, 14.0)
    } else {
        (0.0, 0.0)
    };
    let effort = [
        (0.0, 0.1),
        (load, 0.4),
        (land, 1.0),
        (back, 0.5),
        (1.0, 0.15),
    ];
    // How much aim IK pulls the fist onto the target (combat::aim_strikes).
    let strike = [
        (0.0, 0.0),
        (load, 0.2),
        (land, 1.0),
        (back, 0.6),
        (1.0, 0.0),
    ];
    let name = kind.clip_name();
    AnimationClip::from_fn(name, secs, keys, false, move |ph| {
        let mut p = PoseBuilder::new(s);
        let k = |keys: &[(f32, f32)]| keyed(ph, keys);
        let yaw = k(&[
            (0.0, 0.0),
            (load, wind),
            (land, turn),
            (back, turn * 0.8),
            (1.0, 0.0),
        ]);
        // Knees: the boxer's stance, sinking on the load (an uppercut dips and drives up).
        let sink = k(&[
            (0.0, 0.0),
            (load, 3.0 + dip + body_sink),
            (land, if dip > 0.0 { -4.0 } else { 2.0 + body_sink }),
            (back, body_sink * 0.6),
            (1.0, 0.0),
        ]);
        let lean = k(&[
            (0.0, 0.0),
            (load, body_lean * 0.6),
            (land, body_lean),
            (1.0, 0.0),
        ]);
        p.rot_move(
            "hips",
            Vec3::new(0.0, -0.045 - sink * 0.004, 0.0),
            2.0 + sink * 0.3,
            -24.0 + yaw * 0.5,
            0.0,
        );
        spine(
            &mut p,
            [-2.0 + sink * 0.4 - lean * 0.4, -3.0, 2.0, -8.0],
            [yaw * 0.25, yaw * 0.35, -yaw * 0.2, -yaw * 0.25],
            // Over to the left for a body hook (+ rolls the trunk to its left).
            [lean * 0.5, lean * 0.4, -lean * 0.5, -lean * 0.3],
        );
        let punching = arm_keyed(
            ph,
            &[
                (0.0, guard),
                (load, loaded),
                (land, out),
                (back, out.lerp(&guard, 0.25)),
                (1.0, guard),
            ],
        );
        arm(&mut p, side, punching);
        // The other hand stays up by the chin.
        arm(&mut p, 1 - side, GUARD_ARMS[1 - side]);
        // Stance: lead leg ahead, rear heel up; the rear foot pivots on a cross.
        let pivot = if side == 1 {
            k(&[(0.0, 8.0), (land, 18.0), (1.0, 8.0)])
        } else {
            8.0
        };
        leg(&mut p, 0, 20.0, 4.0, 26.0 + sink, 8.0, 0.0);
        leg(&mut p, 1, -6.0, 6.0, 20.0 + sink, -6.0, pivot);
        p.take()
    })
    .with_curve("effort", keys, move |ph| keyed(ph, &effort))
    .with_curve("fist", 2, |_| 1.0)
    .with_curve("strike", keys, move |ph| keyed(ph, &strike))
    .with_event(
        land,
        "impact",
        Some(if side == 0 { "hand_l" } else { "hand_r" }),
    )
}

/// Head movement that makes a punch miss ([`crate::combat::Evader`]), all from and back to
/// the boxer's stance with the guard up and the eyes on the attacker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evasion {
    /// Head to the left, outside a right hand: knees bent, hips and shoulders turning with it.
    SlipLeft,
    /// Head to the right, outside a jab.
    SlipRight,
    /// A U-shaped dip from left to right under a hook.
    Roll,
    /// Weight onto the rear foot and the trunk back, away from an uppercut.
    PullBack,
}

impl Evasion {
    pub const ALL: [Evasion; 4] = [
        Evasion::SlipLeft,
        Evasion::SlipRight,
        Evasion::Roll,
        Evasion::PullBack,
    ];

    pub fn clip_name(self) -> &'static str {
        match self {
            Evasion::SlipLeft => "slip_left",
            Evasion::SlipRight => "slip_right",
            Evasion::Roll => "roll",
            Evasion::PullBack => "pull_back",
        }
    }

    /// Length, s.
    pub fn secs(self) -> f32 {
        match self {
            Evasion::Roll => 0.75,
            _ => 0.55,
        }
    }
}

/// An evasion: out of the line fast (most of the way by 0.3 of the clip), held, and back to
/// the stance. `effort` rises with it.
fn evasion(s: &Skeleton, kind: Evasion) -> AnimationClip {
    let secs = kind.secs();
    let keys = 31;
    // How far out of the line: fast in, a hold, eased back.
    let out = [(0.0, 0.0), (0.3, 1.0), (0.55, 1.0), (1.0, 0.0)];
    AnimationClip::from_fn(kind.clip_name(), secs, keys, false, move |ph| {
        let mut p = PoseBuilder::new(s);
        let e = keyed(ph, &out);
        // Sideways (+ to its own right), down and back (m) for the hips; trunk roll (+ to
        // its left), pitch (− forward) and yaw (+ turns left), degrees; extra knee bend.
        let (side, down, back, roll, pitch, yaw, knees) = match kind {
            Evasion::SlipLeft => (
                -0.07 * e,
                0.05 * e,
                0.0,
                22.0 * e,
                -4.0 * e,
                12.0 * e,
                14.0 * e,
            ),
            Evasion::SlipRight => (
                0.07 * e,
                0.05 * e,
                0.0,
                -22.0 * e,
                -4.0 * e,
                -12.0 * e,
                14.0 * e,
            ),
            Evasion::Roll => {
                // Down through the bottom of the U as it crosses from left to right.
                let u = ((ph - 0.04) / 0.56).clamp(0.0, 1.0);
                let dip = (u * std::f32::consts::PI).sin();
                // Leaning left first, from and back to the middle.
                let start = (ph / 0.12).clamp(0.0, 1.0);
                let across = -(u * std::f32::consts::PI).cos() * start * (1.0 - smooth_end(ph));
                // Down with the legs, not by bending into the punch.
                (
                    0.07 * across,
                    0.2 * dip,
                    0.0,
                    -14.0 * across,
                    -10.0 * dip,
                    -8.0 * across,
                    50.0 * dip,
                )
            }
            Evasion::PullBack => (0.0, 0.02 * e, 0.07 * e, 0.0, 16.0 * e, -6.0 * e, 6.0 * e),
        };
        p.rot_move(
            "hips",
            Vec3::new(side, -0.045 - down, back),
            2.0 + pitch * 0.2,
            -24.0 + yaw * 0.4,
            roll * 0.25,
        );
        // The trunk carries the head; the neck and head stay nearly level, eyes forward.
        spine(
            &mut p,
            [
                -2.0 + pitch * 0.45,
                -3.0 + pitch * 0.45,
                2.0 - pitch * 0.2,
                -8.0 - pitch * 0.2,
            ],
            [yaw * 0.3, yaw * 0.4, -yaw * 0.3, -yaw * 0.2],
            [roll * 0.4, roll * 0.4, -roll * 0.25, -roll * 0.1],
        );
        for (side, guard) in GUARD_ARMS.into_iter().enumerate() {
            arm(&mut p, side, guard);
        }
        // Stance: the weight goes to the leg on the side the head goes.
        let (lead, rear) = match kind {
            Evasion::SlipLeft => (knees * 1.2, knees * 0.6),
            Evasion::SlipRight => (knees * 0.6, knees * 1.2),
            Evasion::PullBack => (knees * 0.4, knees * 1.6),
            Evasion::Roll => (knees, knees),
        };
        leg(
            &mut p,
            0,
            20.0 + lead * 0.5,
            4.0,
            26.0 + lead,
            8.0 + lead * 0.4,
            0.0,
        );
        leg(
            &mut p,
            1,
            -6.0 + rear * 0.5,
            6.0,
            20.0 + rear,
            -6.0 + rear * 0.4,
            8.0,
        );
        p.take()
    })
    .with_curve("effort", keys, move |ph| 0.15 + 0.35 * keyed(ph, &out))
    .with_curve("fist", 2, |_| 0.9)
}

/// 0 until the last fifth of a clip, then easing to 1 (back to the middle at the end).
fn smooth_end(ph: f32) -> f32 {
    let t = ((ph - 0.75) / 0.25).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Travel that closes a long gap before a strike ([`crate::combat::Approach`]), from and back
/// to the boxer's stance with the guard up. The attacker's warp moves the body; these give
/// it the legs and the lean for it, so it reads as a push-off and a landing rather than a
/// slide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approach {
    /// A step-in lunge: sink, drive off the rear foot with the lead knee up, both feet off the
    /// ground for a moment, land back in the stance.
    Lunge,
    /// A bounding leap: a deep load, the legs tucked in flight, a landing that sinks.
    Leap,
}

impl Approach {
    pub const ALL: [Approach; 2] = [Approach::Lunge, Approach::Leap];

    pub fn clip_name(self) -> &'static str {
        match self {
            Approach::Lunge => "lunge",
            Approach::Leap => "leap",
        }
    }

    /// Length, s.
    pub fn secs(self) -> f32 {
        match self {
            Approach::Lunge => 0.45,
            Approach::Leap => 0.75,
        }
    }

    /// The share of the clip over which the body travels (push-off to touch-down).
    pub fn travel(self) -> (f32, f32) {
        match self {
            Approach::Lunge => (0.1, 0.78),
            Approach::Leap => (0.15, 0.8),
        }
    }
}

/// An approach: the legs and lean of a lunge or a leap (the warp carries the body).
fn approach(s: &Skeleton, kind: Approach) -> AnimationClip {
    let keys = 31;
    // Per phase: hips up (m, from the stance), trunk pitch (− forward), how square the hips
    // turn to the front (0 the stance's -24°, 1 square), lead leg hip/knee/ankle, rear leg
    // hip/knee/ankle (degrees).
    type K = (f32, [f32; 9]);
    let stance: [f32; 9] = [0.0, 0.0, 0.0, 20.0, 26.0, 8.0, -6.0, 20.0, -6.0];
    let frames: Vec<K> = match kind {
        Approach::Lunge => vec![
            (0.0, stance),
            // Load: sink onto both knees, lean in, square up.
            (0.2, [-0.04, -10.0, 0.6, 26.0, 40.0, 12.0, -2.0, 32.0, 2.0]),
            // Drive: the rear leg straightens behind, the lead knee comes up; off the ground.
            (0.5, [0.03, -14.0, 0.8, 48.0, 55.0, 10.0, -22.0, 8.0, -24.0]),
            // Touch-down on the lead foot, the rear foot coming under.
            (0.78, [-0.035, -8.0, 0.5, 24.0, 36.0, 10.0, 4.0, 34.0, -2.0]),
            (1.0, stance),
        ],
        Approach::Leap => vec![
            (0.0, stance),
            // A deep load.
            (
                0.18,
                [-0.11, -18.0, 0.7, 40.0, 70.0, 26.0, 18.0, 66.0, 22.0],
            ),
            // Take-off: legs driving straight.
            (
                0.32,
                [0.02, -20.0, 0.9, 20.0, 8.0, -14.0, -18.0, 6.0, -28.0],
            ),
            // Flight: knees tucked.
            (0.5, [0.16, -12.0, 1.0, 58.0, 88.0, 0.0, 30.0, 92.0, -10.0]),
            // Reaching for the ground.
            (0.68, [0.06, -10.0, 0.8, 40.0, 30.0, 6.0, 12.0, 40.0, -4.0]),
            // Landing: sink into it.
            (0.8, [-0.1, -14.0, 0.5, 38.0, 64.0, 24.0, 16.0, 60.0, 18.0]),
            (1.0, stance),
        ],
    };
    let (go, land) = kind.travel();
    AnimationClip::from_fn(kind.clip_name(), kind.secs(), keys, false, move |ph| {
        let mut p = PoseBuilder::new(s);
        let v = |i: usize| {
            let k: Vec<(f32, f32)> = frames.iter().map(|(t, f)| (*t, f[i])).collect();
            keyed(ph, &k)
        };
        let (up, pitch, square) = (v(0), v(1), v(2));
        let yaw = -24.0 * (1.0 - square);
        p.rot_move(
            "hips",
            Vec3::new(0.0, -0.045 + up, 0.0),
            2.0 + pitch * 0.3,
            yaw,
            0.0,
        );
        // The trunk turns back toward the stance's facing as the hips square, so the chest
        // and eyes stay on the target.
        let turn = yaw + 24.0;
        spine(
            &mut p,
            [
                -2.0 + pitch * 0.4,
                -3.0 + pitch * 0.4,
                2.0 - pitch * 0.3,
                -8.0 - pitch * 0.4,
            ],
            [-turn * 0.2, -turn * 0.3, 0.0, 0.0],
            [0.0; 4],
        );
        // The guard stays up, a little tighter in flight.
        let tight = (up.max(0.0) / 0.16).min(1.0);
        for (side, guard) in GUARD_ARMS.into_iter().enumerate() {
            arm(
                &mut p,
                side,
                Arm {
                    elbow: guard.elbow + 10.0 * tight,
                    ..guard
                },
            );
        }
        leg(&mut p, 0, v(3), 4.0, v(4), v(5), 0.0);
        leg(&mut p, 1, v(6), 6.0, v(7), v(8), 8.0);
        p.take()
    })
    .with_curve("effort", keys, move |ph| {
        keyed(ph, &[(0.0, 0.2), (go, 0.8), (land, 0.6), (1.0, 0.2)])
    })
    .with_curve("fist", 2, |_| 0.9)
}

/// The right hand of the hammer block: the handle held across in front of the face,
/// pointing left, the forearm up.
const BLOCK_ARM: Arm = Arm {
    flex: 3.0,
    twist: -20.0,
    abduct: 38.0,
    elbow: 138.0,
    wrist: -34.5,
    protract: 6.0,
    elevate: 6.0,
};

/// Everything of the hammer block but the right arm: a fighting stance with the left foot
/// forward and the knees bent, turned a little to the right, chin down behind the handle,
/// the left arm up toward the handle (its `ReachIk` puts the hand on it while `guard` is on).
/// `tremble` (−1..1) shakes the arms under the load.
fn block_body(p: &mut PoseBuilder, tremble: f32) {
    // Low enough that both feet reach the floor; foot IK bends the front knee the rest.
    p.rot_move("hips", Vec3::new(0.0, -0.065, 0.0), 0.0, -14.0, 0.0);
    leg(p, 0, 22.0, 3.0, 34.0, 12.0, 0.0);
    leg(p, 1, -6.0, 5.0, 26.0, 20.0, 0.0);
    spine(p, [-8.0, -6.0, 2.0, -12.0], [5.0, 5.0, 2.0, 2.0], [0.0; 4]);
    arm(
        p,
        0,
        Arm {
            flex: 75.0 + 0.6 * tremble,
            twist: 20.0,
            abduct: 10.0,
            elbow: 95.0 + 0.8 * tremble,
            wrist: -10.0,
            protract: 6.0,
            elevate: 6.0,
        },
    );
}

/// A braced block with the hammer: the handle held across in front of the face in both
/// hands, held while `hammer_guard` stays on.
fn hammer_block(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("hammer_block", 1.2, 25, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let tremble = (ph * 6.0 * std::f32::consts::TAU).sin();
        block_body(&mut p, tremble);
        arm(
            &mut p,
            1,
            Arm {
                flex: BLOCK_ARM.flex + 0.6 * tremble,
                elbow: BLOCK_ARM.elbow + 0.8 * tremble,
                ..BLOCK_ARM
            },
        );
        p.take()
    })
    .with_curve("effort", 25, |ph| {
        0.85 + 0.05 * (ph * 3.0 * std::f32::consts::TAU).sin()
    })
    .with_curve("brace", 2, |_| 1.0)
    .with_curve("guard", 2, |_| 1.0)
    // The left hand closes round the handle (the right holds the hammer anyway).
    .with_curve("fist", 2, |_| 0.85)
}

/// A wince at something hard to watch (a hammer coming down): the shoulders jump up, the
/// chin pulls in and the head turns a little away, the arms pull in with the hands drawn up
/// in front of the body (the right toward the chest), the eyes squeeze
/// (the `effort` curve, which the face reads), then it all lets go.
fn wince(s: &Skeleton) -> AnimationClip {
    let squeeze = [(0.0, 0.0), (0.1, 1.0), (0.4, 0.85), (1.0, 0.0)];
    AnimationClip::from_fn("wince", 1.4, 29, false, |ph| {
        let mut p = PoseBuilder::new(s);
        let k = |keys: &[(f32, f32)]| keyed(ph, keys);
        let jump = k(&[(0.0, 0.0), (0.09, 1.0), (0.35, 0.8), (1.0, 0.0)]);
        let away = k(&[(0.0, 0.0), (0.12, 1.0), (0.5, 0.7), (1.0, 0.0)]);
        p.rot_move(
            "hips",
            Vec3::new(0.0, -0.01 - 0.012 * jump, 0.0),
            0.0,
            0.0,
            0.0,
        );
        spine(
            &mut p,
            [
                -1.0 - 3.0 * jump,
                -5.0 * jump,
                -6.0 * jump,
                -8.0 * away - 1.0,
            ],
            [0.0, -3.0 * away, -6.0 * away, -8.0 * away],
            [0.0, 0.0, 2.0 * away, 4.0 * away],
        );
        for side in 0..2 {
            // The arms pull in and the hands come up in front of the belly, the right one
            // higher, toward the chest.
            let high = if side == 1 { 1.0 } else { 0.0 };
            arm(
                &mut p,
                side,
                Arm {
                    flex: RELAXED.flex + (18.0 + 14.0 * high) * jump,
                    twist: RELAXED.twist + (34.0 + 10.0 * high) * jump,
                    abduct: RELAXED.abduct - 4.0 * jump,
                    elbow: RELAXED.elbow + (50.0 + 30.0 * high) * jump,
                    wrist: RELAXED.wrist + 10.0 * jump,
                    protract: 8.0 * jump,
                    elevate: 14.0 * jump,
                },
            );
            leg(
                &mut p,
                side,
                2.0 + 3.0 * jump,
                1.5,
                3.0 + 6.0 * jump,
                1.0 + 3.0 * jump,
                0.0,
            );
        }
        p.take()
    })
    .with_curve("effort", 29, |ph| keyed(ph, &squeeze))
}

/// Bracing for a fall: arms reaching forward and down to take it, knees bent, chin tucked,
/// held for its length. A ragdoll with [`RagdollTone`](crate::ragdoll::RagdollTone) holds
/// this shape in the air until it lands.
fn fall_brace(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("fall_brace", 1.5, 16, false, |ph| {
        let mut p = PoseBuilder::new(s);
        // Into the shape over the first fifth.
        let k = (ph / 0.2).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        p.rot("hips", -6.0 * k, 0.0, 0.0);
        spine(
            &mut p,
            [-14.0 * k, -10.0 * k, -8.0 * k, -14.0 * k],
            [0.0; 4],
            [0.0; 4],
        );
        for side in 0..2 {
            arm(
                &mut p,
                side,
                Arm {
                    flex: 4.0 + 66.0 * k,
                    twist: 10.0 * k,
                    abduct: 4.0 + 18.0 * k,
                    elbow: 10.0 + 22.0 * k,
                    wrist: -35.0 * k,
                    protract: 8.0 * k,
                    elevate: 4.0 * k,
                },
            );
            leg(
                &mut p,
                side,
                4.0 + [38.0, 26.0][side] * k,
                4.0 * k,
                [62.0, 40.0][side] * k,
                10.0 * k,
                0.0,
            );
        }
        p.take()
    })
    .with_curve("brace", 2, |_| 1.0)
    .with_curve("effort", 16, |ph| (ph / 0.2).clamp(0.0, 1.0) * 0.9)
}

/// Bracing for a fall backward (or to the side), as judo's back breakfall teaches: chin
/// tucked to keep the head off the floor, back rounded, hips sitting down toward the heels
/// and the arms going back and out at about 45° with the palms down to take the floor. A
/// ragdoll with [`RagdollTone`](crate::ragdoll::RagdollTone) holds this shape until it lands.
fn fall_back_brace(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("fall_back_brace", 1.5, 16, false, |ph| {
        let mut p = PoseBuilder::new(s);
        // Into the shape over the first sixth: a startled, fast reaction.
        let k = (ph / 0.16).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        p.rot("hips", 8.0 * k, 0.0, 0.0);
        spine(
            &mut p,
            [-16.0 * k, -14.0 * k, -18.0 * k, -24.0 * k],
            [0.0; 4],
            [0.0; 4],
        );
        for side in 0..2 {
            arm(
                &mut p,
                side,
                Arm {
                    flex: 4.0 - 38.0 * k,
                    twist: -18.0 * k,
                    abduct: -10.0 + 46.0 * k,
                    elbow: 12.0 + 8.0 * k,
                    wrist: 30.0 * k,
                    protract: -6.0 * k,
                    elevate: 6.0 * k,
                },
            );
            leg(
                &mut p,
                side,
                4.0 + [62.0, 50.0][side] * k,
                6.0 * k,
                [96.0, 80.0][side] * k,
                22.0 * k,
                0.0,
            );
        }
        p.take()
    })
    .with_curve("brace", 2, |_| 1.0)
    .with_curve("effort", 16, |ph| (ph / 0.16).clamp(0.0, 1.0) * 0.9)
}

/// Up on tiptoe to take hold of something overhead with the right hand (the paddock's
/// lantern): a glance up and a small dip, the arm sweeping up past the face with the elbow
/// leading while the heels rise, the hand closing at the top and holding while the body
/// balances, then down onto the heels with a little give in the knees. The `reach_hand`
/// curve hands the wrist to a [`ReachIk`](crate::ik::ReachIk) for the hold, and `grip`
/// closes the hand.
fn reach(s: &Skeleton) -> AnimationClip {
    // Phase keys: glance and dip, top of the rise, end of the hold, landing.
    const DIP: f32 = 0.12;
    const TOP: f32 = 0.38;
    const HOLD: f32 = 0.7;
    const LAND: f32 = 0.86;
    let effort = [
        (0.0, 0.0),
        (TOP, 0.35),
        (0.5, 0.45),
        (HOLD, 0.4),
        (1.0, 0.0),
    ];
    let grip = [
        (0.0, 0.0),
        (TOP, 0.05),
        (0.45, 0.6),
        (HOLD, 0.6),
        (0.76, 0.0),
    ];
    let reach_hand = [
        (0.0, 0.0),
        (0.24, 0.0),
        (0.4, 1.0),
        (HOLD, 1.0),
        (0.82, 0.0),
    ];
    AnimationClip::from_fn("reach", 3.0, 61, false, |ph| {
        let mut p = PoseBuilder::new(s);
        let k = |keys: &[(f32, f32)]| keyed(ph, keys);
        // Balancing on the balls of the feet: a slow sway while up.
        let up = k(&[(0.0, 0.0), (TOP, 1.0), (HOLD, 1.0), (0.8, 0.0)]);
        let sway = (ph * 5.0 * std::f32::consts::TAU).sin() * up;
        // Legs: a small dip that keeps the feet flat, then up onto the toes. The ankle points
        // the foot and the toes bend back by as much, so the balls of the feet stay down
        // while the heels and hips rise 6.6 cm (see the toe joint's offset).
        let legs = [
            (0.0, 0.0),
            (DIP, -1.0),
            (TOP, 1.0),
            (HOLD, 1.0),
            (LAND, -1.0),
            (1.0, 0.0),
        ];
        let rise = k(&legs).max(0.0);
        let dip = (-k(&legs)).max(0.0);
        p.rot_move(
            "hips",
            Vec3::new(0.006 * sway, -0.01 + 0.066 * rise - 0.013 * dip, 0.0),
            0.0,
            -3.0 * rise,
            1.0 * sway,
        );
        // The trunk lengthens up the reaching side and turns that shoulder in; the head tips
        // back to watch the hand.
        let look = k(&[
            (0.0, 0.0),
            (DIP, 0.7),
            (TOP, 1.0),
            (HOLD, 1.0),
            (LAND, 0.2),
            (1.0, 0.0),
        ]);
        spine(
            &mut p,
            [2.0 * rise - 2.0 * dip, 5.0 * rise, 12.0 * look, 20.0 * look],
            [2.0 * rise, 5.0 * rise, -2.0 * look, -3.0 * look],
            [
                3.0 * rise - 0.8 * sway,
                4.0 * rise,
                -3.0 * rise,
                -3.0 * rise,
            ],
        );
        // Right arm: the elbow leads the hand up past the face, the arm straightens overhead
        // with the shoulder blade lifting, then comes down with the elbow bending again.
        arm(
            &mut p,
            1,
            Arm {
                flex: k(&[
                    (0.0, 4.0),
                    (DIP, 18.0),
                    (TOP, 162.0),
                    (HOLD, 162.0),
                    (1.0, 4.0),
                ]),
                twist: k(&[
                    (0.0, 0.0),
                    (DIP, 10.0),
                    (TOP, -10.0),
                    (HOLD, -10.0),
                    (1.0, 0.0),
                ]),
                abduct: k(&[
                    (0.0, -15.0),
                    (DIP, -12.0),
                    (TOP, -6.0),
                    (HOLD, -6.0),
                    (1.0, -15.0),
                ]),
                elbow: k(&[
                    (0.0, 12.0),
                    (DIP, 35.0),
                    (0.24, 75.0),
                    (TOP, 14.0),
                    (HOLD, 14.0),
                    (0.84, 55.0),
                    (1.0, 12.0),
                ]),
                wrist: k(&[(0.0, 0.0), (DIP, 15.0), (TOP, 6.0), (HOLD, 6.0), (1.0, 0.0)]),
                protract: k(&[(0.0, 0.0), (TOP, 10.0), (HOLD, 10.0), (1.0, 0.0)]),
                elevate: k(&[
                    (0.0, 0.0),
                    (DIP, 2.0),
                    (TOP, 22.0),
                    (HOLD, 22.0),
                    (1.0, 0.0),
                ]),
            },
        );
        // Left arm: drifts out a little for balance while up on the toes.
        arm(
            &mut p,
            0,
            Arm {
                flex: 4.0 + 10.0 * up,
                twist: 0.0,
                abduct: -15.0 + 4.0 * up + 1.5 * sway,
                elbow: 12.0 + 16.0 * up,
                wrist: 4.0 * up,
                protract: 0.0,
                elevate: 0.0,
            },
        );
        for side in 0..2 {
            leg(
                &mut p,
                side,
                2.0 * (1.0 - rise) + 10.0 * dip,
                1.5,
                3.0 * (1.0 - rise) + 20.0 * dip,
                1.0 - 31.0 * rise + 10.0 * dip,
                31.0 * rise,
            );
        }
        p.take()
    })
    .with_curve("effort", 61, |ph| keyed(ph, &effort))
    .with_curve("grip", 61, |ph| keyed(ph, &grip))
    .with_curve("reach_hand", 61, |ph| keyed(ph, &reach_hand))
}

/// A front double-biceps: a breath in, the fists curl up in front of the chest and the
/// elbows swing out to shoulder height, a hit that overshoots and settles, two squeezes in
/// the hold (fists curl in, shoulders lift, chest up, face strained), then the arms open
/// out, drop and shake loose.
fn flex(s: &Skeleton) -> AnimationClip {
    const HIT: f32 = 0.26;
    const HOLD: f32 = 0.72;
    const POSE: Arm = Arm {
        flex: 6.0,
        twist: -86.0,
        abduct: 54.0,
        elbow: 102.0,
        wrist: 8.0,
        protract: -10.0,
        elevate: 3.0,
    };
    let arms = [
        (0.0, Arm::HANG),
        // The fists come up in front of the belly, then the chest...
        (
            0.1,
            Arm {
                flex: 24.0,
                twist: -12.0,
                abduct: -8.0,
                elbow: 72.0,
                wrist: 12.0,
                protract: 4.0,
                elevate: 0.0,
            },
        ),
        (
            0.18,
            Arm {
                flex: 38.0,
                twist: -45.0,
                abduct: 26.0,
                elbow: 118.0,
                wrist: 22.0,
                protract: 2.0,
                elevate: 6.0,
            },
        ),
        // ...and the elbows swing out and up past the pose and settle back into it.
        (
            HIT,
            Arm {
                flex: 2.0,
                twist: -92.0,
                abduct: 60.0,
                elbow: 110.0,
                wrist: 14.0,
                protract: -12.0,
                elevate: 6.0,
            },
        ),
        (0.34, POSE),
        (HOLD, POSE),
        // Letting go: the elbows drop first with the forearms still bent, then the arms
        // straighten down the sides.
        (
            0.8,
            Arm {
                flex: 22.0,
                twist: -50.0,
                abduct: 30.0,
                elbow: 96.0,
                wrist: 6.0,
                protract: -2.0,
                elevate: 3.0,
            },
        ),
        (
            0.9,
            Arm {
                flex: 10.0,
                twist: -10.0,
                abduct: -10.0,
                elbow: 40.0,
                wrist: 4.0,
                protract: 0.0,
                elevate: 0.0,
            },
        ),
        (1.0, Arm::HANG),
    ];
    // How far into the pose the body is, and two squeezes during the hold.
    let pose = [(0.0, 0.0), (0.1, 0.4), (HIT, 1.0), (HOLD, 1.0), (0.88, 0.0)];
    let squeeze = [
        (0.34, 0.0),
        (0.43, 1.0),
        (0.5, 0.25),
        (0.59, 1.0),
        (0.67, 0.3),
        (HOLD, 0.0),
    ];
    // A breath in before the hit, a breath out and a shrug to shake loose after.
    let breath = [
        (0.0, 0.0),
        (0.1, 1.0),
        (HIT, 0.3),
        (HOLD, 0.3),
        (0.84, -0.6),
        (1.0, 0.0),
    ];
    let shrug = [(0.84, 0.0), (0.9, 1.0), (0.96, 0.0)];
    AnimationClip::from_fn("flex", 3.6, 73, false, |ph| {
        let mut p = PoseBuilder::new(s);
        let k = |keys: &[(f32, f32)]| keyed(ph, keys);
        let (h, sq, br, sh) = (k(&pose), k(&squeeze), k(&breath), k(&shrug));
        // Muscles shake as they lock out, harder on each squeeze.
        let shake = (ph * 32.0 * std::f32::consts::TAU).sin() * h * (0.4 + sq);
        p.rot_move(
            "hips",
            Vec3::new(0.0, -0.01 - 0.015 * h, 0.0),
            -2.0 * h,
            3.0 * h,
            0.0,
        );
        // Chest high and the chin up a touch; the trunk turns a little toward the front leg.
        spine(
            &mut p,
            [
                1.0 * h + 1.5 * br,
                5.0 * h + 2.5 * br + 2.0 * sq,
                -5.0 * h - 1.0 * sq,
                3.0 * h + 1.0 * sq,
            ],
            [0.0, -2.0 * h, -h, 0.0],
            [0.0, 0.0, 0.0, 2.0 * h],
        );
        let a = arm_keyed(ph, &arms);
        for side in 0..2 {
            arm(
                &mut p,
                side,
                Arm {
                    abduct: a.abduct + 3.0 * sq,
                    elbow: a.elbow + 6.0 * sq + 0.8 * shake,
                    wrist: a.wrist + 8.0 * sq,
                    protract: a.protract - 3.0 * sq,
                    elevate: a.elevate + 3.0 * sq + 7.0 * sh,
                    ..a
                },
            );
        }
        // Stance: feet set wider, the left foot a little ahead, knees soft and quads locked.
        leg(
            &mut p,
            0,
            3.0 + 7.0 * h,
            1.5 + 5.0 * h,
            6.0 + 9.0 * h,
            3.0 + 2.0 * h,
            0.0,
        );
        leg(
            &mut p,
            1,
            3.0 - 3.0 * h,
            1.5 + 5.0 * h,
            6.0 + 5.0 * h,
            3.0 + 2.0 * h,
            0.0,
        );
        p.take()
    })
    .with_curve("flex", 73, |ph| {
        keyed(ph, &pose) * (0.85 + 0.15 * keyed(ph, &squeeze))
    })
    .with_curve("effort", 73, |ph| {
        0.3 * keyed(ph, &pose) + 0.55 * keyed(ph, &squeeze)
    })
}

/// An arm pose between key poses, eased like [`keyed`].
fn arm_keyed(phase: f32, keys: &[(f32, Arm)]) -> Arm {
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if phase <= b.0 {
            let t = ((phase - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
            return a.1.lerp(&b.1, t * t * (3.0 - 2.0 * t));
        }
    }
    keys.last().map_or(Arm::HANG, |k| k.1)
}

// ------------------------------------------------------------------------------- get-ups

/// The get-up clips (and graph states and triggers of the same names): from the back and
/// from the front.
pub const GET_UP_CLIPS: [&str; 2] = ["get_up_back", "get_up_front"];

/// Where the hips lie when [`get_up_back`] starts, behind the spot where it ends standing
/// (the body lies with its feet toward −Z, the way it will face), meters.
pub const GET_UP_BACK_HIPS: Vec3 = Vec3::new(0.0, 0.12, 0.75);
/// Where the hips lie when [`get_up_front`] starts (the head toward −Z, the way it will face).
pub const GET_UP_FRONT_HIPS: Vec3 = Vec3::new(0.0, 0.12, 0.45);

impl Arm {
    fn lerp(&self, o: &Arm, w: f32) -> Arm {
        let l = |a: f32, b: f32| a + (b - a) * w;
        Arm {
            flex: l(self.flex, o.flex),
            twist: l(self.twist, o.twist),
            abduct: l(self.abduct, o.abduct),
            elbow: l(self.elbow, o.elbow),
            wrist: l(self.wrist, o.wrist),
            protract: l(self.protract, o.protract),
            elevate: l(self.elevate, o.elevate),
        }
    }
}

/// One key pose of a get-up: joint angles, plus where hands and feet are planted (solved
/// with two-bone IK, so they stay put on the floor between keys that plant them).
#[derive(Clone, Copy)]
struct Key {
    /// Phase of the clip (0..1).
    at: f32,
    /// Hips position in the character's frame (meters) and pitch, yaw, roll (degrees;
    /// +pitch tips the trunk back).
    hips: Vec3,
    pitch: f32,
    yaw: f32,
    roll: f32,
    /// Bend of spine, chest, neck and head (degrees, negative forward).
    spine: [f32; 4],
    arm: [Arm; 2],
    /// Hip flexion, abduction, knee bend and ankle (degrees) per leg.
    leg: [[f32; 4]; 2],
    /// Planted wrists (left, right) and ankles (left, right), character frame, meters.
    plant: [Option<Vec3>; 4],
    /// Which way each planted leg's knee points, character frame (it bends in the plane of
    /// this and the line from hip to ankle).
    knee: [Vec3; 2],
    /// How hard the body works (the `effort` curve).
    effort: f32,
}

/// Wrist and ankle bones in `Key::plant` order, with the joints above them.
const LIMBS: [[&str; 3]; 4] = [
    ["upper_arm_l", "lower_arm_l", "hand_l"],
    ["upper_arm_r", "lower_arm_r", "hand_r"],
    ["upper_leg_l", "lower_leg_l", "foot_l"],
    ["upper_leg_r", "lower_leg_r", "foot_r"],
];
/// Every bone a get-up key poses.
const GET_UP_BONES: [&str; 21] = [
    "hips",
    "spine",
    "chest",
    "neck",
    "head",
    "clavicle_l",
    "clavicle_r",
    "upper_arm_l",
    "upper_arm_r",
    "lower_arm_l",
    "lower_arm_r",
    "hand_l",
    "hand_r",
    "upper_leg_l",
    "upper_leg_r",
    "lower_leg_l",
    "lower_leg_r",
    "foot_l",
    "foot_r",
    "toe_l",
    "toe_r",
];

impl Key {
    /// The first frame of `idle`, where every get-up ends.
    const STAND: Key = Key {
        at: 1.0,
        hips: Vec3::new(0.0, HIPS_HEIGHT - 0.008, 0.0),
        pitch: 0.0,
        yaw: 0.0,
        roll: 0.0,
        spine: [-1.0, 0.0, 1.0, -1.0],
        arm: [RELAXED, RELAXED],
        leg: [[4.0, 1.5, 4.0, 0.0], [4.0, 1.5, 4.0, 0.0]],
        plant: [None; 4],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.0,
    };

    /// Joint angles between `self` and `o` (plants are handled by [`Key::sample`]).
    fn lerp(&self, o: &Key, w: f32) -> Key {
        let l = |a: f32, b: f32| a + (b - a) * w;
        let mut k = *self;
        k.hips = self.hips.lerp(o.hips, w);
        k.pitch = l(self.pitch, o.pitch);
        k.yaw = l(self.yaw, o.yaw);
        k.roll = l(self.roll, o.roll);
        for i in 0..4 {
            k.spine[i] = l(self.spine[i], o.spine[i]);
        }
        for side in 0..2 {
            k.arm[side] = self.arm[side].lerp(&o.arm[side], w);
            for j in 0..4 {
                k.leg[side][j] = l(self.leg[side][j], o.leg[side][j]);
            }
            k.knee[side] = self.knee[side].lerp(o.knee[side], w);
        }
        k.effort = l(self.effort, o.effort);
        k
    }

    /// The pose from the joint angles alone, local and model space.
    fn forward(&self, s: &Skeleton) -> (Vec<Xf>, Vec<Xf>) {
        let mut p = PoseBuilder::new(s);
        let rest = s.rest[s.bone("hips")].t;
        let hips = self.hips * body_scale(s);
        p.rot_move("hips", hips - rest, self.pitch, self.yaw, self.roll);
        spine(&mut p, self.spine, [0.0; 4], [0.0; 4]);
        for side in 0..2 {
            arm(&mut p, side, self.arm[side]);
            let [hip, abduct, knee, ankle] = self.leg[side];
            leg(&mut p, side, hip, abduct, knee, ankle, 0.0);
        }
        let mut local = s.rest.clone();
        for (bone, x) in p.take() {
            let b = s.bone(bone);
            local[b].r = x.r;
            if x.t != Vec3::ZERO {
                local[b].t = x.t;
            }
        }
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        (local, model)
    }

    /// Where each wrist and ankle is: planted, or where the joint angles put it.
    fn ends(&self, s: &Skeleton) -> [Vec3; 4] {
        let (_, model) = self.forward(s);
        let k = body_scale(s);
        std::array::from_fn(|i| {
            self.plant[i].map_or_else(|| model[s.bone(LIMBS[i][2])].t, |p| p * k)
        })
    }

    /// The pose at `phase` through `keys` (sorted by phase), eased in and out of each key,
    /// with planted hands and feet held by IK.
    fn sample(s: &Skeleton, keys: &[Key], phase: f32) -> (Key, Vec<(&'static str, Xf)>) {
        let i = keys
            .windows(2)
            .position(|w| phase <= w[1].at)
            .unwrap_or(keys.len().saturating_sub(2));
        let (a, b) = (&keys[i], &keys[(i + 1).min(keys.len() - 1)]);
        let t = ((phase - a.at) / (b.at - a.at).max(1e-6)).clamp(0.0, 1.0);
        let w = t * t * (3.0 - 2.0 * t);
        let key = a.lerp(b, w);
        let (mut local, mut model) = key.forward(s);
        let (ends_a, ends_b) = (a.ends(s), b.ends(s));
        let mut edit = crate::animator::PoseEdit {
            skeleton: s,
            local: &mut local,
            model: &mut model,
        };
        for (limb, [upper, lower, end]) in LIMBS.iter().enumerate() {
            let planted = |k: &Key| if k.plant[limb].is_some() { 1.0 } else { 0.0 };
            let weight = planted(a) + (planted(b) - planted(a)) * w;
            if weight <= 0.0 {
                continue;
            }
            let (upper, lower, end) = (s.bone(upper), s.bone(lower), s.bone(end));
            let target = ends_a[limb].lerp(ends_b[limb], w);
            let pole = if limb < 2 {
                // Elbows back and out.
                let side = if limb == 0 { -1.0 } else { 1.0 };
                edit.model[s.bone("chest")].r * Vec3::new(0.6 * side, 0.0, 1.0)
            } else {
                key.knee[limb - 2]
            };
            let held = edit.model[end].r;
            crate::ik::two_bone(&mut edit, upper, lower, end, target, pole, weight);
            edit.set_model_rotation(end, held);
        }
        let out = GET_UP_BONES
            .iter()
            .map(|&name| {
                let b = s.bone(name);
                let mut x = local[b];
                if name != "hips" {
                    x.t = Vec3::ZERO;
                }
                (name, x)
            })
            .collect();
        (key, out)
    }
}

/// How much larger a (fitted) skeleton is than the humanoid, by its hips.
fn body_scale(s: &Skeleton) -> f32 {
    s.rest_model[s.bone("hips")].t.y / HIPS_HEIGHT
}

/// A get-up clip through `keys`, with `effort` and `floor` curves (`floor` is 1 while the
/// body is on the ground and the feet don't carry it, so foot IK lets go). `standing` is the
/// phase the feet take the weight.
fn get_up(s: &Skeleton, name: &str, duration: f32, keys: &[Key], standing: f32) -> AnimationClip {
    let samples = (duration * 30.0) as usize + 1;
    let floor = [(0.0, 1.0), (standing - 0.08, 1.0), (standing + 0.08, 0.0)];
    AnimationClip::from_fn(name, duration, samples, false, |ph| {
        Key::sample(s, keys, ph).1
    })
    .with_curve("effort", samples, |ph| Key::sample(s, keys, ph).0.effort)
    .with_curve("floor", 25, |ph| keyed(ph, &floor))
}

/// Getting up from lying on the back (feet toward −Z): sit up on the hands, tuck the legs,
/// roll forward onto the right knee with the left foot planted, push up on the left leg.
fn get_up_back(s: &Skeleton) -> AnimationClip {
    get_up(s, "get_up_back", 2.4, &get_up_back_keys(), 0.7)
}

/// [`get_up_back`]'s keys: lying, sitting up on the hands, tucked, stepping, kneeling,
/// rising, standing.
fn get_up_back_keys() -> [Key; 8] {
    let hz = GET_UP_BACK_HIPS.z;
    let lying = Key {
        at: 0.0,
        hips: GET_UP_BACK_HIPS,
        pitch: 90.0,
        yaw: 0.0,
        roll: 0.0,
        spine: [0.0, 0.0, -4.0, -6.0],
        arm: [Arm {
            flex: 6.0,
            abduct: 0.0,
            elbow: 10.0,
            ..Arm::HANG
        }; 2],
        leg: [[4.0, 6.0, 6.0, -10.0]; 2],
        plant: [
            Some(Vec3::new(-0.4, 0.05, hz + 0.05)),
            Some(Vec3::new(0.4, 0.05, hz + 0.05)),
            Some(Vec3::new(-0.18, 0.1, hz - 0.88)),
            Some(Vec3::new(0.18, 0.1, hz - 0.88)),
        ],
        knee: [Vec3::Y; 2],
        effort: 0.0,
    };
    let sitting = Key {
        at: 0.3,
        hips: Vec3::new(0.0, 0.13, hz),
        pitch: 30.0,
        spine: [-12.0, -10.0, -6.0, -8.0],
        // Hands planted behind the hips, pushing.
        arm: [Arm {
            flex: -40.0,
            twist: 0.0,
            abduct: 12.0,
            elbow: 8.0,
            wrist: -50.0,
            protract: -6.0,
            elevate: 6.0,
        }; 2],
        leg: [[50.0, 6.0, 60.0, -6.0]; 2],
        knee: [Vec3::Y; 2],
        plant: [
            Some(Vec3::new(-0.26, 0.04, hz + 0.28)),
            Some(Vec3::new(0.26, 0.04, hz + 0.28)),
            Some(Vec3::new(-0.15, 0.08, hz - 0.62)),
            Some(Vec3::new(0.15, 0.08, hz - 0.62)),
        ],
        effort: 0.7,
        ..lying
    };
    let tucked = Key {
        at: 0.5,
        hips: Vec3::new(0.0, 0.16, hz - 0.12),
        pitch: -12.0,
        spine: [-14.0, -12.0, 0.0, -4.0],
        // Reaching forward, the right hand on the floor beside the knee.
        arm: [
            Arm {
                flex: 40.0,
                abduct: -4.0,
                elbow: 40.0,
                ..Arm::HANG
            },
            Arm {
                flex: 58.0,
                abduct: 14.0,
                elbow: 10.0,
                wrist: -30.0,
                ..Arm::HANG
            },
        ],
        leg: [[110.0, 10.0, 130.0, 34.0], [100.0, 22.0, 130.0, 44.0]],
        plant: [
            None,
            Some(Vec3::new(0.36, 0.04, hz - 0.4)),
            Some(Vec3::new(-0.14, 0.08, hz - 0.3)),
            Some(Vec3::new(0.16, 0.08, hz - 0.24)),
        ],
        knee: [Vec3::new(0.0, 1.0, -1.0); 2],
        effort: 0.8,
        ..sitting
    };
    // Weight on the right knee and hand; the left foot swings through to the front.
    let stepping = Key {
        at: 0.6,
        hips: Vec3::new(0.0, 0.5, 0.5),
        pitch: -14.0,
        leg: [[120.0, 8.0, 120.0, 10.0], [10.0, 10.0, 110.0, 0.0]],
        plant: [
            None,
            Some(Vec3::new(0.36, 0.04, hz - 0.4)),
            None,
            Some(Vec3::new(0.14, 0.17, 0.8)),
        ],
        knee: [Vec3::new(0.0, 0.2, -1.0), Vec3::new(0.0, -0.4, -1.0)],
        effort: 0.8,
        ..tucked
    };
    let kneeling = Key {
        at: 0.68,
        hips: Vec3::new(0.0, 0.58, 0.4),
        pitch: -8.0,
        spine: [-12.0, -8.0, 2.0, -2.0],
        arm: [
            Arm {
                flex: 30.0,
                abduct: -10.0,
                elbow: 70.0,
                wrist: 10.0,
                ..Arm::HANG
            },
            Arm {
                flex: 12.0,
                abduct: -8.0,
                elbow: 30.0,
                ..Arm::HANG
            },
        ],
        // Left foot planted ahead, under the spot it stands on; right knee on the floor.
        leg: [[96.0, 6.0, 92.0, 2.0], [-6.0, 4.0, 98.0, -40.0]],
        plant: [
            None,
            None,
            Some(Vec3::new(-0.12, 0.08, -0.03)),
            Some(Vec3::new(0.12, 0.13, 0.84)),
        ],
        knee: [Vec3::NEG_Z, Vec3::new(0.0, -1.0, -0.4)],
        effort: 0.7,
        ..stepping
    };
    let rising = Key {
        at: 0.84,
        hips: Vec3::new(0.0, 0.82, 0.12),
        pitch: -10.0,
        spine: [-10.0, -6.0, 2.0, -2.0],
        arm: [
            Arm {
                flex: 18.0,
                elbow: 40.0,
                ..Arm::HANG
            },
            Arm {
                flex: 10.0,
                elbow: 24.0,
                ..Arm::HANG
            },
        ],
        leg: [[52.0, 4.0, 62.0, 8.0], [46.0, 4.0, 86.0, 16.0]],
        plant: [None, None, Some(Vec3::new(-0.12, 0.08, -0.03)), None],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.9,
        ..kneeling
    };
    [
        lying,
        Key { at: 0.08, ..lying },
        sitting,
        tucked,
        stepping,
        kneeling,
        rising,
        Key::STAND,
    ]
}

// ----------------------------------------------------------------------------- ripostes

/// The victim's side of a paired riposte ([`crate::combat::Pairing`]), and the clip's name.
pub const RIPOSTED: &str = "riposted";
/// How long [`RIPOSTED`] lasts, s: it ends lying on the back on `get_up_back`'s first pose.
pub const RIPOSTED_SECS: f32 = 2.2;
/// When the blade goes in, seconds into [`RIPOSTED`] (and into the attacker's riposte).
pub const RIPOSTED_LANDS: f32 = 0.5;

/// Run through after a deflect or a broken guard: thrown open, the body jerks forward round
/// the blade as it goes in at [`RIPOSTED_LANDS`] and sags on it, the hands coming in to it;
/// as it is pulled out the legs give and it drops onto the right knee, sits back catching
/// itself on its hands and lies down on its back, ending on `get_up_back`'s first pose (so
/// the get-up plays straight out of it). `floor` is 1 once the knee is down (foot IK lets
/// go).
fn riposted(s: &Skeleton) -> AnimationClip {
    let at = |secs: f32| secs / RIPOSTED_SECS;
    let [lying, _, sitting, ..] = get_up_back_keys();
    let feet = [
        Some(Vec3::new(-0.12, 0.08, -0.03)),
        Some(Vec3::new(0.12, 0.08, 0.08)),
    ];
    // Guard knocked wide, trunk tipped back, weight on the back foot.
    let open = Key {
        at: at(0.22),
        hips: Vec3::new(0.0, 0.86, 0.06),
        pitch: 8.0,
        yaw: 0.0,
        roll: 0.0,
        spine: [6.0, 4.0, -4.0, -4.0],
        arm: [
            Arm {
                flex: 40.0,
                abduct: 30.0,
                elbow: 30.0,
                ..Arm::HANG
            },
            Arm {
                flex: 24.0,
                abduct: 34.0,
                elbow: 26.0,
                ..Arm::HANG
            },
        ],
        leg: [[10.0, 4.0, 18.0, 6.0], [8.0, 4.0, 22.0, 6.0]],
        plant: [None, None, feet[0], feet[1]],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.6,
    };
    // The blade goes in: the trunk folds round it, the hands come in toward it.
    let reach_in = Arm {
        flex: 34.0,
        twist: 30.0,
        abduct: -6.0,
        elbow: 72.0,
        wrist: 10.0,
        protract: 6.0,
        elevate: 4.0,
    };
    let struck = Key {
        at: at(RIPOSTED_LANDS + 0.08),
        hips: Vec3::new(0.0, 0.84, 0.1),
        pitch: -14.0,
        spine: [-14.0, -12.0, -6.0, -10.0],
        arm: [reach_in, reach_in],
        leg: [[24.0, 4.0, 40.0, 10.0], [22.0, 4.0, 44.0, 10.0]],
        effort: 1.0,
        ..open
    };
    // Sagging on the blade.
    let sagging = Key {
        at: at(0.95),
        hips: Vec3::new(0.0, 0.79, 0.08),
        pitch: -22.0,
        spine: [-18.0, -16.0, -6.0, -16.0],
        arm: [Arm {
            elbow: 60.0,
            flex: 26.0,
            ..reach_in
        }; 2],
        leg: [[34.0, 4.0, 56.0, 12.0], [32.0, 4.0, 58.0, 12.0]],
        effort: 0.5,
        ..struck
    };
    // The blade out, down on the right knee, slumped, the arms hanging.
    let kneeling = Key {
        at: at(1.28),
        hips: Vec3::new(0.0, 0.58, 0.4),
        pitch: -10.0,
        spine: [-14.0, -10.0, -6.0, -20.0],
        arm: [Arm {
            flex: 12.0,
            elbow: 24.0,
            ..Arm::HANG
        }; 2],
        leg: [[96.0, 6.0, 92.0, 2.0], [-6.0, 4.0, 98.0, -40.0]],
        plant: [None, None, feet[0], Some(Vec3::new(0.12, 0.13, 0.84))],
        knee: [Vec3::NEG_Z, Vec3::new(0.0, -1.0, -0.4)],
        effort: 0.2,
        ..sagging
    };
    let keys = [
        Key {
            at: 0.0,
            ..Key::STAND
        },
        open,
        Key {
            at: at(RIPOSTED_LANDS),
            ..open
        },
        struck,
        sagging,
        kneeling,
        // Sat back, caught on the hands behind.
        Key {
            at: at(1.62),
            effort: 0.3,
            ..sitting
        },
        Key {
            at: at(1.95),
            ..lying
        },
        Key { at: 1.0, ..lying },
    ];
    let samples = (RIPOSTED_SECS * 30.0) as usize + 1;
    let floor = [(0.0, 0.0), (at(1.1), 0.0), (at(1.28), 1.0), (1.0, 1.0)];
    let brace = [
        (0.0, 0.2),
        (at(RIPOSTED_LANDS), 0.3),
        (at(0.6), 1.0),
        (at(1.3), 0.5),
        (1.0, 0.0),
    ];
    AnimationClip::from_fn(RIPOSTED, RIPOSTED_SECS, samples, false, |ph| {
        Key::sample(s, &keys, ph).1
    })
    .with_curve("effort", samples, |ph| Key::sample(s, &keys, ph).0.effort)
    .with_curve("floor", 25, move |ph| keyed(ph, &floor))
    .with_curve("brace", 25, move |ph| keyed(ph, &brace))
}

/// A liver shot folding the body onto a knee ([`crate::hurt`]): the legs give and the trunk
/// folds over the right hand clutching the side, the right foot slides back and the body goes
/// down on the right knee, the left forearm on the left knee and the head down, breathing
/// hard; then it pushes up off the left leg and stands. `floor` is 1 while the knee is down
/// (foot IK lets go); `brace` screws up the face.
fn kneel_hurt(s: &Skeleton) -> AnimationClip {
    // The right hand pressed to the lower ribs, the elbow tucked in.
    let clutch = Arm {
        flex: -6.0,
        twist: 55.0,
        abduct: -16.0,
        elbow: 108.0,
        wrist: 12.0,
        protract: 6.0,
        elevate: 0.0,
    };
    let feet = [
        Some(Vec3::new(-0.12, 0.08, -0.03)),
        Some(Vec3::new(0.12, 0.08, 0.05)),
    ];
    let buckled = Key {
        at: 0.1,
        hips: Vec3::new(0.0, 0.8, 0.06),
        pitch: -24.0,
        yaw: 0.0,
        roll: 0.0,
        spine: [-16.0, -14.0, -4.0, -10.0],
        arm: [
            Arm {
                flex: 30.0,
                abduct: -8.0,
                elbow: 30.0,
                ..Arm::HANG
            },
            clutch,
        ],
        leg: [[34.0, 4.0, 56.0, 12.0], [26.0, 6.0, 58.0, 12.0]],
        plant: [None, None, feet[0], feet[1]],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.6,
    };
    // Left foot where it stood, right knee on the floor behind, the left forearm on the knee.
    let down = Key {
        at: 0.24,
        hips: Vec3::new(0.0, 0.58, 0.4),
        pitch: -20.0,
        spine: [-18.0, -14.0, -6.0, -16.0],
        arm: [
            Arm {
                flex: 52.0,
                abduct: -6.0,
                elbow: 84.0,
                ..Arm::HANG
            },
            clutch,
        ],
        leg: [[96.0, 6.0, 92.0, 2.0], [-6.0, 4.0, 98.0, -40.0]],
        plant: [None, None, feet[0], Some(Vec3::new(0.12, 0.13, 0.84))],
        knee: [Vec3::NEG_Z, Vec3::new(0.0, -1.0, -0.4)],
        effort: 0.5,
        ..buckled
    };
    let held = Key {
        at: 0.62,
        spine: [-15.0, -12.0, -4.0, -12.0],
        effort: 0.4,
        ..down
    };
    let rising = Key {
        at: 0.82,
        hips: Vec3::new(0.0, 0.82, 0.12),
        pitch: -10.0,
        spine: [-10.0, -6.0, 2.0, -2.0],
        arm: [
            Arm {
                flex: 18.0,
                elbow: 40.0,
                ..Arm::HANG
            },
            clutch,
        ],
        leg: [[52.0, 4.0, 62.0, 8.0], [46.0, 4.0, 86.0, 16.0]],
        plant: [None, None, feet[0], None],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.9,
        ..held
    };
    let keys = [
        Key {
            at: 0.0,
            ..Key::STAND
        },
        buckled,
        down,
        // Breathing hard on the knee.
        Key {
            at: 0.4,
            spine: [-17.0, -13.0, -5.0, -14.0],
            ..down
        },
        held,
        rising,
        Key::STAND,
    ];
    let duration = 4.0;
    let samples = (duration * 30.0) as usize + 1;
    let floor = [(0.0, 0.0), (0.06, 1.0), (0.84, 1.0), (0.94, 0.0)];
    let brace = [(0.0, 0.3), (0.1, 1.0), (0.62, 0.8), (1.0, 0.0)];
    AnimationClip::from_fn("kneel_hurt", duration, samples, false, |ph| {
        Key::sample(s, &keys, ph).1
    })
    .with_curve("effort", samples, |ph| Key::sample(s, &keys, ph).0.effort)
    .with_curve("floor", 25, move |ph| keyed(ph, &floor))
    .with_curve("brace", 25, move |ph| keyed(ph, &brace))
}

/// Getting up from lying face down (head toward −Z): hands under the shoulders, push up onto
/// hands and knees, bring the left foot forward, push up on the left leg.
fn get_up_front(s: &Skeleton) -> AnimationClip {
    let hz = GET_UP_FRONT_HIPS.z;
    let hands = [
        Some(Vec3::new(-0.24, 0.04, hz - 0.46)),
        Some(Vec3::new(0.24, 0.04, hz - 0.46)),
    ];
    let lying = Key {
        at: 0.0,
        hips: GET_UP_FRONT_HIPS,
        pitch: -90.0,
        yaw: 0.0,
        roll: 0.0,
        // Chin up, the face clear of the floor.
        spine: [2.0, 2.0, 22.0, 18.0],
        // Hands on the floor beside the chest.
        arm: [Arm {
            flex: 20.0,
            abduct: 50.0,
            elbow: 90.0,
            ..Arm::HANG
        }; 2],
        leg: [[0.0, 6.0, 4.0, -40.0]; 2],
        plant: [
            Some(Vec3::new(-0.34, 0.05, hz - 0.22)),
            Some(Vec3::new(0.34, 0.05, hz - 0.22)),
            None,
            None,
        ],
        knee: [Vec3::NEG_Y; 2],
        effort: 0.0,
    };
    // Hands under the shoulders, elbows bent.
    let palms = Key {
        at: 0.16,
        arm: [Arm {
            flex: 80.0,
            twist: 10.0,
            abduct: 16.0,
            elbow: 95.0,
            wrist: -70.0,
            protract: 0.0,
            elevate: 0.0,
        }; 2],
        plant: [hands[0], hands[1], None, None],
        effort: 0.3,
        ..lying
    };
    let pushed = Key {
        at: 0.34,
        hips: Vec3::new(0.0, 0.26, hz),
        pitch: -68.0,
        spine: [10.0, 8.0, 6.0, 0.0],
        arm: [Arm {
            flex: 66.0,
            twist: 10.0,
            abduct: 10.0,
            elbow: 12.0,
            wrist: -66.0,
            protract: 6.0,
            elevate: 4.0,
        }; 2],
        leg: [[10.0, 6.0, 10.0, -40.0]; 2],
        plant: [
            hands[0],
            hands[1],
            Some(Vec3::new(-0.17, 0.13, hz + 0.86)),
            Some(Vec3::new(0.17, 0.13, hz + 0.86)),
        ],
        effort: 0.9,
        ..palms
    };
    let all_fours = Key {
        at: 0.52,
        hips: Vec3::new(0.0, 0.5, hz + 0.05),
        pitch: -78.0,
        spine: [-6.0, -4.0, 14.0, 6.0],
        arm: [Arm {
            flex: 80.0,
            twist: 8.0,
            abduct: 6.0,
            elbow: 6.0,
            wrist: -76.0,
            protract: 8.0,
            elevate: 4.0,
        }; 2],
        leg: [[86.0, 6.0, 92.0, -75.0]; 2],
        plant: [
            hands[0],
            hands[1],
            Some(Vec3::new(-0.14, 0.1, hz + 0.5)),
            Some(Vec3::new(0.14, 0.1, hz + 0.5)),
        ],
        effort: 0.6,
        ..pushed
    };
    // Weight on the hands and right knee; the left foot swings through to the front.
    let stepping = Key {
        at: 0.61,
        hips: Vec3::new(0.0, 0.5, hz),
        pitch: -50.0,
        spine: [-10.0, -8.0, 10.0, 4.0],
        leg: [[130.0, 8.0, 125.0, 25.0], [80.0, 6.0, 95.0, -40.0]],
        // The left foot is held mid-swing, clear of the floor.
        plant: [
            hands[0],
            Some(Vec3::new(0.28, 0.04, hz - 0.3)),
            Some(Vec3::new(-0.12, 0.24, 0.42)),
            Some(Vec3::new(0.14, 0.12, 0.84)),
        ],
        knee: [Vec3::new(0.0, 0.2, -1.0), Vec3::new(0.0, -1.0, -0.4)],
        effort: 0.8,
        ..all_fours
    };
    // The left foot comes through under the body; the right knee stays down.
    let kneeling = Key {
        at: 0.7,
        hips: Vec3::new(0.0, 0.58, 0.4),
        pitch: -12.0,
        spine: [-14.0, -10.0, 4.0, 0.0],
        arm: [
            Arm {
                flex: 34.0,
                abduct: -10.0,
                elbow: 70.0,
                wrist: 10.0,
                ..Arm::HANG
            },
            Arm {
                flex: 30.0,
                abduct: 4.0,
                elbow: 20.0,
                wrist: -20.0,
                ..Arm::HANG
            },
        ],
        leg: [[96.0, 6.0, 92.0, 2.0], [-6.0, 4.0, 98.0, -40.0]],
        plant: [
            None,
            None,
            Some(Vec3::new(-0.12, 0.08, -0.03)),
            Some(Vec3::new(0.12, 0.13, 0.84)),
        ],
        knee: [Vec3::NEG_Z, Vec3::new(0.0, -1.0, -0.4)],
        effort: 0.7,
        ..stepping
    };
    let rising = Key {
        at: 0.86,
        hips: Vec3::new(0.0, 0.82, 0.12),
        pitch: -10.0,
        spine: [-10.0, -6.0, 2.0, -2.0],
        arm: [
            Arm {
                flex: 18.0,
                elbow: 40.0,
                ..Arm::HANG
            },
            Arm {
                flex: 10.0,
                elbow: 24.0,
                ..Arm::HANG
            },
        ],
        leg: [[52.0, 4.0, 62.0, 8.0], [46.0, 4.0, 86.0, 16.0]],
        plant: [None, None, Some(Vec3::new(-0.12, 0.08, -0.03)), None],
        knee: [Vec3::NEG_Z; 2],
        effort: 0.9,
        ..kneeling
    };
    let keys = [
        lying,
        Key { at: 0.06, ..lying },
        palms,
        pushed,
        all_fours,
        stepping,
        kneeling,
        rising,
        Key::STAND,
    ];
    get_up(s, "get_up_front", 2.0, &keys, 0.72)
}

/// Every clip the humanoid graph uses.
pub fn clips(s: &Skeleton) -> Vec<AnimationClip> {
    vec![
        idle(s),
        walk_slow(s),
        walk(s),
        run(s),
        sprint(s),
        arms_folded(s),
        sidestep_left(s),
        sidestep_right(s),
        backpedal(s),
        jog_left(s),
        jog_right(s),
        jog_back(s),
        run_left(s),
        run_right(s),
        run_back(s),
        lean(s, "lean_left", 1.0),
        lean(s, "lean_right", -1.0),
        lean_pitch(s, "lean_forward", 1.0),
        lean_pitch(s, "lean_back", -1.0),
        grade_posture(s, "climb", 1.0),
        grade_posture(s, "descend", -1.0),
        heavy_swing(s),
        block(s),
        hammer_block(s),
        guard(s),
        wince(s),
        reach(s),
        flex(s),
        get_up_back(s),
        get_up_front(s),
        riposted(s),
        fall_brace(s),
        fall_back_brace(s),
        punch(s, Punch::Jab),
        punch(s, Punch::Cross),
        punch(s, Punch::Hook),
        punch(s, Punch::Uppercut),
        punch(s, Punch::BodyHook),
        kneel_hurt(s),
        evasion(s, Evasion::SlipLeft),
        evasion(s, Evasion::SlipRight),
        evasion(s, Evasion::Roll),
        evasion(s, Evasion::PullBack),
        approach(s, Approach::Lunge),
        approach(s, Approach::Leap),
    ]
    .into_iter()
    .chain(super::weapons::clips(s))
    .collect()
}

/// Locomotion (idle/walk/run by `speed`, leaning by `turn`), plus `heavy_swing` (also on
/// the contract's `attack_heavy`), `block` (on its trigger or while `blocking` > 0.5, held
/// while it stays there), `hammer_block` (while `hammer_guard` > 0.5), `guard` (a boxer's
/// guard while `guarding` > 0.5), `reach`, `flex` and `wince` actions, and getting up off the floor
/// (`get_up_back`, `get_up_front`: triggered by the ragdoll when a limp body comes round), and
/// `fall_brace` and `fall_back_brace` (bracing for a fall forward or backward, on their
/// triggers from anywhere), and `kneel_hurt` (folding onto a knee from a liver shot, on its
/// trigger from anywhere), and the weapon stances and carries ([`super::weapons`]: `sword_stance`,
/// `shield_guard`, `greatsword_stance` while their parameters are up, and arms-only layers
/// weighted by `sword_carry`, `greatsword_carry` and `bow_carry`).
pub fn graph() -> AnimGraph {
    graph_with(GRAPH, &Locomotion::default())
}

/// Which clips a [`graph_with`] moves with, and the speeds (m/s) they cover.
#[derive(Debug, Clone, PartialEq)]
pub struct Locomotion {
    pub idle: String,
    /// A slower walk between standing and walking; empty for none (then the walk fades into
    /// standing).
    pub slow_walk: String,
    pub slow_walk_speed: f32,
    pub walk: String,
    pub walk_speed: f32,
    pub run: String,
    pub run_speed: f32,
    /// Flat out, above the run; empty for none.
    pub sprint: String,
    pub sprint_speed: f32,
    /// A motion-matching database in the library to move with while going forward (the
    /// blends above take over for strafing and backpedalling); empty for none.
    pub motion: String,
}

impl Default for Locomotion {
    fn default() -> Self {
        Self {
            idle: "idle".into(),
            slow_walk: "walk_slow".into(),
            slow_walk_speed: SLOW_WALK_SPEED,
            walk: "walk".into(),
            walk_speed: WALK_SPEED,
            run: "run".into(),
            run_speed: RUN_SPEED,
            sprint: "sprint".into(),
            sprint_speed: SPRINT_SPEED,
            motion: String::new(),
        }
    }
}

/// From this speed (m/s) up a weapon stance walks: the legs step with the locomotion and the
/// stance holds the upper body.
const STANCE_STEPPING: f32 = 0.3;

/// The humanoid graph named `name`, moving with other locomotion clips (motion capture,
/// say); the actions stay the humanoid's. Walk and run clips must start on the right foot's
/// strike, like the humanoid's, so they blend in step.
pub fn graph_with(name: &str, motion: &Locomotion) -> AnimGraph {
    // One speed blend per direction of travel relative to facing; `heading` (radians,
    // positive to the left) blends the two nearest, all in step.
    let by_speed = |clips: &[(f32, &str)]| NodeDef::Blend1d {
        param: "speed".into(),
        children: clips
            .iter()
            .filter(|(_, clip)| !clip.is_empty())
            .map(|&(at, clip)| Blend1dChild {
                at,
                node: NodeDef::clip(clip),
            })
            .collect(),
        sync: true,
        match_speed: true,
    };
    let ahead = by_speed(&[
        (0.0, &motion.idle),
        (motion.slow_walk_speed, &motion.slow_walk),
        (motion.walk_speed, &motion.walk),
        (motion.run_speed, &motion.run),
        (motion.sprint_speed, &motion.sprint),
    ]);
    let back = || {
        by_speed(&[
            (0.0, &motion.idle),
            (BACKPEDAL_SPEED, "backpedal"),
            (BACK_JOG_SPEED, "jog_back"),
            (BACK_RUN_SPEED, "run_back"),
        ])
    };
    let pi = std::f32::consts::PI;
    let locomotion = NodeDef::Blend1d {
        param: "heading".into(),
        children: [
            (-pi, back()),
            (
                -pi / 2.0,
                by_speed(&[
                    (0.0, &motion.idle),
                    (SIDESTEP_SPEED, "sidestep_right"),
                    (SIDE_JOG_SPEED, "jog_right"),
                    (SIDE_RUN_SPEED, "run_right"),
                ]),
            ),
            (0.0, ahead),
            (
                pi / 2.0,
                by_speed(&[
                    (0.0, &motion.idle),
                    (SIDESTEP_SPEED, "sidestep_left"),
                    (SIDE_JOG_SPEED, "jog_left"),
                    (SIDE_RUN_SPEED, "run_left"),
                ]),
            ),
            (pi, back()),
        ]
        .into_iter()
        .map(|(at, node)| Blend1dChild { at, node })
        .collect(),
        sync: true,
        match_speed: false,
    };
    // Captured moves carry their own leans into turns, starts and stops.
    let (locomotion, lean) = if motion.motion.is_empty() {
        (locomotion, Weight::default())
    } else {
        (
            NodeDef::MotionMatch {
                database: motion.motion.clone(),
                fallback: Box::new(locomotion),
            },
            Weight {
                value: 0.0,
                param: None,
            },
        )
    };
    let leaning = NodeDef::Additive {
        base: Box::new(locomotion),
        additive: Box::new(NodeDef::Blend1d {
            param: "turn".into(),
            children: vec![
                Blend1dChild {
                    at: -1.0,
                    node: NodeDef::clip("lean_right"),
                },
                Blend1dChild {
                    at: 0.0,
                    node: NodeDef::Rest,
                },
                Blend1dChild {
                    at: 1.0,
                    node: NodeDef::clip("lean_left"),
                },
            ],
            sync: false,
            match_speed: false,
        }),
        weight: lean.clone(),
    };
    // Leaning into a start and sitting back into a stop, by `accel` (−1..1).
    let leaning = NodeDef::Additive {
        base: Box::new(leaning),
        additive: Box::new(NodeDef::Blend1d {
            param: "accel".into(),
            children: vec![
                Blend1dChild {
                    at: -1.0,
                    node: NodeDef::clip("lean_back"),
                },
                Blend1dChild {
                    at: 0.0,
                    node: NodeDef::Rest,
                },
                Blend1dChild {
                    at: 1.0,
                    node: NodeDef::clip("lean_forward"),
                },
            ],
            sync: false,
            match_speed: false,
        }),
        weight: lean,
    };
    // Leaning into a climb and watching the steps on the way down, by `grade` (−1..1).
    let leaning = NodeDef::Additive {
        base: Box::new(leaning),
        additive: Box::new(NodeDef::Blend1d {
            param: "grade".into(),
            children: vec![
                Blend1dChild {
                    at: -1.0,
                    node: NodeDef::clip("descend"),
                },
                Blend1dChild {
                    at: 0.0,
                    node: NodeDef::Rest,
                },
                Blend1dChild {
                    at: 1.0,
                    node: NodeDef::clip("climb"),
                },
            ],
            sync: false,
            match_speed: false,
        }),
        weight: Weight::default(),
    };
    // Arms folded while standing about or stepping round, by `arms_folded` (0..1; ease it).
    let leaning = NodeDef::Layer {
        base: Box::new(leaning),
        overlay: Box::new(NodeDef::clip("arms_folded")),
        mask: BoneMaskDef {
            bones: UPPER_ARM.iter().map(|b| b.to_string()).collect(),
            include_children: true,
        },
        weight: Weight::param("arms_folded"),
        additive: false,
    };
    // Carrying a weapon about (`sword_carry`, `greatsword_carry`, `bow_carry`, 0..1): the
    // arms hold it while the legs walk.
    let leaning = ["sword_carry", "greatsword_carry", "bow_carry"]
        .into_iter()
        .fold(leaning, |base, carry| NodeDef::Layer {
            base: Box::new(base),
            overlay: Box::new(NodeDef::clip(carry)),
            mask: BoneMaskDef {
                bones: UPPER_ARM.iter().map(|b| b.to_string()).collect(),
                include_children: true,
            },
            weight: Weight::param(carry),
            additive: false,
        });
    // A weapon stance standing still, or stepping and circling in guard: the stance's upper
    // body (its `_moving` clip, over square hips) from the spine up over the legs'
    // locomotion, by speed.
    let stance = |name: &str| NodeDef::Blend1d {
        param: "speed".into(),
        children: vec![
            Blend1dChild {
                at: 0.0,
                node: NodeDef::clip(name),
            },
            Blend1dChild {
                at: STANCE_STEPPING,
                node: NodeDef::Layer {
                    base: Box::new(leaning.clone()),
                    overlay: Box::new(NodeDef::clip(&format!("{name}_moving"))),
                    mask: BoneMaskDef {
                        bones: vec!["spine".into()],
                        include_children: true,
                    },
                    weight: Weight::default(),
                    additive: false,
                },
            },
        ],
        sync: false,
        match_speed: false,
    };
    let [sword_stance, shield_guard, greatsword_stance] =
        ["sword_stance", "shield_guard", "greatsword_stance"].map(stance);
    let state = |name: &str, node: NodeDef| StateDef {
        name: name.into(),
        node,
        speed: 1.0,
    };
    AnimGraph {
        name: name.into(),
        root: NodeDef::StateMachine {
            states: vec![
                state("move", leaning),
                state("heavy_swing", NodeDef::one_shot("heavy_swing")),
                state("block", NodeDef::clip("block")),
                state("hammer_block", NodeDef::clip("hammer_block")),
                state("guard", NodeDef::clip("guard")),
                state("reach", NodeDef::one_shot("reach")),
                state("flex", NodeDef::one_shot("flex")),
                state("wince", NodeDef::one_shot("wince")),
                state("get_up_back", NodeDef::one_shot("get_up_back")),
                state("get_up_front", NodeDef::one_shot("get_up_front")),
                state("fall_brace", NodeDef::one_shot("fall_brace")),
                state("fall_back_brace", NodeDef::one_shot("fall_back_brace")),
                state("kneel_hurt", NodeDef::one_shot("kneel_hurt")),
                state("sword_stance", sword_stance),
                state("shield_guard", shield_guard),
                state("greatsword_stance", greatsword_stance),
            ],
            transitions: vec![
                TransitionDef::new(
                    "move",
                    "heavy_swing",
                    Condition::trigger("heavy_swing"),
                    0.2,
                ),
                // The contract's name for it.
                TransitionDef::new(
                    "move",
                    "heavy_swing",
                    Condition::trigger("attack_heavy"),
                    0.2,
                ),
                TransitionDef::new("heavy_swing", "move", Condition::Finished, 0.35).inertialize(),
                TransitionDef::new("move", "block", Condition::trigger("block"), 0.15)
                    .inertialize(),
                // Blocking without a trigger (a mover's `LocomotionState.blocking`).
                TransitionDef::new(
                    "move",
                    "block",
                    Condition::param("blocking", Cmp::Gt, 0.5),
                    0.15,
                )
                .inertialize(),
                TransitionDef::new(
                    "block",
                    "move",
                    Condition::param("blocking", Cmp::Lt, 0.5),
                    0.3,
                ),
                TransitionDef::new(
                    "move",
                    "hammer_block",
                    Condition::param("hammer_guard", Cmp::Gt, 0.5),
                    0.15,
                )
                .inertialize(),
                TransitionDef::new(
                    "hammer_block",
                    "move",
                    Condition::param("hammer_guard", Cmp::Lt, 0.5),
                    0.3,
                ),
                // A boxer's guard while `guarding` is up (a sparring partner).
                TransitionDef::new(
                    "move",
                    "guard",
                    Condition::param("guarding", Cmp::Gt, 0.5),
                    0.3,
                )
                .inertialize(),
                TransitionDef::new(
                    "guard",
                    "move",
                    Condition::param("guarding", Cmp::Lt, 0.5),
                    0.4,
                ),
                // Weapon stances while their parameters are up: sword and shield ready
                // (`sword_stance`), the shield raised over it (`shield_guard`, from either),
                // both hands on a great sword (`greatsword_stance`).
                TransitionDef::new(
                    "move",
                    "sword_stance",
                    Condition::param("sword_stance", Cmp::Gt, 0.5),
                    0.3,
                )
                .inertialize(),
                TransitionDef::new(
                    "sword_stance",
                    "move",
                    Condition::param("sword_stance", Cmp::Lt, 0.5),
                    0.4,
                ),
                TransitionDef::new(
                    "move",
                    "shield_guard",
                    Condition::param("shield_guard", Cmp::Gt, 0.5),
                    0.15,
                )
                .inertialize(),
                TransitionDef::new(
                    "sword_stance",
                    "shield_guard",
                    Condition::param("shield_guard", Cmp::Gt, 0.5),
                    0.15,
                )
                .inertialize(),
                TransitionDef::new(
                    "shield_guard",
                    "sword_stance",
                    Condition::param("shield_guard", Cmp::Lt, 0.5),
                    0.25,
                ),
                TransitionDef::new(
                    "move",
                    "greatsword_stance",
                    Condition::param("greatsword_stance", Cmp::Gt, 0.5),
                    0.3,
                )
                .inertialize(),
                TransitionDef::new(
                    "greatsword_stance",
                    "move",
                    Condition::param("greatsword_stance", Cmp::Lt, 0.5),
                    0.4,
                ),
                TransitionDef::new("move", "reach", Condition::trigger("reach"), 0.25),
                TransitionDef::new("reach", "move", Condition::Finished, 0.35).inertialize(),
                TransitionDef::new("move", "wince", Condition::trigger("wince"), 0.08),
                TransitionDef::new("wince", "move", Condition::Finished, 0.35).inertialize(),
                TransitionDef::new("move", "flex", Condition::trigger("flex"), 0.3),
                TransitionDef::new("flex", "move", Condition::Finished, 0.4).inertialize(),
                // From wherever the body was when it went limp; the ragdoll's own blend
                // covers the change, so the switch is instant.
                TransitionDef::new("*", "get_up_back", Condition::trigger("get_up_back"), 0.0),
                TransitionDef::new("*", "get_up_front", Condition::trigger("get_up_front"), 0.0),
                TransitionDef::new("get_up_back", "move", Condition::Finished, 0.3).inertialize(),
                TransitionDef::new("get_up_front", "move", Condition::Finished, 0.3).inertialize(),
                TransitionDef::new("*", "fall_brace", Condition::trigger("fall_brace"), 0.15),
                TransitionDef::new("fall_brace", "move", Condition::Finished, 0.4).inertialize(),
                TransitionDef::new(
                    "*",
                    "fall_back_brace",
                    Condition::trigger("fall_back_brace"),
                    0.12,
                ),
                TransitionDef::new("fall_back_brace", "move", Condition::Finished, 0.4)
                    .inertialize(),
                TransitionDef::new("*", "kneel_hurt", Condition::trigger("kneel_hurt"), 0.15),
                TransitionDef::new("kneel_hurt", "move", Condition::Finished, 0.4).inertialize(),
            ],
            initial: Some("move".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphInstance;
    use crate::muscle::MorphWeights;
    use std::sync::Arc;

    #[test]
    fn skeleton_proportions() {
        let s = skeleton();
        let y = |b: &str| joint(&s, b).y;
        assert!((y("foot_r") - 0.08).abs() < 1e-5);
        assert!((y("head") - 1.60).abs() < 1e-5);
        // Right arm hangs to +X; its local X is the elbow's bend axis.
        let elbow = joint(&s, "lower_arm_r");
        assert!(elbow.x > 0.3 && elbow.y < 1.25);
    }

    #[test]
    fn arm_rotations_bend_the_way_the_clips_expect() {
        let s = skeleton();
        let mut local = s.rest.clone();
        let mut model = s.rest_model.clone();
        let mut p = PoseBuilder::new(&s);
        // Elbow flexion and shoulder flexion bring the hand forward (−Z), on both sides.
        for side in 0..2 {
            arm(
                &mut p,
                side,
                Arm {
                    flex: 30.0,
                    elbow: 90.0,
                    ..Arm::HANG
                },
            );
        }
        for (bone, x) in p.take() {
            local[s.bone(bone)].r = x.r;
        }
        s.to_model(&local, &mut model);
        for side in 0..2 {
            let hand = model[s.bone(HAND[side])].t;
            let elbow = model[s.bone(LOWER_ARM[side])].t;
            assert!(hand.z < elbow.z - 0.15, "{side}: hand {hand} elbow {elbow}");
            assert!(
                hand.x * S[side] > 0.05,
                "{side}: hand crossed the body: {hand}"
            );
        }
    }

    #[test]
    fn graph_compiles_and_swing_has_effort() {
        let s = Arc::new(skeleton());
        let clips: Vec<Arc<AnimationClip>> = clips(&s).into_iter().map(Arc::new).collect();
        let lookup = |n: &str| clips.iter().find(|c| c.name == n).cloned();
        let g = GraphInstance::compile(&graph(), &s, &lookup).unwrap();
        assert!(g.in_state("move"));
        for clip in GET_UP_CLIPS {
            assert!(lookup(clip).is_some(), "{clip}");
        }
        let swing = lookup("heavy_swing").unwrap();
        assert!(swing.curve("effort", 1.0).unwrap() > 0.9);
        assert!(swing.curve("effort", 0.0).unwrap() < 0.05);
    }

    /// Model-space pose of `clip` at `phase` (0..1).
    fn model_at(s: &Skeleton, clip: &AnimationClip, phase: f32) -> Vec<Xf> {
        let binding = crate::clip::ClipBinding::new(clip, s);
        let mut local = s.rest.clone();
        clip.sample(phase * clip.duration, &binding, &mut local);
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        model
    }

    #[test]
    fn walk_arms_reach_forward_more_than_back_with_a_soft_elbow() {
        let s = skeleton();
        let clip = walk(&s);
        for side in 0..2 {
            let (mut front, mut back, mut bend_min) = (0.0f32, 0.0f32, f32::MAX);
            for i in 0..60 {
                let m = model_at(&s, &clip, i as f32 / 60.0);
                let shoulder = m[s.bone(UPPER_ARM[side])].t;
                let elbow = m[s.bone(LOWER_ARM[side])].t;
                let hand = m[s.bone(HAND[side])].t;
                // The body faces −Z.
                let reach = shoulder.z - hand.z;
                front = front.max(reach);
                back = back.max(-reach);
                let bend = (elbow - shoulder).angle_between(hand - elbow).to_degrees();
                bend_min = bend_min.min(bend);
            }
            assert!(
                front > 0.12 && front > back * 1.3,
                "{side}: {front} vs {back}"
            );
            assert!(back > 0.05, "{side}: the arm never swings back ({back})");
            assert!(bend_min > 8.0, "{side}: elbow locks straight ({bend_min}°)");
        }
    }

    #[test]
    fn sprint_pumps_the_arms_higher_and_folds_the_heel_higher_than_the_run() {
        let s = skeleton();
        // Highest hand and highest heel over a cycle, relative to the chest and the hips.
        let reach = |clip: &AnimationClip| {
            let (mut hand, mut heel, mut lean) = (f32::MIN, f32::MIN, 0.0f32);
            for i in 0..40 {
                let m = model_at(&s, clip, i as f32 / 40.0);
                let chest = m[s.bone("chest")].t;
                let hips = m[s.bone("hips")].t;
                hand = hand.max(m[s.bone(HAND[1])].t.y - chest.y);
                heel = heel.max(m[s.bone(FOOT[1])].t.y - hips.y);
                let up = (m[s.bone("neck")].t - hips).normalize();
                lean += up.z / 40.0;
            }
            (hand, heel, lean)
        };
        let (run_hand, run_heel, run_lean) = reach(&run(&s));
        let (hand, heel, lean) = reach(&sprint(&s));
        assert!(hand > run_hand + 0.08, "hand {hand} vs run {run_hand}");
        assert!(heel > run_heel + 0.08, "heel {heel} vs run {run_heel}");
        // The body faces −Z: a forward lean tips the neck toward −Z.
        assert!(lean < run_lean - 0.05, "lean {lean} vs run {run_lean}");
    }

    #[test]
    fn sidesteps_shuffle_low_with_the_hands_up() {
        let s = skeleton();
        let clips = clips(&s);
        let clip = |n: &str| clips.iter().find(|c| c.name == n).unwrap();
        // Lowest hips, and highest hand above the hips, over a cycle.
        let stance = |c: &AnimationClip| {
            let (mut hips, mut hand) = (f32::MAX, f32::MIN);
            for i in 0..20 {
                let m = model_at(&s, c, i as f32 / 20.0);
                let y = m[s.bone("hips")].t.y;
                hips = hips.min(y);
                for h in HAND {
                    hand = hand.max(m[s.bone(h)].t.y - y);
                }
            }
            (hips, hand)
        };
        let (back_hips, back_hand) = stance(clip("backpedal"));
        for name in ["sidestep_left", "sidestep_right"] {
            let (hips, hand) = stance(clip(name));
            assert!(
                hips < back_hips - 0.03,
                "{name}: hips {hips} vs {back_hips}"
            );
            assert!(
                hand > back_hand + 0.08,
                "{name}: hand {hand} vs {back_hand}"
            );
        }
    }

    #[test]
    fn folded_arms_cross_in_front_of_the_chest() {
        let s = skeleton();
        let m = model_at(&s, &arms_folded(&s), 0.0);
        let chest = m[s.bone("chest")].t;
        let (left, right) = (m[s.bone(HAND[0])].t, m[s.bone(HAND[1])].t);
        // Each hand crosses to the other side, in front of the chest (the body faces −Z), the
        // left forearm over the right.
        assert!(right.x < 0.0 && left.x > 0.0, "hands {left} {right}");
        assert!(
            right.z < chest.z - 0.12 && left.z < chest.z - 0.12,
            "hands {left} {right}"
        );
        assert!(left.y > right.y + 0.03, "hands {left} {right}");
        assert!(
            (right.y - chest.y).abs() < 0.12,
            "hands {left} {right} chest {chest}"
        );
    }

    #[test]
    fn standing_shifts_the_weight_from_leg_to_leg_with_the_arms_at_the_sides() {
        let s = skeleton();
        let clip = idle(&s);
        let at = |ph: f32, b: &str| model_at(&s, &clip, ph)[s.bone(b)].t;
        // On the left leg (−X) a third of the way in, on the right three quarters in; the
        // free knee eases forward.
        assert!(at(0.3, "hips").x < -0.04, "{}", at(0.3, "hips"));
        assert!(at(0.75, "hips").x > 0.04, "{}", at(0.75, "hips"));
        assert!(at(0.3, LOWER_LEG[1]).z < at(0.3, LOWER_LEG[0]).z - 0.03);
        assert!(at(0.75, LOWER_LEG[0]).z < at(0.75, LOWER_LEG[1]).z - 0.03);
        // The hands hang close beside the thighs, not out to the sides.
        for ph in [0.0, 0.3, 0.75] {
            for side in 0..2 {
                let out = (at(ph, HAND[side]).x - at(ph, UPPER_LEG[side]).x).abs();
                assert!(out < 0.22, "hand {side} {out} m out from the hip at {ph}");
            }
        }
    }

    #[test]
    fn a_wince_draws_the_hands_in_and_up() {
        let s = skeleton();
        let clip = wince(&s);
        let at = |ph: f32, b: &str| model_at(&s, &clip, ph)[s.bone(b)].t;
        for (side, hand) in HAND.iter().enumerate() {
            let (calm, flinch) = (at(0.0, hand), at(0.15, hand));
            assert!(
                flinch.x.abs() < calm.x.abs() - 0.03,
                "{side}: {calm} -> {flinch}"
            );
            assert!(flinch.y > calm.y + 0.15, "{side}: {calm} -> {flinch}");
        }
        assert!(at(0.15, HAND[1]).y > at(0.15, HAND[0]).y + 0.05);
    }

    #[test]
    fn get_ups_start_lying_and_end_on_the_first_idle_frame() {
        let s = skeleton();
        let idle = model_at(&s, &idle(&s), 0.0);
        for (clip, up) in [(get_up_back(&s), 1.0), (get_up_front(&s), -1.0)] {
            let start = model_at(&s, &clip, 0.0);
            let hips = start[s.bone("hips")].t;
            assert!(hips.y < 0.25, "{}: hips at {hips}", clip.name);
            // The chest faces up for the back, down for the front.
            let front = start[s.bone("chest")].r * Vec3::NEG_Z;
            assert!(front.y * up > 0.9, "{}: chest faces {front}", clip.name);
            let end = model_at(&s, &clip, 1.0);
            for b in 1..s.len() {
                let angle = end[b].r.angle_between(idle[b].r).to_degrees();
                assert!(angle < 1.0, "{}: {} off by {angle}°", clip.name, s.names[b]);
            }
            let off = (end[s.bone("hips")].t - idle[s.bone("hips")].t).length();
            assert!(off < 0.01, "{}: hips {off} m from idle", clip.name);
        }
    }

    #[test]
    fn get_ups_stay_on_the_floor_and_the_standing_foot_stays_put() {
        let s = skeleton();
        for clip in [get_up_back(&s), get_up_front(&s)] {
            let mut planted = Vec::new();
            for i in 0..=60 {
                let ph = i as f32 / 60.0;
                let m = model_at(&s, &clip, ph);
                // Joint centers sit at least 4 cm inside the skin: nothing sinks deeper.
                for (b, x) in m.iter().enumerate().skip(1) {
                    assert!(
                        x.t.y > -0.04,
                        "{} at {ph:.2}: {} at {}",
                        clip.name,
                        s.names[b],
                        x.t
                    );
                }
                if ph >= 0.7 {
                    planted.push(m[s.bone("foot_l")].t);
                }
            }
            let slip = planted
                .iter()
                .map(|p| (*p - planted[0]).with_y(0.0).length())
                .fold(0.0, f32::max);
            assert!(slip < 0.08, "{}: the left foot slides {slip} m", clip.name);
        }
    }

    #[test]
    fn mesh_has_every_muscle_target() {
        let mesh = mesh(Outfit::Athlete);
        assert!(mesh.triangle_count() > 8000, "{}", mesh.triangle_count());
        let morphs = MorphWeights::new(mesh.morph_names());
        for m in muscles() {
            assert!(morphs.index_of(&m.bulge).is_some(), "no target {}", m.bulge);
        }
        // The shirt covers the forearms, the athlete's are bare and veined.
        let s = skeleton();
        let forearm = s.bone("lower_arm_r") as u16;
        let bare = mesh
            .vertices
            .iter()
            .filter(|v| v.joints[0] == forearm && v.weights[0] > 0.9)
            .all(|v| v.mask.z > 0.5);
        assert!(bare);
        let shirt = super::mesh(Outfit::Shirt);
        assert!(
            shirt
                .vertices
                .iter()
                .filter(|v| v.joints[0] == forearm && v.weights[0] > 0.9)
                .all(|v| v.mask.x > 0.5)
        );
    }
}
