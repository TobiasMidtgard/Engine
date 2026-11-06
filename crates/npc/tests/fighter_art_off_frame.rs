//! A fighter's look (its team's paint, its face: 10-26 ms a texture set, up to 15 ms a mesh)
//! is made on a worker in play: the fighter waits for it, then gets its parts, and a second
//! fighter of the same look finds everything made. (The NPC systems run in the fixed step, so
//! a fighter is dressed on the first fixed step after its look is in.)

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_npc::*;
use engine_render::{Image, Mesh, MeshRenderer};
use engine_world::WorldPlugin;

fn parts(app: &mut App, of: Entity) -> usize {
    app.world
        .query::<(&ChildOf, &MeshRenderer)>()
        .iter(&app.world)
        .filter(|(c, _)| c.parent() == of)
        .count()
}

/// Update until `of` has its parts; whether an off-frame job was pending meanwhile.
fn dress(app: &mut App, of: Entity) -> bool {
    let mut waited = false;
    for _ in 0..4_000 {
        app.update();
        if parts(app, of) > 0 {
            break;
        }
        waited |= engine_core::off_frame::pending(&app.world, None) > 0;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    waited
}

#[test]
fn a_fighters_look_is_made_on_a_worker_and_the_fighter_waits() {
    // The shared kit is a job of its own: build it here so the jobs below are the look's.
    engine_npc::art::fighter::kit();
    let mut app = App::new();
    app.world.insert_resource(Assets::<Mesh>::default());
    app.world.insert_resource(Assets::<Image>::default());
    app.add_plugins((WorldPlugin, NpcPlugin));
    app.world.resource_mut::<WildlifeSettings>().enabled = false;
    let fighter = |app: &mut App| {
        app.world
            .spawn((
                Transform::default(),
                StandIn {
                    color: Some(0x3a7d5c),
                    plain: false,
                },
            ))
            .id()
    };
    let first = fighter(&mut app);
    let waited = dress(&mut app, first);
    assert!(parts(&mut app, first) > 10, "the fighter got its parts");
    assert!(waited, "its look was made off the frame");
    let meshes = app.world.resource::<Assets<Mesh>>().keys().count();
    let images = app.world.resource::<Assets<Image>>().keys().count();

    let second = fighter(&mut app);
    let waited = dress(&mut app, second);
    assert!(
        parts(&mut app, second) > 10,
        "the second fighter got its parts"
    );
    assert!(!waited, "the same look needs no worker");
    assert_eq!(
        (meshes, images),
        (
            app.world.resource::<Assets<Mesh>>().keys().count(),
            app.world.resource::<Assets<Image>>().keys().count()
        ),
        "the worker made everything the look needs"
    );
}
