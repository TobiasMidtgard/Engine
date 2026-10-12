//! Play mode, the pilot camera and lock-on.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
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
