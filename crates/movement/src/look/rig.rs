//! The fighters' skeleton and procedural full-body animation.
//!
//! A fighter is rigid armored segments on bones (the renderer has no skinning). Every frame
//! [`Animator::update`] turns the movement and combat state ([`AnimInput`], on the
//! character's own clock) into a [`Pose`]: where the pelvis is, how the spine bends, where
//! the feet, the hands and the sword should be. [`solve`] then places every bone, with
//! two-bone IK for the arms and legs.
//!
//! **Character space**: meters, feet on the ground at the origin, +Y up, facing −Z, +X to
//! the fighter's right. Bones are modeled in a bind pose (standing straight, arms hanging,
//! every bone unrotated); [`Bone::joint`] is where each bone's pivot sits in it.
//!
//! **Sword space** (the katana, the scabbard): the blade points along −Z and its cutting edge
//! faces −Y; the grip of the right hand is at the origin.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat3, Quat, Vec3};
use engine_world::LocomotionMode;

use super::fighter::Style;
use crate::combat::AttackPhase;

/// Height of the ankle joint above the sole, m.
pub const ANKLE_HEIGHT: f32 = 0.085;
/// Hip joint to knee, m.
pub const THIGH: f32 = 0.43;
/// Knee to ankle, m.
pub const SHIN: f32 = 0.415;
/// Shoulder joint to elbow, m.
pub const UPPER_ARM: f32 = 0.29;
/// Elbow to wrist, m.
pub const FOREARM: f32 = 0.26;
/// Where the left hand holds the handle, behind the right hand (sword space +Z), m.
pub const LEFT_GRIP_Z: f32 = 0.125;
/// Where the blade meets the guard (sword space z), m.
pub const BLADE_BASE_Z: f32 = -0.052;
/// Blade length from the guard to the point, m.
pub const BLADE_LENGTH: f32 = 0.72;
/// How far the blade's back rises above a straight line at the point (sori), m.
pub const BLADE_SORI: f32 = 0.02;
/// Where the scabbard's mouth sits in the bind pose (left hip), character space.
pub const SHEATH_MOUTH: Vec3 = Vec3::new(-0.155, 0.995, -0.115);

/// The bones of a fighter. The first [`Bone::ANIMATED`] are posed every frame (children of
/// the character entity); `Sword` rides on the right hand and `Blade` on the sword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bone {
    Pelvis,
    Spine,
    Chest,
    Neck,
    Head,
    UpperArmL,
    ForearmL,
    HandL,
    FingersL,
    UpperArmR,
    ForearmR,
    HandR,
    ThighL,
    ShinL,
    FootL,
    ThighR,
    ShinR,
    FootR,
    /// Shoulder armor: turns halfway between the chest and the upper arm.
    PauldronL,
    PauldronR,
    /// Hip armor hanging from the belt: turns halfway between the pelvis and the thigh.
    TassetL,
    TassetR,
    /// The scabbard on the left hip.
    Sheath,
    /// The katana's grip (child of `HandR`).
    Sword,
    /// The blade, from the guard to the point (child of `Sword`).
    Blade,
}

/// Number of bones.
pub const BONES: usize = 25;

impl Bone {
    pub const ALL: [Bone; BONES] = [
        Bone::Pelvis,
        Bone::Spine,
        Bone::Chest,
        Bone::Neck,
        Bone::Head,
        Bone::UpperArmL,
        Bone::ForearmL,
        Bone::HandL,
        Bone::FingersL,
        Bone::UpperArmR,
        Bone::ForearmR,
        Bone::HandR,
        Bone::ThighL,
        Bone::ShinL,
        Bone::FootL,
        Bone::ThighR,
        Bone::ShinR,
        Bone::FootR,
        Bone::PauldronL,
        Bone::PauldronR,
        Bone::TassetL,
        Bone::TassetR,
        Bone::Sheath,
        Bone::Sword,
        Bone::Blade,
    ];

    /// Bones posed every frame (the rest ride on their parents).
    pub const ANIMATED: usize = 23;

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Bone::Pelvis => "pelvis",
            Bone::Spine => "spine",
            Bone::Chest => "chest",
            Bone::Neck => "neck",
            Bone::Head => "head",
            Bone::UpperArmL => "upper_arm_l",
            Bone::ForearmL => "forearm_l",
            Bone::HandL => "hand_l",
            Bone::FingersL => "fingers_l",
            Bone::UpperArmR => "upper_arm_r",
            Bone::ForearmR => "forearm_r",
            Bone::HandR => "hand_r",
            Bone::ThighL => "thigh_l",
            Bone::ShinL => "shin_l",
            Bone::FootL => "foot_l",
            Bone::ThighR => "thigh_r",
            Bone::ShinR => "shin_r",
            Bone::FootR => "foot_r",
            Bone::PauldronL => "pauldron_l",
            Bone::PauldronR => "pauldron_r",
            Bone::TassetL => "tasset_l",
            Bone::TassetR => "tasset_r",
            Bone::Sheath => "sheath",
            Bone::Sword => "sword",
            Bone::Blade => "blade",
        }
    }

    /// The mirror-image bone (left ↔ right).
    pub fn mirror(self) -> Bone {
        match self {
            Bone::UpperArmL => Bone::UpperArmR,
            Bone::ForearmL => Bone::ForearmR,
            Bone::HandL => Bone::HandR,
            Bone::UpperArmR => Bone::UpperArmL,
            Bone::ForearmR => Bone::ForearmL,
            Bone::HandR => Bone::HandL,
            Bone::ThighL => Bone::ThighR,
            Bone::ShinL => Bone::ShinR,
            Bone::FootL => Bone::FootR,
            Bone::ThighR => Bone::ThighL,
            Bone::ShinR => Bone::ShinL,
            Bone::FootR => Bone::FootL,
            Bone::PauldronL => Bone::PauldronR,
            Bone::PauldronR => Bone::PauldronL,
            Bone::TassetL => Bone::TassetR,
            Bone::TassetR => Bone::TassetL,
            b => b,
        }
    }

    /// Where the bone pivots in the bind pose, character space. Meshes of a bone are modeled
    /// in the bind pose; the bone's own frame is this point with no rotation (except
    /// `Sheath`, `Sword` and `Blade`, which are modeled in sword space).
    pub fn joint(self) -> Vec3 {
        let v = Vec3::new;
        match self {
            Bone::Pelvis => v(0.0, 0.97, 0.0),
            Bone::Spine => v(0.0, 1.07, 0.005),
            Bone::Chest => v(0.0, 1.22, 0.01),
            Bone::Neck => v(0.0, 1.5, 0.02),
            Bone::Head => v(0.0, 1.615, 0.01),
            Bone::UpperArmR | Bone::PauldronR => v(0.19, 1.44, 0.02),
            Bone::ForearmR => v(0.19, 1.44 - UPPER_ARM, 0.02),
            Bone::HandR => v(0.19, 1.44 - UPPER_ARM - FOREARM, 0.02),
            Bone::ThighR => v(0.095, 0.93, 0.0),
            Bone::ShinR => v(0.095, 0.93 - THIGH, 0.0),
            Bone::FootR => v(0.095, ANKLE_HEIGHT, 0.0),
            Bone::TassetR => v(0.13, 1.0, -0.02),
            Bone::FingersL => Bone::HandL.joint() + KNUCKLE_L,
            Bone::Sheath => SHEATH_MOUTH,
            Bone::Sword | Bone::Blade => Vec3::ZERO,
            b => b.mirror().joint() * Vec3::new(-1.0, 1.0, 1.0),
        }
    }
}

/// The left knuckles relative to the left wrist (bind pose), m.
pub const KNUCKLE_L: Vec3 = Vec3::new(0.004, -0.094, 0.0);

/// A rigid transform: rotation, then translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xf {
    pub pos: Vec3,
    pub rot: Quat,
}

impl Default for Xf {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Xf {
    pub const IDENTITY: Xf = Xf {
        pos: Vec3::ZERO,
        rot: Quat::IDENTITY,
    };

    pub fn new(pos: Vec3, rot: Quat) -> Self {
        Self { pos, rot }
    }

    pub fn at(pos: Vec3) -> Self {
        Self::new(pos, Quat::IDENTITY)
    }

    /// `self` then `other` (other is expressed in self's frame).
    pub fn then(self, other: Xf) -> Xf {
        Xf::new(self.pos + self.rot * other.pos, self.rot * other.rot)
    }

    pub fn point(self, p: Vec3) -> Vec3 {
        self.pos + self.rot * p
    }

    pub fn inverse(self) -> Xf {
        let r = self.rot.inverse();
        Xf::new(-(r * self.pos), r)
    }

    pub fn lerp(self, other: Xf, t: f32) -> Xf {
        Xf::new(self.pos.lerp(other.pos, t), slerp(self.rot, other.rot, t))
    }
}

fn slerp(a: Quat, b: Quat, t: f32) -> Quat {
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        a.slerp(b, t).normalize()
    }
}

/// Where the right hand holds the sword: the grip in the hand's frame. The handle sits
/// diagonally in the fist (blade tilted toward the fingers), as in a real grip.
pub fn grip_r() -> Xf {
    Xf::new(Vec3::new(-0.03, -0.072, 0.0), Quat::from_rotation_x(-0.38))
}

/// The left hand's grip (the mirror image of the right's).
pub fn grip_l() -> Xf {
    Xf::new(Vec3::new(0.03, -0.072, 0.0), Quat::from_rotation_x(-0.38))
}

/// The scabbard in the pelvis frame (bind pose): mouth at the left hip, pointing back and
/// down, edge up (as a katana is worn).
pub fn sheath_on_pelvis() -> Xf {
    let dir = Vec3::new(-0.3, -0.48, 0.83).normalize();
    Xf::new(
        SHEATH_MOUTH - Bone::Pelvis.joint(),
        sword_rot(dir, Vec3::new(0.15, 1.0, 0.3)),
    )
}

/// The sword when it sits in the scabbard, in sheath space (the guard against the mouth).
pub fn sheathed() -> Xf {
    Xf::at(Vec3::new(0.0, 0.0, 0.062))
}

/// Length of the point section (kissaki), m.
pub const KISSAKI: f32 = 0.042;

/// The blade's profile `s` m from the guard: (back y, edge y, thickness), blade space. The
/// blade curves up toward the back (sori), narrows toward the point, and in the point
/// section the edge sweeps up in a curve to meet the back.
pub fn blade_profile(s: f32) -> (f32, f32, f32) {
    let s = s.clamp(0.0, BLADE_LENGTH);
    let t = s / BLADE_LENGTH;
    let center = BLADE_SORI * t * t;
    let width = 0.031 - 0.0085 * t;
    let mut thick = 0.0074 - 0.0022 * t;
    let mut back = center + width * 0.5;
    let mut edge = center - width * 0.5;
    let k0 = BLADE_LENGTH - KISSAKI;
    if s > k0 {
        let k = ((s - k0) / KISSAKI).min(1.0);
        let round = 1.0 - (1.0 - k * k).max(0.0).sqrt();
        back -= width * 0.22 * round;
        edge = back - (back - edge) * (1.0 - k * k).max(0.0).sqrt();
        thick *= 1.0 - 0.75 * k * k;
    }
    (back, edge, thick)
}

/// The point of the blade in blade space (the blade's base at the origin).
pub fn blade_tip() -> Vec3 {
    Vec3::new(0.0, blade_profile(BLADE_LENGTH).0, -BLADE_LENGTH)
}

/// The rotation that points the blade along `blade` with the cutting edge toward `edge`.
pub fn sword_rot(blade: Vec3, edge: Vec3) -> Quat {
    let z = -blade.normalize_or(Vec3::NEG_Z);
    let e = -edge;
    let mut y = e - z * e.dot(z);
    if y.length_squared() < 1e-6 {
        y = z.any_orthonormal_vector();
    }
    let y = y.normalize();
    let x = y.cross(z);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// A left-hand rotation with the palm facing `palm` and the fingers pointing `fingers`.
pub fn left_hand(palm: Vec3, fingers: Vec3) -> Quat {
    let x = palm.normalize_or(Vec3::X);
    let mut y = -(fingers - x * fingers.dot(x));
    if y.length_squared() < 1e-6 {
        y = x.any_orthonormal_vector();
    }
    let y = y.normalize();
    let z = x.cross(y);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

// ------------------------------------------------------------------------ the pose

/// A full-body pose in character space; see the module docs. [`solve`] turns it into bone
/// transforms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Pelvis joint position and rotation.
    pub pelvis: Vec3,
    pub pelvis_rot: Quat,
    /// Rotations relative to the parent bone.
    pub spine: Quat,
    pub chest: Quat,
    pub neck: Quat,
    pub head: Quat,
    /// Ankle targets (left, right).
    pub ankle: [Vec3; 2],
    /// Foot rotations (left, right).
    pub foot: [Quat; 2],
    /// Where each knee points.
    pub knee: [Vec3; 2],
    /// The sword (its grip, where the right hand holds it).
    pub sword: Xf,
    /// The left hand (wrist) when it isn't on the handle.
    pub hand_l: Xf,
    /// 0..1: the left hand on the sword's handle.
    pub grip_l: f32,
    /// 0..1: the free left hand keeps a straight wrist (follows the forearm).
    pub follow_l: f32,
    /// Where each elbow points (left, right).
    pub elbow: [Vec3; 2],
    /// 0 = open .. 1 = closed left fingers.
    pub curl_l: f32,
    /// Extra rotation of the scabbard (pulled back while drawing).
    pub sheath: Quat,
    /// Whole-body rotation about `pivot` (flips, wall-run tilt).
    pub body: Quat,
    pub pivot: Vec3,
}

impl Default for Pose {
    fn default() -> Self {
        Self::BIND
    }
}

/// Which channels [`Pose::blend`] mixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channels {
    /// Pelvis, legs, spine, head and body rotation.
    Body,
    /// Hands, sword and elbows.
    Arms,
    All,
}

impl Pose {
    /// Standing straight, arms hanging, sword pointing down.
    pub const BIND: Pose = Pose {
        pelvis: Vec3::new(0.0, 0.97, 0.0),
        pelvis_rot: Quat::IDENTITY,
        spine: Quat::IDENTITY,
        chest: Quat::IDENTITY,
        neck: Quat::IDENTITY,
        head: Quat::IDENTITY,
        ankle: [
            Vec3::new(-0.095, ANKLE_HEIGHT, 0.0),
            Vec3::new(0.095, ANKLE_HEIGHT, 0.0),
        ],
        foot: [Quat::IDENTITY; 2],
        knee: [Vec3::NEG_Z; 2],
        sword: Xf {
            pos: Vec3::new(0.16, 0.82, 0.02),
            rot: Quat::IDENTITY,
        },
        hand_l: Xf {
            pos: Vec3::new(-0.19, 0.89, 0.02),
            rot: Quat::IDENTITY,
        },
        grip_l: 0.0,
        follow_l: 1.0,
        elbow: [Vec3::Z; 2],
        curl_l: 0.3,
        sheath: Quat::IDENTITY,
        body: Quat::IDENTITY,
        pivot: Vec3::new(0.0, 1.0, 0.0),
    };

    /// Mixes `other` into `self` by `t` (0 = self, 1 = other) on the chosen channels.
    pub fn blend(&mut self, other: &Pose, t: f32, channels: Channels) {
        let t = t.clamp(0.0, 1.0);
        if t <= 1e-4 {
            return;
        }
        let q = |a: &mut Quat, b: Quat| *a = slerp(*a, b, t);
        let v = |a: &mut Vec3, b: Vec3| *a = a.lerp(b, t);
        if channels != Channels::Arms {
            v(&mut self.pelvis, other.pelvis);
            q(&mut self.pelvis_rot, other.pelvis_rot);
            q(&mut self.spine, other.spine);
            q(&mut self.chest, other.chest);
            q(&mut self.neck, other.neck);
            q(&mut self.head, other.head);
            for i in 0..2 {
                v(&mut self.ankle[i], other.ankle[i]);
                q(&mut self.foot[i], other.foot[i]);
                v(&mut self.knee[i], other.knee[i]);
            }
            q(&mut self.body, other.body);
            v(&mut self.pivot, other.pivot);
        }
        if channels != Channels::Body {
            self.sword = self.sword.lerp(other.sword, t);
            self.hand_l = self.hand_l.lerp(other.hand_l, t);
            self.grip_l += (other.grip_l - self.grip_l) * t;
            self.follow_l += (other.follow_l - self.follow_l) * t;
            self.curl_l += (other.curl_l - self.curl_l) * t;
            for i in 0..2 {
                v(&mut self.elbow[i], other.elbow[i]);
            }
            q(&mut self.sheath, other.sheath);
        }
    }

    /// The chest bone's transform for this pose (before the body rotation).
    pub fn chest_xf(&self) -> Xf {
        let pelvis = Xf::new(self.pelvis, self.pelvis_rot);
        let spine = pelvis.then(Xf::new(
            Bone::Spine.joint() - Bone::Pelvis.joint(),
            self.spine,
        ));
        spine.then(Xf::new(
            Bone::Chest.joint() - Bone::Spine.joint(),
            self.chest,
        ))
    }

    /// Moves bind-pose coordinates with the chest: a point authored in the bind pose in
    /// front of the chest stays in front of the chest when the fighter leans or twists.
    pub fn torso_xf(&self) -> Xf {
        self.chest_xf().then(Xf::at(-Bone::Chest.joint()))
    }

    /// The scabbard's transform for this pose (before the body rotation).
    pub fn sheath_xf(&self) -> Xf {
        Xf::new(self.pelvis, self.pelvis_rot)
            .then(sheath_on_pelvis())
            .then(Xf::new(Vec3::ZERO, self.sheath))
    }
}

// ------------------------------------------------------------------------ solving

/// Places the knee/elbow of a two-bone limb from `a` (hip, shoulder) toward `c` (ankle,
/// wrist) with the joint bending toward `pole`. Returns (joint, end); the end is pulled in
/// when the target is out of reach.
pub fn two_bone(a: Vec3, c: Vec3, l1: f32, l2: f32, pole: Vec3) -> (Vec3, Vec3) {
    let d = c - a;
    let dir = d.normalize_or(Vec3::NEG_Y);
    let dist = d
        .length()
        .clamp((l1 - l2).abs() + 1e-3, l1 + l2 - 1e-4)
        .max(1e-3);
    let end = a + dir * dist;
    let cos_a = ((l1 * l1 + dist * dist - l2 * l2) / (2.0 * l1 * dist)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();
    let mut bend = pole - dir * pole.dot(dir);
    if bend.length_squared() < 1e-8 {
        bend = dir.any_orthonormal_vector();
    }
    let bend = bend.normalize();
    (a + dir * (l1 * cos_a) + bend * (l1 * sin_a), end)
}

/// A limb segment's rotation: local +Y points from the child joint back up to `up` and
/// local +Z toward `back` (the side the joint bends away from: the knee points along −Z,
/// the elbow along +Z in the bind pose).
fn limb(up: Vec3, back: Vec3) -> Quat {
    let y = up.normalize_or(Vec3::Y);
    let mut z = back - y * back.dot(y);
    if z.length_squared() < 1e-8 {
        z = y.any_orthonormal_vector();
    }
    let z = z.normalize();
    let x = y.cross(z);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// The second segment of a hinge: +Y along `up`, the hinge axis kept from the first segment.
fn hinge(up: Vec3, axis: Vec3) -> Quat {
    let y = up.normalize_or(Vec3::Y);
    let mut x = axis - y * axis.dot(y);
    if x.length_squared() < 1e-8 {
        x = y.any_orthonormal_vector();
    }
    let x = x.normalize();
    let z = x.cross(y);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// Bone transforms (character space) for a pose. Indexed by [`Bone::index`].
pub fn solve(p: &Pose) -> [Xf; BONES] {
    let mut out = [Xf::IDENTITY; BONES];
    let off = |a: Bone, b: Bone| b.joint() - a.joint();
    let pelvis = Xf::new(p.pelvis, p.pelvis_rot);
    let spine = pelvis.then(Xf::new(off(Bone::Pelvis, Bone::Spine), p.spine));
    let chest = spine.then(Xf::new(off(Bone::Spine, Bone::Chest), p.chest));
    let neck = chest.then(Xf::new(off(Bone::Chest, Bone::Neck), p.neck));
    let head = neck.then(Xf::new(off(Bone::Neck, Bone::Head), p.head));
    out[Bone::Pelvis.index()] = pelvis;
    out[Bone::Spine.index()] = spine;
    out[Bone::Chest.index()] = chest;
    out[Bone::Neck.index()] = neck;
    out[Bone::Head.index()] = head;

    // Legs.
    let legs = [
        (Bone::ThighL, Bone::ShinL, Bone::FootL, Bone::TassetL),
        (Bone::ThighR, Bone::ShinR, Bone::FootR, Bone::TassetR),
    ];
    for (i, (thigh, shin, foot, tasset)) in legs.into_iter().enumerate() {
        let hip = pelvis.point(off(Bone::Pelvis, thigh));
        let (knee, ankle) = two_bone(hip, p.ankle[i], THIGH, SHIN, p.knee[i]);
        let t_rot = limb(hip - knee, -p.knee[i]);
        let s_rot = hinge(knee - ankle, t_rot * Vec3::X);
        out[thigh.index()] = Xf::new(hip, t_rot);
        out[shin.index()] = Xf::new(knee, s_rot);
        out[foot.index()] = Xf::new(ankle, p.foot[i]);
        out[tasset.index()] = Xf::new(
            pelvis.point(off(Bone::Pelvis, tasset)),
            slerp(p.pelvis_rot, t_rot, 0.5),
        );
    }

    // Right arm: the hand follows the sword.
    let hand_r_target = p.sword.then(grip_r().inverse());
    let shoulder_r = chest.point(off(Bone::Chest, Bone::UpperArmR));
    let (elbow_r, wrist_r) = two_bone(
        shoulder_r,
        hand_r_target.pos,
        UPPER_ARM,
        FOREARM,
        p.elbow[1],
    );
    let ua_r = limb(shoulder_r - elbow_r, p.elbow[1]);
    let fa_r = hinge(elbow_r - wrist_r, ua_r * Vec3::X);
    let hand_r = Xf::new(wrist_r, hand_r_target.rot);
    out[Bone::UpperArmR.index()] = Xf::new(shoulder_r, ua_r);
    out[Bone::ForearmR.index()] = Xf::new(elbow_r, fa_r);
    out[Bone::HandR.index()] = hand_r;
    out[Bone::PauldronR.index()] = Xf::new(shoulder_r, slerp(chest.rot, ua_r, 0.4));

    // Left arm: free, or on the handle behind the right hand.
    let on_handle = p
        .sword
        .then(Xf::at(Vec3::new(0.0, 0.0, LEFT_GRIP_Z)))
        .then(grip_l().inverse());
    let target_l = p.hand_l.lerp(on_handle, p.grip_l);
    let shoulder_l = chest.point(off(Bone::Chest, Bone::UpperArmL));
    let (elbow_l, wrist_l) = two_bone(shoulder_l, target_l.pos, UPPER_ARM, FOREARM, p.elbow[0]);
    let ua_l = limb(shoulder_l - elbow_l, p.elbow[0]);
    let fa_l = hinge(elbow_l - wrist_l, ua_l * Vec3::X);
    let follow = (p.follow_l * (1.0 - p.grip_l)).clamp(0.0, 1.0);
    let hand_l = Xf::new(wrist_l, slerp(target_l.rot, fa_l, follow));
    out[Bone::UpperArmL.index()] = Xf::new(shoulder_l, ua_l);
    out[Bone::ForearmL.index()] = Xf::new(elbow_l, fa_l);
    out[Bone::HandL.index()] = hand_l;
    let curl = p.curl_l.max(p.grip_l).clamp(0.0, 1.0);
    out[Bone::FingersL.index()] = hand_l.then(Xf::new(
        KNUCKLE_L,
        Quat::from_rotation_z(-0.15 + curl * 1.45),
    ));
    out[Bone::PauldronL.index()] = Xf::new(shoulder_l, slerp(chest.rot, ua_l, 0.4));

    out[Bone::Sheath.index()] = p.sheath_xf();
    let sword = hand_r.then(grip_r());
    out[Bone::Sword.index()] = sword;
    out[Bone::Blade.index()] = sword.then(Xf::at(Vec3::new(0.0, 0.0, BLADE_BASE_Z)));

    if p.body != Quat::IDENTITY {
        for x in &mut out {
            x.pos = p.pivot + p.body * (x.pos - p.pivot);
            x.rot = (p.body * x.rot).normalize();
        }
    }
    out
}

// ------------------------------------------------------------------------ animation input

/// An attack in progress, as the animation sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackAnim {
    /// A dash-strike (drawn from the scabbard: an iaido cut).
    pub dash: bool,
    pub phase: AttackPhase,
    /// 0..1 through the current phase.
    pub progress: f32,
    /// Own seconds since the attack started.
    pub secs: f32,
}

/// Everything the animation reads, in character space and own seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimInput {
    /// Own-time seconds since the last update.
    pub dt: f32,
    pub mode: LocomotionMode,
    /// Velocity in character space, m/s (own time).
    pub velocity: Vec3,
    pub grounded: bool,
    pub air_jumps_left: u32,
    /// Normal of the wall being run on (character space) and the distance from the body's
    /// axis to it, m.
    pub wall: Option<(Vec3, f32)>,
    /// The top of the ledge being mantled or hung from (character space) and whether still
    /// rising.
    pub ledge: Option<(Vec3, bool)>,
    /// The grapple hook (character space) and whether it has bitten.
    pub grapple: Option<(Vec3, bool)>,
    /// Riding a zip line: where the hand holds the cable and the way the line runs (unit),
    /// character space.
    pub zip: Option<(Vec3, Vec3)>,
    /// Part of a dash-strike.
    pub dash_strike: bool,
    pub attack: Option<AttackAnim>,
    pub blocking: bool,
    pub stagger: bool,
    pub hits_taken: u32,
    pub defeated: bool,
    /// Has a `Fighter`: holds a guard stance instead of a relaxed one.
    pub fighter: bool,
    /// `AnimationParams::effort` (0..1).
    pub effort: f32,
}

impl Default for AnimInput {
    fn default() -> Self {
        Self {
            dt: 1.0 / 60.0,
            mode: LocomotionMode::Idle,
            velocity: Vec3::ZERO,
            grounded: true,
            air_jumps_left: 1,
            wall: None,
            ledge: None,
            grapple: None,
            zip: None,
            dash_strike: false,
            attack: None,
            blocking: false,
            stagger: false,
            hits_taken: 0,
            defeated: false,
            fighter: true,
            effort: 0.2,
        }
    }
}

// ------------------------------------------------------------------------ helpers

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Fraction to move toward a target this step for an exponential approach at `rate` /s.
fn approach(dt: f32, rate: f32) -> f32 {
    1.0 - (-rate * dt).exp()
}

fn rx(a: f32) -> Quat {
    Quat::from_rotation_x(a)
}

fn ry(a: f32) -> Quat {
    Quat::from_rotation_y(a)
}

fn rz(a: f32) -> Quat {
    Quat::from_rotation_z(a)
}

/// Leans forward (nose down) by `a` radians.
fn pitch(a: f32) -> Quat {
    rx(-a)
}

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// A sword at `grip`, blade along `blade`, edge toward `edge`.
fn sword(grip: Vec3, blade: Vec3, edge: Vec3) -> Xf {
    Xf::new(grip, sword_rot(blade, edge))
}

// ------------------------------------------------------------------------ cuts

/// One sword cut: the grip follows a smooth path through five keys (cocked, three on the
/// way, follow-through) while the blade turns through five directions; the cutting edge
/// always leads (it faces the way the point moves). Torso-relative bind coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cut {
    /// (grip position, blade direction) keys.
    keys: [(Vec3, Vec3); 5],
    /// Chest twist at the start and the end of the cut, radians (+ = turned left).
    twist: (f32, f32),
    /// Two hands on the handle.
    two_hands: bool,
}

/// Catmull-Rom through `p`, `u` in 0..1 over the whole sequence.
fn spline(p: &[Vec3; 5], u: f32) -> Vec3 {
    let s = u.clamp(0.0, 1.0) * 4.0;
    let i = (s.floor() as usize).min(3);
    let t = s - i as f32;
    let p0 = p[i.saturating_sub(1)];
    let (p1, p2) = (p[i], p[i + 1]);
    let p3 = p[(i + 2).min(4)];
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p1
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

impl Cut {
    fn new(keys: [(Vec3, Vec3); 5], twist: (f32, f32)) -> Self {
        Self {
            keys: keys.map(|(g, b)| (g, b.normalize())),
            twist,
            two_hands: true,
        }
    }

    /// Diagonal cut from high right to low left (kesa-giri).
    fn kesa() -> Self {
        Self::new(
            [
                (v3(0.15, 1.6, -0.04), v3(0.3, 0.55, 0.78)),
                (v3(0.11, 1.56, -0.3), v3(0.05, 0.98, -0.2)),
                (v3(0.01, 1.36, -0.44), v3(-0.35, 0.15, -1.0)),
                (v3(-0.07, 1.16, -0.4), v3(-0.6, -0.65, -0.45)),
                (v3(-0.1, 1.05, -0.3), v3(-0.5, -0.8, 0.3)),
            ],
            (-0.35, 0.45),
        )
    }

    /// Rising diagonal from low left to high right (kiriage).
    fn rising() -> Self {
        Self::new(
            [
                (v3(-0.12, 1.0, -0.18), v3(-0.35, -0.75, 0.55)),
                (v3(-0.08, 1.08, -0.42), v3(-0.5, -0.6, -0.6)),
                (v3(0.05, 1.3, -0.47), v3(0.2, 0.3, -1.0)),
                (v3(0.17, 1.5, -0.36), v3(0.6, 0.75, -0.3)),
                (v3(0.2, 1.6, -0.14), v3(0.4, 0.85, 0.35)),
            ],
            (0.45, -0.35),
        )
    }

    /// Horizontal cut from right to left.
    fn horizontal() -> Self {
        Self::new(
            [
                (v3(0.3, 1.32, 0.0), v3(0.75, 0.15, 0.62)),
                (v3(0.24, 1.3, -0.3), v3(0.9, 0.05, -0.45)),
                (v3(0.0, 1.28, -0.47), v3(0.0, 0.0, -1.0)),
                (v3(-0.2, 1.27, -0.36), v3(-0.9, -0.02, -0.45)),
                (v3(-0.26, 1.25, -0.12), v3(-0.8, -0.1, 0.6)),
            ],
            (-0.4, 0.5),
        )
    }

    /// The draw-cut of an iaido strike: out of the scabbard at the left hip, sweeping across
    /// to the right with one hand.
    fn iaido() -> Self {
        Self {
            two_hands: false,
            ..Self::new(
                [
                    (v3(-0.14, 1.06, -0.3), v3(-0.45, 0.05, -0.9)),
                    (v3(-0.04, 1.14, -0.48), v3(-0.25, 0.06, -1.0)),
                    (v3(0.12, 1.2, -0.52), v3(0.35, 0.08, -0.95)),
                    (v3(0.35, 1.24, -0.36), v3(0.88, 0.1, -0.45)),
                    (v3(0.46, 1.27, -0.06), v3(0.9, 0.12, 0.4)),
                ],
                (0.55, -0.6),
            )
        }
    }

    fn grip(&self, u: f32) -> Vec3 {
        spline(&self.keys.map(|k| k.0), u)
    }

    fn blade(&self, u: f32) -> Vec3 {
        spline(&self.keys.map(|k| k.1), u).normalize_or(Vec3::NEG_Z)
    }

    /// Where the point is at `u` (for the edge direction).
    fn point(&self, u: f32) -> Vec3 {
        self.grip(u) + self.blade(u) * 0.85
    }

    /// The sword `u` (0..1) through the cut.
    fn at(&self, u: f32) -> Xf {
        let e = 0.02;
        let (u0, u1) = if u < e {
            (0.0, e)
        } else if u > 1.0 - e {
            (1.0 - e, 1.0)
        } else {
            (u - e, u + e)
        };
        let blade = self.blade(u);
        let motion = self.point(u1) - self.point(u0);
        let edge = motion - blade * motion.dot(blade);
        sword(self.grip(u), blade, edge)
    }
}

// ------------------------------------------------------------------------ the animator

/// Blend weights, each eased toward its target.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Weights {
    moving: f32,
    crouch: f32,
    slide: f32,
    air: f32,
    wall: f32,
    mantle: f32,
    hang: f32,
    /// The arm out toward the grapple hook.
    aim: f32,
    /// Reeled in on the grapple's rope.
    reel: f32,
    /// Hanging from a zip line.
    zip: f32,
    dash: f32,
    attack: f32,
    block: f32,
    sheathed: f32,
    defeat: f32,
    guard: f32,
}

/// Procedural animation state for one fighter. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct Animator {
    pub style: Style,
    /// Gait cycle, 0..1 (left foot strikes at 0, right at 0.5).
    pub phase: f32,
    speed: f32,
    dir: Vec3,
    lean: Vec3,
    prev_velocity: Vec3,
    w: Weights,
    wall_side: f32,
    air_secs: f32,
    land_secs: f32,
    land_depth: f32,
    flip_secs: Option<f32>,
    prev_air_jumps: u32,
    was_grounded: bool,
    hit_secs: f32,
    prev_hits: u32,
    defeat_secs: f32,
    attacks: u32,
    attack_secs: f32,
    cut: Option<Cut>,
    /// Last cut angle reached (for the recovery hold).
    cut_u: f32,
    ledge: Option<(Vec3, bool)>,
    /// Where the grapple hook was last (character space).
    hook: Option<Vec3>,
    /// Hand-over-hand cycle while shimmying along a ledge, 0..1.
    shimmy: f32,
    /// The zip line last held (grip and direction, character space) and whether it still is.
    zip: Option<(Vec3, Vec3)>,
    zip_on: bool,
    /// Swing about the zip line's grip (rad, legs forward when positive) and its rate.
    swing: f32,
    swing_rate: f32,
    secs: f32,
    last: Option<Pose>,
}

impl Animator {
    pub fn new(style: Style) -> Self {
        Self {
            style,
            phase: 0.0,
            speed: 0.0,
            dir: Vec3::NEG_Z,
            lean: Vec3::ZERO,
            prev_velocity: Vec3::ZERO,
            w: Weights {
                guard: 1.0,
                ..Default::default()
            },
            wall_side: 1.0,
            air_secs: 0.0,
            land_secs: 10.0,
            land_depth: 0.0,
            flip_secs: None,
            prev_air_jumps: 0,
            was_grounded: true,
            hit_secs: 10.0,
            prev_hits: 0,
            defeat_secs: 0.0,
            attacks: 0,
            attack_secs: 0.0,
            cut: None,
            cut_u: 0.0,
            ledge: None,
            hook: None,
            shimmy: 0.0,
            zip: None,
            zip_on: false,
            swing: 0.0,
            swing_rate: 0.0,
            secs: 0.0,
            last: None,
        }
    }

    /// Forget blends and events (after a teleport or a revive).
    pub fn reset(&mut self) {
        *self = Self::new(self.style);
    }

    /// Advances the animation by `input.dt` own seconds and returns the pose.
    pub fn update(&mut self, input: &AnimInput) -> Pose {
        let dt = input.dt.clamp(0.0, 0.1);
        self.secs += dt;
        self.track_events(input, dt);
        self.ease_weights(input, dt);

        let body = self.body_pose(input);
        let arms = self.arm_pose(input, &body);
        let mut pose = body;
        pose.blend(&arms, 1.0, Channels::Arms);

        // Smooth what can jump (cut selection, mode switches); strikes stay sharp.
        let striking = input.attack.is_some_and(|a| a.phase == AttackPhase::Strike);
        if let Some(last) = self.last {
            let mut smoothed = last;
            let k_arms = if striking { 1.0 } else { approach(dt, 28.0) };
            smoothed.blend(&pose, approach(dt, 40.0), Channels::Body);
            smoothed.blend(&pose, k_arms, Channels::Arms);
            // Planted feet must not lag behind the gait.
            smoothed.ankle = pose.ankle;
            smoothed.foot = pose.foot;
            pose = smoothed;
        }
        self.last = Some(pose);
        pose
    }

    fn track_events(&mut self, input: &AnimInput, dt: f32) {
        let horizontal = Vec3::new(input.velocity.x, 0.0, input.velocity.z);
        let speed = horizontal.length();
        self.speed += (speed - self.speed) * approach(dt, 9.0);
        if speed > 0.4 {
            let d = horizontal / speed;
            self.dir = (self.dir + (d - self.dir) * approach(dt, 10.0)).normalize_or(d);
        }
        // Hanging from a zip line, the body swings about the hand like a pendulum: back when
        // the line speeds it up, forward when it slows.
        if let Some((_, along)) = input.zip {
            let ahead = Vec3::new(along.x, 0.0, along.z).normalize_or(Vec3::NEG_Z);
            let acc = if dt > 0.0 && self.zip_on {
                ((input.velocity - self.prev_velocity) / dt).dot(ahead)
            } else {
                0.0
            };
            let pull = -ZIP_SWING_W2 * self.swing
                - ZIP_SWING_DAMP * self.swing_rate
                - acc.clamp(-30.0, 30.0) / ZIP_SWING_LENGTH;
            self.swing_rate += pull * dt;
            self.swing = (self.swing + self.swing_rate * dt).clamp(-0.6, 0.6);
            self.zip = input.zip;
        } else {
            self.swing *= 1.0 - approach(dt, 6.0);
            self.swing_rate = 0.0;
        }
        self.zip_on = input.zip.is_some();
        // Body lean from acceleration (m/s² → radians, capped).
        if dt > 0.0 {
            let acc = (input.velocity - self.prev_velocity) / dt;
            let acc = Vec3::new(acc.x, 0.0, acc.z).clamp_length_max(30.0);
            self.lean += (acc * 0.012 - self.lean) * approach(dt, 6.0);
        }
        self.prev_velocity = input.velocity;
        let cycle = cycle_length(self.speed);
        let stepping = input.grounded || input.mode == LocomotionMode::WallRun;
        if stepping {
            self.phase = (self.phase + self.speed * dt / cycle).fract();
        }

        // Air, landing, double jump.
        if input.grounded {
            if !self.was_grounded && self.air_secs > 0.2 {
                self.land_secs = 0.0;
                self.land_depth = (0.06 + self.air_secs * 0.08).min(0.16);
            }
            self.air_secs = 0.0;
            self.flip_secs = None;
        } else {
            self.air_secs += dt;
            if input.air_jumps_left < self.prev_air_jumps {
                self.flip_secs = Some(0.0);
            }
        }
        if let Some(f) = &mut self.flip_secs {
            *f += dt;
        }
        self.land_secs += dt;
        self.was_grounded = input.grounded;
        self.prev_air_jumps = input.air_jumps_left;
        if let Some((n, _)) = input.wall {
            self.wall_side = if n.x < 0.0 { 1.0 } else { -1.0 };
        }
        if input.ledge.is_some() {
            self.ledge = input.ledge;
        }
        if let Some((hook, _)) = input.grapple {
            self.hook = Some(hook);
        }
        if input.mode == LocomotionMode::Climb {
            // One hand-over-hand cycle per 0.6 m along the ledge.
            self.shimmy = (self.shimmy + input.velocity.x * dt / 0.6).rem_euclid(1.0);
        }

        // Hits and defeat.
        if input.hits_taken > self.prev_hits {
            self.hit_secs = 0.0;
        }
        self.prev_hits = input.hits_taken;
        self.hit_secs += dt;
        if input.defeated {
            self.defeat_secs += dt;
        } else {
            self.defeat_secs = 0.0;
        }

        // A new attack picks the next cut.
        match input.attack {
            Some(a) => {
                if self.cut.is_none() || a.secs + 1e-4 < self.attack_secs {
                    self.attacks += 1;
                    self.cut = Some(self.pick_cut(a.dash));
                    self.cut_u = 0.0;
                }
                self.attack_secs = a.secs;
            }
            None => {
                self.attack_secs = 0.0;
                if self.w.attack < 0.02 {
                    self.cut = None;
                }
            }
        }
    }

    fn pick_cut(&self, dash: bool) -> Cut {
        if dash {
            return Cut::iaido();
        }
        let order: [fn() -> Cut; 3] = match self.style {
            Style::Elite => [Cut::rising, Cut::kesa, Cut::horizontal],
            Style::Grunt => [Cut::kesa, Cut::kesa, Cut::horizontal],
            _ => [Cut::kesa, Cut::horizontal, Cut::rising],
        };
        let mut cut = order[(self.attacks as usize).saturating_sub(1) % 3]();
        if self.style == Style::Pilot {
            cut.two_hands = false;
        }
        cut
    }

    fn ease_weights(&mut self, input: &AnimInput, dt: f32) {
        use LocomotionMode as M;
        let mode = input.mode;
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        let w = &mut self.w;
        let ease = |w: &mut f32, target: f32, rate: f32| *w += (target - *w) * approach(dt, rate);
        let grounded_mode = input.grounded && !matches!(mode, M::Slide | M::Dash | M::Mantle);
        ease(
            &mut w.moving,
            on(grounded_mode) * smooth(0.25, 1.6, self.speed),
            10.0,
        );
        let crouch = if mode == M::Crouch {
            1.0
        } else if input.grounded && mode == M::Idle && input.blocking {
            0.35
        } else {
            0.0
        };
        ease(&mut w.crouch, crouch, 9.0);
        ease(&mut w.slide, on(mode == M::Slide), 13.0);
        ease(
            &mut w.air,
            on(!input.grounded
                && input.zip.is_none()
                && matches!(mode, M::Air | M::Idle | M::Walk | M::Run | M::Sprint)),
            9.0,
        );
        ease(&mut w.wall, on(mode == M::WallRun), 11.0);
        ease(&mut w.mantle, on(mode == M::Mantle), 16.0);
        ease(&mut w.hang, on(mode == M::Climb), 14.0);
        ease(&mut w.aim, on(input.grapple.is_some()), 22.0);
        let reeling = input.grapple.is_some_and(|(_, bit)| bit) && mode == M::Air;
        ease(&mut w.reel, on(reeling), 10.0);
        ease(&mut w.zip, on(input.zip.is_some()), 14.0);
        ease(&mut w.dash, on(mode == M::Dash), 20.0);
        let attacking = input.attack.is_some();
        let attack_target = match input.attack {
            Some(a) if a.phase == AttackPhase::Recovery => 1.0 - smooth(0.35, 1.0, a.progress),
            Some(_) => 1.0,
            None => 0.0,
        };
        // Attacks come in fast; they fade out through the recovery.
        let rate = if attack_target > w.attack { 40.0 } else { 9.0 };
        ease(&mut w.attack, attack_target, rate);
        let sheathed = input.attack.is_some_and(|a| {
            a.dash && matches!(a.phase, AttackPhase::DashIn | AttackPhase::Windup)
        }) || (input.dash_strike && !attacking);
        ease(&mut w.sheathed, on(sheathed), 30.0);
        ease(&mut w.block, on(input.blocking && !attacking), 16.0);
        ease(&mut w.defeat, on(input.defeated), 10.0);
        ease(&mut w.guard, on(input.fighter), 4.0);
    }

    // ---------------------------------------------------------------- body

    fn body_pose(&self, input: &AnimInput) -> Pose {
        let w = self.w;
        let mut p = stance(self.style, input.fighter, w.crouch);
        if w.moving > 1e-3 {
            let g = gait(self.speed, self.dir, self.phase, w.crouch, 0.0, self.style);
            p.blend(&g, w.moving, Channels::Body);
        }
        // Lean into acceleration and speed.
        let lean = self.lean.clamp_length_max(0.35) * (w.moving + w.wall);
        if lean.length_squared() > 1e-6 {
            let axis = Vec3::Y.cross(lean).normalize_or(Vec3::X);
            p.spine = Quat::from_axis_angle(axis, lean.length() * 0.6) * p.spine;
        }
        if w.slide > 1e-3 {
            p.blend(&slide(), w.slide, Channels::Body);
        }
        if w.air > 1e-3 {
            let vy = input.velocity.y;
            p.blend(&air(vy, self.flip_secs), w.air, Channels::Body);
        }
        if w.wall > 1e-3 {
            let dist = input.wall.map_or(0.4, |(_, d)| d);
            p.blend(
                &wall_run(self.speed, self.phase, self.wall_side, dist),
                w.wall,
                Channels::Body,
            );
        }
        if w.mantle > 1e-3
            && let Some((top, rising)) = self.ledge
        {
            p.blend(&mantle(top, rising), w.mantle, Channels::Body);
        }
        if w.hang > 1e-3
            && let Some((top, _)) = self.ledge
        {
            let sway = (self.shimmy * TAU).sin() * smooth(0.1, 0.8, input.velocity.x.abs());
            p.blend(&hang(top, sway), w.hang, Channels::Body);
        }
        if w.reel > 1e-3
            && let Some(hook) = self.hook
        {
            p.blend(&reel(hook), w.reel, Channels::Body);
        }
        if w.zip > 1e-3
            && let Some((grip, _)) = self.zip
        {
            p.blend(&zip_ride(grip, self.swing), w.zip, Channels::Body);
        }
        if w.dash > 1e-3 {
            p.blend(&dash(input.dash_strike), w.dash, Channels::Body);
        }
        // Attacks: step into the cut and turn the shoulders with it.
        if w.attack > 1e-3
            && let Some(cut) = self.cut
        {
            let u = self.cut_progress(input);
            let grounded = 1.0 - w.air.max(w.wall).max(w.slide);
            let mut lunge = stance(self.style, true, 0.0);
            lunge_into(&mut lunge, u);
            p.blend(&lunge, w.attack * grounded * (1.0 - w.dash), Channels::Body);
            let (t0, t1) = cut.twist;
            let twist = if u < 0.0 {
                t0 * (1.0 + u).clamp(0.0, 1.0)
            } else {
                lerp(t0, t1, u.clamp(0.0, 1.0))
            };
            p.chest = ry(twist * 0.6 * w.attack) * p.chest;
            p.spine = ry(twist * 0.4 * w.attack) * p.spine;
        }
        if w.block > 1e-3 {
            p.chest = slerp(p.chest, pitch(0.1) * ry(0.25), w.block);
            p.head = slerp(p.head, pitch(0.12), w.block);
        }
        // Landing: absorb the impact in the knees.
        if self.land_secs < 0.45 && w.slide < 0.5 {
            let k = (self.land_secs / 0.08).min(1.0) * (1.0 - smooth(0.08, 0.45, self.land_secs));
            p.pelvis.y -= self.land_depth * k;
            p.spine = pitch(0.25 * k) * p.spine;
        }
        // Hit reaction: thrown back, then recover.
        let hit = hit_envelope(self.hit_secs) * (1.0 - w.defeat);
        if hit > 1e-3 {
            p.pelvis += v3(0.0, -0.06, 0.08) * hit;
            p.spine = pitch(-0.32 * hit) * p.spine;
            p.chest = pitch(-0.22 * hit) * rz(0.12 * hit) * p.chest;
            p.head = pitch(-0.4 * hit) * p.head;
        }
        if w.defeat > 1e-3 {
            p.blend(&fallen(self.defeat_secs), w.defeat, Channels::Body);
        }
        p
    }

    /// Where the cut is: −1..0 through the windup, 0..1 through the strike and follow
    /// through, held at 1 through the recovery.
    fn cut_progress(&self, input: &AnimInput) -> f32 {
        match input.attack {
            Some(a) => match a.phase {
                AttackPhase::DashIn => -1.0,
                AttackPhase::Windup => -1.0 + a.progress,
                AttackPhase::Strike => 0.85 * a.progress,
                AttackPhase::Recovery => 0.85 + 0.15 * smooth(0.0, 0.3, a.progress),
            },
            None => 1.0,
        }
    }

    // ---------------------------------------------------------------- arms

    fn arm_pose(&mut self, input: &AnimInput, body: &Pose) -> Pose {
        let w = self.w;
        let torso = body.torso_xf();
        let to_ground = |mut a: Pose| {
            a.sword = torso.then(a.sword);
            a.hand_l = torso.then(a.hand_l);
            a.elbow = a.elbow.map(|e| torso.rot * e);
            a
        };
        let mut p = to_ground(guard(self.style, input.fighter, w.crouch));
        if w.moving > 1e-3 {
            let c = carry(self.style, self.phase, self.speed);
            p.blend(&to_ground(c), w.moving, Channels::Arms);
        }
        if w.slide > 1e-3 {
            let (torso_arms, left) = slide_arms();
            let mut a = to_ground(torso_arms);
            a.hand_l = left;
            p.blend(&a, w.slide, Channels::Arms);
        }
        if w.air > 1e-3 {
            let tuck = self.flip_secs.map_or(0.0, flip_tuck);
            p.blend(&to_ground(air_arms(tuck)), w.air, Channels::Arms);
        }
        if w.wall > 1e-3 {
            let mut a = carry(self.style, self.phase, self.speed);
            wall_arms(&mut a, self.wall_side);
            p.blend(&to_ground(a), w.wall, Channels::Arms);
        }
        if w.mantle > 1e-3
            && let Some((top, _)) = self.ledge
        {
            p.blend(&mantle_arms(top, torso), w.mantle, Channels::Arms);
        }
        if w.hang > 1e-3
            && let Some((top, _)) = self.ledge
        {
            let moving = smooth(0.1, 0.8, input.velocity.x.abs());
            p.blend(
                &hang_arms(top, torso, self.shimmy, moving),
                w.hang,
                Channels::Arms,
            );
        }
        if w.aim > 1e-3
            && let Some(hook) = self.hook
        {
            let mut a = to_ground(air_arms(0.0));
            aim_arm(&mut a, hook, body);
            p.blend(&a, w.aim, Channels::Arms);
        }
        if w.zip > 1e-3
            && let Some((grip, _)) = self.zip
        {
            let mut a = to_ground(zip_arms());
            zip_hand(&mut a, grip);
            p.blend(&a, w.zip, Channels::Arms);
        }
        if w.dash > 1e-3 {
            p.blend(&to_ground(dash_arms()), w.dash, Channels::Arms);
        }
        if w.block > 1e-3 {
            p.blend(&to_ground(block_arms()), w.block, Channels::Arms);
        }
        if w.attack > 1e-3
            && let Some(cut) = self.cut
        {
            let u = self.cut_progress(input);
            let mut a = guard(self.style, true, 0.0);
            let s = if u < 0.0 {
                // Windup: from the guard to the cocked position.
                let k = smooth(0.0, 1.0, 1.0 + u);
                a.sword.lerp(cut.at(0.0), k)
            } else {
                cut.at(ease_cut(u))
            };
            self.cut_u = u;
            a.sword = s;
            a.grip_l = if cut.two_hands { 1.0 } else { 0.0 };
            if !cut.two_hands {
                // The free hand swings out to balance the cut.
                a.hand_l = Xf::new(
                    v3(-0.34, 1.2, -0.12 + 0.2 * u.clamp(-1.0, 1.0)),
                    left_hand(v3(0.0, -0.3, 1.0), v3(-0.3, -0.6, -0.5)),
                );
                a.follow_l = 0.6;
                a.curl_l = 0.25;
            }
            a.elbow = [v3(-0.7, -0.55, 0.45), v3(0.7, -0.5, 0.5)];
            p.blend(&to_ground(a), w.attack, Channels::Arms);
        }
        // Iaido: the sword in the scabbard, hand on the hilt, until the draw.
        if w.sheathed > 1e-3 {
            let sheath = body.sheath_xf();
            let draw = match input.attack {
                Some(a) if a.phase == AttackPhase::Windup => smooth(0.0, 1.0, a.progress),
                _ => 0.0,
            };
            let mut a = p;
            a.sword = sheath
                .then(sheathed())
                .then(Xf::at(Vec3::new(0.0, 0.0, 0.22 * draw)));
            a.hand_l = sheath
                .then(Xf::new(Vec3::new(0.0, 0.0, -0.06), rz(PI)))
                .then(grip_l().inverse());
            a.grip_l = 0.0;
            a.follow_l = 0.0;
            a.curl_l = 1.0;
            a.elbow = [
                torso.rot * v3(-0.8, -0.5, 0.2),
                torso.rot * v3(0.4, -0.4, 0.8),
            ];
            p.blend(&a, w.sheathed, Channels::Arms);
            p.sheath = slerp(p.sheath, rx(0.25 * draw) * ry(-0.2 * draw), w.sheathed);
        }
        let hit = hit_envelope(self.hit_secs) * (1.0 - w.defeat);
        if hit > 1e-3 {
            p.blend(&to_ground(stagger_arms()), hit * 0.8, Channels::Arms);
        }
        if w.defeat > 1e-3 {
            p.blend(&fallen_arms(self.defeat_secs), w.defeat, Channels::Arms);
        }
        p
    }
}

/// Cut speed profile: a fast, accelerating sweep that brakes into the follow-through.
fn ease_cut(u: f32) -> f32 {
    let t = u.clamp(0.0, 1.0);
    let s = 1.0 - (1.0 - t).powi(3);
    lerp(t * t * (3.0 - 2.0 * t), s, 0.5)
}

/// How hard a hit throws the body back, `secs` after it landed.
fn hit_envelope(secs: f32) -> f32 {
    if secs >= 0.6 {
        0.0
    } else {
        smooth(0.0, 0.05, secs) * (1.0 - smooth(0.12, 0.6, secs))
    }
}

/// Gait cycle (two steps) length at `speed` m/s.
fn cycle_length(speed: f32) -> f32 {
    (1.25 + 0.31 * speed).min(4.3)
}

/// How tucked the body is `t` seconds into a double-jump flip.
fn flip_tuck(t: f32) -> f32 {
    smooth(0.0, 0.12, t) * (1.0 - smooth(0.42, 0.65, t))
}

// ------------------------------------------------------------------------ body poses

/// The standing stance: a guard per style (or a relaxed stance without a fight), lowered
/// by `crouch`.
fn stance(style: Style, fighter: bool, crouch: f32) -> Pose {
    let mut p = Pose::BIND;
    let a = ANKLE_HEIGHT;
    match (style, fighter) {
        (Style::Duelist, true) => {
            p.pelvis = v3(0.0, 0.925, 0.0);
            p.pelvis_rot = ry(0.18);
            p.ankle = [v3(-0.14, a + 0.02, 0.22), v3(0.1, a, -0.2)];
            p.foot = [ry(0.35) * pitch(0.25), ry(0.1)];
            p.spine = pitch(0.04);
            p.chest = ry(-0.12) * pitch(0.04);
            p.head = ry(-0.06);
        }
        (Style::Elite, true) => {
            p.pelvis = v3(0.0, 0.855, 0.04);
            p.pelvis_rot = ry(-0.42) * pitch(0.1);
            p.ankle = [v3(-0.16, a, -0.27), v3(0.22, a + 0.03, 0.3)];
            p.foot = [ry(-0.15), ry(-0.7) * pitch(0.3)];
            p.knee = [v3(-0.15, 0.0, -1.0), v3(0.3, 0.0, -1.0)];
            p.spine = pitch(0.2) * ry(0.15);
            p.chest = pitch(0.16) * ry(0.22);
            p.neck = pitch(-0.12);
            p.head = pitch(-0.2) * rz(-0.08);
        }
        (Style::Grunt, true) => {
            p.pelvis = v3(0.0, 0.9, 0.0);
            p.pelvis_rot = ry(0.12);
            p.ankle = [v3(-0.17, a, 0.12), v3(0.16, a, -0.12)];
            p.foot = [ry(0.3), ry(0.1)];
            p.knee = [v3(-0.35, 0.0, -1.0), v3(0.35, 0.0, -1.0)];
            p.spine = pitch(0.06);
            p.chest = ry(-0.08) * pitch(0.05);
        }
        _ => {
            // Relaxed: weight on the right leg, hips shifted.
            p.pelvis = v3(0.025, 0.955, 0.0);
            p.pelvis_rot = rz(-0.05) * ry(0.05);
            p.ankle = [v3(-0.15, a, -0.06), v3(0.09, a, 0.03)];
            p.foot = [ry(0.25), ry(-0.1)];
            p.knee = [v3(-0.2, 0.0, -1.0), v3(0.15, 0.0, -1.0)];
            p.spine = rz(0.03);
            p.chest = rz(0.03) * ry(0.05);
            p.head = rz(-0.04) * ry(-0.05);
        }
    }
    if crouch > 1e-3 {
        let mut c = p;
        c.pelvis = v3(0.0, 0.6, 0.06);
        c.pelvis_rot = pitch(0.3) * ry(0.15);
        c.ankle = [v3(-0.2, a + 0.05, 0.22), v3(0.17, a, -0.22)];
        c.foot = [ry(0.3) * pitch(0.6), ry(0.05)];
        c.knee = [v3(-0.5, -0.3, -1.0), v3(0.4, 0.0, -1.0)];
        c.spine = pitch(0.25);
        c.chest = pitch(0.12);
        c.head = pitch(-0.5);
        p.blend(&c, crouch, Channels::Body);
    }
    p
}

/// A walk, run or sprint at `speed` m/s along `dir` (character space), feet planted on a
/// ground `floor` m above the origin.
fn gait(speed: f32, dir: Vec3, phase: f32, crouch: f32, floor: f32, style: Style) -> Pose {
    let mut p = Pose::BIND;
    let run = smooth(1.2, 4.5, speed);
    let sprint = smooth(6.0, 9.5, speed);
    let cycle = cycle_length(speed);
    let duty = lerp(0.6, 0.37, run) - 0.07 * sprint;
    let half = (duty * cycle * 0.5).max(0.04);
    let fwd = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
    let forwardness = (-fwd.z).max(0.0);
    for i in 0..2 {
        let side = if i == 0 { -1.0 } else { 1.0 };
        let ph = (phase + 0.5 * i as f32).fract();
        let (along, lift, toe) = if ph < duty {
            let t = ph / duty;
            // Heel strike (toe up), flat, then roll onto the toes.
            let toe =
                -0.22 * (1.0 - t / 0.2).max(0.0) * run + (0.35 + 0.65 * run) * smooth(0.55, 1.0, t);
            (half * (1.0 - 2.0 * t), 0.0, toe)
        } else {
            let t = (ph - duty) / (1.0 - duty);
            let e = t * t * (3.0 - 2.0 * t);
            // Heel kicks up behind, the knee drives forward, the foot reaches out.
            let kick = (0.12 + 0.3 * sprint) * run * (PI * t).sin() * (1.0 - t);
            let along = -half + 2.0 * half * e - kick;
            let height = lerp(0.09, 0.3 + 0.18 * sprint, run);
            let lift = height * (PI * t.powf(0.8)).sin();
            let toe = lerp(0.9 * run, -0.2, smooth(0.3, 1.0, t)) * (0.3 + 0.7 * run);
            (along, lift, toe)
        };
        let width = lerp(0.115, 0.075, run);
        let mut ankle = fwd * (along + 0.04 * run) + Vec3::X * side * width;
        // Rolling over the toes or the heel lifts the ankle.
        let roll_lift = if toe > 0.0 {
            0.13 * toe.min(1.2).sin()
        } else {
            0.055 * (-toe).sin()
        };
        ankle.y = floor + ANKLE_HEIGHT + lift + roll_lift;
        p.ankle[i] = ankle;
        p.foot[i] = ry(-side * 0.08) * pitch(toe);
        p.knee[i] = v3(side * 0.12, 0.0, -1.0);
    }
    let bob_phase = (phase - duty * 0.5) * 2.0 * TAU;
    let bob = (0.018 * (1.0 - run) - (0.035 + 0.01 * sprint) * run) * bob_phase.cos();
    let low = lerp(0.015, 0.055, run) + 0.02 * sprint + crouch * 0.3;
    p.pelvis = v3(0.0, floor + 0.965 - low + bob, 0.0);
    let swing = (phase * TAU).cos();
    let twist = lerp(0.07, 0.17, run) * forwardness;
    p.pelvis_rot = ry(-twist * swing) * rz(0.04 * (phase * 2.0 * TAU).sin() * run);
    let lean = 0.06 + 0.16 * run + 0.2 * sprint + crouch * 0.3;
    let lean_axis = Vec3::Y.cross(fwd).normalize_or(Vec3::NEG_X);
    let lean_q = |a: f32| Quat::from_axis_angle(lean_axis, a);
    p.pelvis_rot = lean_q(lean * 0.4) * p.pelvis_rot;
    p.spine = lean_q(lean * 0.35) * ry(twist * swing * 0.6);
    p.chest = lean_q(lean * 0.25) * ry(twist * swing * 1.0);
    p.neck = pitch(-lean * 0.35);
    p.head = pitch(-lean * 0.55) * ry(-twist * swing * 0.4);
    if style == Style::Elite {
        // The elite runs low and predatory.
        p.pelvis.y -= 0.04 * run;
        p.spine = pitch(0.12 * run) * p.spine;
        p.head = pitch(-0.12 * run) * p.head;
    }
    p
}

fn slide() -> Pose {
    let mut p = Pose::BIND;
    p.pelvis = v3(0.0, 0.3, 0.1);
    p.pelvis_rot = pitch(-0.9) * rz(-0.12);
    p.ankle = [v3(-0.25, 0.09, 0.1), v3(0.12, 0.1, -0.76)];
    p.foot = [ry(0.5) * rz(-0.9) * pitch(0.3), pitch(-0.35)];
    p.knee = [v3(-0.7, -0.4, -1.0), v3(0.1, 1.0, -0.4)];
    p.spine = pitch(0.08) * rz(0.06);
    p.chest = pitch(0.12) * ry(0.18) * rz(0.08);
    p.neck = pitch(0.3);
    p.head = pitch(0.38) * ry(-0.12);
    p
}

/// In the air: tucked while rising, reaching down for the landing while falling, or a
/// tucked front flip `flip` seconds into a double jump.
fn air(vy: f32, flip: Option<f32>) -> Pose {
    let mut p = Pose::BIND;
    let rising = smooth(-2.0, 3.0, vy);
    let a = ANKLE_HEIGHT;
    let mut up = p;
    up.pelvis = v3(0.0, 0.98, 0.0);
    up.ankle = [v3(-0.12, a + 0.2, 0.32), v3(0.11, a + 0.5, -0.22)];
    up.foot = [pitch(0.7), pitch(0.25)];
    up.knee = [v3(-0.1, -0.3, -1.0), v3(0.15, 0.3, -1.0)];
    up.spine = pitch(0.12);
    up.chest = pitch(0.05);
    up.head = pitch(-0.15);
    let mut down = p;
    down.pelvis = v3(0.0, 0.97, 0.0);
    down.ankle = [v3(-0.15, a + 0.12, 0.12), v3(0.14, a + 0.2, -0.16)];
    down.foot = [pitch(0.35), pitch(0.2)];
    down.knee = [v3(-0.3, 0.0, -1.0), v3(0.3, 0.0, -1.0)];
    down.spine = pitch(0.08);
    down.head = pitch(-0.05);
    p = down;
    p.blend(&up, rising, Channels::Body);
    if let Some(t) = flip {
        let tuck = flip_tuck(t);
        let mut f = p;
        f.pelvis = v3(0.0, 1.0, 0.0);
        f.ankle = [v3(-0.13, 0.86, 0.12), v3(0.13, 0.88, 0.1)];
        f.foot = [pitch(1.0); 2];
        f.knee = [v3(-0.2, 0.8, -1.0), v3(0.2, 0.8, -1.0)];
        f.spine = pitch(0.55);
        f.chest = pitch(0.4);
        f.neck = pitch(0.2);
        f.head = pitch(0.35);
        p.blend(&f, tuck, Channels::Body);
        let turn = smooth(0.0, 0.62, t);
        let turn = turn * turn * (3.0 - 2.0 * turn);
        p.body = rx(-TAU * turn);
        p.pivot = v3(0.0, 1.0, -0.05);
    }
    p
}

/// Running along a wall on `side` (+1 = right) that is `distance` m from the body's axis:
/// the body leans away from the wall and the feet run on it, soles against it.
fn wall_run(speed: f32, phase: f32, side: f32, distance: f32) -> Pose {
    let tilt: f32 = 0.42;
    let pelvis_y = 0.97;
    let g = gait(speed.max(6.0), Vec3::NEG_Z, phase, 0.0, 0.0, Style::Pilot);
    let mut p = g;
    p.pelvis.y = pelvis_y + (g.pelvis.y - 0.9) * 0.5;
    let body = rz(side * tilt);
    let back = body.inverse();
    let pivot = v3(0.0, pelvis_y, 0.0);
    let to_body = |c: Vec3| pivot + back * (c - pivot);
    let wall_x = side * distance.clamp(0.25, 0.6);
    for i in 0..2 {
        let a = g.ankle[i];
        let lift = (a.y - ANKLE_HEIGHT).max(0.0);
        // Planted feet press against the wall; lifted feet swing off it and up.
        let c = v3(
            wall_x - side * (ANKLE_HEIGHT + lift * 0.8),
            pelvis_y - 0.62 + lift * 0.4,
            a.z,
        );
        p.ankle[i] = to_body(c);
        p.foot[i] = back * rz(side * 1.2) * g.foot[i];
        p.knee[i] = back * v3(-side * 0.3, 0.35, -1.0);
    }
    p.body = body;
    p.pivot = pivot;
    // The upper body leans further off the wall and looks ahead.
    p.chest = rz(side * 0.12) * p.chest;
    p.head = rz(side * 0.2) * p.head;
    p
}

/// Climbing onto a ledge whose top is at `top` (character space).
fn mantle(top: Vec3, rising: bool) -> Pose {
    let mut p = Pose::BIND;
    let ledge_y = top.y;
    let z = top.z.min(-0.15);
    p.pelvis = v3(0.0, 0.92, 0.05);
    p.spine = pitch(0.35);
    p.chest = pitch(0.25);
    p.head = pitch(-0.3);
    if rising {
        p.ankle = [
            v3(-0.12, (ledge_y - 0.75).max(0.1) + 0.2, 0.18),
            v3(0.13, (ledge_y - 0.45).max(0.2), -0.05),
        ];
        p.foot = [pitch(0.6), pitch(0.3)];
        p.knee = [v3(-0.2, 0.2, -1.0), v3(0.2, 0.4, -1.0)];
    } else {
        p.pelvis = v3(0.0, 0.88, -0.05);
        p.ankle = [
            v3(-0.12, ledge_y.max(0.0) + ANKLE_HEIGHT, z + 0.1),
            v3(0.13, (ledge_y - 0.3).max(0.1) + 0.15, 0.25),
        ];
        p.foot = [Quat::IDENTITY, pitch(0.6)];
        p.knee = [v3(-0.1, 0.3, -1.0), v3(0.25, -0.2, -1.0)];
        p.spine = pitch(0.45);
    }
    p
}

/// Hanging from a ledge whose top is at `top` (character space): the body below the hands,
/// legs loose with the toes against the wall, looking up at the edge. `sway` (−1..1) swings
/// the hips side to side while shimmying.
fn hang(top: Vec3, sway: f32) -> Pose {
    let mut p = Pose::BIND;
    let wall = top.z.min(-0.2);
    p.pelvis = v3(sway * 0.04, top.y - 0.87, wall + 0.24);
    p.pelvis_rot = rz(sway * 0.1);
    p.spine = pitch(-0.06);
    p.chest = pitch(-0.1);
    p.neck = pitch(-0.1);
    p.head = pitch(-0.3);
    let ankle = p.pelvis.y - 0.8;
    p.ankle = [
        v3(
            -0.11 + sway * 0.03,
            ankle + 0.02 + sway.max(0.0) * 0.05,
            wall + 0.14,
        ),
        v3(
            0.12 + sway * 0.03,
            ankle + 0.08 + (-sway).max(0.0) * 0.05,
            wall + 0.12,
        ),
    ];
    p.foot = [pitch(0.55), pitch(0.45)];
    p.knee = [v3(-0.1, 0.1, -1.0), v3(0.1, 0.1, -1.0)];
    p.body = rz(-sway * 0.04);
    p.pivot = top - v3(0.0, 0.05, 0.0);
    p
}

/// The zip line pendulum: grip to the body's center of mass (m), its natural frequency
/// squared (g / length, 1/s²) and damping (1/s).
const ZIP_SWING_LENGTH: f32 = 1.0;
const ZIP_SWING_W2: f32 = 9.81 / ZIP_SWING_LENGTH;
const ZIP_SWING_DAMP: f32 = 2.6;

/// Hanging one-handed from a zip line at `grip` (character space): the body hangs under the
/// left shoulder, rolled a little to lift it, legs together with the knees up and the toes
/// pointed, looking down the line. `swing` (rad) swings it about the hand, legs forward when
/// positive.
fn zip_ride(grip: Vec3, swing: f32) -> Pose {
    let mut p = Pose::BIND;
    p.pelvis = v3(grip.x + 0.13, grip.y - 1.03, grip.z + 0.03);
    p.pelvis_rot = rz(-0.1) * ry(0.14);
    p.spine = pitch(-0.05) * rz(-0.06);
    p.chest = pitch(-0.03) * rz(-0.08) * ry(0.1);
    p.neck = pitch(0.06);
    p.head = pitch(0.22) * ry(-0.18);
    let (x, y, z) = (p.pelvis.x, p.pelvis.y, p.pelvis.z);
    p.ankle = [
        v3(x - 0.08, y - 0.66, z - 0.3),
        v3(x + 0.09, y - 0.73, z - 0.15),
    ];
    p.foot = [pitch(0.8), pitch(0.65)];
    p.knee = [v3(-0.12, 0.25, -1.0), v3(0.15, 0.15, -1.0)];
    p.body = pitch(-swing);
    p.pivot = grip;
    p
}

/// On a zip line the sword arm reaches out low to the side for balance, the blade trailing
/// behind.
fn zip_arms() -> Pose {
    let mut p = Pose::BIND;
    p.sword = sword(
        v3(0.42, 1.06, 0.12),
        v3(0.35, -0.3, 1.0),
        v3(0.0, -1.0, 0.15),
    );
    p.elbow = [v3(-1.0, -0.2, 0.3), v3(0.8, -0.5, 0.4)];
    p
}

/// The left fist closed around the cable at `grip` (character space, before the body
/// rotation, which turns about the grip): the wrist under it, the fingers over the top.
fn zip_hand(p: &mut Pose, grip: Vec3) {
    let fingers = v3(0.15, 1.0, 0.0).normalize();
    p.hand_l = Xf::new(
        grip - fingers * (-KNUCKLE_L.y * 0.7),
        left_hand(Vec3::X, fingers),
    );
    p.follow_l = 0.0;
    p.grip_l = 0.0;
    p.curl_l = 0.95;
    p.elbow[0] = v3(-1.0, -0.15, 0.25).normalize();
}

/// Dashing: a low forward lunge through the air.
/// The left shoulder, before the body rotation.
const SHOULDER_L: Vec3 = Vec3::new(-0.19, 1.44, 0.0);

/// Reeled in on the grapple toward `hook` (character space): the body leans along the rope,
/// the head looks up it and the legs trail, knees bent.
fn reel(hook: Vec3) -> Pose {
    let mut p = Pose::BIND;
    let a = ANKLE_HEIGHT;
    let dir = (hook - SHOULDER_L).normalize_or(Vec3::NEG_Z);
    let up = dir.y.clamp(-1.0, 1.0);
    p.pelvis = v3(0.0, 0.99, 0.02);
    p.pelvis_rot = ry(0.2);
    p.spine = pitch(-0.08) * ry(0.12);
    p.chest = pitch(-0.06) * ry(0.18);
    p.neck = pitch(-0.1);
    p.head = pitch(-0.2 - 0.35 * up.max(0.0));
    p.ankle = [v3(-0.13, a + 0.3, 0.05), v3(0.12, a + 0.12, 0.34)];
    p.foot = [pitch(0.55), pitch(0.85)];
    p.knee = [v3(-0.15, 0.1, -1.0), v3(0.15, -0.2, -1.0)];
    // Lean a third of the way along the rope.
    let lean = Quat::from_rotation_arc(Vec3::Y, (dir + Vec3::Y * 0.4).normalize());
    p.body = Quat::IDENTITY.slerp(lean, 0.35);
    p.pivot = v3(0.0, 1.3, 0.0);
    p
}

/// Reaches the left arm out along the rope toward `hook` (character space): the launcher on
/// the wrist points at it, the fist closed around its grip.
fn aim_arm(p: &mut Pose, hook: Vec3, body: &Pose) {
    // Hands are placed before the body rotation is applied.
    let hook = body.pivot + body.body.inverse() * (hook - body.pivot);
    let shoulder = body
        .chest_xf()
        .point(Bone::UpperArmL.joint() - Bone::Chest.joint());
    let dir = (hook - shoulder).normalize_or(Vec3::NEG_Z);
    let palm = Vec3::X - dir * dir.x;
    p.hand_l = Xf::new(
        shoulder + dir * (UPPER_ARM + FOREARM - 0.04),
        left_hand(palm, dir),
    );
    p.follow_l = 0.0;
    p.grip_l = 0.0;
    p.curl_l = 0.95;
    let mut elbow = Vec3::NEG_Y - dir * dir.y;
    if elbow.length_squared() < 1e-4 {
        elbow = Vec3::Z;
    }
    p.elbow[0] = (elbow.normalize() + v3(-0.4, 0.0, 0.0)).normalize();
}

fn dash(strike: bool) -> Pose {
    let mut p = Pose::BIND;
    let a = ANKLE_HEIGHT;
    p.pelvis = v3(0.0, 0.8, 0.05);
    p.pelvis_rot = pitch(0.25);
    p.ankle = [v3(-0.12, a + 0.08, 0.62), v3(0.14, a + 0.25, -0.38)];
    p.foot = [pitch(0.9), pitch(0.1)];
    p.knee = [v3(-0.1, -0.6, -1.0), v3(0.2, 0.2, -1.0)];
    p.spine = pitch(0.3);
    p.chest = pitch(if strike { 0.3 } else { 0.22 });
    p.neck = pitch(-0.2);
    p.head = pitch(-0.45);
    p
}

/// Feet and hips for a cut at progress `u` (−1..1).
fn lunge_into(p: &mut Pose, u: f32) {
    let a = ANKLE_HEIGHT;
    let step = smooth(-0.6, 0.3, u);
    let mut l = *p;
    l.pelvis = v3(0.0, 0.85, -0.08);
    l.pelvis_rot = ry(0.25) * pitch(0.12);
    l.ankle = [v3(-0.16, a + 0.03, 0.34), v3(0.13, a, -0.42)];
    l.foot = [ry(0.5) * pitch(0.45), ry(0.05)];
    l.knee = [v3(-0.3, -0.4, -1.0), v3(0.3, 0.0, -1.0)];
    l.spine = pitch(0.12);
    l.chest = pitch(0.1);
    l.head = pitch(-0.12);
    p.blend(&l, step, Channels::Body);
}

/// Cut down: the knees give, then the body falls forward and lies still.
fn fallen(t: f32) -> Pose {
    let a = ANKLE_HEIGHT;
    let mut kneel = Pose::BIND;
    kneel.pelvis = v3(0.0, 0.58, 0.12);
    kneel.pelvis_rot = pitch(0.15);
    kneel.ankle = [v3(-0.12, a, 0.48), v3(0.14, a, 0.42)];
    kneel.foot = [pitch(0.4); 2];
    kneel.knee = [v3(-0.1, -1.0, -0.5), v3(0.1, -1.0, -0.5)];
    kneel.spine = pitch(0.3);
    kneel.chest = pitch(0.35);
    kneel.neck = pitch(0.3);
    kneel.head = pitch(0.4);
    let mut down = Pose::BIND;
    down.pelvis = v3(0.04, 0.16, -0.15);
    down.pelvis_rot = pitch(FRAC_PI_2 - 0.05) * rz(0.1);
    down.ankle = [v3(-0.2, 0.1, 0.72), v3(0.2, 0.1, 0.66)];
    down.foot = [pitch(1.4) * rz(0.3), pitch(1.3) * rz(-0.4)];
    down.knee = [v3(-0.3, -1.0, 0.0), v3(0.3, -1.0, 0.0)];
    down.spine = pitch(-0.08);
    down.chest = pitch(-0.1) * rz(0.06);
    down.neck = pitch(-0.25);
    down.head = ry(0.9) * pitch(-0.3);
    let mut p = Pose::BIND;
    p.blend(&kneel, smooth(0.0, 0.3, t), Channels::Body);
    let fall = smooth(0.25, 0.85, t);
    p.blend(&down, fall * fall, Channels::Body);
    p
}

// ------------------------------------------------------------------------ arm poses
// Torso-relative bind coordinates unless stated otherwise.

/// The guard each style holds when standing.
fn guard(style: Style, fighter: bool, crouch: f32) -> Pose {
    let mut p = Pose::BIND;
    match (style, fighter) {
        (Style::Duelist, true) => {
            // Chudan: both hands, the point at the opponent's throat.
            p.sword = sword(
                v3(0.04, 1.06 - crouch * 0.05, -0.36),
                v3(-0.06, 0.52, -1.0),
                v3(0.0, -1.0, -0.5),
            );
            p.grip_l = 1.0;
            p.elbow = [v3(-0.6, -0.8, 0.3), v3(0.6, -0.8, 0.35)];
        }
        (Style::Elite, true) => {
            // One hand, the blade held low and back; the free hand reaches forward.
            p.sword = sword(v3(0.3, 1.0, 0.02), v3(0.4, -0.3, 1.0), v3(0.3, 1.0, 0.1));
            p.hand_l = Xf::new(
                v3(-0.17, 1.2, -0.38),
                left_hand(v3(0.3, -0.5, -0.4), v3(0.1, -0.1, -1.0)),
            );
            p.follow_l = 0.0;
            p.curl_l = 0.55;
            p.elbow = [v3(-0.8, -0.6, 0.3), v3(0.5, -0.4, 0.8)];
        }
        (Style::Grunt, true) => {
            // Hasso: both hands by the right shoulder, the blade up.
            p.sword = sword(
                v3(0.17, 1.3, -0.16),
                v3(0.12, 1.0, 0.3),
                v3(0.0, -0.2, -1.0),
            );
            p.grip_l = 1.0;
            p.elbow = [v3(-0.3, -0.9, 0.2), v3(0.8, -0.5, 0.3)];
        }
        _ => {
            // Relaxed, one hand, the blade low and forward.
            p.sword = sword(
                v3(0.25, 0.92, -0.1),
                v3(0.18, -0.6, -1.0),
                v3(0.0, -1.0, 0.4),
            );
            p.hand_l = Xf::new(v3(-0.25, 0.93, 0.01), Quat::IDENTITY);
            p.follow_l = 1.0;
            p.curl_l = 0.55;
            p.elbow = [v3(-0.4, -0.2, 1.0), v3(0.5, -0.2, 1.0)];
        }
    }
    p
}

/// Arms while running: counter-swinging, the sword carried low and to the side.
fn carry(style: Style, phase: f32, speed: f32) -> Pose {
    let run = smooth(1.2, 4.5, speed);
    let sprint = smooth(6.0, 9.5, speed);
    let swing = (phase * TAU).cos();
    let amp = lerp(0.12, 0.26, run) + 0.05 * sprint;
    let mut p = Pose::BIND;
    if style == Style::Elite && sprint > 0.0 {
        // Low and predatory: both arms back, the blade trailing.
        let back = sprint;
        p.sword = sword(
            v3(0.3, 1.05, 0.22 * back - swing * amp * (1.0 - back)),
            v3(0.35, 0.2, 1.0),
            v3(0.0, -1.0, 0.2),
        );
        p.hand_l = Xf::new(
            v3(-0.3, 1.05, 0.25 * back + swing * amp * (1.0 - back)),
            Quat::IDENTITY,
        );
        p.elbow = [v3(-0.5, 0.3, 1.0), v3(0.5, 0.3, 1.0)];
    } else {
        // Arms pump with bent elbows, each forward with the opposite leg: the fist rises
        // toward the chest in front and drops past the hip behind.
        let pump = |s: f32, side: f32| {
            let f = s * 0.5 + 0.5;
            let walk = v3(side * 0.24, 0.93 + 0.04 * s, -0.12 * s);
            let run_at = v3(
                side * lerp(0.25, 0.17, f),
                lerp(1.0, 1.24, f * f),
                lerp(0.2, -0.3, f),
            );
            walk.lerp(run_at, run * (amp / 0.31).min(1.0))
        };
        let r = -swing;
        p.sword = sword(
            pump(r, 1.0) + v3(0.03, 0.0, 0.0),
            v3(0.25, -0.5 + 0.3 * (r * 0.5 + 0.5), 1.0),
            v3(0.0, -1.0, 0.3),
        );
        p.hand_l = Xf::new(pump(swing, -1.0), Quat::IDENTITY);
        p.elbow = [v3(-0.3, -0.3, 1.0), v3(0.35, -0.3, 1.0)];
    }
    p.follow_l = 1.0;
    p.curl_l = 0.75;
    p
}

/// Sliding: the sword held up and back; the free hand trails on the ground (character
/// space, returned separately).
fn slide_arms() -> (Pose, Xf) {
    let mut p = Pose::BIND;
    p.sword = sword(
        v3(0.33, 1.2, -0.05),
        v3(0.35, 0.45, 1.0),
        v3(0.0, 1.0, -0.2),
    );
    p.follow_l = 0.0;
    p.curl_l = 0.1;
    p.elbow = [v3(-0.6, 0.5, 0.6), v3(0.6, -0.6, 0.4)];
    // The scabbard swings up along the thigh instead of digging into the ground.
    p.sheath = rx(-0.8);
    let left = Xf::new(
        v3(-0.42, 0.105, 0.36),
        left_hand(Vec3::NEG_Y, v3(-0.3, 0.0, 1.0)),
    );
    (p, left)
}

fn air_arms(tuck: f32) -> Pose {
    let mut p = Pose::BIND;
    p.sword = sword(v3(0.4, 1.28, -0.02), v3(0.7, 0.1, 0.65), v3(0.0, -1.0, 0.2));
    p.hand_l = Xf::new(v3(-0.4, 1.3, -0.08), Quat::IDENTITY);
    p.follow_l = 1.0;
    p.curl_l = 0.25;
    p.elbow = [v3(-0.3, -0.6, 0.6), v3(0.3, -0.6, 0.6)];
    if tuck > 0.0 {
        let mut t = p;
        t.sword = sword(v3(0.18, 0.98, -0.32), v3(0.2, 0.5, 1.0), v3(0.0, -1.0, 0.0));
        t.hand_l = Xf::new(v3(-0.16, 0.98, -0.32), Quat::IDENTITY);
        t.curl_l = 0.9;
        t.elbow = [v3(-1.0, -0.2, 0.3), v3(1.0, -0.2, 0.3)];
        p.blend(&t, tuck, Channels::Arms);
    }
    p
}

/// On a wall the sword trails away from it and the free arm balances.
fn wall_arms(p: &mut Pose, side: f32) {
    if side > 0.0 {
        // Wall on the sword side: the blade trails behind, low.
        p.sword = sword(
            v3(0.22, 1.02, 0.18),
            v3(-0.1, -0.35, 1.0),
            v3(0.0, -1.0, 0.0),
        );
        p.hand_l = Xf::new(v3(-0.42, 1.22, -0.12), Quat::IDENTITY);
    } else {
        // Wall on the left: the left hand brushes it, the sword is held out.
        p.sword = sword(v3(0.42, 1.18, 0.0), v3(0.6, 0.25, 0.75), v3(0.0, -1.0, 0.2));
        p.hand_l = Xf::new(v3(-0.4, 1.05, -0.05), Quat::IDENTITY);
    }
    p.elbow = [v3(-0.3, -0.5, 0.8), v3(0.3, -0.5, 0.8)];
}

/// Hands on the ledge whose top is at `top` (character space).
fn mantle_arms(top: Vec3, torso: Xf) -> Pose {
    let mut p = Pose::BIND;
    let z = top.z.min(-0.12) + 0.04;
    p.sword = sword(
        v3(0.24, top.y + 0.05, z),
        v3(0.35, -0.5, 0.8),
        v3(1.0, 0.0, 0.0),
    );
    p.hand_l = Xf::new(
        v3(-0.23, top.y + 0.065, z + 0.04),
        left_hand(Vec3::NEG_Y, Vec3::NEG_Z),
    );
    p.follow_l = 0.0;
    p.curl_l = 0.05;
    p.elbow = [
        torso.rot * v3(-0.8, -0.3, 0.6),
        torso.rot * v3(0.8, -0.3, 0.6),
    ];
    p
}

/// Both hands on the ledge whose top is at `top` (character space). While shimmying
/// (`moving` 0..1) they go hand over hand along it through the `cycle` (0..1).
fn hang_arms(top: Vec3, torso: Xf, cycle: f32, moving: f32) -> Pose {
    let mut p = mantle_arms(top, torso);
    let a = cycle * TAU;
    let reach = |phase: f32| 0.07 * phase.sin() * moving;
    let lift = |phase: f32| 0.03 * phase.cos().max(0.0) * moving;
    p.sword.pos += v3(0.06 + reach(a), lift(a), 0.0);
    p.hand_l.pos += v3(-0.06 + reach(a + PI), lift(a + PI), 0.0);
    p.elbow = [
        torso.rot * v3(-0.9, -0.1, 0.4),
        torso.rot * v3(0.9, -0.1, 0.4),
    ];
    p
}

/// Dashing: the sword swept back, the free hand forward.
fn dash_arms() -> Pose {
    let mut p = Pose::BIND;
    p.sword = sword(
        v3(0.34, 1.08, 0.16),
        v3(0.25, 0.15, 1.0),
        v3(0.0, -1.0, 0.1),
    );
    p.hand_l = Xf::new(
        v3(-0.14, 1.25, -0.45),
        left_hand(v3(0.2, -1.0, 0.0), v3(0.2, 0.0, -1.0)),
    );
    p.follow_l = 0.2;
    p.curl_l = 0.3;
    p.elbow = [v3(-0.8, -0.5, 0.2), v3(0.3, -0.3, 1.0)];
    p
}

/// Blocking: the blade across in front of the face, the free hand bracing its back.
fn block_arms() -> Pose {
    let mut p = Pose::BIND;
    let blade = v3(-1.0, 0.18, -0.12);
    p.sword = sword(v3(0.22, 1.43, -0.3), blade, v3(0.0, 0.55, -1.0));
    let brace = p.sword.point(v3(0.0, 0.0, BLADE_BASE_Z - 0.3));
    let spine = p.sword.rot * Vec3::Y;
    p.hand_l = Xf::new(
        brace + spine * 0.07 + v3(0.0, 0.0, 0.02),
        left_hand(-spine, v3(-0.2, 0.3, -1.0)),
    );
    p.follow_l = 0.0;
    p.curl_l = 0.1;
    p.elbow = [v3(-0.8, -0.6, 0.0), v3(0.8, -0.6, 0.2)];
    p
}

fn stagger_arms() -> Pose {
    let mut p = Pose::BIND;
    p.sword = sword(
        v3(0.42, 1.18, 0.02),
        v3(0.65, 0.55, 0.4),
        v3(0.0, -1.0, 0.0),
    );
    p.hand_l = Xf::new(v3(-0.4, 1.15, 0.0), Quat::IDENTITY);
    p.follow_l = 1.0;
    p.curl_l = 0.2;
    p.elbow = [v3(-0.3, -0.7, 0.6), v3(0.3, -0.7, 0.6)];
    p
}

/// Arms of a fallen fighter (character space): the sword dropped flat beside the body.
fn fallen_arms(t: f32) -> Pose {
    let mut kneel = Pose::BIND;
    kneel.sword = sword(
        v3(0.28, 0.62, -0.22),
        v3(0.2, -0.85, -0.45),
        v3(0.0, 0.0, -1.0),
    );
    kneel.hand_l = Xf::new(v3(-0.25, 0.7, -0.2), Quat::IDENTITY);
    kneel.follow_l = 1.0;
    kneel.curl_l = 0.4;
    kneel.elbow = [v3(-0.3, -0.3, 1.0), v3(0.3, -0.3, 1.0)];
    let mut down = kneel;
    down.sword = sword(
        v3(0.5, 0.055, -0.95),
        v3(0.55, -0.02, -0.8),
        v3(1.0, 0.0, 0.2),
    );
    down.hand_l = Xf::new(
        v3(-0.42, 0.07, -0.85),
        left_hand(Vec3::NEG_Y, v3(-0.3, 0.0, -1.0)),
    );
    down.follow_l = 0.0;
    down.curl_l = 0.3;
    down.elbow = [v3(-0.5, 1.0, 0.3), v3(0.5, 1.0, 0.3)];
    let mut p = kneel;
    p.blend(&down, smooth(0.3, 0.85, t), Channels::Arms);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(anim: &mut Animator, input: AnimInput, secs: f32) -> Pose {
        let mut pose = Pose::BIND;
        let steps = (secs / input.dt).ceil() as usize;
        for _ in 0..steps {
            pose = anim.update(&input);
        }
        pose
    }

    #[test]
    fn bind_pose_solves_to_the_bind_joints() {
        let bones = solve(&Pose::BIND);
        for b in [
            Bone::Pelvis,
            Bone::Chest,
            Bone::ThighL,
            Bone::ShinR,
            Bone::FootL,
            Bone::UpperArmR,
        ] {
            let d = bones[b.index()].pos.distance(b.joint());
            assert!(d < 0.02, "{b:?} is {d} m off its bind joint");
        }
    }

    #[test]
    fn feet_stand_on_the_ground_in_the_idle_pose() {
        for style in Style::ALL {
            let mut anim = Animator::new(style);
            let pose = run(&mut anim, AnimInput::default(), 1.0);
            let bones = solve(&pose);
            for foot in [Bone::FootL, Bone::FootR] {
                let ankle = bones[foot.index()].pos;
                assert!(
                    (ankle.y - ANKLE_HEIGHT).abs() < 0.04,
                    "{style:?} {foot:?} ankle at {ankle}"
                );
                // The ankle reached its target (the leg is long enough).
                let target = pose.ankle[if foot == Bone::FootL { 0 } else { 1 }];
                assert!(
                    ankle.distance(target) < 0.01,
                    "{style:?} {foot:?} can't reach"
                );
            }
        }
    }

    #[test]
    fn running_plants_each_foot_while_it_carries_the_body() {
        let mut anim = Animator::new(Style::Pilot);
        let input = AnimInput {
            mode: LocomotionMode::Run,
            velocity: v3(0.0, 0.0, -6.0),
            ..Default::default()
        };
        run(&mut anim, input, 2.0);
        // Over one cycle each foot spends a while flat on the ground, moving backward at
        // running speed (planted, not sliding).
        let mut planted = [0usize; 2];
        let mut prev: Option<[Vec3; 2]> = None;
        for _ in 0..60 {
            let pose = anim.update(&input);
            let bones = solve(&pose);
            let feet = [
                bones[Bone::FootL.index()].pos,
                bones[Bone::FootR.index()].pos,
            ];
            if let Some(prev) = prev {
                for i in 0..2 {
                    let v = (feet[i] - prev[i]) / input.dt;
                    if feet[i].y < ANKLE_HEIGHT + 0.02 && (v.z - 6.0).abs() < 1.0 {
                        planted[i] += 1;
                    }
                }
            }
            prev = Some(feet);
        }
        assert!(planted.iter().all(|&n| n >= 6), "{planted:?}");
    }

    #[test]
    fn a_strike_sweeps_the_blade_tip_along_an_arc() {
        let mut anim = Animator::new(Style::Duelist);
        run(&mut anim, AnimInput::default(), 0.5);
        let tip_at = |pose: &Pose| {
            let bones = solve(pose);
            bones[Bone::Blade.index()].point(blade_tip())
        };
        let mut attack = AttackAnim {
            dash: false,
            phase: AttackPhase::Windup,
            progress: 0.0,
            secs: 0.0,
        };
        let dt = 1.0 / 240.0;
        let mut tips = Vec::new();
        let windup = 0.28;
        let strike = 0.1;
        let mut t = 0.0;
        while t < windup + strike {
            if t < windup {
                attack.phase = AttackPhase::Windup;
                attack.progress = t / windup;
            } else {
                attack.phase = AttackPhase::Strike;
                attack.progress = (t - windup) / strike;
            }
            attack.secs = t;
            let pose = anim.update(&AnimInput {
                dt,
                attack: Some(attack),
                ..Default::default()
            });
            if t >= windup {
                tips.push(tip_at(&pose));
            }
            t += dt;
        }
        let first = tips[0];
        let last = *tips.last().unwrap();
        // The tip travels far (a real swing, not a twitch) ...
        let path: f32 = tips.windows(2).map(|w| w[0].distance(w[1])).sum();
        assert!(path > 1.5, "the tip only travelled {path} m");
        // ... along a curve: the path is much longer than the chord, and it sweeps from
        // the right side to the left side through the front.
        assert!(path > first.distance(last) * 1.15, "path {path}");
        assert!(first.x > 0.0 && last.x < 0.0, "{first} → {last}");
        let front = tips.iter().map(|p| p.z).fold(f32::MAX, f32::min);
        assert!(front < -0.8, "the cut never reached forward ({front})");
        // The distance from the chest stays roughly constant (an arc about the body).
        let chest = Bone::Chest.joint();
        let radii: Vec<f32> = tips.iter().map(|p| p.distance(chest)).collect();
        let (lo, hi) = radii
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &r| (a.min(r), b.max(r)));
        assert!(hi - lo < 0.6, "radius varies {lo}..{hi}");
    }

    #[test]
    fn the_sheathed_sword_sits_in_the_scabbard() {
        let pose = Pose::BIND;
        let sheath = pose.sheath_xf();
        let s = sheath.then(sheathed());
        let tip = s
            .then(Xf::at(v3(0.0, 0.0, BLADE_BASE_Z)))
            .point(blade_tip());
        // The point ends up inside the scabbard, about a blade length down its axis.
        let local = sheath.inverse().point(tip);
        assert!((local.z + BLADE_LENGTH).abs() < 0.08, "{local}");
        assert!(local.x.abs() < 0.01 && local.y.abs() < 0.03, "{local}");
    }

    #[test]
    fn hanging_puts_both_hands_on_the_ledge_and_the_body_below_it() {
        // A 1.8 m capsule hanging: the top 2.1 m above its feet, the wall 0.39 m in front.
        let top = v3(0.0, 2.1, -0.39);
        for shimmy in [0.0, 1.5] {
            let mut anim = Animator::new(Style::Pilot);
            let input = AnimInput {
                mode: LocomotionMode::Climb,
                grounded: false,
                ledge: Some((top, true)),
                velocity: v3(shimmy, 0.0, 0.0),
                ..Default::default()
            };
            // Settled into the hang, then through two shimmy cycles.
            run(&mut anim, input, 0.6);
            for _ in 0..50 {
                let pose = anim.update(&input);
                let bones = solve(&pose);
                let hand_r = pose.sword.then(grip_r().inverse()).pos;
                for (wrist, target) in [
                    (bones[Bone::HandR.index()].pos, hand_r),
                    (bones[Bone::HandL.index()].pos, pose.hand_l.pos),
                ] {
                    assert!(
                        wrist.distance(target) < 0.03,
                        "{wrist} can't reach {target}"
                    );
                    assert!((wrist.y - top.y).abs() < 0.12, "hand at {wrist}");
                    assert!(wrist.z > top.z - 0.08, "hand inside the wall: {wrist}");
                }
                for b in [
                    Bone::Pelvis,
                    Bone::Chest,
                    Bone::Head,
                    Bone::FootL,
                    Bone::FootR,
                ] {
                    let p = bones[b.index()].pos;
                    assert!(p.z > top.z - 0.02, "{b:?} inside the wall: {p}");
                    assert!(p.y < top.y, "{b:?} above the ledge: {p}");
                }
            }
        }
    }

    #[test]
    fn defeat_lays_the_body_on_the_ground() {
        let mut anim = Animator::new(Style::Grunt);
        let pose = run(
            &mut anim,
            AnimInput {
                defeated: true,
                ..Default::default()
            },
            2.0,
        );
        let bones = solve(&pose);
        for b in [Bone::Pelvis, Bone::Chest, Bone::Head] {
            assert!(
                bones[b.index()].pos.y < 0.4,
                "{b:?} at {}",
                bones[b.index()].pos
            );
        }
    }

    #[test]
    fn sword_rot_points_blade_and_edge() {
        let q = sword_rot(v3(1.0, 0.0, 0.0), v3(0.0, -1.0, 0.0));
        assert!((q * Vec3::NEG_Z).distance(Vec3::X) < 1e-4);
        assert!((q * Vec3::NEG_Y).distance(Vec3::NEG_Y) < 1e-4);
        let h = left_hand(Vec3::X, Vec3::NEG_Y);
        assert!(h.angle_between(Quat::IDENTITY) < 1e-4);
    }
}
