//! Times loading the fighting arena: the longest main-thread frame, the main thread's total
//! and the time until all its art is in (its fighters, sparring posts and stands), with the art built off the frame (the default) or
//! in the spawning command as before (`inline`). Set `ARENA_LOAD_FRAMES=1` to list the
//! frames over 8 ms.
//!
//! ```sh
//! cargo run -p engine_npc --release --example arena_load
//! cargo run -p engine_npc --release --example arena_load -- inline
//! ```
//!
//! No GPU needed: it counts the meshes the arena spawns without drawing them.

use std::time::Instant;

use engine_core::off_frame::{OffFrameJobs, OffFrameSettings};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::{Image, Mesh, MeshRenderer, StandardMaterial};
use engine_world::WorldPlugin;

fn main() {
    engine_core::log::init("warn");
    let inline = std::env::args().any(|a| a == "inline");
    let list = std::env::var_os("ARENA_LOAD_FRAMES").is_some();
    let mut app = App::new();
    engine_assets::AssetAppExt::init_asset::<Mesh>(&mut app);
    engine_assets::AssetAppExt::init_asset::<StandardMaterial>(&mut app);
    engine_assets::AssetAppExt::init_asset::<Image>(&mut app);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<OffFrameSettings>().inline = inline;
    app.add_plugins((WorldPlugin, NpcPlugin));

    let start = Instant::now();
    let mut frames = Vec::new();
    {
        let t = Instant::now();
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
        app.world.flush();
        frames.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    // Until no job is left and the mesh count has held for 30 frames (stand-ins and posts
    // are dressed over a few frames once their kits are in).
    let count = |app: &mut App| app.world.query::<&MeshRenderer>().iter(&app.world).count();
    let (mut quiet, mut last) = (0, 0);
    while quiet < 30 {
        let t = Instant::now();
        app.update();
        frames.push(t.elapsed().as_secs_f64() * 1000.0);
        let now = count(&mut app);
        if app.world.resource::<OffFrameJobs>().is_empty() && now == last {
            quiet += 1;
        } else {
            quiet = 0;
        }
        last = now;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let all_in = start.elapsed().as_secs_f64();
    // With ENGINE_PROFILE=1, run on until the profiler has written its report.
    while engine_core::profile::enabled() && start.elapsed().as_secs_f64() < 6.0 {
        app.update();
    }
    let meshes = app.world.query::<&MeshRenderer>().iter(&app.world).count();
    let worst = frames.iter().copied().fold(0.0, f64::max);
    let total: f64 = frames.iter().sum();
    let over_33 = frames.iter().filter(|&&ms| ms > 33.0).count();
    if list {
        for (i, ms) in frames.iter().enumerate().filter(|(_, ms)| **ms > 8.0) {
            println!("frame {i}: {ms:.1} ms");
        }
    }
    println!(
        "{}: {meshes} meshes in {all_in:.2} s over {} frames; main thread {total:.0} ms in all, \
         longest frame {worst:.1} ms, {over_33} frames over 33 ms",
        if inline { "inline" } else { "off-frame" },
        frames.len(),
    );
}
