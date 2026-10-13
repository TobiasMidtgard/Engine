//! A sail on a boat's rig pushes the boat: the wind in the cloth reaches the dynamic rigid
//! body that carries it as `ApplyImpulse`, high up, so it also heels the boat. No physics
//! backend runs here, so the boat stays put and the impulses can be summed.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, SoftBodyPlugin, SoftBodySim, SoftMaterial};
use engine_world::WeatherState;
use engine_world::physics::{ApplyImpulse, RigidBody};

/// Impulses each entity received, while `on`.
#[derive(Resource, Default)]
struct Received {
    on: bool,
    by: Vec<(Entity, Vec3, Vec3)>,
}

fn record(mut received: ResMut<Received>, mut impulses: MessageReader<ApplyImpulse>) {
    for i in impulses.read() {
        if received.on {
            received.by.push((i.entity, i.impulse, i.angular));
        }
    }
}

#[test]
fn a_sail_pushes_and_heels_the_boat_that_carries_it() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<Received>().add_systems(Last, record);
    // A steady 6 m/s breeze along +X, square onto the sail.
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::new(6.0, 0.0, 0.0);
        weather.gustiness = 0.0;
    }
    let boat = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            RigidBody::dynamic().with_mass(200.0),
        ))
        .id();
    // The rig on the mast; the sail lies in the rig's ZY plane (its local X runs aft).
    let rig = app
        .world
        .spawn((Transform::from_xyz(0.0, 0.0, -1.2), ChildOf(boat)))
        .id();
    let corners = [[0.1, 3.4], [1.9, 4.7], [2.85, 1.13], [0.1, 1.12]];
    let sail = app
        .world
        .spawn((
            Transform::from_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
            Cloth::sail(corners, SoftMaterial::hex(0xf2ead8)),
            ChildOf(rig),
        ))
        .id();
    app.update_n(120);
    assert!(app.world.get::<SoftBodySim>(sail).is_some(), "sail built");
    app.world.resource_mut::<Received>().on = true;
    app.update_n(60);

    let received = app.world.resource::<Received>();
    assert!(!received.by.is_empty(), "the sail pulled on nothing");
    assert!(
        received.by.iter().all(|(e, _, _)| *e == boat),
        "every pull goes to the boat, not the rig or the sail"
    );
    let push: Vec3 = received.by.iter().map(|(_, j, _)| *j).sum();
    let heel: Vec3 = received.by.iter().map(|(_, _, a)| *a).sum();
    // One second of a 6.9 m² sail square to 6 m/s: about 0.5 ρ Cd A v² ≈ 240 N downwind,
    // plus the sail's own weight (about 19 N) down.
    assert!(push.x > 80.0 && push.x < 400.0, "push {push}");
    assert!(push.z.abs() < 0.3 * push.x, "push {push}");
    assert!(push.y < 0.0 && push.y > -60.0, "push {push}");
    // It pushes about 2.5 m above the boat's center: the boat heels to leeward (about −Z).
    assert!(heel.z < -1.5 * push.x, "heel {heel} for push {push}");
}
