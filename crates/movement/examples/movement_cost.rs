//! What the movement module costs a frame on the CPU: runs the movement demo (the duel, the
//! runners, the climbing quarter) headless without drawing and prints the profiler's rows for
//! the movement systems.
//!
//! ```text
//! ENGINE_PROFILE=1 cargo run -p engine_movement --release --example movement_cost -- [secs]
//! ```
//!
//! Frames run back to back on a manual clock (60 Hz), so the numbers are CPU time per frame;
//! the profiler writes `engine-profile.txt` every 5 s of wall time.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_movement::{MovementPlugin, demo};
use engine_render::RenderDisabled;
use engine_render::prelude::*;
use engine_world::WorldPlugin;

fn main() {
    engine_core::log::init("warn");
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40.0);
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.add_plugins(RenderPlugin)
        .add_plugins(WorldPlugin)
        .add_plugins(MovementPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world
        .insert_resource(RenderDisabled("measuring the CPU".into()));
    app.world.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(0.0, 5.0, 20.0),
    ));
    let mut queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &app.world);
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    queue.apply(&mut app.world);
    // Settle (the look bakes on the first frames).
    for _ in 0..120 {
        app.update();
    }
    let frames = (secs * 60.0) as usize;
    let start = std::time::Instant::now();
    for _ in 0..frames {
        app.update();
    }
    let wall = start.elapsed().as_secs_f64();
    println!(
        "{frames} frames in {wall:.2} s: {:.3} ms a frame (all modules in the app)",
        wall * 1000.0 / frames as f64
    );
}
