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

/// The muzzle's way and how far the left fist is from the fore grip (model space).
fn gun(app: &App, e: Entity) -> (Vec3, f32) {
    use engine_anim::build::weapons::{RIFLE_AXIS, RIFLE_FORE};
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    let bone = |b: &str| pose.bones[pose.index_of(b).unwrap()];
    let fist =
        |b: engine_world::animation::BonePose| b.translation + b.rotation * engine_anim::grip::GRIP;
    let (r, l) = (bone("hand_r"), bone("hand_l"));
    let muzzle = r.rotation * RIFLE_AXIS;
    let fore = fist(r) + muzzle * RIFLE_FORE;
    (muzzle, (fist(l) - fore).length())
}

#[derive(Resource, Default)]
struct Said(Vec<String>);

fn listen(
    mut said: ResMut<Said>,
    mut events: MessageReader<engine_world::animation::AnimationEvent>,
) {
    said.0.extend(events.read().map(|e| e.name.clone()));
}

#[test]
fn lying_down_a_rifleman_aims_on_both_elbows_and_shoots_without_getting_up() {
    use engine_anim::build::humanoid::PRONE_AIMED;
    use engine_world::combat::{ItemClass, Loadout};
    let mut app = app();
    app.world.init_resource::<Said>();
    app.add_systems(Last, listen);
    let e = person(&mut app);
    app.world.entity_mut(e).insert(Loadout {
        main: ItemClass::Firearm,
        off: ItemClass::None,
        two_handed: true,
    });
    run(&mut app, 0.5);
    prone(&mut app, e, true);
    run(&mut app, PRONE_SECS as f64 + 0.5);
    // Lying still with it: in the shoulder, the muzzle down off the target.
    let (muzzle, off) = gun(&app, e);
    assert!(
        (-0.3..-0.08).contains(&muzzle.y) && muzzle.z < -0.9 && off < 0.06,
        "ready lying: muzzle {muzzle}, left fist {off} m off the fore grip"
    );
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set("aim", 1.0);
    run(&mut app, 1.0);
    let aimed = app
        .world
        .get::<AnimationParams>(e)
        .unwrap()
        .get(PRONE_AIMED);
    assert!(aimed > 0.99, "prone_aimed {aimed}");
    let (muzzle, off) = gun(&app, e);
    assert!(
        muzzle.dot(Vec3::NEG_Z) > 0.995,
        "aimed lying: muzzle {muzzle}"
    );
    assert!(off < 0.06, "left fist {off} m off the fore grip");
    let head = bone(&app, e, "head").y;
    assert!(head < 0.6, "head {head}: aiming stood the body up");
    // Propped on both elbows, the gun a forearm's height off the ground.
    for elbow in ["lower_arm_l", "lower_arm_r"] {
        let y = bone(&app, e, elbow).y;
        assert!(y < 0.14, "{elbow} {y} m up: not down on the ground");
    }
    let fist = bone(&app, e, "hand_r").y;
    assert!((0.15..0.4).contains(&fist), "right hand {fist} m up");
    assert!(lowest(&app, e) > -0.03, "a bone under the floor aiming");
    // A shot kicks the gun back and up, goes off, and settles back on the target, all lying.
    let before = bone(&app, e, "hand_r");
    app.world
        .write_message(engine_world::animation::AnimationTrigger {
            entity: e,
            name: "fire".into(),
        });
    let mut kicked: f32 = 0.0;
    let mut climb: f32 = 0.0;
    for _ in 0..30 {
        app.update();
        kicked = kicked.max(bone(&app, e, "hand_r").z - before.z);
        climb = climb.max(gun(&app, e).0.y);
        assert!(bone(&app, e, "head").y < 0.6, "the shot stood the body up");
    }
    assert!(
        app.world.resource::<Said>().0.iter().any(|s| s == "shot"),
        "no shot"
    );
    assert!(kicked > 0.01, "the gun kicked back only {kicked} m");
    assert!(climb > 0.02, "the muzzle climbed only to {climb}");
    let (muzzle, _) = gun(&app, e);
    assert!(muzzle.dot(Vec3::NEG_Z) > 0.995, "after the shot: {muzzle}");
    // Lowered again, and up out of it standing.
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set("aim", 0.0);
    prone(&mut app, e, false);
    run(&mut app, PRONE_SECS as f64 + 1.0);
    let head = bone(&app, e, "head").y;
    assert!(head > 1.5, "head {head}: not up again");
}

#[test]
fn a_rifleman_crawls_with_the_gun_on_its_side_in_his_right_fist() {
    use engine_anim::build::weapons::RIFLE_UP;
    use engine_world::combat::{ItemClass, Loadout};
    let mut app = app();
    let e = person(&mut app);
    app.world.entity_mut(e).insert(Loadout {
        main: ItemClass::Firearm,
        off: ItemClass::None,
        two_handed: true,
    });
    run(&mut app, 0.5);
    prone(&mut app, e, true);
    run(&mut app, PRONE_SECS as f64 + 0.5);
    let mut floor = f32::MAX;
    for f in 0..200 {
        app.world.get_mut::<Transform>(e).unwrap().translation.z -= 0.45 * STEP;
        app.update();
        if f < 60 {
            continue;
        }
        floor = floor.min(lowest(&app, e));
        let (muzzle, _) = gun(&app, e);
        let pose = app.world.get::<SkeletonPose>(e).unwrap();
        let top = pose.bones[pose.index_of("hand_r").unwrap()].rotation * RIFLE_UP;
        assert!(muzzle.z < -0.95, "muzzle {muzzle}: not ahead crawling");
        assert!(top.x < -0.9, "gun top {top}: not on its side, top in");
    }
    assert!(floor > -0.03, "a bone {floor} under the floor");
    assert!(bone(&app, e, "head").y < 0.6, "head up crawling");
}
