//! The animation contract: how gameplay, movement, AI, cloth and hair talk to whichever
//! animation module is installed, without depending on it.
//!
//! * In: [`AnimationParams`] (continuous blend inputs such as speed, lean, effort) and
//!   [`AnimationTrigger`] (one-shots such as dash, block, attack, hit reaction), and
//!   [`Ragdoll`] (how physical the body is).
//! * Out: [`SkeletonPose`] (every bone, every frame, for IK targets, attachments, cloth and
//!   hair pinning, hit boxes) and [`AnimationEvent`] (footsteps, hit frames, sounds).
//! * [`BodyRequest`]: any module can ask for a person or an animal on an entity it moves itself
//!   (a walker on a trail, a villager, a rider); the animation module builds the body and
//!   animates it from that motion.
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

/// Asks the installed animation module for a body on this entity: a person or an animal that
/// it builds (or loads), dresses and animates. Modules that only need someone walking about
/// (a hiker on a footstep trail, villagers, a pilot) use this instead of modelling their own.
///
/// * The entity's `Transform` stays with whoever moves it (the requester, or the movement
///   module through [`MoveIntent`](crate::MoveIntent)); the animation module never moves it. The
///   body stands on the entity's origin and faces its −Z.
/// * The body animates from the entity's own motion: [`LocomotionState`](crate::LocomotionState)
///   when a movement module writes one, otherwise how the transform moves from frame to frame.
///   Speed picks the gait (walk, run; a horse's trot, canter and gallop), turning leans it,
///   feet are planted on the ground. [`AnimationParams`] and [`AnimationTrigger`] still work
///   on top.
/// * The animation module answers by inserting [`SkeletonPose`] on the entity, and from then
///   on it writes the feet's [`GroundContact`](crate::GroundContact)s and footstep
///   [`AnimationEvent`]s. A requester that drew a stand-in body of its own hides it and stops
///   writing contacts once the pose appears, so nothing steps twice. Without an animation
///   module, or for a kind it doesn't know, no pose appears and the stand-in stays.
/// * Removing the component takes the body away again; the entity stays.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyRequest {
    /// What to build. Conventional kinds: `human`, `horse`, `dog`, `spider`.
    pub kind: String,
    /// A particular model, as an asset path under the fetched assets
    /// (`anim/characters/walker.glb`). `None` lets the module choose (by `seed`).
    pub model: Option<String>,
    /// Varies whatever the module varies (build, clothes, coat), the same way every run.
    pub seed: u64,
}

impl Default for BodyRequest {
    fn default() -> Self {
        Self::new("human")
    }
}

impl BodyRequest {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            model: None,
            seed: 0,
        }
    }

    pub fn human(seed: u64) -> Self {
        Self::new("human").with_seed(seed)
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
}

pub(crate) fn register(app: &mut App) {
    app.add_message::<AnimationTrigger>()
        .add_message::<AnimationEvent>()
        .register_component::<AnimationParams>(
            "Named animation inputs: speed (m/s), direction, lean, aim, effort (muscle flex), crouch.",
        )
        .register_component::<Ragdoll>("0 = animated, 1 = limp ragdoll; between = physical animation.")
        .register_component::<BodyRequest>(
            "Ask the animation module for a body (kind: human, horse, dog, spider; optional model \
             path; seed) animated from the entity's own motion.",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_requests_from_data_fill_in_the_rest() {
        let horse: BodyRequest = serde_json::from_str(r#"{"kind": "horse", "seed": 7}"#).unwrap();
        assert_eq!(horse, BodyRequest::new("horse").with_seed(7));
        let anyone: BodyRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(anyone, BodyRequest::human(0));
        let walker = BodyRequest::human(3).with_model("anim/characters/walker.glb");
        let back: BodyRequest =
            serde_json::from_str(&serde_json::to_string(&walker).unwrap()).unwrap();
        assert_eq!(back, walker);
    }
}
