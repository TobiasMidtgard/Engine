//! Wounds that show: what the combat module's [`BodyInjuries`] leave a body able to do.
//!
//! - **A leg that can't hold.** A knee wounded to [`LEG_GIVES_OUT`] (a hamstring cut from
//!   behind goes there at once), or a leg cut off at the thigh, knee or shin, drops the body
//!   onto that knee ([`LEG_OUT`]): hips and legs from the clip, the upper body the graph's, so
//!   it still guards and strikes kneeling. Foot IK lets go of the floor meanwhile.
//! - **A locked knee** (`joint_locked`, from a blunt blow) bends at most [`STIFF_KNEE_DEG`]:
//!   the leg swings through stiff.
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
use engine_world::combat::{BodyInjuries, BodyZone};

use crate::animator::{AnimationLibrary, Animator};
use crate::build::humanoid::{ARMS_LIMP, LEG_OUT};
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
/// locked; 0 or 1.
pub fn wants(injuries: &BodyInjuries) -> [[f32; 2]; 3] {
    use BodyZone::*;
    let w = |z: BodyZone| injuries.get(z);
    let on = |b: bool| if b { 1.0 } else { 0.0 };
    let arms = [[UpperArmL, ForearmL, HandL], [UpperArmR, ForearmR, HandR]];
    let legs = [[ThighL, KneeL, ShinL], [ThighR, KneeR, ShinR]];
    let mut out = [[0.0; 2]; 3];
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
    }
    out
}

/// How far each wound shows on a body now (eased toward [`wants`]).
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct WoundPose {
    /// The arm hanging, the leg out, the knee locked, per side: 0..1.
    pub shown: [[f32; 2]; 3],
}

/// Ease the wounds in and pose them over the graph's pose (in `AnimSet::Procedural`, before
/// the hit reactions and the IK).
#[allow(clippy::type_complexity)]
pub(crate) fn wound_pose(
    mut commands: Commands,
    time: Res<Time>,
    library: Res<AnimationLibrary>,
    mut q: Query<(Entity, &BodyInjuries, &mut Animator, Option<&mut WoundPose>)>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    let step = (dt / EASE_SECS).min(1.0);
    for (entity, injuries, mut animator, pose) in &mut q {
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
        let skeleton = animator.skeleton.clone();
        let sample = |name: &str| -> Option<Vec<Xf>> {
            let clip = library.clip(name)?;
            let binding = ClipBinding::new(&clip, &skeleton);
            let mut out = skeleton.rest.clone();
            let t = now % clip.duration.max(1e-3);
            clip.sample(t, &binding, &mut out);
            Some(out)
        };
        let blend = |local: &mut [Xf], to: &[Xf], bone: &str, w: f32, with_t: bool| {
            if let Some(i) = skeleton.index_of(bone) {
                local[i].r = local[i].r.slerp(to[i].r, w).normalize();
                if with_t {
                    local[i].t = local[i].t.lerp(to[i].t, w);
                }
            }
        };
        // Down on the worse leg (both gone: the one that went first, the larger).
        let [left, right] = shown[1];
        let (side, out) = if right > left { (1, right) } else { (0, left) };
        if out > 0.0
            && let Some(kneel) = sample(LEG_OUT[side])
        {
            let local = &mut animator.local;
            blend(local, &kneel, "hips", out, true);
            for leg in LEG_BONES {
                for bone in leg {
                    blend(local, &kneel, bone, out, false);
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
        assert_eq!(wants(&inj), [[0.0; 2]; 3]);
        inj.get_mut(BodyZone::KneeL).wound = 0.7;
        inj.get_mut(BodyZone::ForearmR).wound = 0.65;
        inj.get_mut(BodyZone::KneeR).joint_locked = true;
        assert_eq!(wants(&inj), [[0.0, 1.0], [1.0, 0.0], [0.0, 1.0]]);
        // A leg cut off below the knee can't hold either; a locked knee on it doesn't matter.
        inj.get_mut(BodyZone::ShinR).severed = true;
        assert_eq!(wants(&inj)[1], [1.0, 1.0]);
        assert_eq!(wants(&inj)[2], [0.0, 0.0]);
        // Nothing left to hang from a shoulder that lost its arm.
        inj.get_mut(BodyZone::UpperArmR).severed = true;
        assert_eq!(wants(&inj)[0], [0.0, 0.0]);
        assert_eq!(severed_bone(BodyZone::Neck), Some("head"));
        assert_eq!(severed_bone(BodyZone::Belly), None);
    }
}
