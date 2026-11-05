//! A species' art (meshes and coat textures, seconds the first time) is made on a worker in
//! play: the animal waits for it, then gets its parts.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_npc::*;
use engine_render::{Image, Mesh, MeshRenderer};
use engine_world::WorldPlugin;

#[test]
fn a_species_art_is_made_on_a_worker_and_its_animals_wait() {
    let mut app = App::new();
    app.world.insert_resource(Assets::<Mesh>::default());
    app.world.insert_resource(Assets::<Image>::default());
    app.add_plugins((WorldPlugin, NpcPlugin));
    app.world.resource_mut::<WildlifeSettings>().enabled = false;
    let rabbit = app
        .world
        .spawn((
            Transform::default(),
            Animal::new(Species::Rabbit),
            StandIn::default(),
            SelfPropelled,
        ))
        .id();
    let parts = |app: &mut App| {
        app.world
            .query::<(&ChildOf, &MeshRenderer)>()
            .iter(&app.world)
            .filter(|(c, _)| c.parent() == rabbit)
            .count()
    };
    let mut waited = false;
    for _ in 0..2_000 {
        app.update();
        if parts(&mut app) > 0 {
            break;
        }
        waited |= engine_core::off_frame::pending(&app.world, None) > 0;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(parts(&mut app) > 3, "the rabbit got its parts");
    assert!(waited, "its art was made off the frame");
}
