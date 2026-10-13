//! Four-legged walkers: a brown retriever and a bay horse (whose meshes are in `horse.rs`),
//! and their four-beat walk.
//!
//! Each leg is a three-bone chain (upper, lower, cannon) and a foot (paw, or pastern and
//! hoof). The upper bone starts high in the body where the shoulder blade turns, so the whole
//! leg swings like a strut the way an animal's does. The foot is planted on its footprint (a
//! hoof's fetlock sinks under the load); the cannon follows the leg's sweep and folds in the
//! swing; the upper two bones are solved by two-bone IK, elbows bending back and stifles
//! forward. The body is two rigid halves hinged at the loin, each turned to the trail, so the
//! spine bends on the curves and flexes with the stride (the horse's halves stay together).

use engine_core::math::{DVec3, PI, Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, StandardMaterial};

use super::super::{TrailWalker, trail_point, yaw};
use super::gait::{self, GROUND, Gait};
use super::rig::{Xf, aim_down, curve, soft_min, two_bone};
use super::shape::{
    Grid, Key, Loft, Paint, bump, ellipsoid, key, lerp, merged, mirror_x, rgb, smooth,
};
use super::textures::{Textures, albedo, normal};
use super::{Part, Parts};

pub const REAR: usize = 0;
pub const FRONT: usize = 1;
pub const NECK: usize = 2;
pub const HEAD: usize = 3;
pub const TAIL: [usize; 3] = [4, 5, 6];
/// Left, right.
pub const EAR: [usize; 2] = [7, 8];
/// First bone of leg `foot` (walking order: left fore, right hind, right fore, left hind):
/// upper, lower, cannon, foot.
pub const fn leg(foot: usize) -> usize {
    9 + 4 * foot
}
/// Each leg's pastern, from the fetlock to the top of the hoof.
pub const PASTERN: [usize; 4] = [25, 26, 27, 28];

/// Is foot `f` a fore foot, and which side is it on?
pub(super) fn foot_info(f: usize) -> (bool, f32) {
    match f {
        0 => (true, -1.0),
        1 => (false, 1.0),
        2 => (true, 1.0),
        _ => (false, -1.0),
    }
}

/// An animal's proportions (meters, radians) and how it moves. Rest frame: root on the
/// ground, −Z forward; the body's pivots sit `half` ahead (shoulders) and behind (hips).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Species {
    pub half: f32,
    /// Standing height of the shoulder and hip pivots.
    pub front_h: f32,
    pub rear_h: f32,
    /// Where along the body (rest Z) the two halves hinge.
    pub hinge: f32,
    /// Right foreleg's top joint (where the shoulder blade turns) from the front pivot, right
    /// hip joint from the rear pivot.
    pub shoulder: Vec3,
    pub hip: Vec3,
    /// Upper, lower and cannon bone lengths.
    pub fore: [f32; 3],
    pub hind: [f32; 3],
    /// How far the fore and hind legs lean forward from straight down in the rest pose
    /// (radians, the fore leg's upper bone usually leans back): the angle the shoulders and
    /// thighs are built at, about where they work in the walk.
    pub lean: [f32; 2],
    /// The foot bone's joint (fetlock) above the ground, and how far behind the footprint
    /// centre.
    pub foot_h: f32,
    pub foot_z: f32,
    /// Footprint centre to the toe (the foot breaks over it).
    pub toe: f32,
    /// The top of the hoof in the foot frame, which the pastern runs down to, and how far the
    /// pastern turns down about it under full load (radians).
    pub coronet: Vec3,
    pub sink: f32,
    /// How much the cannon follows the leg's sweep (fore, hind) and folds in the swing.
    pub cannon_k: [f32; 2],
    pub fold: [f32; 2],
    /// How high the feet lift in the swing (fore, hind).
    pub lift: [f32; 2],
    /// Neck root from the front pivot.
    pub neck_base: Vec3,
    pub neck_len: f32,
    /// Neck's rest angle above horizontal; the head's below horizontal.
    pub neck_up: f32,
    pub head_down: f32,
    /// Tail root from the rear pivot.
    pub tail_base: Vec3,
    pub tail_seg: [f32; 3],
    /// Tail's rest angle below horizontal and its curl per segment.
    pub tail_droop: f32,
    pub tail_curl: f32,
    pub tail_sway: f32,
    /// Right ear root on the head.
    pub ear: Vec3,
    /// Withers/croup bob, head nod and spine flex amplitudes.
    pub bob: f32,
    pub nod: f32,
    pub flex: f32,
}

impl Species {
    fn front_pivot(&self) -> Vec3 {
        Vec3::new(0.0, self.front_h, -self.half)
    }

    fn rear_pivot(&self) -> Vec3 {
        Vec3::new(0.0, self.rear_h, self.half)
    }

    /// The hinge's rest position: on the line between the pivots.
    fn hinge_rest(&self) -> Vec3 {
        let t = (self.half - self.hinge) / (2.0 * self.half);
        self.rear_pivot().lerp(self.front_pivot(), t)
    }
}

pub const DOG: Species = Species {
    half: 0.25,
    front_h: 0.43,
    rear_h: 0.44,
    hinge: 0.08,
    shoulder: Vec3::new(0.062, 0.05, -0.02),
    hip: Vec3::new(0.066, -0.01, 0.0),
    fore: [0.21, 0.2, 0.08],
    hind: [0.19, 0.2, 0.1],
    lean: [0.0, 0.0],
    foot_h: 0.045,
    foot_z: 0.016,
    toe: 0.035,
    coronet: Vec3::new(0.0, -0.03, -0.012),
    sink: 0.0,
    cannon_k: [0.75, 0.55],
    fold: [1.5, 0.7],
    lift: [0.07, 0.06],
    neck_base: Vec3::new(0.0, 0.08, -0.07),
    neck_len: 0.2,
    neck_up: 0.62,
    head_down: 0.18,
    tail_base: Vec3::new(0.0, 0.07, 0.1),
    tail_seg: [0.11, 0.11, 0.1],
    tail_droop: 0.35,
    tail_curl: -0.12,
    tail_sway: 0.32,
    ear: Vec3::new(0.05, 0.022, -0.035),
    bob: 0.008,
    nod: 0.05,
    flex: 0.04,
};

pub const HORSE: Species = Species {
    half: 0.525,
    front_h: 1.12,
    rear_h: 1.16,
    hinge: 0.0,
    shoulder: Vec3::new(0.15, 0.26, -0.07),
    hip: Vec3::new(0.17, 0.08, -0.02),
    fore: [0.52, 0.48, 0.27],
    hind: [0.41, 0.4, 0.33],
    lean: [-0.3, 0.66],
    foot_h: 0.2,
    foot_z: 0.09,
    toe: 0.07,
    coronet: Vec3::new(0.0, -0.115, -0.075),
    sink: 0.3,
    cannon_k: [0.85, 0.55],
    fold: [1.25, 0.55],
    lift: [0.13, 0.11],
    neck_base: Vec3::new(0.0, 0.24, -0.16),
    neck_len: 0.74,
    neck_up: 0.55,
    head_down: 0.95,
    tail_base: Vec3::new(0.0, 0.29, 0.28),
    tail_seg: [0.26, 0.34, 0.34],
    tail_droop: 1.05,
    tail_curl: 0.18,
    tail_sway: 0.1,
    ear: Vec3::new(0.055, 0.03, 0.01),
    bob: 0.015,
    nod: 0.08,
    flex: 0.0,
};

/// A foot `v` (0..1) through its stance on `(centre, heading)`: flat, then breaking over
/// the toe.
fn stance_foot(sp: &Species, (centre, heading): (Vec3, Vec3), v: f32) -> Xf {
    let flat = yaw(heading);
    let joint = centre + flat * Vec3::new(0.0, sp.foot_h, sp.foot_z);
    let pitch = -0.6 * smooth(0.72, 1.0, v);
    let pivot = Vec3::new(0.0, -sp.foot_h, -sp.foot_z - sp.toe);
    let at = joint + flat * pivot;
    let r = flat * Quat::from_rotation_x(pitch);
    Xf::new(r, at - r * pivot)
}

fn swing_foot(sp: &Species, from: Xf, to: Xf, w: f32, fore: bool) -> Xf {
    let i = if fore { 0 } else { 1 };
    let prog = curve(&[0.0, 0.1, 0.42, 0.84, 1.0], w);
    let lift = sp.lift[i] * curve(&[0.0, 0.8, 1.0, 0.5, 0.0], w).max(0.0);
    // The foot folds back under the leg, then reaches forward flat to land.
    let fold = if fore { 1.3 } else { 0.8 } * curve(&[0.0, 0.85, 0.95, 0.3, 0.0], w);
    let r = from.r.slerp(to.r, smooth(0.0, 1.0, w)) * Quat::from_rotation_x(-fold);
    Xf::new(r, from.t.lerp(to.t, prog) + Vec3::Y * lift)
}

/// The fetlock in the foot frame with the pastern turned down by `sink` about the coronet.
fn fetlock(sp: &Species, sink: f32) -> Vec3 {
    sp.coronet + Quat::from_rotation_x(sink) * -sp.coronet
}

/// Where the body halves are: (front, rear), both with their origin at the hinge.
fn halves(sp: &Species, on_trail: &dyn Fn(f32) -> Vec3, heights: [f32; 2], flex: f32) -> [Xf; 2] {
    let fg = on_trail(sp.half);
    let rg = on_trail(-sp.half);
    let hg = on_trail(-sp.hinge);
    let fp = fg + Vec3::Y * heights[0];
    let rp = rg + Vec3::Y * heights[1];
    let chord = fp - rp;
    let rest = (sp.front_h - sp.rear_h).atan2(2.0 * sp.half);
    let pitch = chord.y.atan2(Vec2::new(chord.x, chord.z).length()) - rest;
    let t = (sp.half - sp.hinge) / (2.0 * sp.half);
    let hinge = Vec3::new(hg.x, lerp(rp.y, fp.y, t), hg.z);
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or(Vec3::NEG_Z);
    let (df, dr) = if sp.flex == 0.0 {
        (flat(chord), flat(chord))
    } else {
        (flat(fg - hg), flat(hg - rg))
    };
    let pitch = Quat::from_rotation_x(pitch);
    [
        Xf::new(Quat::from_rotation_y(flex) * yaw(df) * pitch, hinge),
        Xf::new(Quat::from_rotation_y(-flex) * yaw(dr) * pitch, hinge),
    ]
}

/// Which way a body half faces, level.
fn flat_fwd(body: Xf) -> Vec3 {
    let f = body.r * Vec3::NEG_Z;
    Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::NEG_Z)
}

/// A leg's top joint in its body half's frame (origin at the hinge).
fn leg_joint(sp: &Species, fore: bool, side: f32) -> Vec3 {
    let (pivot, j) = if fore {
        (sp.front_pivot(), sp.shoulder)
    } else {
        (sp.rear_pivot(), sp.hip)
    };
    pivot + Vec3::new(side * j.x, j.y, j.z) - sp.hinge_rest()
}

/// Pose a quadruped walking the trail: every bone in the walker's root space.
pub fn pose(w: &TrailWalker, sp: &Species, out: &mut [Xf]) {
    let g = Gait::of(w);
    let c = gait::phase(w);
    let pc = PI * c;
    let (root_p, root_d) = gait::root(w);
    let inv = yaw(root_d).inverse();
    let local = |p: DVec3| inv * (p - root_p).as_vec3();
    let on_trail = |ahead: f32| {
        let (p, _) = trail_point(w.along + ahead as f64);
        local(w.origin + p + DVec3::new(0.0, GROUND, 0.0))
    };

    // Feet, and where each fetlock is (sinking under the load at mid-stance).
    let mut feet = [Xf::IDENTITY; 4];
    let mut joints = [Vec3::ZERO; 4];
    let mut planted = [false; 4];
    let mut swing = [0.0f32; 4];
    for f in 0..4u8 {
        let i = f as usize;
        let (fore, _) = foot_info(i);
        let (u, td) = gait::foot_cycle(w, f);
        let (fp, fd, _) = gait::footprint(w, f, td);
        let here = (local(fp), inv * fd);
        let mut sink = 0.0;
        if u < g.duty {
            let v = u / g.duty;
            feet[i] = stance_foot(sp, here, v);
            planted[i] = true;
            sink = sp.sink * (PI * v).sin().powf(0.7) * (1.0 - smooth(0.7, 0.95, v));
        } else {
            let (np, nd, _) = gait::footprint(w, f, td + gait::stride(w) as f64);
            let from = stance_foot(sp, here, 1.0);
            let to = stance_foot(sp, (local(np), inv * nd), 0.0);
            let ws = (u - g.duty) / (1.0 - g.duty);
            feet[i] = swing_foot(sp, from, to, ws, fore);
            swing[i] = ws;
        }
        joints[i] = feet[i].point(fetlock(sp, sink));
    }

    // Body: each end as high as it wants to be, lowered (smoothly) where a planted leg
    // couldn't otherwise reach its foot.
    let flex = sp.flex * (pc * 0.5).sin();
    let want = [
        sp.front_h - sp.bob * pc.cos(),
        sp.rear_h + sp.bob * pc.cos(),
    ];
    let mut heights = want;
    for _ in 0..3 {
        let body = halves(sp, &on_trail, heights, flex);
        let mut limit = [f32::MAX; 2];
        for i in 0..4 {
            if !planted[i] {
                continue;
            }
            let (fore, side) = foot_info(i);
            let (end, len) = if fore { (0, sp.fore) } else { (1, sp.hind) };
            let joint = body[end].point(leg_joint(sp, fore, side));
            let fwd = flat_fwd(body[end]);
            let knee = joints[i] + cannon_dir(sp, fore, joint, joints[i], 0.0, fwd) * len[2];
            let reach = (len[0] + len[1]) * 0.985;
            let horiz = Vec2::new(joint.x - knee.x, joint.z - knee.z).length();
            let top = knee.y + (reach * reach - horiz * horiz).max(0.0).sqrt();
            limit[end] = limit[end].min(heights[end] - (joint.y - top));
        }
        for e in 0..2 {
            heights[e] = soft_min(want[e], limit[e], 0.03);
        }
    }
    let [front, rear] = halves(sp, &on_trail, heights, flex);
    out[FRONT] = front;
    out[REAR] = rear;

    // Legs.
    for i in 0..4 {
        let (fore, side) = foot_info(i);
        let (body, len, pole_sign) = if fore {
            (front, sp.fore, 1.0)
        } else {
            (rear, sp.hind, -1.0)
        };
        let fwd = flat_fwd(body);
        let joint = body.point(leg_joint(sp, fore, side));
        let dir = cannon_dir(sp, fore, joint, joints[i], swing[i], fwd);
        let knee_want = joints[i] + dir * len[2];
        let pole = fwd * -pole_sign + Vec3::X * side * 0.05;
        let (elbow, knee) = two_bone(joint, knee_want, len[0], len[1], pole);
        // Out of reach (only ever in the swing): hang the foot from where the knee got.
        let shift = knee - knee_want;
        let mut foot = feet[i];
        foot.t += shift;
        let fetlock = joints[i] + shift;
        let b = leg(i);
        out[b] = Xf::new(aim_down(elbow - joint, fwd), joint);
        out[b + 1] = Xf::new(aim_down(knee - elbow, fwd), elbow);
        out[b + 2] = Xf::new(aim_down(fetlock - knee, fwd), knee);
        out[b + 3] = foot;
        out[PASTERN[i]] = Xf::new(aim_down(foot.point(sp.coronet) - fetlock, fwd), fetlock);
    }

    // Neck and head: the head nods with each fore footfall and keeps looking down the trail.
    let at_front = |v: Vec3| front.point(sp.front_pivot() - sp.hinge_rest() + v);
    let nod = -sp.nod * (pc - 0.4).cos();
    let neck_r = front.r * Quat::from_rotation_x(sp.neck_up + nod * 0.6);
    let neck = Xf::new(neck_r, at_front(sp.neck_base));
    out[NECK] = neck;
    let look = on_trail(sp.half * 3.0) - on_trail(sp.half * 2.0);
    let head_r = yaw(look) * Quat::from_rotation_x(-sp.head_down + nod * 0.5);
    out[HEAD] = Xf::new(head_r, neck.point(Vec3::new(0.0, 0.0, -sp.neck_len)));
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        let flop = 0.12 * (2.0 * pc - 0.8).sin();
        out[EAR[i]] = Xf::new(
            head_r * Quat::from_rotation_z(s * (0.08 + flop * 0.5)) * Quat::from_rotation_x(flop),
            out[HEAD].point(Vec3::new(s * sp.ear.x, sp.ear.y, sp.ear.z)),
        );
    }

    // Tail: sways once a stride, each segment a little behind the one before.
    let base = rear.point(sp.rear_pivot() - sp.hinge_rest() + sp.tail_base);
    let mut t = Xf::new(rear.r, base);
    for (k, len) in sp.tail_seg.iter().enumerate() {
        let sway = sp.tail_sway * (pc * 0.5 - 0.9 * k as f32).sin();
        // Rotating +Z (back) about X by a positive angle tips it down.
        let bend = if k == 0 { sp.tail_droop } else { sp.tail_curl };
        let r = t.r
            * Quat::from_rotation_y(sway * if k == 0 { 0.6 } else { 0.5 })
            * Quat::from_rotation_x(bend);
        t = Xf::new(r, t.t);
        out[TAIL[k]] = t;
        t = Xf::new(r, t.point(Vec3::new(0.0, 0.0, *len)));
    }
}

/// The rest pose the meshes are built in: standing square on level ground, each leg straight
/// and leaning by `lean`, neck, head and tail at their rest angles.
pub fn rest(sp: &Species, out: &mut [Xf]) {
    let hinge = sp.hinge_rest();
    out[FRONT] = Xf::new(Quat::IDENTITY, hinge);
    out[REAR] = out[FRONT];
    let neck = Xf::new(
        Quat::from_rotation_x(sp.neck_up),
        sp.front_pivot() + sp.neck_base,
    );
    out[NECK] = neck;
    let head_r = Quat::from_rotation_x(-sp.head_down);
    out[HEAD] = Xf::new(head_r, neck.point(Vec3::new(0.0, 0.0, -sp.neck_len)));
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        out[EAR[i]] = Xf::new(
            head_r * Quat::from_rotation_z(s * 0.08),
            out[HEAD].point(Vec3::new(s * sp.ear.x, sp.ear.y, sp.ear.z)),
        );
    }
    let mut t = Xf::new(Quat::IDENTITY, sp.rear_pivot() + sp.tail_base);
    for (k, len) in sp.tail_seg.iter().enumerate() {
        let bend = if k == 0 { sp.tail_droop } else { sp.tail_curl };
        t = Xf::new(t.r * Quat::from_rotation_x(bend), t.t);
        out[TAIL[k]] = t;
        t = Xf::new(t.r, t.point(Vec3::new(0.0, 0.0, *len)));
    }
    for i in 0..4 {
        let (fore, side) = foot_info(i);
        let (len, lean) = if fore {
            (sp.fore, sp.lean[0])
        } else {
            (sp.hind, sp.lean[1])
        };
        // Turning −Y about X by a positive angle tips it forward (−Z).
        let r = Quat::from_rotation_x(lean);
        let mut at = leg_joint(sp, fore, side) + hinge;
        let b = leg(i);
        for (k, l) in len.iter().enumerate() {
            out[b + k] = Xf::new(r, at);
            at += r * Vec3::NEG_Y * *l;
        }
        out[b + 3] = Xf::new(r, at);
        out[PASTERN[i]] = Xf::new(r * aim_down(sp.coronet, Vec3::NEG_Z), at);
    }
}

/// Unit vector up the cannon from its lower joint: following the leg's sweep from the body
/// joint to the foot, plus `swing`'s fold.
fn cannon_dir(sp: &Species, fore: bool, joint: Vec3, foot: Vec3, swing: f32, fwd: Vec3) -> Vec3 {
    let i = if fore { 0 } else { 1 };
    let d = joint - foot;
    let gamma = d.dot(fwd).atan2(d.y.max(0.05));
    let fold = sp.fold[i] * curve(&[0.0, 0.85, 1.0, 0.4, 0.0], swing);
    let beta = sp.cannon_k[i] * gamma + fold;
    (Vec3::Y * beta.cos() + fwd * beta.sin()).normalize()
}

// ---------------------------------------------------------------------------------------
// Meshes
// ---------------------------------------------------------------------------------------

/// Cross-section on a forward (−Z) spine: centre (0, y, z), half width, below, above.
pub(super) fn fwd(z: f32, y: f32, w: f32, below: f32, above: f32) -> Key {
    key(Vec3::new(0.0, y, z), w, below).q2(below, above)
}

/// Cross-section on a downward spine: half width, front and back radii.
pub(super) fn down(y: f32, w: f32, front: f32, back: f32) -> Key {
    key(Vec3::new(0.0, y, 0.0), w, back).q2(back, front)
}

/// A limb segment hanging from its joint: (y, half width, front, back) keys, domed ends.
pub(super) fn limb(keys: &[[f32; 4]], cols: usize, spacing: f32) -> Loft {
    let k: Vec<Key> = keys.iter().map(|&[y, w, f, b]| down(y, w, f, b)).collect();
    let top = keys[0][1].min(keys[0][2]);
    let end = keys[keys.len() - 1][1];
    Loft::new(k, cols, spacing).caps(top, end)
}

pub(super) fn mat(roughness: f32) -> StandardMaterial {
    StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(roughness)
}

pub(super) fn side_of(s: f32, m: Mesh) -> Mesh {
    if s < 0.0 { mirror_x(m) } else { m }
}

/// Alpha cutoff of fur shells and hair cards.
pub const CUT: f32 = 0.5;

/// How long the fur is at a point (given its normal) and which way it's combed.
type Fur = fn(Vec3, Vec3) -> (f32, Vec3);

/// Weight rising from 0 at `a` to 1 at `b` along a point's Y, Z.
pub(super) fn along_y(a: f32, b: f32) -> impl Fn(Vec3) -> f32 + Copy + 'static {
    move |p: Vec3| smooth(a, b, p.y)
}

pub(super) fn along_z(a: f32, b: f32) -> impl Fn(Vec3) -> f32 + Copy + 'static {
    move |p: Vec3| smooth(a, b, p.z)
}

/// Comb `dir` flat onto the surface with normal `n`.
fn comb(n: Vec3, dir: Vec3) -> Vec3 {
    (dir - n * n.dot(dir)).normalize_or_zero()
}

// ----- Dog ------------------------------------------------------------------------------

/// Brown retriever coat: darker along the back, lighter feathering underneath.
fn dog_coat(v: &Paint, root_y: f32) -> Vec4 {
    let base = rgb(0x5f3b22);
    let dark = rgb(0x3f2614);
    let light = rgb(0x80593a);
    let y = v.p.y + root_y;
    let mut c = base.lerp(dark, smooth(0.46, 0.58, y) * smooth(0.2, 0.9, v.n.y) * 0.8);
    c = c.lerp(light, smooth(0.2, -0.7, v.n.y) * 0.45);
    let n = (v.p.x * 37.0).sin() * (v.p.z * 23.0 + v.p.y * 19.0).sin();
    c * (1.0 + 0.05 * n)
}

/// The dog's trunk, tail to chest, in the rest frame.
fn dog_trunk() -> Loft {
    let keys = vec![
        fwd(0.385, 0.472, 0.045, 0.045, 0.035),
        fwd(0.34, 0.477, 0.082, 0.078, 0.06),
        fwd(0.26, 0.482, 0.1, 0.094, 0.072),
        fwd(0.15, 0.477, 0.09, 0.088, 0.071),
        fwd(0.05, 0.468, 0.097, 0.128, 0.08),
        fwd(-0.06, 0.462, 0.11, 0.162, 0.09),
        fwd(-0.17, 0.457, 0.113, 0.172, 0.104),
        fwd(-0.27, 0.452, 0.104, 0.162, 0.1),
        fwd(-0.34, 0.438, 0.085, 0.132, 0.085),
        fwd(-0.385, 0.423, 0.058, 0.088, 0.058),
    ]
    .into_iter()
    .map(|k| k.pow2(2.0, 2.3))
    .collect();
    Loft::new(keys, 72, 0.012).caps(0.04, 0.035)
}

fn dog_trunk_grid() -> Grid {
    dog_trunk().grid().deform(|p, c, _, _| {
        // Shoulder blades either side of the withers, the ribs' spring, and the croup.
        let blade = bump(
            Vec3::new(p.x.abs() - 0.065, p.y - 0.53, p.z + 0.2),
            Vec3::new(0.03, 0.05, 0.06),
        );
        let hips = bump(
            Vec3::new(p.x.abs() - 0.085, p.y - 0.5, p.z - 0.27),
            Vec3::new(0.03, 0.05, 0.05),
        );
        let out = Vec3::new(p.x, p.y - c.y, 0.0).normalize_or(Vec3::X);
        p + out * (0.008 * blade + 0.006 * hips)
    })
}

fn dog_body_fur(p: Vec3, n: Vec3) -> (f32, Vec3) {
    let under = smooth(-0.2, -0.85, n.y);
    let chest = smooth(0.05, -0.2, p.z);
    let len = 0.011 + 0.004 * smooth(0.3, 0.9, n.y) + 0.022 * under * (0.35 + 0.65 * chest);
    (len, comb(n, Vec3::new(0.0, -0.45, 1.0)) * 1.4)
}

pub fn dog_parts(tex: &Textures) -> Vec<Part> {
    let sp = DOG;
    let fur = albedo(
        normal(mat(0.8).with_subsurface(0.3), tex.fur, 0.05, 0.6),
        tex.fur_albedo,
    );
    let mut shell = mat(0.85).with_subsurface(0.35).with_alpha_mask(CUT);
    if let Some(t) = tex.locks {
        shell = shell
            .with_base_color_texture(t)
            .with_uv_scale(1.0 / 0.07, 1.0 / 0.07);
    }
    let dark = mat(0.42);
    let mut parts = Parts::new("dog");
    let hinge = sp.hinge_rest();
    // The trunk: the chest follows the front half, the loin bends at the hinge and the
    // croup follows the rear half.
    parts.parent(REAR, along_z(-0.1, 0.1)).furry(
        FRONT,
        "trunk",
        || dog_trunk_grid().mesh(|v: &Paint| dog_coat(v, 0.0)),
        fur,
        shell,
        5,
        dog_body_fur,
        -hinge,
    );
    // Neck with its ruff, along −Z from the withers to the head.
    parts
        .parent(FRONT, along_z(-0.04, 0.06))
        .child(HEAD, along_z(-sp.neck_len + 0.04, -sp.neck_len - 0.03))
        .furry(
            NECK,
            "neck",
            || {
                Loft::new(
                    vec![
                        fwd(0.08, -0.02, 0.085, 0.1, 0.068),
                        fwd(0.0, 0.0, 0.08, 0.09, 0.064),
                        fwd(-0.08, 0.0, 0.07, 0.076, 0.058),
                        fwd(-0.16, 0.0, 0.06, 0.064, 0.05),
                        fwd(-0.23, 0.005, 0.05, 0.055, 0.045),
                    ],
                    56,
                    0.012,
                )
                .caps(0.06, 0.035)
                .mesh(|v: &Paint| dog_coat(v, 0.6))
            },
            fur,
            shell,
            5,
            |_, n| {
                let ruff = smooth(0.0, -0.8, n.y);
                (0.015 + 0.012 * ruff, comb(n, Vec3::new(0.0, -0.35, 1.0)))
            },
            Vec3::ZERO,
        );
    parts.parent(NECK, along_z(-0.03, 0.03)).furry(
        HEAD,
        "head",
        dog_head,
        fur,
        shell,
        3,
        |p, n| {
            let skull = smooth(-0.125, -0.09, p.z) * smooth(-0.04, -0.01, p.y + 0.03);
            (0.006 * skull, comb(n, Vec3::new(0.0, -0.2, 1.0)))
        },
        Vec3::ZERO,
    );
    parts.add(HEAD, "nose", dog_nose, dark);
    parts.add(
        HEAD,
        "eyes",
        || {
            merged([-1.0f32, 1.0].map(|s| {
                ellipsoid(
                    Vec3::new(0.035 * s, 0.009, -0.116),
                    Vec3::splat(0.0105),
                    20,
                    |v: &Paint| {
                        // Dark brown iris, nearly black pupil, looking forward.
                        let f = v.n.dot(Vec3::new(0.35 * s, 0.05, -1.0).normalize());
                        rgb(0x2a160a).lerp(rgb(0x0a0605), smooth(0.85, 0.95, f))
                    },
                )
            }))
        },
        mat(0.08),
    );
    // Floppy ears, hanging from the top of the skull against the cheeks.
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        parts.parent(HEAD, along_y(-0.035, 0.01)).furry(
            EAR[i],
            &format!("ear{i}"),
            move || side_of(s, dog_ear()),
            fur,
            shell,
            4,
            move |p, n| {
                let fringe = smooth(-0.05, -0.1, p.y);
                (0.01 + 0.01 * fringe, comb(n, Vec3::NEG_Y))
            },
            Vec3::ZERO,
        );
    }
    // Tail: thick at the root, feathered underneath, tapering to a point.
    let tails = [[0.03, 0.025], [0.025, 0.019], [0.019, 0.006]];
    for (k, [r0, r1]) in tails.into_iter().enumerate() {
        let len = sp.tail_seg[k];
        let feather = [0.6, 1.0, 0.8][k];
        let up = if k == 0 { REAR } else { TAIL[k - 1] };
        parts.parent(up, along_z(0.03, -0.03));
        if k < 2 {
            parts.child(TAIL[k + 1], along_z(len - 0.03, len + 0.03));
        }
        parts.furry(
            TAIL[k],
            &format!("tail{k}"),
            move || {
                Loft::new(
                    vec![
                        key(Vec3::new(0.0, 0.0, -0.012), r0, r0),
                        key(
                            Vec3::new(0.0, 0.0, len * 0.5),
                            lerp(r0, r1, 0.5),
                            lerp(r0, r1, 0.5),
                        ),
                        key(Vec3::new(0.0, 0.0, len + 0.012), r1, r1),
                    ],
                    32,
                    0.012,
                )
                .caps(r0, if k == 2 { 0.025 } else { r1 })
                .mesh(|v: &Paint| dog_coat(v, 0.5))
            },
            fur,
            shell,
            5,
            move |p, n| {
                let under = smooth(0.1, -0.8, n.y);
                let tip = if k == 2 {
                    smooth(len + 0.03, len * 0.3, p.z)
                } else {
                    1.0
                };
                (
                    (0.012 + 0.04 * feather * under) * tip,
                    comb(n, Vec3::new(0.0, -0.7, 1.0)),
                )
            },
            Vec3::ZERO,
        );
    }
    // Legs.
    for f in 0..4 {
        let (fore, s) = foot_info(f);
        let b = leg(f);
        let lens = if fore { sp.fore } else { sp.hind };
        let segs: [Vec<[f32; 4]>; 3] = if fore {
            [
                vec![
                    [0.0, 0.034, 0.048, 0.048],
                    [-0.1, 0.042, 0.054, 0.06],
                    [-0.17, 0.039, 0.045, 0.054],
                    [-lens[0], 0.034, 0.034, 0.044],
                ],
                vec![
                    [0.01, 0.032, 0.032, 0.04],
                    [-0.04, 0.03, 0.031, 0.032],
                    [-0.14, 0.023, 0.023, 0.021],
                    [-lens[1], 0.023, 0.024, 0.024],
                ],
                vec![[0.0, 0.023, 0.024, 0.024], [-lens[2], 0.021, 0.021, 0.021]],
            ]
        } else {
            [
                vec![
                    [0.03, 0.044, 0.068, 0.074],
                    [-0.06, 0.052, 0.074, 0.08],
                    [-0.14, 0.044, 0.055, 0.06],
                    [-lens[0], 0.034, 0.036, 0.04],
                ],
                vec![
                    [0.0, 0.034, 0.036, 0.04],
                    [-0.05, 0.032, 0.03, 0.047],
                    [-0.15, 0.022, 0.019, 0.028],
                    [-lens[1], 0.021, 0.019, 0.03],
                ],
                vec![[0.0, 0.021, 0.019, 0.028], [-lens[2], 0.02, 0.019, 0.02]],
            ]
        };
        for (k, seg) in segs.into_iter().enumerate() {
            let root_y = if fore { sp.front_h } else { sp.rear_h } - [0.0, 0.2, 0.37][k];
            let mesh = move || {
                side_of(
                    s,
                    limb(&seg, 32, 0.01).mesh(|v: &Paint| dog_coat(v, root_y)),
                )
            };
            let name = format!("leg{f}_{k}");
            let feather: Fur = match (fore, k) {
                // Hidden in the chest but for the elbow.
                (true, 0) => |_, n| (0.012, comb(n, Vec3::NEG_Y)),
                (true, 1) => |p, n| {
                    let back = smooth(0.1, 0.8, n.z) * smooth(-0.17, -0.02, p.y);
                    (0.005 + 0.022 * back, comb(n, Vec3::new(0.0, -1.0, 0.35)))
                },
                (false, 0) => |_, n| {
                    let back = smooth(0.0, 0.8, n.z);
                    (0.012 + 0.016 * back, comb(n, Vec3::new(0.0, -1.0, 0.3)))
                },
                (false, 1) => |p, n| {
                    let back = smooth(0.2, 0.8, n.z) * smooth(-0.13, -0.03, p.y);
                    (0.005 + 0.012 * back, comb(n, Vec3::new(0.0, -1.0, 0.3)))
                },
                _ => |_, n| (0.006, comb(n, Vec3::NEG_Y)),
            };
            // The top of the leg stays with the body; the knees, hocks and wrists bend
            // smoothly.
            if k == 0 {
                let body = if fore { FRONT } else { REAR };
                parts.parent(body, along_y(if fore { -0.12 } else { -0.1 }, 0.0));
            } else {
                parts.parent(b + k - 1, along_y(-0.03, 0.03));
            }
            let end = [0.035, 0.025, 0.02][k];
            parts.child(b + k + 1, along_y(-lens[k] + end, -lens[k] - end));
            parts.furry(b + k, &name, mesh, fur, shell, 4, feather, Vec3::ZERO);
        }
        parts.parent(b + 2, along_y(-0.02, 0.02)).furry(
            b + 3,
            &format!("paw{f}"),
            move || side_of(s, paw()),
            fur,
            shell,
            3,
            move |p, n| {
                // Short hair over the toes, none on the pads.
                (
                    0.005 * smooth(-DOG.foot_h + 0.008, -DOG.foot_h + 0.016, p.y),
                    comb(n, Vec3::NEG_Y),
                )
            },
            Vec3::ZERO,
        );
    }
    parts.list
}

fn dog_head() -> Mesh {
    // Skull to nose along −Z (head frame at the back of the skull): a broad skull, a clear
    // stop and a deep, square muzzle.
    let keys = vec![
        fwd(0.03, -0.01, 0.045, 0.042, 0.036),
        fwd(0.0, 0.0, 0.058, 0.054, 0.048),
        fwd(-0.04, 0.004, 0.064, 0.06, 0.05),
        fwd(-0.08, 0.0, 0.061, 0.064, 0.046),
        fwd(-0.106, -0.008, 0.053, 0.062, 0.036),
        fwd(-0.13, -0.021, 0.045, 0.053, 0.024),
        fwd(-0.165, -0.026, 0.042, 0.05, 0.022),
        fwd(-0.195, -0.028, 0.039, 0.047, 0.021),
        fwd(-0.215, -0.03, 0.034, 0.041, 0.019),
    ]
    .into_iter()
    .map(|k| k.pow2(2.5, 2.4))
    .collect();
    Loft::new(keys, 72, 0.006)
        .caps(0.035, 0.012)
        .grid()
        .deform(|p, _, _, _| {
            // Brows over the eyes, a groove up the forehead, cheeks, and the upper lips
            // (flews) hanging over the lower jaw.
            let brow = bump(
                Vec3::new(p.x.abs() - 0.03, p.y - 0.03, p.z + 0.105),
                Vec3::new(0.016, 0.009, 0.014),
            );
            let groove = bump(
                Vec3::new(p.x, p.y - 0.045, p.z + 0.07),
                Vec3::new(0.008, 0.02, 0.04),
            );
            let cheek = bump(
                Vec3::new(p.x.abs() - 0.05, p.y + 0.02, p.z + 0.07),
                Vec3::new(0.015, 0.025, 0.03),
            );
            let flews = bump(
                Vec3::new(p.x.abs() - 0.034, p.y + 0.05, p.z + 0.18),
                Vec3::new(0.01, 0.014, 0.05),
            );
            let jaw = smooth(-0.045, -0.06, p.y) * smooth(-0.1, -0.14, p.z);
            p + Vec3::new(
                p.x.signum() * (0.004 * cheek + 0.003 * flews - 0.008 * jaw),
                0.006 * brow - 0.004 * groove - 0.006 * flews,
                0.0,
            )
        })
        .mesh(|v: &Paint| {
            let mut c = dog_coat(v, 0.66);
            // Dark lips along the mouth line, a slightly darker muzzle.
            let line = -0.05 + 0.012 * smooth(-0.11, -0.22, v.p.z);
            let mouth = smooth(0.005, 0.0, (v.p.y - line).abs()) * smooth(-0.1, -0.13, v.p.z);
            c *= 1.0 - 0.18 * smooth(-0.14, -0.2, v.p.z);
            c.lerp(rgb(0x1e1512), mouth)
        })
}

fn dog_nose() -> Mesh {
    // The nose leather: a rounded wedge, broad on top, with comma-shaped nostrils either
    // side of a groove down the middle.
    let nostril = |p: Vec3| {
        bump(
            Vec3::new(p.x.abs() - 0.0095, p.y + 0.02, p.z + 0.236),
            Vec3::new(0.0045, 0.0035, 0.006),
        )
    };
    Loft::new(
        vec![
            fwd(-0.198, -0.018, 0.019, 0.015, 0.013).pow2(2.4, 2.8),
            fwd(-0.222, -0.017, 0.021, 0.016, 0.015).pow2(2.4, 2.8),
            fwd(-0.233, -0.018, 0.018, 0.013, 0.012).pow2(2.4, 2.6),
        ],
        40,
        0.004,
    )
    .caps(0.004, 0.006)
    .grid()
    .deform(move |p, _, _, _| {
        let groove = bump(
            Vec3::new(p.x, p.y + 0.03, p.z + 0.237),
            Vec3::new(0.0016, 0.008, 0.006),
        );
        p + Vec3::new(0.0, 0.0, 0.005 * nostril(p) + 0.003 * groove)
    })
    .mesh(move |v: &Paint| rgb(0x231915) * (1.0 - 0.8 * nostril(v.p)))
}

/// The right ear: a soft flap hanging down the cheek.
fn dog_ear() -> Mesh {
    Loft::new(
        vec![
            down(0.012, 0.012, 0.024, 0.028),
            down(-0.018, 0.011, 0.038, 0.04),
            down(-0.062, 0.009, 0.042, 0.04),
            down(-0.1, 0.007, 0.032, 0.028),
            down(-0.118, 0.005, 0.014, 0.012),
        ],
        36,
        0.008,
    )
    .caps(0.006, 0.008)
    .grid()
    .deform(|p, _, _, _| {
        // Lie against the cheek, the lower edge curling under a little.
        let d = smooth(0.0, -0.12, p.y);
        Vec3::new(p.x + 0.012 * d - 0.005 * d * d, p.y, p.z + 0.008 * d)
    })
    .mesh(|v: &Paint| dog_coat(v, 0.6) * 0.92)
}

/// A paw under its joint: an oval with four toes and dark pads.
fn paw() -> Mesh {
    let h = DOG.foot_h;
    let pad = move |v: &Paint| {
        let pad = smooth(-h + 0.006, -h + 0.002, v.p.y);
        dog_coat(v, h).lerp(rgb(0x2a2220), pad)
    };
    let body = Loft::new(
        vec![
            fwd(0.012, -h * 0.5, 0.021, 0.022, 0.02),
            fwd(-0.01, -h * 0.58, 0.029, 0.026, 0.024),
            fwd(-0.035, -h * 0.68, 0.031, 0.019, 0.017),
            fwd(-0.05, -h * 0.72, 0.026, 0.013, 0.011),
        ],
        40,
        0.006,
    )
    .caps(0.012, 0.012)
    .grid()
    .deform(move |p, _, _, _| Vec3::new(p.x, p.y.max(-h + 0.002), p.z))
    .mesh(pad);
    let toes = (0..4).map(|i| {
        let x = (i as f32 - 1.5) * 0.0145;
        let z = -0.045 - 0.006 * (1.5 - (i as f32 - 1.5).abs());
        ellipsoid(
            Vec3::new(x, -h + 0.012, z),
            Vec3::new(0.0085, 0.011, 0.012),
            16,
            pad,
        )
    });
    merged(std::iter::once(body).chain(toes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::ContactKind;

    fn step(w: &mut TrailWalker, dx: f32) {
        w.along += dx as f64;
        w.to_next_step -= dx;
        while w.to_next_step <= 0.0 {
            w.to_next_step += w.step_len;
            w.next_foot = (w.next_foot + 1) % w.feet;
        }
    }

    #[test]
    fn planted_feet_stay_put_and_the_body_stays_up() {
        for (sp, kind, speed) in [
            (DOG, ContactKind::Paw, 1.8),
            (HORSE, ContactKind::Hoof, 1.7),
        ] {
            let mut w = TrailWalker::new(DVec3::new(3.0, 0.0, 1.0), 33.0, kind, 300.0, speed);
            let g = Gait::of(&w);
            let mut out = [Xf::IDENTITY; 32];
            let mut prev: [Option<DVec3>; 4] = [None; 4];
            let (mut slide, mut low, mut sag) = (0.0f64, 0.0f32, [0.0f32; 2]);
            let mut flex = [0.0f32; 2];
            let dx = 0.005;
            for _ in 0..(3.0 * gait::stride(&w) / dx) as usize {
                step(&mut w, dx);
                pose(&w, &sp, &mut out);
                let (rp, rd) = gait::root(&w);
                let rot = yaw(rd);
                let world = |p: Vec3| rp + (rot * p).as_dvec3();
                for i in 0..4 {
                    let (fore, _) = foot_info(i);
                    let foot = out[leg(i) + 3];
                    let toe = foot.point(Vec3::new(0.0, -sp.foot_h, -sp.foot_z - sp.toe));
                    let heel = foot.point(Vec3::new(0.0, -sp.foot_h, -sp.foot_z + sp.toe));
                    low = low.min(toe.y.min(heel.y));
                    let (u, _) = gait::foot_cycle(&w, i as u8);
                    if u < g.duty {
                        let p = world(toe);
                        if let Some(q) = prev[i] {
                            slide = slide.max(p.distance(q));
                        }
                        prev[i] = Some(p);
                        // How bent the elbow or stifle is at mid-stance.
                        if (u / g.duty - 0.5).abs() < 0.05 {
                            let b = leg(i);
                            let (j, e, k) = (out[b].t, out[b + 1].t, out[b + 2].t);
                            let bend = PI - (j - e).angle_between(k - e);
                            let f = if fore { 0 } else { 1 };
                            flex[f] = flex[f].max(bend);
                        }
                    } else {
                        prev[i] = None;
                    }
                }
                let hinge = sp.hinge_rest();
                let front = out[FRONT].point(sp.front_pivot() - hinge);
                let rear = out[REAR].point(sp.rear_pivot() - hinge);
                sag[0] = sag[0].max(sp.front_h + sp.bob - front.y);
                sag[1] = sag[1].max(sp.rear_h + sp.bob - rear.y);
            }
            eprintln!(
                "{kind:?}: slide {slide:.4}, lowest foot {low:.4}, sag front {:.3} rear {:.3}, \
                 mid-stance bend fore {:.0} hind {:.0} deg",
                sag[0],
                sag[1],
                flex[0].to_degrees(),
                flex[1].to_degrees()
            );
            assert!(slide < 0.002, "{kind:?}: a planted foot slid {slide} m");
            assert!(
                low > -0.004,
                "{kind:?}: a foot went {low} m into the ground"
            );
            assert!(
                sag[0] < 0.08 && sag[1] < 0.08,
                "{kind:?}: the body sagged {sag:?}"
            );
        }
    }
}
