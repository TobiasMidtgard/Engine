//! Poses and expressions to show a person moving, without an animation module: a few
//! hand-made poses on the person's own skeleton (in the animation module's humanoid
//! conventions, so they drive the body exactly as its clips would), and expressions as ARKit
//! weights.

use engine::prelude::*;
use engine::world::animation::{BonePose, SkeletonPose};
use engine::world::character::{FaceControls, HandControls, HandPose};
use engine_character::BuiltCharacter;

use crate::posing::{PoseEdit, skeleton, turns};

/// Which pose the subject holds (`None` component: the bust's rest).
#[derive(Component, Debug, Clone, PartialEq)]
pub struct PoseName(pub String);

/// The base poses, in the order the creator lists them.
pub const POSES: [&str; 8] = [
    "stand", "walk", "wave", "think", "turn", "guard", "reach", "tpose",
];

/// Local turns of each bone (bone name, rotation in the bone's own frame), for base pose
/// `name` at time `t` (s).
pub fn base_turns(name: &str, t: f32) -> Vec<(&'static str, Quat)> {
    let deg = |d: f32| d.to_radians();
    let x = |d: f32| Quat::from_rotation_x(deg(d));
    let y = |d: f32| Quat::from_rotation_y(deg(d));
    let z = |d: f32| Quat::from_rotation_z(deg(d));
    // Arms from the humanoid's 30° rest down to the sides (local Z swings the arm outward on
    // the right; the left mirrors it).
    let arms_down = |out: f32| {
        [
            ("upper_arm_l", z(30.0 - out) * x(4.0)),
            ("upper_arm_r", z(-(30.0 - out)) * x(4.0)),
            ("lower_arm_l", x(12.0)),
            ("lower_arm_r", x(12.0)),
        ]
    };
    let breathe = (t * 1.6).sin();
    let mut v: Vec<(&'static str, Quat)> = Vec::new();
    match name {
        "walk" => {
            let s = (t * 5.5).sin();
            let c = (t * 5.5).cos();
            v.extend([
                ("upper_leg_l", x(22.0 * s)),
                ("upper_leg_r", x(-22.0 * s)),
                ("lower_leg_l", x(-8.0 - 28.0 * (c.max(0.0)))),
                ("lower_leg_r", x(-8.0 - 28.0 * ((-c).max(0.0)))),
                ("foot_l", x(-6.0 * s)),
                ("foot_r", x(6.0 * s)),
                ("upper_arm_l", z(22.0) * x(-18.0 * s)),
                ("upper_arm_r", z(-22.0) * x(18.0 * s)),
                ("lower_arm_l", x(18.0)),
                ("lower_arm_r", x(18.0)),
                ("spine", y(-6.0 * s)),
                ("chest", y(9.0 * s) * x(3.0)),
                ("head", y(-3.0 * s)),
            ]);
        }
        "wave" => {
            v.extend([
                ("upper_arm_l", z(24.0) * x(4.0)),
                ("lower_arm_l", x(10.0)),
                // The right upper arm out to the side, turned on itself so the elbow bends
                // the forearm straight up, and the forearm swinging side to side.
                (
                    "upper_arm_r",
                    z(48.0) * x(-8.0) * y(-80.0 + 18.0 * (t * 6.0).sin()),
                ),
                ("lower_arm_r", x(100.0)),
                ("hand_r", x(-8.0)),
                ("chest", y(-6.0) * x(-2.0)),
                ("neck", z(4.0)),
                ("head", z(5.0) * y(8.0)),
                ("upper_leg_l", y(-6.0)),
                ("upper_leg_r", x(-4.0)),
            ]);
        }
        "think" => {
            v.extend([
                ("upper_arm_l", z(22.0) * x(28.0)),
                ("lower_arm_l", x(95.0)),
                ("upper_arm_r", z(18.0) * x(55.0) * y(-30.0)),
                ("lower_arm_r", x(130.0)),
                ("hand_r", x(-25.0)),
                ("chest", x(4.0 + 1.0 * breathe)),
                ("neck", x(10.0)),
                ("head", x(6.0) * z(-8.0) * y(-10.0)),
            ]);
        }
        // A fighter's guard: feet apart, knees soft, the right hand high and forward, the
        // left lower in front.
        "guard" => {
            v.extend([
                ("upper_leg_l", x(18.0) * z(-8.0)),
                ("lower_leg_l", x(-24.0)),
                ("foot_l", x(6.0)),
                ("upper_leg_r", x(-12.0) * z(10.0) * y(-20.0)),
                ("lower_leg_r", x(-14.0)),
                ("spine", x(-4.0) * y(-14.0)),
                ("chest", x(-4.0) * y(-10.0)),
                ("head", y(20.0) * x(2.0)),
                ("upper_arm_r", z(-6.0) * x(48.0) * y(30.0)),
                ("lower_arm_r", x(110.0)),
                ("upper_arm_l", z(-14.0) * x(40.0) * y(-20.0)),
                ("lower_arm_l", x(88.0)),
            ]);
        }
        // The right arm reaching forward at shoulder height, the eyes on the hand.
        "reach" => {
            v.extend([
                ("upper_arm_l", z(24.0) * x(4.0)),
                ("lower_arm_l", x(12.0)),
                ("upper_arm_r", z(-30.0) * x(88.0)),
                ("lower_arm_r", x(8.0)),
                ("chest", y(-8.0) * x(-3.0)),
                ("head", y(-10.0) * x(-6.0)),
            ]);
        }
        // Arms straight out to the sides, for checking proportions and skinning.
        "tpose" => {
            v.extend([("upper_arm_l", z(60.0)), ("upper_arm_r", z(-60.0))]);
        }
        "turn" => {
            v.extend(arms_down(8.0));
            v.extend([
                ("spine", y(18.0)),
                ("chest", y(22.0)),
                ("neck", y(16.0)),
                ("head", y(24.0) * x(-4.0)),
            ]);
        }
        _ => {
            v.extend(arms_down(6.0));
            v.extend([
                ("chest", x(1.5 * breathe)),
                ("head", x(-2.0)),
                ("upper_leg_l", z(-2.0)),
                ("upper_leg_r", z(2.0)),
            ]);
        }
    }
    v
}

/// Forward kinematics from a rest pose with each bone's local turn (in `rest` order).
pub fn forward(rest: &SkeletonPose, turns: &[Quat]) -> SkeletonPose {
    let n = rest.bones.len();
    let mut out = rest.clone();
    for b in 0..n {
        let turn = turns.get(b).copied().unwrap_or(Quat::IDENTITY);
        let r = &rest.bones[b];
        out.bones[b] = match rest.parents[b] {
            Some(p) => {
                let p = p as usize;
                let (rp, pp) = (&rest.bones[p], out.bones[p]);
                let local_rot = rp.rotation.inverse() * r.rotation;
                let local_at = rp.rotation.inverse() * (r.translation - rp.translation);
                BonePose {
                    translation: pp.translation + pp.rotation * local_at,
                    rotation: pp.rotation * local_rot * turn,
                    scale: Vec3::ONE,
                }
            }
            None => BonePose {
                rotation: r.rotation * turn,
                ..*r
            },
        };
    }
    out
}

/// Expressions by name, as ARKit weights (and a gaze).
pub const EXPRESSIONS: [&str; 9] = [
    "neutral", "smile", "laugh", "sad", "surprise", "angry", "disgust", "blink", "look",
];

pub fn expression(name: &str) -> FaceControls {
    let mut f = FaceControls::default();
    fn both(f: &mut FaceControls, shape: &str, w: f32) {
        f.set(&format!("{shape}Left"), w);
        f.set(&format!("{shape}Right"), w);
    }
    match name {
        "smile" => {
            both(&mut f, "mouthSmile", 0.8);
            both(&mut f, "cheekSquint", 0.5);
            both(&mut f, "eyeSquint", 0.15);
            both(&mut f, "mouthDimple", 0.25);
        }
        "laugh" => {
            both(&mut f, "mouthSmile", 1.0);
            both(&mut f, "cheekSquint", 0.8);
            both(&mut f, "eyeSquint", 0.45);
            both(&mut f, "mouthDimple", 0.4);
            both(&mut f, "mouthUpperUp", 0.3);
            f.set("jawOpen", 0.35);
            f.set("browInnerUp", 0.2);
        }
        "sad" => {
            both(&mut f, "mouthFrown", 0.75);
            both(&mut f, "mouthPress", 0.25);
            f.set("browInnerUp", 0.9);
            f.set("mouthShrugLower", 0.45);
            f.gaze_pitch = -0.15;
        }
        "surprise" => {
            f.set("browInnerUp", 1.0);
            both(&mut f, "browOuterUp", 1.0);
            both(&mut f, "eyeWide", 0.9);
            f.set("jawOpen", 0.45);
            f.set("mouthFunnel", 0.25);
        }
        "angry" => {
            both(&mut f, "browDown", 1.0);
            both(&mut f, "eyeSquint", 0.35);
            both(&mut f, "noseSneer", 0.45);
            both(&mut f, "mouthPress", 0.5);
            both(&mut f, "mouthStretch", 0.25);
            both(&mut f, "mouthFrown", 0.3);
        }
        "disgust" => {
            both(&mut f, "noseSneer", 0.9);
            both(&mut f, "mouthUpperUp", 0.6);
            both(&mut f, "browDown", 0.45);
            both(&mut f, "eyeSquint", 0.4);
            both(&mut f, "mouthFrown", 0.35);
            f.set("mouthShrugLower", 0.3);
        }
        "blink" => both(&mut f, "eyeBlink", 1.0),
        "look" => {
            f.gaze_yaw = 0.42;
            f.gaze_pitch = 0.1;
        }
        _ => {}
    }
    f
}

type PosedSubject<'a> = (
    Entity,
    &'a BuiltCharacter,
    Option<&'a PoseName>,
    Option<&'a PoseEdit>,
    Option<&'a HandControls>,
    Has<SkeletonPose>,
);

/// Holds the subject in its pose: the design tool's [`PoseEdit`] (with its hands, or the
/// subject's `HandControls` when something else sets them), else a named base pose (moving with
/// time in the window, still in shots).
pub fn drive(
    mut commands: Commands,
    time: Res<Time>,
    config: Res<EngineConfig>,
    q: Query<PosedSubject>,
) {
    let t = if config.headless {
        0.35
    } else {
        time.elapsed_secs()
    };
    for (entity, built, name, edit, hand_controls, has_pose) in &q {
        let Some(rig) = built.meshes.rig.as_ref() else {
            if has_pose {
                commands.entity(entity).remove::<SkeletonPose>();
            }
            continue;
        };
        let hands = hand_controls.map(|h| h.hands);
        let edit = match (edit, name) {
            (Some(e), _) => e.clone(),
            (None, Some(n)) => {
                let mut e = PoseEdit::base(&n.0);
                e.moving = true;
                e
            }
            (None, None) => {
                if has_pose {
                    commands.entity(entity).remove::<SkeletonPose>();
                }
                continue;
            }
        };
        // `HandControls::grip` alone (the shots' `--grip`): close both hands on a hilt.
        let hands = hands.map(|mut h| {
            for (side, hand) in h.iter_mut().enumerate() {
                if *hand == HandPose::default()
                    && let Some(c) = hand_controls
                {
                    *hand = HandPose {
                        shape: engine::world::character::HandShape::PowerGrip,
                        weight: c.grip(side),
                        ..Default::default()
                    };
                }
            }
            h
        });
        let turns = turns(rig, &edit, hands, t);
        commands.entity(entity).insert(skeleton(rig, &turns));
    }
}

/// Where the subject's eyes are in its model space, posed.
pub fn eye_point(built: &BuiltCharacter, pose: Option<&SkeletonPose>) -> Option<Vec3> {
    let lm = built.meshes.landmarks?;
    let eyes = (lm.eyes[0] + lm.eyes[1]) * 0.5;
    let (Some(rig), Some(pose)) = (built.meshes.rig.as_ref(), pose) else {
        return Some(eyes);
    };
    let mut palette = Vec::new();
    rig.palette(pose, &rig.map(pose), &mut palette);
    let head = rig.bones.iter().position(|b| b == "head")?;
    Some(palette[head].transform_point3(eyes))
}

/// Where a bone's joint is in the subject's model space, posed.
pub fn bone_point(built: &BuiltCharacter, pose: Option<&SkeletonPose>, bone: &str) -> Option<Vec3> {
    let rig = built.meshes.rig.as_ref()?;
    let b = rig.bone(bone)?;
    let rest = engine_character::rig::to_humanoid(rig.joints[b]);
    let Some(pose) = pose else {
        return Some(rest);
    };
    let mut palette = Vec::new();
    rig.palette(pose, &rig.map(pose), &mut palette);
    Some(palette[b].transform_point3(rest))
}
