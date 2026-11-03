//! Times loading a vehicle zone's art: the longest main-thread frame, the main thread's total
//! and the time until it is all in, with the art built off the frame (the default) or in the
//! spawning commands as before (`inline`). Also times each model kit's build alone.
//!
//! ```sh
//! cargo run -p engine_vehicles --release --example zone_load -- track
//! cargo run -p engine_vehicles --release --example zone_load -- airfield inline
//! cargo run -p engine_vehicles --release --example zone_load -- kits
//! ```
//!
//! No GPU needed: it counts the meshes the zone spawns without drawing them.

use std::time::Instant;

use engine_core::math::DVec3;
use engine_core::off_frame::{OffFrameJobs, OffFrameSettings};
use engine_core::prelude::*;
use engine_render::{Image, Mesh, MeshRenderer, StandardMaterial};
use engine_vehicles::models;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let inline = args.iter().any(|a| a == "inline");
    let zone = args
        .iter()
        .find(|a| *a != "inline")
        .cloned()
        .unwrap_or_else(|| "track".into());
    if zone == "kits" {
        let time = |name: &str, f: &dyn Fn()| {
            let t = Instant::now();
            f();
            println!("{name}: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        };
        time("sports car kit", &|| drop(models::cars::sports_car_kit()));
        time("rally car kit", &|| drop(models::cars::rally_car_kit()));
        time("sport plane", &|| {
            drop(models::aircraft::build_sport_plane())
        });
        time("spaceplane", &|| drop(models::aircraft::build_spaceplane()));
        return;
    }
    engine_core::log::init("warn");
    let mut app = App::new();
    app.add_plugins(engine_world::WorldPlugin);
    engine_assets::AssetAppExt::init_asset::<Mesh>(&mut app);
    engine_assets::AssetAppExt::init_asset::<StandardMaterial>(&mut app);
    engine_assets::AssetAppExt::init_asset::<Image>(&mut app);
    app.world.resource_mut::<OffFrameSettings>().inline = inline;

    let start = Instant::now();
    let mut frames = Vec::new();
    {
        let t = Instant::now();
        let mut commands = app.world.commands();
        match zone.as_str() {
            "track" => engine_vehicles::demo::spawn_track(&mut commands, DVec3::ZERO),
            "airfield" => engine_vehicles::demo::spawn_airfield(
                &mut commands,
                DVec3::new(-600.0, 0.0, 0.0),
                DVec3::new(600.0, 0.0, 0.0),
            ),
            "spaceport" => engine_vehicles::demo::spawn_spaceport(
                &mut commands,
                DVec3::new(0.0, 0.4, 0.0),
                30.0,
            ),
            other => panic!("no zone {other}: track, airfield, spaceport or kits"),
        }
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
    if std::env::var("ZONE_LOAD_FRAMES").is_ok() {
        for (i, ms) in frames.iter().enumerate().filter(|(_, ms)| **ms > 16.0) {
            println!("frame {i}: {ms:.1} ms");
        }
    }
    println!(
        "{zone} {}: {meshes} meshes in {all_in:.2} s over {} frames; main thread {total:.0} ms \
         in all, longest frame {worst:.1} ms, {over_33} frames over 33 ms",
        if inline { "inline" } else { "off-frame" },
        frames.len(),
    );
}
