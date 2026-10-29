//! Hurt: where blows land, and what they leave behind once the flinch has passed.
//!
//! Head, body and the liver hurt differently, and blows add up. Each blow that lands (a
//! [`DamageEvent`]) is sorted by where it struck the skeleton: on the head (nearer the face
//! than either hand, above the neck), on the trunk between the hips and the chest, or on a
//! glove or arm (which hurts nothing). Its push fills that region's track, which drains over
//! time, as fighting games keep head, body and leg damage apart (EA's UFC).
//!
//! - **Rocked.** Past [`Hurt::rocked_at`] of `daze` the legs go to rubber: balance and gait go
//!   first after a concussive blow (the Association of Ringside Physicians' "anatomy of the
//!   knockout"). The hips sway on two slow sines that don't line up, the knees dip in
//!   bursts, the trunk and head loll against the sway and the hands drop; every second or so
//!   the body lurches a short way with the sway and the feet step to catch it
//!   ([`HitReaction::knock`]).
//! - **Winded.** A blow to the stomach or ribs fills `wind`: the trunk folds over the stomach,
//!   the knees give, the hands drop to cover the body and the face winces.
//! - **Liver.** A blow on the person's own right side below the chest, of at least
//!   [`Hurt::liver_above`] N·s, folds the body onto a knee a moment later
//!   ([`Hurt::liver_delay`]): the reaction to a liver shot is delayed while the nerves respond.
//!   The `kneel_hurt` clip takes it down onto the right knee clutching its side and back up.
//!
//! A rocked, winded or kneeling body can't hold a guard up ([`Hurt::guard_down`]); attackers
//! wait while it kneels ([`Hurt::is_down`]), as they do for a knockdown. People get a `Hurt`
//! when they are first hit; insert one to tune it.

use engine_core::prelude::*;
use engine_world::{DamageEvent, DamageKind};

use crate::animator::Animator;
use crate::hit::HitReaction;
use crate::ik::{to_model, two_bone};
use crate::knockdown::Knockdown;

/// How long `kneel_hurt` keeps the body off its feet, s (the clip is 4 s; it stands from
/// about 0.84 of it).
const KNEEL_SECS: f32 = 3.4;

/// Upper arm, forearm and hand, left and right.
const ARMS: [[&str; 3]; 2] = [
    ["upper_arm_l", "lower_arm_l", "hand_l"],
    ["upper_arm_r", "lower_arm_r", "hand_r"],
];

/// Where a blow landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HurtRegion {
    Head,
    Body,
    /// The body, on its own right side below the chest.
    Liver,
    /// A glove, an arm or a leg: nothing that hurts for long.
    Limb,
}

/// What blows have left a person with (see the module docs).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hurt {
    pub enabled: bool,
    /// Head damage, 0 and up: each blow adds its push over `daze_per`.
    pub daze: f32,
    pub daze_per: f32,
    /// Daze drained per second.
    pub daze_drain: f32,
    /// Daze past which the legs go.
    pub rocked_at: f32,
    /// Body damage: each blow adds its push over `wind_per`.
    pub wind: f32,
    pub wind_per: f32,
    pub wind_drain: f32,
    /// Wind past which the guard drops.
    pub winded_at: f32,
    /// Weakest blow on the liver that folds the body onto a knee, N·s.
    pub liver_above: f32,
    /// Seconds from a liver shot to the knee going down.
    pub liver_delay: f32,
    /// Seconds until a liver shot takes the knee down.
    pub liver: Option<f32>,
    /// Seconds left on the knee.
    pub kneel: f32,
    /// The rubber legs' clock, s.
    pub sway: f32,
    /// Seconds to the next lurch while rocked.
    pub lurch: f32,
    /// How many lurches so far (varies their timing).
    #[serde(skip)]
    lurches: u32,
}

impl Default for Hurt {
    fn default() -> Self {
        Self {
            enabled: true,
            daze: 0.0,
            daze_per: 120.0,
            daze_drain: 0.08,
            rocked_at: 0.6,
            wind: 0.0,
            wind_per: 60.0,
            wind_drain: 0.2,
            winded_at: 0.4,
            liver_above: 30.0,
            liver_delay: 0.45,
            liver: None,
            kneel: 0.0,
            sway: 0.0,
            lurch: 0.6,
            lurches: 0,
        }
    }
}

fn smooth(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Hurt {
    /// Take a blow pushing `push` N·s on `region`.
    pub fn take(&mut self, region: HurtRegion, push: f32) {
        if !self.enabled {
            return;
        }
        match region {
            HurtRegion::Head => self.daze += push / self.daze_per.max(1e-3),
            HurtRegion::Body => self.wind += push / self.wind_per.max(1e-3),
            HurtRegion::Liver => {
                self.wind += push / self.wind_per.max(1e-3);
                if push >= self.liver_above && self.liver.is_none() && self.kneel <= 0.0 {
                    self.liver = Some(self.liver_delay);
                }
            }
            HurtRegion::Limb => {}
        }
    }

    /// How rocked the body is (0..1): 0 at `rocked_at`, 1 by 0.3 past it.
    pub fn rocked(&self) -> f32 {
        smooth(self.rocked_at, self.rocked_at + 0.3, self.daze)
    }

    /// How winded the body is (0..1): doubled over by `winded_at`.
    pub fn winded(&self) -> f32 {
        smooth(self.winded_at * 0.35, self.winded_at, self.wind)
    }

    /// Down on a knee (attackers wait).
    pub fn is_down(&self) -> bool {
        self.kneel > 0.0
    }

    /// Too hurt to hold a guard up.
    pub fn guard_down(&self) -> bool {
        self.is_down()
            || self.liver.is_some()
            || self.wind > self.winded_at
            || self.daze > self.rocked_at
    }
}

/// Where on the body a blow at `point` (model space) landed, from the model-space bones of
/// `animator`'s pose.
pub fn region_of(animator: &Animator, point: Vec3) -> HurtRegion {
    let s = &animator.skeleton;
    let at = |name: &str| s.index_of(name).map(|b| animator.model[b].t);
    let (Some(hips), Some(chest)) = (at("hips"), at("chest")) else {
        return HurtRegion::Limb;
    };
    // A blow nearer a hand than the face landed on the glove.
    let face = at("head").map(|h| h + Vec3::Y * 0.07);
    let hand = ["hand_l", "hand_r"]
        .iter()
        .filter_map(|h| at(h))
        .map(|h| h.distance(point))
        .fold(f32::INFINITY, f32::min);
    let neck = at("neck").map_or(chest.y + 0.2, |n| n.y);
    if let Some(face) = face
        && point.y > neck - 0.02
    {
        return if face.distance(point) < hand && face.distance(point) < 0.3 {
            HurtRegion::Head
        } else {
            HurtRegion::Limb
        };
    }
    if hand < 0.08 || point.y < hips.y - 0.08 || point.y > chest.y + 0.14 {
        return HurtRegion::Limb;
    }
    // Off the trunk to the side: an arm.
    if (point - hips).with_y(0.0).length() > 0.35 {
        return HurtRegion::Limb;
    }
    // People face −Z, so their own right is +X.
    if point.x - hips.x > 0.04 && point.y < chest.y + 0.02 {
        HurtRegion::Liver
    } else {
        HurtRegion::Body
    }
}

/// Fill the hurt tracks from the blows that landed this frame.
pub(crate) fn take_hurts(
    mut commands: Commands,
    mut hits: MessageReader<DamageEvent>,
    mut q: Query<(
        &Animator,
        &GlobalTransform,
        Option<&mut Hurt>,
        Option<&Knockdown>,
    )>,
) {
    for hit in hits.read() {
        let Ok((animator, gt, hurt, down)) = q.get_mut(hit.target) else {
            continue;
        };
        if down.is_some() || !animator.posed() {
            continue;
        }
        let region = match region_of(animator, to_model(gt, hit.point)) {
            // A blade under the ribs is a wound (gameplay's), not the blow to the liver that
            // folds a body onto a knee.
            HurtRegion::Liver if hit.kind != DamageKind::Blunt => HurtRegion::Body,
            region => region,
        };
        let push = if hit.impulse.length_squared() > 1e-6 {
            hit.impulse.length()
        } else {
            hit.amount.max(0.0)
        };
        match hurt {
            Some(mut h) => h.take(region, push),
            None => {
                let mut h = Hurt::default();
                h.take(region, push);
                commands.entity(hit.target).insert(h);
            }
        }
    }
}

/// Drain the tracks, take liver-shot bodies down onto a knee, and bend rocked and winded
/// bodies (after the flinch, before IK).
#[allow(clippy::type_complexity)]
pub(crate) fn hurt_system(
    time: Res<Time>,
    mut q: Query<(
        Entity,
        &mut Hurt,
        &mut Animator,
        &GlobalTransform,
        Option<&mut HitReaction>,
        Option<&Knockdown>,
        Option<&LocalTimeScale>,
    )>,
) {
    for (entity, mut hurt, mut animator, gt, reaction, down, local) in &mut q {
        let hurt = &mut *hurt;
        let dt = if animator.frozen() > 0.0 {
            0.0
        } else {
            time.delta_for(local)
        };
        hurt.daze = (hurt.daze - hurt.daze_drain * dt).max(0.0);
        hurt.wind = (hurt.wind - hurt.wind_drain * dt).max(0.0);
        if down.is_some() {
            // Knocked down: the knockdown has the body; a liver shot's knee is moot.
            hurt.liver = None;
            hurt.kneel = 0.0;
            continue;
        }
        if let Some(left) = &mut hurt.liver {
            *left -= dt;
            if *left <= 0.0 {
                hurt.liver = None;
                hurt.kneel = KNEEL_SECS;
                animator.trigger("kneel_hurt");
            }
        }
        if hurt.kneel > 0.0 {
            // The clip has the body until it is up.
            hurt.kneel = (hurt.kneel - dt).max(0.0);
            continue;
        }
        let rocked = hurt.rocked();
        let winded = hurt.winded();
        if (rocked <= 1e-3 && winded <= 1e-3) || !animator.posed() {
            hurt.sway = 0.0;
            continue;
        }
        hurt.sway += dt;
        let t = hurt.sway;
        let tau = std::f32::consts::TAU;
        // Rubber legs: sideways sway on two sines that don't line up, the knees dipping.
        let side = 0.06 * (tau * 0.55 * t).sin() + 0.025 * (tau * 1.31 * t + 1.0).sin();
        let side_rate = 0.06 * tau * 0.55 * (tau * 0.55 * t).cos();
        let dip = 0.03 + 0.03 * (0.5 + 0.5 * (tau * 0.83 * t).sin());
        // Every second or so the body lurches with the sway and the feet catch it.
        if rocked > 0.3 && dt > 0.0 {
            hurt.lurch -= dt;
            if hurt.lurch <= 0.0 {
                hurt.lurches += 1;
                let jitter = ((entity.to_bits() as u32)
                    .wrapping_mul(7)
                    .wrapping_add(hurt.lurches * 13)
                    % 10) as f32
                    / 10.0;
                hurt.lurch = 0.8 + 0.6 * jitter;
                if let Some(mut r) = reaction
                    && r.stagger.0 == Vec3::ZERO
                {
                    let dir = Vec3::new(side_rate.signum(), 0.0, 0.35).normalize();
                    r.knock(gt.rotation() * dir * 0.14 * rocked, 0.3);
                }
            }
        }
        animator.refresh_model();
        let skeleton = animator.skeleton.clone();
        {
            let mut edit = animator.edit();
            let bone = |name: &str| skeleton.index_of(name);
            if let Some(hips) = bone("hips") {
                // Winded, the knees give. (Not back: foot IK would step the feet after the
                // hips, and leave them there when the wind comes back.)
                let shift = Vec3::new(side * rocked, -dip * rocked - 0.04 * winded, 0.0);
                edit.translate_model(hips, shift);
            }
            // Rotations about the model axes (people face −Z, +X is their right): −X folds
            // forward, +Z rolls to the left.
            let roll = (side / 0.06) * 6f32.to_radians() * rocked;
            let fold = |deg: f32| deg.to_radians() * winded;
            for (name, pitch, roll_share) in [
                ("spine", fold(-16.0), 1.0),
                ("chest", fold(-14.0), 0.6),
                ("neck", fold(-4.0), -0.8),
            ] {
                if let Some(b) = bone(name) {
                    edit.rotate_model(
                        b,
                        Quat::from_rotation_x(pitch) * Quat::from_rotation_z(roll * roll_share),
                    );
                }
            }
            // The head lolls, a beat behind the sway, and nods.
            if let Some(head) = bone("head") {
                let loll = (tau * 0.55 * t - 0.9).sin() * 9f32.to_radians() * rocked;
                let nod = ((tau * 0.37 * t).sin() * 5.0 - 4.0).to_radians() * rocked;
                edit.rotate_model(
                    head,
                    Quat::from_rotation_x(nod) * Quat::from_rotation_z(-loll),
                );
            }
            // The hands come down to cover the stomach when winded (from a guard or from
            // hanging), and part of the way when rocked.
            let cover = winded.max(rocked * 0.4);
            if cover > 1e-3
                && let Some(hips) = bone("hips")
            {
                let belly = edit.position(hips) + Vec3::new(0.0, 0.2, -0.17);
                for (n, x) in [(0usize, -1.0f32), (1, 1.0)] {
                    let [upper, lower, hand] = ARMS[n].map(bone);
                    let (Some(upper), Some(lower), Some(hand)) = (upper, lower, hand) else {
                        continue;
                    };
                    let (a, b, c) = (
                        edit.position(upper),
                        edit.position(lower),
                        edit.position(hand),
                    );
                    // Elbows where they were, nudged out and back so a hanging arm bends
                    // the right way.
                    let pole = b - (a + c) * 0.5 + Vec3::new(0.05 * x, 0.0, 0.05);
                    let target = belly + Vec3::new(0.08 * x, 0.0, 0.0);
                    two_bone(&mut edit, upper, lower, hand, target, pole, cover);
                }
            }
        }
        // The face screws up while winded.
        let brace = winded.max(rocked * 0.4);
        match animator.curves.iter_mut().find(|(n, _)| n == "brace") {
            Some((_, v)) => *v = v.max(brace),
            None => animator.curves.push(("brace".into(), brace)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blows_fill_their_region_and_a_liver_shot_waits_before_the_knee() {
        let mut h = Hurt::default();
        h.take(HurtRegion::Head, 60.0);
        assert!((h.daze - 0.5).abs() < 1e-4 && h.rocked() == 0.0);
        h.take(HurtRegion::Head, 30.0);
        assert!(h.rocked() > 0.0 && h.guard_down());
        h.take(HurtRegion::Limb, 90.0);
        assert!((h.daze - 0.75).abs() < 1e-4);
        let mut h = Hurt::default();
        h.take(HurtRegion::Liver, 20.0);
        assert!(h.liver.is_none() && h.wind > 0.3);
        h.take(HurtRegion::Liver, 40.0);
        assert_eq!(h.liver, Some(h.liver_delay));
        assert!(h.winded() > 0.99 && !h.is_down());
    }
}
