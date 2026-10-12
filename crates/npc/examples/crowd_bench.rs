//! How long one simulation step takes with many NPCs and animals.
//! `cargo run --release -p engine_npc --example crowd_bench -- [arenas] [meadows] [steps]`

use std::time::Instant;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::WorldPlugin;

fn main() {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<usize>().unwrap_or(1));
    let arenas = args.next().unwrap_or(4);
    let meadows = args.next().unwrap_or(4);
    let frames = args.next().unwrap_or(600);
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    {
        let mut commands = app.world.commands();
        for i in 0..arenas {
            demo::spawn_demo(&mut commands, DVec3::new(i as f64 * 120.0, 0.0, 0.0));
        }
        for i in 0..meadows {
            demo::spawn_wildlife(&mut commands, DVec3::new(i as f64 * 200.0, 0.0, 400.0), 60.0);
        }
    }
    app.world.flush();
    let npcs = app.world.query::<&Npc>().iter(&app.world).count();
    let animals = app.world.query::<&Animal>().iter(&app.world).count();
    // Warm up (nav grid, first sightings).
    app.update_n(120);
    let start = Instant::now();
    app.update_n(frames);
    let ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    println!("{npcs} NPCs, {animals} animals: {ms:.3} ms per step");
}
