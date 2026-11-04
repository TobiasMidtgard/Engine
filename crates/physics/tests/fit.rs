//! Colliders fitted to models' meshes ([`engine_physics::FitCollider`]): scenery built from art
//! alone (buildings, piers, boats) is solid once its meshes load.

use engine_assets::Assets;
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{Fit, FitCollider, PhysicsPlugin};
use engine_render::{Mesh, MeshRenderer};
use engine_world::WorldPlugin;
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.insert_resource(Assets::<Mesh>::default());
    app
}

/// A pier made only of art: a 6 × 0.3 × 2 m deck mesh, its top at y = 1, on a model root whose
/// mesh loads a few frames after it is spawned. Once the mesh is in, the pier is solid as
/// drawn, and a ball dropped on it rests on the deck.
#[test]
fn a_pier_of_art_alone_gets_its_collider_once_its_mesh_loads() {
    for fit in [Fit::Mesh, Fit::Hull, Fit::Parts, Fit::Box] {
        let mut app = app();
        let pier = app
            .world
            .spawn((Transform::from_xyz(0.0, 0.85, 0.0), FitCollider::new(fit)))
            .id();
        app.world.spawn((
            Transform::default(),
            MeshRenderer::new(
                engine_render::MeshRef::Asset("pier_deck".into()),
                engine_render::MaterialRef::default(),
            ),
            ChildOf(pier),
        ));
        app.update_n(3);
        assert!(
            app.world.get::<Collider>(pier).is_none(),
            "{fit:?}: fitted before its mesh loaded"
        );
        app.world.resource_mut::<Assets<Mesh>>().insert_keyed(
            "pier_deck",
            engine_render::mesh::cuboid(Vec3::new(6.0, 0.3, 2.0)),
        );
        app.update();
        let collider = app
            .world
            .get::<Collider>(pier)
            .unwrap_or_else(|| panic!("{fit:?}: no collider"))
            .clone();
        assert!(app.world.get::<RigidBody>(pier).is_some());
        assert!(app.world.get::<FitCollider>(pier).is_none());
        let shaped = match (fit, &collider.shape) {
            (Fit::Mesh, ColliderShape::TriMesh { indices, .. }) => indices.len() >= 12,
            (Fit::Hull, ColliderShape::ConvexHull { points }) => points.len() >= 8,
            (Fit::Parts | Fit::Box, ColliderShape::Compound(parts)) => !parts.is_empty(),
            _ => false,
        };
        assert!(shaped, "{fit:?}: {:?}", collider.shape);
        let ball = app
            .world
            .spawn((
                Transform::from_xyz(1.0, 3.0, 0.0),
                RigidBody::dynamic(),
                Collider::sphere(0.25),
                Velocity::default(),
            ))
            .id();
        app.update_n(180);
        let p = app.world.get::<Transform>(ball).unwrap().translation;
        assert!(
            (p.y - 1.25).abs() < 0.05 && p.distance(DVec3::new(1.0, 1.25, 0.0)) < 0.3,
            "{fit:?}: the ball isn't resting on the deck: {p:?}"
        );
    }
}

/// A dynamic body asking for the exact mesh gets convex pieces: moving triangle meshes
/// collide poorly.
#[test]
fn a_moving_model_gets_convex_parts_not_triangles() {
    let mut app = app();
    let handle = app
        .world
        .resource_mut::<Assets<Mesh>>()
        .add(engine_render::mesh::cuboid(Vec3::new(3.0, 0.8, 1.2)));
    let boat = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 5.0, 0.0),
            RigidBody::dynamic(),
            Velocity::default(),
            FitCollider::new(Fit::Mesh),
            MeshRenderer::new(handle, engine_render::MaterialRef::default()),
        ))
        .id();
    app.update();
    let shape = &app.world.get::<Collider>(boat).expect("fitted").shape;
    assert!(
        matches!(shape, ColliderShape::Compound(parts) if parts.iter().all(|p| matches!(p.2, ColliderShape::ConvexHull { .. }))),
        "{shape:?}"
    );
}

/// The collider audit (`engine_physics::audit`) lists a drawn rock with no collider on the
/// ground, not one with its own collider, nor a pebble too small to matter, nor a sail whose
/// mast has a collider of its own, nor a hall whose roof collides, nor one far from the player.
#[test]
fn the_collider_audit_lists_drawn_rocks_nothing_collides_with() {
    let mut app = app();
    let rock = app
        .world
        .resource_mut::<Assets<Mesh>>()
        .add(engine_render::mesh::cuboid(Vec3::ONE));
    let drawn = |app: &mut App, at: Vec3, scale: f32, name: &'static str| {
        app.world
            .spawn((
                Name::new(name),
                Transform::from_translation(at.as_dvec3()).with_scale(Vec3::splat(scale)),
                MeshRenderer::new(rock, engine_render::MaterialRef::default()),
            ))
            .id()
    };
    // The ground: a slab with its top at y = 0.
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(20.0, 0.5, 20.0)),
    ));
    drawn(&mut app, Vec3::new(-3.0, 0.5, 0.0), 1.0, "bare rock");
    let solid = drawn(&mut app, Vec3::new(3.0, 0.5, 0.0), 1.0, "solid rock");
    app.world
        .entity_mut(solid)
        .insert((RigidBody::fixed(), Collider::cuboid(Vec3::splat(0.5))));
    drawn(&mut app, Vec3::new(0.0, 0.1, 3.0), 0.2, "pebble");
    // A drawn sail on a mast whose collider is only a pole, on purpose.
    let mast = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.0, -3.0),
            RigidBody::fixed(),
            Collider::cuboid(Vec3::new(0.05, 2.0, 0.05)),
        ))
        .id();
    let sail = drawn(&mut app, Vec3::new(0.0, 2.0, -3.0), 1.5, "sail");
    app.world.entity_mut(sail).insert(ChildOf(mast));
    // A hall: drawn as one mesh, colliding through a roof of its own (hollow inside).
    drawn(&mut app, Vec3::new(0.0, 3.0, 12.0), 6.0, "hall");
    app.world.spawn((
        Transform::from_xyz(0.0, 5.9, 12.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(3.0, 0.1, 3.0)),
    ));
    // Far from the player: scenery there gets its collider only once someone comes near.
    drawn(&mut app, Vec3::new(100.0, 0.5, 0.0), 1.0, "far rock");
    app.world.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        engine_world::physics::CharacterController::default(),
    ));
    app.update_n(3);
    let found = engine_physics::audit::uncovered_meshes(&app.world);
    let names: Vec<&str> = found.iter().map(|u| u.item.as_str()).collect();
    assert_eq!(names, ["bare rock"], "{found:?}");
    assert!((found[0].size - 1.0).abs() < 1e-3);
}
