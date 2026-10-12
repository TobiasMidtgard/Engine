//! Mass properties: designed overrides (a plane's balance point, a car's low center of mass)
//! and what the backend reports back.

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::PhysicsPlugin;
use engine_world::physics::{ApplyImpulse, Collider, MassProperties, RigidBody, Velocity};
use engine_world::{Gravity, WorldPlugin};

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
    app.world.resource_mut::<Gravity>().uniform = Vec3::ZERO;
    app
}

/// A 2 × 0.2 × 0.4 m plank (density 1000: 160 kg), each one well clear of the others.
fn plank(app: &mut App, body: RigidBody) -> Entity {
    let n = app.world.entities().count_spawned() as f64;
    app.world
        .spawn((
            Transform::from_xyz(0.0, 0.0, 10.0 * n),
            body,
            Collider::cuboid(Vec3::new(1.0, 0.1, 0.2)),
            Velocity::default(),
        ))
        .id()
}

fn mass(app: &App, e: Entity) -> MassProperties {
    *app.world.get::<MassProperties>(e).unwrap()
}

#[test]
fn a_moved_center_of_mass_keeps_the_shapes_inertia_about_it() {
    let mut app = app();
    let plain = plank(&mut app, RigidBody::dynamic());
    let com = Vec3::new(0.8, 0.0, 0.0);
    let heavy_end = plank(
        &mut app,
        RigidBody {
            center_of_mass: Some(com),
            ..RigidBody::dynamic()
        },
    );
    app.update_n(2);
    let (a, b) = (mass(&app, plain), mass(&app, heavy_end));
    assert!((a.mass_kg - 160.0).abs() < 0.5, "{a:?}");
    assert!((b.mass_kg - a.mass_kg).abs() < 1e-3, "{b:?}");
    assert!((b.center_of_mass - com).length() < 1e-4, "{b:?}");
    // Parallel axes: about a point 0.8 m along X, the Y and Z moments grow by m·0.8².
    let grow = a.mass_kg * 0.64;
    let mut expect = [a.inertia.x, a.inertia.y + grow, a.inertia.z + grow];
    let mut got = b.inertia.to_array();
    expect.sort_by(f32::total_cmp);
    got.sort_by(f32::total_cmp);
    for (g, e) in got.iter().zip(expect) {
        assert!(
            (g - e).abs() < 1e-2 * e.max(1.0),
            "inertia {got:?}, expected {expect:?}"
        );
    }
    // A push through the moved center of mass doesn't turn it; one through the middle does.
    for (at, spins) in [(com, false), (Vec3::ZERO, true)] {
        let e = plank(
            &mut app,
            RigidBody {
                center_of_mass: Some(com),
                ..RigidBody::dynamic()
            },
        );
        app.update();
        let origin = app.world.get::<Transform>(e).unwrap().translation;
        app.world.write_message(ApplyImpulse {
            entity: e,
            impulse: Vec3::new(0.0, 0.0, 160.0),
            at: Some(origin + at.as_dvec3()),
            angular: Vec3::ZERO,
        });
        app.update_n(5);
        let w = app.world.get::<Velocity>(e).unwrap().angular.length();
        assert_eq!(w > 0.05, spins, "spin {w} after a push at {at:?}");
    }
}

#[test]
fn dropping_an_override_brings_back_the_shapes_mass() {
    let mut app = app();
    let e = plank(
        &mut app,
        RigidBody::dynamic().with_mass_properties(40.0, Vec3::new(0.0, -0.05, 0.0), Vec3::ONE),
    );
    app.update_n(2);
    assert!((mass(&app, e).mass_kg - 40.0).abs() < 1e-3);
    *app.world.get_mut::<RigidBody>(e).unwrap() = RigidBody {
        mass_kg: Some(500.0),
        ..RigidBody::dynamic()
    };
    app.update_n(2);
    let m = mass(&app, e);
    assert!((m.mass_kg - 500.0).abs() < 0.5, "{m:?}");
    assert!(m.center_of_mass.length() < 1e-4, "{m:?}");
    *app.world.get_mut::<RigidBody>(e).unwrap() = RigidBody::dynamic();
    app.update_n(2);
    let m = mass(&app, e);
    assert!(
        (m.mass_kg - 160.0).abs() < 0.5,
        "back to the shape's mass: {m:?}"
    );
    // And it still moves like 160 kg.
    app.world.write_message(ApplyImpulse {
        entity: e,
        impulse: Vec3::new(160.0, 0.0, 0.0),
        at: None,
        angular: Vec3::ZERO,
    });
    app.update_n(3);
    let v = app.world.get::<Velocity>(e).unwrap().linear;
    assert!((v.x - 1.0).abs() < 0.02, "{v:?}");
}
