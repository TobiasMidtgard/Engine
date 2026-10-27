//! Play mode, the pilot camera and lock-on.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_movement::combat::CombatState;
use engine_movement::prelude::*;
use engine_movement::{AttackPhase, Play};
use engine_render::Camera;
use engine_world::AttackKind;
use engine_world::locomotion::MoveIntent;
use serde_json::{Value, json};
use support::*;

fn api(app: &mut App, name: &str, args: Value) -> Result<Value, String> {
    engine_core::registry::call_api(&mut app.world, name, args)
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn enemy(app: &mut App, name: &str, at: DVec3) -> Entity {
    app.world
        .spawn((
            Name::new(name.to_string()),
            Transform::from_translation(at),
            MovementProfile::human(),
            Fighter::new(2, 100.0),
        ))
        .id()
}

#[test]
fn play_mode_puts_a_pilot_in_front_of_the_camera_and_follows_it() {
    let mut app = app();
    app.world
        .spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, 10.0)));
    app.update();
    let r = api(&mut app, "movement.play", json!({"on": true})).unwrap();
    assert_eq!(r["playing"], true);
    let play = *app.world.resource::<Play>();
    let (pilot, cam) = (play.pilot.unwrap(), play.camera.unwrap());
    run(&mut app, 1.0);

    // On the ground about 6 m in front of the camera, facing away from it.
    let p = pos(&app, pilot);
    assert!(p.distance(DVec3::new(0.0, 0.9, 4.0)) < 0.3, "{p:?}");
    let m = app.world.get::<Mover>(pilot).unwrap();
    assert!(m.grounded && m.facing.distance(Vec3::NEG_Z) < 0.05);

    // The pilot camera is drawn over the old one, behind and above the pilot, looking at it.
    let c = app.world.get::<Camera>(cam).unwrap();
    assert!(c.active && c.order > 0);
    let t = *app.world.get::<Transform>(cam).unwrap();
    let eye = t.translation;
    assert!(eye.z > p.z + 2.5 && eye.y > p.y + 0.5, "{eye:?} vs {p:?}");
    let to_pilot = (p - eye).normalize().as_vec3();
    assert!(t.forward().dot(to_pilot) > 0.9, "{}", t.forward());

    // Off: both are gone and the old camera is the one in use again.
    let r = api(&mut app, "movement.play", json!({"on": false})).unwrap();
    assert_eq!(r["was_playing"], true);
    assert!(app.world.get_entity(pilot).is_err() && app.world.get_entity(cam).is_err());
    assert_eq!(*app.world.resource::<Play>(), Play::default());
}

#[test]
fn locked_on_the_pilot_faces_and_dash_strikes_its_target_past_a_closer_enemy() {
    let mut app = app();
    api(
        &mut app,
        "movement.play",
        json!({"on": true, "at": [0.0, 0.0, 0.0]}),
    )
    .unwrap();
    let pilot = app.world.resource::<Play>().pilot.unwrap();
    let near = enemy(&mut app, "Near grunt", DVec3::new(1.6, 0.9, -1.2));
    let far = enemy(&mut app, "Far grunt", DVec3::new(-1.5, 0.9, -8.0));
    run(&mut app, 0.5);

    let r = api(&mut app, "movement.lock_on", json!({"target": "Far grunt"})).unwrap();
    assert_eq!(r["target"], far.to_bits());
    assert_eq!(app.world.get::<LockOn>(pilot).unwrap().target, Some(far));

    // Attack: a dash-strike at the locked enemy, not the one at arm's length.
    app.world.get_mut::<MoveIntent>(pilot).unwrap().attack = Some(AttackKind::Melee);
    app.update();
    app.world.get_mut::<MoveIntent>(pilot).unwrap().attack = None;
    let a = app
        .world
        .get::<CombatState>(pilot)
        .unwrap()
        .attack
        .expect("attacking");
    assert_eq!(
        a.target,
        Some(far),
        "went for {:?} (near is {near:?})",
        a.target
    );
    assert_eq!(a.kind, AttackKind::Dash);
    run_until(&mut app, 2.0, |w| {
        w.get::<CombatState>(pilot)
            .unwrap()
            .attack
            .is_none_or(|a| a.phase == AttackPhase::Recovery)
    });
    let gap = (pos(&app, far) - pos(&app, pilot)).with_y(0.0).length();
    assert!(gap < 2.5, "closed to {gap:.2} m");

    // Letting go.
    api(&mut app, "movement.lock_on", json!({"target": null})).unwrap();
    assert_eq!(app.world.get::<LockOn>(pilot).unwrap().target, None);
}

#[test]
fn a_lock_moves_on_when_its_target_falls() {
    let mut app = app();
    api(
        &mut app,
        "movement.play",
        json!({"on": true, "at": [0.0, 0.0, 0.0]}),
    )
    .unwrap();
    let pilot = app.world.resource::<Play>().pilot.unwrap();
    let a = enemy(&mut app, "A", DVec3::new(0.0, 0.9, -6.0));
    let b = enemy(&mut app, "B", DVec3::new(3.0, 0.9, -7.0));
    run(&mut app, 0.3);
    let r = api(&mut app, "movement.lock_on", json!({"target": "auto"})).unwrap();
    assert_eq!(r["target"], a.to_bits());
    app.world.get_mut::<CombatState>(a).unwrap().defeated = true;
    run(&mut app, 0.1);
    assert_eq!(app.world.get::<LockOn>(pilot).unwrap().target, Some(b));
    app.world.get_mut::<CombatState>(b).unwrap().defeated = true;
    run(&mut app, 0.1);
    assert_eq!(app.world.get::<LockOn>(pilot).unwrap().target, None);
}

/// Playtest 2026-10-27 #25 ("way too fast ... allow me to hold in shift and scroll to run
/// faster for that sprint only"): the player runs unless sprint is held, the wheel with sprint
/// held speeds that sprint up, and letting go gives the normal sprint back.
#[test]
fn the_player_runs_sprints_on_shift_and_scrolls_its_sprint_faster() {
    use engine_core::math::Vec2;
    use engine_input::{ActionState, InputEvent, InputPlugin};
    let mut app = app();
    app.add_plugins(InputPlugin);
    let p = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 0.9, 0.0)),
            MovementProfile::pilot(),
            PlayerControlled::default(),
        ))
        .id();
    let speed = |app: &App| app.world.get::<Mover>(p).unwrap().horizontal_speed();
    app.world
        .resource_mut::<ActionState>()
        .set_axis("move", Vec2::Y, None);
    run(&mut app, 1.5);
    assert!((speed(&app) - 5.5).abs() < 0.1, "runs: {}", speed(&app));

    app.world.resource_mut::<ActionState>().press("sprint");
    run(&mut app, 1.5);
    assert!((speed(&app) - 8.5).abs() < 0.1, "sprints: {}", speed(&app));

    // Three notches up: 1.15³ times the sprint.
    app.world.write_message(InputEvent::MouseWheel {
        delta: Vec2::new(0.0, 3.0),
    });
    run(&mut app, 2.0);
    let fast = 8.5 * 1.15f32.powi(3);
    assert!(
        (speed(&app) - fast).abs() < 0.2,
        "{} vs {fast}",
        speed(&app)
    );

    // Let go: back to running, and the next sprint is a normal one.
    app.world.resource_mut::<ActionState>().release("sprint");
    run(&mut app, 3.0);
    assert!(
        (speed(&app) - 5.5).abs() < 0.1,
        "runs again: {}",
        speed(&app)
    );
    app.world.resource_mut::<ActionState>().press("sprint");
    run(&mut app, 1.5);
    assert!(
        (speed(&app) - 8.5).abs() < 0.1,
        "normal sprint: {}",
        speed(&app)
    );
}

/// Play mode's pilot keeps a trained fighter's reflexes but moves on a normal clock (#25:
/// level 10's 1.45 clock made everything it did 45% fast).
#[test]
fn play_mode_pilot_moves_at_normal_speed() {
    let mut app = app();
    app.world
        .spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, 10.0)));
    app.update();
    api(&mut app, "movement.play", json!({"on": true})).unwrap();
    let pilot = app.world.resource::<Play>().pilot.unwrap();
    let tempo = *app.world.get::<Tempo>(pilot).unwrap();
    assert_eq!(tempo.speed, 1.0);
    assert!(tempo.reflexes > 5.0);
    assert!(
        app.world
            .get::<PlayerControlled>(pilot)
            .unwrap()
            .hold_to_sprint
    );
    // Its body reaches what it can use (playtest #1).
    assert!(
        app.world
            .get::<engine_world::interact::Interactor>(pilot)
            .is_some()
    );
}

/// Playtest #28 ("taking damage forces the player to stand still ... fire damage ... holding
/// them hostage in the fire"): burning hurts every frame without a flinch, so a fighter keeps
/// running out of the fire; a blow still staggers.
#[test]
fn burning_hurts_without_stopping_a_fighter_but_a_blow_staggers() {
    use engine_world::{DamageEvent, DamageKind};
    let mut app = app();
    let f = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 0.9, 0.0)),
            MovementProfile::pilot(),
            Fighter::new(1, 100.0),
        ))
        .id();
    app.world.get_mut::<MoveIntent>(f).unwrap().direction = Vec3::X;
    run(&mut app, 1.0);
    let burn = |app: &mut App, kind| {
        app.world.write_message(DamageEvent {
            target: f,
            amount: 0.2,
            kind,
            point: pos(app, f),
            impulse: Vec3::ZERO,
            source: None,
        });
        app.update();
    };
    for _ in 0..60 {
        burn(&mut app, DamageKind::Fire);
    }
    let m = app.world.get::<Mover>(f).unwrap();
    assert!(
        (m.horizontal_speed() - 5.5).abs() < 0.2,
        "{}",
        m.horizontal_speed()
    );
    let health = app.world.get::<Fighter>(f).unwrap().health;
    assert!((health - 88.0).abs() < 0.1, "burned to {health}");
    assert_eq!(app.world.get::<CombatState>(f).unwrap().hits_taken, 0);

    burn(&mut app, DamageKind::Blunt);
    let cs = app.world.get::<CombatState>(f).unwrap();
    assert!(cs.stagger_secs > 0.0 && cs.hits_taken == 1);
}

/// Playtest #8 ("running then hitting stops all momentum"): a swing out of a sprint carries
/// the run's speed and skids out over a couple of meters instead of stopping dead.
#[test]
fn a_swing_out_of_a_sprint_keeps_its_momentum() {
    use engine_world::AttackKind;
    let mut app = app();
    let f = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 0.9, 0.0)),
            MovementProfile::pilot(),
            Fighter::new(1, 100.0),
        ))
        .id();
    let intent = |app: &mut App, change: &dyn Fn(&mut MoveIntent)| {
        change(&mut app.world.get_mut::<MoveIntent>(f).unwrap());
    };
    intent(&mut app, &|i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 1.5);
    let start = pos(&app, f);
    intent(&mut app, &|i| i.attack = Some(AttackKind::Melee));
    app.update();
    intent(&mut app, &|i| i.attack = None);
    run(&mut app, 0.1);
    let speed = app.world.get::<Mover>(f).unwrap().horizontal_speed();
    assert!(
        speed > 6.0,
        "stopped dead: {speed} m/s 0.1 s into the swing"
    );
    run(&mut app, 0.25);
    assert!(app.world.get::<CombatState>(f).unwrap().attack.is_some());
    let skid = (pos(&app, f) - start).x;
    assert!((1.8..3.5).contains(&skid), "skidded {skid:.2} m");
}
