//! The proving ground keeps playing: stations rebuild on their cycles without piling up
//! bodies, and the replay API restarts one now.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_physics::PhysicsPlugin;
use engine_physics::demo::{self, Station};
use engine_world::WorldPlugin;
use engine_world::physics::RigidBody;
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
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    app
}

fn named(app: &mut App, name: &str) -> Vec<Entity> {
    app.world
        .query::<(Entity, &Name)>()
        .iter(&app.world)
        .filter(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
        .collect()
}

fn bodies(app: &mut App) -> usize {
    app.world.query::<&RigidBody>().iter(&app.world).count()
}

/// How far the domino's long axis leans from upright, radians.
fn lean(app: &App, e: Entity) -> f32 {
    let up = app.world.get::<Transform>(e).unwrap().rotation * Vec3::Y;
    up.angle_between(Vec3::Y)
}

#[test]
fn stations_replay_on_their_cycles() {
    let mut app = app();
    app.update();
    let total = bodies(&mut app);
    let first = named(&mut app, "First domino")[0];
    let cycle = Station::Dominoes.cycle_secs().unwrap();
    app.update_n(((cycle - 1.0) * 60.0) as usize);
    assert!(
        lean(&app, first) > 1.0,
        "the first domino stands after {} s",
        cycle - 1.0
    );
    // Step to the replay: the first domino is a new one, standing.
    let mut again = Vec::new();
    for _ in 0..2 * 60 {
        app.update();
        again = named(&mut app, "First domino");
        if again != [first] {
            break;
        }
    }
    assert_eq!(again.len(), 1, "dominoes piled up: {again:?}");
    assert_ne!(again[0], first, "the dominoes didn't replay");
    assert!(
        lean(&app, again[0]) < 0.1,
        "the new first domino isn't standing"
    );
    assert_eq!(bodies(&mut app), total, "replays changed the body count");
}

#[test]
fn the_api_replays_a_station_now() {
    let mut app = app();
    app.update_n(60);
    let before = named(&mut app, "Ragdoll (red)");
    let total = bodies(&mut app);
    let r = call_api(
        &mut app.world,
        "physics.proving_ground.replay",
        json!({ "station": "ragdolls" }),
    )
    .unwrap();
    assert_eq!(r["stations"], json!(["ragdolls"]));
    assert_eq!(r["proving_grounds"], 1);
    app.update();
    let after = named(&mut app, "Ragdoll (red)");
    assert_eq!(after.len(), 1);
    assert_ne!(after, before, "the ragdolls weren't rebuilt");
    assert_eq!(bodies(&mut app), total);
    let err = call_api(
        &mut app.world,
        "physics.proving_ground.replay",
        json!({ "station": "trampoline" }),
    );
    assert!(
        err.as_ref().is_err_and(|e| e.contains("dominoes")),
        "{err:?}"
    );
}
