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
//! character. Conventional names: `speed` (m/s), `direction` (rad, the way the character moves
//! relative to its facing: 0 ahead, positive toward its left, counter-clockwise seen from above
//! like yaw and `turn`, ±π backwards), `lean` (−1..1), `aim_pitch`/`aim_yaw` (rad), `effort`
//! (0..1, drives muscle flex), `crouch` (0..1); triggers `jump`, `dash`, `block`,
//! `attack_light`, `attack_heavy`, `hit_react`, `land`.

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

/// The humanoid skeleton's bones, each with its parent: the names every module poses, skins
/// and reads people by (`SkeletonPose::names`, [`crate::BodyFit`], [`crate::combat::BodyZone`]).
/// Model space, meters, facing −Z, +X to the person's right; `_l` / `_r` are the person's own
/// left and right. The first 25 are the original skeleton; 30 finger bones (three joints
/// each) and 12 twist bones came with characters week's finger-and-twist skeleton. Posers that
/// don't know a bone leave it at rest. Index a pose by its own `names`, never by this order.
///
/// Rest pose: the animation module's humanoid, with straight legs and straight arms hanging 30°
/// out from the body. A finger bone's rest rotation is its hand's, and the hand's own modeled
/// shape (slightly bent, fanned) is the fingers' rest, so a finger's rotation in a pose is a
/// turn relative to its hand from that shape. A pose without finger bones gets its fingers
/// from [`crate::HandControls`]. Twist bones are never posed: each follows a share of its
/// limb's roll.
pub const HUMANOID_BONES: [(&str, Option<&str>); 67] = [
    ("root", None),
    ("hips", Some("root")),
    ("spine", Some("hips")),
    ("chest", Some("spine")),
    ("neck", Some("chest")),
    ("head", Some("neck")),
    ("ponytail_1", Some("head")),
    ("ponytail_2", Some("ponytail_1")),
    ("ponytail_3", Some("ponytail_2")),
    ("clavicle_l", Some("chest")),
    ("upper_arm_l", Some("clavicle_l")),
    ("lower_arm_l", Some("upper_arm_l")),
    ("hand_l", Some("lower_arm_l")),
    ("clavicle_r", Some("chest")),
    ("upper_arm_r", Some("clavicle_r")),
    ("lower_arm_r", Some("upper_arm_r")),
    ("hand_r", Some("lower_arm_r")),
    ("upper_leg_l", Some("hips")),
    ("lower_leg_l", Some("upper_leg_l")),
    ("foot_l", Some("lower_leg_l")),
    ("toe_l", Some("foot_l")),
    ("upper_leg_r", Some("hips")),
    ("lower_leg_r", Some("upper_leg_r")),
    ("foot_r", Some("lower_leg_r")),
    ("toe_r", Some("foot_r")),
    ("thumb_01_l", Some("hand_l")),
    ("thumb_02_l", Some("thumb_01_l")),
    ("thumb_03_l", Some("thumb_02_l")),
    ("index_01_l", Some("hand_l")),
    ("index_02_l", Some("index_01_l")),
    ("index_03_l", Some("index_02_l")),
    ("middle_01_l", Some("hand_l")),
    ("middle_02_l", Some("middle_01_l")),
    ("middle_03_l", Some("middle_02_l")),
    ("ring_01_l", Some("hand_l")),
    ("ring_02_l", Some("ring_01_l")),
    ("ring_03_l", Some("ring_02_l")),
    ("pinky_01_l", Some("hand_l")),
    ("pinky_02_l", Some("pinky_01_l")),
    ("pinky_03_l", Some("pinky_02_l")),
    ("thumb_01_r", Some("hand_r")),
    ("thumb_02_r", Some("thumb_01_r")),
    ("thumb_03_r", Some("thumb_02_r")),
    ("index_01_r", Some("hand_r")),
    ("index_02_r", Some("index_01_r")),
    ("index_03_r", Some("index_02_r")),
    ("middle_01_r", Some("hand_r")),
    ("middle_02_r", Some("middle_01_r")),
    ("middle_03_r", Some("middle_02_r")),
    ("ring_01_r", Some("hand_r")),
    ("ring_02_r", Some("ring_01_r")),
    ("ring_03_r", Some("ring_02_r")),
    ("pinky_01_r", Some("hand_r")),
    ("pinky_02_r", Some("pinky_01_r")),
    ("pinky_03_r", Some("pinky_02_r")),
    ("upper_arm_twist_01_l", Some("upper_arm_l")),
    ("upper_arm_twist_02_l", Some("upper_arm_l")),
    ("lower_arm_twist_01_l", Some("lower_arm_l")),
    ("lower_arm_twist_02_l", Some("lower_arm_l")),
    ("upper_arm_twist_01_r", Some("upper_arm_r")),
    ("upper_arm_twist_02_r", Some("upper_arm_r")),
    ("lower_arm_twist_01_r", Some("lower_arm_r")),
    ("lower_arm_twist_02_r", Some("lower_arm_r")),
    ("thigh_twist_01_l", Some("upper_leg_l")),
    ("calf_twist_01_l", Some("lower_leg_l")),
    ("thigh_twist_01_r", Some("upper_leg_r")),
    ("calf_twist_01_r", Some("lower_leg_r")),
];

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

/// Who a character's strikes go at, for the animation module to close on and land on.
/// Gameplay (the combat module, AI) writes it: `target` is the one gameplay picked, and `foes`
/// limits whom the animation module's own freeflow picking may choose (empty: anyone it can
/// strike).
///
/// `zone` and `point` are where on the target the strike aims (where the player looks, the zone
/// an NPC picked), `point` in the target's model space; defenders read them when the
/// attacker's `AttackEvent` telegraph arrives.
#[derive(Component, Debug, Clone, PartialEq, Default)]
pub struct StrikeTargets {
    pub target: Option<Entity>,
    pub foes: Vec<Entity>,
    pub zone: Option<crate::combat::BodyZone>,
    pub point: Option<Vec3>,
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
    /// Standing height of a person or shoulder height of an animal, m (children, a small
    /// dog). `None` lets the module choose.
    pub height: Option<f32>,
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
            height: None,
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

    pub fn with_height(mut self, height: f32) -> Self {
        self.height = Some(height);
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
             path and height in m; seed) animated from the entity's own motion.",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_humanoid_bone_has_one_name_and_a_parent_listed_before_it() {
        let mut seen = std::collections::HashSet::new();
        for (name, parent) in HUMANOID_BONES {
            assert!(seen.insert(name), "{name} twice");
            if let Some(p) = parent {
                assert!(
                    seen.contains(p),
                    "{name}'s parent {p} comes later or is missing"
                );
            }
        }
        assert_eq!(HUMANOID_BONES.iter().filter(|b| b.1.is_none()).count(), 1);
    }

    #[test]
    fn body_requests_from_data_fill_in_the_rest() {
        let horse: BodyRequest = serde_json::from_str(r#"{"kind": "horse", "seed": 7}"#).unwrap();
        assert_eq!(horse, BodyRequest::new("horse").with_seed(7));
        let anyone: BodyRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(anyone, BodyRequest::human(0));
        let walker = BodyRequest::human(3)
            .with_model("anim/characters/walker.glb")
            .with_height(1.78);
        let back: BodyRequest =
            serde_json::from_str(&serde_json::to_string(&walker).unwrap()).unwrap();
        assert_eq!(back, walker);
    }
}
