//! People crouch (NPC AI's ask): an archer down behind a waist-high wall holding its draw, a
//! player creeping along a low wall. Callers set the contract's `crouch` (or a mover's
//! `MoveIntent::crouch`, or `LocomotionMode::Crouch`) to 0 or 1; the body sinks into a
//! crouch, creeps, hurries and steps aside in it, and rises again.

use engine_anim::build::humanoid::CROUCHED;
use engine_anim::build::weapons::{BOW_DRAW, BOW_HOLD};
use engine_anim::{AnimationPlugin, Animator};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{AnimationParams, BodyRequest, SkeletonPose};
use engine_world::{LocomotionMode, LocomotionState, MoveIntent, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app
}

fn person(app: &mut App, x: f64) -> Entity {
    let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0));
    app.world
        .spawn((
            t,
            GlobalTransform::from(t),
            BodyRequest::new("human").with_seed(3),
            AnimationParams::default(),
            MoveIntent::default(),
        ))
        .id()
}

fn run(app: &mut App, secs: f64) {
    for _ in 0..(secs / STEP).round() as usize {
        app.update();
    }
}

/// Walk `e` at `speed` m/s for `secs`, `dir` (x left.., z ahead..) relative to where it faces
/// (−Z), reporting the lowest and highest heights each foot reached.
fn walk(app: &mut App, e: Entity, dir: DVec3, speed: f64, secs: f64) -> [[f32; 2]; 2] {
    let mut feet = [[f32::MAX, f32::MIN]; 2];
    let along = DVec3::new(-dir.x, 0.0, -dir.z).normalize();
    for _ in 0..(secs / STEP).round() as usize {
        app.world.get_mut::<Transform>(e).unwrap().translation += along * speed * STEP;
        app.update();
        for (i, foot) in ["foot_l", "foot_r"].into_iter().enumerate() {
            let y = bone(app, e, foot).y;
            feet[i] = [feet[i][0].min(y), feet[i][1].max(y)];
        }
    }
    feet
}

fn bone(app: &App, e: Entity, name: &str) -> Vec3 {
    let p = app.world.get::<SkeletonPose>(e).expect("a pose");
    p.bones[p.index_of(name).expect(name)].translation
}

fn crouch(app: &mut App, e: Entity, on: bool) {
    app.world.get_mut::<MoveIntent>(e).unwrap().crouch = on;
}

fn crouched(app: &App, e: Entity) -> f32 {
    app.world.get::<AnimationParams>(e).unwrap().get(CROUCHED)
}

#[test]
fn a_person_sinks_into_a_crouch_low_enough_to_hide_and_creeps_hurries_and_steps_aside_in_it() {
    let mut app = app();
    let e = person(&mut app, 0.0);
    run(&mut app, 1.0);
    let standing = bone(&app, e, "head").y;
    assert!(standing > 1.5, "standing head at {standing}");

    crouch(&mut app, e, true);
    // It sinks into it rather than snapping down...
    run(&mut app, 0.05);
    let partway = crouched(&app, e);
    assert!(
        partway > 0.1 && partway < 0.6,
        "crouched {partway} after 0.05 s"
    );
    // ...and is down in well under a second, its head under the top of a waist-high wall.
    run(&mut app, 0.6);
    assert!(crouched(&app, e) > 0.95);
    let low = bone(&app, e, "head").y;
    assert!(low < 1.12, "crouched head at {low} (standing {standing})");
    let hips = bone(&app, e, "hips").y;
    assert!(hips < 0.7, "crouched hips at {hips}");
    // Feet on the ground, not pulled up into the body: the left flat ahead, the right
    // behind up on its ball.
    let [left, right] = ["foot_l", "foot_r"].map(|foot| bone(&app, e, foot).y);
    assert!(left < 0.11, "left ankle at {left} crouched");
    assert!(
        right > left + 0.01 && right < 0.2,
        "right ankle at {right} crouched"
    );

    // Creeping ahead, hurrying low, backing off and stepping aside: low the whole way, the
    // feet lifting in turn and coming back down.
    for (what, dir, speed) in [
        ("creeping", DVec3::Z, 1.1),
        ("hurrying", DVec3::Z, 2.4),
        ("backing", -DVec3::Z, 0.8),
        ("stepping left", DVec3::X, 0.8),
        ("stepping right", -DVec3::X, 0.8),
    ] {
        let feet = walk(&mut app, e, dir, speed, 2.0);
        let head = bone(&app, e, "head").y;
        assert!(head < 1.2, "{what}: head at {head}");
        for (side, [lo, hi]) in feet.into_iter().enumerate() {
            assert!(lo < 0.12, "{what}: foot {side} never down ({lo})");
            assert!(
                hi - lo > 0.03,
                "{what}: foot {side} never lifts ({lo}..{hi})"
            );
        }
    }

    // Standing up again.
    crouch(&mut app, e, false);
    run(&mut app, 1.0);
    let up = bone(&app, e, "head").y;
    assert!(
        up > standing - 0.03,
        "head back at {up} (stood at {standing})"
    );
}

#[test]
fn a_crouched_walker_keeps_its_knees_well_off_the_ground_and_its_head_low() {
    let mut app = app();
    let e = person(&mut app, 0.0);
    crouch(&mut app, e, true);
    run(&mut app, 1.0);
    for (what, dir, speed) in [
        ("creeping", DVec3::Z, 1.1),
        ("hurrying", DVec3::Z, 2.4),
        ("backing", -DVec3::Z, 0.8),
        ("stepping left", DVec3::X, 0.8),
    ] {
        // Into the gait first, then the lowest knee and highest head over two strides.
        walk(&mut app, e, dir, speed, 0.7);
        let along = DVec3::new(-dir.x, 0.0, -dir.z);
        let (mut knee, mut head) = (f32::MAX, f32::MIN);
        for _ in 0..90 {
            app.world.get_mut::<Transform>(e).unwrap().translation += along * speed * STEP;
            app.update();
            for b in ["lower_leg_l", "lower_leg_r"] {
                knee = knee.min(bone(&app, e, b).y);
            }
            head = head.max(bone(&app, e, "head").y);
        }
        assert!(knee > 0.16, "{what}: a knee down at {knee}");
        assert!(head < 1.15, "{what}: head up at {head}");
    }
}

#[test]
fn a_mover_reporting_a_crouch_crouches_its_body_too() {
    let mut app = app();
    let e = person(&mut app, 0.0);
    run(&mut app, 0.5);
    let standing = bone(&app, e, "head").y;
    app.world.entity_mut(e).insert(LocomotionState {
        mode: LocomotionMode::Crouch,
        grounded: true,
        ..Default::default()
    });
    run(&mut app, 1.0);
    assert!(bone(&app, e, "head").y < standing - 0.4);
    // And the contract's own `crouch` parameter, set by whoever moves it.
    let other = person(&mut app, 3.0);
    run(&mut app, 0.5);
    app.world
        .get_mut::<AnimationParams>(other)
        .unwrap()
        .set("crouch", 1.0);
    run(&mut app, 1.0);
    assert!(bone(&app, other, "head").y < standing - 0.4);
}

#[test]
fn an_archer_draws_and_holds_its_bow_crouched() {
    let mut app = app();
    let e = person(&mut app, 0.0);
    run(&mut app, 0.5);
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set("bow_carry", 1.0);
    crouch(&mut app, e, true);
    run(&mut app, 1.0);
    let low = bone(&app, e, "head").y;
    app.world.get_mut::<Animator>(e).unwrap().trigger("draw");
    run(&mut app, 0.3);
    let states = app
        .world
        .get::<Animator>(e)
        .unwrap()
        .graph()
        .unwrap()
        .active_states();
    assert!(states.iter().any(|s| s == BOW_DRAW), "{states:?}");
    run(&mut app, 1.5);
    let states = app
        .world
        .get::<Animator>(e)
        .unwrap()
        .graph()
        .unwrap()
        .active_states();
    assert!(states.iter().any(|s| s == BOW_HOLD), "{states:?}");
    // Still down: the draw is the upper body's, the crouch the legs'.
    let holding = bone(&app, e, "head").y;
    assert!(
        holding < low + 0.12,
        "holding the draw the head rose from {low} to {holding}"
    );
    assert!(bone(&app, e, "hips").y < 0.7);
    // Standing up with the bow still drawn.
    crouch(&mut app, e, false);
    run(&mut app, 1.0);
    assert!(bone(&app, e, "head").y > low + 0.4);
}

#[test]
fn a_fighter_on_a_ruined_knee_kneels_on_it_even_when_told_to_crouch() {
    use engine_world::combat::{BodyInjuries, BodyZone};
    let mut app = app();
    let kneeling = [person(&mut app, 0.0), person(&mut app, 3.0)];
    run(&mut app, 0.5);
    for &e in &kneeling {
        let mut inj = BodyInjuries::default();
        inj.get_mut(BodyZone::KneeL).wound = 0.7;
        app.world.entity_mut(e).insert(inj);
    }
    // NPC AI keeps a fighter on a bad knee crouched, so it doesn't chase.
    crouch(&mut app, kneeling[1], true);
    run(&mut app, 1.5);
    let [hips, crouched] = kneeling.map(|e| bone(&app, e, "hips").y);
    assert!(
        (hips - crouched).abs() < 0.03,
        "kneeling hips at {hips}, kneeling told to crouch at {crouched}"
    );
    let knee = bone(&app, kneeling[1], "lower_leg_l").y;
    assert!(knee < 0.15, "the bad knee on the ground: {knee}");
}
