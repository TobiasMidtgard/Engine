//! Where a blow lands matters: head blows add up until the legs go to rubber, a blow to the
//! stomach doubles the body over with the hands covering it, and a liver shot folds the body
//! onto a knee a moment after it lands, then it stands again. Runs with or without the
//! fetched models.

use engine_anim::{AnimationPlugin, Animator, Hurt};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::BodyRequest;
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

fn person() -> (App, Entity) {
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

/// A bone in the person's own frame (they face −Z, +X is their right), m.
fn local(app: &App, e: Entity, bone: &str) -> Vec3 {
    let a = app.world.get::<Animator>(e).unwrap();
    a.model[a.skeleton.bone(bone)].t
}

/// Strike `e` from the front at `at` (its own frame) with `impulse` N·s.
fn strike(app: &mut App, e: Entity, at: Vec3, impulse: f32) {
    let gt = *app.world.get::<GlobalTransform>(e).unwrap();
    app.world.write_message(DamageEvent {
        target: e,
        amount: 10.0,
        kind: DamageKind::Blunt,
        point: gt.transform_point(at.as_dvec3()),
        impulse: gt.rotation() * Vec3::new(0.0, 0.0, impulse),
        source: None,
    });
}

fn face(app: &App, e: Entity) -> Vec3 {
    local(app, e, "head") + Vec3::new(0.0, 0.07, -0.1)
}

fn in_state(app: &App, e: Entity, state: &str) -> bool {
    let a = app.world.get::<Animator>(e).unwrap();
    a.graph().is_some_and(|g| g.in_state(state))
}

#[test]
fn head_blows_add_up_until_the_legs_go_and_it_wears_off() {
    let (mut app, e) = person();
    let standing = local(&app, e, "hips");
    // Three blows to the chin in quick succession.
    for _ in 0..3 {
        let at = face(&app, e);
        strike(&mut app, e, at, 36.0);
        wait(&mut app, 0.4);
    }
    let hurt = app.world.get::<Hurt>(e).expect("hurt").clone();
    assert!(
        hurt.rocked() > 0.3 && hurt.guard_down(),
        "daze {:.2}",
        hurt.daze
    );
    assert_eq!(hurt.wind, 0.0, "head blows winded it");
    // Rubber legs: the hips sway from side to side and dip, and the body lurches about.
    let start = app.world.get::<Transform>(e).unwrap().translation;
    let (mut left, mut right, mut low) = (f32::MAX, f32::MIN, f32::MAX);
    let mut wandered = 0.0f64;
    for _ in 0..(2.5 / STEP) as usize {
        app.update();
        let hips = local(&app, e, "hips");
        left = left.min(hips.x);
        right = right.max(hips.x);
        low = low.min(hips.y);
        let at = app.world.get::<Transform>(e).unwrap().translation;
        wandered = wandered.max((at - start).length());
    }
    assert!(right - left > 0.05, "hips swayed {:.3} m", right - left);
    assert!(low < standing.y - 0.02, "knees didn't dip: {low:.3}");
    assert!(low > standing.y - 0.15, "sagged to {low:.3}");
    assert!(wandered > 0.05, "never lurched ({wandered:.3} m)");
    // It wears off.
    wait(&mut app, 6.0);
    let hurt = app.world.get::<Hurt>(e).unwrap();
    assert!(
        hurt.rocked() == 0.0 && !hurt.guard_down(),
        "daze {:.2}",
        hurt.daze
    );
}

#[test]
fn a_blow_to_the_stomach_doubles_the_body_over_with_the_hands_covering_it() {
    let (mut app, e) = person();
    let hips = local(&app, e, "hips");
    let lean = (local(&app, e, "neck") - local(&app, e, "chest")).z;
    let hands = (local(&app, e, "hand_l").z + local(&app, e, "hand_r").z) * 0.5;
    strike(&mut app, e, hips + Vec3::new(0.0, 0.18, -0.15), 36.0);
    wait(&mut app, 0.8);
    let hurt = app.world.get::<Hurt>(e).expect("hurt");
    assert!(
        hurt.winded() > 0.5 && hurt.guard_down(),
        "wind {:.2}",
        hurt.wind
    );
    assert_eq!(hurt.daze, 0.0);
    let folded = (local(&app, e, "neck") - local(&app, e, "chest")).z;
    assert!(folded < lean - 0.03, "folded from {lean:.3} to {folded:.3}");
    // Both hands in front of the stomach.
    let belly = local(&app, e, "hips") + Vec3::new(0.0, 0.2, -0.17);
    for h in ["hand_l", "hand_r"] {
        let at = local(&app, e, h);
        assert!(at.distance(belly) < 0.2, "{h} at {at}, belly at {belly}");
    }
    let covered = (local(&app, e, "hand_l").z + local(&app, e, "hand_r").z) * 0.5;
    assert!(
        covered < hands - 0.08,
        "hands from z {hands:.2} to {covered:.2}"
    );
    // The wind comes back.
    wait(&mut app, 4.0);
    assert_eq!(app.world.get::<Hurt>(e).unwrap().winded(), 0.0);
}

#[test]
fn a_liver_shot_folds_the_body_onto_a_knee_a_moment_later_and_it_gets_up() {
    let (mut app, e) = person();
    let standing = local(&app, e, "hips").y;
    // Under the ribs on its own right.
    let liver = local(&app, e, "spine") + Vec3::new(0.1, 0.0, -0.12);
    strike(&mut app, e, liver, 40.0);
    wait(&mut app, 0.25);
    assert!(
        app.world.get::<Hurt>(e).unwrap().liver.is_some(),
        "no liver shot"
    );
    assert!(!in_state(&app, e, "kneel_hurt"), "went down at once");
    assert!(local(&app, e, "hips").y > standing - 0.12);
    // Down on a knee.
    let mut lowest = f32::MAX;
    let mut knelt = false;
    for _ in 0..(1.5 / STEP) as usize {
        app.update();
        lowest = lowest.min(local(&app, e, "hips").y);
        knelt |= in_state(&app, e, "kneel_hurt");
    }
    assert!(knelt, "never knelt");
    assert!(app.world.get::<Hurt>(e).unwrap().is_down());
    assert!(
        lowest < standing * 0.72,
        "hips only down to {lowest:.2} of {standing:.2}"
    );
    // And up again.
    wait(&mut app, 4.0);
    let hurt = app.world.get::<Hurt>(e).unwrap();
    assert!(!hurt.is_down() && !in_state(&app, e, "kneel_hurt"));
    let hips = local(&app, e, "hips").y;
    assert!(
        hips > standing - 0.06,
        "stands at {hips:.2} of {standing:.2}"
    );
}
