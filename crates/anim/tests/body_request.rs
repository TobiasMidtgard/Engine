//! Bodies on request: a module moves an entity itself and asks for a person or an animal on
//! it ([`BodyRequest`]). The body appears as a `SkeletonPose`, animates from the entity's
//! motion, steps on the ground, and goes again with the request. Runs with or without the
//! fetched models.

use engine_anim::{AnimationPlugin, Body, BodyKind, HorseModel, LeggedGait};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{AnimationEvent, AnimationParams, BodyRequest, SkeletonPose};
use engine_world::{ContactKind, GroundContact, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

/// Footsteps heard: (entity, contact kind, speed) and (entity, event name, bone).
#[derive(Resource, Default)]
struct Heard {
    contacts: Vec<(Entity, ContactKind, f32)>,
    events: Vec<(Entity, String, Option<String>)>,
}

fn listen(
    mut heard: ResMut<Heard>,
    mut contacts: MessageReader<GroundContact>,
    mut events: MessageReader<AnimationEvent>,
) {
    for c in contacts.read() {
        if let Some(e) = c.entity {
            heard.contacts.push((e, c.kind, c.speed));
        }
    }
    for e in events.read() {
        heard
            .events
            .push((e.entity, e.name.clone(), e.bone.clone()));
    }
}

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app.world.init_resource::<Heard>();
    app.add_systems(Last, listen);
    app
}

fn requester(app: &mut App, name: &'static str, request: BodyRequest) -> Entity {
    let t = Transform::default();
    app.world
        .spawn((Name::new(name), t, GlobalTransform::from(t), request))
        .id()
}

/// Move `e` straight ahead (−Z) at `speed` for `secs`, the way a requester would.
fn walk(app: &mut App, e: Entity, speed: f64, secs: f64) {
    for _ in 0..(secs / STEP) as usize {
        app.world.get_mut::<Transform>(e).unwrap().translation.z -= speed * STEP;
        app.update();
    }
}

/// Speeds the footstep contacts of `e` reported.
fn step_speeds(app: &App, e: Entity) -> Vec<f32> {
    let heard = app.world.resource::<Heard>();
    heard
        .contacts
        .iter()
        .filter(|c| c.0 == e)
        .map(|c| c.2)
        .collect()
}

fn footsteps(app: &mut App, e: Entity) -> (usize, usize) {
    let mut heard = app.world.resource_mut::<Heard>();
    let contacts = heard.contacts.iter().filter(|c| c.0 == e).count();
    let events = heard
        .events
        .iter()
        .filter(|ev| ev.0 == e && ev.1.starts_with("footstep") && ev.2.is_some())
        .count();
    *heard = Heard::default();
    (contacts, events)
}

#[test]
fn a_hiker_gets_a_body_that_walks_runs_and_goes_again() {
    let mut app = app();
    let hiker = requester(&mut app, "Trail hiker", BodyRequest::human(0));
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    let w = &app.world;
    assert!(
        w.get::<SkeletonPose>(hiker).is_some(),
        "the body answers with a pose"
    );
    assert_eq!(w.get::<Body>(hiker).map(|b| b.kind), Some(BodyKind::Human));

    walk(&mut app, hiker, 1.4, 1.0);
    footsteps(&mut app, hiker);
    walk(&mut app, hiker, 1.4, 3.0);
    let speed = app
        .world
        .get::<AnimationParams>(hiker)
        .unwrap()
        .get("speed");
    assert!((speed - 1.4).abs() < 0.15, "walking speed {speed}");
    // Each step carries the walking speed (footstep sounds key on it).
    let speeds = step_speeds(&app, hiker);
    assert!(
        speeds.iter().all(|s| (s - 1.4).abs() < 0.3),
        "walking step speeds {speeds:?}"
    );
    let (contacts, events) = footsteps(&mut app, hiker);
    assert!(
        contacts >= 4,
        "{contacts} footstep contacts in 3 s of walking"
    );
    assert!(events >= 4, "{events} footstep events in 3 s of walking");

    walk(&mut app, hiker, 4.0, 3.0);
    let speed = app
        .world
        .get::<AnimationParams>(hiker)
        .unwrap()
        .get("speed");
    assert!((speed - 4.0).abs() < 0.3, "running speed {speed}");
    let speeds = step_speeds(&app, hiker);
    assert!(
        speeds.iter().filter(|s| **s > 3.5).count() * 2 > speeds.len(),
        "running step speeds {speeds:?}"
    );
    let (contacts, _) = footsteps(&mut app, hiker);
    assert!(
        contacts >= 6,
        "{contacts} footstep contacts in 3 s of running"
    );
    // The body never moved the entity: it is where the requester put it.
    let z = app.world.get::<Transform>(hiker).unwrap().translation.z;
    assert!((z + 1.4 * 4.0 + 4.0 * 3.0).abs() < 0.05, "entity at z {z}");

    app.world.entity_mut(hiker).remove::<BodyRequest>();
    app.update();
    let w = &app.world;
    assert!(
        w.get::<SkeletonPose>(hiker).is_none(),
        "the pose goes with the request"
    );
    assert!(w.get::<Body>(hiker).is_none());
    assert!(
        w.get::<AnimationParams>(hiker).is_none(),
        "added parameters go too"
    );
    assert!(w.get::<Name>(hiker).is_some(), "the entity stays");
    walk(&mut app, hiker, 1.4, 1.0);
    assert_eq!(footsteps(&mut app, hiker), (0, 0), "no body, no footsteps");
}

#[test]
fn a_horse_on_request_gallops_from_its_motion() {
    let mut app = app();
    let horse = requester(
        &mut app,
        "Trail horse",
        BodyRequest::new("horse").with_height(1.5),
    );
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    assert_eq!(
        app.world.get::<Body>(horse).map(|b| b.kind),
        Some(BodyKind::Horse)
    );
    // The imported horse (when fetched) is scaled to the requested shoulder height.
    if let Some(model) = app.world.get::<HorseModel>(horse) {
        assert!(
            (model.scale - 1.5 / 1.9).abs() < 1e-4,
            "scale {}",
            model.scale
        );
    }
    walk(&mut app, horse, 1.5, 3.0);
    let walking = app
        .world
        .get::<LeggedGait>(horse)
        .unwrap()
        .stage_name()
        .to_string();
    footsteps(&mut app, horse);
    walk(&mut app, horse, 10.0, 4.0);
    let gait = app.world.get::<LeggedGait>(horse).unwrap();
    assert_eq!(walking, "walk");
    assert_eq!(gait.stage_name(), "gallop", "at {} m/s", gait.speed());
    let heard = app.world.resource::<Heard>();
    assert!(
        heard
            .contacts
            .iter()
            .any(|c| c.0 == horse && c.1 == ContactKind::Hoof),
        "hoof falls reach the ground"
    );
    let (_, events) = footsteps(&mut app, horse);
    assert!(events >= 8, "{events} hoof-fall events in 4 s of galloping");
}

#[test]
fn a_dog_on_request_is_built_at_its_height_and_gallops() {
    let mut app = app();
    let dog = requester(
        &mut app,
        "Trail dog",
        BodyRequest::new("dog").with_height(0.4).with_seed(4),
    );
    app.update();
    let w = &app.world;
    assert_eq!(w.get::<Body>(dog).map(|b| b.kind), Some(BodyKind::Dog));
    assert_eq!(
        w.get::<engine_anim::AnimatorSettings>(dog)
            .map(|s| s.skeleton.as_str()),
        Some("dog@40cm"),
        "a skeleton of the asked size"
    );
    let paw = w
        .get::<SkeletonPose>(dog)
        .and_then(|p| Some(p.bones[p.index_of("front_paw_l")?].translation.y))
        .expect("front paw in the pose");
    assert!((paw - 0.035 * 0.4 / 0.56).abs() < 0.01, "paw at {paw}");
    let fur = w
        .get::<engine_anim::SkinnedMeshRenderer>(dog)
        .map(|r| r.material.fur_length)
        .unwrap_or(0.0);
    assert!(fur > 0.005, "a furred coat ({fur} m)");

    walk(&mut app, dog, 0.9, 3.0);
    let walking = app
        .world
        .get::<LeggedGait>(dog)
        .unwrap()
        .stage_name()
        .to_string();
    footsteps(&mut app, dog);
    walk(&mut app, dog, 8.0, 3.0);
    let gait = app.world.get::<LeggedGait>(dog).unwrap();
    assert_eq!(walking, "walk");
    assert_eq!(gait.stage_name(), "gallop", "at {} m/s", gait.speed());
    let heard = app.world.resource::<Heard>();
    assert!(
        heard
            .contacts
            .iter()
            .any(|c| c.0 == dog && c.1 == ContactKind::Paw),
        "paw falls reach the ground"
    );
    let (_, events) = footsteps(&mut app, dog);
    assert!(events >= 12, "{events} paw-fall events in 3 s of galloping");
}

#[test]
fn unknown_kinds_get_no_body_and_set_parameters_stay_with_the_requester() {
    let mut app = app();
    let dragon = requester(&mut app, "Dragon", BodyRequest::new("dragon"));
    let mut params = AnimationParams::default();
    params.set("speed", 0.0);
    let t = Transform::default();
    let mime = app
        .world
        .spawn((
            Name::new("Mime"),
            t,
            GlobalTransform::from(t),
            params,
            BodyRequest::human(1),
        ))
        .id();
    // Walking on the spot: the requester says speed 0 while the entity turns and slides.
    for _ in 0..90 {
        let mut t = app.world.get_mut::<Transform>(mime).unwrap();
        t.translation.z -= 2.0 * STEP;
        t.rotate_y(0.5 * STEP as f32);
        app.update();
    }
    let w = &app.world;
    assert!(
        w.get::<SkeletonPose>(dragon).is_none(),
        "no dragon body: the stand-in stays"
    );
    let p = w.get::<AnimationParams>(mime).unwrap();
    assert_eq!(p.get("speed"), 0.0, "the requester's speed stays");
    assert!(
        p.get("turn") > 0.1,
        "turning still leans: {}",
        p.get("turn")
    );

    // A changed request builds the body again.
    app.world
        .entity_mut(mime)
        .insert(BodyRequest::new("spider"));
    app.update();
    let w = &app.world;
    assert_eq!(w.get::<Body>(mime).map(|b| b.kind), Some(BodyKind::Spider));
    assert!(w.get::<LeggedGait>(mime).is_some());
    assert!(
        w.get::<engine_anim::FootIk>(mime).is_none(),
        "the person's feet are gone"
    );
    assert!(
        w.get::<AnimationParams>(mime).is_some(),
        "the requester's parameters stay"
    );
}
