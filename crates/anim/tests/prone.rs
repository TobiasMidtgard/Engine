//! People lie prone and crawl (Movement's traversal list: crawling under low cover). Callers
//! set the contract-style `prone` parameter to 0 or 1; the body goes down through hands and
//! knees, lies propped on its forearms, crawls on with a forearm and the opposite knee
//! planted in turn, and gets up the same way.

use engine_anim::AnimationPlugin;
use engine_anim::build::humanoid::{PRONE_SECS, PRONED};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{AnimationParams, BodyRequest, SkeletonPose};

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

fn person(app: &mut App) -> Entity {
    let t = Transform::default();
    app.world
        .spawn((
            t,
            GlobalTransform::from(t),
            BodyRequest::new("human").with_seed(3),
            AnimationParams::default(),
        ))
        .id()
}

fn run(app: &mut App, secs: f64) {
    for _ in 0..(secs / STEP).round() as usize {
        app.update();
    }
}

fn bone(app: &App, e: Entity, name: &str) -> Vec3 {
    let p = app.world.get::<SkeletonPose>(e).expect("a pose");
    p.bones[p.index_of(name).expect(name)].translation
}

/// The lowest of every bone, m.
fn lowest(app: &App, e: Entity) -> f32 {
    let p = app.world.get::<SkeletonPose>(e).expect("a pose");
    p.bones
        .iter()
        .map(|b| b.translation.y)
        .fold(f32::MAX, f32::min)
}

fn prone(app: &mut App, e: Entity, on: bool) {
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set("prone", if on { 1.0 } else { 0.0 });
}

#[test]
fn a_person_goes_down_through_hands_and_knees_lies_flat_and_gets_up_again() {
    let mut app = app();
    let e = person(&mut app);
    run(&mut app, 1.0);
    assert!(bone(&app, e, "head").y > 1.5, "not standing at first");

    prone(&mut app, e, true);
    // Halfway down: on hands and knees.
    run(&mut app, PRONE_SECS as f64 * 0.5);
    let proned = app.world.get::<AnimationParams>(e).unwrap().get(PRONED);
    assert!((proned - 0.5).abs() < 0.08, "proned {proned} halfway");
    let hips = bone(&app, e, "hips").y;
    let hands = bone(&app, e, "hand_l").y.max(bone(&app, e, "hand_r").y);
    assert!(
        (0.35..0.7).contains(&hips) && hands < 0.15,
        "not on hands and knees halfway down: hips {hips}, hands {hands}"
    );

    run(&mut app, PRONE_SECS as f64);
    let (hips, head, floor) = (
        bone(&app, e, "hips").y,
        bone(&app, e, "head").y,
        lowest(&app, e),
    );
    assert!(hips < 0.25, "hips {hips} lying");
    assert!(head < 0.6, "head {head} lying (under a 0.7 m gap)");
    assert!(floor > -0.03, "a bone {floor} under the floor lying");

    prone(&mut app, e, false);
    run(&mut app, PRONE_SECS as f64 + 1.0);
    let head = bone(&app, e, "head").y;
    assert!(head > 1.5, "head {head}: not up again");
}

#[test]
fn a_crawler_pulls_on_a_planted_forearm_and_knee_in_turn() {
    let mut app = app();
    let e = person(&mut app);
    run(&mut app, 0.5);
    prone(&mut app, e, true);
    run(&mut app, PRONE_SECS as f64 + 0.5);
    let speed = 0.45;
    // Settle into the crawl, then watch two cycles.
    let crawl = |app: &mut App| {
        app.world.get_mut::<Transform>(e).unwrap().translation.z -= speed * STEP;
        app.update();
    };
    for _ in 0..90 {
        crawl(&mut app);
    }
    let world = |app: &App, name: &str| {
        let at = app.world.get::<Transform>(e).unwrap().translation.as_vec3();
        at + bone(app, e, name)
    };
    let limbs = ["hand_l", "hand_r", "foot_l", "foot_r"];
    let mut last = limbs.map(|l| world(&app, l));
    let mut still = [0usize; 4];
    let mut reach = [[f32::MAX, f32::MIN]; 4];
    let mut floor = f32::MAX;
    let frames = 192;
    for _ in 0..frames {
        crawl(&mut app);
        floor = floor.min(lowest(&app, e));
        for (i, l) in limbs.iter().enumerate() {
            let now = world(&app, l);
            if (now - last[i]).length() / (STEP as f32) < 0.15 {
                still[i] += 1;
            }
            let local = bone(&app, e, l).z;
            reach[i] = [reach[i][0].min(local), reach[i][1].max(local)];
            last[i] = now;
        }
    }
    for (i, l) in limbs.iter().enumerate() {
        let share = still[i] as f32 / frames as f32;
        assert!(
            share > 0.3,
            "{l} still on the ground only {share:.2} of the time"
        );
        let swing = reach[i][1] - reach[i][0];
        assert!(swing > 0.25, "{l} moves only {swing} m along the body");
    }
    assert!(floor > -0.03, "a bone {floor} under the floor crawling");
    assert!(bone(&app, e, "head").y < 0.6, "head up crawling");
}
