//! Wildlife behavior tests: herds bolt from a runner but let a sneaker come closer, panic
//! spreads through a herd, wolf packs run prey down and feed, birds take off together and
//! land again.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    app
}

fn animal(app: &mut App, species: Species, at: DVec3, herd: u32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Animal::new(species),
            Herd(herd),
        ))
        .id()
}

fn herd(app: &mut App, species: Species, center: DVec3, n: usize, id: u32) -> Vec<Entity> {
    (0..n)
        .map(|k| {
            let a = k as f64 / n as f64 * std::f64::consts::TAU;
            animal(
                app,
                species,
                center + DVec3::new(a.cos() * 2.5, 0.0, a.sin() * 2.5),
                id,
            )
        })
        .collect()
}

fn state(app: &App, e: Entity) -> AnimalState {
    app.world.get::<AnimalMind>(e).unwrap().state
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// Walk a stand-in player straight at `target` in `mode` until something in `watch` flees;
/// returns the distance at which the first one bolted.
fn approach(
    app: &mut App,
    from: DVec3,
    target: DVec3,
    mode: LocomotionMode,
    speed: f64,
    watch: &[Entity],
) -> Option<f64> {
    let player = app
        .world
        .spawn((
            Transform::from_translation(from),
            Faction::PLAYER,
            LocomotionState {
                mode,
                ..Default::default()
            },
        ))
        .id();
    let dir = (target - from).normalize();
    for _ in 0..60 * 60 {
        {
            let mut t = app.world.get_mut::<Transform>(player).unwrap();
            if t.translation.distance(target) > 1.5 {
                t.translation += dir * speed / 60.0;
            }
        }
        app.world
            .get_mut::<LocomotionState>(player)
            .unwrap()
            .velocity = (dir * speed).as_vec3();
        app.update();
        if watch.iter().any(|&e| state(app, e) == AnimalState::Flee) {
            let p = app.world.get::<Transform>(player).unwrap().translation;
            return Some(p.distance(target));
        }
    }
    None
}

#[test]
fn deer_bolt_from_a_runner_but_let_a_sneaker_closer() {
    let center = DVec3::new(0.0, 0.0, 0.0);
    let mut a = app();
    let deer = herd(&mut a, Species::Deer, center, 4, 1);
    a.update_n(60);
    let run = approach(
        &mut a,
        DVec3::new(0.0, 0.0, -60.0),
        center,
        LocomotionMode::Sprint,
        7.0,
        &deer,
    )
    .expect("a sprinter spooks them");

    let mut b = app();
    let deer = herd(&mut b, Species::Deer, center, 4, 1);
    b.update_n(60);
    let sneak = approach(
        &mut b,
        DVec3::new(0.0, 0.0, -60.0),
        center,
        LocomotionMode::Crouch,
        1.5,
        &deer,
    )
    .expect("even a sneaker spooks them up close");

    assert!(run > 25.0, "a sprinter should spook them early: {run:.1} m");
    assert!(
        sneak < run * 0.55,
        "a sneaker gets much closer: {sneak:.1} m vs {run:.1} m"
    );
}

#[test]
fn panic_spreads_through_the_herd() {
    let mut app = app();
    let deer = herd(&mut app, Species::Deer, DVec3::ZERO, 5, 2);
    app.update_n(60);
    // Shoot one from the side.
    let shooter = app.world.spawn(Transform::from_xyz(-40.0, 1.5, 0.0)).id();
    app.world.write_message(DamageEvent {
        target: deer[0],
        amount: 5.0,
        kind: DamageKind::Ballistic,
        point: pos(&app, deer[0]),
        impulse: Vec3::ZERO,
        source: Some(shooter),
    });
    app.update_n(60);
    let fleeing = deer
        .iter()
        .filter(|&&e| state(&app, e) == AnimalState::Flee)
        .count();
    assert_eq!(fleeing, deer.len(), "the whole herd runs");
    app.update_n(120);
    // Away from the shot (+X), together.
    for &e in &deer {
        assert!(pos(&app, e).x > 8.0, "{e:?} at {}", pos(&app, e));
    }
}

#[test]
fn a_wolf_pack_runs_prey_down_and_feeds() {
    let mut app = app();
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let deer = herd(&mut app, Species::Deer, DVec3::new(0.0, 0.0, 0.0), 3, 1);
    let wolves = herd(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -45.0), 3, 2);
    for &w in &wolves {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    let mut stalked = false;
    let mut chased = false;
    let mut killed = None;
    for _ in 0..60 * 90 {
        app.update();
        stalked |= wolves.iter().any(|&w| state(&app, w) == AnimalState::Stalk);
        chased |= wolves.iter().any(|&w| state(&app, w) == AnimalState::Chase);
        if let Some(&d) = deer.iter().find(|&&d| state(&app, d) == AnimalState::Dead) {
            killed = Some(d);
            break;
        }
    }
    assert!(stalked, "the pack stalks first");
    assert!(chased, "then charges");
    let kill = killed.expect("the pack should catch a deer within 90 s");
    app.update_n(60 * 6);
    let feeding = wolves
        .iter()
        .filter(|&&w| state(&app, w) == AnimalState::Feed)
        .count();
    assert!(feeding >= 2, "the pack feeds: {feeding}");
    let k = pos(&app, kill);
    for &w in &wolves {
        if state(&app, w) == AnimalState::Feed {
            assert!(pos(&app, w).distance(k) < 3.5);
        }
    }
}

#[test]
fn birds_take_off_together() {
    let mut app = app();
    let flock = herd(&mut app, Species::Bird, DVec3::ZERO, 8, 5);
    app.update_n(60);
    assert!(
        flock
            .iter()
            .all(|&b| state(&app, b) == AnimalState::Perched)
    );
    // Someone runs at the flock from the side: the nearest birds flush and lift the rest.
    let runner = app
        .world
        .spawn((
            Transform::from_xyz(-30.0, 0.0, 0.0),
            Faction::PLAYER,
            LocomotionState {
                mode: LocomotionMode::Run,
                velocity: Vec3::new(5.0, 0.0, 0.0),
                ..Default::default()
            },
        ))
        .id();
    let mut first = None;
    for frame in 0..60 * 8 {
        app.world
            .get_mut::<Transform>(runner)
            .unwrap()
            .translation
            .x += 5.0 / 60.0;
        app.update();
        let up = flock
            .iter()
            .filter(|&&b| state(&app, b) == AnimalState::Flying)
            .count();
        if up > 0 && first.is_none() {
            first = Some(frame);
        }
        if up == flock.len() {
            let spread = frame - first.unwrap();
            assert!(
                spread < 60,
                "the flock lifts within a second: {spread} frames"
            );
            app.update_n(90);
            let high = flock.iter().filter(|&&b| pos(&app, b).y > 3.0).count();
            assert!(high >= 6, "{high} are high in the air");
            return;
        }
    }
    panic!("the flock never took off");
}

#[test]
fn birds_settle_when_the_coast_is_clear() {
    let mut app = app();
    let flock = herd(&mut app, Species::Bird, DVec3::ZERO, 6, 5);
    app.update_n(30);
    // A gunshot scares them up; nothing follows.
    app.world.write_message(engine_world::NoiseEvent {
        position: DVec3::new(10.0, 1.0, 0.0),
        radius: 90.0,
        source: None,
    });
    app.update_n(60);
    assert!(flock.iter().all(|&b| state(&app, b) == AnimalState::Flying));
    app.update_n(60 * 40);
    let perched = flock
        .iter()
        .filter(|&&b| state(&app, b) == AnimalState::Perched)
        .count();
    assert!(perched >= 5, "{perched} landed again");
    for &b in &flock {
        assert!(pos(&app, b).distance(DVec3::ZERO) < 20.0);
    }
}
