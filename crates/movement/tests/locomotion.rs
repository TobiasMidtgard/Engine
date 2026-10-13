//! Locomotion against the physics contract (run here by the stand-in character physics):
//! sprint, jump, double jump, slide under a bar, wall-run and wall-jump, mantle, ledge hang,
//! dash, and characters running on their own clocks in slow motion.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_movement::prelude::*;
use engine_world::LocomotionMode;
use engine_world::locomotion::MoveIntent;
use engine_world::physics::CharacterController;
use support::*;

fn character(app: &mut App, pos: DVec3, profile: MovementProfile) -> Entity {
    app.world
        .spawn((Transform::from_translation(pos), profile))
        .id()
}

fn intent(app: &mut App, e: Entity, f: impl FnOnce(&mut MoveIntent)) {
    f(&mut app.world.get_mut::<MoveIntent>(e).unwrap());
}

fn mover(app: &App, e: Entity) -> Mover {
    app.world.get::<Mover>(e).unwrap().clone()
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// Press jump for one step.
fn tap_jump(app: &mut App, e: Entity) {
    intent(app, e, |i| i.jump = true);
    app.update();
    intent(app, e, |i| i.jump = false);
}

#[test]
fn sprints_to_profile_speed_on_the_ground() {
    let mut app = app();
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 2.0);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Sprint);
    assert!(m.grounded);
    assert!(
        (m.horizontal_speed() - 8.5).abs() < 0.2,
        "{}",
        m.horizontal_speed()
    );
    assert!((pos(&app, c).y - 0.9).abs() < 0.05, "stays on the ground");
    // It turned to face where it runs.
    assert!(m.facing.dot(Vec3::X) > 0.99);
}

#[test]
fn jumps_and_double_jumps_to_profile_heights() {
    let mut app = app();
    let p = MovementProfile::pilot();
    let c = character(&mut app, DVec3::new(0.0, 0.9, 0.0), p);
    app.update();
    tap_jump(&mut app, c);
    let mut peak: f64 = 0.0;
    run_until(&mut app, 1.5, |w| {
        peak = peak.max(w.get::<Transform>(c).unwrap().translation.y - 0.9);
        false
    });
    assert!(
        (peak - p.jump_height as f64).abs() < 0.08,
        "jump peak {peak} m, profile {}",
        p.jump_height
    );
    assert!(mover(&app, c).grounded, "landed");

    // Jump, then jump again near the top: higher. A third press does nothing.
    tap_jump(&mut app, c);
    run(&mut app, 0.35);
    tap_jump(&mut app, c);
    run(&mut app, 0.2);
    tap_jump(&mut app, c);
    let mut peak: f64 = 0.0;
    run_until(&mut app, 2.0, |w| {
        peak = peak.max(w.get::<Transform>(c).unwrap().translation.y - 0.9);
        false
    });
    assert_eq!(triggered(&app, c, "double_jump"), 1);
    assert!(
        peak > p.jump_height as f64 + p.air_jump_height as f64 * 0.7,
        "double jump peak {peak} m"
    );
}

#[test]
fn slides_under_a_bar_that_blocks_running() {
    let mut app = app();
    // Bar with its bottom at 1.15 m, across the path at x = 12.
    block(
        &mut app,
        DVec3::new(12.0, 1.35, 0.0),
        Vec3::new(0.3, 0.4, 6.0),
    );
    let slider = character(
        &mut app,
        DVec3::new(0.0, 0.9, -1.0),
        MovementProfile::pilot(),
    );
    let runner = character(
        &mut app,
        DVec3::new(0.0, 0.9, 1.0),
        MovementProfile::pilot(),
    );
    for c in [slider, runner] {
        intent(&mut app, c, |i| {
            i.direction = Vec3::X;
            i.sprint = true;
        });
    }
    run(&mut app, 1.2);
    intent(&mut app, slider, |i| i.crouch = true);
    app.update();
    let m = mover(&app, slider);
    assert_eq!(m.mode, LocomotionMode::Slide);
    assert!(
        m.horizontal_speed() > 8.5,
        "the slide boosts: {}",
        m.horizontal_speed()
    );
    assert!(app.world.get::<CharacterController>(slider).unwrap().height < 1.2);
    run(&mut app, 1.0);
    assert!(
        pos(&app, slider).x > 13.0,
        "slid under: {}",
        pos(&app, slider).x
    );
    assert!(
        pos(&app, runner).x < 12.0,
        "blocked: {}",
        pos(&app, runner).x
    );
    assert_eq!(triggered(&app, slider, "slide"), 1);
    // Let go of crouch past the bar: stands up again.
    intent(&mut app, slider, |i| i.crouch = false);
    run(&mut app, 0.5);
    assert!(app.world.get::<CharacterController>(slider).unwrap().height > 1.7);
}

#[test]
fn wall_runs_along_a_wall_and_wall_jumps_off() {
    let mut app = app();
    // A wall along +X whose face is at z = 1.2.
    block(
        &mut app,
        DVec3::new(15.0, 2.5, 1.5),
        Vec3::new(30.0, 5.0, 0.6),
    );
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 1.0);
    // Jump, and strafe toward the wall in the air.
    tap_jump(&mut app, c);
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(0.2, 0.0, 1.0).normalize()
    });
    let start = pos(&app, c);
    let mut wallrun_secs = 0.0;
    run_until(&mut app, 1.6, |w| {
        if w.get::<Mover>(c).unwrap().mode == LocomotionMode::WallRun {
            wallrun_secs += DT;
        }
        false
    });
    let m = mover(&app, c);
    assert!(wallrun_secs > 1.0, "wall-ran for {wallrun_secs:.2} s");
    assert_eq!(m.mode, LocomotionMode::WallRun);
    assert!(m.wall_side() > 0.5, "wall on the right (+Z when facing +X)");
    let p = pos(&app, c);
    assert!(
        p.x - start.x > 12.0,
        "ran {} m along the wall",
        p.x - start.x
    );
    assert!(p.y > 1.4, "still high: {}", p.y);
    assert!(m.velocity.x > 8.0, "full speed along the wall");

    // Wall-jump: away from the wall (−Z) and up.
    tap_jump(&mut app, c);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Air);
    assert!(
        m.velocity.z < -3.0 && m.velocity.y > 3.0,
        "{:?}",
        m.velocity
    );
    assert_eq!(triggered(&app, c, "wall_jump"), 1);
    // It doesn't stick straight back onto the wall it just left.
    run(&mut app, 0.3);
    assert_ne!(mover(&app, c).mode, LocomotionMode::WallRun);
}

#[test]
fn mantles_onto_a_ledge() {
    let mut app = app();
    // A 1.6 m ledge starting at x = 5.
    block(
        &mut app,
        DVec3::new(10.0, 0.8, 0.0),
        Vec3::new(10.0, 1.6, 10.0),
    );
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| i.direction = Vec3::X);
    run(&mut app, 0.6);
    tap_jump(&mut app, c);
    let mut mantled = false;
    run_until(&mut app, 2.0, |w| {
        mantled |= w.get::<Mover>(c).unwrap().mode == LocomotionMode::Mantle;
        false
    });
    assert!(mantled);
    let p = pos(&app, c);
    let m = mover(&app, c);
    assert!(m.grounded && p.x > 5.5, "standing on top: {p:?}");
    assert!((p.y - (1.6 + 0.9)).abs() < 0.05, "{p:?}");
}

/// A 3.6 m wall whose face is at x = 1.0, 8 m wide (z from −4 to 4), and a pilot in front of
/// it that jumps at it and catches its top.
fn hang_from_a_high_wall(app: &mut App) -> Entity {
    block(app, DVec3::new(3.0, 1.8, 0.0), Vec3::new(4.0, 3.6, 8.0));
    let c = character(app, DVec3::new(0.0, 0.9, 0.0), MovementProfile::pilot());
    run(app, 0.1);
    intent(app, c, |i| i.direction = Vec3::X);
    tap_jump(app, c);
    run_until(app, 1.5, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::Climb
    });
    assert_eq!(mover(app, c).mode, LocomotionMode::Climb);
    assert_eq!(triggered(app, c, "ledge_grab"), 1);
    intent(app, c, |i| i.direction = Vec3::ZERO);
    run(app, 0.4);
    c
}

#[test]
fn catches_a_ledge_too_high_to_mantle_and_shimmies_along_it() {
    let mut app = app();
    let c = hang_from_a_high_wall(&mut app);
    // Hanging still: arms' length under the top, against the wall, facing it.
    let p = pos(&app, c);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Climb);
    assert!((p.y - (3.6 - 0.9 - 0.3)).abs() < 0.05, "{p:?}");
    assert!((p.x - (1.0 - 0.35 - 0.04)).abs() < 0.05, "{p:?}");
    assert!(m.facing.distance(Vec3::X) < 0.05, "{}", m.facing);
    assert!(m.velocity.length() < 0.2, "{}", m.velocity);

    // Shimmy toward +Z: hand over hand along the top, at the same height.
    intent(&mut app, c, |i| i.direction = Vec3::Z);
    run(&mut app, 1.0);
    let q = pos(&app, c);
    assert_eq!(mover(&app, c).mode, LocomotionMode::Climb);
    assert!(q.z - p.z > 1.5, "shimmied {:.2} m", q.z - p.z);
    assert!((q.y - p.y).abs() < 0.05, "{q:?}");
    // ...and stops at the end of the wall instead of falling off it.
    run(&mut app, 3.0);
    let end = pos(&app, c);
    assert_eq!(mover(&app, c).mode, LocomotionMode::Climb);
    assert!(end.z > 3.0 && end.z < 4.0, "{end:?}");

    // Jump: climbs up and stands on top.
    intent(&mut app, c, |i| i.direction = Vec3::ZERO);
    tap_jump(&mut app, c);
    assert_eq!(mover(&app, c).mode, LocomotionMode::Mantle);
    run(&mut app, 1.5);
    let top = pos(&app, c);
    let m = mover(&app, c);
    assert!(m.grounded && top.x > 1.2, "{top:?}");
    assert!((top.y - (3.6 + 0.9)).abs() < 0.05, "{top:?}");
}

#[test]
fn holding_toward_the_wall_climbs_up() {
    let mut app = app();
    let c = hang_from_a_high_wall(&mut app);
    intent(&mut app, c, |i| i.direction = Vec3::X);
    let secs = run_until(&mut app, 2.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.grounded && m.mode != LocomotionMode::Mantle
    });
    let p = pos(&app, c);
    assert!(secs < 1.2 && p.x > 1.2, "on top after {secs:.2} s at {p:?}");
}

#[test]
fn lets_go_of_a_ledge_or_kicks_off_it() {
    let mut app = app();
    let c = hang_from_a_high_wall(&mut app);
    // Crouch: drops to the ground and doesn't catch the ledge again on the way down.
    intent(&mut app, c, |i| i.crouch = true);
    app.update();
    intent(&mut app, c, |i| i.crouch = false);
    assert_eq!(mover(&app, c).mode, LocomotionMode::Air);
    run_until(&mut app, 2.0, |w| w.get::<Mover>(c).unwrap().grounded);
    run(&mut app, 0.3);
    assert!(
        mover(&app, c).grounded && pos(&app, c).y < 1.0,
        "{:?} {:?}",
        mover(&app, c).mode,
        pos(&app, c)
    );
    assert_eq!(triggered(&app, c, "ledge_grab"), 1);

    // Back up, then jump while pulling away from the wall: a kick off it.
    intent(&mut app, c, |i| i.direction = Vec3::X);
    tap_jump(&mut app, c);
    run_until(&mut app, 1.5, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::Climb
    });
    assert_eq!(mover(&app, c).mode, LocomotionMode::Climb);
    intent(&mut app, c, |i| i.direction = Vec3::NEG_X);
    tap_jump(&mut app, c);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Air);
    assert!(m.velocity.x < -4.0 && m.velocity.y > 3.0, "{}", m.velocity);
    assert_eq!(triggered(&app, c, "wall_jump"), 1);
}

#[test]
fn dashes_fast_with_charges_and_a_dodge_window() {
    let mut app = app();
    let p = MovementProfile::pilot();
    let c = character(&mut app, DVec3::new(0.0, 0.9, 0.0), p);
    app.update();
    intent(&mut app, c, |i| {
        i.dash_to = Some(DVec3::new(20.0, 0.9, 0.0))
    });
    app.update();
    intent(&mut app, c, |i| i.dash_to = None);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Dash);
    assert!(m.evade_secs > 0.0, "dodge window open");
    assert!((m.dash_charges - (p.dash_charges as f32 - 1.0)).abs() < 0.05);
    run(&mut app, p.dash_secs as f64 + 0.05);
    let x = pos(&app, c).x;
    assert!(
        (x - p.dash_distance as f64).abs() < 0.6,
        "dashed {x} m in {} s",
        p.dash_secs
    );
    assert_ne!(mover(&app, c).mode, LocomotionMode::Dash);
    assert_eq!(triggered(&app, c, "dash"), 1);
}

#[test]
fn characters_run_on_their_own_clocks_in_slow_motion() {
    let mut app = app();
    let walker = |app: &mut App, z: f64| {
        let c = character(app, DVec3::new(0.0, 0.9, z), MovementProfile::human());
        intent(app, c, |i| i.direction = Vec3::X);
        c
    };
    let slowed = walker(&mut app, -4.0);
    let focused = walker(&mut app, 0.0);
    let fast = walker(&mut app, 4.0);
    app.world
        .entity_mut(focused)
        .insert(LocalTimeScale::unaffected_by_dilation(1.0));
    // An elite whose whole clock runs twice as fast.
    app.world.entity_mut(fast).insert(Tempo {
        speed: 2.0,
        reflexes: 1.0,
    });
    // Get up to speed, then slow the world to a quarter for two real seconds.
    run(&mut app, 2.0);
    let before = [slowed, focused, fast].map(|c| pos(&app, c).x);
    app.world
        .resource_mut::<TimeDilation>()
        .hold("test", 0.25, 0.0);
    run(&mut app, 2.0);
    let moved: Vec<f64> = [slowed, focused, fast]
        .iter()
        .zip(before)
        .map(|(c, b)| pos(&app, *c).x - b)
        .collect();
    let run_speed = MovementProfile::human().run_speed as f64;
    assert!((moved[0] - run_speed * 0.5).abs() < 0.2, "{moved:?}");
    assert!((moved[1] - run_speed * 2.0).abs() < 0.2, "{moved:?}");
    assert!((moved[2] - run_speed * 1.0).abs() < 0.2, "{moved:?}");
    // What other modules read: velocity in world seconds.
    let loco = app
        .world
        .get::<engine_world::LocomotionState>(focused)
        .unwrap();
    assert!(
        (loco.velocity.x as f64 - run_speed * 4.0).abs() < 0.3,
        "{loco:?}"
    );
}

#[test]
fn the_stand_in_physics_steps_aside_for_a_physics_module() {
    let mut app = app();
    engine_world::physics::claim_backend(&mut app, "some physics module");
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| i.direction = Vec3::X);
    run(&mut app, 1.0);
    // Nothing moves characters now except the (absent) physics module, but movement still
    // asks for motion.
    assert_eq!(pos(&app, c), DVec3::new(0.0, 0.9, 0.0));
    let mv = app
        .world
        .get::<engine_world::physics::CharacterMove>(c)
        .unwrap();
    assert!(mv.velocity.x > 1.0);
}

#[test]
fn bodies_from_other_modules_are_moved_too() {
    // Movement claims the locomotion backend, so it moves any character with a MoveIntent and a
    // capsule, such as an NPC spawned by another module without a MovementProfile.
    let mut app = app();
    let npc = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            CharacterController {
                radius: 0.5,
                height: 2.0,
                ..Default::default()
            },
            MoveIntent {
                direction: Vec3::X,
                ..Default::default()
            },
        ))
        .id();
    // No capsule: not a character, left alone.
    let bird = app
        .world
        .spawn((Transform::from_xyz(0.0, 5.0, 4.0), MoveIntent::default()))
        .id();
    run(&mut app, 2.0);
    let profile = *app.world.get::<MovementProfile>(npc).expect("adopted");
    assert_eq!(profile, MovementProfile::human());
    assert_eq!(
        app.world.get::<CharacterController>(npc).unwrap().radius,
        0.5,
        "keeps its own capsule"
    );
    assert!(mover(&app, npc).grounded);
    let x = pos(&app, npc).x;
    assert!(x > 6.0, "walked {x:.2} m in 2 s");
    assert!(app.world.get::<Mover>(bird).is_none());
    assert_eq!(pos(&app, bird), DVec3::new(0.0, 5.0, 4.0));
}
