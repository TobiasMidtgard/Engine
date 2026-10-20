//! Contacts follow small bodies: a ball sliding off a thin pin falls away at once instead of
//! riding on the pin's old contact plane, and a ball dropped between two pins drops through.
//! The pins and balls are frictionless, so a ball slides without turning (a turning ball gets
//! fresh contacts anyway): the case where reused contacts show.

use engine_core::math::{DVec3, Quat};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::PhysicsPlugin;
use engine_world::physics::{Collider, ColliderShape, RigidBody};
use engine_world::{SurfaceId, SurfaceMaterial, SurfaceMaterials, WorldPlugin};

/// Ball and pin radii, meters: a Galton board's steel ball and brass pin.
const BALL: f32 = 0.007;
const PIN: f32 = 0.004;
const PIN_Y: f64 = 1.0;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app
}

/// A frictionless, dead surface.
fn ice(app: &mut App) -> SurfaceId {
    let mut table = app.world.resource_mut::<SurfaceMaterials>();
    match table.id("test_ice") {
        Some(id) => id,
        None => table.add(SurfaceMaterial {
            name: "test_ice".into(),
            friction: 0.0,
            restitution: 0.0,
            ..Default::default()
        }),
    }
}

/// A fixed horizontal pin along Z at (`x`, [`PIN_Y`]).
fn pin(app: &mut App, x: f64) {
    let surface = ice(app);
    app.world.spawn((
        Transform {
            translation: DVec3::new(x, PIN_Y, 0.0),
            rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ..Default::default()
        },
        RigidBody::fixed(),
        Collider {
            shape: ColliderShape::Cylinder {
                radius: PIN,
                half_height: 0.01,
            },
            surface,
            ..Default::default()
        },
    ));
}

/// A small ball at (`x`, `y`), at rest.
fn ball(app: &mut App, x: f64, y: f64) -> Entity {
    let surface = ice(app);
    app.world
        .spawn((
            Transform::from_translation(DVec3::new(x, y, 0.0)),
            RigidBody::dynamic(),
            Collider {
                shape: ColliderShape::Sphere { radius: BALL },
                surface,
                ..Default::default()
            },
        ))
        .id()
}

fn at(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// A ball set just off a pin's crown slides off: the frames until its center drops below the
/// pin's, and how far sideways it is then, meters.
fn slide_off() -> (Option<usize>, f64) {
    let mut app = app();
    pin(&mut app, 0.0);
    let b = ball(&mut app, 0.0015, PIN_Y + (BALL + PIN) as f64);
    let frames = (1..=60).find(|_| {
        app.update();
        at(&app, b).y < PIN_Y
    });
    (frames, at(&app, b).x)
}

#[test]
fn a_ball_sliding_off_a_pin_falls_away_at_once() {
    let (frames, x) = slide_off();
    let frames = frames.expect("the ball never fell off the pin");
    // Fresh contacts every step: 7 frames, 14 mm out (it leaves the pin 11 mm out and falls
    // on). Reusing the old contact plane for Rapier's 5 cm: 18 frames, 60 mm out, in mid-air.
    assert!(frames <= 10, "the ball took {frames} frames to fall off");
    assert!(
        x < 0.02,
        "the ball slid {:.1} mm sideways before it fell",
        x * 1000.0
    );
}

#[test]
fn a_ball_dropped_on_one_of_two_pins_drops_through_between_them() {
    let mut app = app();
    // Two pins 3 ball widths apart, as on the Galton board.
    let half = 0.5 * (3.0 * 2.0 * BALL + 2.0 * PIN) as f64;
    pin(&mut app, -half);
    pin(&mut app, half);
    let b = ball(&mut app, -half + 0.002, PIN_Y + 0.03);
    app.update_n(90);
    let p = at(&app, b);
    // Reusing contacts for Rapier's 5 cm, it stayed wedged 8 mm above the pins.
    assert!(
        p.y < PIN_Y - 0.1,
        "the ball still hangs {:.1} mm above the pins, {:.1} mm off the middle",
        (p.y - PIN_Y) * 1000.0,
        p.x * 1000.0
    );
}
