//! The ripple grid reads the terrain under the lakes once: its periodic refresh of wave
//! speeds and currents (every 2 s, for the streams) reads none, and a small move of the grid
//! reads only the cells it newly covers. Reading all of them every 2 s cost 1.8 s a time
//! under the showcase's lake.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::WaterPlugin;
use engine_water::demo::{Valley, ValleyStream, spawn_valley};
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

/// The valley's ground, counting how often it is read.
struct Land(Arc<AtomicUsize>);

impl GroundQuery for Land {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Some(GroundSample {
            height: height(p.x, p.z),
            normal: Vec3::Y,
            surface: SurfaceId(3),
        })
    }
}

#[test]
fn the_ripple_grid_reads_the_lake_bed_once() {
    let reads = Arc::new(AtomicUsize::new(0));
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(Ground(Arc::new(Land(reads.clone()))));
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
    let eye = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(0.0, 2.0, -30.0)),
            SimulationFocus::default(),
        ))
        .id();
    app.update_n(30);
    let cells = 256 * 256;

    // Still focus: several 2 s refreshes, no terrain reads (the floats, puddles and shores
    // read a few hundred a second; a refresh of the grid would read tens of thousands).
    let before = reads.load(Ordering::Relaxed);
    app.update_n(30 * 7);
    let still = reads.load(Ordering::Relaxed) - before;
    assert!(
        still < cells / 4,
        "{still} terrain reads in 7 s with the focus still"
    );

    // A step of 20 m: the grid moves by a part of itself and reads only the new cells.
    app.world.get_mut::<Transform>(eye).unwrap().translation = DVec3::new(20.0, 2.0, -30.0);
    let before = reads.load(Ordering::Relaxed);
    app.update_n(3);
    let moved = reads.load(Ordering::Relaxed) - before;
    assert!(
        moved < cells * 3 / 4,
        "{moved} terrain reads for a 20 m move"
    );

    // A jump across the lake (a teleport, a tour stop): the new grid is read a few thousand
    // cells a step, not all in one frame, and is whole a second later.
    app.world.get_mut::<Transform>(eye).unwrap().translation = DVec3::new(-20.0, 2.0, 35.0);
    let before = reads.load(Ordering::Relaxed);
    app.update();
    let jump = reads.load(Ordering::Relaxed) - before;
    assert!(
        jump < cells / 4,
        "{jump} terrain reads in the frame after a jump"
    );
    let before = reads.load(Ordering::Relaxed);
    app.update_n(30);
    let after = reads.load(Ordering::Relaxed) - before;
    assert!(
        jump + after > cells / 2,
        "only {} of the new grid read a second after the jump",
        jump + after
    );
    println!(
        "terrain reads: {still} in 7 s still, {moved} for a 20 m move, {jump} the frame after \
         a jump ({cells} cells)"
    );
}
