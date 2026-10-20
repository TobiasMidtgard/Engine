//! Posing a character with the animation module's humanoid skeleton.
//!
//! The pack folds MakeHuman's skin weights onto the humanoid bones (`root`, `hips`, `spine`,
//! `chest`, `neck`, `head`, and per side `clavicle`, `upper_arm`, `lower_arm`, `hand`,
//! `upper_leg`, `lower_leg`, `foot`, `toe`) and says where each joint sits on the base mesh, so
//! the joints follow the morphs. A [`Rig`] holds one character's joints; [`Rig::palette`]
//! turns a `SkeletonPose` into one matrix per bone for the shaders.
//!
//! Conventions bridged here:
//! * The base mesh faces +Z with +X on the person's left; the humanoid faces −Z with +X on
//!   the person's right. The palette turns the body half a turn about Y, so a posed character
//!   lives in the animation module's model space.
//! * The humanoid's rest pose has straight arms hanging 30° out from the body and straight
//!   legs; the base mesh's arms hang wider with the elbows bent. Each limb bone is turned onto
//!   the humanoid's rest direction first, so a pose means the same on both.
//! * Bone positions are rebuilt down the character's own skeleton from the pose's rotations
//!   (a pose written for a taller skeleton still fits), then the body is moved so its feet land
//!   where the pose puts them.

use engine_core::math::{Mat3, Mat4, Quat, Vec3};
use engine_world::animation::SkeletonPose;

use crate::pack::HumanPack;

/// Each humanoid bone the pack weights and its parent.
const PARENTS: [(&str, Option<&str>); 22] = [
    ("root", None),
    ("hips", Some("root")),
    ("spine", Some("hips")),
    ("chest", Some("spine")),
    ("neck", Some("chest")),
    ("head", Some("neck")),
    ("clavicle_l", Some("chest")),
    ("upper_arm_l", Some("clavicle_l")),
    ("lower_arm_l", Some("upper_arm_l")),
    ("hand_l", Some("lower_arm_l")),
    ("clavicle_r", Some("chest")),
    ("upper_arm_r", Some("clavicle_r")),
    ("lower_arm_r", Some("upper_arm_r")),
    ("hand_r", Some("lower_arm_r")),
    ("upper_leg_l", Some("hips")),
    ("lower_leg_l", Some("upper_leg_l")),
    ("foot_l", Some("lower_leg_l")),
    ("toe_l", Some("foot_l")),
    ("upper_leg_r", Some("hips")),
    ("lower_leg_r", Some("upper_leg_r")),
    ("foot_r", Some("lower_leg_r")),
    ("toe_r", Some("foot_r")),
];

/// The humanoid's rest arm angle out from the body, radians (engine_anim's `humanoid`).
const REST_ARM_ANGLE: f32 = 30.0 * std::f32::consts::PI / 180.0;

/// The humanoid's rest offset of the toe from the ankle (forward and down).
const REST_TOE: Vec3 = Vec3::new(0.0, -0.05, -0.14);

/// Base mesh space (facing +Z, +X left) to the humanoid's (facing −Z, +X right).
pub fn to_humanoid(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, -p.z)
}

/// One character's skeleton, in the humanoid's model space.
#[derive(Debug, Clone, PartialEq)]
pub struct Rig {
    /// Bone names, in the pack's order (the order of the skin's bone indices).
    pub bones: Vec<String>,
    pub parents: Vec<Option<usize>>,
    /// Where each joint sits at rest, humanoid model space (m).
    pub joints: Vec<Vec3>,
    /// The humanoid's rest rotation of each bone (model space).
    pub rest: Vec<Quat>,
    /// Turns each bone of the character from its own rest direction onto the humanoid's.
    pub correction: Vec<Quat>,
}

impl Rig {
    /// The rig of a morphed body (`morphed`: the pack's vertices after the morphs). `None` when
    /// the pack has no rig.
    pub fn new(pack: &HumanPack, morphed: &[Vec3]) -> Option<Self> {
        if !pack.has_rig() {
            return None;
        }
        let bones = pack.bones.clone();
        let index = |name: &str| bones.iter().position(|b| b == name);
        let parents: Vec<Option<usize>> = bones
            .iter()
            .map(|b| {
                PARENTS
                    .iter()
                    .find(|(n, _)| n == b)
                    .and_then(|(_, p)| p.and_then(index))
            })
            .collect();
        let mut joints: Vec<Vec3> = pack
            .joints
            .iter()
            .map(|verts| {
                let sum: Vec3 = verts.iter().map(|v| morphed[*v as usize]).sum();
                to_humanoid(sum / verts.len().max(1) as f32)
            })
            .collect();
        joints.resize(bones.len(), Vec3::ZERO);
        // The root stands on the ground under the hips.
        if let (Some(root), Some(hips)) = (index("root"), index("hips")) {
            let ground = joints[root].y;
            joints[root] = Vec3::new(joints[hips].x, ground, joints[hips].z);
        }
        let side = |b: &str| {
            if b.ends_with("_l") { -1.0 } else { 1.0 }
        };
        let rest: Vec<Quat> = bones
            .iter()
            .map(|b| {
                if b.starts_with("upper_arm") || b.starts_with("lower_arm") || b.starts_with("hand")
                {
                    Quat::from_rotation_z(REST_ARM_ANGLE * side(b))
                } else {
                    Quat::IDENTITY
                }
            })
            .collect();
        // Limb bones: from the character's direction to the child joint onto the humanoid's.
        let mut correction = vec![Quat::IDENTITY; bones.len()];
        let limbs = [
            ("upper_arm", "lower_arm"),
            ("lower_arm", "hand"),
            ("upper_leg", "lower_leg"),
            ("lower_leg", "foot"),
            ("foot", "toe"),
        ];
        for s in ["l", "r"] {
            for (bone, child) in limbs {
                let (Some(b), Some(c)) = (
                    index(&format!("{bone}_{s}")),
                    index(&format!("{child}_{s}")),
                ) else {
                    continue;
                };
                let own = (joints[c] - joints[b]).normalize_or_zero();
                let want = if bone == "foot" {
                    REST_TOE.normalize()
                } else {
                    rest[b] * Vec3::NEG_Y
                };
                if own != Vec3::ZERO {
                    correction[b] = Quat::from_rotation_arc(own, want);
                }
            }
            // Hands and toes keep their place on the limb they end.
            for (end, limb) in [("hand", "lower_arm"), ("toe", "foot")] {
                if let (Some(e), Some(l)) =
                    (index(&format!("{end}_{s}")), index(&format!("{limb}_{s}")))
                {
                    correction[e] = correction[l];
                }
            }
        }
        Some(Self {
            bones,
            parents,
            joints,
            rest,
            correction,
        })
    }

    /// Where each of this rig's bones is in `pose` (`None` for bones the pose lacks).
    pub fn map(&self, pose: &SkeletonPose) -> Vec<Option<usize>> {
        self.bones.iter().map(|b| pose.index_of(b)).collect()
    }

    /// One matrix per bone taking a base-mesh-space rest point to the humanoid's model space
    /// in `pose` (`map` from [`Self::map`]). Bones the pose lacks follow their parents.
    pub fn palette(&self, pose: &SkeletonPose, map: &[Option<usize>], out: &mut Vec<Mat4>) {
        let n = self.bones.len();
        out.clear();
        let mut turn = vec![Quat::IDENTITY; n];
        let mut at = vec![Vec3::ZERO; n];
        let mut posed = vec![false; n];
        for b in 0..n {
            let bone = map
                .get(b)
                .copied()
                .flatten()
                .and_then(|i| pose.bones.get(i));
            let parent = self.parents[b];
            turn[b] = match (bone, parent) {
                (Some(p), _) => {
                    posed[b] = true;
                    p.rotation * self.rest[b].inverse() * self.correction[b]
                }
                (None, Some(p)) => turn[p],
                (None, None) => Quat::IDENTITY,
            };
            at[b] = match parent {
                Some(p) => at[p] + turn[p] * (self.joints[b] - self.joints[p]),
                None => bone.map_or(self.joints[b], |p| p.translation),
            };
            // The hips are where the pose puts them; the root just carries them.
            if parent.is_some_and(|p| self.parents[p].is_none())
                && let Some(p) = bone
            {
                at[b] = p.translation;
            }
        }
        // Land the feet where the pose puts its feet (the pose's legs may be longer).
        let mut shift = Vec3::ZERO;
        let mut feet = 0.0;
        for (b, name) in self.bones.iter().enumerate() {
            if !name.starts_with("foot_") || !posed[b] {
                continue;
            }
            if let Some(p) = map[b].and_then(|i| pose.bones.get(i)) {
                shift += p.translation - at[b];
                feet += 1.0;
            }
        }
        if feet > 0.0 {
            shift /= feet;
        }
        // Hands go where the pose puts its hands (a sword grip, a rifle's), whatever this
        // person's arm length: two-bone IK from the shoulder, the elbow bending the way the
        // pose bends it, as far as the arm reaches at the pose's own bend.
        for s in ["l", "r"] {
            let index = |name: String| self.bones.iter().position(|b| *b == name);
            let (Some(u), Some(f), Some(h)) = (
                index(format!("upper_arm_{s}")),
                index(format!("lower_arm_{s}")),
                index(format!("hand_{s}")),
            ) else {
                continue;
            };
            let Some(target) = map[h]
                .and_then(|i| pose.bones.get(i))
                .map(|p| p.translation)
            else {
                continue;
            };
            let target = target - shift;
            let (shoulder, elbow, wrist) = (at[u], at[f], at[h]);
            let upper = (self.joints[f] - self.joints[u]).length();
            let lower = (self.joints[h] - self.joints[f]).length();
            let to = target - shoulder;
            let reach = to.length();
            if upper < 1e-4 || lower < 1e-4 || reach < 1e-4 {
                continue;
            }
            let axis = to / reach;
            // A hand out of reach points toward its target with the pose's own elbow bend
            // rather than straightening the arm (a wave stays a wave on a short person).
            let bent = (wrist - shoulder).length().max(1e-4);
            let d = reach
                .min(bent)
                .clamp((upper - lower).abs() + 1e-5, upper + lower - 1e-5);
            let along = (upper * upper - lower * lower + d * d) / (2.0 * d);
            let out = (upper * upper - along * along).max(0.0).sqrt();
            // The bend's direction: where the elbow already points off the shoulder-hand line
            // (or, with the arm straight, back).
            let perp = |v: Vec3| v - axis * v.dot(axis);
            let mut pole = perp(elbow - shoulder);
            if pole.length() < 1e-3 * upper {
                // Elbows point back (the humanoid faces −Z), or down for an arm along Z.
                pole = perp(Vec3::Z);
                if pole.length() < 0.1 {
                    pole = perp(Vec3::NEG_Y);
                }
            }
            let pole = perp(pole.normalize_or(Vec3::Z)).normalize_or(Vec3::Z);
            let new_elbow = shoulder + axis * along + pole * out;
            let new_wrist = shoulder + axis * d;
            let swing = |from: Vec3, to: Vec3| {
                Quat::from_rotation_arc(from.normalize_or(Vec3::Y), to.normalize_or(Vec3::Y))
            };
            turn[u] = swing(elbow - shoulder, new_elbow - shoulder) * turn[u];
            turn[f] = swing(wrist - elbow, new_wrist - new_elbow) * turn[f];
            at[f] = new_elbow;
            at[h] = new_wrist;
        }
        let flip = Mat4::from_mat3(Mat3::from_diagonal(Vec3::new(-1.0, 1.0, -1.0)));
        for b in 0..n {
            let m = Mat4::from_translation(at[b] + shift)
                * Mat4::from_quat(turn[b])
                * Mat4::from_translation(-self.joints[b])
                * flip;
            out.push(m);
        }
    }

    /// The rig's own rest pose in the humanoid's conventions: what a skeleton fitted to this
    /// body (engine_anim's `fit_skeleton`) holds at rest, standing on the origin. Bones in
    /// `names` order.
    pub fn rest_pose(&self) -> SkeletonPose {
        use engine_world::animation::BonePose;
        let mut bones = Vec::with_capacity(self.bones.len());
        for b in 0..self.bones.len() {
            // A limb hangs along the humanoid's rest direction with the character's length.
            let at = match self.parents[b] {
                Some(p) if self.correction[p] != Quat::IDENTITY => {
                    let pp: &BonePose = &bones[p];
                    pp.translation + self.correction[p] * (self.joints[b] - self.joints[p])
                }
                Some(p) => {
                    let pp: &BonePose = &bones[p];
                    pp.translation + (self.joints[b] - self.joints[p])
                }
                None => Vec3::ZERO,
            };
            bones.push(BonePose {
                translation: at,
                rotation: self.rest[b],
                scale: Vec3::ONE,
            });
        }
        SkeletonPose {
            names: self.bones.clone().into(),
            parents: self
                .parents
                .iter()
                .map(|p| p.map(|i| i as u16))
                .collect::<Vec<_>>()
                .into(),
            bones,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{HumanPack, SkinWeights};
    use engine_core::math::Vec2;

    /// A pack with one vertex per joint, laid out like the base mesh (facing +Z, +X left):
    /// arms hanging 45° out with the elbows bent forward.
    pub(crate) fn stick_pack() -> HumanPack {
        let joints: Vec<(&str, Vec3)> = vec![
            ("root", Vec3::new(0.0, -0.81, 0.0)),
            ("hips", Vec3::new(0.0, 0.07, 0.0)),
            ("spine", Vec3::new(0.0, 0.19, 0.0)),
            ("chest", Vec3::new(0.0, 0.28, 0.0)),
            ("neck", Vec3::new(0.0, 0.59, 0.0)),
            ("head", Vec3::new(0.0, 0.70, 0.0)),
            ("clavicle_l", Vec3::new(0.03, 0.52, 0.05)),
            ("upper_arm_l", Vec3::new(0.17, 0.52, 0.0)),
            ("lower_arm_l", Vec3::new(0.37, 0.32, 0.0)),
            ("hand_l", Vec3::new(0.47, 0.22, 0.12)),
            ("clavicle_r", Vec3::new(-0.03, 0.52, 0.05)),
            ("upper_arm_r", Vec3::new(-0.17, 0.52, 0.0)),
            ("lower_arm_r", Vec3::new(-0.37, 0.32, 0.0)),
            ("hand_r", Vec3::new(-0.47, 0.22, 0.12)),
            ("upper_leg_l", Vec3::new(0.11, 0.05, 0.0)),
            ("lower_leg_l", Vec3::new(0.15, -0.37, 0.0)),
            ("foot_l", Vec3::new(0.18, -0.75, 0.0)),
            ("toe_l", Vec3::new(0.18, -0.80, 0.13)),
            ("upper_leg_r", Vec3::new(-0.11, 0.05, 0.0)),
            ("lower_leg_r", Vec3::new(-0.15, -0.37, 0.0)),
            ("foot_r", Vec3::new(-0.18, -0.75, 0.0)),
            ("toe_r", Vec3::new(-0.18, -0.80, 0.13)),
        ];
        let positions: Vec<Vec3> = joints.iter().map(|j| j.1).collect();
        let mut pack = HumanPack::new(positions, vec![Vec2::ZERO], vec![], vec![], vec![]);
        pack.bones = joints.iter().map(|j| j.0.to_string()).collect();
        pack.skin = (0..joints.len())
            .map(|i| SkinWeights {
                bones: [i as u8, 0, 0, 0],
                weights: [255, 0, 0, 0],
            })
            .collect();
        pack.joints = (0..joints.len() as u32).map(|i| vec![i]).collect();
        pack
    }

    fn apply(m: &Mat4, p: Vec3) -> Vec3 {
        m.transform_point3(p)
    }

    #[test]
    fn the_rest_pose_straightens_limbs_onto_the_humanoid() {
        let pack = stick_pack();
        let rig = Rig::new(&pack, &pack.positions).unwrap();
        let pose = rig.rest_pose();
        let map = rig.map(&pose);
        let mut palette = Vec::new();
        rig.palette(&pose, &map, &mut palette);
        let at = |name: &str| {
            let b = pack.bone(name).unwrap();
            apply(&palette[b], pack.positions[b])
        };
        // Facing −Z now: the person's left hand is on −X.
        assert!(at("hand_l").x < -0.3, "{}", at("hand_l"));
        assert!(at("hand_r").x > 0.3);
        // Arms straight, 30° out from the body, at the character's own length.
        let (shoulder, elbow, wrist) = (at("upper_arm_r"), at("lower_arm_r"), at("hand_r"));
        let upper = elbow - shoulder;
        let lower = wrist - elbow;
        assert!(upper.normalize().dot(lower.normalize()) > 0.999);
        let angle = upper.normalize().dot(Vec3::NEG_Y).acos().to_degrees();
        assert!((angle - 30.0).abs() < 0.5, "{angle}");
        let own = pack.positions[pack.bone("lower_arm_l").unwrap()]
            - pack.positions[pack.bone("upper_arm_l").unwrap()];
        assert!(
            (upper.length() - own.length()).abs() < 1e-4,
            "{} {} {shoulder} {elbow} {wrist}",
            upper.length(),
            own.length()
        );
        // The trunk doesn't move but for the half turn and standing on the origin.
        let head = at("head");
        assert!(
            (head - Vec3::new(0.0, 0.70 + 0.81, 0.0)).length() < 1e-4,
            "{head}"
        );
        // Feet land where the pose has them, toes forward (−Z).
        let foot = pose.bones[pose.index_of("foot_l").unwrap()].translation;
        assert!((at("foot_l") - foot).length() < 1e-4);
        assert!(at("toe_l").z < at("foot_l").z - 0.05);
    }

    #[test]
    fn a_turned_head_turns_the_face_and_a_short_skeleton_keeps_the_feet_down() {
        let pack = stick_pack();
        let rig = Rig::new(&pack, &pack.positions).unwrap();
        let mut pose = rig.rest_pose();
        // Turn the head 90° to the person's left (counter-clockwise from above).
        let head = pose.index_of("head").unwrap();
        pose.bones[head].rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        // A skeleton 10 cm lower: hips and feet both dropped.
        for name in ["hips", "foot_l", "foot_r"] {
            let i = pose.index_of(name).unwrap();
            pose.bones[i].translation.y -= 0.1;
        }
        let map = rig.map(&pose);
        let mut palette = Vec::new();
        rig.palette(&pose, &map, &mut palette);
        let b = pack.bone("head").unwrap();
        // A nose 10 cm in front of the head joint (base mesh +Z).
        let nose = apply(&palette[b], Vec3::new(0.0, 0.70, 0.10));
        let center = apply(&palette[b], Vec3::new(0.0, 0.70, 0.0));
        let ahead = nose - center;
        // Facing −Z, a quarter turn left faces −X.
        assert!(ahead.x < -0.099, "{ahead}");
        let foot = pack.bone("foot_l").unwrap();
        let landed = apply(&palette[foot], pack.positions[foot]);
        let want = pose.bones[pose.index_of("foot_l").unwrap()].translation;
        assert!((landed - want).length() < 1e-4, "{landed} {want}");
    }

    #[test]
    fn hands_reach_where_the_pose_holds_them() {
        let pack = stick_pack();
        let rig = Rig::new(&pack, &pack.positions).unwrap();
        let mut pose = rig.rest_pose();
        // A grip 8 cm in toward the left shoulder and 5 cm forward: the elbow has to bend.
        let (shoulder, hand) = (
            pose.index_of("upper_arm_l").unwrap(),
            pose.index_of("hand_l").unwrap(),
        );
        let s = pose.bones[shoulder].translation;
        let h = pose.bones[hand].translation;
        let grip = h + (s - h).normalize() * 0.08 + Vec3::new(0.0, 0.0, -0.05);
        pose.bones[hand].translation = grip;
        let map = rig.map(&pose);
        let mut palette = Vec::new();
        rig.palette(&pose, &map, &mut palette);
        let at = |name: &str| {
            let b = pack.bone(name).unwrap();
            apply(&palette[b], pack.positions[b])
        };
        assert!(
            (at("hand_l") - grip).length() < 1e-3,
            "{} {grip}",
            at("hand_l")
        );
        // The arm keeps its lengths.
        let len = |a: &str, b: &str| {
            (pack.positions[pack.bone(a).unwrap()] - pack.positions[pack.bone(b).unwrap()]).length()
        };
        let upper = (at("lower_arm_l") - at("upper_arm_l")).length();
        let lower = (at("hand_l") - at("lower_arm_l")).length();
        assert!((upper - len("lower_arm_l", "upper_arm_l")).abs() < 1e-4);
        assert!((lower - len("hand_l", "lower_arm_l")).abs() < 1e-4);
        // The other hand stays put.
        let other = pose.bones[pose.index_of("hand_r").unwrap()].translation;
        assert!((at("hand_r") - other).length() < 1e-3);
        // Out of reach, the hand points toward its target without straightening the arm.
        let elbow = pose.index_of("lower_arm_l").unwrap();
        pose.bones[elbow].rotation *= Quat::from_rotation_x(0.8);
        pose.bones[hand].translation = s + (grip - s).normalize() * 2.0;
        rig.palette(&pose, &map, &mut palette);
        let at = |name: &str| {
            let b = pack.bone(name).unwrap();
            apply(&palette[b], pack.positions[b])
        };
        let upper = at("lower_arm_l") - at("upper_arm_l");
        let lower = at("hand_l") - at("lower_arm_l");
        assert!(upper.normalize().dot(lower.normalize()) < 0.99);
        let toward = (at("hand_l") - at("upper_arm_l")).normalize();
        let want = (pose.bones[hand].translation - at("upper_arm_l")).normalize();
        assert!(toward.dot(want) > 0.999, "{toward} {want}");
    }
}
