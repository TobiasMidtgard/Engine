//! Ragdolls through the physics contract: a body turns physical when its `Ragdoll` weight
//! rises, its pose follows the part bodies, and it blends back when the weight returns to 0.
//! A stand-in backend here only moves bodies by their velocity (the real solver is tested by
//! the physics module); runs with or without the fetched models.

use engine_anim::ragdoll::{BLEND_OUT_SECS, GET_UP_BLEND_SECS, RAGDOLL_LAYER};
use engine_anim::{AnimationPlugin, RagdollPart, RagdollRig};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{AnimationEvent, BodyRequest, Ragdoll, SkeletonPose};
use engine_world::physics::{
    CharacterController, Collider, ContactEvent, ContactPhase, ExternalForce, Joint, RigidBody,
    Velocity, claim_backend,
};
use engine_world::{ContactKind, GroundContact, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

/// Moves dynamic bodies by their velocity and nothing else.
struct DriftPhysics;

impl Plugin for DriftPhysics {
    fn build(&self, app: &mut App) {
        claim_backend(app, "ragdoll test drift");
        app.add_systems(FixedUpdate, drift.in_set(SimSet::Physics));
    }
}

fn drift(time: Res<Time>, mut q: Query<(&mut Transform, &Velocity), With<RigidBody>>) {
    let dt = time.delta_secs_f64();
    for (mut t, v) in &mut q {
        t.translation += v.linear.as_dvec3() * dt;
    }
}

#[derive(Resource, Default)]
struct Heard(Vec<GroundContact>);

fn listen(mut heard: ResMut<Heard>, mut contacts: MessageReader<GroundContact>) {
    heard.0.extend(contacts.read().copied());
}

/// Animation events, by name.
#[derive(Resource, Default)]
struct Said(Vec<String>);

fn listen_events(mut said: ResMut<Said>, mut events: MessageReader<AnimationEvent>) {
    said.0.extend(events.read().map(|e| e.name.clone()));
}

fn app(physics: bool) -> App {
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
    app.world.init_resource::<Heard>();
    app.world.init_resource::<Said>();
    app.add_systems(Last, (listen, listen_events));
    app
}

fn person(app: &mut App) -> Entity {
    let t = Transform::default();
    let e = app
        .world
        .spawn((
            Name::new("Stuntman"),
            t,
            GlobalTransform::from(t),
            BodyRequest::human(2),
            CharacterController::default(),
        ))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    e
}

fn walk(app: &mut App, e: Entity, speed: f64, secs: f64) {
    for _ in 0..(secs / STEP) as usize {
        app.world.get_mut::<Transform>(e).unwrap().translation.z -= speed * STEP;
        app.update();
    }
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

fn parts(app: &mut App) -> Vec<(Entity, RagdollPart)> {
    let mut q = app.world.query::<(Entity, &RagdollPart)>();
    let mut v: Vec<_> = q.iter(&app.world).map(|(e, p)| (e, p.clone())).collect();
    v.sort_by_key(|p| p.1.index);
    v
}

#[test]
fn a_walker_goes_limp_keeps_moving_and_comes_back() {
    let mut app = app(true);
    let e = person(&mut app);
    walk(&mut app, e, 1.4, 1.5);
    assert!(
        app.world.get::<RagdollRig>(e).is_some(),
        "people have ragdolls"
    );
    assert!(parts(&mut app).is_empty(), "no bodies while animated");
    let walking_hips = bone(&app, e, "hips");

    app.world.entity_mut(e).insert(Ragdoll { weight: 1.0 });
    app.update();
    let ps = parts(&mut app);
    assert_eq!(
        ps.len(),
        14,
        "pelvis, belly, chest, head, arms, legs and feet"
    );
    let mut joints = app.world.query::<&Joint>();
    let joints: Vec<Joint> = joints.iter(&app.world).cloned().collect();
    assert_eq!(joints.len(), 13);
    // Every joint starts together, and every part carries the walk's momentum.
    for j in &joints {
        let a = app.world.get::<Transform>(j.body_a).unwrap();
        let b = app.world.get::<Transform>(j.body_b.unwrap()).unwrap();
        let gap = (a.translation + (a.rotation * j.anchor_a).as_dvec3())
            - (b.translation + (b.rotation * j.anchor_b.as_vec3()).as_dvec3());
        assert!(gap.length() < 1e-3, "joint apart by {gap}");
    }
    let mut momentum = Vec3::ZERO;
    let mut mass = 0.0;
    for (p, _) in &ps {
        let c = app.world.get::<Collider>(*p).unwrap();
        assert_eq!(c.layers.member, RAGDOLL_LAYER);
        assert_eq!(
            c.layers.collides_with & RAGDOLL_LAYER,
            0,
            "parts pass through parts"
        );
        let kg = app.world.get::<RigidBody>(*p).unwrap().mass_kg.unwrap();
        momentum += app.world.get::<Velocity>(*p).unwrap().linear * kg;
        mass += kg;
    }
    let v = momentum / mass;
    assert!((70.0..80.0).contains(&mass), "{mass} kg");
    assert!(v.z < -1.0 && v.z > -1.8, "the body moves on at {v}");
    let controller = app.world.get::<CharacterController>(e).unwrap();
    assert_eq!(
        controller.layers.collides_with & RAGDOLL_LAYER,
        0,
        "its own controller ignores the parts"
    );

    // The pose follows the bodies: they drift on at the walking speed while the entity
    // stays put, and limp bodies take no steps.
    let hips_part = ps.iter().find(|p| p.1.bone == "hips").unwrap().0;
    app.world.resource_mut::<Heard>().0.clear();
    wait(&mut app, 1.0);
    let hips = bone(&app, e, "hips");
    let body = app.world.get::<Transform>(hips_part).unwrap().translation;
    assert!(
        (hips - body).length() < 1e-3,
        "hips bone {hips} on its body {body}"
    );
    assert!(
        hips.z < walking_hips.z - 0.8,
        "drifted on from {walking_hips} to {hips}"
    );
    let steps = app
        .world
        .resource::<Heard>()
        .0
        .iter()
        .filter(|c| c.kind != ContactKind::Body)
        .count();
    assert_eq!(steps, 0, "no footsteps while limp");
    // A limp body gets only joint friction and limits, no pull toward the animation.
    let f = app.world.get::<ExternalForce>(hips_part).unwrap();
    assert!(f.force.length() < 1e-3, "limp hips pushed by {}", f.force);

    // Back to animated: the bodies go, the pose blends back from where the body was.
    let fallen = bone(&app, e, "hips");
    app.world.get_mut::<Ragdoll>(e).unwrap().weight = 0.0;
    app.update();
    app.update();
    assert!(parts(&mut app).is_empty(), "the bodies go");
    let mut joints = app.world.query::<&Joint>();
    assert_eq!(joints.iter(&app.world).count(), 0, "and their joints");
    let controller = app.world.get::<CharacterController>(e).unwrap();
    assert_ne!(controller.layers.collides_with & RAGDOLL_LAYER, 0);
    let early = bone(&app, e, "hips");
    assert!(
        (early - fallen).length() < 0.4,
        "starts from where it lay: {early} vs {fallen}"
    );
    wait(&mut app, BLEND_OUT_SECS as f64 + 0.1);
    let standing = bone(&app, e, "hips");
    let origin = app.world.get::<GlobalTransform>(e).unwrap().translation();
    assert!(
        (standing.x - origin.x).abs() < 0.1 && (standing.z - origin.z).abs() < 0.25,
        "stands on its entity again: {standing} vs {origin}"
    );
    assert!(!app.world.get::<RagdollRig>(e).unwrap().is_blending_back());
}

#[test]
fn a_body_lying_face_down_gets_up_off_its_front_and_stands_on_its_entity() {
    let mut app = app(true);
    let e = person(&mut app);
    wait(&mut app, 0.5);
    app.world.entity_mut(e).insert(Ragdoll { weight: 1.0 });
    app.update();
    // Topple every part forward about the feet onto the floor, and let it lie still.
    let topple = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let lift = DVec3::new(0.0, 0.12, 0.0);
    for (p, _) in parts(&mut app) {
        let mut t = app.world.get_mut::<Transform>(p).unwrap();
        t.translation = (topple * t.translation.as_vec3()).as_dvec3() + lift;
        t.rotation = topple * t.rotation;
        app.world.get_mut::<Velocity>(p).unwrap().linear = Vec3::ZERO;
    }
    wait(&mut app, 0.5);
    let lying = bone(&app, e, "hips");
    assert!(
        lying.y < 0.3 && lying.z < -0.6,
        "lies face down ahead: {lying}"
    );

    app.world.resource_mut::<Said>().0.clear();
    app.world.get_mut::<Ragdoll>(e).unwrap().weight = 0.0;
    app.update();
    app.update();
    let rig = app.world.get::<RagdollRig>(e).unwrap();
    assert_eq!(rig.getting_up(), Some("get_up_front"));
    assert!(
        app.world
            .resource::<Said>()
            .0
            .iter()
            .any(|n| n == "getting_up")
    );
    // It starts from where it lay, not from its feet.
    let early = bone(&app, e, "hips");
    assert!(early.y < 0.45, "starts on the floor: {early}");

    // Halfway, it is still near where it lay and off the floor's surface.
    wait(&mut app, 0.9);
    let head = bone(&app, e, "head");
    assert!(head.y > 0.2, "head off the floor: {head}");
    wait(&mut app, 2.0 + GET_UP_BLEND_SECS as f64);
    assert!(app.world.resource::<Said>().0.iter().any(|n| n == "got_up"));
    let rig = app.world.get::<RagdollRig>(e).unwrap();
    assert!(!rig.is_blending_back() && rig.getting_up().is_none());
    let standing = bone(&app, e, "hips");
    let origin = app.world.get::<GlobalTransform>(e).unwrap().translation();
    assert!(
        standing.y > origin.y + 0.8
            && (standing.x - origin.x).abs() < 0.1
            && (standing.z - origin.z).abs() < 0.25,
        "stands on its entity: {standing} vs {origin}"
    );
}

#[test]
fn half_limp_bodies_are_pulled_toward_the_animation_and_held_up() {
    let mut app = app(true);
    let e = person(&mut app);
    wait(&mut app, 0.5);
    app.world.entity_mut(e).insert(Ragdoll { weight: 0.3 });
    app.update();
    let ps = parts(&mut app);
    let chest = ps.iter().find(|p| p.1.bone == "chest").unwrap().0;
    // Knock the chest back half a meter: it is pulled forward again and held up.
    app.world.get_mut::<Transform>(chest).unwrap().translation.z += 0.5;
    app.update();
    let f = app.world.get::<ExternalForce>(chest).unwrap();
    assert!(
        f.force.z < -100.0,
        "pulled back toward the pose: {}",
        f.force
    );
    assert!(f.force.y > 100.0, "held up against gravity: {}", f.force);
}

#[test]
fn hard_hits_sound_like_a_body_falling() {
    let mut app = app(true);
    let e = person(&mut app);
    app.world.entity_mut(e).insert(Ragdoll { weight: 1.0 });
    app.update();
    let ps = parts(&mut app);
    let head = ps.iter().find(|p| p.1.bone == "neck").unwrap().0;
    let ground = app.world.spawn(Transform::default()).id();
    app.world.resource_mut::<Heard>().0.clear();
    // Normals point from `a` toward `b`.
    for (impulse, a, b, normal) in [
        (30.0, head, ground, Vec3::NEG_Y),
        (0.5, ground, head, Vec3::Y),
    ] {
        app.world.write_message(ContactEvent {
            phase: ContactPhase::Started,
            a,
            b,
            point: DVec3::new(0.0, 0.1, 0.0),
            normal,
            impulse,
            sensor: false,
        });
    }
    app.update();
    let heard = &app.world.resource::<Heard>().0;
    let falls: Vec<_> = heard
        .iter()
        .filter(|c| c.kind == ContactKind::Body)
        .collect();
    assert_eq!(falls.len(), 1, "one hard hit, one soft touch: {falls:?}");
    assert_eq!(falls[0].entity, Some(e));
    assert!(falls[0].speed > 3.0, "{}", falls[0].speed);
    assert!(
        falls[0].normal.y > 0.9,
        "from the ground up: {}",
        falls[0].normal
    );
}

#[test]
fn without_physics_a_ragdoll_stays_animated() {
    let mut app = app(false);
    let e = person(&mut app);
    app.world.entity_mut(e).insert(Ragdoll { weight: 1.0 });
    wait(&mut app, 0.5);
    assert!(parts(&mut app).is_empty());
    assert!(!app.world.get::<RagdollRig>(e).unwrap().is_physical());
    assert!(bone(&app, e, "head").y > 1.4, "still standing");
}

#[test]
fn the_stuntman_climbs_the_deck_goes_limp_at_its_edge_and_comes_round_again() {
    use engine_anim::stunt::{DECK_HEIGHT, Stunt, StuntPhase, YARD, spawn_yard};
    let mut app = app(false);
    let e = spawn_yard(&mut app.world, DVec3::ZERO);
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    let phase = |app: &App| app.world.get::<Stunt>(e).unwrap().phase;
    let mut secs = 0.0;
    while phase(&app) == StuntPhase::Run && secs < 12.0 {
        app.update();
        secs += STEP;
    }
    // Up the stairs and across the deck: limp at its edge.
    assert_eq!(
        phase(&app),
        StuntPhase::Fall,
        "still running after {secs:.1} s"
    );
    let at = app.world.get::<Transform>(e).unwrap().translation;
    assert!(
        (at.y - DECK_HEIGHT as f64).abs() < 0.05,
        "on the deck: {at}"
    );
    assert!((at.x - YARD[0]).abs() < 0.2, "at its edge: {at}");
    app.update();
    assert_eq!(app.world.get::<Ragdoll>(e).map(|r| r.weight), Some(1.0));

    // Without physics nothing falls: he gets up again and walks back round to the start.
    wait(&mut app, 4.0);
    assert_eq!(phase(&app), StuntPhase::Back);
    assert_eq!(app.world.get::<Ragdoll>(e).map(|r| r.weight), Some(0.0));
    let feet = app.world.get::<GlobalTransform>(e).unwrap().translation();
    assert!(bone(&app, e, "head").y > feet.y + 1.4, "on his feet");
    secs = 0.0;
    while phase(&app) == StuntPhase::Back && secs < 20.0 {
        app.update();
        secs += STEP;
    }
    assert_eq!(phase(&app), StuntPhase::Run, "came round in {secs:.1} s");
    let at = app.world.get::<Transform>(e).unwrap().translation;
    assert!(at.y.abs() < 0.05, "back on the ground: {at}");
}

#[test]
fn dogs_and_horses_go_limp_at_their_weight_and_get_up_again() {
    let mut app = app(true);
    let mut animals = Vec::new();
    for (kind, height, x, kg) in [("dog", 0.56, -3.0, 30.0), ("horse", 1.6, 3.0, 500.0)] {
        let t = Transform::from_xyz(x, 0.0, 0.0);
        let e = app
            .world
            .spawn((
                Name::new(kind),
                t,
                GlobalTransform::from(t),
                BodyRequest::new(kind).with_height(height),
            ))
            .id();
        animals.push((kind, e, kg));
    }
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    for _ in 0..90 {
        for (_, e, _) in &animals {
            app.world.get_mut::<Transform>(*e).unwrap().translation.z -= 2.0 * STEP;
        }
        app.update();
    }
    // Where each body sits on its entity while it walks.
    let mut standing = Vec::new();
    for (_, e, _) in &animals {
        let origin = app.world.get::<GlobalTransform>(*e).unwrap().translation();
        standing.push(bone(&app, *e, "body") - origin);
        app.world.entity_mut(*e).insert(Ragdoll { weight: 1.0 });
    }
    app.update();
    for (kind, e, kg) in &animals {
        let mut q = app.world.query::<(Entity, &RagdollPart)>();
        let ps: Vec<Entity> = q
            .iter(&app.world)
            .filter(|(_, p)| p.owner == *e)
            .map(|(pe, _)| pe)
            .collect();
        assert_eq!(
            ps.len(),
            16,
            "{kind}: trunk, neck, head and four legs of three parts"
        );
        let mut mass = 0.0;
        let mut momentum = Vec3::ZERO;
        for p in &ps {
            let m = app.world.get::<RigidBody>(*p).unwrap().mass_kg.unwrap();
            mass += m;
            momentum += app.world.get::<Velocity>(*p).unwrap().linear * m;
        }
        // The imported horse may be a little bigger or smaller than the generated one.
        assert!((mass - kg).abs() < kg * 0.35, "{kind}: {mass} kg");
        let v = momentum / mass;
        assert!(v.z < -1.4 && v.z > -2.6, "{kind} moves on at {v}");
        let mut joints = app.world.query::<&Joint>();
        let joints: Vec<Joint> = joints
            .iter(&app.world)
            .filter(|j| ps.contains(&j.body_a))
            .cloned()
            .collect();
        assert_eq!(joints.len(), 15, "{kind}");
        for j in &joints {
            let a = app.world.get::<Transform>(j.body_a).unwrap();
            let b = app.world.get::<Transform>(j.body_b.unwrap()).unwrap();
            let gap = (a.translation + (a.rotation * j.anchor_a).as_dvec3())
                - (b.translation + (b.rotation * j.anchor_b.as_vec3()).as_dvec3());
            assert!(gap.length() < 1e-3, "{kind} joint apart by {gap}");
        }
    }
    // The pose follows the bodies, then blends back onto the entity.
    wait(&mut app, 0.5);
    for (kind, e, _) in &animals {
        let body = bone(&app, *e, "body");
        let origin = app.world.get::<GlobalTransform>(*e).unwrap().translation();
        assert!(
            body.z < origin.z - 0.5,
            "{kind} drifted on: {body} vs {origin}"
        );
        app.world.get_mut::<Ragdoll>(*e).unwrap().weight = 0.0;
    }
    wait(&mut app, BLEND_OUT_SECS as f64 + 0.2);
    for ((kind, e, _), was) in animals.iter().zip(standing) {
        let origin = app.world.get::<GlobalTransform>(*e).unwrap().translation();
        let body = bone(&app, *e, "body") - origin;
        assert!(
            (body - was).length() < 0.3,
            "{kind} back on its feet: {body} vs {was}"
        );
        assert!(!app.world.get::<RagdollRig>(*e).unwrap().is_physical());
    }
}
