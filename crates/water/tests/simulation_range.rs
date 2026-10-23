//! Water costs what the player is near: beyond `SimulationRange` of every focus, floating
//! things stay where they are and streams hold still; when a focus comes near they carry on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::demo::{Valley, ValleyStream, spawn_valley};
use engine_water::{Buoyancy, SharedWater, WaterPlugin};
use engine_world::{Ground, GroundQuery, GroundSample, SimulationFocus, SurfaceId, WorldPlugin};

/// From a spring to the south, over a 5 m cliff at z = -80, into the lake.
const STREAM: [[f64; 2]; 3] = [[0.0, -120.0], [0.0, -80.0], [0.0, -36.0]];

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A round lake 6 m deep with its shore near r = 38 m, land rising to the south over a cliff,
/// and a channel cut along x = 0.
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
        Some(GroundSample {
            height: height(p.x, p.z),
            normal: Vec3::Y,
            surface: SurfaceId(3),
        })
    }
}

#[derive(Component)]
struct Crate;

/// The valley with 60 floating crates over the lake, and a focus at `focus`.
fn valley(focus: DVec3) -> (App, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(Ground(Arc::new(Land)));
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
                jetty_bearing: None,
            },
        );
        for k in 0..60 {
            let a = k as f64 * 2.4;
            let r = 4.0 + (k as f64).sqrt() * 3.5;
            commands.spawn((
                Crate,
                Transform::from_translation(DVec3::new(r * a.cos(), 0.3, r * a.sin())),
                Buoyancy::new(Vec3::splat(0.3), 500.0).with_probes(3, 3),
            ));
        }
    });
    let eye = app
        .world
        .spawn((
            Transform::from_translation(focus),
            SimulationFocus::default(),
        ))
        .id();
    // Let the crates drop in and the stream run before anything is measured.
    app.update_n(30);
    (app, eye)
}

fn crates(app: &mut App) -> Vec<DVec3> {
    let mut q = app.world.query_filtered::<&Transform, With<Crate>>();
    q.iter(&app.world).map(|t| t.translation).collect()
}

fn stream_version(app: &App) -> u64 {
    let water = app.world.resource::<SharedWater>().0.read();
    water.flows.iter().map(|f| f.version).sum()
}

fn time_frames(app: &mut App, n: usize) -> Duration {
    let start = Instant::now();
    app.update_n(n);
    start.elapsed() / n as u32
}

#[test]
fn far_water_rests_and_wakes_when_a_focus_comes_near() {
    // Near: everything runs.
    let (mut app, eye) = valley(DVec3::new(0.0, 5.0, 0.0));
    let near = time_frames(&mut app, 60);
    let before = crates(&mut app);
    let version = stream_version(&app);
    app.update_n(10);
    let moved = crates(&mut app)
        .iter()
        .zip(&before)
        .filter(|(a, b)| a.distance(**b) > 1e-6)
        .count();
    assert!(moved > 30, "near crates bob: {moved} of 60 moved");
    assert!(stream_version(&app) > version, "the near stream runs");

    // Far beyond the range (150 m + 25%): crates hold still and the stream rests.
    app.world.get_mut::<Transform>(eye).unwrap().translation = DVec3::new(2000.0, 5.0, 0.0);
    app.update_n(2);
    let far = time_frames(&mut app, 60);
    let before = crates(&mut app);
    let version = stream_version(&app);
    app.update_n(10);
    assert_eq!(crates(&mut app), before, "far crates rest where they are");
    assert_eq!(stream_version(&app), version, "the far stream rests");

    // Back near: they carry on.
    app.world.get_mut::<Transform>(eye).unwrap().translation = DVec3::new(0.0, 5.0, 0.0);
    app.update_n(2);
    let before = crates(&mut app);
    app.update_n(10);
    assert_ne!(crates(&mut app), before, "crates wake");
    assert!(stream_version(&app) > version, "the stream wakes");
    println!(
        "ms per frame: water near {:.3}, far {:.3}",
        near.as_secs_f64() * 1e3,
        far.as_secs_f64() * 1e3
    );
}
