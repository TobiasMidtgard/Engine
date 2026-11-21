//! The terrain's part of the frame's ray scene (`TerrainFeature::ray_scene`, the ray-traced
//! lighting round's step 5): the patches of ground around the camera, whichever way it looks,
//! are ground in the rays, built straight from the terrain's own pages, and they go with the
//! camera when it goes elsewhere. Needs a GPU with ray queries (lavapipe has them); passes
//! with a note without.

use engine_core::prelude::*;
use engine_render::RenderDisabled;
use engine_render::prelude::*;
use engine_render::settings::trial_cut_more::RAY_WORLD;
use engine_terrain::{FlatPad, PlanetTerrain, TerrainPlugin};

fn diag(app: &App, name: &str) -> f64 {
    app.world
        .resource::<Diagnostics>()
        .get(name)
        .unwrap_or(-1.0)
}

fn app() -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 96;
        w.height = 64;
    }
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(TerrainPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.gi_probes_per_frame = 16;
        s.trial_cuts_more |= RAY_WORLD;
    }
    // A flat square 4 km across, its pad level at the origin.
    let pad = FlatPad::default();
    let terrain = PlanetTerrain::flat(4096.0, 3).with_pad(pad);
    let at = terrain.home_transform(pad);
    app.world.spawn((at, terrain));
    app.world.spawn((
        DirectionalLight {
            illuminance: 50_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, -0.5), Vec3::Y),
    ));
    app
}

/// Frames until every patch the rays' walk asks for is resident, none is on its way, and the
/// terrain's choice of patches for the rays and the ground in the rays agree, for 5 frames
/// running (the patches stream in, coarse ones first, then their structures are built a share
/// a frame): the patches chosen, and the frames it took.
fn settle(app: &mut App, what: &str) -> (f64, u32) {
    let mut held = 0;
    for frame in 1..=2000 {
        app.update();
        let chosen = diag(app, "terrain.ray_patches");
        let placed = diag(app, "render.ray.class_ground");
        let streaming = diag(app, "terrain.ray_patches_missing") != 0.0
            || diag(app, "terrain.patches_building") != 0.0;
        if !streaming && chosen > 0.0 && placed == chosen.min(512.0) {
            held += 1;
            if held == 5 {
                return (chosen, frame);
            }
        } else {
            held = 0;
        }
    }
    panic!(
        "{what}: the rays' ground never settled ({} chosen, {} placed, {} missing, {} building)",
        diag(app, "terrain.ray_patches"),
        diag(app, "render.ray.class_ground"),
        diag(app, "terrain.ray_patches_missing"),
        diag(app, "terrain.patches_building")
    );
}

fn gpu_errors() -> Vec<String> {
    engine_core::log::log_buffer()
        .tail(64, Some("error"))
        .into_iter()
        .filter(|l| l.message.contains("GPU error"))
        .map(|l| l.message)
        .collect()
}

#[test]
fn the_ground_around_the_camera_is_in_the_rays() {
    let mut app = app();
    let look = |from: DVec3, toward: DVec3| {
        Transform::from_translation(from).looking_at(from + toward, Vec3::Y)
    };
    let north = DVec3::new(0.0, -0.1, -1.0);
    let camera = app
        .world
        .spawn((Camera::default(), look(DVec3::new(0.0, 2.0, 0.0), north)))
        .id();
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    if diag(&app, "render.ray.supported") != 1.0 {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return;
    }
    assert_eq!(diag(&app, "render.ray.on"), 1.0, "no ray scene");
    let (ahead, frames) = settle(&mut app, "looking north");
    eprintln!(
        "looking north: {ahead} patches in the rays after {frames} frames ({} resident, {} \
         drawn, {} triangles in the rays)",
        diag(&app, "terrain.patches_resident"),
        diag(&app, "terrain.patches_drawn"),
        diag(&app, "render.ray.triangles")
    );
    // A few hundred patches: 16 m ones near the camera at the view's own detail, coarser ones
    // farther off, and past 144 m none smaller than a third of its distance, out to the far
    // reach (384 m).
    assert!(
        (50.0..=512.0).contains(&ahead),
        "{ahead} patches around the camera"
    );

    // The ground behind the camera is in the rays as much as the ground ahead.
    let south = DVec3::new(0.0, -0.1, 1.0);
    *app.world.get_mut::<Transform>(camera).unwrap() = look(DVec3::new(0.0, 2.0, 0.0), south);
    let (behind, frames) = settle(&mut app, "looking south");
    eprintln!("looking south: {behind} patches after {frames} frames");
    assert!(
        (behind - ahead).abs() <= 0.2 * ahead,
        "{behind} patches looking south, {ahead} looking north"
    );

    // Off 1.5 km: the patches left behind leave the rays, those around the camera come in.
    *app.world.get_mut::<Transform>(camera).unwrap() = look(DVec3::new(1500.0, 2.0, 0.0), north);
    let (there, frames) = settle(&mut app, "1.5 km off");
    eprintln!("1.5 km off: {there} patches after {frames} frames");
    assert!(
        (behind - there).abs() <= 0.2 * behind,
        "{there} patches 1.5 km off, {behind} before"
    );
    let errors = gpu_errors();
    assert!(errors.is_empty(), "GPU errors: {errors:#?}");
}
