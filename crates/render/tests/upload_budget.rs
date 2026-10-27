//! The upload budget: new meshes past `RenderSettings::upload_mb_per_frame` wait for later
//! frames (`render.pending_upload_mb` says how much), and all are drawn once it is back to
//! zero; behind a loading screen (`LoadingBudget`) they all go in the first frame. Needs a GPU
//! or lavapipe; passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::mesh::icosphere;
use engine_render::prelude::*;
use engine_render::{Mesh, RenderDisabled, RenderStats};
use engine_world::LoadingBudget;

const BALLS: usize = 12;

/// A row of `BALLS` balls, each its own ~0.9 MB mesh, under a sun, budget `mb` a frame.
fn app(mb: f32) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 160;
        w.height = 90;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<RenderSettings>()
        .upload_mb_per_frame = mb;
    for i in 0..BALLS {
        let mesh = app
            .world
            .resource_mut::<Assets<Mesh>>()
            .add(icosphere(0.4 + 0.01 * i as f32, 5));
        app.world.spawn((
            Transform::from_xyz(i as f64 - BALLS as f64 / 2.0, 0.5, 0.0),
            MeshRenderer::new(mesh, StandardMaterial::color(0.8, 0.3, 0.2)),
        ));
    }
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 3.0, 12.0).looking_at(DVec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, -0.2), Vec3::Y),
    ));
    app
}

fn pending_mb(app: &App) -> f64 {
    app.world.resource::<Diagnostics>().metrics["render.pending_upload_mb"]
}

#[test]
fn new_meshes_spread_over_frames_and_all_arrive() {
    let mut app = app(1.0);
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU ({}), skipping", d.0);
        return;
    }
    let first = app.world.resource::<RenderStats>().instances;
    let pending = pending_mb(&app);
    eprintln!("first frame: {first} instances, {pending:.2} MB pending");
    assert!(first < BALLS as u32, "all {first} balls in the first frame");
    assert!(pending > 2.0, "only {pending:.2} MB pending");

    let mut frames = 1;
    while pending_mb(&app) > 0.0 {
        app.update();
        frames += 1;
        assert!(
            frames < 4 * BALLS,
            "still {:.2} MB pending",
            pending_mb(&app)
        );
    }
    app.update();
    let instances = app.world.resource::<RenderStats>().instances;
    eprintln!("all uploaded after {frames} frames: {instances} instances");
    assert!(frames >= BALLS / 2, "done in {frames} frames");
    assert_eq!(instances, BALLS as u32);
}

#[test]
fn a_loading_screen_uploads_everything_at_once() {
    let mut app = app(1.0);
    app.world.insert_resource(LoadingBudget {
        budget_ms: 10_000.0,
    });
    app.update();
    if app.world.get_resource::<RenderDisabled>().is_some() {
        return;
    }
    assert_eq!(pending_mb(&app), 0.0);
    assert_eq!(app.world.resource::<RenderStats>().instances, BALLS as u32);
}
