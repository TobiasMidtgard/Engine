//! Bodies on request: the [`BodyRequest`] contract. A module that moves an entity itself (a
//! hiker on a footstep trail, a villager, a pilot) asks for a person or an animal on it; this
//! module gives the entity the paddock's rigs and models, animates them from the entity's own
//! motion, and takes them away again when the request goes.
//!
//! * `human`: one of the fetched Rocketbox people (picked by seed, or by height: the woman is
//!   1.74 m, the men 1.81 m), with motion-capture locomotion; the generated person of the
//!   same outfit when they aren't fetched. Speed blends idle, walk and run; turning leans.
//! * `horse`: the imported horse when fetched (scaled to the requested shoulder height), else
//!   the generated one; its gait picks walk, trot, canter and gallop from the motion.
//! * `dog`: a furred Labrador-type dog built at the requested withers height (0.56 m when
//!   none is given); its gait picks walk, trot, canter and rotary gallop from the motion.
//! * `spider`: eight procedural legs.
//!
//! Other kinds get no body (one warning per kind), so the requester's stand-in stays.
//!
//! While a character module draws every [`Appearance`] (`AppearanceBodies` exists), people
//! with an appearance (or asking for one) keep the generated humanoid skeleton, posed as
//! always, but none of this module's meshes: the character module skins its own body to their
//! `SkeletonPose`.

use std::collections::HashSet;
use std::sync::Mutex;

use engine_core::prelude::*;
use engine_world::LocomotionState;
use engine_world::animation::{AnimationParams, BodyRequest, SkeletonPose};
use engine_world::character::{Appearance, AppearanceBodies, AppearanceRequest};

use crate::animator::{Animator, AnimatorSettings};
use crate::build::dog;
use crate::build::humanoid::Outfit;
use crate::character::{AppliedHumanModel, HumanModel};
use crate::demo::{
    character_model, fetched_path, give_dog_body, give_horse_body, give_humanoid_body,
    give_spider_body, horse_model,
};
use crate::horse_model::{AppliedHorseModel, HorseModel};
use crate::ik::FootIk;
use crate::muscle::{MorphWeights, MuscleRig};
use crate::procedural::gait::LeggedGait;
use crate::procedural::spring::SpringChains;
use crate::ragdoll::RagdollRig;
use crate::retarget::Retarget;
use crate::skin::{SkinPose, SkinnedMeshRenderer};
use crate::xform::smoothing;

/// The fetched people: file name, the generated outfit that stands in for them, standing
/// height (m).
const PEOPLE: [(&str, Outfit, f32); 4] = [
    ("walker", Outfit::Shirt, 1.81),
    ("athlete", Outfit::Athlete, 1.74),
    ("reacher", Outfit::Shirt, 1.82),
    ("lifter", Outfit::Lifter, 1.81),
];
/// Shoulder height of the imported horse at scale 1, m (the paddock wears it at 0.85).
const HORSE_FILE_WITHERS: f32 = 1.9;
/// Speed at which a person's effort (breathing, arm swing) is full, m/s.
const EFFORT_SPEED: f32 = 4.0;
/// Faster than this between two frames is a teleport, not motion, m/s.
const TELEPORT_SPEED: f32 = 60.0;

/// What a [`BodyRequest`] was answered with.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Body {
    /// The request this body answers; a changed request builds it again.
    pub request: BodyRequest,
    pub kind: BodyKind,
    /// The fetched model it wears (or will once loaded), if any.
    pub model: Option<String>,
    /// Whether the entity had no `AnimationParams` of its own (they go with the body).
    added_params: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Human,
    Horse,
    Dog,
    Spider,
}

/// How a body's entity has been moving, and which animation parameters this module writes.
#[derive(Component, Debug, Clone, Default)]
pub struct BodyMotion {
    last: Option<(DVec3, f32)>,
    /// Horizontal speed, m/s (smoothed).
    pub speed: f32,
    /// Turn rate, rad/s (smoothed; positive turns left).
    pub yaw_rate: f32,
    /// Which way it moves relative to where it faces, radians (smoothed): 0 ahead, positive
    /// to the left, ±π backwards.
    pub heading: f32,
    /// Velocity, m/s (smoothed).
    pub velocity: Vec3,
    /// How fast it speeds up along where it faces, m/s² (smoothed; negative slowing down or
    /// setting off backwards).
    pub accel: f32,
    /// How steeply it climbs (+) or descends (−) as it goes: rise per meter traveled
    /// (smoothed).
    pub grade: f32,
    owners: [Owner; 7],
}

/// Who sets one of a person's animation parameters.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Owner {
    /// This module, from the motion (nothing written yet).
    #[default]
    Motion,
    /// This module, which wrote this value last frame.
    Wrote(f32),
    /// The requester (or anyone else): it was set before the body came, or changed since.
    Others,
}

const PARAMS: [&str; 7] = [
    "speed",
    "turn",
    "move_effort",
    "blocking",
    "heading",
    "accel",
    "grade",
];

/// Build bodies for new or changed requests.
pub(crate) fn answer_requests(world: &mut World) {
    let mut q =
        world.query_filtered::<(Entity, &BodyRequest, Option<&Body>), Changed<BodyRequest>>();
    let todo: Vec<(Entity, BodyRequest, bool)> = q
        .iter(world)
        .filter(|(_, r, body)| body.is_none_or(|b| b.request != **r))
        .map(|(e, r, body)| (e, r.clone(), body.is_some()))
        .collect();
    for (entity, request, rebuild) in todo {
        if rebuild {
            strip(world, entity);
        }
        build(world, entity, request);
    }
}

/// Whether a character module draws this person (see the module docs).
fn drawn_elsewhere(world: &World, entity: Entity) -> bool {
    world.contains_resource::<AppearanceBodies>()
        && world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<Appearance>() || e.contains::<AppearanceRequest>())
}

/// Hide this module's mesh on people a character module draws.
pub(crate) fn hide_drawn_bodies(
    bodies: Option<Res<AppearanceBodies>>,
    mut q: Query<&mut SkinnedMeshRenderer, With<Appearance>>,
) {
    if bodies.is_none() {
        return;
    }
    for mut renderer in &mut q {
        if renderer.visible {
            renderer.visible = false;
        }
    }
}

/// Take the body away from entities whose request was removed.
pub(crate) fn remove_bodies(world: &mut World) {
    let mut q = world.query_filtered::<Entity, (With<Body>, Without<BodyRequest>)>();
    let gone: Vec<Entity> = q.iter(world).collect();
    for entity in gone {
        strip(world, entity);
    }
}

/// Turn each body's motion into its animation parameters (people; animals' gaits measure the
/// motion themselves).
#[allow(clippy::type_complexity)]
pub(crate) fn drive_bodies(
    time: Res<Time>,
    mut q: Query<(
        &Body,
        &GlobalTransform,
        &mut BodyMotion,
        &mut AnimationParams,
        Option<&LocomotionState>,
        Option<&LocalTimeScale>,
    )>,
) {
    for (body, gt, mut motion, mut params, locomotion, local) in &mut q {
        let dt = time.delta_for(local);
        let p = gt.translation();
        let f = gt.rotation() * Vec3::NEG_Z;
        let yaw = (-f.x).atan2(-f.z);
        let Some((last_p, last_yaw)) = motion.last.replace((p, yaw)) else {
            continue;
        };
        if dt <= 0.0 {
            continue;
        }
        let measured = ((p - last_p).as_vec3() / dt).with_y(0.0);
        let velocity = locomotion.map_or(measured, |l| l.velocity.with_y(0.0));
        if velocity.length() > TELEPORT_SPEED {
            continue;
        }
        let k = smoothing(dt, 0.15);
        motion.speed += (velocity.length() - motion.speed) * k;
        let was = motion.velocity;
        motion.velocity = was + (velocity - was) * k;
        let forward = ((motion.velocity - was) / dt).dot(f.with_y(0.0).normalize_or_zero());
        motion.accel += (forward - motion.accel) * smoothing(dt, 0.2);
        // Rise per meter along the way: none standing about or in the air (a jump's or a
        // fall's height change is not a slope).
        let horizontal = measured.length();
        let grounded = locomotion.is_none_or(|l| l.grounded);
        let rise = if horizontal > 0.3 && grounded {
            ((p.y - last_p.y) as f32 / dt / horizontal).clamp(-1.5, 1.5)
        } else {
            0.0
        };
        motion.grade += (rise - motion.grade) * smoothing(dt, 0.25);
        let mut turn = yaw - last_yaw;
        turn =
            (turn + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        motion.yaw_rate += (turn / dt - motion.yaw_rate) * smoothing(dt, 0.25);
        // Heading follows the motion; standing still it drifts back to ahead (unseen, as the
        // blends are all standing then) so a person sets off forwards.
        let pi = std::f32::consts::PI;
        let wrap = |a: f32| (a + pi).rem_euclid(std::f32::consts::TAU) - pi;
        let ahead = velocity.dot(f.with_y(0.0).normalize_or_zero());
        let left = velocity.dot(Vec3::new(f.z, 0.0, -f.x).normalize_or_zero());
        let (target, tau) = if velocity.length() > 0.2 {
            (left.atan2(ahead), 0.12)
        } else {
            (0.0, 0.5)
        };
        motion.heading = wrap(motion.heading + wrap(target - motion.heading) * smoothing(dt, tau));
        if body.kind != BodyKind::Human {
            continue;
        }
        let values = [
            Some(motion.speed),
            Some((motion.yaw_rate / 1.2).clamp(-1.0, 1.0)),
            Some((motion.speed / EFFORT_SPEED).clamp(0.0, 1.0)),
            // Holds the graph's block pose while the mover says it blocks.
            locomotion.map(|l| if l.blocking { 1.0 } else { 0.0 }),
            Some(motion.heading),
            Some(accel_param(motion.accel)),
            Some(grade_param(motion.grade)),
        ];
        for (i, (name, value)) in PARAMS.iter().zip(values).enumerate() {
            let Some(value) = value else {
                continue;
            };
            let current = params.values.get(*name).copied();
            motion.owners[i] = match motion.owners[i] {
                Owner::Wrote(w) if current != Some(w) => Owner::Others,
                Owner::Others => Owner::Others,
                _ => {
                    params.set(name, value);
                    Owner::Wrote(value)
                }
            };
        }
    }
}

/// Acceleration (m/s²) at which a person leans fully into a start or back into a stop.
pub const FULL_LEAN_ACCEL: f32 = 4.0;

/// The `accel` animation parameter (−1..1) for an acceleration along the facing, m/s².
pub fn accel_param(accel: f32) -> f32 {
    (accel / FULL_LEAN_ACCEL).clamp(-1.0, 1.0)
}

/// Rise per meter at which a person takes a climb (or a descent) fully: about a flight of
/// stairs.
pub const FULL_GRADE: f32 = 0.35;

/// The `grade` animation parameter (−1 down a flight of stairs .. 1 up one) for a rise per
/// meter traveled.
pub fn grade_param(rise: f32) -> f32 {
    (rise / FULL_GRADE).clamp(-1.0, 1.0)
}

impl BodyMotion {
    /// Parameters already set on the entity stay with whoever set them.
    fn new(params: Option<&AnimationParams>) -> Self {
        let mut motion = Self::default();
        for (owner, name) in motion.owners.iter_mut().zip(PARAMS) {
            if params.is_some_and(|p| p.values.contains_key(name)) {
                *owner = Owner::Others;
            }
        }
        motion
    }
}

fn build(world: &mut World, entity: Entity, request: BodyRequest) {
    let kind = match request.kind.as_str() {
        "human" | "person" => BodyKind::Human,
        "horse" => BodyKind::Horse,
        "dog" => BodyKind::Dog,
        "spider" => BodyKind::Spider,
        other => {
            static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
            let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
            if warned.get_or_insert_default().insert(other.to_string()) {
                tracing::warn!(
                    "body request: no {other:?} body yet (human, horse, dog and spider are \
                     built); the stand-in stays"
                );
            }
            return;
        }
    };
    let Ok(e) = world.get_entity(entity) else {
        return;
    };
    let params = e.get::<AnimationParams>().cloned();
    let model = match kind {
        BodyKind::Human => {
            let (mut model, outfit) = person(&request);
            if drawn_elsewhere(world, entity) {
                model = None;
            }
            give_humanoid_body(world, entity, outfit);
            if let Some(m) = &model {
                world.entity_mut(entity).insert(m.clone());
            }
            model.map(|m| m.path)
        }
        BodyKind::Horse => {
            let model = horse(&request);
            give_horse_body(world, entity, model.clone());
            model.map(|m| m.path)
        }
        BodyKind::Dog => {
            let height = request.height.unwrap_or(dog::WITHERS);
            give_dog_body(world, entity, dog::scale_for_height(height));
            None
        }
        BodyKind::Spider => {
            give_spider_body(world, entity);
            None
        }
    };
    let motion = BodyMotion::new(params.as_ref());
    world.entity_mut(entity).insert((
        Body {
            request,
            kind,
            model,
            added_params: params.is_none(),
        },
        motion,
    ));
}

/// The fetched person a request asks for (if fetched) and the generated outfit standing in.
fn person(request: &BodyRequest) -> (Option<HumanModel>, Outfit) {
    if let Some(path) = &request.model {
        let stem = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if let Some(&(name, outfit, _)) = PEOPLE.iter().find(|p| p.0 == stem) {
            return (character_model(name), outfit);
        }
        let model = fetched_path(path)
            .or_else(|| std::path::Path::new(path).exists().then(|| path.clone()))
            .map(|p| {
                let clip =
                    |c: &str| fetched_path(&format!("anim/clips/m_{c}.glb")).unwrap_or_default();
                HumanModel {
                    idle: clip("idle_neutral_01"),
                    walk: clip("walk_neutral_01"),
                    run: clip("run_neutral_01"),
                    ..HumanModel::new(p)
                }
            });
        if model.is_none() {
            tracing::warn!(
                "body request: model {path} isn't fetched; a generated person stands in"
            );
        }
        return (model, Outfit::Shirt);
    }
    // By height: the people closest to it (the woman, or one of the men).
    let best = request.height.map(|h| {
        PEOPLE
            .iter()
            .map(|p| p.2)
            .min_by(|a, b| (a - h).abs().total_cmp(&(b - h).abs()))
            .unwrap_or(1.8)
    });
    let candidates: Vec<_> = PEOPLE
        .iter()
        .filter(|p| best.is_none_or(|b| (p.2 - b).abs() < 0.02))
        .collect();
    let &(name, outfit, _) = candidates[request.seed as usize % candidates.len()];
    (character_model(name), outfit)
}

/// The imported horse, scaled to the requested shoulder height, if fetched.
fn horse(request: &BodyRequest) -> Option<HorseModel> {
    let mut model = match &request.model {
        Some(path) => HorseModel {
            path: fetched_path(path)?,
            ..Default::default()
        },
        None => horse_model()?,
    };
    if let Some(h) = request.height {
        model.scale = (h / HORSE_FILE_WITHERS).clamp(0.3, 1.5);
    }
    Some(model)
}

/// Remove everything a body added, leaving the entity as the requester made it.
fn strip(world: &mut World, entity: Entity) {
    let Ok(mut e) = world.get_entity_mut(entity) else {
        return;
    };
    if e.get::<Body>().is_some_and(|b| b.added_params) {
        e.remove::<AnimationParams>();
    }
    e.remove::<(
        Body,
        BodyMotion,
        HumanModel,
        AppliedHumanModel,
        HorseModel,
        AppliedHorseModel,
        Retarget,
        AnimatorSettings,
        Animator,
        SkeletonPose,
        SkinnedMeshRenderer,
        SkinPose,
        MorphWeights,
        MuscleRig,
    )>();
    e.remove::<(FootIk, SpringChains, LeggedGait, RagdollRig)>();
}
