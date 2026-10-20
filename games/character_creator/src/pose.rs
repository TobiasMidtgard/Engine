//! Poses and expressions to show a person moving, without an animation module: a few
//! hand-made poses on the person's own skeleton (in the animation module's humanoid
//! conventions, so they drive the body exactly as its clips would), and expressions as ARKit
//! weights.

use engine::prelude::*;
use engine::world::animation::{BonePose, SkeletonPose};
use engine::world::character::FaceControls;
use engine_character::BuiltCharacter;

/// Which pose the subject holds (`None` component: the bust's rest).
#[derive(Component, Debug, Clone, PartialEq)]
pub struct PoseName(pub String);

/// Local turns of each bone (bone name, rotation in the bone's own frame), for pose `name` at
/// time `t` (s).
fn turns(name: &str, t: f32) -> Vec<(&'static str, Quat)> {
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

/// The pose `name` at time `t` on a body's rest skeleton (forward kinematics from the rest
/// pose with each bone's local turn).
pub fn pose(rest: &SkeletonPose, name: &str, t: f32) -> SkeletonPose {
    let turns = turns(name, t);
    let n = rest.bones.len();
    let mut out = rest.clone();
    for b in 0..n {
        let turn = turns
            .iter()
            .filter(|(bone, _)| rest.names[b] == *bone)
            .fold(Quat::IDENTITY, |q, (_, r)| q * *r);
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

/// Holds the subject in its named pose (moving with time in the window, still in shots).
pub fn drive(
    mut commands: Commands,
    time: Res<Time>,
    config: Res<EngineConfig>,
    q: Query<(
        Entity,
        &BuiltCharacter,
        Option<&PoseName>,
        Has<SkeletonPose>,
    )>,
) {
    let t = if config.headless {
        0.35
    } else {
        time.elapsed_secs()
    };
    for (entity, built, name, has_pose) in &q {
        match (name, built.meshes.rig.as_ref()) {
            (Some(name), Some(rig)) => {
                commands
                    .entity(entity)
                    .insert(pose(&rig.rest_pose(), &name.0, t));
            }
            _ if has_pose => {
                commands.entity(entity).remove::<SkeletonPose>();
            }
            _ => {}
        }
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
