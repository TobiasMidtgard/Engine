//! Times loading the Spell court's art: the longest main-thread frame, the main thread's total
//! and the time until every prop is in, with the art built off the frame (the default) or in
//! the spawning command as before (`inline`).
//!
//! ```sh
//! cargo run -p engine_vfx --release --example court_load
//! cargo run -p engine_vfx --release --example court_load -- inline
//! ```
//!
//! No GPU needed: it counts the meshes the court spawns without drawing them.

use std::time::Instant;

use engine_core::math::DVec3;
use engine_core::off_frame::{OffFrameJobs, OffFrameSettings};
use engine_core::prelude::*;
use engine_render::{Image, Mesh, MeshRenderer, StandardMaterial};

fn main() {
    let inline = std::env::args().any(|a| a == "inline");
    let mut app = App::new();
    engine_assets::AssetAppExt::init_asset::<Mesh>(&mut app);
    engine_assets::AssetAppExt::init_asset::<StandardMaterial>(&mut app);
    engine_assets::AssetAppExt::init_asset::<Image>(&mut app);
    app.world.resource_mut::<OffFrameSettings>().inline = inline;

    let start = Instant::now();
    let mut frames = Vec::new();
    {
        let t = Instant::now();
        let mut commands = app.world.commands();
        engine_vfx::demo::spawn_demo(&mut commands, DVec3::ZERO);
        app.world.flush();
        frames.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    loop {
        let t = Instant::now();
        app.update();
        frames.push(t.elapsed().as_secs_f64() * 1000.0);
        if app.world.resource::<OffFrameJobs>().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let all_in = start.elapsed().as_secs_f64();
    let meshes = app.world.query::<&MeshRenderer>().iter(&app.world).count();
    let worst = frames.iter().copied().fold(0.0, f64::max);
    let total: f64 = frames.iter().sum();
    let over_33 = frames.iter().filter(|&&ms| ms > 33.0).count();
    println!(
        "{}: {meshes} meshes in {all_in:.2} s over {} frames; main thread {total:.0} ms in all, \
         longest frame {worst:.1} ms, {over_33} frames over 33 ms",
        if inline { "inline" } else { "off-frame" },
        frames.len(),
    );
}
