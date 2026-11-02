//! Times the physics step at the proving ground, headless (no renderer): every station
//! replays at once a second in, the busiest the yard gets, and the step's times are printed.
//!
//! ```sh
//! cargo run -p engine_physics --release --example yard_bench -- [seconds] [stations,...]
//! ```
//!
//! `stations` defaults to all of them (`galton`, `tower`, `jenga`, ...: [`Station::key`]).
//! `PHYSICS_REPORT=N` also logs the per-region report every N steps; `PHYSICS_BUDGET_MS=ms`
//! sets the step budget (`0`: none).

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_physics::demo::{self, Station};
use engine_physics::{PhysicsPlugin, PhysicsSettings, PhysicsStats};
use engine_world::{SimulationFocus, WorldPlugin};
use serde_json::json;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let only: Option<Vec<String>> = args
        .next()
        .map(|s| s.split(',').map(str::to_owned).collect());

    if std::env::var_os("PHYSICS_REPORT").is_some() {
        engine_core::log::init("info");
    }
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    if let Some(ms) = std::env::var("PHYSICS_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        app.world.resource_mut::<PhysicsSettings>().step_budget_ms = (ms > 0.0).then_some(ms);
    }
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 1.7, 30.0)),
        SimulationFocus::default(),
    ));
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    app.update_n(60);

    for s in Station::ALL {
        if only.as_ref().is_none_or(|o| o.iter().any(|k| k == s.key())) {
            let r = call_api(
                &mut app.world,
                "physics.proving_ground.replay",
                json!({ "station": s.key() }),
            );
            assert!(r.is_ok(), "{}: {r:?}", s.key());
        }
    }

    let frames = (seconds * 60.0) as usize;
    let mut steps = Vec::with_capacity(frames);
    let mut walls = Vec::with_capacity(frames);
    let mut peak = PhysicsStats::default();
    let mut slowest = (0, PhysicsStats::default());
    let start = Instant::now();
    for frame in 0..frames {
        let t = Instant::now();
        app.update();
        walls.push(t.elapsed().as_secs_f64() * 1e3);
        let st = app.world.resource::<PhysicsStats>();
        steps.push(st.step_ms);
        if st.awake > peak.awake {
            peak = *st;
        }
        if st.step_ms > slowest.1.step_ms {
            slowest = (frame, *st);
        }
    }
    let total = start.elapsed().as_secs_f64();
    let pct = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        v[((v.len() - 1) as f64 * p) as usize]
    };
    let mean = steps.iter().sum::<f64>() / steps.len() as f64;
    println!(
        "yard_bench: {frames} frames in {total:.1} s; physics step ms mean {mean:.2} median {:.2} \
         p95 {:.2} p99 {:.2} max {:.2}; frame ms p95 {:.2} max {:.2}",
        pct(&mut steps, 0.5),
        pct(&mut steps, 0.95),
        pct(&mut steps, 0.99),
        pct(&mut steps, 1.0),
        pct(&mut walls, 0.95),
        pct(&mut walls, 1.0),
    );
    let st = app.world.resource::<PhysicsStats>();
    println!("yard_bench: at the end, load level {}", st.load_level);
    println!(
        "yard_bench: busiest step: {} awake of {} bodies, {} regions, {} touching pairs, {:.2} ms",
        peak.awake, peak.bodies, peak.regions, peak.touching, peak.step_ms
    );
    let (frame, s) = slowest;
    println!(
        "yard_bench: slowest step: frame {frame}, {:.2} ms, {} awake, {} regions, {} touching \
         pairs, {} fine, {} substeps",
        s.step_ms, s.awake, s.regions, s.touching, s.fine_awake, s.substeps
    );
}
