//! A new stream settles on a worker: the frame thread never waits for it, `WaterLoading` says
//! when it is ready, and it comes out the same as one made in place (headless runs do that).

use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::demo::{Valley, ValleyStream, spawn_valley};
use engine_water::{SharedWater, WaterLoading, WaterPlugin};
use engine_world::{Ground, GroundQuery, GroundSample, SurfaceId, WorldPlugin};

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

/// The valley's stream, made in place (`headless`) or on a worker.
fn valley(headless: bool) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = headless;
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
    });
    app
}

fn stream_volume(app: &App) -> f64 {
    let water = app.world.resource::<SharedWater>().0.read();
    water.flows.iter().map(|f| f.volume()).sum()
}

#[test]
fn a_new_stream_settles_off_the_frame_thread() {
    // Made in place: the first frame does all of it.
    let mut app = valley(true);
    let start = Instant::now();
    app.update();
    let in_place = start.elapsed();
    assert!(app.world.resource::<WaterLoading>().is_ready());
    let settled = stream_volume(&app);
    assert!(settled > 1.0, "the stream runs: {settled} m³");

    // On a worker: frames go on meanwhile, the loading screen sees it settling, and it
    // comes out with the same water.
    let mut app = valley(false);
    let mut worst = Duration::ZERO;
    let mut frames = 0;
    let waited = Instant::now();
    loop {
        let t = Instant::now();
        app.update();
        worst = worst.max(t.elapsed());
        frames += 1;
        if frames > 1 && app.world.resource::<WaterLoading>().is_ready() {
            break;
        }
        if frames == 2 {
            assert_eq!(app.world.resource::<WaterLoading>().settling, 1);
        }
        assert!(waited.elapsed() < Duration::from_secs(120), "never ready");
        std::thread::sleep(Duration::from_millis(1));
    }
    let volume = stream_volume(&app);
    assert!(
        (volume - settled).abs() < settled * 0.05,
        "{volume} m³ against {settled} m³ made in place"
    );
    println!(
        "made in place: {:.0} ms in one frame; on a worker: worst frame {:.1} ms over {frames} frames",
        in_place.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3
    );
    assert!(worst < in_place / 3, "{worst:?} against {in_place:?}");
}
