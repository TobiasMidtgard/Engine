//! Hit flinches: a person struck (a [`DamageEvent`] aimed at them) jerks away from the blow
//! and springs back, on top of whatever they are doing.
//!
//! The push at the hit point (the event's impulse, or its amount pushing from the point to
//! the body when it has none) turns the spine, neck and head the way the blow drives them:
//! each joint about the axis from it to the point crossed with the push, so a jab to the
//! left shoulder twists it back, a blow to the face snaps the head away and one to the
//! stomach folds the body over it. Bones nearer the point take more of it, an arm that was
//! hit swings with the blow, and the hips are shoved along it. Each bone then settles on a
//! critically damped spring, and the face winces (the `brace` curve the [`Face`] reads) for
//! as long as it lasts. It runs before foot IK, so planted feet stay put.
//!
//! People get a [`HitReaction`] when they are first hit; insert one to tune it.
//!
//! [`Face`]: crate::face::Face

use engine_core::prelude::*;
use engine_world::DamageEvent;

use crate::animator::Animator;
use crate::ik::to_model;

/// Bones that flinch, and how much of the blow each can take.
const BONES: [(&str, f32); 7] = [
    ("spine", 0.5),
    ("chest", 0.6),
    ("neck", 0.5),
    ("head", 0.8),
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
        let shove = (flat.length() * self.shove_per_impulse).min(0.12);
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
pub(crate) fn take_hits(
    mut commands: Commands,
    mut hits: MessageReader<DamageEvent>,
    mut q: Query<(&Animator, &GlobalTransform, Option<&mut HitReaction>)>,
) {
    for hit in hits.read() {
        let Ok((animator, gt, reaction)) = q.get_mut(hit.target) else {
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
        match reaction {
            Some(mut r) => r.strike(point, push, pelvis, &bones),
            None => {
                let mut r = HitReaction::default();
                r.strike(point, push, pelvis, &bones);
                commands.entity(hit.target).insert(r);
            }
        }
    }
}

/// Bend flinching bodies (after the graph and procedural motion, before IK).
pub(crate) fn flinch_system(
    time: Res<Time>,
    mut q: Query<(&mut Animator, &mut HitReaction, Option<&LocalTimeScale>)>,
) {
    for (mut animator, mut reaction, local) in &mut q {
        if reaction.state.is_empty() {
            continue;
        }
        let dt = time.delta_for(local);
        let wince = reaction.settle(dt);
        if wince < 1e-3 {
            continue;
        }
        let skeleton = animator.skeleton.clone();
        let mut edit = animator.edit();
        if let Some(hips) = skeleton.index_of("hips") {
            edit.translate_model(hips, reaction.shove.0);
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
            (8.0..30.0).contains(&peak.to_degrees()),
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
