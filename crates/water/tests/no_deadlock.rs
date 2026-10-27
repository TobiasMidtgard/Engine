//! Water never holds its lock across parallel work: a stream steps wide on the compute pool,
//! and a thread waiting there runs other modules' tasks. Here those tasks read `Water` the way
//! fire's fuel sampling does, beside the stream's step and the floats', and every frame must
//! still finish (round 9 froze the showcase this way).

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::demo::{Valley, ValleyStream, spawn_valley};
use engine_water::{Buoyancy, WaterPlugin};
use engine_world::{Ground, GroundQuery, GroundSample, SurfaceId, Water, WorldPlugin};

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

/// Fire's way of reading the water: many points at once, spread over the compute pool.
fn sample_wide(water: Res<Water>) {
    let mut wet = vec![false; 4096];
    engine_core::tasks::par_chunks_mut(&mut wet, 64, |k, chunk| {
        for (i, w) in chunk.iter_mut().enumerate() {
            let n = k * 64 + i;
            let p = DVec3::new((n % 16) as f64 - 8.0, 2.0, -40.0 - (n / 16) as f64 * 0.3);
            *w = water.0.sample(p).is_some();
        }
    });
}

#[test]
fn reading_water_beside_its_parallel_steps_never_deadlocks() {
    let (done, frames) = mpsc::channel();
    std::thread::spawn(move || {
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
            for k in 0..40 {
                commands.spawn((
                    Transform::from_translation(DVec3::new(k as f64 - 20.0, 0.5, 10.0)),
                    Buoyancy::new(Vec3::splat(0.3), 500.0).with_probes(3, 3),
                ));
            }
        });
        app.add_systems(
            FixedUpdate,
            (sample_wide, sample_wide, sample_wide).in_set(SimSet::Environment),
        );
        app.add_systems(FixedUpdate, sample_wide.in_set(SimSet::Gameplay));
        for frame in 0..400 {
            app.update();
            if done.send(frame).is_err() {
                return;
            }
        }
    });
    let mut last = None;
    loop {
        match frames.recv_timeout(Duration::from_secs(60)) {
            Ok(frame) => last = Some(frame),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("the frame after {last:?} never finished: water deadlocked")
            }
        }
    }
    assert_eq!(last, Some(399));
}
