//! Frame cost of the destruction demos, without drawing: the fortress, its storehouse and the
//! voxel hill run their loops on a ground about as costly to sample as the showcase's, and the
//! profiler's table (`engine-profile.txt`, every 5 s) is printed as it comes.
//!
//! `cargo run --release -p engine_destruction --example bench -- [seconds]` (default 75).

use std::sync::Arc;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::{DestructionPlugin, demo, hill};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer};
use engine_world::{Ground, GroundQuery, GroundSample, SurfaceId, WorldPlugin};

/// Rolling ground from six octaves of noise, its normal from four more samples: about what a
/// sample of the showcase's landscape costs.
struct Rolling;

fn height(x: f64, z: f64) -> f64 {
    engine_core::math::noise::fbm2(x * 0.01, z * 0.01, 7, 6, 2.0, 0.5) as f64 * 0.6
}

impl GroundQuery for Rolling {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let e = 0.5;
        let dx = height(p.x + e, p.z) - height(p.x - e, p.z);
        let dz = height(p.x, p.z + e) - height(p.x, p.z - e);
        Some(GroundSample {
            height: height(p.x, p.z),
            normal: Vec3::new(-dx as f32, (2.0 * e) as f32, -dz as f32).normalize(),
            surface: SurfaceId(3),
        })
    }
}

fn setup(mut commands: Commands) {
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    hill::spawn_voxel_hill(&mut commands, DVec3::new(-120.0, 0.0, 130.0));
}

fn main() {
    let secs: f32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(75.0);
    // SAFETY: set before any thread starts.
    unsafe { std::env::set_var("ENGINE_PROFILE", "1") };
    engine_core::log::init("warn,engine_destruction=info");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    let step = 1.0 / 30.0;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: step });
    app.add_plugins((RenderPlugin, WorldPlugin, DestructionPlugin))
        .insert_resource(Ground(Arc::new(Rolling)))
        .add_systems(Startup, setup);
    app.update();
    if app.world.remove_resource::<Renderer>().is_some() {
        app.world.insert_resource(RenderDisabled("bench".into()));
    }
    let mut seen = None;
    let mut t = 0.0;
    while t < secs {
        app.update();
        t += step as f32;
        let m = std::fs::metadata("engine-profile.txt")
            .and_then(|m| m.modified())
            .ok();
        if m.is_some() && m != seen {
            seen = m;
            let report = std::fs::read_to_string("engine-profile.txt").unwrap_or_default();
            println!("--- at {t:.1} s of simulation");
            for line in report.lines().take(3).chain(
                report
                    .lines()
                    .filter(|l| l.contains("destruction") || l.contains("in engine_")),
            ) {
                println!("{line}");
            }
        }
    }
}
