//! CPU cost of the animation paddock without a renderer: spawns the paddock demo, puts the
//! simulation focus `distance` meters from it, runs `secs` of 60 Hz frames and prints the
//! average frame time. Add `--profile` for the per-system table in `engine-profile.txt`.
//!
//! ```sh
//! cargo run -p engine_anim --release --example paddock_bench -- 20 30 --profile   # near
//! cargo run -p engine_anim --release --example paddock_bench -- 400 30 --profile  # far
//! ```

use engine_anim::AnimationPlugin;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::{SimulationFocus, WorldPlugin};

fn main() {
    let mut args = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .map(|a| a.parse::<f64>().expect("numbers: distance secs"));
    let distance = args.next().unwrap_or(20.0);
    let secs = args.next().unwrap_or(30.0);
    engine_core::log::init("info");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let focus = Transform::from_xyz(0.0, 1.7, distance);
    app.world.spawn((
        focus,
        GlobalTransform::from(focus),
        SimulationFocus::default(),
    ));
    engine_anim::demo::spawn_demo_world(&mut app.world, DVec3::ZERO);
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    // Settle (first poses, models worn) before timing.
    for _ in 0..60 {
        app.update();
    }
    let frames = (secs * 60.0) as usize;
    let mut worst = 0.0f64;
    let start = std::time::Instant::now();
    for _ in 0..frames {
        let t = std::time::Instant::now();
        app.update();
        worst = worst.max(t.elapsed().as_secs_f64());
    }
    let avg = start.elapsed().as_secs_f64() / frames as f64;
    println!(
        "paddock, focus {distance} m away: {:.3} ms per frame on average, worst {:.3} ms ({frames} frames)",
        avg * 1e3,
        worst * 1e3
    );
}
