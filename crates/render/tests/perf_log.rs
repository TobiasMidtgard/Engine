//! The renderer's frame timing: frame time, GPU time (where the GPU has timestamps) and the
//! wait for the window image reach `Diagnostics`. Needs a GPU or lavapipe; passes with a note
//! when neither is available.

use engine_core::prelude::*;
use engine_render::RenderDisabled;
use engine_render::prelude::*;

#[test]
fn frame_and_gpu_times_reach_diagnostics() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 64;
        w.height = 64;
    }
    app.add_plugins(RenderPlugin);
    app.world.resource_mut::<RenderSettings>().perf_log_secs = 0.01;
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
    let get = |k: &str| d.metrics.get(k).copied();
    eprintln!(
        "frame {:?} ms, gpu {:?} ms, memory {:?} MB",
        get("render.frame_ms"),
        get("render.gpu_ms"),
        get("render.gpu_memory_mb")
    );
    assert!(get("render.frame_ms").is_some_and(|ms| ms > 0.0));
    assert!(
        get("render.acquire_ms").is_some_and(|ms| ms == 0.0),
        "no window, no wait"
    );
    if let Some(ms) = get("render.gpu_ms") {
        assert!(ms > 0.0 && ms < 10_000.0, "GPU time {ms} ms");
    }
}
