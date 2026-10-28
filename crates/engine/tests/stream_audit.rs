//! The streaming audit tells a clean cell from one that leaves things behind on unload.

use engine::debug::{StreamAuditPlugin, StreamAuditReport};
use engine::stream::{StreamCell, StreamCellsExt, StreamingPlugin};
use engine_core::prelude::*;

/// Marks the leaky demo's walkers, spawned by its own system after the cell's spawn.
#[derive(Component)]
struct Walker;

#[derive(Resource, Default)]
struct LeakyDemo {
    wants_walker: bool,
}

fn spawn_walkers(mut demo: ResMut<LeakyDemo>, mut commands: Commands) {
    if std::mem::take(&mut demo.wants_walker) {
        commands.spawn((Name::new("walker"), Walker));
    }
}

#[test]
fn the_audit_finds_what_an_unload_leaves() {
    let mut app = App::new();
    app.add_plugins((
        engine::world::WorldPlugin,
        StreamingPlugin,
        StreamAuditPlugin {
            settle: 3,
            only: None,
        },
    ))
    .init_resource::<LeakyDemo>()
    .add_systems(Update, spawn_walkers);
    app.stream_cell(StreamCell::new("tidy", DVec3::ZERO, 10.0, |world| {
        world
            .spawn((Name::new("hut"), Transform::default()))
            .with_children(|c| {
                c.spawn(Name::new("door"));
            });
    }));
    // Its spawn asks a system for a walker, which the cell doesn't own.
    app.stream_cell(StreamCell::new(
        "leaky",
        DVec3::new(30.0, 0.0, 0.0),
        10.0,
        |world| {
            world.spawn(Name::new("stall"));
            world.resource_mut::<LeakyDemo>().wants_walker = true;
        },
    ));
    for _ in 0..200 {
        app.update();
        if app.world.resource::<StreamAuditReport>().done {
            break;
        }
    }
    let report = app.world.resource::<StreamAuditReport>().clone();
    assert!(report.done, "{report:?}");
    let cell = |key: &str| report.cells.iter().find(|c| c.key == key).unwrap();
    assert!(cell("tidy").clean(), "{:?}", cell("tidy"));
    assert_eq!(cell("tidy").loaded, 2);
    let leaky = cell("leaky");
    assert!(!leaky.clean());
    assert_eq!(
        leaky.left.iter().map(|(_, n)| n).sum::<i64>(),
        1,
        "{leaky:?}"
    );
    assert_eq!(
        leaky.reload.iter().map(|(_, n)| n).sum::<i64>(),
        1,
        "{leaky:?}"
    );
}
