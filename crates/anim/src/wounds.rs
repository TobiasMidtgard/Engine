//! Wounds that show: what the combat module's [`BodyInjuries`] leave a body able to do.
//!
//! - **A leg that can't hold.** A knee wounded to [`LEG_GIVES_OUT`] (a hamstring cut from
//!   behind goes there at once), or a leg cut off at the thigh, knee or shin, drops the body
//!   onto that knee ([`LEG_OUT`]): hips and legs from the clip, the upper body the graph's, so
//!   it still guards and strikes kneeling. Foot IK lets go of the floor meanwhile.
//!   Moving on like that, it drags itself along on the knee ([`LEG_DRAG`]), one hitch every
//!   [`LEG_DRAG_STRIDE`] it goes; how fast it goes is the mover's (NPC AI, Movement) to slow.
//!   With both legs out it kneels on both ([`KNEES_DOWN`]), and walks on its knees as it goes
//!   ([`KNEE_WALK`]).
//! - **A locked knee** (`joint_locked`, from a blunt blow) bends at most [`STIFF_KNEE_DEG`]:
//!   the leg swings through stiff.
//! - **A limp** (a locked knee, or a thigh or shin wounded to [`LEG_GIVES_OUT`]): walking, the
//!   body sinks onto the hurt leg while it holds and leans its trunk over it
//!   ([`LIMP_DIP`], [`LIMP_LEAN_DEG`]).
//! - **A ruined arm** (upper arm, forearm or hand wounded to [`ARM_HANGS`]) hangs dead from
//!   the shoulder ([`ARMS_LIMP`]). The combat module empties that hand's [`Loadout`] slot,
//!   so its props go too.
//! - **A severed part** is gone from its joint ([`severed_bone`]): the bone there and every
//!   bone below it shrink to nothing at the joint, on the logic skeleton and on a retargeted
//!   model alike, so the skin there closes to a point for the character module to cap.
//!
//! Each change eases in over [`EASE_SECS`] (a knee giving way drops the body that fast),
//! and out again if the wound is healed.
//!
//! [`Loadout`]: engine_world::combat::Loadout

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;
use engine_world::combat::{BodyInjuries, BodyZone};

use crate::animator::{AnimationLibrary, Animator};
use crate::build::humanoid::{
    ARMS_LIMP, KNEE_WALK, KNEE_WALK_STRIDE, KNEES_DOWN, LEG_DRAG, LEG_DRAG_STRIDE, LEG_OUT,
};
use crate::clip::ClipBinding;
use crate::xform::Xf;

/// A knee wound this bad can't hold the leg (the combat module's `rules::LEG_GIVES_OUT`).
pub const LEG_GIVES_OUT: f32 = 0.6;

/// An arm wound this bad leaves the arm hanging (the combat module's `rules::DISARMED_AT`).
pub const ARM_HANGS: f32 = 0.6;

/// How far a locked knee still bends, degrees.
pub const STIFF_KNEE_DEG: f32 = 12.0;

/// How long a change takes to show, s.
pub const EASE_SECS: f32 = 0.35;

/// How small a severed part shrinks (a scale of exactly 0 would make degenerate matrices).
pub const SEVERED_SCALE: f32 = 1e-3;

const ARM_BONES: [[&str; 3]; 2] = [
    ["upper_arm_l", "lower_arm_l", "hand_l"],
    ["upper_arm_r", "lower_arm_r", "hand_r"],
];
const LEG_BONES: [[&str; 4]; 2] = [
    ["upper_leg_l", "lower_leg_l", "foot_l", "toe_l"],
    ["upper_leg_r", "lower_leg_r", "foot_r", "toe_r"],
];

/// The bone a severed zone comes off at: that bone and everything below it go.
pub fn severed_bone(zone: BodyZone) -> Option<&'static str> {
    use BodyZone::*;
    Some(match zone {
        Head | Neck => "head",
        UpperArmL => "upper_arm_l",
        UpperArmR => "upper_arm_r",
        ForearmL => "lower_arm_l",
        ForearmR => "lower_arm_r",
        HandL => "hand_l",
        HandR => "hand_r",
        ThighL => "upper_leg_l",
        ThighR => "upper_leg_r",
        KneeL => "lower_leg_l",
        KneeR => "lower_leg_r",
        ShinL => "foot_l",
        ShinR => "foot_r",
        Chest | Belly => return None,
    })
}

/// What the wounds want of each side (left, right): the arm hanging, the leg out, the knee
/// locked, the leg limping (a locked knee, or a thigh or shin wounded to [`LEG_GIVES_OUT`]);
/// 0 or 1.
pub fn wants(injuries: &BodyInjuries) -> [[f32; 2]; 4] {
    use BodyZone::*;
    let w = |z: BodyZone| injuries.get(z);
    let on = |b: bool| if b { 1.0 } else { 0.0 };
    let arms = [[UpperArmL, ForearmL, HandL], [UpperArmR, ForearmR, HandR]];
    let legs = [[ThighL, KneeL, ShinL], [ThighR, KneeR, ShinR]];
    let mut out = [[0.0; 2]; 4];
    for side in 0..2 {
        let [upper, fore, hand] = arms[side];
        // An arm gone at the shoulder has nothing left to hang.
        out[0][side] = on(!w(upper).severed
            && [upper, fore, hand]
                .iter()
                .any(|z| w(*z).wound >= ARM_HANGS || w(*z).severed));
        let [thigh, knee, shin] = legs[side];
        let gone = [thigh, knee, shin].iter().any(|z| w(*z).severed);
        out[1][side] = on(gone || w(knee).wound >= LEG_GIVES_OUT);
        out[2][side] = on(w(knee).joint_locked && out[1][side] == 0.0);
        out[3][side] = on(out[1][side] == 0.0
            && (w(knee).joint_locked
                || w(thigh).wound >= LEG_GIVES_OUT
                || w(shin).wound >= LEG_GIVES_OUT));
    }
    out
}

/// How far each wound shows on a body now (eased toward [`wants`]).
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct WoundPose {
    /// The arm hanging, the leg out, the knee locked, the leg limping, per side: 0..1.
    pub shown: [[f32; 2]; 4],
    /// How far through a hitch of the knee drag it is (0..1), while it moves with a leg out,
    /// and through a pair of knee steps, with both out.
    pub drag: f32,
    pub knee_walk: f32,
}

/// How far a limping body sinks onto its hurt leg, m, and leans its trunk over it, degrees,
/// at walking pace.
pub const LIMP_DIP: f32 = 0.035;
pub const LIMP_LEAN_DEG: f32 = 7.0;

/// Moving this fast (m/s) or more with a leg out, the body drags itself along on the knee
/// fully; slower, it blends from kneeling still.
pub const DRAG_FULL_SPEED: f32 = 0.25;

/// Ease the wounds in and pose them over the graph's pose (in `AnimSet::Procedural`, before
/// the hit reactions and the IK).
#[allow(clippy::type_complexity)]
pub(crate) fn wound_pose(
    mut commands: Commands,
    time: Res<Time>,
    library: Res<AnimationLibrary>,
    mut q: Query<(
        Entity,
        &BodyInjuries,
        &mut Animator,
        Option<&mut WoundPose>,
        Option<&AnimationParams>,
    )>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    let step = (dt / EASE_SECS).min(1.0);
    for (entity, injuries, mut animator, pose, params) in &mut q {
        let want = wants(injuries);
        let Some(mut pose) = pose else {
            if want.iter().flatten().any(|w| *w > 0.0) {
                commands.entity(entity).try_insert(WoundPose::default());
            }
            continue;
        };
        for (shown, want) in pose.shown.iter_mut().flatten().zip(want.iter().flatten()) {
            *shown += (want - *shown).clamp(-step, step);
        }
        let shown = pose.shown;
        if !animator.posed() || shown.iter().flatten().all(|w| *w <= 0.0) {
            continue;
        }
        // Along the ground, on a leg out: hitches of the knee drag, as far as the body goes.
        let speed = params.map_or(0.0, |p| p.get("speed").max(0.0));
        let dragging = (speed / DRAG_FULL_SPEED).clamp(0.0, 1.0);
        let dragging = dragging * dragging * (3.0 - 2.0 * dragging);
        if shown[1].iter().any(|w| *w > 0.0) {
            pose.drag = (pose.drag + dt * speed / LEG_DRAG_STRIDE).fract();
            pose.knee_walk = (pose.knee_walk + dt * speed / KNEE_WALK_STRIDE).fract();
        }
        let (drag, knee_walk) = (pose.drag, pose.knee_walk);
        let skeleton = animator.skeleton.clone();
        // A clip at `phase` of it (None: running on the clock).
        let sample_at = |name: &str, phase: Option<f32>| -> Option<Vec<Xf>> {
            let clip = library.clip(name)?;
            let binding = ClipBinding::new(&clip, &skeleton);
            let mut out = skeleton.rest.clone();
            let length = clip.duration.max(1e-3);
            let t = phase.map_or(now % length, |p| p * length);
            clip.sample(t, &binding, &mut out);
            Some(out)
        };
        let sample = |name: &str| sample_at(name, None);
        let blend = |local: &mut [Xf], to: &[Xf], bone: &str, w: f32, with_t: bool| {
            if let Some(i) = skeleton.index_of(bone) {
                local[i].r = local[i].r.slerp(to[i].r, w).normalize();
                if with_t {
                    local[i].t = local[i].t.lerp(to[i].t, w);
                }
            }
        };
        // Down on the worse leg (both gone: the one that went first, the larger), and with
        // both out, on both knees.
        let [left, right] = shown[1];
        let (side, out) = if right > left { (1, right) } else { (0, left) };
        let both = left.min(right);
        if out > 0.0
            && let Some(kneel) = sample(LEG_OUT[side])
        {
            let moving = dragging > 0.0;
            let hauling = moving
                .then(|| sample_at(LEG_DRAG[side], Some(drag)))
                .flatten();
            let knees = (both > 0.0).then(|| sample(KNEES_DOWN)).flatten();
            let walking = (both > 0.0 && moving)
                .then(|| sample_at(KNEE_WALK, Some(knee_walk)))
                .flatten();
            let local = &mut animator.local;
            for (pose, w) in [
                (Some(kneel), out),
                (hauling, out * dragging),
                (knees, both),
                (walking, both * dragging),
            ] {
                let Some(pose) = pose else {
                    continue;
                };
                blend(local, &pose, "hips", w, true);
                for leg in LEG_BONES {
                    for bone in leg {
                        blend(local, &pose, bone, w, false);
                    }
                }
            }
            // Foot IK lets go of the floor while the knee is on it.
            match animator.curves.iter_mut().find(|c| c.0 == "floor") {
                Some(c) => c.1 = c.1.max(out),
                None => animator.curves.push(("floor".into(), out)),
            }
        }
        if (shown[0][0] > 0.0 || shown[0][1] > 0.0)
            && let Some(limp) = sample(ARMS_LIMP)
        {
            for side in 0..2 {
                let w = shown[0][side];
                if w > 0.0 {
                    for bone in ARM_BONES[side] {
                        blend(&mut animator.local, &limp, bone, w, false);
                    }
                }
            }
        }
        // A limp: while the hurt leg holds the body, it sinks onto it and leans the trunk
        // over it (taking the weight off the hip), and is quick to get off it again.
        let limping = (speed / 1.0).clamp(0.0, 1.0);
        if limping > 0.0 && shown[3].iter().any(|w| *w > 0.0) {
            animator.refresh_model();
            let feet = [LEG_BONES[0][2], LEG_BONES[1][2]].map(|b| skeleton.index_of(b));
            if let ([Some(fl), Some(fr)], Some(hips), Some(spine)) =
                (feet, skeleton.index_of("hips"), skeleton.index_of("spine"))
            {
                let y = [animator.model[fl].t.y, animator.model[fr].t.y];
                for side in 0..2 {
                    let w = shown[3][side] * limping;
                    // On it: that foot down while the other is up.
                    let on = ((y[1 - side] - y[side]) / 0.06).clamp(0.0, 1.0) * w;
                    if on <= 0.0 {
                        continue;
                    }
                    animator.local[hips].t.y -= LIMP_DIP * on;
                    // Lean over the hurt side: about the body's forward axis, in the spine's
                    // parent frame.
                    let parent = skeleton.parents[spine].map_or(hips, usize::from);
                    let forward = animator.model[parent].r.conjugate() * Vec3::NEG_Z;
                    // (A turn about −Z tips the trunk toward +X, the body's right.)
                    let toward = [-1.0, 1.0][side];
                    let lean =
                        Quat::from_axis_angle(forward, (LIMP_LEAN_DEG * toward * on).to_radians());
                    animator.local[spine].r = (lean * animator.local[spine].r).normalize();
                }
            }
        }
        for side in 0..2 {
            let w = shown[2][side];
            if w > 0.0
                && let Some(i) = skeleton.index_of(LEG_BONES[side][1])
            {
                let r = animator.local[i].r;
                let rest = skeleton.rest[i].r;
                // The bend from rest, held to the locked knee's few degrees.
                let bend = rest.conjugate() * r;
                let angle = bend.angle_between(Quat::IDENTITY).to_degrees();
                let most = STIFF_KNEE_DEG + (angle - STIFF_KNEE_DEG).max(0.0) * (1.0 - w);
                if angle > most {
                    let held = Quat::IDENTITY.slerp(bend, most / angle);
                    animator.local[i].r = (rest * held).normalize();
                }
            }
        }
        animator.refresh_model();
    }
}

/// Shrink severed parts to nothing at their joints (last, before the pose is published).
pub(crate) fn sever_parts(mut q: Query<(&BodyInjuries, &mut Animator)>) {
    for (injuries, mut animator) in &mut q {
        if !injuries.any_severed() || !animator.posed() {
            continue;
        }
        for zone in BodyZone::ALL {
            if !injuries.get(zone).severed {
                continue;
            }
            if let Some(i) = severed_bone(zone).and_then(|b| animator.skeleton.index_of(b)) {
                animator.local[i].s = Vec3::splat(SEVERED_SCALE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wounds_map_to_what_shows() {
        let mut inj = BodyInjuries::default();
        assert_eq!(wants(&inj), [[0.0; 2]; 4]);
        inj.get_mut(BodyZone::KneeL).wound = 0.7;
        inj.get_mut(BodyZone::ForearmR).wound = 0.65;
        inj.get_mut(BodyZone::KneeR).joint_locked = true;
        assert_eq!(
            wants(&inj),
            [[0.0, 1.0], [1.0, 0.0], [0.0, 1.0], [0.0, 1.0]]
        );
        // A leg cut off below the knee can't hold either; a locked knee on it doesn't matter.
        inj.get_mut(BodyZone::ShinR).severed = true;
        assert_eq!(wants(&inj)[1], [1.0, 1.0]);
        assert_eq!(wants(&inj)[2], [0.0, 0.0]);
        assert_eq!(wants(&inj)[3], [0.0, 0.0]);
        // Nothing left to hang from a shoulder that lost its arm.
        inj.get_mut(BodyZone::UpperArmR).severed = true;
        assert_eq!(wants(&inj)[0], [0.0, 0.0]);
        assert_eq!(severed_bone(BodyZone::Neck), Some("head"));
        assert_eq!(severed_bone(BodyZone::Belly), None);
    }
}
