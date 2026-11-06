//! Measures the physics proving ground's step the way the showcase's physics stop sees it: the
//! whole demo built at the origin, the focus where the stop's camera stands, every station
//! cycling. Headless, no GPU needed.
//!
//! ```sh
//! cargo run -p engine_physics --release --example pg_bench -- [frames]
//! PHYSICS_REPORT=600 cargo run -p engine_physics --release --example pg_bench
//! ```
//!
//! Prints the mean, p95 and worst step time (physics's own work) and how many bodies are awake.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats, demo};
use engine_world::{SimulationFocus, WorldPlugin};

/// Frames before timing starts: the stations build and start their cycles.
const WARMUP: usize = 600;
/// The showcase's physics stop: its camera, from the proving ground's origin, meters.
const EYE: DVec3 = DVec3::new(0.0, 24.0, 44.0);

fn main() {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(900);
    engine_core::log::init("info");
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world
        .spawn((Transform::from_translation(EYE), SimulationFocus::default()));
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    for _ in 0..WARMUP {
        app.update();
    }
    eprintln!("proving ground: warmed up");
    let mut times = Vec::with_capacity(frames);
    let mut awake = 0usize;
    for _ in 0..frames {
        app.update();
        let s = app.world.resource::<PhysicsStats>();
        times.push(s.step_ms);
        awake += s.awake;
    }
    let s = *app.world.resource::<PhysicsStats>();
    let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
    times.sort_by(f64::total_cmp);
    let p95 = times[(times.len() * 95 / 100).min(times.len() - 1)];
    let worst = times.last().copied().unwrap_or(0.0);
    println!(
        "proving ground: {mean:.2} ms per step (p95 {p95:.2}, worst {worst:.2}) over {frames} steps; \
         {} bodies, {:.0} awake on average, {} regions",
        s.bodies,
        awake as f64 / frames.max(1) as f64,
        s.regions
    );
}
