//! Renders the proving ground offscreen after a second of simulation and checks the picture.
//! Needs a GPU or lavapipe; passes with a note when neither is available (CI sets
//! `ENGINE_REQUIRE_GPU=1`, which makes the renderer fail instead).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats, demo};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::WorldPlugin;

#[test]
fn proving_ground_renders_and_moves() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 320;
        w.height = 180;
    }
    app.add_plugins(RenderPlugin);
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.spawn((
        Camera::perspective(55.0),
        Transform::from_xyz(0.0, 24.0, 44.0).looking_at(DVec3::new(0.0, 2.0, -3.0), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.45, -0.75, -0.5), Vec3::Y),
    ));
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    app.update();
    demo::finish_building(&mut app.world);
    let ball = {
        let mut q = app.world.query::<(Entity, &Name)>();
        q.iter(&app.world)
            .find(|(_, n)| n.as_str() == "Wrecking ball")
            .map(|(e, _)| e)
            .expect("the demo has a wrecking ball")
    };
    let start = app.world.get::<Transform>(ball).unwrap().translation;
    // Simulate a second without drawing (the renderer draws every frame, from the origin when
    // no camera is active, so it is parked), then draw a few frames for auto exposure.
    let renderer = app.world.remove_resource::<Renderer>();
    app.world
        .insert_resource(RenderDisabled("simulating without drawing".into()));
    app.update_n(57);
    app.world.remove_resource::<RenderDisabled>();
    if let Some(r) = renderer {
        app.world.insert_resource(r);
    }
    app.update_n(3);

    let stats = *app.world.resource::<PhysicsStats>();
    assert!(stats.bodies > 400, "{stats:?}");
    let now = app.world.get::<Transform>(ball).unwrap().translation;
    assert!(
        now.distance(start) > 2.0,
        "the wrecking ball should be swinging: {start:?} → {now:?}"
    );

    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping the picture: no GPU ({})", d.0);
        return;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("proving_ground.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let mut distinct = std::collections::HashSet::new();
    for (_, _, p) in img.enumerate_pixels() {
        distinct.insert([p[0] / 16, p[1] / 16, p[2] / 16]);
    }
    assert!(
        distinct.len() > 40,
        "image looks flat ({} colors)",
        distinct.len()
    );
    eprintln!("{} ({stats:?})", path.display());
}
