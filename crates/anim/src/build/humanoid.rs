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
    AnimGraph, Blend1dChild, Cmp, Condition, NodeDef, StateDef, TransitionDef, Weight,
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
pub const RUN_SPEED: f32 = 3.8;
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

fn idle(s: &Skeleton) -> AnimationClip {
    AnimationClip::from_fn("idle", 3.2, 33, true, |ph| {
        let mut p = PoseBuilder::new(s);
        let breath = cycle_sin(ph);
        let sway = cycle_sin(ph);
        p.rot_move(
            "hips",
            Vec3::new(0.012 * sway, -0.008, 0.0),
            0.0,
            2.0 * sway,
            -1.5 * sway,
        );
        spine(
            &mut p,
            [-1.0, 1.2 * breath, 1.0, -1.0 - 0.6 * breath],
            [-sway, -0.8 * sway, 0.5 * sway, 1.0 * sway],
            [1.0 * sway, 0.5 * sway, -0.8 * sway, -0.8 * sway],
        );
        for (side, s) in S.iter().enumerate() {
            arm(
                &mut p,
                side,
                Arm {
                    elevate: 1.2 * breath,
                    flex: 4.0 + 1.0 * breath,
                    ..Arm::HANG
                },
            );
            let k = 4.0 + 2.0 * s * sway;
            leg(&mut p, side, 2.0 + k * 0.5, 1.5, k, k * 0.5 - 2.0, 0.0);
        }
        p.take()
    })
}

fn walk(s: &Skeleton) -> AnimationClip {
    walk_with(s, "walk", 1.0, 1.0)
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
            arm(
                &mut p,
                side,
                Arm {
                    flex: -16.0 * swing * cq + 2.0,
                    elbow: 20.0 - 6.0 * swing * cq,
                    protract: -3.0 * swing * cq,
                    ..Arm::HANG
                },
            );
        }
        p.take()
    })
    .with_event(0.02, "footstep", Some("foot_r"))
    .with_event(0.52, "footstep", Some("foot_l"))
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

fn reach(s: &Skeleton) -> AnimationClip {
    let effort = [
        (0.0, 0.0),
        (0.22, 0.25),
        (0.5, 0.45),
        (0.78, 0.4),
        (1.0, 0.0),
    ];
    AnimationClip::from_fn("reach", 2.6, 53, false, |ph| {
        let mut p = PoseBuilder::new(s);
        let k = |keys: &[(f32, f32)]| keyed(ph, keys);
        p.rot_move(
            "hips",
            Vec3::new(
                0.0,
                -0.01,
                k(&[(0.0, 0.0), (0.5, 0.04), (0.78, 0.04), (1.0, 0.0)]),
            ),
            k(&[(0.0, 0.0), (0.5, 6.0), (0.78, 6.0), (1.0, 0.0)]),
            0.0,
            0.0,
        );
        spine(
            &mut p,
            [
                k(&[(0.0, 0.0), (0.5, -8.0), (0.78, -8.0), (1.0, 0.0)]),
                k(&[
                    (0.0, 0.0),
                    (0.22, 2.0),
                    (0.5, -16.0),
                    (0.78, -16.0),
                    (1.0, 0.0),
                ]),
                k(&[(0.0, 0.0), (0.5, 9.0), (0.78, 9.0), (1.0, 0.0)]),
                k(&[(0.0, 0.0), (0.5, 8.0), (0.78, 8.0), (1.0, 0.0)]),
            ],
            [0.0; 4],
            [0.0; 4],
        );
        for side in 0..2 {
            arm(
                &mut p,
                side,
                Arm {
                    flex: k(&[
                        (0.0, 4.0),
                        (0.22, 45.0),
                        (0.32, 45.0),
                        (0.5, 92.0),
                        (0.78, 92.0),
                        (1.0, 4.0),
                    ]),
                    twist: k(&[(0.0, 0.0), (0.22, 25.0), (0.5, 0.0)]),
                    abduct: k(&[
                        (0.0, -15.0),
                        (0.22, -20.0),
                        (0.5, -4.0),
                        (0.78, -4.0),
                        (1.0, -15.0),
                    ]),
                    elbow: k(&[
                        (0.0, 12.0),
                        (0.22, 135.0),
                        (0.32, 135.0),
                        (0.5, 4.0),
                        (0.78, 4.0),
                        (1.0, 12.0),
                    ]),
                    wrist: k(&[
                        (0.0, 0.0),
                        (0.22, -10.0),
                        (0.5, 10.0),
                        (0.78, 10.0),
                        (1.0, 0.0),
                    ]),
                    protract: k(&[
                        (0.0, 0.0),
                        (0.22, -5.0),
                        (0.5, 24.0),
                        (0.78, 24.0),
                        (1.0, 0.0),
                    ]),
                    elevate: k(&[(0.0, 0.0), (0.5, 4.0), (1.0, 0.0)]),
                },
            );
            leg(&mut p, side, 2.0, 1.5, 3.0, 1.0, 0.0);
        }
        p.take()
    })
    .with_curve("effort", 53, |ph| keyed(ph, &effort))
}

fn flex(s: &Skeleton) -> AnimationClip {
    let flex = [(0.0, 0.0), (0.2, 1.0), (0.8, 1.0), (1.0, 0.0)];
    AnimationClip::from_fn("flex", 3.0, 31, false, |ph| {
        let mut p = PoseBuilder::new(s);
        let w = keyed(ph, &flex);
        let shake = (ph * 9.0 * std::f32::consts::TAU).sin() * w;
        p.rot_move("hips", Vec3::new(0.0, -0.01, 0.0), 0.0, 0.0, 0.0);
        spine(
            &mut p,
            [0.0, 4.0 * w, -2.0 * w, 2.0 * w],
            [0.0; 4],
            [0.0; 4],
        );
        for side in 0..2 {
            arm(
                &mut p,
                side,
                Arm {
                    flex: 4.0 * (1.0 - w) + 5.0 * w,
                    twist: -90.0 * w,
                    abduct: -15.0 + 78.0 * w,
                    elbow: 12.0 + 108.0 * w + 0.6 * shake,
                    wrist: 20.0 * w,
                    protract: -6.0 * w,
                    elevate: 8.0 * w,
                },
            );
            leg(&mut p, side, 3.0, 4.0 * w + 1.5, 6.0, 3.0, 0.0);
        }
        p.take()
    })
    .with_curve("flex", 31, |ph| keyed(ph, &flex))
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
        arm: [
            Arm {
                flex: 4.0,
                ..Arm::HANG
            },
            Arm {
                flex: 4.0,
                ..Arm::HANG
            },
        ],
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
    let keys = [
        lying,
        Key { at: 0.08, ..lying },
        sitting,
        tucked,
        stepping,
        kneeling,
        rising,
        Key::STAND,
    ];
    get_up(s, "get_up_back", 2.4, &keys, 0.7)
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
        lean(s, "lean_left", 1.0),
        lean(s, "lean_right", -1.0),
        heavy_swing(s),
        block(s),
        reach(s),
        flex(s),
        get_up_back(s),
        get_up_front(s),
        fall_brace(s),
    ]
}

/// Locomotion (idle/walk/run by `speed`, leaning by `turn`), plus `heavy_swing` (also on
/// the contract's `attack_heavy`), `block` (on its trigger or while `blocking` > 0.5, held
/// while it stays there), `reach` and `flex` actions, and getting up off the floor
/// (`get_up_back`, `get_up_front`: triggered by the ragdoll when a limp body comes round), and
/// `fall_brace` (bracing for a fall, on its trigger from anywhere).
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
        }
    }
}

/// The humanoid graph named `name`, moving with other locomotion clips (motion capture,
/// say); the actions stay the humanoid's. Walk and run clips must start on the right foot's
/// strike, like the humanoid's, so they blend in step.
pub fn graph_with(name: &str, motion: &Locomotion) -> AnimGraph {
    let locomotion = NodeDef::Blend1d {
        param: "speed".into(),
        children: [
            (0.0, &motion.idle),
            (motion.slow_walk_speed, &motion.slow_walk),
            (motion.walk_speed, &motion.walk),
            (motion.run_speed, &motion.run),
        ]
        .into_iter()
        .filter(|(_, clip)| !clip.is_empty())
        .map(|(at, clip)| Blend1dChild {
            at,
            node: NodeDef::clip(clip),
        })
        .collect(),
        sync: true,
        match_speed: true,
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
        weight: Weight::default(),
    };
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
                state("reach", NodeDef::one_shot("reach")),
                state("flex", NodeDef::one_shot("flex")),
                state("get_up_back", NodeDef::one_shot("get_up_back")),
                state("get_up_front", NodeDef::one_shot("get_up_front")),
                state("fall_brace", NodeDef::one_shot("fall_brace")),
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
                TransitionDef::new("move", "reach", Condition::trigger("reach"), 0.25),
                TransitionDef::new("reach", "move", Condition::Finished, 0.35).inertialize(),
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
