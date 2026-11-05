//! Times the frames as someone arrives at the proving ground on the wall clock, headless:
//! the yard is spawned with mesh, material and image stores present (as in the game, so its
//! stations are built over several frames and their art on workers), and every frame's wall
//! time is kept until every station stands and its bodies are in the physics.
//!
//! ```sh
//! cargo run -p engine_physics --release --example yard_arrival -- [seconds]
//! ```
//!
//! `YARD_ARRIVAL_FOCUS_M=<m>` puts the one watching that far from the yard's middle (default
//! 200: the village, out of reach, so the stations are built resting). `YARD_ARRIVAL_MANUAL=1` steps a manual clock of 1/60 s instead (as tours and tests do).
//! `YARD_ARRIVAL_HITCH_MS=<ms>` makes the frame the yard arrives on that long (a zone
//! loading), so the next frame catches up several physics steps, as in play.
//! `YARD_ARRIVAL_FRAMES=<file>` writes each frame's wall time (ms), one a line.
//! Prints the slowest frames, the frames over 33 ms, and when the last station was built.

use engine_assets::Assets;
use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats, demo};
use engine_render::{Image, Mesh, StandardMaterial};
use engine_world::{SimulationFocus, WorldPlugin};
use std::time::Instant;

fn main() {
    let seconds: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let manual = std::env::var_os("YARD_ARRIVAL_MANUAL").is_some_and(|v| v != "0");
    if std::env::var_os("PHYSICS_REPORT").is_some() {
        engine_core::log::init("info");
    }
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world.insert_resource(Assets::<Mesh>::default());
    app.world
        .insert_resource(Assets::<StandardMaterial>::default());
    app.world.insert_resource(Assets::<Image>::default());
    if manual {
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
    }
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let away: f64 = std::env::var("YARD_ARRIVAL_FOCUS_M")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200.0);
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 1.7, away)),
        SimulationFocus::default(),
    ));
    app.update();
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    let hitch: u64 = std::env::var("YARD_ARRIVAL_HITCH_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    std::thread::sleep(std::time::Duration::from_millis(hitch));

    let start = Instant::now();
    let mut frames: Vec<(usize, f64, f64, usize)> = Vec::new();
    let mut built_at = None;
    while start.elapsed().as_secs_f64() < seconds {
        let t = Instant::now();
        app.update();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let s = *app.world.resource::<PhysicsStats>();
        frames.push((frames.len(), ms, s.step_ms, s.bodies));
        if built_at.is_none() && !demo::building(&mut app.world) {
            built_at = Some((frames.len(), start.elapsed().as_secs_f64()));
        }
    }
    if let Some(path) = std::env::var_os("YARD_ARRIVAL_FRAMES") {
        let lines: Vec<String> = frames.iter().map(|f| format!("{:.3}", f.1)).collect();
        std::fs::write(path, lines.join("\n") + "\n").expect("write the frame times");
    }
    let mut by_ms = frames.clone();
    by_ms.sort_by(|a, b| b.1.total_cmp(&a.1));
    let over = frames.iter().filter(|f| f.1 > 33.0).count();
    let over_ms: f64 = frames.iter().filter(|f| f.1 > 33.0).map(|f| f.1).sum();
    println!(
        "yard_arrival ({}): {} frames in {seconds} s; {over} over 33 ms ({over_ms:.0} ms in all); all built {}",
        if manual { "manual clock" } else { "wall clock" },
        frames.len(),
        built_at.map_or("never".into(), |(f, s)| format!("at frame {f}, {s:.2} s")),
    );
    for (f, ms, step, bodies) in by_ms.iter().take(8) {
        println!("  frame {f}: {ms:.1} ms (physics step {step:.1} ms, {bodies} bodies)");
    }
}
