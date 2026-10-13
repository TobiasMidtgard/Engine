//! A landscape lake filled by `spawn_valley`: the lake holds water over its basin, the stream
//! runs from its spring over a cliff into the lake and throws spray where it falls, and the
//! boats moored at the jetty float and stay tied up.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::WaterPlugin;
use engine_water::demo::{Valley, ValleyStream, spawn_valley};
use engine_world::{Ground, GroundQuery, GroundSample, SurfaceId, Water, WaterSplash, WorldPlugin};

/// The stream's course: from a spring on a plateau to the south, over a 5 m cliff at z = -80,
/// into the lake.
const STREAM: [[f64; 2]; 3] = [[0.0, -120.0], [0.0, -80.0], [0.0, -36.0]];

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A round lake (still surface at y = 0, 6 m deep, shore near r = 38 m) with land rising to
/// the south, a cliff, and a 5 m wide channel cut along x = 0.
fn height(x: f64, z: f64) -> f64 {
    let r = (x * x + z * z).sqrt();
    let mut h = if r < 40.0 {
        -6.0 + 6.6 * (r / 40.0).powi(2)
    } else {
        0.6 + (r - 40.0) * 0.05
    };
    if z < -40.0 {
        h = h.max(0.6 + (-40.0 - z) * 0.02) + 5.0 * smoothstep(-79.0, -81.0, z);
    }
    if z < -30.0 {
        h -= 1.2 * (1.0 - smoothstep(2.5, 5.0, x.abs()));
    }
    h
}

struct Land;

impl GroundQuery for Land {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let e = 0.25;
        let (hx, hz) = (
            height(p.x + e, p.z) - height(p.x - e, p.z),
            height(p.x, p.z + e) - height(p.x, p.z - e),
        );
        Some(GroundSample {
            height: height(p.x, p.z),
            normal: Vec3::new(-hx as f32, 2.0 * e as f32, -hz as f32).normalize(),
            surface: SurfaceId(3),
        })
    }
}

#[derive(Resource, Default)]
struct Spray(f32);

fn run(seconds: f32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(Ground(Arc::new(Land)));
    app.init_resource::<Spray>();
    app.add_systems(Startup, |mut commands: Commands| {
        let streams = [ValleyStream {
            path: &STREAM,
            width_m: 5.0,
            flow_m3_s: 0.4,
        }];
        spawn_valley(
            &mut commands,
            &Valley {
                lake_center: DVec3::ZERO,
                lake_radius: 60.0,
                streams: &streams,
                ground: &height,
                jetty_bearing: Some(std::f64::consts::FRAC_PI_2),
            },
        );
    });
    app.add_systems(
        Update,
        |mut splashes: MessageReader<WaterSplash>, mut spray: ResMut<Spray>| {
            spray.0 += splashes.read().map(|s| s.mass_kg).sum::<f32>();
        },
    );
    app.update_n((seconds * 30.0) as usize);
    app
}

fn find(app: &mut App, name: &str) -> Vec<DVec3> {
    let mut q = app.world.query::<(&Name, &Transform)>();
    q.iter(&app.world)
        .filter(|(n, _)| n.as_str() == name)
        .map(|(_, t)| t.translation)
        .collect()
}

#[test]
fn the_valley_fills_flows_and_moors() {
    let mut app = run(10.0);
    let water = app.world.resource::<Water>().clone();

    let lake = water.0.sample(DVec3::new(0.0, 0.0, 0.0)).expect("lake");
    assert!(
        lake.depth > 5.0 && lake.surface_height.abs() < 0.3,
        "{lake:?}"
    );
    assert!(
        water.0.sample(DVec3::new(0.0, 0.0, 50.0)).is_none(),
        "water on the bank"
    );

    // On the plateau the stream runs toward the lake (+z) in its channel.
    let above = water
        .0
        .sample(DVec3::new(0.0, 10.0, -100.0))
        .expect("stream above the cliff");
    assert!(above.depth > 0.03, "{above:?}");
    assert!(above.velocity.z > 0.1, "{above:?}");
    assert!(
        water.0.sample(DVec3::new(15.0, 10.0, -100.0)).is_none(),
        "the stream left its channel"
    );
    // Below the cliff it carries on into the lake.
    let below = water
        .0
        .sample(DVec3::new(0.0, 5.0, -70.0))
        .expect("stream below the cliff");
    assert!(below.velocity.z > 0.1, "{below:?}");

    // The fall throws spray (as `WaterSplash` puffs for particles and audio).
    let spray = app.world.resource::<Spray>().0;
    assert!(spray > 1.0, "only {spray} kg of spray");

    // The jetty stands on the north bank; the boats float beside it and stay tied up.
    let jetty = find(&mut app, "Jetty");
    assert_eq!(jetty.len(), 1);
    assert!(
        jetty[0].z > 30.0 && jetty[0].z < 45.0,
        "jetty at {:?}",
        jetty[0]
    );
    for name in ["Rowboat", "Motorboat"] {
        let boat = find(&mut app, name)[0];
        let w = water
            .0
            .sample(boat)
            .unwrap_or_else(|| panic!("{name} at {boat:?} is ashore"));
        assert!(
            (boat.y - w.surface_height).abs() < 0.6,
            "{name} at {boat:?}"
        );
        assert!(
            boat.z > 15.0 && boat.x.abs() < 10.0,
            "{name} drifted to {boat:?}"
        );
    }
    let buoys = find(&mut app, "Buoy");
    assert_eq!(buoys.len(), 3);
    for b in buoys {
        assert!(b.y.abs() < 0.8, "buoy at {b:?}");
    }
}
