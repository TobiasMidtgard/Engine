//! The 2026-11-05 second movement pass: landings that roll or stagger, catching a ledge on a
//! long fall, crawling under a low roof, and wall kicks across an alley.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_movement::prelude::*;
use engine_world::locomotion::MoveIntent;
use support::*;

fn character(app: &mut App, pos: DVec3) -> Entity {
    let e = app
        .world
        .spawn((Transform::from_translation(pos), MovementProfile::pilot()))
        .id();
    app.update();
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

/// Walks off a ledge `height` m up (its edge at x = 1, toward −X), crouching in the air
/// if `crouch`; returns the character once it is down.
fn drop_from(app: &mut App, height: f64, crouch: bool) -> Entity {
    block(
        app,
        DVec3::new(5.0, height / 2.0, 0.0),
        Vec3::new(8.0, height as f32, 8.0),
    );
    let c = character(app, DVec3::new(2.0, height + 0.9, 0.0));
    run(app, 0.2);
    intent(app, c, |i| i.direction = -Vec3::X);
    run_until(app, 1.0, |w| !w.get::<Mover>(c).unwrap().grounded);
    intent(app, c, |i| i.crouch = crouch);
    run_until(app, 3.0, |w| w.get::<Mover>(c).unwrap().grounded);
    c
}

#[test]
fn a_long_drop_with_crouch_rolls_and_keeps_going() {
    let mut app = app();
    floor(&mut app);
    let c = drop_from(&mut app, 6.0, true);
    let m = mover(&app, c);
    assert!(m.roll.is_some(), "rolled: {:?}", m.mode);
    assert_eq!(m.recover.0, 0.0, "no stagger out of a roll");
    let x = pos(&app, c).x;
    run(&mut app, 0.6);
    assert!(pos(&app, c).x < x - 1.0, "the roll carried on");
}

#[test]
fn the_same_drop_without_crouch_lands_hard_and_staggers() {
    let mut app = app();
    floor(&mut app);
    let c = drop_from(&mut app, 6.0, false);
    let m = mover(&app, c);
    assert!(m.roll.is_none());
    assert!(m.recover.0 > 0.3, "staggered: {:?}", m.recover);
    // A jump right away is swallowed by the stagger.
    intent(&mut app, c, |i| i.jump = true);
    run(&mut app, 0.1);
    assert!(mover(&app, c).grounded, "no jump while recovering");
}

#[test]
fn a_short_hop_down_neither_rolls_nor_staggers() {
    let mut app = app();
    floor(&mut app);
    let c = drop_from(&mut app, 1.5, true);
    let m = mover(&app, c);
    assert!(m.roll.is_none() && m.recover.0 == 0.0, "{m:?}");
}

#[test]
fn falling_past_a_band_with_grab_held_catches_it() {
    let mut app = app();
    floor(&mut app);
    // A wall 14 m tall (its face at x = 0, toward −X) with a band 0.3 m deep at 7 m.
    block(
        &mut app,
        DVec3::new(3.0, 7.0, 0.0),
        Vec3::new(6.0, 14.0, 10.0),
    );
    block(
        &mut app,
        DVec3::new(-0.15, 6.875, 0.0),
        Vec3::new(0.3, 0.25, 10.0),
    );
    // Dropped from 12 m beside it, grab (sprint) held.
    let c = character(&mut app, DVec3::new(-0.9, 12.0, 0.0));
    intent(&mut app, c, |i| i.sprint = true);
    run_until(&mut app, 2.0, |w| w.get::<Mover>(c).unwrap().hang.is_some());
    let m = mover(&app, c);
    let h = m.hang.expect("caught the band");
    assert!((h.top.y - 7.0).abs() < 0.05, "{h:?}");
    assert_eq!(triggered(&app, c, "fall_catch"), 1);
}

#[test]
fn a_low_roof_is_crawled_under_and_stood_up_from() {
    let mut app = app();
    floor(&mut app);
    // A slab 0.85 m over the ground from x = −2 to −6.
    block(
        &mut app,
        DVec3::new(-4.0, 1.35, 0.0),
        Vec3::new(4.0, 1.0, 4.0),
    );
    let c = character(&mut app, DVec3::new(0.0, 0.9, 0.0));
    intent(&mut app, c, |i| i.direction = -Vec3::X);
    run_until(&mut app, 3.0, |w| {
        w.get::<Transform>(c).unwrap().translation.x < -3.0
    });
    assert!(mover(&app, c).crawl.is_some(), "crawling under the slab");
    run_until(&mut app, 6.0, |w| {
        w.get::<Transform>(c).unwrap().translation.x < -7.5
    });
    assert!(pos(&app, c).x < -7.5, "through: {:?}", pos(&app, c));
    run(&mut app, 0.5);
    assert!(mover(&app, c).crawl.is_none(), "up again past it");
}

#[test]
fn wall_kicks_climb_an_alley_wall_to_wall() {
    let mut app = app();
    floor(&mut app);
    // Two walls 3.6 m apart (faces at x = ±1.8), 12 m tall.
    for x in [-3.8, 3.8] {
        block(
            &mut app,
            DVec3::new(x, 6.0, 0.0),
            Vec3::new(4.0, 12.0, 10.0),
        );
    }
    let c = character(&mut app, DVec3::new(-1.4, 0.9, 0.0));
    run(&mut app, 0.1);
    let mut peak = 0.0f64;
    for _ in 0..240 {
        let m = mover(&app, c);
        let dir = match m.runup {
            Some(r) if r.secs > 0.12 => {
                intent(&mut app, c, |i| {
                    i.direction = -r.normal;
                    i.jump = true;
                    i.sprint = true;
                });
                None
            }
            Some(r) => Some(-r.normal),
            None if !m.grounded => {
                // In the air: hands off (the next wall's run-up comes on its own).
                intent(&mut app, c, |i| *i = MoveIntent::default());
                None
            }
            None => Some(Vec3::X),
        };
        if let Some(d) = dir {
            intent(&mut app, c, |i| {
                i.direction = d;
                i.jump = false;
                i.sprint = true;
            });
        }
        app.update();
        peak = peak.max(pos(&app, c).y);
    }
    assert!(
        triggered(&app, c, "wall_runup") >= 2,
        "ran up both walls: {}",
        triggered(&app, c, "wall_runup")
    );
    assert!(peak > 4.5, "climbed the alley: {peak:.2}");
}
