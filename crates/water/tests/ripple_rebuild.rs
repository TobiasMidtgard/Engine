//! The ripple grid reads the terrain under the lakes once: its periodic refresh of wave
//! speeds and currents (every 2 s, for the streams) reads none, and a small move of the grid
//! reads only the cells it newly covers. Reading all of them every 2 s cost 1.8 s a time
//! under the showcase's lake. Since the lakes keep their beds in a grid read once
//! (`BedCache`), moving or jumping the grid reads the terrain not at all.

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

/// The ripple grid's middle (x/z) and whether it has water under it.
fn grid(app: &App) -> (DVec2, bool) {
    let w = app.world.resource::<engine_water::SharedWater>().0.clone();
    let w = w.read();
    let r = &w.ripples;
    (r.origin + DVec2::splat(r.size() * 0.5), r.has_water)
}

/// The valley with its lake and stream, a focus at `at`, and the terrain read counter.
fn valley_app(at: DVec3) -> (App, Entity, Arc<AtomicUsize>) {
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
        .spawn((Transform::from_translation(at), SimulationFocus::default()))
        .id();
    (app, eye, reads)
}

#[test]
fn the_ripple_grid_reads_the_lake_bed_once() {
    let (mut app, eye, reads) = valley_app(DVec3::new(0.0, 2.0, -30.0));
    app.update_n(30);
    let cells = 256 * 256;
    let first = reads.load(Ordering::Relaxed);

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
    // The grid is there, over the lake (its bed comes from the lake's grid, not the terrain).
    let (middle, wet) = grid(&app);
    assert!(
        middle.distance(DVec2::new(-20.0, 35.0)) < 8.0 && wet,
        "the grid at {middle} (water {wet}) a second after the jump"
    );
    assert!(
        after < cells / 4,
        "{after} terrain reads in the second after"
    );
    println!(
        "terrain reads: {first} at load, {still} in 7 s still, {moved} for a 20 m move, {jump} the frame after \
         a jump ({cells} cells)"
    );
}

/// Flying fast high over the lake (the free-flight route's 2 km legs), the ripple grid holds
/// still instead of moving and re-reading the lake bed every frame; back down at the lake, it
/// follows the focus again.
#[test]
fn the_ripple_grid_rests_while_the_focus_flies_high() {
    let (mut app, eye, reads) = valley_app(DVec3::new(0.0, 2.0, -30.0));
    app.update_n(60);
    let cells = 256 * 256;

    // 2 s at 2 km, crossing the lake back and forth at 120 m/s.
    let before = reads.load(Ordering::Relaxed);
    for i in 0..60 {
        let x = -120.0 + (i % 30) as f64 * 8.0;
        app.world.get_mut::<Transform>(eye).unwrap().translation = DVec3::new(x, 2000.0, 0.0);
        app.update();
    }
    let high = reads.load(Ordering::Relaxed) - before;
    // The shores and puddles around a moving focus read a few hundred a frame; a grid that
    // followed the flight read about 300 000 in these 2 s.
    assert!(
        high < cells / 2,
        "{high} terrain reads in 2 s of flight at 2 km ({cells} cells in the grid)"
    );

    // Down on the far shore: the grid comes there and reads the bed under it.
    app.world.get_mut::<Transform>(eye).unwrap().translation = DVec3::new(-20.0, 2.0, 35.0);
    let (high_middle, _) = grid(&app);
    assert!(
        high_middle.distance(DVec2::new(0.0, -30.0)) < 8.0,
        "the grid followed the flight to {high_middle}"
    );
    let before = reads.load(Ordering::Relaxed);
    app.update_n(30);
    let low = reads.load(Ordering::Relaxed) - before;
    let (middle, wet) = grid(&app);
    assert!(
        middle.distance(DVec2::new(-20.0, 35.0)) < 8.0 && wet,
        "the grid at {middle} (water {wet}) a second after landing by the lake"
    );
    println!("terrain reads: {high} in 2 s at 2 km, {low} in 1 s after landing");
}
