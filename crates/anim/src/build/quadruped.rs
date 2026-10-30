//! Action clips for four-legged animals: key stances of the body, neck, head and tail, with
//! each leg either left to the gait (planted where it stands) or taken off it and folded or
//! raised by IK to a joint position. A leg taken off the gait gets its clip's `release` curve
//! at 1 ([`crate::procedural::gait::GaitLeg::release`]), so the gait hands it the clip's pose;
//! as the curve falls the gait brings it back onto the foot it left planted.

use engine_core::prelude::*;

use super::{PoseBuilder, keyed};
use crate::clip::AnimationClip;
use crate::skeleton::Skeleton;
use crate::xform::{Xf, rotation_between};

/// What an animal's action clips move: the leg chains (top to bottom, ending
/// `[upper, lower, cannon, end]`; a foreleg on a shoulder blade starts with it), in the gait's
/// order (front left, front right, hind left, hind right), their release curves, the neck
/// bones (base to head, the head last) and the tail's.
pub(crate) struct Rig {
    pub legs: [&'static [&'static str]; 4],
    pub release: [&'static str; 4],
    pub neck: &'static [&'static str],
    pub jaw: Option<&'static str>,
    pub tail: &'static [&'static str],
    pub loin: Option<&'static str>,
}

/// A leg in a [`Stance`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Leg {
    /// On the ground, where the gait planted it.
    Planted,
    /// Off the gait: the cannon's top (the knee or hock) at `joint` (model space, m), the
    /// cannon pointing along `cannon`, the end bone (hoof, paw) bent `end` degrees back from
    /// it, the joint above bending toward `pole`.
    Free {
        joint: Vec3,
        cannon: Vec3,
        end: f32,
        pole: Vec3,
    },
}

/// One key of an action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stance {
    /// Seconds into the clip.
    pub at: f32,
    /// The body bone: offset from rest (m), pitch (+ nose up), yaw, roll (degrees).
    pub body: Vec3,
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
    /// The loin's pitch against the body (dogs), degrees.
    pub loin: f32,
    /// Each neck bone's pitch (+ up) and yaw, degrees, base first; the head last.
    pub neck: [f32; 4],
    pub neck_yaw: [f32; 4],
    /// The head's roll, degrees.
    pub head_roll: f32,
    pub jaw: f32,
    /// The tail's lift (+ up) and swing, degrees per bone.
    pub tail: f32,
    pub tail_swing: f32,
    pub legs: [Leg; 4],
}

impl Stance {
    /// Standing as at rest.
    pub const REST: Stance = Stance {
        at: 0.0,
        body: Vec3::ZERO,
        pitch: 0.0,
        yaw: 0.0,
        roll: 0.0,
        loin: 0.0,
        neck: [0.0; 4],
        neck_yaw: [0.0; 4],
        head_roll: 0.0,
        jaw: 0.0,
        tail: 0.0,
        tail_swing: 0.0,
        legs: [Leg::Planted; 4],
    };

    pub fn at(self, at: f32) -> Stance {
        Stance { at, ..self }
    }
}

fn lerp(a: f32, b: f32, w: f32) -> f32 {
    a + (b - a) * w
}

fn lerp4(a: [f32; 4], b: [f32; 4], w: f32) -> [f32; 4] {
    std::array::from_fn(|i| lerp(a[i], b[i], w))
}

/// Where a leg's cannon starts and which way it points at rest (model space), and its pole.
fn rest_leg(s: &Skeleton, chain: &[&str]) -> (Vec3, Vec3) {
    let n = chain.len();
    let joint = s.rest_model[s.bone(chain[n - 2])].t;
    let end = s.rest_model[s.bone(chain[n - 1])].t;
    (joint, (end - joint).normalize())
}

/// The pose and the release weights at `t` seconds through `keys` (sorted), eased in and out
/// of each key; lengths in the keys are at scale 1 and multiplied by `scale`.
fn sample(
    rig: &Rig,
    s: &Skeleton,
    scale: f32,
    keys: &[Stance],
    t: f32,
) -> (Vec<(&'static str, Xf)>, [f32; 4]) {
    let i = keys
        .windows(2)
        .position(|w| t <= w[1].at)
        .unwrap_or(keys.len().saturating_sub(2));
    let (a, b) = (&keys[i], &keys[(i + 1).min(keys.len() - 1)]);
    let u = ((t - a.at) / (b.at - a.at).max(1e-6)).clamp(0.0, 1.0);
    let w = u * u * (3.0 - 2.0 * u);
    let mut p = PoseBuilder::new(s);
    p.rot_move(
        "body",
        a.body.lerp(b.body, w) * scale,
        lerp(a.pitch, b.pitch, w),
        lerp(a.yaw, b.yaw, w),
        lerp(a.roll, b.roll, w),
    );
    if let Some(loin) = rig.loin {
        p.rot(loin, lerp(a.loin, b.loin, w), 0.0, 0.0);
    }
    let (neck, yaw) = (lerp4(a.neck, b.neck, w), lerp4(a.neck_yaw, b.neck_yaw, w));
    let last = rig.neck.len() - 1;
    for (k, &bone) in rig.neck.iter().enumerate() {
        // The head takes the last entry whatever the neck's length.
        let j = if k == last { 3 } else { k.min(2) };
        let roll = if k == last {
            lerp(a.head_roll, b.head_roll, w)
        } else {
            0.0
        };
        p.rot(bone, neck[j], yaw[j], roll);
    }
    if let Some(jaw) = rig.jaw {
        p.rot(jaw, lerp(a.jaw, b.jaw, w), 0.0, 0.0);
    }
    let (lift, swing) = (lerp(a.tail, b.tail, w), lerp(a.tail_swing, b.tail_swing, w));
    for &bone in rig.tail {
        // The tail points back: lifting it turns it the other way from the neck.
        p.rot(bone, -lift, swing, 0.0);
    }
    let posed = p.take();
    let mut local = s.rest.clone();
    for (bone, x) in &posed {
        let k = s.bone(bone);
        local[k].r = x.r;
        if x.t != Vec3::ZERO {
            local[k].t = x.t;
        }
    }
    let mut model = s.rest_model.clone();
    s.to_model(&local, &mut model);
    let mut released = [0.0; 4];
    let mut edit = crate::animator::PoseEdit {
        skeleton: s,
        local: &mut local,
        model: &mut model,
    };
    for (leg, chain) in rig.legs.iter().enumerate() {
        let free = |l: &Leg| matches!(l, Leg::Free { .. });
        let (fa, fb) = (free(&a.legs[leg]), free(&b.legs[leg]));
        released[leg] = lerp(f32::from(u8::from(fa)), f32::from(u8::from(fb)), w);
        if !fa && !fb {
            continue;
        }
        let (rest_joint, rest_dir) = rest_leg(s, chain);
        let front = leg < 2;
        let rest_pole = if front { Vec3::Z } else { Vec3::NEG_Z };
        let parts = |l: &Leg| match *l {
            Leg::Free {
                joint,
                cannon,
                end,
                pole,
            } => (joint * scale, cannon.normalize(), end, pole),
            Leg::Planted => (rest_joint, rest_dir, 0.0, rest_pole),
        };
        let (ja, ca, ea, pa) = parts(&a.legs[leg]);
        let (jb, cb, eb, pb) = parts(&b.legs[leg]);
        let n = chain.len();
        let [upper, lower, cannon, end] =
            [chain[n - 4], chain[n - 3], chain[n - 2], chain[n - 1]].map(|b| s.bone(b));
        crate::ik::two_bone(
            &mut edit,
            upper,
            lower,
            cannon,
            ja.lerp(jb, w),
            pa.lerp(pb, w).normalize_or(rest_pole),
            1.0,
        );
        let dir = ca.lerp(cb, w).normalize_or(rest_dir);
        let turn = rotation_between(rest_dir, dir);
        edit.set_model_rotation(cannon, turn);
        // The end bends about the leg's lateral axis.
        let lateral = dir.cross(Vec3::Y).normalize_or(Vec3::X);
        let bend = lerp(ea, eb, w).to_radians();
        edit.set_model_rotation(end, Quat::from_axis_angle(lateral, -bend) * turn);
    }
    let mut out = posed;
    for chain in rig.legs {
        for &bone in chain {
            let k = s.bone(bone);
            out.retain(|(b, _)| *b != bone);
            out.push((
                bone,
                Xf {
                    t: Vec3::ZERO,
                    ..local[k]
                },
            ));
        }
    }
    (out, released)
}

/// An action clip through `keys` for an animal of `scale`, with each leg's release curve and a
/// `reach` curve (1 while the head reaches for what the action is about: water, grass, a
/// scent). A clip that moves the body is only right at its own size: name it with
/// [`sized_name`].
pub(crate) fn action_clip(
    rig: &Rig,
    s: &Skeleton,
    scale: f32,
    name: &str,
    keys: &[Stance],
    reach: &[(f32, f32)],
) -> AnimationClip {
    let secs = keys.last().map_or(1.0, |k| k.at).max(0.1);
    let samples = (secs * 30.0) as usize + 1;
    let mut clip = AnimationClip::from_fn(name, secs, samples, false, |ph| {
        sample(rig, s, scale, keys, ph * secs).0
    });
    for (leg, curve) in rig.release.iter().enumerate() {
        clip = clip.with_curve(curve, samples, |ph| {
            sample(rig, s, scale, keys, ph * secs).1[leg]
        });
    }
    clip.with_curve("reach", samples, |ph| keyed(ph * secs, reach))
}

/// An action's clip name for a skeleton: the action, with the size the skeleton's name carries
/// (`dog@35cm` → `dog_sit@35cm`; a full-size `dog` → `dog_sit`).
pub fn sized_name(action: &str, skeleton: &str) -> String {
    match skeleton.find('@') {
        Some(at) => format!("{action}{}", &skeleton[at..]),
        None => action.to_string(),
    }
}
