//! The 2026-11-05 movement pass (AC Unity + Titanfall): the jump arc and air control,
//! climbing from ledge to ledge, dropping to a hang, running up walls, ropes, and no slow
//! motion from fights the player isn't in.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_movement::PerceptionStatus;
use engine_movement::prelude::*;
use engine_world::LocomotionMode;
use engine_world::locomotion::MoveIntent;
use support::*;

fn character(app: &mut App, pos: DVec3) -> Entity {
    let e = app
        .world
        .spawn((Transform::from_translation(pos), MovementProfile::pilot()))
        .id();
    app.update();
    // A player's jump: as high as it is held.
    app.world.get_mut::<Mover>(e).unwrap().variable_jump = true;
    e
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

fn floor(app: &mut App) {
    block(
        app,
        DVec3::new(0.0, -0.5, 0.0),
        Vec3::new(200.0, 1.0, 200.0),
    );
}

/// The highest the feet get in a jump with jump held for `hold` seconds.
fn jump_height(hold: f64) -> f64 {
    let mut app = app();
    floor(&mut app);
    let c = character(&mut app, DVec3::new(0.0, 0.9, 0.0));
    run(&mut app, 0.5);
    let start = pos(&app, c).y;
    intent(&mut app, c, |i| i.jump = true);
    let mut top = start;
    let mut t = 0.0;
    run_until(&mut app, 2.0, |w| {
        t += DT;
        if t >= hold {
            w.get_mut::<MoveIntent>(c).unwrap().jump = false;
        }
        let y = w.get::<Transform>(c).unwrap().translation.y;
        top = top.max(y);
        w.get::<Mover>(c).unwrap().grounded && t > 0.2
    });
    top - start
}

#[test]
fn a_tapped_jump_is_low_and_a_held_one_high() {
    let tap = jump_height(0.05);
    let hold = jump_height(1.0);
    assert!((1.3..1.6).contains(&hold), "held {hold:.2} m");
    assert!(tap < 0.8 && tap > 0.35, "tapped {tap:.2} m");
}

/// Sprinting off the ground, then the stick turned 90° in the air.
#[test]
fn the_stick_turns_a_jump_in_the_air_without_losing_its_speed() {
    let mut app = app();
    floor(&mut app);
    let c = character(&mut app, DVec3::new(0.0, 0.9, 0.0));
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 1.5);
    intent(&mut app, c, |i| i.jump = true);
    app.update();
    intent(&mut app, c, |i| i.direction = Vec3::Z);
    run(&mut app, 0.5);
    let m = mover(&app, c);
    assert!(!m.grounded);
    let v = m.velocity.with_y(0.0);
    let turned = v.normalize().dot(Vec3::X).acos().to_degrees();
    assert!(turned > 40.0, "turned {turned:.0}° in 0.5 s");
    assert!(v.length() > 8.0, "kept {:.1} m/s", v.length());
}

#[test]
fn the_stick_held_back_brakes_a_jump() {
    let mut app = app();
    floor(&mut app);
    let c = character(&mut app, DVec3::new(0.0, 0.9, 0.0));
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run(&mut app, 1.5);
    intent(&mut app, c, |i| i.jump = true);
    app.update();
    intent(&mut app, c, |i| i.direction = -Vec3::X);
    run(&mut app, 0.4);
    let speed = mover(&app, c).velocity.with_y(0.0).length();
    assert!(speed < 5.0, "still {speed:.1} m/s");
    // And it doesn't build back up past where it braked to, held forward again.
    intent(&mut app, c, |i| i.direction = Vec3::X);
    run(&mut app, 0.2);
    let m = mover(&app, c);
    if !m.grounded {
        assert!(m.velocity.with_y(0.0).length() < 5.3);
    }
}

#[test]
fn a_standing_jump_can_be_steered_to_a_jog() {
    let mut app = app();
    floor(&mut app);
    let c = character(&mut app, DVec3::new(0.0, 0.9, 0.0));
    run(&mut app, 0.3);
    intent(&mut app, c, |i| {
        i.jump = true;
        i.direction = Vec3::X;
    });
    run(&mut app, 0.45);
    let m = mover(&app, c);
    assert!(!m.grounded);
    let s = m.velocity.with_y(0.0).length();
    assert!((4.0..=5.1).contains(&s), "{s:.1} m/s");
}

// ---- climbing (Assassin's Creed Unity's explicit intent; Dying Light 2's runtime ledges)

/// Band tops on the facade, m.
const BANDS: [f64; 4] = [1.75, 3.5, 5.25, 7.0];
/// The facade's roof, m.
const ROOF: f64 = 8.75;

/// A building whose face is at x = 1 (facing −X), 8.75 m tall, with a stone band 0.3 m deep
/// and 0.25 m tall standing out of the face every 1.75 m, from z = `z0` to `z1`.
fn facade(app: &mut App, z0: f64, z1: f64) {
    block(
        app,
        DVec3::new(3.0, ROOF / 2.0, 0.0),
        Vec3::new(4.0, ROOF as f32, 12.0),
    );
    bands(app, z0, z1);
}

fn bands(app: &mut App, z0: f64, z1: f64) {
    for top in BANDS {
        block(
            app,
            DVec3::new(0.85, top - 0.125, (z0 + z1) / 2.0),
            Vec3::new(0.3, 0.25, (z1 - z0) as f32),
        );
    }
}

/// Jumps at the facade from the street and catches the band at 3.5 m.
fn hang_on_facade(app: &mut App) -> Entity {
    let c = character(app, DVec3::new(-0.6, 0.9, 0.0));
    run(app, 0.2);
    intent(app, c, |i| {
        i.direction = Vec3::X;
        i.jump = true;
    });
    run_until(app, 1.5, |w| {
        w.get::<Mover>(c).unwrap().mode == LocomotionMode::Climb
    });
    intent(app, c, |i| {
        i.direction = Vec3::ZERO;
        i.jump = false;
    });
    run(app, 0.5);
    let m = mover(app, c);
    let h = m.hang.expect("hanging from a band");
    assert!((h.top.y - 3.5).abs() < 0.05, "{h:?}");
    // The hold is the band's front edge: the body hangs clear of it.
    assert!((h.top.x - 0.7).abs() < 0.03, "{h:?}");
    assert!(pos(app, c).x < 0.7 - 0.3, "{:?}", pos(app, c));
    c
}

fn tap(app: &mut App, e: Entity, f: impl Fn(&mut MoveIntent, bool)) {
    intent(app, e, |i| f(i, true));
    app.update();
    intent(app, e, |i| f(i, false));
}

#[test]
fn holding_toward_a_facade_climbs_it_band_by_band_and_onto_the_roof() {
    let mut app = app();
    floor(&mut app);
    facade(&mut app, -6.0, 6.0);
    let c = hang_on_facade(&mut app);
    intent(&mut app, c, |i| i.direction = Vec3::X);
    let mut tops = vec![3.5];
    run_until(&mut app, 6.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        if let Some(h) = m.hang.filter(|h| !h.transit) {
            let y = (h.top.y * 4.0).round() / 4.0;
            if tops.last() != Some(&y) {
                tops.push(y);
            }
        }
        m.grounded && w.get::<Transform>(c).unwrap().translation.y > ROOF
    });
    // Band to band, then the roof's edge (the last band at the hips), then over.
    assert_eq!(tops, vec![3.5, 5.25, 7.0, ROOF], "hung from");
    assert_eq!(triggered(&app, c, "ledge_hop"), 3);
    let p = pos(&app, c);
    assert!(
        (p.y - (ROOF + 0.9)).abs() < 0.05 && p.x > 1.0,
        "on the roof: {p:?}"
    );
}

#[test]
fn jump_hops_up_and_crouch_drops_down_the_bands() {
    let mut app = app();
    floor(&mut app);
    facade(&mut app, -6.0, 6.0);
    let c = hang_on_facade(&mut app);
    let top = |app: &App| mover(app, c).hang.map(|h| h.top.y);
    // Jump with the stick at rest: no room on a band, so up to the next.
    tap(&mut app, c, |i, on| i.jump = on);
    run(&mut app, 0.6);
    assert_eq!(top(&app).map(|y| (y * 4.0).round() / 4.0), Some(5.25));
    // A hop is quick: well under half a second band to band.
    assert!(!mover(&app, c).hang.unwrap().transit);
    // Crouch: down one band, and again.
    tap(&mut app, c, |i, on| i.crouch = on);
    let secs = run_until(&mut app, 1.0, |w| {
        w.get::<Mover>(c).unwrap().hang.is_some_and(|h| !h.transit)
    });
    assert!(secs < 0.5, "dropped in {secs:.2} s");
    assert_eq!(top(&app).map(|y| (y * 4.0).round() / 4.0), Some(3.5));
    assert_eq!(triggered(&app, c, "ledge_drop"), 1);
    tap(&mut app, c, |i, on| i.crouch = on);
    run(&mut app, 0.6);
    // The band at 1.75 m is too low to hang from over the street (the feet would be on it):
    // it lets go and lands instead.
    let m = mover(&app, c);
    assert!(
        m.hang.is_none_or(|h| (h.top.y - 1.75).abs() < 0.05),
        "{:?}",
        m.hang
    );
}

#[test]
fn jump_along_the_wall_leaps_across_a_gap_to_the_next_ledge() {
    let mut app = app();
    floor(&mut app);
    // The bands break for 1.4 m at z = 0.6 .. 2.0.
    block(
        &mut app,
        DVec3::new(3.0, ROOF / 2.0, 0.0),
        Vec3::new(4.0, ROOF as f32, 12.0),
    );
    bands(&mut app, -6.0, 0.6);
    bands(&mut app, 2.0, 6.0);
    let c = hang_on_facade(&mut app);
    // Shimmy to the end of the band (+Z is to the right, facing +X).
    intent(&mut app, c, |i| i.direction = Vec3::Z);
    run(&mut app, 1.0);
    let before = pos(&app, c);
    assert!(before.z < 0.7, "stopped at the end: {before:?}");
    intent(&mut app, c, |i| i.jump = true);
    app.update();
    intent(&mut app, c, |i| i.jump = false);
    run(&mut app, 0.8);
    let m = mover(&app, c);
    let h = m.hang.expect("caught the band across the gap");
    assert!(h.top.z > 2.0 && (h.top.y - 3.5).abs() < 0.05, "{h:?}");
    assert_eq!(triggered(&app, c, "ledge_leap"), 1);
}

#[test]
fn walking_off_a_roof_with_crouch_held_catches_the_edge() {
    let mut app = app();
    floor(&mut app);
    // A roof 6 m up, its edge at x = 1, facing −X.
    block(
        &mut app,
        DVec3::new(5.0, 3.0, 0.0),
        Vec3::new(8.0, 6.0, 8.0),
    );
    let c = character(&mut app, DVec3::new(3.0, 6.9, 0.0));
    run(&mut app, 0.2);
    intent(&mut app, c, |i| {
        i.direction = -Vec3::X;
        i.crouch = true;
    });
    run_until(&mut app, 3.0, |w| {
        w.get::<Mover>(c).unwrap().hang.is_some_and(|h| !h.transit)
    });
    intent(&mut app, c, |i| {
        i.direction = Vec3::ZERO;
        i.crouch = false;
    });
    run(&mut app, 0.3);
    let m = mover(&app, c);
    let h = m.hang.expect("hanging from the edge");
    assert!(
        (h.top.y - 6.0).abs() < 0.05 && (h.top.x - 1.0).abs() < 0.05,
        "{h:?}"
    );
    assert!(
        m.facing.distance(Vec3::X) < 0.1,
        "turned to face the wall: {}",
        m.facing
    );
    let p = pos(&app, c);
    assert!(p.y < 5.0 && p.x < 1.0, "{p:?}");
    assert_eq!(triggered(&app, c, "drop_hang"), 1);
    // Without crouch, walking off just falls.
    let d = character(&mut app, DVec3::new(3.0, 6.9, 3.0));
    run(&mut app, 0.2);
    intent(&mut app, d, |i| i.direction = -Vec3::X);
    run(&mut app, 2.0);
    assert!(mover(&app, d).hang.is_none());
}

#[test]
fn sprinting_into_a_wall_runs_up_it_and_catches_a_ledge() {
    let mut app = app();
    floor(&mut app);
    facade(&mut app, -6.0, 6.0);
    let c = character(&mut app, DVec3::new(-8.0, 0.9, 0.0));
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run_until(&mut app, 3.0, |w| {
        w.get::<Mover>(c).unwrap().hang.is_some_and(|h| !h.transit)
    });
    assert_eq!(triggered(&app, c, "wall_runup"), 1);
    let m = mover(&app, c);
    let h = m.hang.expect("caught a band");
    // Higher than a standing jump reaches (3.5 m): the run-up carried it to 5.25.
    assert!((h.top.y - 5.25).abs() < 0.05, "{h:?}");
}

#[test]
fn sprinting_into_a_wall_a_little_over_head_height_runs_up_onto_it() {
    let mut app = app();
    floor(&mut app);
    block(
        &mut app,
        DVec3::new(3.0, 1.3, 0.0),
        Vec3::new(4.0, 2.6, 8.0),
    );
    let c = character(&mut app, DVec3::new(-8.0, 0.9, 0.0));
    intent(&mut app, c, |i| {
        i.direction = Vec3::X;
        i.sprint = true;
    });
    run_until(&mut app, 3.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        m.grounded && w.get::<Transform>(c).unwrap().translation.y > 3.0
    });
    let p = pos(&app, c);
    assert!((p.y - 3.5).abs() < 0.05 && p.x > 1.0, "on top: {p:?}");
    assert_eq!(triggered(&app, c, "wall_runup"), 1);
}

// ---- ropes

fn rope(app: &mut App, anchor: DVec3, length: f32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(anchor),
            engine_movement::ClimbRope {
                length,
                ..Default::default()
            },
        ))
        .id()
}

/// Runs at a rope hanging at x = 6 and jumps into it.
fn catch_rope(app: &mut App) -> Entity {
    rope(app, DVec3::new(6.0, 10.0, 0.0), 8.5);
    let c = character(app, DVec3::new(0.0, 0.9, 0.0));
    intent(app, c, |i| i.direction = Vec3::X);
    run(app, 0.6);
    intent(app, c, |i| i.jump = true);
    run_until(app, 1.5, |w| w.get::<Mover>(c).unwrap().rope.is_some());
    intent(app, c, |i| {
        i.jump = false;
        i.direction = Vec3::ZERO;
    });
    assert_eq!(triggered(app, c, "rope_grab"), 1);
    c
}

#[test]
fn a_running_jump_into_a_rope_catches_it_and_swings_out() {
    let mut app = app();
    floor(&mut app);
    let c = catch_rope(&mut app);
    // The run carries on as a swing: out past the rope, then back.
    let mut furthest = 0.0f64;
    let mut back = false;
    run_until(&mut app, 3.0, |w| {
        let x = w.get::<Transform>(c).unwrap().translation.x;
        furthest = furthest.max(x);
        back |= x < furthest - 0.5;
        false
    });
    let m = mover(&app, c);
    assert_eq!(m.mode, LocomotionMode::Climb);
    assert!(m.rope.is_some());
    assert!(furthest > 7.0, "swung out to x {furthest:.2}");
    assert!(back, "and back");
}

#[test]
fn the_stick_climbs_a_rope_and_jump_lets_go_with_the_swing() {
    let mut app = app();
    floor(&mut app);
    let c = catch_rope(&mut app);
    let s0 = mover(&app, c).rope.unwrap().s;
    // Facing +X (the run): forward climbs.
    intent(&mut app, c, |i| i.direction = Vec3::X);
    run(&mut app, 2.0);
    let r = mover(&app, c).rope.unwrap();
    assert!(
        s0 - r.s > 3.6 && s0 - r.s < 5.0,
        "climbed {:.2} m",
        s0 - r.s
    );
    // Back climbs down, faster.
    intent(&mut app, c, |i| i.direction = -Vec3::X);
    run(&mut app, 1.0);
    let r2 = mover(&app, c).rope.unwrap();
    assert!(r2.s - r.s > 1.5, "down {:.2} m", r2.s - r.s);
    // Sideways, the stick only pumps the swing.
    intent(&mut app, c, |i| i.direction = Vec3::Z);
    run(&mut app, 0.3);
    let r2 = mover(&app, c).rope.unwrap();
    run(&mut app, 1.0);
    let r3 = mover(&app, c).rope.unwrap();
    assert!((r3.s - r2.s).abs() < 0.05);
    assert!(
        r3.swing.length() > 1.5,
        "pumped to {:.2} m/s",
        r3.swing.length()
    );
    // Jump: off the rope, flying on with the swing.
    intent(&mut app, c, |i| i.jump = true);
    app.update();
    let m = mover(&app, c);
    assert!(m.rope.is_none() && m.mode == LocomotionMode::Air);
    assert!(m.velocity.y > 2.0);
    assert_eq!(triggered(&app, c, "rope_release"), 1);
    // Not caught again before landing.
    intent(&mut app, c, |i| i.jump = false);
    run(&mut app, 0.6);
    assert!(mover(&app, c).rope.is_none());
}

#[test]
fn climbing_to_the_top_of_a_rope_steps_off_onto_the_roof_beside_it() {
    let mut app = app();
    floor(&mut app);
    // A roof at 9 m whose edge is 0.65 m behind the rope.
    block(
        &mut app,
        DVec3::new(10.65, 4.5, 0.0),
        Vec3::new(8.0, 9.0, 8.0),
    );
    rope(&mut app, DVec3::new(6.0, 10.2, 0.0), 8.5);
    let c = character(&mut app, DVec3::new(2.0, 0.9, 0.0));
    intent(&mut app, c, |i| i.direction = Vec3::X);
    run(&mut app, 0.3);
    intent(&mut app, c, |i| i.jump = true);
    run_until(&mut app, 1.5, |w| w.get::<Mover>(c).unwrap().rope.is_some());
    intent(&mut app, c, |i| i.jump = false);
    run_until(&mut app, 8.0, |w| {
        let m = w.get::<Mover>(c).unwrap();
        let p = w.get::<Transform>(c).unwrap().translation;
        m.grounded && p.y > 9.0
    });
    let p = pos(&app, c);
    assert!((p.y - 9.9).abs() < 0.05 && p.x > 6.65, "on the roof: {p:?}");
}

#[test]
fn a_hard_hit_knocks_a_climber_off_the_rope() {
    let mut app = app();
    floor(&mut app);
    let c = catch_rope(&mut app);
    run(&mut app, 0.3);
    app.world.get_mut::<Mover>(c).unwrap().knockback = Vec3::new(-8.0, 2.0, 0.0);
    app.update();
    let m = mover(&app, c);
    assert!(m.rope.is_none() && m.mode == LocomotionMode::Air);
}

// ---- no slow motion from fights the player isn't in

fn attack_at(distance: f64) -> u32 {
    let mut app = app();
    floor(&mut app);
    let me = character(&mut app, DVec3::new(0.0, 0.9, 0.0));
    app.world
        .entity_mut(me)
        .insert((Perceiver, Tempo::level(10)));
    let other = app
        .world
        .spawn(Transform::from_translation(DVec3::new(distance, 0.9, 0.0)))
        .id();
    app.update();
    app.world.write_message(engine_world::AttackEvent {
        attacker: other,
        target: None,
        windup_secs: 0.4,
        reach: 2.0,
        kind: Default::default(),
    });
    app.update();
    app.world.resource::<PerceptionStatus>().threats
}

#[test]
fn a_swing_at_someone_else_8_m_away_does_not_slow_the_world() {
    assert_eq!(attack_at(8.0), 0);
    // One in reach still does.
    assert_eq!(attack_at(3.0), 1);
}
