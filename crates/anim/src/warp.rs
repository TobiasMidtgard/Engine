//! Locomotion warping: the captured walk fitted to how the body actually moves (Unreal's
//! Animation Warping, as Lyra and Paragon use it).
//!
//! Motion matching plays forward moves only. A body that travels off to one side of where it
//! faces, or faster or slower than the clip's feet cover the ground, would otherwise hand over
//! to the generated strafe cycle or slide its feet. Before foot IK, [`LocomotionWarp`]:
//!
//! - **Orientation warp.** Turns the hips toward the travel (up to [`LocomotionWarp::max_angle`],
//!   60°) and lets the spine, chest, neck and head take the turn back, so the legs walk the
//!   way the body goes while the chest and eyes keep facing where it faces. Motion matching
//!   reads `warp_max` from the animation parameters and looks for a forward walk while the
//!   travel is within it (the fallback takes over a little past it). Off below
//!   [`LocomotionWarp::min_speed`], so starts, stops and turns on the spot play as captured.
//! - **Stride warp.** Scales how far each ankle reaches ahead of or behind the hips along the
//!   travel by the body's speed over the speed the playing frames' feet cover (after motion
//!   matching's own play rate, 0.8..1.25), within [`LocomotionWarp::stride_min`] ..
//!   [`LocomotionWarp::stride_max`]; the legs follow by two-bone IK and the hips drop just
//!   enough for the farther foot to reach. Play rate and stride share the change, as Epic
//!   recommends (about 15-20% each).
//!
//! People whose feet are placed by [`FootIk`] get a `LocomotionWarp`; it does nothing unless
//! their graph plays motion matching.

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;

use crate::animator::{Animator, PoseEdit};
use crate::ik::{FootIk, two_bone};
use crate::xform::smoothing;

/// How much of a leg's length its hip-to-ankle span may reach before the hips drop.
const REACH: f32 = 0.985;
/// The most the hips drop for a long stride, meters.
const MAX_DROP: f32 = 0.08;

/// Fits the captured walk to the body's travel (see the module docs).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocomotionWarp {
    pub enabled: bool,
    /// Largest turn of the lower body toward the travel, radians.
    pub max_angle: f32,
    /// Speeds (m/s) between which warping comes in.
    pub min_speed: f32,
    pub full_speed: f32,
    /// Time constant of the lower body's turn, seconds…
    pub angle_smoothing: f32,
    /// …which turns no faster than this, radians per second.
    pub max_turn_rate: f32,
    /// Hips bone, and the bones above it that take the turn back, each with the share of it
    /// undone at that bone (rising to 1: the head faces ahead).
    pub hips: String,
    pub spine: Vec<(String, f32)>,
    /// Range of the stride scale.
    pub stride_min: f32,
    pub stride_max: f32,
    /// Time constants of the stride scale and of the hips' drop, seconds.
    pub stride_smoothing: f32,
    pub drop_smoothing: f32,
    /// The lower body's turn now, radians (positive to the left).
    #[serde(skip)]
    pub angle: f32,
    /// The stride scale now.
    #[serde(skip)]
    pub stride: f32,
    /// How far the hips are lowered now, meters.
    #[serde(skip)]
    pub drop: f32,
}

impl Default for LocomotionWarp {
    fn default() -> Self {
        Self {
            enabled: true,
            max_angle: 60f32.to_radians(),
            min_speed: 0.3,
            full_speed: 0.6,
            angle_smoothing: 0.1,
            max_turn_rate: 270f32.to_radians(),
            hips: "hips".into(),
            spine: vec![
                ("spine".into(), 0.35),
                ("chest".into(), 0.7),
                ("neck".into(), 0.9),
                ("head".into(), 1.0),
            ],
            stride_min: 0.75,
            stride_max: 1.35,
            stride_smoothing: 0.15,
            drop_smoothing: 0.08,
            angle: 0.0,
            stride: 1.0,
            drop: 0.0,
        }
    }
}

impl LocomotionWarp {
    /// Move toward the turn and stride wanted for travel at `speed` (m/s) toward `heading`
    /// (radians from where the body faces, positive to the left), with motion matching
    /// showing `share` of the pose and its feet covering `stride_speed` m/s.
    pub fn track(&mut self, speed: f32, heading: f32, share: f32, stride_speed: f32, dt: f32) {
        let on = if self.enabled {
            share.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let span = (self.full_speed - self.min_speed).max(1e-3);
        let x = ((speed - self.min_speed) / span).clamp(0.0, 1.0);
        let ramp = x * x * (3.0 - 2.0 * x) * on;
        let want = heading.clamp(-self.max_angle, self.max_angle) * ramp;
        let step = (want - self.angle) * smoothing(dt, self.angle_smoothing);
        let most = self.max_turn_rate * dt;
        self.angle += step.clamp(-most, most);
        let stride = if stride_speed > 0.3 && speed > 0.4 {
            let s = (speed / stride_speed).clamp(self.stride_min, self.stride_max);
            1.0 + (s - 1.0) * ramp
        } else {
            1.0
        };
        self.stride += (stride - self.stride) * smoothing(dt, self.stride_smoothing);
    }

    /// Whether this frame changes the pose.
    fn active(&self) -> bool {
        self.angle.abs() > 1e-4 || (self.stride - 1.0).abs() > 1e-4 || self.drop > 1e-5
    }

    /// Turn and stretch the pose as tracked (`heading` is the travel's direction, radians;
    /// `legs` are upper leg, lower leg, ankle and the knee's direction, model space).
    pub fn apply(
        &mut self,
        pose: &mut PoseEdit,
        heading: f32,
        legs: &[(usize, usize, usize, Vec3)],
        dt: f32,
    ) {
        let Some(hips) = pose.skeleton.index_of(&self.hips) else {
            return;
        };
        if !self.active() {
            return;
        }
        // Orientation: the hips (and the legs under them) turn toward the travel; the bones
        // above take the turn back bit by bit.
        if self.angle.abs() > 1e-4 {
            let chain: Vec<(usize, f32, Quat)> = self
                .spine
                .iter()
                .filter_map(|(name, back)| {
                    let b = pose.skeleton.index_of(name)?;
                    Some((b, *back, pose.model[b].r))
                })
                .collect();
            pose.rotate_model(hips, Quat::from_rotation_y(self.angle));
            for (b, back, was) in chain {
                let r = Quat::from_rotation_y(self.angle * (1.0 - back)) * was;
                pose.set_model_rotation(b, r.normalize());
            }
        }
        // Stride: each ankle's reach ahead of or behind the hips along the travel, scaled.
        let along = Quat::from_rotation_y(heading) * Vec3::NEG_Z;
        let center = pose.position(hips);
        let targets: Vec<(Vec3, f32)> = legs
            .iter()
            .map(|&(upper, lower, foot, _)| {
                let p = pose.position(foot);
                let reach = (p - center).dot(along);
                let length = (pose.position(lower) - pose.position(upper)).length()
                    + (p - pose.position(lower)).length();
                (p + along * reach * (self.stride - 1.0), length)
            })
            .collect();
        // The hips drop just enough for the farther ankle to stay within reach.
        let mut drop: f32 = 0.0;
        for (&(upper, ..), &(target, length)) in legs.iter().zip(&targets) {
            let v = target - pose.position(upper);
            let level = v.x * v.x + v.z * v.z;
            let most = (REACH * length).powi(2);
            let depth = (most - level).max(0.0).sqrt();
            drop = drop.max(-v.y - depth);
        }
        let drop = drop.clamp(0.0, MAX_DROP);
        self.drop += (drop - self.drop) * smoothing(dt, self.drop_smoothing);
        if self.drop > 1e-5 {
            pose.translate_model(hips, Vec3::NEG_Y * self.drop);
        }
        if (self.stride - 1.0).abs() > 1e-4 || self.drop > 1e-5 {
            let turn = Quat::from_rotation_y(self.angle);
            for (&(upper, lower, foot, knee), &(target, _)) in legs.iter().zip(&targets) {
                let keep = pose.model[foot].r;
                two_bone(pose, upper, lower, foot, target, turn * knee, 1.0);
                pose.set_model_rotation(foot, keep);
            }
        }
    }
}

/// People with foot placement get a warp.
#[allow(clippy::type_complexity)]
pub(crate) fn add_warps(
    mut commands: Commands,
    q: Query<Entity, (With<FootIk>, With<Animator>, Without<LocomotionWarp>)>,
) {
    for e in &q {
        commands.entity(e).insert(LocomotionWarp::default());
    }
}

/// Warp each person's pose before their feet are placed.
pub(crate) fn warp_system(
    mut q: Query<(
        &mut Animator,
        &mut LocomotionWarp,
        &FootIk,
        Option<&mut AnimationParams>,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(mut animator, mut warp, ik, params)| {
            let Some(mut params) = params else {
                return;
            };
            let max = if warp.enabled { warp.max_angle } else { 0.0 };
            if params.get("warp_max") != max {
                params.set("warp_max", max);
            }
            if !animator.posed() {
                return;
            }
            let (share, stride_speed) = animator
                .graph()
                .and_then(|g| g.motion())
                .map_or((0.0, 0.0), |p| (1.0 - p.fallback, p.stride_speed()));
            let (speed, heading) = (params.get("speed"), params.get("heading"));
            let dt = animator.dt;
            warp.track(speed, heading, share, stride_speed, dt);
            if !warp.active() {
                return;
            }
            let skeleton = animator.skeleton.clone();
            let legs: Vec<_> = ik
                .legs
                .iter()
                .filter_map(|l| {
                    Some((
                        skeleton.index_of(&l.upper)?,
                        skeleton.index_of(&l.lower)?,
                        skeleton.index_of(&l.foot)?,
                        l.knee_direction,
                    ))
                })
                .collect();
            animator.refresh_model();
            let mut pose = animator.edit();
            warp.apply(&mut pose, heading, &legs, dt);
        });
}
