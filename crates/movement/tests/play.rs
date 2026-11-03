//! Play mode, the pilot camera and lock-on.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_movement::combat::CombatState;
use engine_movement::play::{self, Parked};
use engine_movement::player::PlayerControlled;
use engine_movement::prelude::*;
use engine_movement::{AttackPhase, Play};
use engine_render::{Camera, MeshRenderer};
use engine_world::interact::Using;
use engine_world::locomotion::{LocomotionMode, MoveIntent};
use engine_world::physics::CharacterController;
use engine_world::{AttackKind, Ground, GroundQuery, GroundSample, Water, WaterQuery, WaterSample};
use serde_json::{Value, json};
use std::sync::Arc;
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

    // Off: the camera is gone and the old one is in use again; the pilot is parked, hidden
    // and out of physics.
    let r = api(&mut app, "movement.play", json!({"on": false})).unwrap();
    assert_eq!(r["was_playing"], true);
    assert!(app.world.get_entity(cam).is_err());
    assert_parked(&mut app, pilot);
    assert_eq!(
        *app.world.resource::<Play>(),
        Play {
            parked: Some(pilot),
            ..Play::default()
        }
    );
}

fn drawn_meshes(app: &mut App, root: Entity) -> (usize, usize) {
    let mut stack = vec![root];
    let (mut shown, mut all) = (0, 0);
    while let Some(e) = stack.pop() {
        if let Some(c) = app.world.get::<Children>(e) {
            stack.extend(c.iter());
        }
        if let Some(m) = app.world.get::<MeshRenderer>(e) {
            all += 1;
            shown += m.visible as usize;
        }
    }
    (shown, all)
}

fn assert_parked(app: &mut App, pilot: Entity) {
    assert!(app.world.get::<Parked>(pilot).is_some());
    assert!(app.world.get::<CharacterController>(pilot).is_none());
    assert!(app.world.get::<PlayerControlled>(pilot).is_none());
    let (shown, all) = drawn_meshes(app, pilot);
    assert!(all > 0 && shown == 0, "{shown} of {all} meshes drawn");
}

/// Playtest 2026-10-29 #10: one pilot, parked while flying and brought along, not a fresh
/// one each time.
#[test]
fn play_mode_brings_the_same_pilot_back_where_the_camera_is_now() {
    let mut app = app();
    let cam = app
        .world
        .spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, 10.0)))
        .id();
    app.update();
    let pilot = play::start(&mut app.world, None).unwrap();
    run(&mut app, 0.5);
    app.world.get_mut::<Fighter>(pilot).unwrap().health = 40.0;
    assert!(play::stop(&mut app.world));
    let parked_at = pos(&app, pilot);
    // Parked, it stays put (no gravity, no controls) while the camera flies off.
    app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(60.0, 30.0, 10.0);
    run(&mut app, 1.0);
    assert!(pos(&app, pilot).distance(parked_at) < 1e-6);
    assert_parked(&mut app, pilot);

    let again = play::start(&mut app.world, None).unwrap();
    assert_eq!(again, pilot);
    run(&mut app, 1.0);
    let p = pos(&app, pilot);
    assert!(p.distance(DVec3::new(60.0, 0.9, 4.0)) < 0.3, "{p:?}");
    assert!(app.world.get::<Mover>(pilot).unwrap().grounded);
    assert_eq!(app.world.get::<Fighter>(pilot).unwrap().health, 40.0);
    assert_eq!(
        *app.world.get::<MovementProfile>(pilot).unwrap(),
        MovementProfile::pilot()
    );
    assert!(app.world.get::<Parked>(pilot).is_none());
    let (shown, all) = drawn_meshes(&mut app, pilot);
    assert_eq!(shown, all);
    assert_eq!(
        app.world
            .query_filtered::<(), With<PlayerControlled>>()
            .iter(&app.world)
            .count(),
        1
    );
}

struct NoGround;
impl GroundQuery for NoGround {
    fn sample(&self, _p: DVec3) -> Option<GroundSample> {
        None
    }
}

/// A 3 m deep pond, 20 m across, around the origin.
struct Pond;
impl WaterQuery for Pond {
    fn sample(&self, p: DVec3) -> Option<WaterSample> {
        (p.x.abs() < 10.0 && p.z.abs() < 10.0).then_some(WaterSample {
            surface_height: 3.0,
            depth: 3.0,
            velocity: Vec3::ZERO,
            normal: Vec3::Y,
        })
    }
}

/// Playtest 2026-10-29 #10: P from high up found nothing to stand on (a 400 m look), and it
/// put the pilot on a lake's bed.
#[test]
fn play_mode_from_high_up_finds_the_ground_and_starts_on_water_not_its_bed() {
    let mut app = app();
    // Only colliders: no terrain answers the ground query.
    app.world.insert_resource(Ground(Arc::new(NoGround)));
    block(
        &mut app,
        DVec3::new(0.0, -0.5, 0.0),
        Vec3::new(400.0, 1.0, 400.0),
    );
    let cam = app
        .world
        .spawn((Camera::default(), Transform::from_xyz(40.0, 1500.0, 10.0)))
        .id();
    app.update();
    let pilot = play::start(&mut app.world, None).unwrap();
    let p = pos(&app, pilot);
    assert!(p.distance(DVec3::new(40.0, 0.95, 4.0)) < 0.1, "{p:?}");

    // Over the pond: on its surface, then swimming.
    app.world.insert_resource(Water(Arc::new(Pond)));
    play::stop(&mut app.world);
    app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(0.0, 8.0, 6.0);
    app.update();
    let again = play::start(&mut app.world, None).unwrap();
    assert_eq!(again, pilot);
    let p = pos(&app, pilot);
    assert!((p.y - 3.95).abs() < 0.05, "{p:?}");
    run(&mut app, 3.0);
    let m = app.world.get::<Mover>(pilot).unwrap();
    assert_eq!(m.mode, LocomotionMode::Swim);

    // Nothing under the camera at all: the pilot drops from the air in front of it.
    app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(900.0, 100.0, 6.0);
    app.update();
    play::start(&mut app.world, None).unwrap();
    let p = pos(&app, pilot);
    assert!(p.distance(DVec3::new(900.0, 100.95, 0.0)) < 0.1, "{p:?}");
}

/// Playtest 2026-10-29 #10: P while driving despawned the seated pilot (the drive ended by
/// force and the free camera jumped back to where the pilot got in).
#[test]
fn play_mode_keeps_a_pilot_in_a_vehicle_until_it_gets_out() {
    let mut app = app();
    app.world
        .spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, 10.0)));
    app.update();
    let pilot = play::start(&mut app.world, None).unwrap();
    let car = app.world.spawn(Name::new("Car")).id();
    app.world.entity_mut(pilot).insert(Using { target: car });
    assert!(play::in_vehicle(&app.world));
    assert!(!play::stop(&mut app.world));
    assert!(api(&mut app, "movement.play", json!({"on": false})).is_err());
    assert!(play::start(&mut app.world, None).is_err());
    let play = *app.world.resource::<Play>();
    assert_eq!(play.pilot, Some(pilot));
    assert!(app.world.get_entity(play.camera.unwrap()).is_ok());

    // Out of the car: P parks it as usual.
    app.world.entity_mut(pilot).remove::<Using>();
    assert!(play::stop(&mut app.world));
    assert_parked(&mut app, pilot);
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

/// Playtest 2026-10-29 #19: the reticle's answer, in reach of a wall and turned away from it,
/// across a restart of play mode.
#[test]
fn grapple_aim_is_ready_facing_a_wall_in_reach_and_not_facing_away() {
    use engine_movement::camera::PilotCamera;
    use engine_movement::reticle::GrappleAim;
    let mut app = app();
    block(
        &mut app,
        DVec3::new(0.0, -0.5, 0.0),
        Vec3::new(30.0, 1.0, 30.0),
    );
    block(
        &mut app,
        DVec3::new(0.0, 6.0, -16.0),
        Vec3::new(14.0, 12.0, 1.0),
    );
    let cam = app
        .world
        .spawn((Camera::default(), Transform::from_xyz(0.0, 2.0, 4.0)))
        .id();
    app.update();
    // Play starts facing the way the free camera looks.
    let aim_at = |app: &mut App, toward: f64| {
        *app.world.get_mut::<Transform>(cam).unwrap() =
            Transform::from_xyz(0.0, 2.0, 0.0).looking_at(DVec3::new(0.0, 2.0, toward), Vec3::Y);
        app.update();
        play::start(&mut app.world, Some(DVec3::ZERO)).unwrap();
        run(app, 0.2);
        let pc = app.world.resource::<Play>().camera.unwrap();
        app.world.get_mut::<PilotCamera>(pc).unwrap().pitch = 0.06;
        run(app, 1.0);
        *app.world.resource::<GrappleAim>()
    };
    let near = aim_at(&mut app, -1.0);
    assert!(near.active && near.ready, "{near:?}");
    assert!((near.distance_m - 15.5).abs() < 1.0, "{near:?}");
    let away = aim_at(&mut app, 1.0);
    assert!(away.active && !away.ready, "{away:?}");
    let _ = cam;

    play::stop(&mut app.world);
    app.update();
    assert!(!app.world.resource::<GrappleAim>().active);
}
