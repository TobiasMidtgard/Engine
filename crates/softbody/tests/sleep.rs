//! A body at rest sleeps (isn't stepped) and wakes when something reaches it.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, ClothPins, SoftBodyPlugin, SoftBodySim};
use engine_world::physics::{Collider, ColliderShape};
use engine_world::{WeatherState, WorldImpulse};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    app
}

fn curtain(app: &mut App) -> Entity {
    app.world
        .spawn((
            Transform::from_xyz(0.0, 3.0, 0.0),
            Cloth {
                size: [1.0, 1.5],
                resolution: [12, 18],
                pins: ClothPins::TopEdge { every: 1 },
                ..Cloth::default()
            },
        ))
        .id()
}

fn asleep(app: &App, e: Entity) -> bool {
    app.world.get::<SoftBodySim>(e).unwrap().asleep()
}

#[test]
fn a_still_curtain_sleeps_and_a_blast_wakes_it() {
    let mut app = app();
    let e = curtain(&mut app);
    app.update_n(360);
    assert!(
        asleep(&app, e),
        "a curtain hanging in still air should sleep"
    );
    let before: Vec<DVec3> = {
        let b = app.world.get::<SoftBodySim>(e).unwrap();
        (0..b.particle_count())
            .map(|i| b.world_position(i))
            .collect()
    };
    app.update_n(30);
    let b = app.world.get::<SoftBodySim>(e).unwrap();
    assert!((0..b.particle_count()).all(|i| b.world_position(i) == before[i]));

    app.world.write_message(WorldImpulse {
        center: DVec3::new(0.0, 2.3, 0.6),
        radius: 2.0,
        strength: 20.0,
        energy_j: 0.0,
        heat_c: 0.0,
        source: None,
    });
    app.update_n(2);
    assert!(!asleep(&app, e), "a blast should wake it");
    let b = app.world.get::<SoftBodySim>(e).unwrap();
    let moved = (0..b.particle_count())
        .map(|i| b.world_position(i).distance(before[i]))
        .fold(0.0, f64::max);
    assert!(moved > 0.01, "the blast moved it only {moved} m");
}

#[test]
fn a_collider_moving_past_wakes_a_sleeping_curtain() {
    let mut app = app();
    let e = curtain(&mut app);
    app.update_n(360);
    assert!(asleep(&app, e));
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 2.3, 1.0),
            Collider {
                shape: ColliderShape::Sphere { radius: 0.2 },
                ..Collider::default()
            },
        ))
        .id();
    app.update_n(2);
    // Still there: nothing near it moves.
    assert!(asleep(&app, e), "a still collider shouldn't wake it");
    for k in 0..30 {
        app.world.get_mut::<Transform>(ball).unwrap().translation.z = 1.0 - 0.04 * k as f64;
        app.update();
    }
    assert!(!asleep(&app, e), "a ball pushing through should wake it");
}

#[test]
fn a_far_cloth_that_collides_with_itself_rests() {
    let mut app = app();
    let plain = curtain(&mut app);
    let folded = app
        .world
        .spawn((
            Transform::from_xyz(2.0, 3.0, 0.0),
            Cloth {
                size: [1.0, 1.5],
                resolution: [12, 18],
                pins: ClothPins::TopEdge { every: 1 },
                sim: engine_softbody::SimSettings {
                    self_collide: true,
                    ..Default::default()
                },
                ..Cloth::default()
            },
        ))
        .id();
    // Light wind, so neither sleeps; the focus 80 m off (past twice the LOD distance).
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(3.0, 0.0, 0.0);
    app.add_systems(PreUpdate, |mut focus: ResMut<engine_world::FocusPoints>| {
        focus.points.clear();
        focus.points.push(DVec3::new(80.0, 1.7, 0.0));
    });
    app.update_n(240);
    let body = |e| app.world.get::<SoftBodySim>(e).unwrap();
    assert!(body(folded).frozen, "far self-colliding cloth should rest");
    assert!(
        !body(plain).frozen,
        "far plain cloth keeps stepping (coarsely)"
    );
}
