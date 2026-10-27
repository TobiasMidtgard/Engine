//! The demos' art (textures, the tarn's ground, the crag, boulders, people) is made on workers
//! once a game asks for it (`WaterDemoArt::prefetch`): `WaterLoading` counts it down to ready,
//! and the water test area spawned after that takes it as it is instead of making it on the
//! frame.

use std::time::Instant;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::{Image, Mesh};
use engine_water::demo::materials::DemoMaterials;
use engine_water::{WaterLoading, WaterPlugin, demo};
use engine_world::WorldPlugin;

#[test]
fn demo_art_is_made_on_workers_before_the_demo_spawns() {
    let mut app = App::new();
    // Not headless: the area's stream settles on a worker too, as in the game.
    app.world.resource_mut::<EngineConfig>().headless = false;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(Assets::<Mesh>::default());
    app.insert_resource(Assets::<Image>::default());
    // Nothing is made until a game asks for it.
    app.update();
    assert!(app.world.resource::<WaterLoading>().is_ready());
    engine_water::demo::art::WaterDemoArt::prefetch(&mut app.world);

    // The first frame starts the art; the frames meanwhile stay short.
    let started = Instant::now();
    app.update();
    assert!(app.world.resource::<WaterLoading>().art > 0);
    let mut worst = 0.0f64;
    while !app.world.resource::<WaterLoading>().is_ready() {
        let t = Instant::now();
        app.update();
        worst = worst.max(t.elapsed().as_secs_f64());
        if t.elapsed().as_millis() > 20 {
            eprintln!(
                "slow frame {:?} art {}",
                t.elapsed(),
                app.world.resource::<WaterLoading>().art
            );
        }
        assert!(started.elapsed().as_secs() < 300, "the art never came in");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let made = started.elapsed().as_secs_f64();
    assert!(app.world.contains_resource::<DemoMaterials>());

    // Spawned now, the area takes the finished art.
    let t = Instant::now();
    demo::spawn_demo(&mut app.world.commands(), DVec3::new(170.0, 0.0, -140.0));
    app.world.flush();
    app.update();
    let spawn = t.elapsed().as_secs_f64();
    println!(
        "art made in {:.0} ms on workers (worst frame meanwhile {:.1} ms); the area spawned in {:.0} ms",
        made * 1e3,
        worst * 1e3,
        spawn * 1e3
    );
    assert!(
        worst < made / 4.0,
        "{worst} s frames while the art took {made} s"
    );
    let mut parts = app.world.query::<&Name>();
    let n = parts
        .iter(&app.world)
        .filter(|n| n.as_str() == "Part")
        .count();
    assert!(n > 50, "the area's props drew {n} parts");
}
