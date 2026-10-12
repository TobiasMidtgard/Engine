//! The physics debug view: nothing until asked, each layer adds its lines, far things are left
//! out, and the `physics.debug` API switches layers.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_debug::DebugDraw;
use engine_physics::{PhysicsDebug, PhysicsPlugin};
use engine_world::physics::{Collider, ColliderShape, Joint, JointKind, RigidBody, Velocity};
use engine_world::{FocusPoints, WorldPlugin};
use serde_json::json;

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
    // The draw buffer alone (no renderer): enough to count what gets drawn.
    app.init_resource::<DebugDraw>();
    app.world.resource_mut::<FocusPoints>().points = vec![DVec3::ZERO];
    app
}

/// A floor, a small stack, a ball and a pendulum pinned to the world.
fn scene(app: &mut App) {
    let w = &mut app.world;
    w.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)),
    ));
    for i in 0..3 {
        w.spawn((
            Transform::from_xyz(0.0, 0.25 + 0.5 * i as f64, 0.0),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.25)),
            Velocity::default(),
        ));
    }
    w.spawn((
        Transform::from_xyz(2.0, 0.3, 0.0),
        RigidBody::dynamic(),
        Collider::sphere(0.3),
        Velocity {
            linear: Vec3::X,
            ..Default::default()
        },
    ));
    let bob = w
        .spawn((
            Transform::from_xyz(-3.0, 3.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.2),
            Velocity::default(),
        ))
        .id();
    w.spawn(Joint {
        kind: JointKind::Spherical { cone_limit: None },
        body_a: bob,
        body_b: None,
        anchor_a: Vec3::new(0.0, 1.0, 0.0),
        anchor_b: DVec3::new(-3.0, 4.0, 0.0),
        break_force: None,
        collide_connected: false,
    });
}

/// Lines drawn in one frame with `settings`.
fn lines(app: &mut App, settings: PhysicsDebug) -> usize {
    *app.world.resource_mut::<PhysicsDebug>() = settings;
    app.world.resource_mut::<DebugDraw>().clear();
    app.update();
    app.world.resource::<DebugDraw>().line_count()
}

#[test]
fn each_layer_draws_only_when_asked() {
    let mut app = app();
    scene(&mut app);
    app.update_n(30);
    assert_eq!(
        lines(&mut app, PhysicsDebug::default()),
        0,
        "drew while off"
    );
    let colliders = lines(
        &mut app,
        PhysicsDebug {
            colliders: true,
            ..Default::default()
        },
    );
    assert!(colliders > 50, "only {colliders} collider lines");
    for (name, layer) in [
        (
            "contacts",
            PhysicsDebug {
                contacts: true,
                ..Default::default()
            },
        ),
        (
            "joints",
            PhysicsDebug {
                joints: true,
                ..Default::default()
            },
        ),
        (
            "regions",
            PhysicsDebug {
                regions: true,
                ..Default::default()
            },
        ),
        (
            "centers of mass",
            PhysicsDebug {
                centers_of_mass: true,
                ..Default::default()
            },
        ),
        (
            "velocities",
            PhysicsDebug {
                velocities: true,
                ..Default::default()
            },
        ),
    ] {
        let n = lines(&mut app, layer);
        assert!(n > 0, "the {name} layer drew nothing");
    }
}

#[test]
fn contacts_on_terrain_are_drawn() {
    // A ball on terrain touches several triangles; Rapier merges their contacts for the solver.
    let mut app = app();
    let (rows, cols) = (21, 21);
    app.world.spawn((
        Transform::default(),
        RigidBody::fixed(),
        Collider {
            shape: ColliderShape::Heightfield {
                heights: vec![0.0; rows * cols],
                rows: rows as u32,
                cols: cols as u32,
                size: [20.0, 20.0],
            },
            ..Default::default()
        },
    ));
    app.world.spawn((
        Transform::from_xyz(0.0, 0.3, 0.0),
        RigidBody::dynamic(),
        Collider::sphere(0.3),
        Velocity::default(),
    ));
    app.update_n(30);
    let n = lines(
        &mut app,
        PhysicsDebug {
            contacts: true,
            ..Default::default()
        },
    );
    assert!(n > 0, "no contact drawn for a ball resting on terrain");
}

#[test]
fn far_bodies_are_left_out() {
    let mut app = app();
    scene(&mut app);
    app.update_n(5);
    let only_colliders = PhysicsDebug {
        colliders: true,
        ..Default::default()
    };
    let near = lines(&mut app, only_colliders.clone());
    app.world.spawn((
        Transform::from_xyz(5_000.0, 0.0, 0.0),
        RigidBody::dynamic(),
        Collider::cuboid(Vec3::splat(0.5)),
        Velocity::default(),
    ));
    app.update_n(2);
    assert_eq!(
        lines(&mut app, only_colliders.clone()),
        near,
        "drew a crate 5 km away"
    );
    let everywhere = lines(
        &mut app,
        PhysicsDebug {
            radius_m: 10_000.0,
            ..only_colliders
        },
    );
    assert!(everywhere > near, "a big radius should include it");
}

#[test]
fn the_api_switches_layers() {
    let mut app = app();
    let on = call_api(&mut app.world, "physics.debug", json!({ "all": true })).unwrap();
    assert_eq!(on["colliders"], true);
    assert_eq!(on["velocities"], true);
    let some = call_api(
        &mut app.world,
        "physics.debug",
        json!({ "all": false, "joints": true, "radius_m": 40.0 }),
    )
    .unwrap();
    assert_eq!(some["colliders"], false);
    assert_eq!(some["joints"], true);
    assert_eq!(app.world.resource::<PhysicsDebug>().radius_m, 40.0);
    let err = call_api(
        &mut app.world,
        "physics.debug",
        json!({ "wireframes": true }),
    );
    assert!(
        err.as_ref().is_err_and(|e| e.contains("colliders")),
        "{err:?}"
    );
}
