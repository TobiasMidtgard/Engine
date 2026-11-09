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
//! * Fingers (`thumb_01_l` .. `pinky_03_r`) rest as modeled: a finger bone's rest rotation is
//!   its hand's, and the person's own finger shape (slightly bent, fanned) is the rest pose.
//!   A pose without finger bones gets its fingers from `HandControls` ([`crate::hands`]).
//! * Twist bones are never posed: each turns by a share of its limb's roll
//!   ([`crate::skin::TWIST`]).

use engine_core::math::{Mat3, Mat4, Quat, Vec3};
use engine_world::animation::SkeletonPose;
use engine_world::character::BodyFit;

use engine_world::character::HandControls;

use crate::hands::{self, HandAngles};
use crate::pack::HumanPack;
use crate::skin::{self, FINGERS, TWIST};

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
    /// Standing height, soles to crown (m).
    pub height: f32,
    /// Per bone, what it is (body, finger, twist), for the palette's passes.
    pub kind: Vec<BoneKind>,
    /// Per finger bone `_03`: where its tip is at rest (same space as `joints`).
    pub tips: Vec<Option<Vec3>>,
    /// Per bone: its joint's axes at rest (same space as `joints`): bend, side and twist
    /// (zero for bones without measured axes; see [`Self::joint_axes`]).
    pub axes: Vec<[Vec3; 3]>,
    /// Per finger bone: how far the person's fingers already bend at rest, rad (the hand
    /// pose library's angles count from straight).
    pub rest_bend: Vec<f32>,
}

/// What a bone of the rig is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoneKind {
    Body,
    /// Finger `0..5` (thumb first), joint `0..3` (knuckle first), side (0 left, 1 right).
    Finger(u8, u8, u8),
    /// Index into [`TWIST`], side.
    Twist(u8, u8),
}

fn side_of(bone: &str) -> Option<usize> {
    if bone.ends_with("_l") {
        Some(0)
    } else if bone.ends_with("_r") {
        Some(1)
    } else {
        None
    }
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
            .map(|b| skin::parent_of(b).and_then(|p| index(&p)))
            .collect();
        let mean = |verts: &[u32]| {
            let sum: Vec3 = verts.iter().map(|v| morphed[*v as usize]).sum();
            to_humanoid(sum / verts.len().max(1) as f32)
        };
        let mut joints: Vec<Vec3> = pack.joints.iter().map(|v| mean(v)).collect();
        joints.resize(bones.len(), Vec3::ZERO);
        let kind: Vec<BoneKind> = bones
            .iter()
            .map(|b| {
                let side = side_of(b).unwrap_or(0) as u8;
                if let Some((f, j)) = skin::finger_of(b) {
                    BoneKind::Finger(f as u8, j as u8, side)
                } else if let Some(t) = TWIST.iter().position(|t| b.starts_with(t.bone)) {
                    BoneKind::Twist(t as u8, side)
                } else {
                    BoneKind::Body
                }
            })
            .collect();
        // Twist bones sit along their limbs.
        for (b, k) in kind.iter().enumerate() {
            if let BoneKind::Twist(t, side) = *k {
                let t = TWIST[t as usize];
                let s = ["l", "r"][side as usize];
                if let (Some(l), Some(c)) = (
                    index(&format!("{}_{s}", t.limb)),
                    index(&format!("{}_{s}", t.child)),
                ) {
                    joints[b] = joints[l].lerp(joints[c], t.at);
                }
            }
        }
        let mut tips = vec![None; bones.len()];
        for (name, verts) in &pack.tips {
            let Some((finger, side)) = name.split_once("_tip_") else {
                continue;
            };
            if let Some(b) = index(&format!("{finger}_03_{side}"))
                && !verts.is_empty()
            {
                tips[b] = Some(mean(verts));
            }
        }
        // The root stands on the ground under the hips.
        let mut ground = morphed.iter().fold(f32::MAX, |m, p| m.min(p.y));
        if let (Some(root), Some(hips)) = (index("root"), index("hips")) {
            ground = joints[root].y;
            joints[root] = Vec3::new(joints[hips].x, ground, joints[hips].z);
        }
        let body = pack.group_vertices("body");
        let crown = if body.is_empty() {
            morphed.iter().fold(f32::MIN, |m, p| m.max(p.y))
        } else {
            body.iter()
                .fold(f32::MIN, |m, v| m.max(morphed[*v as usize].y))
        };
        let side = |b: &str| {
            if b.ends_with("_l") { -1.0 } else { 1.0 }
        };
        let rest: Vec<Quat> = bones
            .iter()
            .zip(&kind)
            .map(|(b, k)| {
                let arm = b.starts_with("upper_arm")
                    || b.starts_with("lower_arm")
                    || b.starts_with("hand")
                    || matches!(k, BoneKind::Finger(..));
                if arm {
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
        // Fingers keep their shape on the hand; twist bones turn with their limbs.
        for b in 0..bones.len() {
            let carrier = match kind[b] {
                BoneKind::Finger(_, _, side) => {
                    index(&format!("hand_{}", ["l", "r"][side as usize]))
                }
                BoneKind::Twist(t, side) => index(&format!(
                    "{}_{}",
                    TWIST[t as usize].limb,
                    ["l", "r"][side as usize]
                )),
                BoneKind::Body => None,
            };
            if let Some(c) = carrier {
                correction[b] = correction[c];
            }
        }
        let mut rig = Self {
            bones,
            parents,
            joints,
            rest,
            correction,
            height: (crown - ground).max(0.0),
            axes: vec![[Vec3::ZERO; 3]; kind.len()],
            rest_bend: vec![0.0; kind.len()],
            kind,
            tips,
        };
        rig.measure_hands();
        Some(rig)
    }

    /// Index of a bone.
    pub fn bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b == name)
    }

    /// Where a bone's segment ends at rest: its child joint (the first child along the
    /// limb), or a finger's tip.
    pub fn segment_end(&self, b: usize) -> Option<Vec3> {
        if let Some(t) = self.tips[b] {
            return Some(t);
        }
        let name = &self.bones[b];
        let child = |n: &str| self.bone(n).map(|i| self.joints[i]);
        let s = name.rsplit_once('_').map(|x| x.1).unwrap_or("");
        let base = name.strip_suffix(&format!("_{s}")).unwrap_or(name);
        match (base, s) {
            (_, _) if matches!(self.kind[b], BoneKind::Finger(..)) => {
                let (f, j) = skin::finger_of(name)?;
                child(&format!("{}_0{}_{s}", FINGERS[f], j + 2))
            }
            ("upper_arm", _) => child(&format!("lower_arm_{s}")),
            ("lower_arm", _) => child(&format!("hand_{s}")),
            ("hand", _) => child(&format!("middle_01_{s}")),
            ("upper_leg", _) => child(&format!("lower_leg_{s}")),
            ("lower_leg", _) => child(&format!("foot_{s}")),
            ("foot", _) => child(&format!("toe_{s}")),
            ("clavicle", _) => child(&format!("upper_arm_{s}")),
            ("hips", _) => child("spine"),
            ("spine", _) => child("chest"),
            ("chest", _) => child("neck"),
            ("neck", _) => child("head"),
            _ => None,
        }
    }

    /// Measures each hand's palm and every finger joint's axes and rest bend.
    fn measure_hands(&mut self) {
        for (si, s) in ["l", "r"].into_iter().enumerate() {
            let j = |n: String| self.bone(&n).map(|i| self.joints[i]);
            let (Some(wrist), Some(index), Some(middle), Some(pinky)) = (
                j(format!("hand_{s}")),
                j(format!("index_01_{s}")),
                j(format!("middle_01_{s}")),
                j(format!("pinky_01_{s}")),
            ) else {
                continue;
            };
            let along = (middle - wrist).normalize_or_zero();
            // Toward the thumb's side.
            let across = {
                let a = index - pinky;
                (a - along * a.dot(along)).normalize_or_zero()
            };
            let mut palm = along.cross(across).normalize_or_zero();
            // The palm is the side the fingers curl toward at rest.
            let mut bend = Vec3::ZERO;
            for finger in &FINGERS[1..] {
                let name = |k: u32| format!("{finger}_0{k}_{s}");
                if let (Some(a), Some(b)) = (self.bone(&name(1)), self.bone(&name(3))) {
                    let tip = self.tips[b].unwrap_or(self.joints[b]);
                    let d = self.segment_end(a).unwrap_or(tip) - self.joints[a];
                    let d = d.normalize_or_zero();
                    let t = tip - self.joints[a];
                    bend += t - d * t.dot(d);
                }
            }
            if bend.dot(palm) < 0.0 {
                palm = -palm;
            }
            let hand = self.bone(&format!("hand_{s}"));
            if let Some(h) = hand {
                // Bend (palm toward the forearm), toward the thumb, and the turn of the palm.
                let flex = along.cross(palm).normalize_or_zero();
                let radial = along.cross(across).normalize_or_zero();
                // A positive turn turns the palm backward from the hanging arm's rest (pronation).
                let c = self.correction[h];
                let mut twist = along;
                if (c * twist).cross(c * palm).z < 0.0 {
                    twist = -twist;
                }
                self.axes[h] = [flex, radial, twist];
            }
            for b in 0..self.bones.len() {
                let BoneKind::Finger(f, k, side) = self.kind[b] else {
                    continue;
                };
                if side as usize != si {
                    continue;
                }
                let Some(end) = self.segment_end(b) else {
                    continue;
                };
                let d = (end - self.joints[b]).normalize_or_zero();
                if d == Vec3::ZERO {
                    continue;
                }
                let perp = |v: Vec3| (v - d * v.dot(d)).normalize_or_zero();
                // The direction this joint bends the finger toward.
                let toward = if f == 0 {
                    // The thumb: its base swings across the palm toward the little finger;
                    // its other joints bend its pad toward the palm and the fingers.
                    if k == 0 {
                        perp(-across)
                    } else {
                        perp(palm * 0.5 - across * 0.85)
                    }
                } else {
                    perp(palm)
                };
                let flex = d.cross(toward).normalize_or_zero();
                // Spread: away from the middle finger (the thumb's: out of the palm).
                let out = if f == 0 {
                    perp(palm)
                } else {
                    let knuckle = self.joints[b];
                    let o = if f == 2 { across } else { knuckle - middle };
                    perp(o)
                };
                let spread = d.cross(out).normalize_or_zero();
                self.axes[b] = [flex, spread, d];
                // How far it bends already at rest: from the segment before it.
                let before = match (k, self.parents[b]) {
                    (0, _) if f != 0 => Some((self.joints[b] - wrist).normalize_or_zero()),
                    (0, _) => None,
                    (_, Some(p)) => Some((self.joints[b] - self.joints[p]).normalize_or_zero()),
                    _ => None,
                };
                if let Some(before) = before {
                    let x = d.dot(before);
                    let y = d.dot(toward);
                    // Bent toward the palm from the line of the segment before.
                    let y0 = before.dot(toward);
                    self.rest_bend[b] = (y - y0).atan2(x.max(1e-3)).max(0.0);
                }
            }
        }
    }

    /// A joint's bend, side and twist axes at rest, in the rig's rest space; zero for bones
    /// without measured axes (the body's follow the humanoid's, see `crate::anatomy`).
    pub fn joint_axes(&self, b: usize) -> [Vec3; 3] {
        self.axes.get(b).copied().unwrap_or([Vec3::ZERO; 3])
    }

    /// A turn in the rig's rest space (`q`, about rest-space axes such as
    /// [`Self::joint_axes`]) as the local turn a pose gives the bone (its rotation is its
    /// parent's, times the rest offset, times this).
    pub fn pose_turn(&self, b: usize, q: Quat) -> Quat {
        let f = self.rest[b].inverse() * self.correction[b];
        f * q * f.inverse()
    }

    /// A rest-space direction in a bone's pose frame (the frame of [`Self::pose_turn`]'s result).
    pub fn pose_axis(&self, b: usize, v: Vec3) -> Vec3 {
        self.rest[b].inverse() * self.correction[b] * v
    }

    /// A finger joint's turn in rest space for bend, side and twist angles (rad); `bend`
    /// counts from straight, so the person's own rest bend is taken off.
    pub fn finger_turn(&self, b: usize, bend: f32, spread: f32, twist: f32) -> Quat {
        let [f, s, t] = self.joint_axes(b);
        if f == Vec3::ZERO {
            return Quat::IDENTITY;
        }
        let bend = bend - self.rest_bend[b];
        Quat::from_axis_angle(s, spread)
            * Quat::from_axis_angle(f, bend)
            * Quat::from_axis_angle(t, twist)
    }

    /// The rest-space turn of finger bone `b` in a hand pose.
    fn hand_turn(&self, b: usize, angles: &[HandAngles; 2]) -> Quat {
        let BoneKind::Finger(f, k, side) = self.kind[b] else {
            return Quat::IDENTITY;
        };
        let a = &angles[side as usize].fingers[f as usize];
        let spread = if k == 0 { a.spread } else { 0.0 };
        let twist = if k == 0 { a.roll } else { 0.0 };
        self.finger_turn(
            b,
            a.bend[k as usize].to_radians(),
            spread.to_radians(),
            twist.to_radians(),
        )
    }

    /// Where each of this rig's bones is in `pose` (`None` for bones the pose lacks).
    pub fn map(&self, pose: &SkeletonPose) -> Vec<Option<usize>> {
        self.bones.iter().map(|b| pose.index_of(b)).collect()
    }

    /// One matrix per bone taking a base-mesh-space rest point to the humanoid's model space
    /// in `pose` (`map` from [`Self::map`]). Body bones the pose lacks follow their parents,
    /// fingers it lacks hold their rest shape, and twist bones follow their limbs' roll.
    pub fn palette(&self, pose: &SkeletonPose, map: &[Option<usize>], out: &mut Vec<Mat4>) {
        self.palette_with(pose, map, None, out);
    }

    /// [`Self::palette`], with the fingers the pose lacks posed from `hands` (the hand pose
    /// library's shapes and grips).
    pub fn palette_with(
        &self,
        pose: &SkeletonPose,
        map: &[Option<usize>],
        hands: Option<&HandControls>,
        out: &mut Vec<Mat4>,
    ) {
        let n = self.bones.len();
        out.clear();
        let mut turn = vec![Quat::IDENTITY; n];
        let mut at = vec![Vec3::ZERO; n];
        let mut posed = vec![false; n];
        for b in 0..n {
            if self.kind[b] != BoneKind::Body {
                continue;
            }
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
        // Fingers and twist bones, on the arms as the reach left them.
        let angles = hands.map(|h| [hands::from_controls(h, 0), hands::from_controls(h, 1)]);
        for b in 0..n {
            let Some(p) = self.parents[b] else {
                continue;
            };
            match self.kind[b] {
                BoneKind::Body => continue,
                BoneKind::Finger(..) => {
                    let bone = map
                        .get(b)
                        .copied()
                        .flatten()
                        .and_then(|i| pose.bones.get(i));
                    turn[b] = match (bone, &angles) {
                        (Some(pb), _) => pb.rotation * self.rest[b].inverse() * self.correction[b],
                        (None, Some(a)) => turn[p] * self.hand_turn(b, a),
                        (None, None) => turn[p],
                    };
                }
                BoneKind::Twist(t, side) => {
                    turn[b] = self.twist_turn(b, TWIST[t as usize], side as usize, &turn);
                }
            }
            at[b] = at[p] + turn[p] * (self.joints[b] - self.joints[p]);
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

    /// A twist bone's turn: its limb's, plus a share of the roll it follows.
    fn twist_turn(&self, b: usize, t: skin::Twist, side: usize, turn: &[Quat]) -> Quat {
        let s = ["l", "r"][side];
        let (Some(l), Some(c)) = (
            self.bone(&format!("{}_{s}", t.limb)),
            self.bone(&format!("{}_{s}", t.child)),
        ) else {
            return self.parents[b].map_or(Quat::IDENTITY, |p| turn[p]);
        };
        let axis = (self.joints[c] - self.joints[l]).normalize_or(Vec3::NEG_Y);
        let roll = if t.share >= 0.0 {
            turn[l].inverse() * turn[c]
        } else {
            match self.parents[l] {
                Some(p) => (turn[p].inverse() * turn[l]).inverse(),
                None => Quat::IDENTITY,
            }
        };
        let twist = skin::twist_about(roll, axis);
        // The shortest way round, and never more than a forearm turns.
        let twist = if twist.w < 0.0 { -twist } else { twist };
        let (ax, angle) = twist.to_axis_angle();
        let angle = angle.min(2.2) * t.share.abs();
        turn[l] * Quat::from_axis_angle(ax, angle)
    }

    /// This body's rest skeleton and height, for posers fitting their skeletons to it.
    pub fn body_fit(&self) -> BodyFit {
        let pose = self.rest_pose();
        BodyFit {
            names: pose.names.to_vec(),
            parents: pose.parents.to_vec(),
            rest: pose.bones,
            height: self.height,
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
    fn the_body_fit_is_the_rest_pose_and_the_standing_height() {
        let pack = stick_pack();
        let rig = Rig::new(&pack, &pack.positions).unwrap();
        let fit = rig.body_fit();
        let rest = rig.rest_pose();
        assert_eq!(*fit.pose().names, *rest.names);
        assert_eq!(*fit.pose().parents, *rest.parents);
        assert_eq!(fit.rest, rest.bones);
        // Soles (the root) to the highest vertex, the head joint here.
        assert!((fit.height - (0.70 + 0.81)).abs() < 1e-4, "{}", fit.height);
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
    fn a_turned_wrist_twists_the_forearm_along_its_length() {
        let mut pack = stick_pack();
        // A twist bone on the left forearm, carrying one vertex at the elbow's joint.
        pack.bones.push("lower_arm_twist_02_l".into());
        pack.joints.push(Vec::new());
        let rig = Rig::new(&pack, &pack.positions).unwrap();
        let tw = rig.bone("lower_arm_twist_02_l").unwrap();
        assert!(matches!(rig.kind[tw], BoneKind::Twist(..)));
        let (elbow, wrist) = (
            rig.joints[rig.bone("lower_arm_l").unwrap()],
            rig.joints[rig.bone("hand_l").unwrap()],
        );
        // It sits 85% of the way down the forearm.
        assert!((rig.joints[tw] - elbow.lerp(wrist, 0.85)).length() < 1e-5);
        let mut pose = rig.rest_pose();
        let hand = pose.index_of("hand_l").unwrap();
        let fore = pose.index_of("lower_arm_l").unwrap();
        // Turn the hand 90° about the forearm (the forearm's own direction in the pose).
        let axis = (pose.bones[hand].translation - pose.bones[fore].translation).normalize();
        pose.bones[hand].rotation = Quat::from_axis_angle(axis, 1.5) * pose.bones[hand].rotation;
        let map = rig.map(&pose);
        let mut palette = Vec::new();
        rig.palette(&pose, &map, &mut palette);
        let m = |b: usize| Quat::from_mat4(&palette[b]);
        let fb = rig.bone("lower_arm_l").unwrap();
        let rel = m(fb).inverse() * m(tw);
        let (_, angle) = rel.to_axis_angle();
        assert!((angle - 1.5 * 0.85).abs() < 0.02, "{angle}");
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
