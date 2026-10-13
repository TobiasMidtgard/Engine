//! A sail on a boat's rig pushes the boat: the wind in the cloth reaches the dynamic rigid
//! body that carries it as `ApplyImpulse`, high up, so it also heels the boat. No physics
//! backend runs here, so the boat stays put and the impulses can be summed.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, SoftBodyPlugin, SoftBodySim, SoftMaterial};
use engine_world::physics::{ApplyImpulse, RigidBody};
use engine_world::{Water, WaterQuery, WaterSample, WeatherState};

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

/// Still water 5 m deep everywhere.
struct Pond;

impl WaterQuery for Pond {
    fn sample(&self, _p: DVec3) -> Option<WaterSample> {
        Some(WaterSample {
            surface_height: 0.0,
            depth: 5.0,
            velocity: Vec3::ZERO,
            normal: Vec3::Y,
        })
    }
}

#[test]
fn sailboat_home_sends_a_dinghy_back_to_its_mooring() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.insert_resource(Water(Arc::new(Pond)));
    // Wind along +X: head to wind is a quarter turn from the heading it spawns with.
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::new(6.0, 0.0, 0.0);
        weather.gustiness = 0.0;
    }
    let home = DVec3::new(10.0, 0.0, -5.0);
    app.add_systems(Startup, move |mut commands: Commands| {
        engine_softbody::demo::spawn_sailboat(&mut commands, home, 22.0);
    });
    app.update_n(30);
    let mut boats = app
        .world
        .query_filtered::<Entity, With<engine_softbody::demo::sailboat::Helm>>();
    let boat = boats.single(&app.world).expect("one dinghy");

    // Out on its lane (afloat, not lost), it stays where it is...
    let away = home + DVec3::new(8.0, 0.0, 3.0);
    {
        let mut t = app.world.get_mut::<Transform>(boat).unwrap();
        t.translation = away;
        t.rotation = Quat::from_rotation_y(2.5);
    }
    app.update_n(30);
    let at = app.world.get::<Transform>(boat).unwrap().translation;
    assert!((at - away).length() < 1.0, "left its lane on its own: {at}");

    // ...until it is sent home: back at its mooring, and the sail goes along rather than
    // being dragged across the jump and torn.
    let mut sails = app.world.query::<(&Cloth, &SoftBodySim)>();
    let version = sails
        .single(&app.world)
        .expect("the sail")
        .1
        .topology_version;
    let reply = call_api(
        &mut app.world,
        "softbody.sailboat_home",
        serde_json::json!({}),
    )
    .unwrap();
    assert_eq!(reply["boats"], 1);
    app.update_n(10);
    let at = app.world.get::<Transform>(boat).unwrap().translation;
    assert!((at - home).length() < 0.05, "not back at its mooring: {at}");
    let (_, sail) = sails.single(&app.world).unwrap();
    assert_eq!(sail.topology_version, version, "the sail tore in the jump");
    let far = (0..sail.sim.len())
        .map(|i| (sail.world_position(i) - home).length())
        .fold(0.0, f64::max);
    assert!(
        far < 6.0,
        "sail particles left behind: {far} m from the boat"
    );
}
