//! With the frame profiler on, the scene pass's GPU time is split by feature
//! (`render.gpu.scene: <feature>`, then `render.gpu.scene: sky phase`) and `render.gpu.scene`
//! stays their sum, so a run on a player's machine says what the scene's time is spent drawing.
//! Needs a GPU (or lavapipe) with timestamps; passes with a note otherwise.

use engine_core::prelude::*;
use engine_render::RenderDisabled;
use engine_render::prelude::*;

#[test]
fn the_profiler_splits_the_scene_by_feature() {
    // SAFETY: set before anything else in this test binary reads the environment.
    unsafe { std::env::set_var("ENGINE_PROFILE", "1") };
    engine_core::log::init("error");
    assert!(engine_core::profile::enabled(), "the profiler is on");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 64;
        w.height = 64;
    }
    app.add_plugins(RenderPlugin);
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 1.0, 4.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [1.0, 1.0, 1.0],
            },
            StandardMaterial::color(0.5, 0.5, 0.5),
        ),
    ));
    for _ in 0..12 {
        app.update();
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let d = app.world.resource::<Diagnostics>();
    let Some(&scene) = d.metrics.get("render.gpu.scene") else {
        eprintln!("skipping: no GPU timestamps");
        return;
    };
    let parts: Vec<_> = d
        .metrics
        .iter()
        .filter(|(k, _)| k.starts_with("render.gpu.scene: "))
        .collect();
    eprintln!("scene {scene} ms: {parts:?}");
    for part in ["meshes", "sky phase"] {
        assert!(
            d.metrics.contains_key(&format!("render.gpu.scene: {part}")),
            "no scene time for {part}"
        );
    }
    let sum: f64 = parts.iter().map(|(_, ms)| **ms).sum();
    assert!(
        (sum - scene).abs() <= 1e-3 + 1e-3 * scene,
        "the parts add up to {sum} ms of {scene}"
    );
}
