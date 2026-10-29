//! Hit flinches: a person struck (a [`DamageEvent`] aimed at them) jerks away from the blow
//! and springs back, on top of whatever they are doing.
//!
//! The push at the hit point (the event's impulse, or its amount pushing from the point to
//! the body when it has none) turns the spine, neck and head the way the blow drives them:
//! each joint about the axis from it to the point crossed with the push, so a jab to the
//! left shoulder twists it back, a blow to the face snaps the head away and one to the
//! stomach folds the body over it. Bones nearer the point take more of it, an arm that was
//! hit swings with the blow, and a blow low on the body shoves the hips along it (one to
//! the head barely moves them); the knees give and the trunk leans against the shove to
//! keep the body over its planted feet. Each bone then settles on a
//! critically damped spring, and the face winces (the `brace` curve the [`Face`] reads) for
//! as long as it lasts. It runs before foot IK, so planted feet stay put.
//!
//! A hard blow (at least [`HitReaction::physical_above`] N·s) also rocks the upper body with
//! physics for a moment when a physics backend is installed: the parts from the spine up
//! simulate, driven toward the flinching pose, take part of the blow at the point it landed
//! and show up to about half their simulated pose before settling back, while the legs stay
//! animated and planted ([`PhysicalBlend`]). That gives each blow its own wobble on top of
//! the flinch, as Unreal's physical-animation hit reactions and Euphoria do.
//!
//! A blow pushing hard along the ground (at least [`HitReaction::stagger_above`] N·s) also
//! knocks the body back a step or two. Its center of mass sits on its feet like an inverted
//! pendulum: moving at the speed the blow gives it, it stops if a foot lands at the capture
//! point, `x + v·sqrt(h/g)` (Pratt et al. 2006). The body glides there, fast at first and
//! easing out over about `sqrt(h/g)` (0.3 s), and the feet step to keep up: the one
//! farthest along the push first, so the stance widens to catch the body ([`FootIk::hurry`]).
//!
//! People get a [`HitReaction`] when they are first hit; insert one to tune it.
//!
//! [`FootIk::hurry`]: crate::ik::FootIk::hurry
//!
//! [`Face`]: crate::face::Face

use engine_core::prelude::*;
use engine_world::DamageEvent;
use engine_world::physics::CharacterController;

use crate::animator::Animator;
use crate::build::humanoid::HIPS_HEIGHT;
use crate::ik::{FootIk, to_model, to_world};
use crate::ragdoll::{PhysicalBlend, PhysicalPulse, RagdollRig};

/// Bones that flinch, and how much of the blow each can take.
const BONES: [(&str, f32); 7] = [
    ("spine", 0.5),
    ("chest", 0.6),
    ("neck", 1.45),
    ("head", 1.7),
    ("upper_arm_l", 1.2),
    ("upper_arm_r", 1.2),
    ("hips", 0.05),
];

/// How a person flinches when hit.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HitReaction {
    pub enabled: bool,
    /// Flinch per unit of push, degrees per N·s (a 30 N·s blow turns the body about 24°).
    pub degrees_per_impulse: f32,
    /// Largest flinch of one bone, degrees.
    pub max_degrees: f32,
    /// Roughly how long a flinch takes to settle, s.
    pub settle_secs: f32,
    /// How far from the hit point a bone still takes much of it, m.
    pub reach: f32,
    /// How far the hips are shoved along the blow, meters per N·s.
    pub shove_per_impulse: f32,
    /// Blows at least this hard (N·s) rock the upper body with physics for a moment (0 = never).
    pub physical_above: f32,
    /// Most of the simulated pose such a blow shows (0..1), reached at three times
    /// `physical_above`.
    pub physical_peak: f32,
    /// How long the upper body takes to settle back to the animation, s.
    pub physical_secs: f32,
    /// Share of the blow the physical parts take (the flinch shows the rest).
    pub physical_push: f32,
    /// Blows pushing at least this hard along the ground (N·s) knock the body back a step or
    /// two (0 = never).
    pub stagger_above: f32,
    /// Mass a blow knocks back, kg, for a person of the generated size (scaled by size³).
    pub stagger_mass: f32,
    /// How much farther than the blow alone would carry it the body goes (game feel).
    pub stagger_scale: f32,
    /// Farthest one blow knocks the body back, m.
    pub stagger_max: f32,
    /// Move its own `Transform` while knocked back. Characters with a `CharacterController`
    /// never do (their movement owns the transform, and collides); for them, and with this
    /// off, [`HitReaction::stagger_velocity`] says how fast the blow pushes them.
    pub stagger_move: bool,
    /// How fast the knock-back moves the body this frame (world, m/s; published).
    #[serde(skip)]
    pub stagger_velocity: Vec3,
    /// Knock-back still to come (world, m) and how fast it comes (time constant, s).
    #[serde(skip)]
    pub stagger: (Vec3, f32),
    /// Each bone's turn (model space, radians, as an axis times an angle) and its rate.
    #[serde(skip)]
    pub state: Vec<(Vec3, Vec3)>,
    /// The hips' shove (model space, meters) and its rate.
    #[serde(skip)]
    pub shove: (Vec3, Vec3),
}

impl Default for HitReaction {
    fn default() -> Self {
        Self {
            enabled: true,
            degrees_per_impulse: 0.8,
            max_degrees: 35.0,
            settle_secs: 0.45,
            reach: 0.35,
            shove_per_impulse: 0.002,
            physical_above: 15.0,
            physical_peak: 0.6,
            physical_secs: 0.5,
            physical_push: 0.2,
            stagger_above: 25.0,
            stagger_mass: 75.0,
            stagger_scale: 1.5,
            stagger_max: 0.5,
            stagger_move: true,
            stagger_velocity: Vec3::ZERO,
            stagger: (Vec3::ZERO, 0.3),
            state: Vec::new(),
            shove: (Vec3::ZERO, Vec3::ZERO),
        }
    }
}

impl HitReaction {
    /// Strike the body at `point` (model space) with `push` (model space, N·s). `bones` are
    /// the model positions of [`BONES`] (None where the skeleton lacks one); `pelvis` the
    /// hips'.
    pub fn strike(&mut self, point: Vec3, push: Vec3, pelvis: Vec3, bones: &[Option<Vec3>]) {
        self.state.resize(BONES.len(), (Vec3::ZERO, Vec3::ZERO));
        if (point - pelvis).cross(push) == Vec3::ZERO || !self.enabled {
            return;
        }
        let peak = (push.length() * self.degrees_per_impulse)
            .min(self.max_degrees)
            .to_radians();
        // A critically damped spring kicked at rate v peaks at v / (e·ω).
        let omega = self.omega();
        let flat = push.with_y(0.0);
        // Blows near the hips move them; one to the head mostly moves the head.
        let above = (point.y - pelvis.y - 0.1).max(0.0) / 0.4;
        let low = (-above * above).exp();
        let shove = (flat.length() * self.shove_per_impulse * low).min(0.12);
        self.shove.1 += flat.normalize_or_zero() * shove * std::f32::consts::E * omega;
        for ((&(_, take), (_, rate)), at) in BONES.iter().zip(&mut self.state).zip(bones) {
            let Some(at) = at else {
                continue;
            };
            // Each joint turns the way the blow drives the point about it: joints below the
            // point give with it, joints above lag behind (a blow to the stomach folds the
            // body over it; one to a shoulder leaves the head behind).
            let lever = point - *at;
            let turn = lever.cross(push);
            let sin = turn.length() / (lever.length() * push.length()).max(1e-6);
            let lag = if lever.y < 0.0 { 1.4 } else { 1.0 };
            let axis = turn.normalize_or_zero() * (2.0 * sin).min(1.0) * lag;
            let near = (-(at.distance_squared(point)) / (self.reach * self.reach)).exp();
            let share = take * (0.25 + 0.75 * near);
            *rate += axis * peak * share * std::f32::consts::E * omega * 0.5;
        }
    }

    /// Move the body `by` (world, along the ground, m) with stepping feet: most of the way
    /// within `tau` seconds, fast at first and easing out. Adds to a knock-back under way.
    pub fn knock(&mut self, by: Vec3, tau: f32) {
        self.stagger = (self.stagger.0 + by.with_y(0.0), tau.max(0.05));
    }

    /// Knock the body back from a blow pushing `push` (world, N·s) on hips `hips` m high:
    /// to its capture point, `v·sqrt(h/g)` along the push for the speed `v` the blow gives it.
    fn knock_back(&mut self, push: Vec3, hips: f32) {
        let flat = push.with_y(0.0);
        if !self.enabled || self.stagger_above <= 0.0 || flat.length() < self.stagger_above {
            return;
        }
        let size = (hips / HIPS_HEIGHT).clamp(0.2, 5.0);
        let speed = flat / (self.stagger_mass * size.powi(3)).max(1.0);
        let tau = (hips.max(0.05) / 9.81).sqrt();
        let by = (speed * tau * self.stagger_scale).clamp_length_max(self.stagger_max);
        self.knock(by, tau);
        let left = self.stagger.0.clamp_length_max(self.stagger_max);
        self.stagger.0 = left;
    }

    fn omega(&self) -> f32 {
        // Settled to a few percent after about 4.7 / ω.
        4.7 / self.settle_secs.max(0.05)
    }

    /// Advance the springs by `dt`; returns how strongly the body is flinching (0..1).
    pub fn settle(&mut self, dt: f32) -> f32 {
        let omega = self.omega();
        let steps = (dt / (1.0 / 240.0)).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        let mut most = 0.0f32;
        let (at, rate) = &mut self.shove;
        for _ in 0..steps {
            *rate += (-omega * omega * *at - 2.0 * omega * *rate) * h;
            *at += *rate * h;
        }
        most = most.max(at.length() / 0.05);
        for (angle, rate) in &mut self.state {
            for _ in 0..steps {
                *rate += (-omega * omega * *angle - 2.0 * omega * *rate) * h;
                *angle += *rate * h;
            }
            let limit = self.max_degrees.to_radians() * 1.5;
            if angle.length() > limit {
                *angle = angle.normalize() * limit;
            }
            most = most.max(angle.length() / self.max_degrees.to_radians().max(1e-3));
        }
        most.min(1.0)
    }
}

/// Kick the bodies that were hit this frame.
#[allow(clippy::type_complexity)]
pub(crate) fn take_hits(
    mut commands: Commands,
    mut hits: MessageReader<DamageEvent>,
    mut q: Query<(
        &Animator,
        &GlobalTransform,
        Option<&mut HitReaction>,
        Option<&RagdollRig>,
        Option<&mut PhysicalBlend>,
    )>,
) {
    for hit in hits.read() {
        let Ok((animator, gt, reaction, rig, physical)) = q.get_mut(hit.target) else {
            continue;
        };
        let skeleton = &animator.skeleton;
        let Some(pelvis) = skeleton.index_of("hips") else {
            continue;
        };
        if skeleton.index_of("spine").is_none() {
            continue;
        }
        let pelvis = animator.model[pelvis].t;
        let point = to_model(gt, hit.point);
        let push = if hit.impulse.length_squared() > 1e-6 {
            gt.rotation().conjugate() * hit.impulse
        } else {
            // No push given: the blow drives in from the point towards the body.
            (pelvis + Vec3::Y * 0.4 - point).normalize_or_zero() * hit.amount.max(0.0)
        };
        let bones: Vec<Option<Vec3>> = BONES
            .iter()
            .map(|(name, _)| skeleton.index_of(name).map(|i| animator.model[i].t))
            .collect();
        let world_push = gt.rotation() * push;
        let hips = pelvis.y * gt.scale().y;
        let tuning = match reaction {
            Some(mut r) => {
                r.strike(point, push, pelvis, &bones);
                r.knock_back(world_push, hips);
                r.clone()
            }
            None => {
                let mut r = HitReaction::default();
                r.strike(point, push, pelvis, &bones);
                r.knock_back(world_push, hips);
                commands.entity(hit.target).insert(r.clone());
                r
            }
        };
        // A hard blow rocks the upper body with physics too.
        let strength = push.length();
        if rig.is_some()
            && tuning.enabled
            && tuning.physical_above > 0.0
            && strength >= tuning.physical_above
        {
            let peak = tuning.physical_peak * (strength / (3.0 * tuning.physical_above)).min(1.0);
            let pulse = PhysicalPulse::new("spine", peak, 0.05, tuning.physical_secs);
            let impulse = world_push * tuning.physical_push;
            match physical {
                Some(mut p) => p.hit(pulse, hit.point, impulse),
                None => {
                    let mut p = PhysicalBlend::default();
                    p.hit(pulse, hit.point, impulse);
                    commands.entity(hit.target).insert(p);
                }
            }
        }
    }
}

/// Carry knocked-back bodies along, hurrying their feet to step after them.
#[allow(clippy::type_complexity)]
pub(crate) fn stagger_system(
    time: Res<Time>,
    mut q: Query<(
        &mut HitReaction,
        &mut Transform,
        &Animator,
        &GlobalTransform,
        Option<&mut FootIk>,
        Option<&LocalTimeScale>,
        Option<&CharacterController>,
    )>,
) {
    for (mut reaction, mut transform, animator, gt, ik, local, controller) in &mut q {
        let (left, tau) = reaction.stagger;
        if left == Vec3::ZERO {
            if reaction.stagger_velocity != Vec3::ZERO {
                reaction.stagger_velocity = Vec3::ZERO;
            }
            continue;
        }
        // Held still with the pose through hitstop.
        let dt = if animator.frozen() > 0.0 {
            0.0
        } else {
            time.delta_for(local)
        };
        if let Some(mut ik) = ik {
            if ik.hurry <= 0.0 {
                // The foot farthest along the push steps first.
                let dir = left.normalize_or_zero();
                ik.lead = (0..ik.legs.len())
                    .filter_map(|n| {
                        let foot = animator.skeleton.index_of(&ik.legs[n].foot)?;
                        let at = to_world(gt, animator.model[foot].t) - gt.translation();
                        Some((n, at.as_vec3().dot(dir)))
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(n, _)| n);
            }
            ik.hurry = ik.hurry.max(4.0 * tau);
        }
        let step = left * (1.0 - (-dt / tau).exp());
        let rest = left - step;
        reaction.stagger.0 = if rest.length() < 1e-3 {
            Vec3::ZERO
        } else {
            rest
        };
        reaction.stagger_velocity = if dt > 0.0 { step / dt } else { Vec3::ZERO };
        if reaction.stagger_move && controller.is_none() {
            transform.translation += step.as_dvec3();
        }
    }
}

/// Bend flinching bodies (after the graph and procedural motion, before IK).
pub(crate) fn flinch_system(
    time: Res<Time>,
    mut q: Query<(&mut Animator, &mut HitReaction, Option<&LocalTimeScale>)>,
) {
    for (mut animator, mut reaction, local) in &mut q {
        if reaction.state.is_empty() || !animator.posed() {
            continue;
        }
        let dt = time.delta_for(local);
        let wince = reaction.settle(dt);
        if wince < 1e-3 {
            continue;
        }
        // Edit this frame's pose: model space is stale until refreshed (left alone, each
        // frame's edit would build on the last one and the hips would slide away).
        animator.refresh_model();
        let skeleton = animator.skeleton.clone();
        let mut edit = animator.edit();
        let shove = reaction.shove.0;
        if let Some(hips) = skeleton.index_of("hips") {
            // The knees give a little as the hips are driven back.
            edit.translate_model(hips, shove - Vec3::Y * 0.5 * shove.length());
        }
        // The trunk leans against the shove, so the body bends rather than tipping over
        // its planted feet like a plank.
        if let Some(spine) = skeleton.index_of("spine") {
            let lean = Vec3::Y.cross(-shove.with_y(0.0));
            if lean.length() > 1e-6 {
                let angle = lean.length() / 0.45;
                edit.rotate_model(spine, Quat::from_axis_angle(lean.normalize(), angle));
            }
        }
        for ((name, _), (angle, _)) in BONES.iter().zip(&reaction.state) {
            let Some(bone) = skeleton.index_of(name) else {
                continue;
            };
            let a = angle.length();
            if a > 1e-5 {
                edit.rotate_model(bone, Quat::from_axis_angle(*angle / a, a));
            }
        }
        // The face screws up while it lasts.
        let brace = (wince * 1.5).min(1.0);
        match animator.curves.iter_mut().find(|(n, _)| n == "brace") {
            Some((_, v)) => *v = v.max(brace),
            None => animator.curves.push(("brace".into(), brace)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bones() -> Vec<Option<Vec3>> {
        BONES
            .iter()
            .map(|(n, _)| {
                Some(match *n {
                    "spine" => Vec3::new(0.0, 1.05, 0.0),
                    "chest" => Vec3::new(0.0, 1.25, 0.0),
                    "neck" => Vec3::new(0.0, 1.45, 0.0),
                    "head" => Vec3::new(0.0, 1.55, 0.0),
                    "upper_arm_l" => Vec3::new(-0.18, 1.42, 0.0),
                    "upper_arm_r" => Vec3::new(0.18, 1.42, 0.0),
                    _ => Vec3::new(0.0, 0.98, 0.0),
                })
            })
            .collect()
    }

    #[test]
    fn a_blow_turns_the_body_away_and_it_springs_back() {
        let mut r = HitReaction::default();
        // Punched in the face from the front (the person faces −Z): pushed towards +Z.
        r.strike(
            Vec3::new(0.0, 1.6, -0.1),
            Vec3::new(0.0, 0.0, 30.0),
            Vec3::new(0.0, 0.98, 0.0),
            &bones(),
        );
        let head = 3;
        let mut peak = 0.0f32;
        let mut peak_at = 0;
        for i in 0..120 {
            r.settle(1.0 / 60.0);
            // The head's whole turn: its own and the bones below it.
            let a: Vec3 = [6, 0, 1, 2, 3].iter().map(|&b| r.state[b].0).sum();
            if a.length() > peak {
                peak = a.length();
                peak_at = i;
            }
            if i == 5 {
                // The head tips back: a turn about +X lifts the face towards +Z.
                assert!(a.x > 0.0, "head turned {a}");
            }
        }
        assert!(
            (12.0..40.0).contains(&peak.to_degrees()),
            "peak {}°",
            peak.to_degrees()
        );
        assert!(peak_at < 20, "peaked after {peak_at} frames");
        assert!(r.state[head].0.length() < 0.01, "still turned after 2 s");
        // The head took more than the hips.
        let mut r2 = HitReaction::default();
        r2.strike(
            Vec3::new(0.0, 1.6, -0.1),
            Vec3::new(0.0, 0.0, 30.0),
            Vec3::new(0.0, 0.98, 0.0),
            &bones(),
        );
        assert!(r2.state[head].1.length() > 3.0 * r2.state[6].1.length());
    }

    #[test]
    fn a_blow_to_the_stomach_folds_the_body_over() {
        let mut r = HitReaction::default();
        r.strike(
            Vec3::new(0.0, 1.1, -0.15),
            Vec3::new(0.0, 0.0, 30.0),
            Vec3::new(0.0, 0.98, 0.0),
            &bones(),
        );
        for _ in 0..6 {
            r.settle(1.0 / 60.0);
        }
        // The head's whole turn tips it forward (about −X), the hips back (about +X).
        let head: Vec3 = [6, 0, 1, 2, 3].iter().map(|&b| r.state[b].0).sum();
        assert!(head.x < -0.05, "head turned {head}");
        assert!(r.state[6].0.x > 0.0, "hips turned {}", r.state[6].0);
    }
}
