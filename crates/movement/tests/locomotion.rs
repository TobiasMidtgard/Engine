//! Locomotion against the physics contract (run here by the stand-in character physics):
//! sprint, jump, double jump, slide under a bar, wall-run and wall-jump, mantle, ledge hang,
//! grapple, riding moving walls, ledges and hooks, zip lines, ladders, dash, and characters
//! running on their own clocks in slow motion.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_movement::prelude::*;
use engine_movement::zipline::Zipline;
use engine_world::locomotion::{Climbable, LocomotionState, MoveIntent};
use engine_world::physics::{CharacterController, Velocity};
use engine_world::{GroundContact, LocomotionMode};
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
fn footprints_point_the_way_the_character_runs() {
    let mut app = app();
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| i.direction = Vec3::X);
    let mut cursor = app.world.resource::<Messages<GroundContact>>().get_cursor();
    let mut toes = Vec::new();
    run_until(&mut app, 1.5, |w| {
        let contacts = w.resource::<Messages<GroundContact>>();
        toes.extend(
            cursor
                .read(contacts)
                .filter(|g| g.entity == Some(c))
                .map(|g| g.forward()),
        );
        false
    });
    assert!(toes.len() >= 2, "{} steps", toes.len());
    for toe in toes {
        assert!(toe.dot(Vec3::X) > 0.9, "toe along {toe}, running along +X");
    }
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
            // On the wall: steer along it (steering into it near its top climbs over).
            w.get_mut::<MoveIntent>(c).unwrap().direction = Vec3::X;
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
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(0.2, 0.0, 1.0).normalize()
    });
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
fn steering_into_a_low_wall_while_wall_running_climbs_over_its_top() {
    let mut app = app();
    // A 3.4 m wall along +X whose face is at z = 1.2.
    block(
        &mut app,
        DVec3::new(15.0, 1.7, 2.2),
        Vec3::new(30.0, 3.4, 2.0),
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
    tap_jump(&mut app, c);
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(0.2, 0.0, 1.0).normalize()
    });
    run_until(&mut app, 1.0, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::WallRun
    });
    // Along the wall first: it runs on it rather than climbing at once.
    intent(&mut app, c, |i| i.direction = Vec3::X);
    run(&mut app, 0.3);
    assert_eq!(mover(&app, c).mode, LocomotionMode::WallRun);
    assert_eq!(triggered(&app, c, "mantle"), 0);
    // Into the wall: over the top, at a run.
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(1.0, 0.0, 1.0).normalize()
    });
    run_until(&mut app, 0.5, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::Mantle
    });
    assert_eq!(triggered(&app, c, "mantle"), 1);
    run_until(&mut app, 1.5, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.grounded && m.mode != LocomotionMode::Mantle
    });
    let p = pos(&app, c);
    assert!(mover(&app, c).grounded && p.z > 1.4, "{p:?}");
    assert!((p.y - (3.4 + 0.9)).abs() < 0.05, "{p:?}");
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
    // ...and at the end of the wall goes on around the corner onto its end face, still
    // holding +Z, at the same height.
    run(&mut app, 2.5);
    let around = pos(&app, c);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Climb);
    assert_eq!(triggered(&app, c, "ledge_corner"), 1);
    let h = m.hang.unwrap();
    assert!(
        h.corner.is_none() && h.normal.distance(Vec3::Z) < 0.05,
        "{h:?}"
    );
    assert!(m.facing.distance(Vec3::NEG_Z) < 0.1, "{}", m.facing);
    assert!((around.z - (4.0 + 0.35 + 0.04)).abs() < 0.05, "{around:?}");
    assert!(around.x > 1.3, "{around:?}");
    assert!((around.y - p.y).abs() < 0.05, "{around:?}");

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
fn shimmies_into_an_inside_corner_and_on_along_the_other_wall() {
    let mut app = app();
    let c = hang_from_a_high_wall(&mut app);
    // A wall across the way: its face at z = 2 runs out in front of the first one.
    block(
        &mut app,
        DVec3::new(1.0, 1.8, 3.0),
        Vec3::new(4.0, 3.6, 2.0),
    );
    let p = pos(&app, c);
    intent(&mut app, c, |i| i.direction = Vec3::Z);
    let secs = run_until(&mut app, 3.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.hang
            .is_some_and(|h| h.corner.is_none() && h.normal.dot(Vec3::NEG_Z) > 0.99)
    });
    let m = mover(&app, c);
    assert!(secs < 2.5, "{:?}", m.hang);
    assert_eq!(triggered(&app, c, "ledge_corner"), 1);
    // Hanging from the second wall, clear of the first, and still going (the stick still
    // held toward +Z): along the second wall, away from the corner.
    let q = pos(&app, c);
    assert!((q.z - (2.0 - 0.35 - 0.04)).abs() < 0.06, "{q:?}");
    assert!(q.x < 1.0 - 0.35, "{q:?}");
    run(&mut app, 0.3);
    let r = pos(&app, c);
    assert_eq!(mover(&app, c).mode, LocomotionMode::Climb);
    assert!(r.x < q.x - 0.2, "{q:?} → {r:?}");
    assert!((r.y - p.y).abs() < 0.05, "{r:?}");
}

#[test]
fn stops_in_a_corner_whose_other_wall_is_too_high() {
    let mut app = app();
    let c = hang_from_a_high_wall(&mut app);
    block(
        &mut app,
        DVec3::new(1.0, 3.0, 3.0),
        Vec3::new(4.0, 6.0, 2.0),
    );
    intent(&mut app, c, |i| i.direction = Vec3::Z);
    run(&mut app, 2.0);
    let m = mover(&app, c);
    let q = pos(&app, c);
    assert_eq!(m.mode, LocomotionMode::Climb);
    assert_eq!(triggered(&app, c, "ledge_corner"), 0);
    assert!(m.hang.unwrap().normal.distance(Vec3::NEG_X) < 0.05);
    assert!(q.z > 1.4 && q.z < 1.7, "{q:?}");
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

/// A 9 m roof whose face is at x = 6 (8 m deep, z from −4 to 4) and a pilot 6 m in front of
/// it.
fn under_a_roof(app: &mut App) -> Entity {
    block(app, DVec3::new(10.0, 4.5, 0.0), Vec3::new(8.0, 9.0, 8.0));
    let c = character(app, DVec3::new(0.0, 0.9, 0.0), MovementProfile::pilot());
    run(app, 0.1);
    c
}

/// Fire the grapple at `at` (one step).
fn fire_grapple(app: &mut App, c: Entity, at: DVec3) {
    app.world.get_mut::<Mover>(c).unwrap().pending_grapple = Some(at);
    app.update();
}

fn hooked(w: &mut World, c: Entity) -> bool {
    w.get::<Mover>(c)
        .unwrap()
        .grapple
        .is_some_and(|g| g.attached)
}

#[test]
fn grapples_up_onto_a_roof() {
    let mut app = app();
    let c = under_a_roof(&mut app);
    // Standing still, hook the roof's edge: the rope lifts the pilot off the ground, reels it
    // up the wall and it climbs over the top.
    fire_grapple(&mut app, c, DVec3::new(6.0, 8.6, 0.0));
    assert_eq!(triggered(&app, c, "grapple_fire"), 1);
    // The hook flies the 9.5 m at 90 m/s, then bites.
    let flight = run_until(&mut app, 0.5, |w| hooked(w, c));
    assert!(flight > 0.08 && flight < 0.15, "hook flew {flight:.2} s");
    assert_eq!(triggered(&app, c, "grapple_hit"), 1);
    let mut fastest: f32 = 0.0;
    let secs = run_until(&mut app, 3.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        fastest = fastest.max(m.velocity.length());
        let on_top = w.get::<Transform>(c).unwrap().translation.y > 9.5;
        m.grounded && m.mode != LocomotionMode::Mantle && on_top
    });
    let p = pos(&app, c);
    let m = mover(&app, c);
    assert!(
        secs < 1.6 && p.x > 6.2,
        "on the roof after {secs:.2} s at {p:?}"
    );
    assert!((p.y - (9.0 + 0.9)).abs() < 0.05, "{p:?}");
    assert_eq!(triggered(&app, c, "mantle"), 1);
    assert_eq!(triggered(&app, c, "grapple_release"), 1);
    assert!(m.grapple.is_none() && m.grapple_cooldown() > 1.5);
    let limit = MovementProfile::pilot().grapple_speed * 1.3;
    assert!(fastest <= limit + 0.01, "{fastest} m/s");
    // Over the edge at a run, not flung across the roof.
    assert!(m.horizontal_speed() <= MovementProfile::pilot().sprint_speed + 0.01);
}

#[test]
fn a_shot_at_nothing_comes_back_and_a_jump_lets_go() {
    let mut app = app();
    let c = under_a_roof(&mut app);
    // Straight up: nothing to bite. The hook flies out to full range and comes back.
    fire_grapple(&mut app, c, DVec3::new(0.0, 50.0, 0.0));
    assert!(mover(&app, c).grapple.is_some_and(|g| g.miss));
    run(&mut app, 1.2);
    assert!(mover(&app, c).grapple.is_none());
    assert!(pos(&app, c).y < 1.0, "never pulled: {:?}", pos(&app, c));
    assert_eq!(triggered(&app, c, "grapple_hit"), 0);

    // Hook the wall high up and jump halfway: an air jump that keeps the rope's momentum.
    fire_grapple(&mut app, c, DVec3::new(6.0, 7.0, 0.0));
    run_until(&mut app, 0.5, |w| hooked(w, c));
    run(&mut app, 0.25);
    let before = mover(&app, c).velocity;
    assert!(before.x > 5.0 && before.y > 3.0, "{before}");
    tap_jump(&mut app, c);
    let m = mover(&app, c);
    assert!(m.grapple.is_none());
    assert_eq!(triggered(&app, c, "grapple_release"), 1);
    assert_eq!(triggered(&app, c, "double_jump"), 1);
    assert!(m.velocity.x > before.x * 0.9, "{} vs {before}", m.velocity);
    // Recharging: a shot right away does nothing.
    fire_grapple(&mut app, c, DVec3::new(6.0, 7.0, 0.0));
    assert!(mover(&app, c).grapple.is_none());
    assert_eq!(triggered(&app, c, "grapple_fire"), 2);
}

#[test]
fn humans_have_no_grapple() {
    let mut app = app();
    block(
        &mut app,
        DVec3::new(10.0, 4.5, 0.0),
        Vec3::new(8.0, 9.0, 8.0),
    );
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::human(),
    );
    run(&mut app, 0.1);
    fire_grapple(&mut app, c, DVec3::new(6.0, 8.6, 0.0));
    run(&mut app, 0.5);
    assert!(mover(&app, c).grapple.is_none());
    assert_eq!(triggered(&app, c, "grapple_fire"), 0);
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

/// Moves a body at a steady velocity, m/s (a lift, a ship sliding past). The stand-in physics
/// sees colliders where their transforms are each step.
#[derive(Component)]
struct Drift(Vec3);

fn drift(time: Res<Time>, mut q: Query<(&mut Transform, &Drift)>) {
    for (mut t, d) in &mut q {
        t.translation += (d.0 * time.delta_secs()).as_dvec3();
    }
}

fn drifting(app: &mut App) {
    app.add_systems(FixedUpdate, drift.in_set(SimSet::Physics));
}

#[test]
fn wall_runs_on_a_moving_wall_and_jumps_off_with_its_speed() {
    let mut app = app();
    drifting(&mut app);
    // The wall from the wall-run test, sliding away from the runner and along its length.
    let wall = block(
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
    tap_jump(&mut app, c);
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(0.2, 0.0, 1.0).normalize()
    });
    run_until(&mut app, 1.0, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::WallRun
    });
    intent(&mut app, c, |i| i.direction = Vec3::X);
    // On the wall: it starts sliding away from the runner and along its length.
    let speed = Vec3::new(3.0, 0.0, 2.0);
    app.world.entity_mut(wall).insert(Drift(speed));
    let mut wallrun_secs = 0.0;
    run_until(&mut app, 1.2, |w| {
        if w.get::<Mover>(c).unwrap().mode == LocomotionMode::WallRun {
            wallrun_secs += DT;
        }
        false
    });
    let m = mover(&app, c);
    assert!(wallrun_secs > 1.1, "wall-ran for {wallrun_secs:.2} s");
    assert_eq!(m.mode, LocomotionMode::WallRun);
    // Still against the wall, which slid 2.4 m away from where it started.
    let face = app.world.get::<Transform>(wall).unwrap().translation.z - 0.3;
    let p = pos(&app, c);
    assert!(face > 3.5, "{face}");
    assert!(
        (face - 0.35 - p.z).abs() < 0.15,
        "wall at {face:.2}, runner at {p:?}"
    );
    // Running at full speed along the wall, plus the wall's own speed.
    let world = app.world.get::<LocomotionState>(c).unwrap().velocity;
    assert!(m.velocity.x > 9.0, "along the wall: {}", m.velocity);
    assert!(world.x > 12.0 && world.z > 1.5, "in the world: {world}");

    // Wall-jump: off the wall, keeping its speed.
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(0.2, 0.0, 1.0).normalize()
    });
    tap_jump(&mut app, c);
    assert_eq!(mover(&app, c).mode, LocomotionMode::Air);
    app.update();
    let m = mover(&app, c);
    assert!(m.platform.is_none());
    assert!(
        m.velocity.x > 12.0 && m.velocity.z < -2.0 && m.velocity.y > 2.0,
        "{}",
        m.velocity
    );
}

/// The wall from the wall-run test moving along its length at `speed` (m/s along +X), with
/// a `Velocity` that says so.
fn sliding_wall(app: &mut App, speed: f32) -> Entity {
    drifting(app);
    let wall = block(app, DVec3::new(15.0, 2.5, 1.5), Vec3::new(30.0, 5.0, 0.6));
    app.world.entity_mut(wall).insert((
        Drift(Vec3::X * speed),
        Velocity {
            linear: Vec3::X * speed,
            angular: Vec3::ZERO,
        },
    ));
    wall
}

fn wall_ran(app: &mut App, c: Entity, secs: f64) -> f64 {
    let mut wallrun_secs = 0.0;
    run_until(app, secs, |w| {
        if w.get::<Mover>(c).unwrap().mode == LocomotionMode::WallRun {
            wallrun_secs += DT;
        }
        false
    });
    wallrun_secs
}

#[test]
fn running_beside_a_wall_as_fast_as_it_moves_is_no_wall_run() {
    let mut app = app();
    sliding_wall(&mut app, 7.0);
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::pilot(),
    );
    // Sprint beside it and jump at it, as in the wall-run test: 1.5 m/s past it is too slow.
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 1.0);
    tap_jump(&mut app, c);
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(0.2, 0.0, 1.0).normalize()
    });
    let secs = wall_ran(&mut app, c, 1.2);
    assert_eq!(secs, 0.0, "wall-ran for {secs:.2} s");
    assert_eq!(triggered(&app, c, "wallrun"), 0);
}

#[test]
fn a_jump_at_a_wall_sliding_past_runs_along_it() {
    let mut app = app();
    sliding_wall(&mut app, -9.0);
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 0.0),
        MovementProfile::pilot(),
    );
    // Walk up to it and jump: barely moving along it, but it slides past at 9 m/s.
    intent(&mut app, c, |i| i.direction = Vec3::new(0.0, 0.0, 0.4));
    run(&mut app, 0.3);
    tap_jump(&mut app, c);
    intent(&mut app, c, |i| {
        i.direction = Vec3::new(0.2, 0.0, 1.0).normalize()
    });
    run_until(&mut app, 1.0, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::WallRun
    });
    assert_eq!(triggered(&app, c, "wallrun"), 1);
    // It runs along the wall the way the wall passes it (+X relative to the wall).
    intent(&mut app, c, |i| i.direction = Vec3::X);
    let secs = wall_ran(&mut app, c, 0.6);
    assert!(secs > 0.5, "wall-ran for {secs:.2} s");
    let m = mover(&app, c);
    assert!(m.velocity.x > 6.0, "along the wall: {}", m.velocity);
}

#[test]
fn hangs_from_a_moving_ledge_and_climbs_onto_it() {
    let mut app = app();
    drifting(&mut app);
    let c = hang_from_a_high_wall(&mut app);
    let start = pos(&app, c);
    // The wall becomes a lift that also slides along its ledge.
    let wall = app
        .world
        .query_filtered::<Entity, With<engine_world::physics::Collider>>()
        .iter(&app.world)
        .next()
        .unwrap();
    let speed = Vec3::new(0.0, 0.5, 2.0);
    app.world.entity_mut(wall).insert(Drift(speed));
    run(&mut app, 1.5);
    let p = pos(&app, c);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Climb);
    assert_eq!(m.platform.map(|d| d.entity), Some(wall));
    let rode = p - start;
    assert!(
        (rode - (speed * 1.5).as_dvec3()).length() < 0.1,
        "rode {rode:?}"
    );
    assert!(
        m.velocity.length() < 0.3,
        "still against the wall: {}",
        m.velocity
    );

    // Climb up: the mantle follows the moving ledge onto the top.
    tap_jump(&mut app, c);
    assert_eq!(mover(&app, c).mode, LocomotionMode::Mantle);
    let secs = run_until(&mut app, 1.5, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.grounded && m.mode != LocomotionMode::Mantle
    });
    let top = app.world.get::<Transform>(wall).unwrap().translation.y + 1.8;
    let p = pos(&app, c);
    assert!(secs < 1.2 && p.x > 1.2, "on top after {secs:.2} s at {p:?}");
    assert!(
        (p.y - (top + 0.9)).abs() < 0.08,
        "{p:?} on a top at {top:.2}"
    );
}

#[test]
fn a_grapple_hook_rides_a_moving_roof() {
    let mut app = app();
    drifting(&mut app);
    let c = under_a_roof(&mut app);
    let roof = app
        .world
        .query_filtered::<Entity, With<engine_world::physics::Collider>>()
        .iter(&app.world)
        .next()
        .unwrap();
    app.world
        .entity_mut(roof)
        .insert(Drift(Vec3::new(0.0, 0.0, 3.0)));
    fire_grapple(&mut app, c, DVec3::new(6.0, 8.6, 0.0));
    // The hook stays where it bit on the roof while the roof slides on.
    let mut held = 0;
    let secs = run_until(&mut app, 3.0, |w| {
        let roof_z = w.get::<Transform>(roof).unwrap().translation.z;
        let m = w.get::<Mover>(c).unwrap();
        if let Some(g) = m.grapple.filter(|g| g.attached) {
            // (The roof slides on after the character's step.)
            let then = roof_z - 3.0 * DT;
            assert!((g.anchor.z - then).abs() < 0.01, "{} vs {then}", g.anchor.z);
            assert!((g.anchor_velocity.z - 3.0).abs() < 0.01);
            held += 1;
        }
        let on_top = w.get::<Transform>(c).unwrap().translation.y > 9.5;
        m.grounded && m.mode != LocomotionMode::Mantle && on_top
    });
    assert!(held > 10, "held for {held} steps");
    let p = pos(&app, c);
    assert!(
        secs < 1.8 && p.x > 6.2,
        "on the roof after {secs:.2} s at {p:?}"
    );
    // Pulled along with the roof on the way up, and over its edge.
    assert!(p.z > 2.0, "{p:?}");
    assert!((p.y - (9.0 + 0.9)).abs() < 0.05, "{p:?}");
}

#[test]
fn the_stand_in_carries_a_character_on_a_moving_box_and_a_jump_lands_on_the_same_spot() {
    let mut app = app();
    drifting(&mut app);
    let deck = block(
        &mut app,
        DVec3::new(0.0, 0.5, 0.0),
        Vec3::new(6.0, 1.0, 6.0),
    );
    let c = character(
        &mut app,
        DVec3::new(0.0, 1.9, 0.0),
        MovementProfile::pilot(),
    );
    run(&mut app, 0.3);
    app.world
        .entity_mut(deck)
        .insert(Drift(Vec3::new(4.0, 0.0, 1.0)));
    run(&mut app, 1.0);
    let rel = |app: &App| pos(app, c) - app.world.get::<Transform>(deck).unwrap().translation;
    let before = rel(&app);
    assert!(mover(&app, c).grounded);
    assert!(before.x.abs() < 0.1 && before.z.abs() < 0.1, "{before:?}");
    let world = app.world.get::<LocomotionState>(c).unwrap().velocity;
    assert!(
        (world.x - 4.0).abs() < 0.1 && (world.z - 1.0).abs() < 0.1,
        "{world}"
    );

    tap_jump(&mut app, c);
    let mut flew = 0.0;
    run_until(&mut app, 2.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        flew = m.air_secs.max(flew);
        m.grounded && flew > 0.3
    });
    let after = rel(&app);
    assert!(flew > 0.3, "never left the box");
    assert!((after - before).length() < 0.15, "{before:?} -> {after:?}");
}

// ------------------------------------------------------------------------ zip lines

/// A 30 m zip line along +X from x = −3, dropping `drop` meters, with 0.3 m of sag and its
/// cable 5.35 m up at x = 1.5, and a 2 m high deck to jump into it from (its edge at x = −0.5).
fn zip_line(app: &mut App, drop: f32) -> Entity {
    block(app, DVec3::new(-6.0, 1.0, 0.0), Vec3::new(11.0, 2.0, 4.0));
    app.world
        .spawn((
            Transform::from_translation(DVec3::new(-3.0, 5.5 + 0.15 * drop as f64, 0.0)),
            Zipline {
                to: Vec3::new(30.0, -drop, 0.0),
                sag: 0.3,
                end_clear: 1.5,
                exit_hop: 2.0,
            },
        ))
        .id()
}

/// A pilot sprinting +X along the deck that jumps into the line at its edge.
fn jump_into_the_line(app: &mut App) -> Entity {
    let c = character(app, DVec3::new(-9.0, 2.9, 0.0), MovementProfile::pilot());
    intent(app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run_until(app, 2.0, |w| {
        w.get::<Transform>(c).unwrap().translation.x > -1.2
    });
    tap_jump(app, c);
    intent(app, c, |i| i.direction = Vec3::ZERO);
    run_until(app, 1.0, |w| w.get::<Mover>(c).unwrap().zip.is_some());
    c
}

fn cable_y(app: &App, line: Entity, x: f64) -> f64 {
    let t = app.world.get::<Transform>(line).unwrap();
    let z = app.world.get::<Zipline>(line).unwrap();
    let s = ((x - t.translation.x) / z.to.x as f64) as f32;
    let c = engine_movement::zipline::Cable::new(t, z, Vec3::Y);
    c.grip(s).y
}

#[test]
fn jumps_into_a_zip_line_rides_it_down_and_lets_go_at_the_end() {
    let mut app = app();
    let line = zip_line(&mut app, 4.0);
    let c = jump_into_the_line(&mut app);
    assert_eq!(triggered(&app, c, "zip_grab"), 1);
    let m = mover(&app, c);
    let z = m.zip.expect("caught the line");
    assert_eq!(z.dir, 1.0, "rides the way it ran");
    assert_eq!(m.air_jumps_left, 1);

    // Hanging under the cable, feet 1.95 m below it, facing down the line, at speed.
    run(&mut app, 1.0);
    let p = pos(&app, c);
    let m = mover(&app, c);
    assert!(m.zip.is_some());
    let below = cable_y(&app, line, p.x) - p.y;
    assert!(
        (below - 1.8 * 0.58).abs() < 0.08,
        "hangs {below:.2} m below"
    );
    assert!(m.facing.dot(Vec3::X) > 0.98, "{}", m.facing);
    let speed = m.velocity.x;
    assert!(speed > 11.0 && speed < 14.5, "{speed:.1} m/s");
    assert!(!m.grounded);

    // Lets go 1.5 m before the far anchor with a hop, flies on and lands running.
    let end_x = -3.0 + 30.0;
    run_until(&mut app, 3.0, |w| w.get::<Mover>(c).unwrap().zip.is_none());
    let p = pos(&app, c);
    assert!(
        (p.x - (end_x - 1.5)).abs() < 0.6,
        "let go at x = {:.2}",
        p.x
    );
    assert_eq!(triggered(&app, c, "zip_release"), 1);
    assert!(mover(&app, c).velocity.y > 0.0, "a hop off the end");
    run_until(&mut app, 2.0, |w| w.get::<Mover>(c).unwrap().grounded);
    assert!(mover(&app, c).horizontal_speed() > 6.0);
}

#[test]
fn a_jump_lets_go_of_a_zip_line_and_the_double_jump_is_back() {
    let mut app = app();
    zip_line(&mut app, 4.0);
    let c = jump_into_the_line(&mut app);
    // The jump into the line used no air jump; spend it, then catch nothing for a moment.
    run(&mut app, 0.6);
    let before = mover(&app, c).velocity;
    tap_jump(&mut app, c);
    let m = mover(&app, c);
    assert!(m.zip.is_none());
    assert_eq!(triggered(&app, c, "jump"), 2);
    assert!(m.velocity.y > 4.0, "{}", m.velocity);
    assert!(
        (m.velocity.x - before.x).abs() < 0.5,
        "keeps its speed: {}",
        m.velocity
    );
    // Up and away from the cable, the air jump still works.
    run(&mut app, 0.3);
    tap_jump(&mut app, c);
    assert_eq!(triggered(&app, c, "double_jump"), 1);
    // It doesn't catch the line again on the way down past it.
    run_until(&mut app, 3.0, |w| w.get::<Mover>(c).unwrap().grounded);
    assert_eq!(triggered(&app, c, "zip_grab"), 1);

    // Crouch drops off without a jump.
    let mut app2 = app_with_line(4.0);
    let c2 = jump_into_the_line(&mut app2);
    run(&mut app2, 0.3);
    intent(&mut app2, c2, |i| i.crouch = true);
    app2.update();
    let m = mover(&app2, c2);
    assert!(m.zip.is_none());
    assert_eq!(triggered(&app2, c2, "zip_release"), 1);
    assert!(m.velocity.y < 0.5, "drops: {}", m.velocity);
}

fn app_with_line(drop: f32) -> App {
    let mut app = app();
    zip_line(&mut app, drop);
    app
}

#[test]
fn zip_lines_are_faster_downhill_than_uphill() {
    let speed_after = |drop: f32| {
        let mut app = app_with_line(drop);
        let c = jump_into_the_line(&mut app);
        run(&mut app, 1.2);
        mover(&app, c).zip.expect("still riding").speed
    };
    let down = speed_after(5.0);
    let level = speed_after(0.0);
    let up = speed_after(-5.0);
    assert!(
        down > level + 0.6 && level > up + 0.6,
        "{down:.1} {level:.1} {up:.1}"
    );
    assert!(up > 7.0, "still climbs at {up:.1} m/s");
}

#[test]
fn a_zip_line_rider_s_camera_widens_like_a_sprinter_s() {
    let mut app = app();
    zip_line(&mut app, 4.0);
    let c = jump_into_the_line(&mut app);
    let cam = app
        .world
        .spawn((
            engine_render::Camera::default(),
            Transform::default(),
            CameraFx {
                target: Some(c),
                ..CameraFx::default()
            },
        ))
        .id();
    run(&mut app, 1.0);
    assert!(mover(&app, c).zip.is_some());
    let fx = app.world.get::<CameraFx>(cam).unwrap();
    assert!(
        fx.fov_offset_deg > 0.9 * fx.sprint_fov_deg,
        "{:.1}°",
        fx.fov_offset_deg
    );
}

#[test]
fn a_rider_moves_with_its_zip_line() {
    let mut app = app();
    let line = zip_line(&mut app, 4.0);
    let c = jump_into_the_line(&mut app);
    run(&mut app, 0.3);
    // The line moves 0.8 m sideways at once: the rider is pulled after it and hangs under it
    // again.
    app.world.get_mut::<Transform>(line).unwrap().translation.z += 0.8;
    run(&mut app, 0.4);
    let m = mover(&app, c);
    assert!(m.zip.is_some(), "still on the line");
    assert!((pos(&app, c).z - 0.8).abs() < 0.05, "{:?}", pos(&app, c));
}

// ------------------------------------------------------------------------ ladders

/// A 6 m wall whose north face is at z = 0 (x from −3 to 3) with a ladder up it at x = 0:
/// climbed facing −Z 0.45 m out from the face, feet on the lowest rung 0.3 m up and on the top
/// one at the top, stepped off 1.5 m in onto the roof.
fn ladder_wall(app: &mut App) -> Entity {
    block(app, DVec3::new(0.0, 3.0, -2.0), Vec3::new(6.0, 6.0, 4.0));
    app.world
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 0.0, 0.45)),
            Climbable::ladder(Vec3::Y * 0.3, Vec3::Y * 6.0, Vec3::NEG_Z)
                .with_exit(Vec3::new(0.0, 6.0, -1.5)),
        ))
        .id()
}

/// A pilot 4 m out from the ladder walking at it until it grabs it.
fn walk_onto_the_ladder(app: &mut App) -> Entity {
    let c = character(app, DVec3::new(0.0, 0.9, 4.0), MovementProfile::pilot());
    intent(app, c, |i| i.direction = Vec3::NEG_Z);
    run_until(app, 3.0, |w| w.get::<Mover>(c).unwrap().ladder.is_some());
    c
}

#[test]
fn climbs_a_ladder_onto_the_roof_and_back_down() {
    let mut app = app();
    ladder_wall(&mut app);
    let c = walk_onto_the_ladder(&mut app);
    assert_eq!(triggered(&app, c, "ladder_grab"), 1);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Climb);
    assert_eq!(m.air_jumps_left, 1);

    // Holding forward climbs at the profile's speed, on the line, facing the ladder.
    run(&mut app, 1.0);
    let m = mover(&app, c);
    let l = m.ladder.expect("on the ladder");
    assert!((l.speed - 2.6).abs() < 0.05, "{:.2} m/s", l.speed);
    let p = pos(&app, c);
    assert!(
        p.x.abs() < 0.02 && (p.z - 0.45).abs() < 0.02,
        "on the line: {p:?}"
    );
    assert!(
        (p.y - 0.9 - 0.3 - l.climbed as f64).abs() < 0.03,
        "{p:?} {}",
        l.climbed
    );
    assert!(m.facing.dot(Vec3::NEG_Z) > 0.99, "{}", m.facing);
    assert!(!m.grounded);

    // Over the top and onto the roof.
    let secs = run_until(&mut app, 4.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.grounded && m.mode != LocomotionMode::Mantle
    });
    assert!(secs < 2.6, "took {secs:.2} s");
    let p = pos(&app, c);
    assert!((p.y - 6.9).abs() < 0.05 && p.z < -0.3, "on the roof: {p:?}");
    assert_eq!(triggered(&app, c, "ladder_exit"), 1);

    // Walk back to the ladder: it's grabbed at the top, and holding the same way climbs down.
    intent(&mut app, c, |i| i.direction = Vec3::Z);
    run_until(&mut app, 2.0, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_some()
    });
    let l = mover(&app, c).ladder.expect("grabbed it from the top");
    assert!(l.t > 0.95, "{}", l.t);
    assert_eq!(triggered(&app, c, "ladder_grab"), 2);
    run(&mut app, 0.6);
    let m = mover(&app, c);
    assert!(
        m.facing.dot(Vec3::NEG_Z) > 0.99,
        "turned to face it: {}",
        m.facing
    );
    assert!(m.ladder.expect("climbing down").speed < -2.0);
    run_until(&mut app, 4.0, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_none()
    });
    run(&mut app, 0.5);
    let m = mover(&app, c);
    assert!(m.grounded && m.ladder.is_none());
    assert_eq!(triggered(&app, c, "ladder_exit"), 2);
    let p = pos(&app, c);
    assert!((p.y - 0.9).abs() < 0.05 && p.z > 0.6, "stepped off: {p:?}");
}

#[test]
fn rungs_ring_under_a_climber() {
    let mut app = app();
    #[derive(Resource, Default)]
    struct Rungs(Vec<GroundContact>);
    app.init_resource::<Rungs>();
    app.add_systems(
        Last,
        |mut seen: ResMut<Rungs>, mut contacts: MessageReader<GroundContact>| {
            seen.0
                .extend(contacts.read().filter(|c| c.position.y > 0.5).cloned());
        },
    );
    ladder_wall(&mut app);
    walk_onto_the_ladder(&mut app);
    run(&mut app, 1.5);
    let rungs = &app.world.resource::<Rungs>().0;
    // About one rung every 0.3 m climbed, in front of the line, on metal.
    assert!(rungs.len() >= 8, "{} rungs", rungs.len());
    let metal = app
        .world
        .resource::<engine_world::SurfaceMaterials>()
        .id("metal");
    for r in rungs {
        assert!((r.position.z - 0.25).abs() < 1e-3, "{:?}", r.position);
        assert_eq!(r.surface, metal);
    }
    assert!(rungs.windows(2).all(|w| w[1].position.y > w[0].position.y));
}

#[test]
fn crouch_slides_down_and_jump_kicks_off() {
    let mut app = app();
    ladder_wall(&mut app);
    let c = walk_onto_the_ladder(&mut app);
    run(&mut app, 1.5);
    let high = pos(&app, c).y;
    intent(&mut app, c, |i| {
        i.direction = Vec3::ZERO;
        i.crouch = true;
    });
    run(&mut app, 0.6);
    let l = mover(&app, c).ladder.expect("still on");
    assert!(l.sliding && l.speed < -6.0, "{l:?}");
    assert!(pos(&app, c).y < high - 1.5);
    run_until(&mut app, 2.0, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_none()
    });
    run(&mut app, 0.3);
    assert!(mover(&app, c).grounded);

    // Climb again, then jump: kicked off backward, not grabbed again on the way down.
    intent(&mut app, c, |i| {
        i.crouch = false;
        i.direction = Vec3::NEG_Z;
    });
    run_until(&mut app, 2.0, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_some()
    });
    run(&mut app, 1.0);
    intent(&mut app, c, |i| i.direction = Vec3::ZERO);
    tap_jump(&mut app, c);
    let m = mover(&app, c);
    assert!(m.ladder.is_none());
    assert!(m.velocity.z > 3.5 && m.velocity.y > 3.0, "{}", m.velocity);
    let grabs = triggered(&app, c, "ladder_grab");
    run_until(&mut app, 3.0, |w| w.get::<Mover>(c).unwrap().grounded);
    assert_eq!(triggered(&app, c, "ladder_grab"), grabs);
    assert!(pos(&app, c).z > 1.5, "{:?}", pos(&app, c));
}

/// A roof deck 3.24 to 3.29 m up with a 1 m square hatch in it (x −0.5..0.5, z −2.1..−1.1)
/// and a header joist along its back edge, a steel ladder hung from the header up through the
/// hatch (as in the breach house): its rungs 0.18 m in from the header, climbed facing −Z
/// 0.2 m out from them, feet on the lowest rung 0.27 m up and on the top one 3.07 m up,
/// stepped off over the header onto the deck 0.35 m behind it.
fn roof_hatch(app: &mut App) -> Entity {
    let (y, t) = (3.265, 0.05);
    block(app, DVec3::new(0.0, y, 0.45), Vec3::new(4.0, t, 3.1));
    block(app, DVec3::new(0.0, y, -3.05), Vec3::new(4.0, t, 1.9));
    for x in [-1.25, 1.25] {
        block(app, DVec3::new(x, y, -1.6), Vec3::new(1.5, t, 1.0));
    }
    block(app, DVec3::new(0.0, 3.12, -2.1), Vec3::new(1.6, 0.24, 0.1));
    app.world
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 1.61, -1.93)),
            Climbable::ladder(
                Vec3::new(0.0, 0.25 - 1.59, 0.26),
                Vec3::new(0.0, 3.05 - 1.59, 0.26),
                Vec3::NEG_Z,
            )
            .with_exit(Vec3::new(0.0, 1.68, -0.52)),
        ))
        .id()
}

#[test]
fn climbs_up_through_a_roof_hatch_onto_the_deck() {
    // Climbing back down through it is checked against the real physics backend: the
    // stand-in pushes a capsule out of a box the shortest way, onto the header's edge.
    let mut app = app();
    roof_hatch(&mut app);
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 1.5),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| i.direction = Vec3::NEG_Z);
    run_until(&mut app, 3.0, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_some()
    });
    assert_eq!(triggered(&app, c, "ladder_grab"), 1);
    // Up through the hatch, over the header and onto the deck behind it.
    let secs = run_until(&mut app, 5.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.grounded && m.mode != LocomotionMode::Mantle && m.ladder.is_none()
    });
    let p = pos(&app, c);
    assert!(secs < 3.0, "took {secs:.2} s, at {p:?}");
    assert!(
        (p.y - 3.29 - 0.9).abs() < 0.05 && p.z < -2.2,
        "on the deck: {p:?}"
    );
    assert_eq!(triggered(&app, c, "ladder_exit"), 1);
}

#[test]
fn waits_a_rung_or_two_below_a_closed_hatch_until_it_opens() {
    let mut app = app();
    roof_hatch(&mut app);
    let cover = block(
        &mut app,
        DVec3::new(0.0, 3.265, -1.6),
        Vec3::new(1.0, 0.05, 1.0),
    );
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 1.5),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| i.direction = Vec3::NEG_Z);
    run_until(&mut app, 3.0, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_some()
    });
    // Up the ladder, then held with room over the head: out of the way of a cover that drops.
    let mut head = 0.0f64;
    let secs = run_until(&mut app, 4.0, |w| {
        head = head.max(w.get::<Transform>(c).unwrap().translation.y + 0.9);
        false
    });
    assert!(secs >= 4.0);
    let underside = 3.24;
    assert!(
        underside - head >= 0.4 && underside - head < 0.6,
        "head {head:.2} under a cover at {underside}"
    );
    assert!(app.world.get::<Mover>(c).unwrap().ladder.is_some());
    // Breached: on up and onto the deck.
    app.world.despawn(cover);
    run_until(&mut app, 4.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.grounded && m.mode != LocomotionMode::Mantle && m.ladder.is_none()
    });
    let p = pos(&app, c);
    assert!(
        (p.y - 3.29 - 0.9).abs() < 0.05 && p.z < -2.2,
        "on the deck: {p:?}"
    );
}

#[test]
fn a_ladder_guest_climbs_any_ladder_up_and_back_down() {
    let mut app = app();
    let ladder = ladder_wall(&mut app);
    let r = engine_core::registry::call_api(
        &mut app.world,
        "movement.ladder_guest",
        serde_json::json!({"near": [0.0, 0.0, 3.0]}),
    )
    .unwrap();
    assert_eq!(r["ladder"].as_u64(), Some(ladder.to_bits()));
    let guest = Entity::from_bits(r["guest"].as_u64().unwrap());
    // Up onto the roof…
    let mut top = 0.0f64;
    let secs = run_until(&mut app, 8.0, |w| {
        top = top.max(w.get::<Transform>(guest).unwrap().translation.y);
        triggered_in(w, guest, "ladder_exit") == 1
    });
    assert!(secs < 6.0, "{secs:.2} s");
    // …and back down to the floor.
    run_until(&mut app, 8.0, |w| {
        top = top.max(w.get::<Transform>(guest).unwrap().translation.y);
        triggered_in(w, guest, "ladder_exit") == 2
    });
    run(&mut app, 0.5);
    assert!(top > 6.8, "reached the roof: {top}");
    assert_eq!(triggered(&app, guest, "ladder_grab"), 2);
    let p = pos(&app, guest);
    assert!(p.y < 1.0, "back on the floor: {p:?}");
    // Calling again restarts the same guest.
    let again = engine_core::registry::call_api(
        &mut app.world,
        "movement.ladder_guest",
        serde_json::json!({"near": [0.0, 0.0, 3.0]}),
    )
    .unwrap();
    assert_eq!(again["guest"], r["guest"]);
}

#[test]
fn a_jump_into_a_ladder_catches_it() {
    let mut app = app();
    ladder_wall(&mut app);
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 5.0),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| {
        i.direction = Vec3::NEG_Z;
        i.sprint = true;
    });
    run_until(&mut app, 2.0, |w| {
        w.get::<Transform>(c).unwrap().translation.z < 3.2
    });
    tap_jump(&mut app, c);
    intent(&mut app, c, |i| i.direction = Vec3::ZERO);
    run_until(&mut app, 1.5, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_some()
    });
    let l = mover(&app, c).ladder.expect("caught it");
    assert!(l.climbed > 0.4, "caught {:.2} m up", l.climbed);
    // Let go of the stick: it hangs on.
    run(&mut app, 0.5);
    let m = mover(&app, c);
    assert!(m.ladder.is_some_and(|l| l.speed.abs() < 0.01));
    assert!(!m.grounded);
}

#[test]
fn a_moving_ladder_carries_its_climber_and_one_that_goes_drops_it() {
    let mut app = app();
    drifting(&mut app);
    // A free-standing ladder (no wall), sliding along +X at 1.5 m/s.
    let ladder = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 0.0, 0.45)),
            Climbable::ladder(Vec3::Y * 0.3, Vec3::Y * 8.0, Vec3::NEG_Z),
            Drift(Vec3::X * 1.5),
        ))
        .id();
    let c = character(
        &mut app,
        DVec3::new(0.0, 0.9, 1.6),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| i.direction = Vec3::NEG_Z);
    run_until(&mut app, 2.0, |w| {
        w.get::<Mover>(c).unwrap().ladder.is_some()
    });
    intent(&mut app, c, |i| i.direction = Vec3::ZERO);
    run(&mut app, 1.0);
    run(&mut app, 1.0);
    let lx = app.world.get::<Transform>(ladder).unwrap().translation.x;
    let p = pos(&app, c);
    assert!(mover(&app, c).ladder.is_some());
    assert!(
        (p.x - lx).abs() < 0.05,
        "{:.2} vs the ladder at {lx:.2}",
        p.x
    );
    // Loco reports the world's motion: the ladder's.
    let loco = app.world.get::<LocomotionState>(c).unwrap();
    assert!((loco.velocity.x - 1.5).abs() < 0.1, "{}", loco.velocity);

    app.world.entity_mut(ladder).remove::<Climbable>();
    app.update();
    assert!(mover(&app, c).ladder.is_none());
    assert_eq!(triggered(&app, c, "ladder_exit"), 1);
    run_until(&mut app, 2.0, |w| w.get::<Mover>(c).unwrap().grounded);
    assert!(mover(&app, c).grounded);
}

// ------------------------------------------------------------------------ vaults

/// A barrier `height` high and `depth` deep across the +X run, its face at x = 6, 6 m wide.
fn barrier(app: &mut App, height: f32, depth: f32) -> Entity {
    block(
        app,
        DVec3::new(6.0 + depth as f64 * 0.5, height as f64 * 0.5, 0.0),
        Vec3::new(depth, height, 6.0),
    )
}

/// A pilot at x = 0 running +X (sprinting or not).
fn runner(app: &mut App, sprint: bool) -> Entity {
    let c = character(app, DVec3::new(0.0, 0.9, 0.0), MovementProfile::pilot());
    intent(app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = sprint;
    });
    c
}

#[test]
fn sprints_over_a_low_barrier_without_slowing_down() {
    let mut app = app();
    #[derive(Resource, Default)]
    struct Hands(Vec<GroundContact>);
    app.init_resource::<Hands>();
    app.add_systems(
        Last,
        |mut seen: ResMut<Hands>, mut contacts: MessageReader<GroundContact>| {
            seen.0.extend(
                contacts
                    .read()
                    .filter(|c| c.kind == engine_world::ContactKind::Body)
                    .cloned(),
            );
        },
    );
    barrier(&mut app, 1.0, 0.5);
    let c = runner(&mut app, true);
    run_until(&mut app, 2.0, |w| {
        w.get::<Mover>(c).unwrap().vault.is_some()
    });
    assert_eq!(triggered(&app, c, "vault"), 1);
    let before = mover(&app, c).velocity.x;
    assert!(before > 8.0, "{before:.1} m/s at the take-off");
    // Took off late: the rise just clears the top as the body reaches the face.
    let gap = 6.0 - (pos(&app, c).x + 0.35);
    assert!(
        gap > 0.5 && gap < 1.6,
        "took off {gap:.2} m before the face"
    );

    // Over the top without touching it, the hand planted on it, then down the far side.
    let mut lowest = f64::MAX;
    run_until(&mut app, 1.5, |w| {
        let x = w.get::<Transform>(c).unwrap().translation;
        if (x.x - 6.25).abs() < 0.6 {
            lowest = lowest.min(x.y - 0.9);
        }
        w.get::<Mover>(c).unwrap().grounded
    });
    assert!(lowest > 0.98, "feet {lowest:.2} m over the 1 m top");
    let p = pos(&app, c);
    let m = mover(&app, c);
    assert!(m.grounded && p.x > 7.0 && p.x < 10.0, "landed at {p:?}");
    assert!(m.vault.is_none());
    assert!(
        m.velocity.x > 0.85 * before,
        "{:.1} m/s after, {before:.1} before",
        m.velocity.x
    );
    let hands = &app.world.resource::<Hands>().0;
    assert_eq!(hands.len(), 1, "{hands:?}");
    let h = hands[0].position;
    assert!((h.y - 1.0).abs() < 0.02 && h.x > 5.9 && h.x < 6.2, "{h:?}");
    assert!(h.z < -0.1, "planted with the left hand: {h:?}");
}

#[test]
fn a_running_jump_vaults_and_a_run_into_it_does_not() {
    // Running (not sprinting) into a railing stops at it.
    let mut app = app();
    barrier(&mut app, 1.1, 0.06);
    let c = runner(&mut app, false);
    run(&mut app, 2.0);
    assert_eq!(triggered(&app, c, "vault"), 0);
    let p = pos(&app, c);
    assert!(p.x < 6.0 && p.x > 5.4, "{p:?}");

    // Jumping into it from a run vaults it.
    let mut app = self::app();
    barrier(&mut app, 1.1, 0.06);
    let c = runner(&mut app, false);
    run_until(&mut app, 2.0, |w| {
        w.get::<Transform>(c).unwrap().translation.x > 4.3
    });
    tap_jump(&mut app, c);
    run_until(&mut app, 0.2, |w| {
        w.get::<Mover>(c).unwrap().vault.is_some()
    });
    assert_eq!(triggered(&app, c, "vault"), 1);
    assert_eq!(triggered(&app, c, "jump"), 0, "the vault took the jump");
    assert_eq!(
        mover(&app, c).air_jumps_left,
        1,
        "the double jump is still there"
    );
    run(&mut app, 1.2);
    let p = pos(&app, c);
    assert!(p.x > 7.5 && mover(&app, c).grounded, "{p:?}");
}

#[test]
fn a_jump_too_early_for_the_vault_vaults_out_of_the_air() {
    let mut app = app();
    barrier(&mut app, 1.3, 0.6);
    let c = runner(&mut app, true);
    run_until(&mut app, 2.0, |w| {
        w.get::<Transform>(c).unwrap().translation.x > 0.5
    });
    tap_jump(&mut app, c);
    run(&mut app, 1.5);
    assert_eq!(triggered(&app, c, "jump"), 1);
    assert_eq!(triggered(&app, c, "vault"), 1);
    let p = pos(&app, c);
    assert!(p.x > 8.0 && mover(&app, c).grounded, "{p:?}");
}

#[test]
fn deep_or_tall_obstacles_or_a_drop_beyond_are_not_vaulted() {
    // Deeper than the profile vaults: a sprint stops at it and a jump climbs onto it.
    let mut app = app();
    barrier(&mut app, 1.0, 3.0);
    let c = runner(&mut app, true);
    run(&mut app, 1.5);
    assert_eq!(triggered(&app, c, "vault"), 0);
    assert!(pos(&app, c).x < 6.0);
    tap_jump(&mut app, c);
    run_until(&mut app, 1.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.mantle.is_none() && m.grounded && m.mode_secs > 0.1
    });
    assert_eq!(triggered(&app, c, "vault"), 0);
    assert_eq!(triggered(&app, c, "mantle"), 1);
    let p = pos(&app, c);
    assert!((p.y - 1.9).abs() < 0.05 && p.x < 9.0, "on top: {p:?}");

    // Too tall: a top the probe finds, above the profile's limit (1.3 m), and one above
    // where the probe starts.
    for height in [1.4, 1.5] {
        let mut app = self::app();
        barrier(&mut app, height, 0.3);
        let c = runner(&mut app, true);
        run(&mut app, 1.5);
        assert_eq!(triggered(&app, c, "vault"), 0, "{height}");
        assert_eq!(triggered(&app, c, "mantle"), 0, "{height}");
    }

    // A parapet at a roof's edge: a 6 m drop beyond.
    let mut app = self::app();
    block(
        &mut app,
        DVec3::new(0.0, 3.0, 0.0),
        Vec3::new(12.15, 6.0, 12.0),
    );
    block(
        &mut app,
        DVec3::new(6.075, 6.5, 0.0),
        Vec3::new(0.15, 1.0, 6.0),
    );
    let c = character(
        &mut app,
        DVec3::new(0.0, 6.9, 0.0),
        MovementProfile::pilot(),
    );
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 1.5);
    assert_eq!(triggered(&app, c, "vault"), 0);
    let p = pos(&app, c);
    assert!(p.x < 6.0 && p.y > 6.5, "{p:?}");
}

/// Terrain from a height function, with normals from its slope (as the terrain module gives).
struct Terrain(fn(f64, f64) -> f64);

impl engine_world::GroundQuery for Terrain {
    fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
        let (h, e) = (self.0, 0.01);
        let dx = (h(p.x + e, p.z) - h(p.x - e, p.z)) / (2.0 * e);
        let dz = (h(p.x, p.z + e) - h(p.x, p.z - e)) / (2.0 * e);
        Some(engine_world::GroundSample {
            height: h(p.x, p.z),
            normal: Vec3::new(-dx as f32, 1.0, -dz as f32).normalize(),
            surface: engine_world::SurfaceId(0),
        })
    }
}

/// Play mode's pilot (level 10 reflexes, so its clock runs fast) on terrain `h`, feet at x, z.
fn pilot_on(h: fn(f64, f64) -> f64, x: f64, z: f64) -> (App, Entity) {
    let mut app = app();
    app.world
        .insert_resource(engine_world::Ground(std::sync::Arc::new(Terrain(h))));
    let c = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(x, h(x, z) + 0.95, z)),
            MovementProfile::pilot(),
            Tempo::level(10),
        ))
        .id();
    (app, c)
}

/// Playtest 2026-10-27 #23: a player standing still on a slope crept uphill (0.6 m every 5 s
/// on 10°, 1.5 m on 30°), because the stick-to-ground push pressed along the slope's normal.
#[test]
fn standing_still_on_a_slope_stays_put() {
    fn ten(x: f64, _: f64) -> f64 {
        x * 10f64.to_radians().tan()
    }
    fn thirty(x: f64, _: f64) -> f64 {
        x * 30f64.to_radians().tan()
    }
    for h in [ten as fn(f64, f64) -> f64, thirty] {
        let (mut app, c) = pilot_on(h, 0.0, 0.0);
        // Walk a little and stop, as a player does.
        intent(&mut app, c, |i| i.direction = Vec3::X);
        run(&mut app, 0.5);
        intent(&mut app, c, |i| i.direction = Vec3::ZERO);
        run(&mut app, 1.0);
        let stopped = pos(&app, c);
        run(&mut app, 5.0);
        let drift = pos(&app, c) - stopped;
        assert!(drift.length() < 0.05, "standing drifted {drift:?}");
        assert!(mover(&app, c).grounded);
    }
}

/// Rolling hills (bumps up to about 1 m), like the showcase valley.
fn rolling(x: f64, z: f64) -> f64 {
    (x * 0.31).sin() * 0.9
        + (z * 0.23).cos() * 0.8
        + (x * 1.7 + z * 0.9).sin() * 0.18
        + (x * 0.8 - z * 2.1).cos() * 0.12
}

/// Playtest #18 ("forces the player to jump every third step"): sprinting over rolling
/// ground left it every crest and fell back, 25 times in 6 s. Walking keeps to a floor it can
/// step down onto, as Unreal's walking mode does, so only real drops become falls.
#[test]
fn sprinting_over_rolling_ground_keeps_its_feet() {
    let (mut app, c) = pilot_on(rolling, -40.0, 0.0);
    run(&mut app, 0.5);
    let dir = Vec3::new(1.0, 0.0, 0.6).normalize();
    intent(&mut app, c, |i| {
        i.direction = dir;
        i.sprint = true;
    });
    let (mut falls, mut was_air) = (0, false);
    for _ in 0..(6.0 / DT) as usize {
        app.update();
        let air = mover(&app, c).mode == LocomotionMode::Air;
        falls += (air && !was_air) as u32;
        was_air = air;
    }
    assert!(falls <= 1, "left the ground {falls} times");
    assert!(pos(&app, c).x > -10.0, "barely moved: {:?}", pos(&app, c));
}

/// Playtest #30 ("walking into terrain ... wall run multiple times a second ... sending them
/// flying"): ground too steep to stand on was walked on as if it were a floor, turning a
/// sprint into it into a climb up the slope. Too-steep ground blocks like a wall instead, and
/// running into it never adds upward speed (Unreal's SlideAlongSurface and
/// HandleSlopeBoosting do the same).
#[test]
fn sprinting_into_a_too_steep_slope_does_not_climb_it() {
    fn cliff(x: f64, _: f64) -> f64 {
        (x - 4.0).max(0.0) * 70f64.to_radians().tan()
    }
    let (mut app, c) = pilot_on(cliff, 0.0, 0.0);
    run(&mut app, 0.5);
    // Head-on and at a glancing angle.
    for dir in [Vec3::X, Vec3::new(1.0, 0.0, 1.0).normalize()] {
        intent(&mut app, c, |i| {
            i.direction = dir;
            i.sprint = true;
        });
        let (mut top, mut fastest, mut rise) = (0.0f64, 0.0f32, 0.0f32);
        for _ in 0..(4.0 / DT) as usize {
            app.update();
            let m = mover(&app, c);
            top = top.max(pos(&app, c).y - 0.95);
            fastest = fastest.max(m.velocity.length());
            rise = rise.max(m.velocity.y);
        }
        assert!(top < 1.0, "climbed the slope to {top:.2} m");
        assert!(rise < 1.5, "launched upward at {rise:.1} m/s");
        assert!(fastest < 9.5, "sped up to {fastest:.1} m/s");
        assert_eq!(triggered(&app, c, "wallrun"), 0);
    }
}

/// A lake: still water up to y = 0 from x = 0 on.
struct Lake;

impl engine_world::WaterQuery for Lake {
    fn sample(&self, p: DVec3) -> Option<engine_world::WaterSample> {
        (p.x > 0.0).then_some(engine_world::WaterSample {
            surface_height: 0.0,
            depth: 10.0,
            velocity: Vec3::ZERO,
            normal: Vec3::Y,
        })
    }
}

/// What characters did to the water.
#[derive(Resource, Default)]
struct Pushed(Vec<engine_world::WaterDisturbance>);

fn collect_pushes(
    mut seen: ResMut<Pushed>,
    mut pushes: MessageReader<engine_world::WaterDisturbance>,
) {
    seen.0.extend(pushes.read().copied());
}

/// Playtest #6 ("wading into water ... no waves, no splash, and you can't swim"): a lake shore
/// sloping down 1 m every 4 m. Wading slows the character and pushes the water, deep water
/// floats it into swimming with its head out, and running off a jetty splashes.
#[test]
fn wading_slows_deep_water_swims_and_going_in_splashes() {
    fn shore(x: f64, _: f64) -> f64 {
        0.3 - x.max(0.0) * 0.25
    }
    let (mut app, c) = pilot_on(shore, -3.0, 0.0);
    app.world
        .insert_resource(engine_world::Water(std::sync::Arc::new(Lake)));
    app.init_resource::<Pushed>();
    app.add_systems(Last, collect_pushes);
    run(&mut app, 0.5);
    intent(&mut app, c, |i| i.direction = Vec3::X);
    // Knee to waist deep (0.45-0.7 m over the feet at x = 3-4): slower than on land.
    run_until(&mut app, 5.0, |w| {
        w.get::<Transform>(c).unwrap().translation.x > 3.3
    });
    let m = mover(&app, c);
    assert!(m.grounded && m.mode != LocomotionMode::Swim, "{:?}", m.mode);
    assert!(
        m.horizontal_speed() < 4.8 && m.horizontal_speed() > 2.5,
        "wading at {}",
        m.horizontal_speed()
    );
    assert!(!app.world.resource::<Pushed>().0.is_empty(), "no rings");

    // On into deep water: it swims, floating with its head out.
    run_until(&mut app, 8.0, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::Swim
    });
    run(&mut app, 3.0);
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Swim);
    assert!(
        (m.horizontal_speed() - 1.8).abs() < 0.2,
        "{}",
        m.horizontal_speed()
    );
    let head = pos(&app, c).y + 0.9;
    assert!(head > 0.2 && head < 1.0, "head at {head:.2}");

    // Turn back: walking out where the feet find ground again.
    intent(&mut app, c, |i| i.direction = -Vec3::X);
    run_until(&mut app, 15.0, |w| {
        w.get::<Transform>(c).unwrap().translation.x < 1.0
    });
    let m = mover(&app, c);
    assert!(m.grounded && m.mode != LocomotionMode::Swim, "{:?}", m.mode);

    // Running off a jetty 2 m up into deep water: one splash the size of a body going in fast.
    let (mut app, c) = pilot_on(|_, _| -6.0, -3.0, 0.0);
    block(
        &mut app,
        DVec3::new(-3.0, 1.5, 0.0),
        Vec3::new(6.0, 3.0, 4.0),
    );
    app.world
        .entity_mut(c)
        .get_mut::<Transform>()
        .unwrap()
        .translation
        .y = 3.95;
    app.world
        .insert_resource(engine_world::Water(std::sync::Arc::new(Lake)));
    app.init_resource::<Pushed>();
    app.add_systems(Last, collect_pushes);
    run(&mut app, 0.3);
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 2.0);
    let pushes = &app.world.resource::<Pushed>().0;
    let plunge = pushes.iter().find(|d| d.velocity.y < -4.0);
    assert!(plunge.is_some_and(|d| d.mass_kg >= 30.0), "{pushes:?}");
    assert_eq!(mover(&app, c).mode, LocomotionMode::Swim);
}
