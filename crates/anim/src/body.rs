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
//! * `deer` (or `stag`, `doe`, `fawn`), `wolf`, `boar`, `rabbit` (or `hare`): the wild animals
//!   ([`crate::build::wild`]) at the requested shoulder height (a typical adult when none is
//!   given: 1.2, 0.8, 0.85 and 0.2 m). A deer taller than the typical adult is a stag with
//!   antlers; one under 70% of it a spotted fawn. Their gaits pick walk, trot, canter and
//!   gallop (a rabbit hops and bounds) from the motion; the parameters `graze`, `lie` (or
//!   `rest`), `alert` and `downed` hold a pose, and the triggers `flinch`, `rear` (deer) and
//!   `howl` (wolf) play once ([`crate::actions`]).
//!
//! Other kinds get no body (one warning per kind), so the requester's stand-in stays.
//!
//! While a character module draws every [`Appearance`] (`AppearanceBodies` exists), people
//! with an appearance (or asking for one) keep the generated humanoid skeleton, posed as
//! always, but none of this module's meshes once the character module has fitted its own body
//! to them (`BodyFit`): it skins that body to their `SkeletonPose`. Until then the generated
//! mesh stands in.
//!
//! People also get [`HandControls`]: how closed each hand is, from the clips' `grip`, `flex`
//! and `fist` curves and what the hand holds ([`HandGrip`]), so a character module curls the
//! fingers.

use std::collections::HashSet;
use std::sync::Mutex;

use engine_core::prelude::*;
use engine_world::animation::{AnimationParams, BodyRequest, SkeletonPose};
use engine_world::character::{
    Appearance, AppearanceBodies, AppearanceRequest, BodyFit, HandControls, HandPose, HandShape,
};
use engine_world::{LocomotionMode, LocomotionState, MoveIntent};

use crate::animator::{AnimationLibrary, Animator, AnimatorSettings};
use crate::build::dog;
use crate::build::humanoid::{self, Outfit};
use crate::character::{AppliedHumanModel, HumanModel};
use crate::demo::{
    character_model, fetched_path, give_dog_body, give_horse_body, give_humanoid_body,
    give_spider_body, horse_model,
};
use crate::grip::HeldItems;
use crate::horse_model::{AppliedHorseModel, HorseModel};
use crate::ik::FootIk;
use crate::muscle::{MorphWeights, MuscleRig};
use crate::procedural::gait::LeggedGait;
use crate::procedural::spring::SpringChains;
use crate::ragdoll::RagdollRig;
use crate::retarget::{HandGrip, Retarget};
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
    Wild(crate::build::wild::Wild),
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
    /// How far crouched, 0..1 (eased toward the contract's `crouch`, a crouching
    /// [`MoveIntent`] or a crouched [`LocomotionMode`]).
    pub crouch: f32,
    owners: [Owner; PARAMS.len()],
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

const PARAMS: [&str; 8] = [
    "speed",
    "turn",
    "move_effort",
    "blocking",
    "heading",
    "accel",
    "grade",
    humanoid::CROUCHED,
];

/// How quickly a person drops into a crouch or rises from one, s (time constant).
const CROUCH_EASE: f32 = 0.14;

/// Build bodies for new or changed requests. A normal system that only queues world work
/// when a request changed, so on most frames it runs beside everything else instead of as
/// an exclusive system that waits for the whole schedule to drain.
pub(crate) fn answer_requests(
    mut commands: Commands,
    q: Query<(Entity, &BodyRequest, Option<&Body>), Changed<BodyRequest>>,
) {
    let todo: Vec<(Entity, BodyRequest, bool)> = q
        .iter()
        .filter(|(_, r, body)| body.is_none_or(|b| b.request != **r))
        .map(|(e, r, body)| (e, r.clone(), body.is_some()))
        .collect();
    if todo.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for (entity, request, rebuild) in todo {
            // (Despawned, or its request taken away, before the queue ran.)
            if !world
                .get_entity(entity)
                .is_ok_and(|e| e.contains::<BodyRequest>())
            {
                continue;
            }
            if rebuild {
                strip(world, entity);
            }
            build(world, entity, request);
        }
    });
}

/// Whether a character module draws this person (see the module docs).
fn drawn_elsewhere(world: &World, entity: Entity) -> bool {
    world.contains_resource::<AppearanceBodies>()
        && world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<Appearance>() || e.contains::<AppearanceRequest>())
}

/// Hide this module's mesh on people a character module draws, once it does: its first
/// build takes a few seconds, and until it has fitted the body ([`BodyFit`]) the generated
/// mesh stands in, so nobody shows as a floating sword.
pub(crate) fn hide_drawn_bodies(
    bodies: Option<Res<AppearanceBodies>>,
    mut q: Query<&mut SkinnedMeshRenderer, (With<Appearance>, With<BodyFit>)>,
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

/// Take the body away from entities whose request was removed (queued, like
/// [`answer_requests`], only when there is one).
pub(crate) fn remove_bodies(
    mut commands: Commands,
    q: Query<Entity, (With<Body>, Without<BodyRequest>)>,
) {
    let gone: Vec<Entity> = q.iter().collect();
    if gone.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for entity in gone {
            if world
                .get_entity(entity)
                .is_ok_and(|e| e.contains::<Body>() && !e.contains::<BodyRequest>())
            {
                strip(world, entity);
            }
        }
    });
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
        Option<&MoveIntent>,
        Option<&LocalTimeScale>,
    )>,
) {
    for (body, gt, mut motion, mut params, locomotion, intent, local) in &mut q {
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
        // Callers say crouch or not (0 or 1); the body sinks into it and rises out of it.
        let crouching = params.get("crouch") > 0.5
            || intent.is_some_and(|i| i.crouch)
            || locomotion.is_some_and(|l| l.mode == LocomotionMode::Crouch);
        let target = if crouching { 1.0 } else { 0.0 };
        motion.crouch += (target - motion.crouch) * smoothing(dt, CROUCH_EASE);
        if (motion.crouch - target).abs() < 1e-3 {
            motion.crouch = target;
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
            Some(motion.crouch),
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

/// Fit each person's logic skeleton to the body the character module draws ([`BodyFit`]):
/// their limb lengths, hips and shoulders, so feet plant and hands meet their grips on the
/// drawn proportions instead of the stock humanoid's. People with the same build share one
/// fitted skeleton (named after the build, to the millimetre); the animator is rebuilt on it.
#[allow(clippy::type_complexity)]
pub(crate) fn fit_bodies(
    mut library: ResMut<AnimationLibrary>,
    mut q: Query<
        (&Body, &BodyFit, &mut AnimatorSettings),
        Or<(Changed<BodyFit>, Added<AnimatorSettings>, Added<Body>)>,
    >,
) {
    use std::hash::{Hash, Hasher};
    for (body, fit, mut settings) in &mut q {
        if body.kind != BodyKind::Human
            || !(settings.skeleton == humanoid::SKELETON
                || settings.skeleton.starts_with(FIT_PREFIX))
        {
            continue;
        }
        // The spring chain of the stock ponytail keeps its own offsets (hair is drawn by the
        // character module).
        let joint = |bone: &str| -> Option<Vec3> {
            if bone.starts_with("ponytail") {
                return None;
            }
            let i = fit.names.iter().position(|n| n == bone)?;
            let t = fit.rest.get(i)?.translation;
            t.is_finite().then_some(t)
        };
        let base = humanoid::skeleton();
        if joint("hips").is_none() || joint("hand_r").is_none() || joint("foot_l").is_none() {
            continue;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for name in base.names.iter() {
            if let Some(p) = joint(name) {
                name.hash(&mut h);
                ((p * 1000.0).round().as_ivec3()).to_array().hash(&mut h);
            }
        }
        let name = format!("{FIT_PREFIX}{:016x}", h.finish());
        if !library.skeletons.contains_key(&name) {
            library.add_skeleton(humanoid::fit_skeleton(&name, joint));
        }
        if settings.skeleton != name {
            settings.skeleton = name;
        }
    }
}

/// Skeletons [`fit_bodies`] fits start with this.
pub const FIT_PREFIX: &str = "humanoid.fit.";

/// Acceleration (m/s²) at which a person leans fully into a start or back into a stop.
pub const FULL_LEAN_ACCEL: f32 = 4.0;

/// Curves that close a person's hands (0..1); the same ones close an imported person's
/// fingers.
const GRIP_CURVES: [&str; 3] = ["grip", "flex", "fist"];

/// Give people their [`HandControls`].
#[allow(clippy::type_complexity)]
pub(crate) fn give_hand_controls(
    mut commands: Commands,
    q: Query<(Entity, &Body), (With<Animator>, Without<HandControls>)>,
) {
    for (e, body) in &q {
        if body.kind == BodyKind::Human {
            commands.entity(e).insert(HandControls::default());
        }
    }
}

/// Write how closed each person's hands are: the closing curves of what they're doing, at
/// least what they hold (closed all the way on a held item).
#[allow(clippy::type_complexity)]
pub(crate) fn hand_controls_system(
    mut q: Query<(
        &Body,
        &Animator,
        Option<&HandGrip>,
        Option<&HeldItems>,
        &mut HandControls,
    )>,
) {
    for (body, animator, held, items, mut out) in &mut q {
        if body.kind != BodyKind::Human || !animator.posed() {
            continue;
        }
        let closed = GRIP_CURVES
            .iter()
            .map(|c| animator.curve(c))
            .fold(0.0, f32::max);
        let mut held = held.map_or([0.0; 2], |h| [h.left, h.right]);
        if let Some(items) = items {
            for (side, handle) in items.hands.iter().enumerate() {
                if handle.is_some() {
                    held[side] = 1.0;
                }
            }
        }
        let grip = held.map(|h| closed.max(h).clamp(0.0, 1.0));
        let fist = animator.curve("fist");
        let hands = [0, 1].map(|side| hand_pose(items, side, grip[side], fist));
        if out.grip != grip || out.hands != hands {
            out.grip = grip;
            out.hands = hands;
        }
    }
}

/// The hand pose library shape for one hand: closed on what it holds (a shield's strap, a
/// bow's grip, a handle of its own radius), else a fist as far as the `fist` curve says, a
/// grip as far as the other closing curves do, or relaxed (the natural cascade).
fn hand_pose(items: Option<&HeldItems>, side: usize, grip: f32, fist: f32) -> HandPose {
    if let Some(items) = items
        && let Some(handle) = items.hands[side]
    {
        let preset = items.presets[side].as_str();
        let shape = if preset.contains("shield") {
            HandShape::ShieldStrap
        } else if preset.contains("bow") {
            HandShape::Bow
        } else {
            HandShape::PowerGrip
        };
        return HandPose {
            shape,
            weight: 1.0,
            handle_radius_m: handle.radius,
            curl: None,
        };
    }
    if fist > 0.01 && fist >= grip - 1e-3 {
        return HandPose {
            shape: HandShape::Fist,
            weight: fist.min(1.0),
            ..Default::default()
        };
    }
    if grip > 0.01 {
        return HandPose {
            shape: HandShape::PowerGrip,
            weight: grip.min(1.0),
            handle_radius_m: crate::grip::STANDARD_RADIUS,
            curl: None,
        };
    }
    HandPose {
        shape: HandShape::Relaxed,
        weight: 1.0,
        ..Default::default()
    }
}

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
        other if crate::build::wild::Wild::of_kind(other).is_some() => {
            BodyKind::Wild(crate::build::wild::Wild::of_kind(other).expect("a wild kind"))
        }
        other => {
            static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
            let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
            if warned.get_or_insert_default().insert(other.to_string()) {
                tracing::warn!(
                    "body request: no {other:?} body yet (human, horse, dog, spider, deer, \
                     wolf, boar and rabbit are built); the stand-in stays"
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
        BodyKind::Wild(wild) => {
            let scale = request
                .height
                .map_or(1.0, |h| crate::build::wild::scale_for_height(wild, h));
            let look = crate::build::wild::Look::of(wild, &request.kind, scale, request.seed);
            crate::demo::give_wild_body(world, entity, wild, scale, look);
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
                    moves: crate::demo::moves("m"),
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
        HandControls,
    )>();
    e.remove::<(FootIk, SpringChains, LeggedGait, RagdollRig)>();
}
