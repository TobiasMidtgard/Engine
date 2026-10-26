//! Muscles: flex from what the body is doing, relax afterwards, and jiggle when slack.
//!
//! Each muscle's activation (0..1) comes from its drivers: joint angles (a biceps flexes as
//! the elbow bends), effort (a heavy swing or bracing for a block, from the `effort`
//! parameter or an animation curve), gait load (a horse's hindquarters push off each stride).
//! Activation rises fast and falls slowly, like real muscle. It drives
//!
//! * a **bulge** morph target (the muscle swells and hardens its outline),
//! * the skin shader's **vein and striation** detail, which fades in with activation, and
//! * a **sway** morph target excited by the bone's acceleration: relaxed muscle and fat
//!   wobble after a sudden stop, tensed muscle barely moves.
//!
//! All of it is a handful of multiply-adds per muscle per frame; the deformation runs in the
//! GPU skinning pass.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::WorldShift;
use engine_world::animation::AnimationParams;

use crate::animator::Animator;
use crate::ik::to_world;
use crate::xform::{smoothing, twist_angle};

/// What makes a muscle flex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuscleDriver {
    /// Rotation of `bone` about its local `axis`: 0 at `from_deg`, 1 at `to_deg`.
    Joint {
        bone: String,
        axis: Vec3,
        from_deg: f32,
        to_deg: f32,
        weight: f32,
    },
    /// A parameter or animation curve (`effort`, `gait.intensity`, `gait.load.2`), times
    /// `weight`.
    Input { name: String, weight: f32 },
}

/// One muscle of a [`MuscleRig`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Muscle {
    pub name: String,
    /// Morph target that swells the muscle.
    pub bulge: String,
    /// Morph target that shifts it sideways for jiggle (optional).
    pub sway: Option<String>,
    pub drivers: Vec<MuscleDriver>,
    /// Resting activation (muscle tone), 0..1.
    pub tone: f32,
    /// Time to tense, seconds.
    pub rise_secs: f32,
    /// Time to relax, seconds.
    pub fall_secs: f32,
    /// Bone whose acceleration shakes this muscle.
    pub bone: String,
    /// Wobble frequency, Hz.
    pub jiggle_hz: f32,
    /// Wobble damping ratio (0.1 floppy .. 1 none).
    pub jiggle_damping: f32,
}

impl Default for Muscle {
    fn default() -> Self {
        Self {
            name: String::new(),
            bulge: String::new(),
            sway: None,
            drivers: Vec::new(),
            tone: 0.05,
            rise_secs: 0.08,
            fall_secs: 0.4,
            bone: String::new(),
            jiggle_hz: 5.0,
            jiggle_damping: 0.22,
        }
    }
}

/// Morph target weights of a skinned mesh, by target index. Written by the muscle system (and
/// by anyone else who wants to drive shapes: faces, corrective shapes, damage).
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct MorphWeights {
    /// Target names, in the mesh's order.
    pub names: Arc<[String]>,
    pub weights: Vec<f32>,
}

impl MorphWeights {
    pub fn new(names: Arc<[String]>) -> Self {
        let n = names.len();
        Self {
            names,
            weights: vec![0.0; n],
        }
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    pub fn set(&mut self, name: &str, w: f32) {
        if let Some(i) = self.index_of(name) {
            self.weights[i] = w;
        }
    }

    pub fn get(&self, name: &str) -> f32 {
        self.index_of(name).map_or(0.0, |i| self.weights[i])
    }
}

/// The muscles of a character. See the module docs.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MuscleRig {
    pub enabled: bool,
    pub muscles: Vec<Muscle>,
    /// How much muscles swell (1 = as modeled, 2 = bodybuilder).
    pub strength: f32,
    /// How much relaxed muscle wobbles (0 = never).
    pub jiggle: f32,
    /// Extra activation added to every muscle (e.g. a "rage" ability), 0..1.
    pub boost: f32,
    #[serde(skip)]
    state: Vec<MuscleState>,
    #[serde(skip)]
    last_origin: Option<DVec3>,
}

impl Default for MuscleRig {
    fn default() -> Self {
        Self {
            enabled: true,
            muscles: Vec::new(),
            strength: 1.0,
            jiggle: 1.0,
            boost: 0.0,
            state: Vec::new(),
            last_origin: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
struct MuscleState {
    activation: f32,
    x: f32,
    v: f32,
    prev_pos: Option<Vec3>,
    prev_vel: Option<Vec3>,
    bulge: Option<usize>,
    sway: Option<usize>,
    bone: Option<usize>,
    resolved: bool,
}

impl MuscleRig {
    /// Carry the jiggle's motion history into a shifted world frame.
    pub(crate) fn follow_shift(&mut self, s: &WorldShift) {
        self.last_origin = self.last_origin.map(|o| s.point(o));
        for m in &mut self.state {
            m.prev_pos = m.prev_pos.map(|p| s.direction(p));
            m.prev_vel = m.prev_vel.map(|v| s.direction(v));
        }
    }

    pub fn new(muscles: Vec<Muscle>) -> Self {
        Self {
            muscles,
            ..Default::default()
        }
    }

    /// Current activation of a muscle (0..1).
    pub fn activation(&self, name: &str) -> f32 {
        self.muscles
            .iter()
            .position(|m| m.name == name)
            .and_then(|i| self.state.get(i))
            .map_or(0.0, |s| s.activation)
    }

    /// Every muscle with its activation.
    pub fn activations(&self) -> Vec<(String, f32)> {
        self.muscles
            .iter()
            .enumerate()
            .map(|(i, m)| {
                (
                    m.name.clone(),
                    self.state.get(i).map_or(0.0, |s| s.activation),
                )
            })
            .collect()
    }
}

/// Target activation of one muscle for the current pose and inputs.
pub fn muscle_target(m: &Muscle, animator: &Animator, params: &AnimationParams, boost: f32) -> f32 {
    let mut a = m.tone + boost;
    for d in &m.drivers {
        match d {
            MuscleDriver::Joint {
                bone,
                axis,
                from_deg,
                to_deg,
                weight,
            } => {
                let Some(i) = animator.skeleton.index_of(bone) else {
                    continue;
                };
                let rest = animator.skeleton.rest[i].r;
                let rel = rest.conjugate() * animator.local[i].r;
                let angle = twist_angle(rel, axis.normalize_or(Vec3::X)).to_degrees();
                let span = to_deg - from_deg;
                let t = if span.abs() < 1e-3 {
                    0.0
                } else {
                    ((angle - from_deg) / span).clamp(0.0, 1.0)
                };
                a += t * weight;
            }
            MuscleDriver::Input { name, weight } => {
                let v = params.get(name).max(animator.curve(name));
                a += v.clamp(0.0, 1.0) * weight;
            }
        }
    }
    a.clamp(0.0, 1.0)
}

pub(crate) fn muscle_system(
    mut q: Query<(
        &Animator,
        &mut MuscleRig,
        &mut MorphWeights,
        &AnimationParams,
        &GlobalTransform,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(animator, mut rig, mut morphs, params, gt)| {
            if !rig.enabled || !animator.posed() {
                return;
            }
            if animator.fresh() {
                // The body moved while it rested: that isn't a shove to jiggle from.
                rig.last_origin = None;
            }
            let dt = animator.dt;
            let origin = gt.translation();
            let shift = rig
                .last_origin
                .map_or(Vec3::ZERO, |o| (origin - o).as_vec3());
            rig.last_origin = Some(origin);
            let rig = &mut *rig;
            rig.state.resize(rig.muscles.len(), MuscleState::default());
            for (m, s) in rig.muscles.iter().zip(rig.state.iter_mut()) {
                if !s.resolved {
                    s.bulge = morphs.index_of(&m.bulge);
                    s.sway = m.sway.as_deref().and_then(|n| morphs.index_of(n));
                    s.bone = animator.skeleton.index_of(&m.bone);
                    s.resolved = true;
                }
                let target = muscle_target(m, animator, params, rig.boost);
                let tau = if target > s.activation {
                    m.rise_secs
                } else {
                    m.fall_secs
                };
                s.activation += (target - s.activation) * smoothing(dt, tau);
                // Jiggle: a damped spring pushed by the bone's acceleration.
                if let Some(b) = s.bone
                    && dt > 0.0
                {
                    // Bone position relative to the entity, in world orientation; last frame's
                    // is shifted by how far the entity moved, so this measures world motion.
                    let p = (to_world(gt, animator.model[b].t) - origin).as_vec3();
                    let vel = s.prev_pos.map(|prev| (p - (prev - shift)) / dt);
                    let acc = match (vel, s.prev_vel) {
                        (Some(v), Some(pv)) => (v - pv) / dt,
                        _ => Vec3::ZERO,
                    };
                    s.prev_pos = Some(p);
                    s.prev_vel = vel;
                    let w = std::f32::consts::TAU * m.jiggle_hz;
                    let push = (acc.y + 0.3 * (acc.x + acc.z)).clamp(-200.0, 200.0) * 0.004;
                    // Semi-implicit Euler in small steps for stability.
                    let steps = ((dt * 240.0).ceil() as usize).clamp(1, 8);
                    let h = dt / steps as f32;
                    for _ in 0..steps {
                        let a = -w * w * s.x - 2.0 * m.jiggle_damping * w * s.v - push * w;
                        s.v += a * h;
                        s.x += s.v * h;
                    }
                    s.x = s.x.clamp(-1.0, 1.0);
                }
                if let Some(i) = s.bulge {
                    morphs.weights[i] = s.activation * rig.strength;
                }
                if let Some(i) = s.sway {
                    morphs.weights[i] = s.x * rig.jiggle * (1.0 - 0.75 * s.activation);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{BoneDef, Skeleton};

    fn arm() -> Animator {
        Animator::procedural(Arc::new(
            Skeleton::new(
                "arm",
                vec![
                    BoneDef::new("upper", None, Vec3::ZERO),
                    BoneDef::new("lower", Some("upper"), Vec3::new(0.0, -0.3, 0.0)),
                ],
            )
            .unwrap(),
        ))
    }

    fn biceps() -> Muscle {
        Muscle {
            name: "biceps".into(),
            bulge: "biceps".into(),
            bone: "upper".into(),
            tone: 0.0,
            drivers: vec![
                MuscleDriver::Joint {
                    bone: "lower".into(),
                    axis: Vec3::X,
                    from_deg: 0.0,
                    to_deg: 120.0,
                    weight: 0.6,
                },
                MuscleDriver::Input {
                    name: "effort".into(),
                    weight: 0.6,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn flexing_and_effort_activate() {
        let mut a = arm();
        let params = AnimationParams::default();
        assert_eq!(muscle_target(&biceps(), &a, &params, 0.0), 0.0);
        a.local[1].r = Quat::from_rotation_x(60f32.to_radians());
        let t = muscle_target(&biceps(), &a, &params, 0.0);
        assert!((t - 0.3).abs() < 0.01, "{t}");
        let mut params = AnimationParams::default();
        params.set("effort", 1.0);
        assert!((muscle_target(&biceps(), &a, &params, 0.0) - 0.9).abs() < 0.01);
    }

    #[test]
    fn activation_rises_fast_and_falls_slowly() {
        use engine_core::App;
        let mut app = App::new();
        let mut a = arm();
        a.dt = 1.0 / 60.0;
        let names: Arc<[String]> = vec!["biceps".to_string()].into();
        let e = app
            .world
            .spawn((
                a,
                MuscleRig::new(vec![biceps()]),
                MorphWeights::new(names),
                AnimationParams::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world
            .get_mut::<AnimationParams>(e)
            .unwrap()
            .set("effort", 1.0);
        let mut schedule = Schedule::default();
        schedule.add_systems(muscle_system);
        for _ in 0..10 {
            schedule.run(&mut app.world);
        }
        let up = app.world.get::<MorphWeights>(e).unwrap().weights[0];
        assert!(up > 0.5, "after 1/6 s: {up}");
        app.world
            .get_mut::<AnimationParams>(e)
            .unwrap()
            .set("effort", 0.0);
        for _ in 0..10 {
            schedule.run(&mut app.world);
        }
        let down = app.world.get::<MorphWeights>(e).unwrap().weights[0];
        assert!(down > 0.2 && down < up, "relaxing: {down}");
    }
}
