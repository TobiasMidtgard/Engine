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

/// Something else streaming meanwhile (foliage around a moving camera): a leaf a frame.
#[derive(Component)]
struct Leaf;

fn fall_leaves(mut commands: Commands) {
    commands.spawn((Name::new("leaf"), Leaf));
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
    .add_systems(Update, (spawn_walkers, fall_leaves));
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
    // The leaves change with every cell out too, so they are nobody's leak.
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

/// A demo that caches its mesh handle across loads (the mistake `release_assets` exposes).
#[derive(Resource, Default)]
struct CachedMesh(Option<engine::assets::Handle<engine::render::Mesh>>);

#[test]
fn the_audit_counts_assets_left_and_meshes_missing_theirs() {
    use engine::assets::{AssetAppExt, Assets};
    use engine::render::{Mesh, MeshRef, MeshRenderer, Shape, StandardMaterial};

    fn cube() -> Mesh {
        Shape::Cuboid { size: [1.0; 3] }.mesh()
    }
    fn draw(world: &mut World, mesh: engine::assets::Handle<Mesh>) {
        world.spawn(MeshRenderer {
            mesh: MeshRef::Handle(mesh),
            ..Default::default()
        });
    }
    let mut app = App::new();
    app.add_plugins((
        engine::world::WorldPlugin,
        StreamingPlugin,
        StreamAuditPlugin {
            settle: 3,
            only: None,
        },
    ))
    .init_asset::<Mesh>()
    .init_asset::<StandardMaterial>()
    .init_resource::<CachedMesh>();
    // Makes a mesh each load and never frees it.
    app.stream_cell(StreamCell::new("hoarder", DVec3::ZERO, 10.0, |world| {
        let mesh = world.resource_mut::<Assets<Mesh>>().add(cube());
        draw(world, mesh);
    }));
    // Frees what it made.
    app.stream_cell(
        StreamCell::new("tidy", DVec3::new(30.0, 0.0, 0.0), 10.0, |world| {
            let mesh = world.resource_mut::<Assets<Mesh>>().add(cube());
            draw(world, mesh);
        })
        .release_assets(),
    );
    // Frees what it made but draws its first mesh again on the reload.
    app.stream_cell(
        StreamCell::new("cacher", DVec3::new(60.0, 0.0, 0.0), 10.0, |world| {
            let mesh = match world.resource::<CachedMesh>().0 {
                Some(mesh) => mesh,
                None => world.resource_mut::<Assets<Mesh>>().add(cube()),
            };
            world.resource_mut::<CachedMesh>().0 = Some(mesh);
            draw(world, mesh);
        })
        .release_assets(),
    );
    for _ in 0..300 {
        app.update();
        if app.world.resource::<StreamAuditReport>().done {
            break;
        }
    }
    let report = app.world.resource::<StreamAuditReport>().clone();
    assert!(report.done, "{report:?}");
    let cell = |key: &str| report.cells.iter().find(|c| c.key == key).unwrap().clone();
    let hoarder = cell("hoarder");
    assert_eq!(hoarder.left, Vec::new(), "its entities go");
    assert_eq!(
        hoarder
            .assets_left
            .iter()
            .map(|(k, n, _)| (k.as_str(), *n))
            .collect::<Vec<_>>(),
        vec![("Mesh", 1)],
        "{hoarder:?}"
    );
    assert!(!hoarder.clean());
    assert!(cell("tidy").clean(), "{:?}", cell("tidy"));
    let cacher = cell("cacher");
    assert_eq!(cacher.missing, 1, "{cacher:?}");
    assert!(!cacher.clean());
}
