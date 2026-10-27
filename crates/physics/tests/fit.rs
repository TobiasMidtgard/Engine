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
