//! Spring chains: tails, manes, antennae, ponytails, cables and floppy ears that lag behind,
//! swing with gravity and blow in the wind, then settle back to the animated pose.

use engine_core::prelude::*;
use engine_world::{Env, WorldShift};

use crate::animator::Animator;
use crate::ik::{to_model, to_world};
use crate::xform::rotation_between;

/// One chain of a [`SpringChains`] component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpringChainDef {
    /// Bones from the root (animated, pinned) to the tip.
    pub bones: Vec<String>,
    /// 0..1: how strongly each joint returns to its animated pose (per 1/60 s).
    pub stiffness: f32,
    /// 0..1: fraction of velocity kept per 1/60 s.
    pub damping: f32,
    /// Multiplier on gravity (0 = weightless).
    pub gravity: f32,
    /// How much the wind pushes the chain (0 = none).
    pub wind: f32,
    /// Extra length beyond the last bone (the tip's child offset), meters.
    pub tip_length: f32,
}

impl Default for SpringChainDef {
    fn default() -> Self {
        Self {
            bones: Vec::new(),
            stiffness: 0.15,
            damping: 0.9,
            gravity: 0.3,
            wind: 0.3,
            tip_length: 0.1,
        }
    }
}

/// Secondary motion for chains of bones.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SpringChains {
    pub chains: Vec<SpringChainDef>,
    pub enabled: bool,
    #[serde(skip)]
    state: Vec<ChainState>,
    #[serde(skip)]
    last_origin: Option<DVec3>,
}

impl SpringChains {
    /// Carry the particles into a shifted world frame (floating origin, a planet's frame).
    pub(crate) fn follow_shift(&mut self, s: &WorldShift) {
        self.last_origin = self.last_origin.map(|o| s.point(o));
        for c in &mut self.state {
            for p in c.pos.iter_mut().chain(c.prev.iter_mut()) {
                *p = s.direction(*p);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
struct ChainState {
    /// Particle positions relative to the entity's position, world orientation.
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
}

impl SpringChains {
    pub fn new(chains: Vec<SpringChainDef>) -> Self {
        Self {
            chains,
            enabled: true,
            ..Default::default()
        }
    }
}

pub(crate) fn spring_chain_system(
    env: Env,
    mut q: Query<(&mut Animator, &mut SpringChains, &GlobalTransform)>,
) {
    q.par_iter_mut()
        .for_each(|(mut animator, mut springs, gt)| {
            if !springs.enabled || springs.chains.is_empty() {
                return;
            }
            let dt = animator.dt;
            let origin = gt.translation();
            // The world moved under the particles by the entity's displacement.
            let shift = springs
                .last_origin
                .map_or(Vec3::ZERO, |o| (origin - o).as_vec3());
            springs.last_origin = Some(origin);
            let gravity = env.gravity(origin);
            let wind_v = env.wind(origin);
            let skeleton = animator.skeleton.clone();
            animator.refresh_model();
            let springs = &mut *springs;
            springs
                .state
                .resize(springs.chains.len(), ChainState::default());
            for (def, state) in springs.chains.iter().zip(springs.state.iter_mut()) {
                let bones: Vec<usize> = def
                    .bones
                    .iter()
                    .filter_map(|b| skeleton.index_of(b))
                    .collect();
                if bones.len() < 2 {
                    continue;
                }
                // Animated target positions (bones plus the tip), entity-relative world space.
                let last = *bones.last().unwrap();
                let tip_local = (skeleton.rest[last].r.conjugate() * skeleton.rest[last].t)
                    .normalize_or(Vec3::NEG_Z)
                    * def.tip_length.max(0.01);
                let mut targets: Vec<Vec3> = bones
                    .iter()
                    .map(|&b| (to_world(gt, animator.model[b].t) - origin).as_vec3())
                    .collect();
                let tip_model = animator.model[last].transform_point(tip_local);
                targets.push((to_world(gt, tip_model) - origin).as_vec3());
                let lengths: Vec<f32> =
                    targets.windows(2).map(|w| (w[1] - w[0]).length()).collect();
                if state.pos.len() != targets.len() || dt <= 0.0 && state.pos.is_empty() {
                    state.pos = targets.clone();
                    state.prev = targets.clone();
                }
                for p in state.pos.iter_mut().chain(state.prev.iter_mut()) {
                    *p -= shift;
                }
                // Integrate in steps of at most 1/60 s.
                let steps = ((dt * 60.0).ceil() as usize).clamp(1, 4);
                let h = dt / steps as f32;
                let rate = h * 60.0;
                let keep = def.damping.clamp(0.0, 1.0).powf(rate);
                let pull = 1.0 - (1.0 - def.stiffness.clamp(0.0, 1.0)).powf(rate);
                let accel = gravity * def.gravity + wind_v * def.wind * 0.5;
                for _ in 0..steps {
                    state.pos[0] = targets[0];
                    state.prev[0] = targets[0];
                    for i in 1..state.pos.len() {
                        let v = (state.pos[i] - state.prev[i]) * keep;
                        state.prev[i] = state.pos[i];
                        state.pos[i] += v + accel * h * h;
                        state.pos[i] = state.pos[i].lerp(targets[i], pull);
                        let parent = state.pos[i - 1];
                        let d = (state.pos[i] - parent).normalize_or(targets[i] - targets[i - 1]);
                        state.pos[i] = parent + d * lengths[i - 1];
                    }
                }
                // Rotate bones to follow the particles, root first.
                let mut edit = animator.edit();
                for (k, &bone) in bones.iter().enumerate() {
                    let here = edit.position(bone);
                    let child_model = if k + 1 < bones.len() {
                        edit.position(bones[k + 1])
                    } else {
                        edit.model[bone].transform_point(tip_local)
                    };
                    let from = (child_model - here).normalize_or(Vec3::NEG_Z);
                    let goal_model = to_model(gt, origin + state.pos[k + 1].as_dvec3());
                    let to = (goal_model - here).normalize_or(from);
                    edit.rotate_model(bone, rotation_between(from, to));
                }
            }
        });
}
