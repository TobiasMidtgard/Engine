//! The ripple grid over a lake on the ground isn't rebuilt every time the terrain installs a
//! new ground provider (it does for every edit, so digging at the siege fortress rebuilt it
//! every fixed step, 28-36 ms, round 24 on the PC), and a lake bed that really changed is
//! still read in.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::sim::SimBook;
use engine_water::state::SharedWater;
use engine_water::{WaterBed, WaterBody, WaterPlugin};
use engine_world::{
    Ground, GroundQuery, GroundSample, SimulationFocus, SurfaceId, TerrainChanged, WorldPlugin,
};

/// A bowl 4 m deep in the middle (`floor` m lower with `deeper`), flat 1 m above the water
/// beyond 50 m.
struct Bowl {
    deeper: f64,
}

impl Bowl {
    fn height(&self, x: f64, z: f64) -> f64 {
        let r = (x * x + z * z).sqrt();
        (-4.0 - self.deeper + r * r * 5.0 / 2500.0).min(1.0)
    }
}

impl GroundQuery for Bowl {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        Some(GroundSample {
            height: self.height(p.x, p.z),
            normal: Vec3::Y,
            surface: SurfaceId(3),
        })
    }
}

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(Ground(Arc::new(Bowl { deeper: 0.0 })));
    app.world.spawn((
        Transform::from_translation(DVec3::new(3.0, 1.7, 2.0)),
        SimulationFocus::default(),
    ));
    app.world.spawn((
        Name::new("Lake"),
        Transform::default(),
        WaterBody::lake(45.0).with_bed(WaterBed::Ground),
    ));
    app
}

fn rebuilds(app: &App) -> u64 {
    app.world.resource::<SimBook>().rebuilds
}

fn layout(app: &App) -> u64 {
    app.world.resource::<SimBook>().layout
}

/// Ripple grid depth under a point (from the grid's own sample).
fn depth(app: &App, x: f64, z: f64) -> f32 {
    let water = app.world.resource::<SharedWater>().0.read();
    let r = &water.ripples;
    let (i, j) = (
        ((x - r.origin.x) / r.cell as f64) as usize,
        ((z - r.origin.y) / r.cell as f64) as usize,
    );
    r.depth[j * r.n + i]
}

#[test]
fn digging_far_off_leaves_the_ripple_grid_alone() {
    let mut app = app();
    app.update_n(3 * 60);
    assert!(
        (depth(&app, 3.0, 2.0) - 4.0).abs() < 0.1,
        "lake depth read in: {}",
        depth(&app, 3.0, 2.0)
    );
    let (before, layout_before) = (rebuilds(&app), layout(&app));
    // A pick at work 200 m off: a new provider with each edit, three times a second.
    for k in 0..12 {
        app.insert_resource(Ground(Arc::new(Bowl { deeper: 0.0 })));
        app.world.write_message(TerrainChanged {
            center: DVec3::new(200.0 + k as f64, 1.0, 0.0),
            radius: 1.3,
        });
        app.update_n(20);
    }
    // And a planet coming near (a new provider with no edit to say where).
    for _ in 0..4 {
        app.insert_resource(Ground(Arc::new(Bowl { deeper: 0.0 })));
        app.update_n(30);
    }
    assert_eq!(layout(&app), layout_before, "the lake's bed read the same");
    // At most the rebuild every 2 s that runs anyway over the 6 s.
    let more = rebuilds(&app) - before;
    assert!(more <= 3, "{more} rebuilds while digging far off");
}

#[test]
fn a_lake_bed_that_really_changed_is_read_in() {
    let mut app = app();
    app.update_n(3 * 60);
    let before = depth(&app, 3.0, 2.0);
    // The bed lowered by a metre under a new provider, with no edit to say where.
    app.insert_resource(Ground(Arc::new(Bowl { deeper: 1.0 })));
    app.update_n(60);
    let after = depth(&app, 3.0, 2.0);
    assert!(
        (after - before - 1.0).abs() < 0.1,
        "deeper by a metre: {before} -> {after}"
    );
}

#[test]
fn walking_along_a_lake_rebuilds_the_grid_where_it_moves() {
    let mut app = app();
    app.update_n(3 * 60);
    let focus = app
        .world
        .query_filtered::<Entity, With<SimulationFocus>>()
        .single(&app.world)
        .expect("one focus");
    let (before, mut moves, mut origin) = (
        rebuilds(&app),
        0,
        app.world.resource::<SharedWater>().0.read().ripples.origin,
    );
    // Across the lake at a run, 6 m/s for 15 s: the grid follows every few meters.
    for k in 0..15 * 60 {
        let x = -40.0 + k as f64 * 0.1;
        app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::new(x, 1.7, 2.0);
        app.update();
        let now = app.world.resource::<SharedWater>().0.read().ripples.origin;
        if now != origin {
            moves += 1;
            origin = now;
        }
    }
    let more = rebuilds(&app) - before;
    println!("{moves} moves, {more} rebuilds");
    assert!(moves >= 5, "the grid followed: {moves} moves");
    // Each move rebuilds, and again once its reads are in; not at every step of the reads.
    assert!(more <= 2 * moves + 2, "{more} rebuilds for {moves} moves");
}
