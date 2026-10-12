//! The animation contract: how gameplay, movement, AI, cloth and hair talk to whichever
//! animation module is installed, without depending on it.
//!
//! * In: [`AnimationParams`] (continuous blend inputs such as speed, lean, effort) and
//!   [`AnimationTrigger`] (one-shots such as dash, block, attack, hit reaction), and
//!   [`Ragdoll`] (how physical the body is).
//! * Out: [`SkeletonPose`] (every bone, every frame, for IK targets, attachments, cloth and
//!   hair pinning, hit boxes) and [`AnimationEvent`] (footsteps, hit frames, sounds).
//!
//! Parameter and trigger names are plain strings so data files and AI agents can drive any
//! character. Conventional names: `speed` (m/s), `direction` (rad, relative to facing),
//! `lean` (−1..1), `aim_pitch`/`aim_yaw` (rad), `effort` (0..1, drives muscle flex),
//! `crouch` (0..1); triggers `jump`, `dash`, `block`, `attack_light`, `attack_heavy`,
//! `hit_react`, `land`.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;

/// One bone's transform in the entity's local (model) space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BonePose {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for BonePose {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

/// Every bone of a skinned entity, written by the animation module after it evaluates each
/// frame (and after IK and physics blending).
#[derive(Component, Debug, Clone, Default)]
pub struct SkeletonPose {
    /// Bone names, shared between all instances of a skeleton.
    pub names: Arc<[String]>,
    /// Parent index per bone (`None` for roots).
    pub parents: Arc<[Option<u16>]>,
    /// Model-space transforms, same order as `names`.
    pub bones: Vec<BonePose>,
}

impl SkeletonPose {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// World position and rotation of bone `i` for an entity at `entity_transform`.
    pub fn bone_world(
        &self,
        entity_transform: &GlobalTransform,
        i: usize,
    ) -> Option<(DVec3, Quat)> {
        let b = self.bones.get(i)?;
        let rot = entity_transform.rotation();
        let pos = entity_transform.translation()
            + (rot * (b.translation * entity_transform.scale())).as_dvec3();
        Some((pos, rot * b.rotation))
    }
}

/// Continuous inputs to a character's animation graph, by name (see module docs).
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimationParams {
    pub values: HashMap<String, f32>,
}

impl AnimationParams {
    pub fn set(&mut self, name: &str, value: f32) {
        self.values.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> f32 {
        self.values.get(name).copied().unwrap_or(0.0)
    }
}

/// Fire a one-shot animation (dash, block, attack, hit reaction) on `entity`.
#[derive(Message, Debug, Clone)]
pub struct AnimationTrigger {
    pub entity: Entity,
    pub name: String,
}

/// Something happened at a marked frame of an animation: `footstep_left`, `hit`,
/// `weapon_release`. `bone` names where (for footprints, sparks).
#[derive(Message, Debug, Clone)]
pub struct AnimationEvent {
    pub entity: Entity,
    pub name: String,
    pub bone: Option<String>,
}

/// How physical a character is: 0 = fully animated, 1 = limp ragdoll; in between, the
/// animation drives the ragdoll with motors (stumbles, hit reactions). The animation module
/// builds the ragdoll from physics contract components (`RigidBody`, `Collider`, `Joint`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ragdoll {
    pub weight: f32,
}

pub(crate) fn register(app: &mut App) {
    app.add_message::<AnimationTrigger>()
        .add_message::<AnimationEvent>()
        .register_component::<AnimationParams>(
            "Named animation inputs: speed (m/s), direction, lean, aim, effort (muscle flex), crouch.",
        )
        .register_component::<Ragdoll>("0 = animated, 1 = limp ragdoll; between = physical animation.");
}
