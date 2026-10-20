//! Demo characters: a display mannequin and a dog on procedural skeletons (the same
//! `SkeletonPose` contract the animation module writes), with seamless sculpted bodies
//! skinned on the CPU ([`super::skin`]), wearing this module's cloth, hair and fur.

use super::body::{self, BodyPart, Dog, Human, aim};
use super::skin::{ScalpPaint, SkinnedPart};
use super::*;
use crate::hair::FurZone;

/// Which procedural rig a demo skeleton uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rig {
    Humanoid,
    Dog,
}

/// Animates this entity's `SkeletonPose` with a walk (or trot) cycle of `cycle` seconds,
/// covering `stride` meters per cycle (so planted feet stay put on the ground).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DemoSkeleton {
    pub rig: Rig,
    pub cycle: f32,
    pub stride: f32,
}

// ---------------------------------------------------------------------------------------
// Skeletons

pub const HUMANOID: [(&str, Option<u16>); 17] = [
    ("pelvis", None),
    ("spine", Some(0)),
    ("chest", Some(1)),
    ("neck", Some(2)),
    ("head", Some(3)),
    ("upper_arm_l", Some(2)),
    ("forearm_l", Some(5)),
    ("hand_l", Some(6)),
    ("upper_arm_r", Some(2)),
    ("forearm_r", Some(8)),
    ("hand_r", Some(9)),
    ("thigh_l", Some(0)),
    ("shin_l", Some(11)),
    ("foot_l", Some(12)),
    ("thigh_r", Some(0)),
    ("shin_r", Some(14)),
    ("foot_r", Some(15)),
];

/// Legs: upper leg (from the shoulder or hip), lower leg (from the elbow or stifle), pastern
/// (from the wrist or hock) and paw.
pub const DOG: [(&str, Option<u16>); 24] = [
    ("hips", None),
    ("spine", Some(0)),
    ("chest", Some(1)),
    ("neck", Some(2)),
    ("head", Some(3)),
    ("tail_1", Some(0)),
    ("tail_2", Some(5)),
    ("tail_3", Some(6)),
    ("leg_fl", Some(2)),
    ("knee_fl", Some(8)),
    ("ankle_fl", Some(9)),
    ("paw_fl", Some(10)),
    ("leg_fr", Some(2)),
    ("knee_fr", Some(12)),
    ("ankle_fr", Some(13)),
    ("paw_fr", Some(14)),
    ("leg_hl", Some(0)),
    ("knee_hl", Some(16)),
    ("ankle_hl", Some(17)),
    ("paw_hl", Some(18)),
    ("leg_hr", Some(0)),
    ("knee_hr", Some(20)),
    ("ankle_hr", Some(21)),
    ("paw_hr", Some(22)),
];

/// Meters per walk cycle of the mannequins and per trot cycle of the dog, as spawned.
const HUMAN_STRIDE: f32 = 1.25;
const DOG_STRIDE: f32 = 0.6;

pub fn skeleton(bones: &[(&str, Option<u16>)], rig: Rig) -> SkeletonPose {
    let mut pose = SkeletonPose {
        names: bones
            .iter()
            .map(|(n, _)| n.to_string())
            .collect::<Vec<_>>()
            .into(),
        parents: Arc::from(bones.iter().map(|(_, p)| *p).collect::<Vec<_>>()),
        bones: vec![BonePose::default(); bones.len()],
    };
    pose_rig(rig, 0.0, &mut pose);
    pose
}

fn set(p: &mut [BonePose], i: usize, t: Vec3, r: Quat) {
    p[i].translation = t;
    p[i].rotation = r;
    p[i].scale = Vec3::ONE;
}

fn hermite(p0: f32, p1: f32, m0: f32, m1: f32, t: f32) -> f32 {
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * p0
        + (t3 - 2.0 * t2 + t) * m0
        + (-2.0 * t3 + 3.0 * t2) * p1
        + (t3 - t2) * m1
}

/// Smooth minimum of two heights.
fn soft_min(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

/// Two-bone IK: the middle joint for a chain from `root` toward `target` (reaching as far as
/// it can), bending toward `pole`. Returns the middle joint and the end.
fn two_bone_ik(root: Vec3, target: Vec3, l1: f32, l2: f32, pole: Vec3) -> (Vec3, Vec3) {
    let d = target - root;
    let dist = d.length().clamp((l1 - l2).abs() + 1e-4, l1 + l2 - 1e-4);
    let dir = d.normalize_or(Vec3::NEG_Y);
    let cos_a = ((l1 * l1 + dist * dist - l2 * l2) / (2.0 * l1 * dist)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();
    let bend = (pole - dir * pole.dot(dir)).normalize_or(dir.any_orthonormal_vector());
    (root + (dir * cos_a + bend * sin_a) * l1, root + dir * dist)
}

/// Write a walk (humanoid) or trot (dog) pose at `phase` radians into `pose`.
pub fn pose_rig(rig: Rig, phase: f32, pose: &mut SkeletonPose) {
    let stride = match rig {
        Rig::Humanoid => HUMAN_STRIDE,
        Rig::Dog => DOG_STRIDE,
    };
    pose_rig_stride(rig, phase, stride, pose);
}

/// [`pose_rig`] covering `stride` meters per cycle.
pub fn pose_rig_stride(rig: Rig, phase: f32, stride: f32, pose: &mut SkeletonPose) {
    let u = (phase / TAU).rem_euclid(1.0);
    match rig {
        Rig::Humanoid => {
            if pose.bones.len() >= Human::BONES {
                human_walk(u, stride, &mut pose.bones);
            }
        }
        Rig::Dog => {
            if pose.bones.len() >= Dog::BONES {
                dog_trot(u, stride, &mut pose.bones);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// The walk

/// Fraction of the cycle a foot is on the ground.
const HUMAN_STANCE: f32 = 0.6;
const HEEL_STRIKE_PITCH: f32 = 0.3;
const TOE_OFF_PITCH: f32 = -0.75;

/// Ankle height, ankle z (relative to the hips, +Z behind) and foot pitch (+ = toes up) of
/// a planted foot, `c` = 0 at heel strike .. 1 at toe-off: it rolls over the heel, stands flat,
/// then peels off over the ball of the foot.
fn planted_foot(c: f32, travel: f32) -> (f32, f32, f32) {
    let z_ref = -0.5 * travel + travel * c;
    let pitch = if c < 0.15 {
        HEEL_STRIKE_PITCH * (1.0 - smoothstep(0.0, 0.15, c))
    } else if c > 0.5 {
        TOE_OFF_PITCH * smoothstep(0.5, 1.0, c)
    } else {
        0.0
    };
    let (sin, cos) = pitch.sin_cos();
    let (ah, hb, bf) = (Human::ANKLE_HEIGHT, Human::HEEL_BACK, Human::BALL_FORWARD);
    if pitch >= 0.0 {
        // Pivot on the heel.
        (ah * cos + hb * sin, z_ref + hb + ah * sin - hb * cos, pitch)
    } else {
        // Pivot on the ball of the foot.
        (ah * cos - bf * sin, z_ref - bf + ah * sin + bf * cos, pitch)
    }
}

/// Ankle (height, z), foot pitch at gait phase `u` (0 = heel strike).
fn human_foot(u: f32, stride: f32) -> (f32, f32, f32) {
    let travel = stride * HUMAN_STANCE;
    if u < HUMAN_STANCE {
        return planted_foot(u / HUMAN_STANCE, travel);
    }
    let w = (u - HUMAN_STANCE) / (1.0 - HUMAN_STANCE);
    let (y0, z0, p0) = planted_foot(1.0, travel);
    let (y1, z1, p1) = planted_foot(0.0, travel);
    // Leave and land moving with the ground, swing through in between.
    let m = stride * (1.0 - HUMAN_STANCE) * 0.5;
    let z = hermite(z0, z1, m, m, w);
    let y =
        lerp(y0, y1, smoothstep(0.0, 1.0, w)) + 0.075 * (std::f32::consts::PI * w).sin().powf(1.3);
    let pitch = lerp(p0, p1, smoothstep(0.0, 0.55, w)) + 0.15 * (std::f32::consts::PI * w).sin();
    (y, z, pitch)
}

fn human_walk(u: f32, stride: f32, p: &mut [BonePose]) {
    let ur = u;
    let ul = (u + 0.5).rem_euclid(1.0);
    let feet = [human_foot(ul, stride), human_foot(ur, stride)];
    let wave = |x: f32| (x * TAU).cos();
    // Pelvis: turns with the leading leg, lists toward the stance leg, sways over it.
    let yaw = 0.07 * wave(ur);
    let roll = 0.04 * wave(ur - 0.3);
    let sway = 0.016 * wave(ur - 0.3);
    let pelvis_rot =
        Quat::from_rotation_y(yaw) * Quat::from_rotation_z(roll) * Quat::from_rotation_x(-0.04);
    // As high as the planted legs reach (knees never quite locked).
    let reach = 0.995 * (Human::THIGH + Human::SHIN);
    let foot_x = 0.095;
    let mut heights = [0.0f32; 2];
    for (k, side) in [-1.0f32, 1.0].into_iter().enumerate() {
        let (y, z, _) = feet[k];
        let hip_off = pelvis_rot * (body::side_v(Human::HIP, side) - Human::PELVIS);
        let dx = side * foot_x - (sway + hip_off.x);
        let dz = z - hip_off.z;
        let flat = (reach * reach - dx * dx - dz * dz).max(0.0).sqrt();
        heights[k] = y + flat - hip_off.y;
    }
    let pelvis_y = soft_min(soft_min(heights[0], heights[1], 0.03), 0.957, 0.02);
    let pelvis = Vec3::new(sway, pelvis_y, 0.0);
    // Spine counter-rotates against the pelvis; the head stays level and forward.
    let spine_rot = Quat::from_rotation_y(yaw * 0.3) * Quat::from_rotation_z(roll * 0.3);
    let chest_rot = Quat::from_rotation_y(-yaw * 0.9)
        * Quat::from_rotation_z(-roll * 0.4)
        * Quat::from_rotation_x(-0.035);
    let neck_rot = Quat::from_rotation_y(-yaw * 0.3) * Quat::from_rotation_x(0.04);
    let head_rot = Quat::from_rotation_x(0.0 + 0.012 * wave(2.0 * u));
    let spine = pelvis + pelvis_rot * (Human::SPINE - Human::PELVIS);
    let chest = spine + spine_rot * (Human::CHEST - Human::SPINE);
    let neck = chest + chest_rot * (Human::NECK - Human::CHEST);
    let head = neck + neck_rot * (Human::HEAD - Human::NECK);
    set(p, Human::PELVIS_I, pelvis, pelvis_rot);
    set(p, Human::SPINE_I, spine, spine_rot);
    set(p, Human::CHEST_I, chest, chest_rot);
    set(p, Human::NECK_I, neck, neck_rot);
    set(p, Human::HEAD_I, head, head_rot);
    for (k, (side, arm, leg, uu)) in [
        (-1.0f32, Human::ARM_L, Human::LEG_L, ul),
        (1.0, Human::ARM_R, Human::LEG_R, ur),
    ]
    .into_iter()
    .enumerate()
    {
        // Arms swing against the leg on the same side, elbows bending more going forward.
        let swing = -0.3 * wave(uu - 0.05);
        let spread: f32 = 0.13;
        let down = Vec3::new(side * spread.sin(), -spread.cos(), 0.0);
        let bend = 0.25 + 0.3 * (swing / 0.3).max(0.0);
        let shoulder = chest + chest_rot * (body::side_v(Human::SHOULDER, side) - Human::CHEST);
        let upper = chest_rot * (Quat::from_rotation_x(swing) * down);
        let elbow = shoulder + upper * Human::UPPER_ARM;
        let fore = chest_rot * (Quat::from_rotation_x(swing + bend) * down);
        let wrist = elbow + fore * Human::FOREARM;
        set(p, arm, shoulder, aim(shoulder, elbow));
        set(p, arm + 1, elbow, aim(elbow, wrist));
        set(p, arm + 2, wrist, aim(elbow, wrist));
        // Legs: IK from the hip to the planted (or swinging) ankle, knees forward.
        let (y, z, pitch) = feet[k];
        let hip = pelvis + pelvis_rot * (body::side_v(Human::HIP, side) - Human::PELVIS);
        let target = Vec3::new(side * foot_x, y, z);
        let (knee, ankle) = two_bone_ik(
            hip,
            target,
            Human::THIGH,
            Human::SHIN,
            Vec3::new(side * 0.1, 0.0, -1.0),
        );
        set(p, leg, hip, aim(hip, knee));
        set(p, leg + 1, knee, aim(knee, ankle));
        set(p, leg + 2, ankle, Quat::from_rotation_x(pitch));
    }
}

// ---------------------------------------------------------------------------------------
// The statue

/// A standing youth in marble after Michelangelo's David, written into a humanoid pose:
/// weight on the right leg, the left knee eased forward on a raised heel, hips and shoulders
/// tilted against each other, the right arm hanging, the left hand raised to the shoulder and
/// the head turned to the left. Returns the head bone's pose (model space).
pub fn statue_pose(p: &mut [BonePose]) -> (Vec3, Quat) {
    // The standing hip rides up and the pelvis sways over the standing foot.
    let pelvis_rot =
        Quat::from_rotation_y(-0.08) * Quat::from_rotation_z(0.07) * Quat::from_rotation_x(-0.03);
    let sway = 0.035;
    let reach = 0.995 * (Human::THIGH + Human::SHIN);
    let stand = Vec3::new(0.075, Human::ANKLE_HEIGHT, 0.01);
    let hip_off = pelvis_rot * (body::side_v(Human::HIP, 1.0) - Human::PELVIS);
    let (dx, dz) = (stand.x - (sway + hip_off.x), stand.z - hip_off.z);
    let flat = (reach * reach - dx * dx - dz * dz).max(0.0).sqrt();
    let pelvis = Vec3::new(sway, stand.y + flat - hip_off.y, 0.0);
    // The shoulders tilt against the hips; the head turns over the raised hand.
    let spine_rot = Quat::from_rotation_z(-0.03) * Quat::from_rotation_y(0.03);
    let chest_rot =
        Quat::from_rotation_y(0.06) * Quat::from_rotation_z(-0.07) * Quat::from_rotation_x(-0.02);
    let neck_rot = Quat::from_rotation_y(0.12) * Quat::from_rotation_z(0.03);
    let head_rot =
        Quat::from_rotation_y(0.42) * Quat::from_rotation_x(0.08) * Quat::from_rotation_z(0.05);
    let spine = pelvis + pelvis_rot * (Human::SPINE - Human::PELVIS);
    let chest = spine + spine_rot * (Human::CHEST - Human::SPINE);
    let neck = chest + chest_rot * (Human::NECK - Human::CHEST);
    let head = neck + neck_rot * (Human::HEAD - Human::NECK);
    set(p, Human::PELVIS_I, pelvis, pelvis_rot);
    set(p, Human::SPINE_I, spine, spine_rot);
    set(p, Human::CHEST_I, chest, chest_rot);
    set(p, Human::NECK_I, neck, neck_rot);
    set(p, Human::HEAD_I, head, head_rot);
    // Arms: upper arm, forearm and hand directions (model space). The raised hand bends back
    // at the wrist, its fingers resting toward the shoulder.
    for (side, arm, upper, fore, hand) in [
        (
            -1.0f32,
            Human::ARM_L,
            Vec3::new(-0.25, -0.9, -0.25),
            Vec3::new(0.3, 0.75, -0.45),
            Vec3::new(0.1, 0.85, 0.5),
        ),
        (
            1.0,
            Human::ARM_R,
            Vec3::new(0.12, -0.99, 0.02),
            Vec3::new(0.08, -0.95, -0.25),
            Vec3::new(0.08, -0.95, -0.25),
        ),
    ] {
        let shoulder = chest + chest_rot * (body::side_v(Human::SHOULDER, side) - Human::CHEST);
        let elbow = shoulder + upper.normalize() * Human::UPPER_ARM;
        let wrist = elbow + fore.normalize() * Human::FOREARM;
        set(p, arm, shoulder, aim(shoulder, elbow));
        set(p, arm + 1, elbow, aim(elbow, wrist));
        set(p, arm + 2, wrist, aim(wrist, wrist + hand));
    }
    // Legs: the standing one nearly straight, the free one forward and out on the ball of
    // its foot, toes turned out.
    let pitch = -0.3f32;
    let (sin, cos) = pitch.sin_cos();
    let free_y = Human::ANKLE_HEIGHT * cos - Human::BALL_FORWARD * sin;
    for (side, leg, target, foot) in [
        (
            -1.0f32,
            Human::LEG_L,
            Vec3::new(-0.15, free_y, -0.15),
            Quat::from_rotation_y(0.2) * Quat::from_rotation_x(pitch),
        ),
        (1.0, Human::LEG_R, stand, Quat::from_rotation_y(-0.12)),
    ] {
        let hip = pelvis + pelvis_rot * (body::side_v(Human::HIP, side) - Human::PELVIS);
        let (knee, ankle) = two_bone_ik(
            hip,
            target,
            Human::THIGH,
            Human::SHIN,
            Vec3::new(side * 0.1, 0.0, -1.0),
        );
        set(p, leg, hip, aim(hip, knee));
        set(p, leg + 1, knee, aim(knee, ankle));
        set(p, leg + 2, ankle, foot);
    }
    (head, head_rot)
}

// ---------------------------------------------------------------------------------------
// The trot

const DOG_STANCE: f32 = 0.42;

fn dog_trot(u: f32, stride: f32, p: &mut [BonePose]) {
    let wave = |x: f32| (x * TAU).cos();
    // Two beats per cycle: the body dips as each diagonal pair lands.
    let bob = 0.012 * wave(2.0 * u - 0.1);
    let roll = 0.025 * wave(u);
    let hips_rot = Quat::from_rotation_z(roll) * Quat::from_rotation_y(0.03 * wave(u + 0.25));
    let spine_rot = Quat::from_rotation_z(roll * 0.5);
    let chest_rot =
        Quat::from_rotation_z(-roll * 0.6) * Quat::from_rotation_y(-0.03 * wave(u + 0.25));
    let hips = Dog::HIPS + Vec3::Y * bob;
    let spine = hips + hips_rot * (Dog::SPINE - Dog::HIPS);
    let chest = spine + spine_rot * (Dog::CHEST - Dog::SPINE);
    let neck_rot = Quat::from_rotation_x(0.04 * wave(2.0 * u + 0.2));
    let neck = chest + chest_rot * (Dog::NECK - Dog::CHEST);
    let head_rot = Quat::from_rotation_y(0.1 * (u * TAU).sin())
        * Quat::from_rotation_x(-0.05 * wave(2.0 * u + 0.25));
    let head = neck + neck_rot * (Dog::HEAD - Dog::NECK);
    set(p, Dog::HIPS_I, hips, hips_rot);
    set(p, Dog::SPINE_I, spine, spine_rot);
    set(p, Dog::CHEST_I, chest, chest_rot);
    set(p, Dog::NECK_I, neck, neck_rot);
    set(p, Dog::HEAD_I, head, head_rot);
    // Tail carried up, wagging.
    let wag = (2.0 * u * TAU).sin();
    let mut t = hips + hips_rot * (Dog::TAIL[0] - Dog::HIPS);
    let mut dirs = Vec::with_capacity(3);
    for k in 0..3 {
        let rest = Dog::TAIL[k + 1] - Dog::TAIL[k];
        let lift = Quat::from_rotation_x(0.35 + 0.1 * k as f32);
        let side = Quat::from_rotation_y(0.35 * wag * (k as f32 + 1.0) / 2.0);
        dirs.push(hips_rot * side * lift * rest);
    }
    for (k, dir) in dirs.iter().enumerate() {
        let next = t + *dir;
        set(p, Dog::TAIL_I + k, t, aim(t, next));
        t = next;
    }
    // Legs: diagonal pairs move together; paws planted, pasterns folding in the swing.
    let travel = stride * DOG_STANCE;
    for (first, side, front, offset) in [
        (Dog::LEG_FL, -1.0f32, true, 0.0f32),
        (Dog::LEG_FR, 1.0, true, 0.5),
        (Dog::LEG_HL, -1.0, false, 0.5),
        (Dog::LEG_HR, 1.0, false, 0.0),
    ] {
        let joints = if front { Dog::FRONT } else { Dog::HIND };
        let j = joints.map(|q| body::side_v(q, side));
        let (parent, parent_rot, parent_bind) = if front {
            (chest, chest_rot, Dog::CHEST)
        } else {
            (hips, hips_rot, Dog::HIPS)
        };
        let top = parent + parent_rot * (j[0] - parent_bind);
        let lu = (u + offset).rem_euclid(1.0);
        // Front paws land a little ahead of the shoulder.
        let center = j[3].z + if front { -0.03 } else { 0.0 };
        let (paw_y, paw_z, fold) = if lu < DOG_STANCE {
            (
                Dog::PAW_HEIGHT,
                center - 0.5 * travel + travel * lu / DOG_STANCE,
                0.0,
            )
        } else {
            let w = (lu - DOG_STANCE) / (1.0 - DOG_STANCE);
            let m = stride * (1.0 - DOG_STANCE) * 0.5;
            let z = hermite(center + 0.5 * travel, center - 0.5 * travel, m, m, w);
            let arc = (std::f32::consts::PI * w).sin();
            let lift = if front { 0.07 } else { 0.06 };
            let fold = if front { 1.4 } else { 0.7 } * arc.powf(0.7);
            (Dog::PAW_HEIGHT + lift * arc, z, fold)
        };
        let paw = Vec3::new(j[3].x, paw_y, paw_z);
        let fold_rot = Quat::from_rotation_x(-fold);
        let wrist = paw - fold_rot * (j[3] - j[2]);
        let (l1, l2) = (j[0].distance(j[1]), j[1].distance(j[2]));
        let pole = if front { Vec3::Z } else { Vec3::NEG_Z };
        let (mid, wrist) = two_bone_ik(top, wrist, l1, l2, pole);
        let paw = wrist + fold_rot * (j[3] - j[2]);
        set(p, first, top, aim(top, mid));
        set(p, first + 1, mid, aim(mid, wrist));
        set(p, first + 2, wrist, aim(wrist, paw));
        set(p, first + 3, paw, fold_rot);
    }
}

// ---------------------------------------------------------------------------------------
// Spawning

fn fiberglass(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial {
        reflectance: 0.5,
        ..StandardMaterial::hex(hex)
            .with_roughness(roughness)
            .with_subsurface(0.12)
    }
}

/// Capsules fitted inside the mannequin's surface (cloth keeps its thickness off them).
pub fn mannequin_colliders() -> BoneColliders {
    let mut capsules = vec![
        BoneCapsule {
            offset: [0.0, 0.0, -0.005],
            ..BoneCapsule::capsule("pelvis", "chest", 0.08)
        },
        BoneCapsule::capsule("chest", "neck", 0.1),
        BoneCapsule::capsule("upper_arm_l", "upper_arm_r", 0.052),
        BoneCapsule::capsule("neck", "head", 0.046),
        // The cranium as two balls, and the face.
        BoneCapsule::sphere("head", 0.078, [0.0, 0.135, -0.012]),
        BoneCapsule::sphere("head", 0.078, [0.0, 0.128, 0.03]),
        BoneCapsule::sphere("head", 0.058, [0.0, 0.055, -0.03]),
        // Hips and glutes.
        BoneCapsule {
            offset: [0.0, 0.0, 0.035],
            ..BoneCapsule::capsule("thigh_l", "thigh_r", 0.088)
        },
    ];
    for side in ["l", "r"] {
        let b = |n: &str| format!("{n}_{side}");
        capsules.push(BoneCapsule::capsule("chest", &b("upper_arm"), 0.05));
        capsules.push(BoneCapsule::capsule(&b("upper_arm"), &b("forearm"), 0.034));
        capsules.push(BoneCapsule::capsule(&b("forearm"), &b("hand"), 0.027));
        capsules.push(BoneCapsule::sphere(&b("hand"), 0.034, [0.0, -0.06, 0.0]));
        capsules.push(BoneCapsule::capsule(&b("thigh"), &b("shin"), 0.054));
        capsules.push(BoneCapsule::capsule(&b("shin"), &b("foot"), 0.036));
    }
    BoneColliders {
        capsules,
        friction: 0.4,
    }
}

/// A mannequin with a cape and long hair walking around `center`. `time_scale` < 1 runs it
/// (and its cloth and hair) in slow motion. `finish` is its fiberglass color.
pub(super) fn mannequin(
    commands: &mut Commands,
    center: DVec3,
    radius: f32,
    time_scale: f32,
    name: &str,
    finish: StandardMaterial,
) -> Entity {
    let pose = skeleton(&HUMANOID, Rig::Humanoid);
    let cycle = 1.0;
    let speed = HUMAN_STRIDE / cycle;
    let mut root = commands.spawn((
        Name::new(name.to_string()),
        Transform::from_translation(center + DVec3::X * radius as f64),
        TransformInterpolation::default(),
        pose,
        mannequin_colliders(),
        DemoSkeleton {
            rig: Rig::Humanoid,
            cycle,
            stride: HUMAN_STRIDE,
        },
        DemoMotion::Circle {
            center,
            radius,
            period: TAU * radius / speed,
        },
        DemoClock::default(),
    ));
    if time_scale != 1.0 {
        root.insert(LocalTimeScale {
            factor: time_scale,
            ignore_world_dilation: false,
        });
    }
    let man = root.id();
    // Thick enough to hide the scalp: 560 guides, 48 drawn strands around each.
    let style = HairStyle {
        melanin: 0.8,
        redness: 0.2,
        strands_per_guide: 48,
        spread: 0.015,
        ..HairStyle::long()
    };
    // The cranium of the sculpt, in the head bone's frame, with a hair's breadth to spare.
    let cranium = Vec3::new(0.0, 1.737, 0.012) - Human::HEAD;
    let (radii, hairline) = (Vec3::new(0.079, 0.099, 0.103), 0.55);
    // The scalp under the hair takes the hair's color, a shade darker as the roots in their
    // own shadow, so the gaps between strands don't show white fiberglass.
    let [r, g, b] = style.color();
    let base = finish.base_color;
    let tint = [r / base[0], g / base[1], b / base[2]].map(|t| (t * 0.8).clamp(0.0, 1.0));
    commands.spawn((
        Name::new(format!("{name} body")),
        Transform::default(),
        SkinnedPart::new(BodyPart::Mannequin, man, &format!("{name} body"), finish).with_scalp(
            ScalpPaint {
                bone: Human::HEAD_I,
                center: cranium,
                radii,
                hairline,
                tint,
            },
        ),
        ChildOf(man),
    ));
    // Cape tied across the shoulders, hanging down the back.
    let cape = SoftMaterial::hex(0x922b3e)
        .with_pattern(Pattern::Border, 0xd4ac0d, 0xf4d03f)
        .with_roughness(0.85)
        .with_sheen(0.7);
    commands.spawn((
        Name::new(format!("{name} cape")),
        Transform::from_xyz(0.0, 1.475, 0.125),
        Cloth {
            pins: ClothPins::Bones {
                rows: 1,
                bones: vec!["chest".into()],
            },
            ..Cloth::cape([0.56, 1.22], cape, &["chest"])
        },
        ChildOf(man),
    ));
    commands.spawn((
        Name::new(format!("{name} hair")),
        Transform::default(),
        HairGroom {
            emitters: vec![HairEmitter::Scalp {
                bone: "head".into(),
                center: cranium.to_array(),
                radii: radii.to_array(),
                guides: 560,
                hairline,
            }],
            ..HairGroom::long_hair("head", style)
        },
        ChildOf(man),
    ));
    man
}

/// Turn a walking entity (the bridge walker) into a skinned mannequin in a traveling cloak:
/// a skeleton, a body, the cloak, and a walk cycle paced by its [`DemoStride`].
pub(super) fn bridge_walker_body(commands: &mut Commands, walker: Entity) {
    commands.entity(walker).insert((
        skeleton(&HUMANOID, Rig::Humanoid),
        mannequin_colliders(),
        DemoSkeleton {
            rig: Rig::Humanoid,
            cycle: 1.0,
            stride: HUMAN_STRIDE,
        },
    ));
    commands.spawn((
        Name::new("Bridge walker body"),
        Transform::default(),
        SkinnedPart::new(
            BodyPart::Mannequin,
            walker,
            "Bridge walker body",
            fiberglass(0xc8a27a, 0.72),
        ),
        ChildOf(walker),
    ));
    // A heavy wool cloak, long enough to swing about the knees as the deck sways.
    let wool = SoftMaterial::hex(0x2f4858)
        .with_pattern(Pattern::Border, 0x9c7a3c, 0xc9a35a)
        .with_roughness(0.95)
        .with_sheen(0.45);
    commands.spawn((
        Name::new("Bridge walker cloak"),
        Transform::from_xyz(0.0, 1.475, 0.125),
        Cloth {
            mass_per_m2: 0.5,
            ..Cloth::cape([0.6, 1.25], wool, &["chest"])
        },
        ChildOf(walker),
    ));
}

/// The fur's color (and the skin under it, a shade darker).
fn dog_coat() -> HairStyle {
    HairStyle {
        length: 0.042,
        length_variation: 0.3,
        segments: 3,
        strands_per_guide: 34,
        // Wide enough that the coat closes over skin that stretches around a bent joint.
        spread: 0.016,
        clumping: 0.12,
        curl: 0.0012,
        curl_frequency: 25.0,
        stiffness: 0.85,
        droop: 0.0,
        thickness: 0.00042,
        melanin: 0.3,
        redness: 0.75,
        tint: None,
        comb: 0.86,
    }
}

pub(super) fn dog(commands: &mut Commands, center: DVec3) {
    let pose = skeleton(&DOG, Rig::Dog);
    // Capsules inside the body, for the fur.
    let mut capsules = vec![
        BoneCapsule::capsule("hips", "chest", 0.085),
        BoneCapsule::capsule("neck", "head", 0.05),
        BoneCapsule::sphere("head", 0.055, [0.0, 0.04, -0.03]),
        BoneCapsule::sphere("head", 0.03, [0.0, 0.0, -0.15]),
        BoneCapsule::capsule("tail_1", "tail_3", 0.015),
    ];
    for leg in ["fl", "fr", "hl", "hr"] {
        let b = |n: &str| format!("{n}_{leg}");
        capsules.push(BoneCapsule::capsule(&b("leg"), &b("knee"), 0.03));
        capsules.push(BoneCapsule::capsule(&b("knee"), &b("ankle"), 0.018));
    }
    let radius = 2.0;
    let cycle = 0.48;
    let speed = DOG_STRIDE / cycle;
    let dog = commands
        .spawn((
            Name::new("Dog"),
            Transform::from_translation(center + DVec3::X * radius as f64),
            TransformInterpolation::default(),
            pose,
            BoneColliders {
                capsules,
                friction: 0.3,
            },
            DemoSkeleton {
                rig: Rig::Dog,
                cycle,
                stride: DOG_STRIDE,
            },
            DemoMotion::Circle {
                center,
                radius,
                period: TAU * radius / speed,
            },
            DemoClock::default(),
        ))
        .id();
    let coat = dog_coat();
    // Darker than the coat, so skin glimpsed between the hairs reads as the shade inside it.
    let skin = coat.color().map(|c| c * 0.5);
    let parts = [
        (
            BodyPart::Dog,
            "Dog body",
            StandardMaterial::color(skin[0], skin[1], skin[2]).with_roughness(0.8),
        ),
        (
            BodyPart::DogNose,
            "Dog nose",
            StandardMaterial::hex(0x1b1715).with_roughness(0.32),
        ),
        (
            BodyPart::DogEyes,
            "Dog eyes",
            StandardMaterial {
                reflectance: 0.7,
                ..StandardMaterial::hex(0x2a1608).with_roughness(0.04)
            },
        ),
    ];
    for (part, key, material) in parts {
        commands.spawn((
            Name::new(key),
            Transform::default(),
            SkinnedPart::new(part, dog, key, material),
            ChildOf(dog),
        ));
    }
    let zones = dog_fur_zones();
    let tail: Vec<String> = ["tail_1", "tail_2", "tail_3"].map(String::from).to_vec();
    let surface = |guides: u32, zones: Vec<FurZone>, bones: Vec<String>, skip: Vec<String>| {
        HairEmitter::Surface {
            mesh: "Dog body".into(),
            guides,
            flow_from: "head".into(),
            gravity: 0.3,
            zones,
            bones,
            skip_bones: skip,
        }
    };
    let fur_sim = SimSettings {
        substeps: 6,
        ..HairGroom::hair_settings()
    };
    commands.spawn((
        Name::new("Dog fur"),
        Transform::default(),
        HairGroom {
            emitters: vec![surface(5200, zones, Vec::new(), tail.clone())],
            style: coat,
            sim: fur_sim,
        },
        ChildOf(dog),
    ));
    commands.spawn((
        Name::new("Dog tail fur"),
        Transform::default(),
        HairGroom {
            emitters: vec![surface(
                2600,
                vec![FurZone::new("tail_3", [0.0, -0.08, 0.0], 0.1, 0.7)],
                tail,
                Vec::new(),
            )],
            style: HairStyle {
                length: 0.11,
                segments: 5,
                stiffness: 0.45,
                spread: 0.014,
                strands_per_guide: 36,
                clumping: 0.3,
                comb: 0.93,
                ..coat
            },
            sim: HairGroom::hair_settings(),
        },
        ChildOf(dog),
    ));
}

/// Where the dog's coat is longer or shorter than its base length: short on the muzzle, the
/// forehead and the paws, a ruff on the neck, feathering on the ears, brisket, belly and the
/// backs of the legs.
pub(crate) fn dog_fur_zones() -> Vec<FurZone> {
    let mut zones = vec![
        FurZone::new("head", [0.0, -0.01, -0.175], 0.11, 0.2),
        FurZone::new("head", [0.0, 0.045, -0.06], 0.07, 0.55),
        FurZone::new("head", [0.068, 0.0, -0.013], 0.06, 1.25),
        FurZone::new("head", [-0.068, 0.0, -0.013], 0.06, 1.25),
        FurZone::new("neck", [0.0, -0.07, -0.05], 0.15, 1.5),
        FurZone::new("chest", [0.0, -0.2, -0.02], 0.15, 1.6),
        FurZone::new("spine", [0.0, -0.12, 0.0], 0.12, 1.25),
    ];
    for leg in ["fl", "fr", "hl", "hr"] {
        zones.push(FurZone::new(
            &format!("paw_{leg}"),
            [0.0, 0.01, -0.01],
            0.05,
            0.5,
        ));
        zones.push(FurZone::new(
            &format!("ankle_{leg}"),
            [0.0, -0.03, 0.0],
            0.07,
            0.85,
        ));
        zones.push(FurZone::new(
            &format!("knee_{leg}"),
            [0.0, -0.1, 0.02],
            0.07,
            0.8,
        ));
    }
    for leg in ["hl", "hr"] {
        zones.push(FurZone::new(
            &format!("leg_{leg}"),
            [0.0, -0.08, 0.06],
            0.1,
            1.35,
        ));
    }
    zones
}

/// Procedural walk and trot cycles.
pub fn animate_skeletons(
    mut rigs: Query<(
        &DemoSkeleton,
        &DemoClock,
        Option<&DemoStride>,
        &mut SkeletonPose,
    )>,
) {
    for (rig, clock, walked, mut pose) in &mut rigs {
        let phase = match walked {
            Some(walked) => walked.0 / rig.stride.max(0.05) * TAU,
            None => clock.0 / rig.cycle.max(0.05) * TAU,
        };
        pose_rig_stride(rig.rig, phase, rig.stride, &mut pose);
    }
}

/// The ivory and graphite finishes of the two mannequins.
pub(super) fn ivory() -> StandardMaterial {
    fiberglass(0xe8dccb, 0.72)
}

pub(super) fn graphite() -> StandardMaterial {
    fiberglass(0x2a2c30, 0.74)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigs_have_valid_parents_and_keep_feet_near_the_ground() {
        for (bones, rig) in [(&HUMANOID[..], Rig::Humanoid), (&DOG[..], Rig::Dog)] {
            let mut pose = skeleton(bones, rig);
            for (i, p) in pose.parents.iter().enumerate() {
                assert!(p.is_none_or(|p| (p as usize) < i), "parent after child");
            }
            for k in 0..32 {
                pose_rig(rig, k as f32 / 32.0 * TAU, &mut pose);
                let lowest = pose
                    .bones
                    .iter()
                    .map(|b| b.translation.y)
                    .fold(f32::MAX, f32::min);
                assert!((-0.02..0.15).contains(&lowest), "{rig:?} lowest {lowest}");
                assert!(pose.bones.iter().all(|b| b.rotation.is_normalized()));
            }
        }
    }

    #[test]
    fn limbs_keep_their_bind_lengths_and_planted_feet_stay_put() {
        for (bones, rig, bind) in [
            (&HUMANOID[..], Rig::Humanoid, body::human_bind()),
            (&DOG[..], Rig::Dog, body::dog_bind()),
        ] {
            let mut pose = skeleton(bones, rig);
            for k in 0..64 {
                pose_rig(rig, k as f32 / 64.0 * TAU, &mut pose);
                for (i, parent) in pose.parents.iter().enumerate() {
                    let Some(pa) = parent.map(|p| p as usize) else {
                        continue;
                    };
                    let rest = bind[i].translation.distance(bind[pa].translation);
                    let now = pose.bones[i]
                        .translation
                        .distance(pose.bones[pa].translation);
                    assert!(
                        (rest - now).abs() < 0.004,
                        "{rig:?} {} {rest} → {now}",
                        pose.names[i]
                    );
                }
            }
        }
        // A planted foot moves backward at walking speed (so it stands still on the ground).
        let mut pose = skeleton(&HUMANOID, Rig::Humanoid);
        let foot = Human::LEG_R + 2;
        let du = 0.01;
        pose_rig(Rig::Humanoid, 0.3 * TAU, &mut pose);
        let a = pose.bones[foot].translation;
        pose_rig(Rig::Humanoid, (0.3 + du) * TAU, &mut pose);
        let b = pose.bones[foot].translation;
        assert!(((b.z - a.z) / du - HUMAN_STRIDE).abs() < 0.05, "{a} {b}");
        assert!(a.y < Human::ANKLE_HEIGHT + 0.005 && (a.y - b.y).abs() < 1e-3);
    }
}
