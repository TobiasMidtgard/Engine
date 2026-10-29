//! A hard shove knocks a person back to the capture point (`HitReaction::stagger_*`) and
//! the feet step after the body, so it ends standing on them again; a light one doesn't
//! move it, and a character its movement moves (a character controller) only publishes the
//! knock-back. Runs
//! with or without the fetched models.

use engine_anim::{AnimationPlugin, FootIk, HitReaction};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{BodyRequest, SkeletonPose};
use engine_world::physics::CharacterController;
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;
const FEET: [&str; 2] = ["foot_l", "foot_r"];

fn person(reaction: Option<HitReaction>) -> (App, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let t = Transform::default();
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::human(0)))
        .id();
    if let Some(r) = reaction {
        app.world.entity_mut(e).insert(r);
    }
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

fn body(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// Shove `e` in the chest from the front (people face −Z) with `impulse` N·s.
fn shove(app: &mut App, e: Entity, impulse: f32) {
    let at = bone(app, e, "chest") + DVec3::new(0.0, 0.0, -0.12);
    app.world.write_message(DamageEvent {
        target: e,
        amount: 10.0,
        kind: DamageKind::Blunt,
        point: at,
        impulse: Vec3::new(0.0, 0.0, impulse),
        source: None,
    });
}

/// Each foot relative to the body, along the ground.
fn stance(app: &App, e: Entity) -> Vec<DVec3> {
    FEET.iter()
        .map(|f| (bone(app, e, f) - body(app, e)).with_y(0.0))
        .collect()
}

#[test]
fn a_hard_shove_knocks_the_body_back_and_the_feet_step_after_it() {
    let (mut app, e) = person(None);
    assert!(app.world.get::<FootIk>(e).is_some(), "people have foot IK");
    let start = body(&app, e);
    let stance_before = stance(&app, e);
    let ground: Vec<f64> = FEET.iter().map(|f| bone(&app, e, f).y).collect();
    shove(&mut app, e, 40.0);
    let mut lifted = [0.0f64; 2];
    let mut hurried = false;
    for _ in 0..150 {
        app.update();
        for (n, f) in FEET.iter().enumerate() {
            lifted[n] = lifted[n].max(bone(&app, e, f).y - ground[n]);
        }
        hurried |= app.world.get::<FootIk>(e).unwrap().hurry > 0.0;
    }
    let moved = body(&app, e) - start;
    assert!(
        moved.z > 0.1 && moved.z < 0.5 && moved.x.abs() < 0.02,
        "knocked back by {moved}"
    );
    assert!(hurried, "the feet weren't hurried");
    // Both feet stepped (lifted in an arc) and stand under the body again.
    for (n, f) in FEET.iter().enumerate() {
        assert!(lifted[n] > 0.02, "{f} lifted only {:.3} m", lifted[n]);
    }
    // The body stands over its feet again, each held within foot IK's reach of its place
    // in the stance (an idle's own weight shifts move those places a little).
    let after = stance(&app, e);
    let mid = |s: &[DVec3]| (s[0] + s[1]) * 0.5;
    let off = (mid(&after) - mid(&stance_before)).length();
    assert!(
        off < 0.08,
        "the feet ended {off:.3} m off-centre under the body"
    );
    for ((f, a), b) in FEET.iter().zip(&stance_before).zip(&after) {
        let off = (*a - *b).length();
        assert!(
            off < 0.15,
            "{f} ended {off:.3} m from its place in the stance"
        );
    }
    let r = app.world.get::<HitReaction>(e).unwrap();
    assert_eq!(r.stagger.0, Vec3::ZERO, "the knock-back ran out");
    assert_eq!(r.stagger_velocity, Vec3::ZERO);
}

#[test]
fn a_light_shove_does_not_step() {
    let (mut app, e) = person(None);
    let start = body(&app, e);
    let feet: Vec<DVec3> = FEET.iter().map(|f| bone(&app, e, f)).collect();
    shove(&mut app, e, 10.0);
    for _ in 0..60 {
        app.update();
        assert_eq!(app.world.get::<FootIk>(e).unwrap().hurry, 0.0);
    }
    assert!((body(&app, e) - start).length() < 1e-6);
    for (f, was) in FEET.iter().zip(feet) {
        let moved = (bone(&app, e, f) - was).length();
        assert!(moved < 0.01, "{f} moved {moved:.3} m");
    }
}

#[test]
fn a_character_moved_by_its_movement_module_only_publishes_the_knock_back() {
    // Told not to move itself, or moved by a character controller.
    for controller in [false, true] {
        let (mut app, e) = person(Some(HitReaction {
            stagger_move: controller,
            ..Default::default()
        }));
        if controller {
            app.world
                .entity_mut(e)
                .insert(CharacterController::default());
        }
        let start = body(&app, e);
        shove(&mut app, e, 40.0);
        let mut fastest = 0.0f32;
        let mut asked = 0.0f32;
        for _ in 0..90 {
            app.update();
            let v = app.world.get::<HitReaction>(e).unwrap().stagger_velocity;
            fastest = fastest.max(v.z);
            asked += v.z * STEP as f32;
        }
        assert!((body(&app, e) - start).length() < 1e-6, "it moved itself");
        assert!(fastest > 0.5, "asked for only {fastest:.2} m/s");
        assert!(asked > 0.1 && asked < 0.5, "asked to move {asked:.3} m");
    }
}
