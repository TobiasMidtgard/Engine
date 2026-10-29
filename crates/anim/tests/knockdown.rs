//! A blow (or blows) that would knock the body back farther than two steps can catch it
//! (`HitReaction::fall_beyond`) knocks it down: limp with muscle tone, braced for the way it
//! falls, the blow spread over its parts; once it has lain still it gets up where it lies
//! and the knockdown is over. Without a physics backend the same blow only staggers. A
//! stand-in backend here moves bodies by their velocity only, so the test lays the body
//! down itself; runs with or without the fetched models.

use engine_anim::{AnimationPlugin, HitReaction, Knockdown, KnockdownPhase, RagdollPart};
use engine_anim::{RagdollRig, RagdollTone};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{BodyRequest, Ragdoll, SkeletonPose};
use engine_world::physics::{ExternalForce, RigidBody, Velocity, claim_backend};
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

/// Moves dynamic bodies by their velocity and nothing else.
struct DriftPhysics;

impl Plugin for DriftPhysics {
    fn build(&self, app: &mut App) {
        claim_backend(app, "knockdown test drift");
        app.add_systems(FixedUpdate, drift.in_set(SimSet::Physics));
    }
}

fn drift(time: Res<Time>, mut q: Query<(&mut Transform, &Velocity), With<RigidBody>>) {
    let dt = time.delta_secs_f64();
    for (mut t, v) in &mut q {
        t.translation += v.linear.as_dvec3() * dt;
    }
}

fn person(physics: bool) -> (App, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    if physics {
        app.add_plugins(DriftPhysics);
    }
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let t = Transform::default();
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::human(0)))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    wait(&mut app, 1.0);
    (app, e)
}

fn wait(app: &mut App, secs: f64) {
    for _ in 0..(secs / STEP).round() as usize {
        app.update();
    }
}

fn bone(app: &App, e: Entity, name: &str) -> DVec3 {
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    let gt = app.world.get::<GlobalTransform>(e).unwrap();
    pose.bone_world(gt, pose.index_of(name).unwrap()).unwrap().0
}

/// Punch `e` on the chin from the front (people face −Z) with `impulse` N·s.
fn punch(app: &mut App, e: Entity, impulse: f32) {
    let at = bone(app, e, "head") + DVec3::new(0.0, 0.07, -0.1);
    app.world.write_message(DamageEvent {
        target: e,
        amount: 20.0,
        kind: DamageKind::Blunt,
        point: at,
        impulse: Vec3::new(0.0, 0.0, impulse),
        source: None,
    });
}

fn parts(app: &mut App) -> Vec<Entity> {
    let mut q = app.world.query_filtered::<Entity, With<RagdollPart>>();
    q.iter(&app.world).collect()
}

/// Topple every part backward about the feet onto the floor and hold it still.
fn lay_on_back(app: &mut App) {
    let topple = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    for p in parts(app) {
        let mut t = app.world.get_mut::<Transform>(p).unwrap();
        t.translation = (topple * t.translation.as_vec3()).as_dvec3() + DVec3::Y * 0.12;
        t.rotation = topple * t.rotation;
        app.world.get_mut::<Velocity>(p).unwrap().linear = Vec3::ZERO;
    }
}

#[test]
fn a_heavy_blow_knocks_a_person_down_braced_and_they_get_up_where_they_lie() {
    let (mut app, e) = person(true);
    punch(&mut app, e, 95.0);
    app.update();
    let k = app.world.get::<Knockdown>(e).expect("knocked down");
    assert_eq!(k.phase, KnockdownPhase::Down);
    // The blow spread over its parts once they exist, pushing them back (+Z).
    let mut pushed = Vec3::ZERO;
    for _ in 0..6 {
        app.update();
        pushed += parts(&mut app)
            .iter()
            .map(|&p| app.world.get::<ExternalForce>(p).unwrap().force)
            .sum::<Vec3>()
            * STEP as f32;
    }
    assert!(
        pushed.z > 80.0 && pushed.x.abs() < 10.0,
        "parts pushed by {pushed} N·s"
    );
    // Limp and toned.
    assert_eq!(app.world.get::<Ragdoll>(e).unwrap().weight, 1.0);
    assert!(app.world.get::<RagdollTone>(e).is_some());
    assert!(app.world.get::<RagdollRig>(e).unwrap().is_physical());
    assert!(parts(&mut app).len() > 10);
    // Knocked from the front: it braces to fall back.
    let animator = app.world.get::<engine_anim::Animator>(e).unwrap();
    assert!(
        animator
            .graph()
            .is_some_and(|g| g.in_state("fall_back_brace")),
        "braced to fall back"
    );
    // It lands on its back half a meter behind; once still for down_secs it gets up there.
    lay_on_back(&mut app);
    app.update();
    let lying = bone(&app, e, "hips");
    assert!(lying.y < 0.35 && lying.z > 0.5, "lies at {lying}");
    let start = app.world.get::<Transform>(e).unwrap().translation;
    wait(&mut app, 0.5);
    assert!(
        app.world.get::<Knockdown>(e).unwrap().is_down(),
        "got up before lying still for a second"
    );
    let mut rose = false;
    for _ in 0..(6.0 / STEP) as usize {
        app.update();
        let Some(k) = app.world.get::<Knockdown>(e) else {
            break;
        };
        rose |= k.phase == KnockdownPhase::Rising;
    }
    assert!(rose, "never got up");
    assert!(
        app.world.get::<Knockdown>(e).is_none(),
        "still knocked down"
    );
    assert!(app.world.get::<RagdollTone>(e).is_none());
    assert_eq!(app.world.get::<Ragdoll>(e).unwrap().weight, 0.0);
    let rig = app.world.get::<RagdollRig>(e).unwrap();
    assert!(!rig.is_physical() && !rig.is_blending_back());
    // Standing where it lay (a get-up off the back ends over its feet, short of the hips).
    let moved = app.world.get::<Transform>(e).unwrap().translation - start;
    assert!(
        moved.z > 0.1 && moved.z < lying.z,
        "the entity moved {moved}"
    );
    let hips = bone(&app, e, "hips");
    let at = app.world.get::<Transform>(e).unwrap().translation;
    assert!(
        hips.y > at.y + 0.8 && (hips - at).with_y(0.0).length() < 0.25,
        "stands at {hips} over {at}"
    );
}

#[test]
fn without_physics_the_same_blow_only_staggers() {
    let (mut app, e) = person(false);
    let start = app.world.get::<Transform>(e).unwrap().translation;
    punch(&mut app, e, 95.0);
    wait(&mut app, 1.5);
    assert!(app.world.get::<Knockdown>(e).is_none());
    assert!(app.world.get::<Ragdoll>(e).is_none_or(|r| r.weight == 0.0));
    let moved = app.world.get::<Transform>(e).unwrap().translation - start;
    let most = HitReaction::default().stagger_max as f64;
    assert!(
        moved.z > 0.2 && moved.z <= most + 1e-3,
        "knocked back {moved}"
    );
}

#[test]
fn blows_that_stack_past_two_steps_knock_down_where_one_does_not() {
    // One 55 N·s blow knocks the body back about 0.35 m: a step or two.
    let (mut app, e) = person(true);
    punch(&mut app, e, 55.0);
    wait(&mut app, 0.1);
    assert!(
        app.world.get::<Knockdown>(e).is_none(),
        "one blow felled it"
    );
    // A second before the first has run out carries it past what two steps can catch.
    punch(&mut app, e, 55.0);
    app.update();
    assert!(app.world.get::<Knockdown>(e).is_some(), "two blows didn't");
    let r = app.world.get::<HitReaction>(e).unwrap();
    assert_eq!(r.stagger.0, Vec3::ZERO, "a felled body doesn't glide too");
}
