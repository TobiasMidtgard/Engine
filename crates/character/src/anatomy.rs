//! Each joint's movements by their anatomical names, with normal ranges of motion, and how a
//! set of angles turns the bone: what a posing tool shows as sliders and a pose checker holds
//! poses to.
//!
//! Every joint has up to three movements: a bend (flexion), a side movement (abduction,
//! side bend, a finger's spread) and a twist about the bone. Angles are degrees; a positive
//! value is the movement named first (`Raise forward · back`: positive raises forward). Left
//! and right use the same values for mirror images, so a pose mirrors by swapping sides.
//!
//! Axes: the body's bones turn about the humanoid's own axes (facing −Z, +X to the person's
//! right; arms in their 30° rest frame), hands and fingers about axes measured on each
//! person's hand ([`Rig::joint_axes`]). Ranges: normal active range of motion
//! (research-ik-hands-skinning.md 2.3 for hands; the AAOS shoulder, hip and spine norms for
//! the rest), a little wider where actors and fighters go.

use engine_core::math::{Quat, Vec3};

use crate::hands::ROM;
use crate::rig::{BoneKind, Rig};

/// One movement of a joint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Movement {
    /// "Bend forward · back": positive first.
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
}

const fn m(name: &'static str, min: f32, max: f32) -> Option<Movement> {
    Some(Movement { name, min, max })
}

/// A joint's movements: bend, side, twist (`None` where it has none).
pub type Movements = [Option<Movement>; 3];

/// A bone's name without its side, and the side (0 left, 1 right; 1 for the trunk).
fn base(bone: &str) -> (&str, usize) {
    if let Some(b) = bone.strip_suffix("_l") {
        (b, 0)
    } else if let Some(b) = bone.strip_suffix("_r") {
        (b, 1)
    } else {
        (bone, 1)
    }
}

/// The movements of a bone, `None` for bones nobody poses (root, twist bones).
pub fn movements(bone: &str) -> Option<Movements> {
    let (b, _) = base(bone);
    Some(match b {
        "hips" => [
            m("Tilt forward · back", -30.0, 30.0),
            m("Drop right · left", -20.0, 20.0),
            m("Turn left · right", -45.0, 45.0),
        ],
        "spine" => [
            m("Bend forward · back", -40.0, 45.0),
            m("Lean right · left", -30.0, 30.0),
            m("Turn left · right", -35.0, 35.0),
        ],
        "chest" => [
            m("Bend forward · back", -30.0, 30.0),
            m("Lean right · left", -25.0, 25.0),
            m("Turn left · right", -35.0, 35.0),
        ],
        "neck" => [
            m("Nod down · up", -45.0, 50.0),
            m("Tilt right · left", -35.0, 35.0),
            m("Turn left · right", -50.0, 50.0),
        ],
        "head" => [
            m("Nod down · up", -40.0, 35.0),
            m("Tilt right · left", -30.0, 30.0),
            m("Turn left · right", -50.0, 50.0),
        ],
        "clavicle" => [
            m("Shrug up · down", -10.0, 40.0),
            m("Shoulder forward · back", -25.0, 30.0),
            None,
        ],
        "upper_arm" => [
            m("Raise forward · back", -60.0, 180.0),
            m("Raise out · in", -45.0, 150.0),
            m("Turn out · in", -90.0, 90.0),
        ],
        "lower_arm" => [m("Bend elbow", 0.0, 150.0), None, None],
        "hand" => [
            m("Bend palm · back", -75.0, 85.0),
            m("Toward thumb · little finger", -40.0, 25.0),
            m("Turn palm back · forward", -90.0, 90.0),
        ],
        "upper_leg" => [
            m("Raise forward · back", -30.0, 125.0),
            m("Out · in", -30.0, 50.0),
            m("Turn out · in", -45.0, 45.0),
        ],
        "lower_leg" => [m("Bend knee", 0.0, 150.0), None, None],
        "foot" => [
            m("Toes up · down", -50.0, 30.0),
            m("Turn out · in", -30.0, 30.0),
            m("Sole in · out", -25.0, 35.0),
        ],
        "toe" => [m("Toes up · down", -40.0, 70.0), None, None],
        _ => {
            let (f, k) = crate::skin::finger_of(bone)?;
            let (bends, spread, roll) = ROM[f];
            let bend_name = match (f, k) {
                (0, 0) => "Across the palm",
                (0, _) | (_, 0) => "Bend knuckle",
                (_, 1) => "Bend middle joint",
                _ => "Bend end joint",
            };
            let bend = m(bend_name, bends[k].0, bends[k].1);
            if k == 0 {
                let side = if f == 0 { "Out of the palm" } else { "Spread" };
                let twist = if f == 0 {
                    "Turn to face the fingers"
                } else {
                    "Turn"
                };
                [bend, m(side, spread.0, spread.1), m(twist, roll.0, roll.1)]
            } else {
                [bend, None, None]
            }
        }
    })
}

/// A body bone's axes in its pose frame, for its bend, side and twist movements (positive as
/// [`movements`] names them).
fn body_axes(bone: &str) -> Option<[Vec3; 3]> {
    let (b, side) = base(bone);
    // Mirror images: the side movement and the twist turn the other way on the left.
    let s = if side == 0 { -1.0 } else { 1.0 };
    Some(match b {
        // Forward is −Z: a positive turn about +X tips +Y backward.
        "hips" | "spine" | "chest" | "neck" | "head" => [-Vec3::X, -Vec3::Z, Vec3::Y],
        "clavicle" => [Vec3::Z * s, Vec3::Y * s, Vec3::ZERO],
        "upper_arm" => [Vec3::X, Vec3::Z * s, -Vec3::Y * s],
        "lower_arm" => [Vec3::X, Vec3::ZERO, Vec3::ZERO],
        "upper_leg" => [Vec3::X, Vec3::Z * s, -Vec3::Y * s],
        "lower_leg" => [-Vec3::X, Vec3::ZERO, Vec3::ZERO],
        "foot" => [Vec3::X, -Vec3::Y * s, -Vec3::Z * s],
        "toe" => [Vec3::X, Vec3::ZERO, Vec3::ZERO],
        _ => return None,
    })
}

/// Angles clamped to a bone's ranges.
pub fn clamp(bone: &str, angles: [f32; 3]) -> [f32; 3] {
    let Some(ms) = movements(bone) else {
        return [0.0; 3];
    };
    let mut out = [0.0; 3];
    for k in 0..3 {
        if let Some(mv) = ms[k] {
            out[k] = angles[k].clamp(mv.min, mv.max);
        }
    }
    out
}

/// The local turn a pose gives bone `b` of `rig` for these angles (degrees; bend, side,
/// twist). Fingers count their bend from straight (the hand pose library's convention), the
/// rest from the humanoid's rest pose.
pub fn turn(rig: &Rig, b: usize, angles: [f32; 3]) -> Quat {
    let name = &rig.bones[b];
    let a = clamp(name, angles).map(f32::to_radians);
    match rig.kind[b] {
        BoneKind::Finger(..) => rig.pose_turn(b, rig.finger_turn(b, a[0], a[1], a[2])),
        BoneKind::Twist(..) => Quat::IDENTITY,
        BoneKind::Body if name.starts_with("hand_") => {
            let [f, s, t] = rig.joint_axes(b);
            if f == Vec3::ZERO {
                return Quat::IDENTITY;
            }
            let q = Quat::from_axis_angle(f, a[0])
                * Quat::from_axis_angle(s, a[1])
                * Quat::from_axis_angle(t, a[2]);
            rig.pose_turn(b, q)
        }
        BoneKind::Body => {
            let Some(axes) = body_axes(name) else {
                return Quat::IDENTITY;
            };
            let r = |k: usize| {
                if axes[k] == Vec3::ZERO {
                    Quat::IDENTITY
                } else {
                    Quat::from_axis_angle(axes[k], a[k])
                }
            };
            r(0) * r(1) * r(2)
        }
    }
}

/// The same bone on the other side (`hand_l` ↔ `hand_r`); the trunk's own name.
pub fn mirror(bone: &str) -> String {
    if let Some(b) = bone.strip_suffix("_l") {
        format!("{b}_r")
    } else if let Some(b) = bone.strip_suffix("_r") {
        format!("{b}_l")
    } else {
        bone.to_string()
    }
}

/// The bones a posing tool shows, by group: trunk and head, then each arm, hand and leg.
pub const GROUPS: [(&str, &[&str]); 7] = [
    (
        "Trunk and head",
        &["hips", "spine", "chest", "neck", "head"],
    ),
    (
        "Left arm",
        &["clavicle_l", "upper_arm_l", "lower_arm_l", "hand_l"],
    ),
    (
        "Right arm",
        &["clavicle_r", "upper_arm_r", "lower_arm_r", "hand_r"],
    ),
    (
        "Left leg",
        &["upper_leg_l", "lower_leg_l", "foot_l", "toe_l"],
    ),
    (
        "Right leg",
        &["upper_leg_r", "lower_leg_r", "foot_r", "toe_r"],
    ),
    ("Left hand", &[]),
    ("Right hand", &[]),
];

/// A bone's name for people (`upper_arm_l` → "Left upper arm", `index_02_r` → "Right index,
/// middle joint").
pub fn label(bone: &str) -> String {
    let (b, side) = base(bone);
    let sided = bone.ends_with("_l") || bone.ends_with("_r");
    let prefix = if !sided {
        ""
    } else if side == 0 {
        "Left "
    } else {
        "Right "
    };
    if let Some((f, k)) = crate::skin::finger_of(bone) {
        let joint = ["knuckle", "middle joint", "end joint"][k];
        let joint = if f == 0 {
            ["base", "knuckle", "end joint"][k]
        } else {
            joint
        };
        return format!("{prefix}{}, {joint}", crate::skin::FINGERS[f]);
    }
    let name = match b {
        "upper_arm" => "upper arm",
        "lower_arm" => "forearm (elbow)",
        "upper_leg" => "thigh (hip)",
        "lower_leg" => "shin (knee)",
        "foot" => "foot (ankle)",
        "toe" => "toes",
        "clavicle" => "shoulder",
        "hips" => "hips",
        other => other,
    };
    let mut out = format!("{prefix}{name}");
    if let Some(c) = out.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_posed_humanoid_bone_has_movements_and_a_label() {
        for (bone, _) in engine_world::animation::HUMANOID_BONES {
            let posed = !bone.starts_with("ponytail") && !bone.contains("twist") && bone != "root";
            assert_eq!(movements(bone).is_some(), posed, "{bone}");
            if posed {
                assert!(
                    body_axes(bone).is_some()
                        || crate::skin::finger_of(bone).is_some()
                        || bone.starts_with("hand_"),
                    "{bone}"
                );
                assert!(!label(bone).is_empty());
            }
        }
        assert_eq!(label("upper_arm_l"), "Left upper arm");
        assert_eq!(label("index_02_r"), "Right index, middle joint");
        assert_eq!(mirror("foot_l"), "foot_r");
        assert_eq!(mirror("neck"), "neck");
    }

    #[test]
    fn angles_clamp_to_the_range_of_motion() {
        assert_eq!(clamp("lower_arm_l", [200.0, 30.0, 10.0]), [150.0, 0.0, 0.0]);
        assert_eq!(clamp("index_01_r", [120.0, -40.0, 0.0])[..2], [95.0, -20.0]);
    }

    #[test]
    fn a_raised_arm_points_forward_and_a_bent_knee_takes_the_shin_back() {
        // Right arm hanging down in its rest frame (local −Y): raised forward 90°, it points
        // forward (−Z) in the same frame.
        let axes = body_axes("upper_arm_r").unwrap();
        let q = Quat::from_axis_angle(axes[0], 90f32.to_radians());
        assert!((q * Vec3::NEG_Y - Vec3::NEG_Z).length() < 1e-4);
        // Out to the side: the right arm moves toward +X, the left toward −X.
        let out_r = Quat::from_axis_angle(body_axes("upper_arm_r").unwrap()[1], 0.5) * Vec3::NEG_Y;
        let out_l = Quat::from_axis_angle(body_axes("upper_arm_l").unwrap()[1], 0.5) * Vec3::NEG_Y;
        assert!(out_r.x > 0.4 && out_l.x < -0.4);
        let knee = Quat::from_axis_angle(body_axes("lower_leg_l").unwrap()[0], 1.0) * Vec3::NEG_Y;
        assert!(knee.z > 0.8, "{knee}");
        // Bending forward tips the spine toward −Z.
        let spine = Quat::from_axis_angle(body_axes("spine").unwrap()[0], 0.5) * Vec3::Y;
        assert!(spine.z < -0.4);
        // Toes up.
        let toe = Quat::from_axis_angle(body_axes("foot_r").unwrap()[0], 0.5) * Vec3::NEG_Z;
        assert!(toe.y > 0.4);
    }
}
