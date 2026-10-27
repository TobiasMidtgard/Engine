//! The float test (`demo::spawn_floaters`): everything dropped onto the water settles the way
//! the real thing does, and the drop replays on its cycle without piling up bodies.

use std::sync::Arc;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::PhysicsPlugin;
use engine_physics::demo::{self, FLOAT_CYCLE_SECS};
use engine_world::physics::{RigidBody, Velocity};
use engine_world::{Water, WaterQuery, WaterSample, WorldPlugin};

/// Still water with its surface at y = 0.
struct Pond;

impl WaterQuery for Pond {
    fn sample(&self, _p: DVec3) -> Option<WaterSample> {
        Some(WaterSample {
            surface_height: 0.0,
            depth: 20.0,
            velocity: Vec3::ZERO,
            normal: Vec3::Y,
        })
    }
}

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
    app.world.insert_resource(Water(Arc::new(Pond)));
    let mut commands = app.world.commands();
    demo::spawn_floaters(&mut commands, DVec3::ZERO);
    app.world.flush();
    app
}

fn named(app: &mut App, name: &str) -> Entity {
    let found: Vec<Entity> = app
        .world
        .query::<(Entity, &Name)>()
        .iter(&app.world)
        .filter(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
        .collect();
    assert_eq!(found.len(), 1, "{name}: {found:?}");
    found[0]
}

fn transform(app: &App, e: Entity) -> Transform {
    *app.world.get::<Transform>(e).unwrap()
}

/// How far the body's local `axis` points out of the horizontal, degrees (+ up).
fn elevation(app: &App, e: Entity, axis: Vec3) -> f32 {
    (transform(app, e).rotation * axis)
        .y
        .clamp(-1.0, 1.0)
        .asin()
        .to_degrees()
}

/// How far the body's local up is from world up, degrees.
fn tilt(app: &App, e: Entity) -> f32 {
    90.0 - elevation(app, e, Vec3::Y)
}

#[test]
fn floaters_settle_the_way_real_ones_do() {
    let mut app = app();
    app.update_n(30 * 60);
    let post = named(&mut app, "Float test: post");
    let raft = named(&mut app, "Float test: raft");
    let light = named(&mut app, "Float test: light crate");
    let heavy = named(&mut app, "Float test: waterlogged crate");
    let plank = named(&mut app, "Float test: plank");
    let ball = named(&mut app, "Float test: ball");
    let drum = named(&mut app, "Float test: drum");
    let steel = named(&mut app, "Float test: steel ball");

    // The post lies down, rolled onto a corner.
    let lying = elevation(&app, post, Vec3::Y);
    assert!(lying.abs() < 5.0, "post's long axis {lying:.1}° from flat");
    let roll = elevation(&app, post, Vec3::X).abs();
    assert!((roll - 45.0).abs() < 5.0, "post rolled {roll:.1}°");
    // The raft of five such posts floats flat; so do the plank and the waterlogged crate.
    for (e, what) in [
        (raft, "raft"),
        (plank, "plank"),
        (heavy, "waterlogged crate"),
    ] {
        let t = tilt(&app, e);
        assert!(t < 3.0, "{what} tilted {t:.1}°");
    }
    let showing = transform(&app, heavy).translation.y + 0.4;
    assert!(
        (showing - 0.08).abs() < 0.015,
        "waterlogged crate shows {showing:.3} m"
    );
    // The light crate floats on an edge.
    let t = tilt(&app, light);
    assert!((t - 45.0).abs() < 4.0, "light crate at {t:.1}°");
    // The ball rides high, the empty drum floats, the steel ball sinks.
    let y = |e| transform(&app, e).translation.y;
    assert!(y(ball) > 0.1, "ball's center at {:.2}", y(ball));
    assert!(
        (-0.3..0.4).contains(&y(drum)),
        "drum's center at {:.2}",
        y(drum)
    );
    assert!(y(steel) < -2.0, "steel ball at {:.2}", y(steel));
    for (e, what) in [
        (post, "post"),
        (raft, "raft"),
        (light, "light crate"),
        (heavy, "waterlogged crate"),
        (plank, "plank"),
        (ball, "ball"),
        (drum, "drum"),
    ] {
        let v = *app.world.get::<Velocity>(e).unwrap();
        assert!(
            v.linear.length() < 0.05 && v.angular.length() < 0.1,
            "{what} still moving: {v:?}"
        );
    }
}

#[test]
fn floaters_drop_again_on_their_cycle() {
    let mut app = app();
    app.update();
    let bodies = |app: &mut App| app.world.query::<&RigidBody>().iter(&app.world).count();
    let total = bodies(&mut app);
    assert_eq!(total, 8);
    let post = named(&mut app, "Float test: post");
    // Just past the cycle: the new post stands, about to topple.
    app.update_n((FLOAT_CYCLE_SECS * 60.0) as usize + 5);
    let again = named(&mut app, "Float test: post");
    assert_ne!(again, post, "the float test didn't drop again");
    assert!(
        tilt(&app, again) < 6.0,
        "the new post isn't standing: {:.1}°",
        tilt(&app, again)
    );
    assert_eq!(bodies(&mut app), total, "replays changed the body count");
}

/// With nobody near, the float test rests where it floats (floating bodies never fall asleep
/// by themselves), and it drops again when somebody comes back.
#[test]
fn floaters_rest_while_nobody_is_near() {
    let mut app = app();
    let focus = app
        .world
        .spawn((
            Transform::from_xyz(300.0, 1.7, 0.0),
            engine_world::SimulationFocus::default(),
        ))
        .id();
    app.update_n(12 * 60);
    let stats = *app.world.resource::<engine_physics::PhysicsStats>();
    assert!(stats.bodies >= 8 && stats.awake == 0, "{stats:?}");
    let ball = named(&mut app, "Float test: ball");
    app.world.get_mut::<Transform>(focus).unwrap().translation.x = 20.0;
    app.update_n(10);
    assert!(app.world.get_entity(ball).is_err(), "it didn't drop again");
    let stats = *app.world.resource::<engine_physics::PhysicsStats>();
    assert!(stats.awake >= 8, "{stats:?}");
}
