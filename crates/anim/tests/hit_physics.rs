//! A hard blow rocks the upper body with physics for a moment (`PhysicalBlend`): the parts
//! from the spine up simulate and take part of the blow, the legs move with the animation as
//! kinematic bodies so the feet stay planted, and the bodies go again once it settles. A
//! light blow only flinches. A stand-in backend here integrates forces on free bodies (no
//! joints; the drives keep the parts together); runs with or without the fetched models.

use engine_anim::{AnimationPlugin, HitReaction, PhysicalBlend, RagdollPart, RagdollRig};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{BodyRequest, SkeletonPose};
use engine_world::physics::{BodyKind, ExternalForce, RigidBody, Velocity, claim_backend};
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

/// Moves dynamic bodies by the forces on them (each a solid ball of radius 0.1 m for turning).
struct ForcePhysics;

impl Plugin for ForcePhysics {
    fn build(&self, app: &mut App) {
        claim_backend(app, "hit physics test");
        app.add_systems(FixedUpdate, integrate.in_set(SimSet::Physics));
    }
}

fn integrate(
    time: Res<Time>,
    mut q: Query<(&mut Transform, &mut Velocity, &RigidBody, &ExternalForce)>,
) {
    let dt = time.delta_secs();
    for (mut t, mut v, body, f) in &mut q {
        if body.kind != BodyKind::Dynamic {
            continue;
        }
        let mass = body.mass_kg.unwrap_or(1.0).max(0.1);
        let inertia = 0.4 * mass * 0.1 * 0.1;
        v.linear += (f.force / mass + Vec3::new(0.0, -9.81, 0.0)) * dt;
        v.angular += f.torque / inertia * dt;
        t.translation += v.linear.as_dvec3() * dt as f64;
        let spin = v.angular * dt;
        if spin.length() > 0.0 {
            t.rotation = (Quat::from_scaled_axis(spin) * t.rotation).normalize();
        }
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
        app.add_plugins(ForcePhysics);
    }
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let t = Transform::default();
    // Only the upper body's physics here: no knock-back step (tests/stagger.rs has that).
    let reaction = HitReaction {
        stagger_above: 0.0,
        ..Default::default()
    };
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::human(0), reaction))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    for _ in 0..60 {
        app.update();
    }
    (app, e)
}

fn bone(app: &App, e: Entity, name: &str) -> DVec3 {
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    let gt = app.world.get::<GlobalTransform>(e).unwrap();
    pose.bone_world(gt, pose.index_of(name).unwrap()).unwrap().0
}

/// Punch `e` in the face from the front (people face −Z) with `impulse` N·s.
fn punch(app: &mut App, e: Entity, impulse: f32) {
    let at = bone(app, e, "head") + DVec3::new(0.0, 0.07, -0.1);
    app.world.write_message(DamageEvent {
        target: e,
        amount: 10.0,
        kind: DamageKind::Blunt,
        point: at,
        impulse: Vec3::new(0.0, 0.0, impulse),
        source: None,
    });
}

/// How far `name` is bent from its parent bone `parent`, beyond the animation, radians.
fn bend(phys: &App, e: Entity, anim: &App, a: Entity, parent: &str, name: &str) -> f32 {
    let rel = |app: &App, e: Entity| {
        let pose = app.world.get::<SkeletonPose>(e).unwrap();
        let gt = app.world.get::<GlobalTransform>(e).unwrap();
        let r = |b: &str| pose.bone_world(gt, pose.index_of(b).unwrap()).unwrap().1;
        r(parent).inverse() * r(name)
    };
    rel(phys, e).angle_between(rel(anim, a))
}

fn parts(app: &mut App) -> Vec<(Entity, RagdollPart)> {
    let mut q = app.world.query::<(Entity, &RagdollPart)>();
    q.iter(&app.world).map(|(e, p)| (e, p.clone())).collect()
}

#[test]
fn a_hard_blow_rocks_the_upper_body_while_the_feet_stay_planted() {
    // The same person twice, both punched; one of them has physics.
    let (mut phys, e) = person(true);
    let (mut anim, a) = person(false);
    punch(&mut phys, e, 40.0);
    punch(&mut anim, a, 40.0);
    let (mut rocked, mut feet_moved, mut pushed) = (0.0f64, 0.0f64, 0.0f32);
    let mut kinematic_legs = false;
    let (mut neck_bend, mut back_bend) = (0.0f32, 0.0f32);
    let mut physical_frames = 0;
    for frame in 0..90 {
        phys.update();
        anim.update();
        if phys.world.get::<RagdollRig>(e).unwrap().is_physical() {
            physical_frames += 1;
        }
        for (p, part) in parts(&mut phys) {
            let body = phys.world.get::<RigidBody>(p).unwrap();
            if part.bone.starts_with("lower_leg") || part.bone.starts_with("upper_leg") {
                kinematic_legs |= body.kind == BodyKind::Kinematic;
            }
            if part.bone == "neck" {
                pushed = pushed.max(phys.world.get::<ExternalForce>(p).unwrap().force.z);
            }
        }
        neck_bend = neck_bend.max(bend(&phys, e, &anim, a, "chest", "head"));
        back_bend = back_bend.max(bend(&phys, e, &anim, a, "hips", "chest"));
        if frame < 40 {
            rocked = rocked.max((bone(&phys, e, "head") - bone(&anim, a, "head")).length());
        }
        for foot in ["foot_l", "foot_r"] {
            feet_moved = feet_moved.max((bone(&phys, e, foot) - bone(&anim, a, foot)).length());
        }
    }
    assert!(
        physical_frames > 10,
        "physical for {physical_frames} frames"
    );
    assert!(kinematic_legs, "the legs were simulated");
    assert!(pushed > 100.0, "the head took a push of only {pushed} N");
    assert!(
        rocked > 0.02,
        "the head moved only {rocked:.3} m beyond the flinch"
    );
    // ...but not so far that the neck or back look broken.
    assert!(
        neck_bend.to_degrees() < 30.0 && back_bend.to_degrees() < 20.0,
        "bent {:.0}° at the neck and {:.0}° in the back beyond the flinch",
        neck_bend.to_degrees(),
        back_bend.to_degrees()
    );
    assert!(feet_moved < 0.005, "the feet moved {feet_moved:.3} m");
    // Settled: the bodies are gone and the pose is the animation's again.
    for _ in 0..60 {
        phys.update();
        anim.update();
    }
    assert!(!phys.world.get::<RagdollRig>(e).unwrap().is_physical());
    assert!(parts(&mut phys).is_empty());
    let gap = (bone(&phys, e, "head") - bone(&anim, a, "head")).length();
    assert!(gap < 0.01, "the head is {gap:.3} m off after 2.5 s");
    let blend = phys.world.get::<PhysicalBlend>(e).unwrap();
    assert!(blend.pulses.is_empty() && blend.pushes.is_empty());
}

#[test]
fn a_light_blow_only_flinches() {
    let (mut app, e) = person(true);
    punch(&mut app, e, 8.0);
    for _ in 0..30 {
        app.update();
        assert!(!app.world.get::<RagdollRig>(e).unwrap().is_physical());
    }
    assert!(app.world.get::<PhysicalBlend>(e).is_none());
}
