//! Loading the paddock: its meshes are made off the main thread (prefetched on a worker, or
//! by `build_demo` for a streamed cell), so spawning it makes none.

use engine_anim::AnimationPlugin;
use engine_anim::demo::{self, spawn_demo_world};
use engine_anim::skin::SkinnedMesh;
use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::Mesh;
use engine_world::WorldPlugin;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app.world.init_resource::<Assets<Mesh>>();
    app
}

fn counts(world: &World) -> (usize, usize) {
    (
        world.resource::<Assets<SkinnedMesh>>().len(),
        world.resource::<Assets<Mesh>>().len(),
    )
}

#[test]
fn a_prefetched_paddock_spawns_without_making_meshes() {
    let mut app = app();
    demo::prefetch(&mut app.world);
    assert!(!demo::ready(&app.world));
    let start = std::time::Instant::now();
    while !demo::ready(&app.world) {
        assert!(
            start.elapsed().as_secs() < 300,
            "the paddock's meshes never came"
        );
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let before = counts(&app.world);
    assert!(before.0 >= 7, "only {} bodies prefetched", before.0);
    spawn_demo_world(&mut app.world, DVec3::ZERO);
    assert_eq!(counts(&app.world), before, "spawning made meshes");
}

#[test]
fn build_and_place_spawn_the_same_paddock() {
    let mut whole = app();
    let spawned = spawn_demo_world(&mut whole.world, DVec3::ZERO);
    let mut split = app();
    let parts = demo::build_demo(DVec3::new(30.0, 0.0, -40.0));
    let before = counts(&split.world);
    let placed = demo::place_demo(&mut split.world, parts);
    let made = counts(&split.world);
    assert!(made.0 > before.0, "place stored no bodies");
    // Placing again (a paddock restart, a reload) finds them all.
    demo::place_demo(&mut split.world, demo::build_demo(DVec3::ZERO));
    assert_eq!(counts(&split.world), made);
    assert_eq!(
        counts(&whole.world),
        made,
        "build_demo misses a mesh the paddock makes"
    );
    let at = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation;
    let moved = at(&split, placed.horse) - at(&whole, spawned.horse);
    assert!(
        moved.distance(DVec3::new(30.0, 0.0, -40.0)) < 1e-6,
        "{moved}"
    );
}
