//! Runs the water test area (without drawing) and checks that every test case in it does what
//! it shows: the stream runs and fills the pond, blocks float by density, the motorboat laps,
//! and splashes reach other modules as `WaterSplash` messages.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::WaterPlugin;
use engine_water::demo::{self, LAKE_DEPTH, LAKE_LEVEL};
use engine_world::{Water, WaterSplash, WorldPlugin};

const ORIGIN: DVec3 = DVec3::new(170.0, 0.0, -140.0);

#[derive(Resource, Default)]
struct Splashes(usize);

fn run(seconds: f32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.init_resource::<Splashes>();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, ORIGIN);
    });
    app.add_systems(
        Update,
        |mut splashes: MessageReader<WaterSplash>, mut count: ResMut<Splashes>| {
            count.0 += splashes.read().count();
        },
    );
    app.update_n((seconds * 30.0) as usize);
    app
}

fn position(app: &mut App, name: &str) -> DVec3 {
    let mut q = app.world.query::<(&Name, &Transform)>();
    q.iter(&app.world)
        .find(|(n, _)| n.as_str() == name)
        .map(|(_, t)| t.translation)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn the_test_area_does_what_it_shows() {
    let mut app = run(12.0);
    let level = ORIGIN.y + LAKE_LEVEL as f64;

    // Density blocks, dropped at 1 s, have settled: cork rides high, pine lower, ice almost
    // awash, concrete on the bottom.
    let cork = position(&mut app, "Cork block").y - level;
    let pine = position(&mut app, "Pine block").y - level;
    let ice = position(&mut app, "Ice block").y - level;
    let concrete = position(&mut app, "Concrete block").y - level;
    assert!(
        cork > pine && pine > ice,
        "cork {cork:.2} pine {pine:.2} ice {ice:.2}"
    );
    assert!(cork > 0.05, "cork {cork:.2}");
    // A 0.5 m ice cube floats with 92% under: its top just clears the water.
    let ice_top = ice + 0.25;
    assert!(ice_top > -0.12 && ice_top < 0.15, "ice top {ice_top:.2}");
    assert!(
        concrete < -1.0 && concrete > -(LAKE_DEPTH as f64) - 0.5,
        "concrete {concrete:.2}"
    );

    // The stream runs and the pond is full.
    let water = app.world.resource::<Water>().clone();
    let volume = app
        .world
        .resource::<Diagnostics>()
        .get("water.flow_volume_m3")
        .unwrap_or(0.0);
    assert!(volume > 20.0, "flow volume {volume:.1} m³");
    let pond = ORIGIN + DVec3::new(46.7, 0.0, 46.7);
    let depth = water.0.sample(pond + DVec3::Y).map(|w| w.depth);
    assert!(depth.is_some_and(|d| d > 0.3), "pond depth {depth:?}");
    // Below the falls the stream is moving.
    let below_falls = ORIGIN + DVec3::new(30.4, 0.0, 30.4);
    let stream = water.0.sample(below_falls + DVec3::Y * 3.0);
    assert!(
        stream.is_some_and(|w| w.velocity.length() > 0.3),
        "stream below the falls {stream:?}"
    );

    // The motorboat has left its start and is still on the water.
    let boat = position(&mut app, "Motorboat");
    let start = ORIGIN + DVec3::new(7.0, LAKE_LEVEL as f64, -3.0);
    assert!(
        (boat - start).length() > 10.0,
        "motorboat at {:?}",
        boat - ORIGIN
    );
    assert!(
        (boat.y - level).abs() < 1.0,
        "motorboat height {}",
        boat.y - level
    );

    // Gunfire, the depth charge and the dropped blocks all splashed.
    let splashes = app.world.resource::<Splashes>().0;
    assert!(splashes >= 10, "{splashes} splashes");
}
