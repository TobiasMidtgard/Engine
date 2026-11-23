//! The renderer's frame timing: frame time, GPU time (where the GPU has timestamps, also per
//! pass), the render system's CPU steps and the wait for the window image reach `Diagnostics`.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

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
        // Every pass is timed, and the passes add up to the frame.
        let passes: Vec<_> = d
            .metrics
            .iter()
            .filter(|(k, _)| k.starts_with("render.gpu."))
            .collect();
        eprintln!("GPU passes {passes:?}");
        for pass in ["scene", "prepass", "tonemap", "compute meshes"] {
            assert!(
                get(&format!("render.gpu.{pass}")).is_some_and(|ms| ms >= 0.0),
                "no GPU time for {pass}"
            );
        }
        let sum: f64 = passes.iter().map(|(_, ms)| **ms).sum();
        assert!(
            (sum - ms).abs() <= 0.05 * ms + 0.01,
            "passes add up to {sum} ms of {ms}"
        );
    }
    // The render system's CPU steps too.
    for step in ["view", "prepare meshes", "scene", "submit"] {
        assert!(
            get(&format!("render.cpu.{step}")).is_some_and(|ms| ms >= 0.0),
            "no CPU time for {step}"
        );
    }
}

/// With the frame's ray scene on (`trial_cut_more::RAY_WORLD`) the frame's rays share one
/// compute pass: its parts are still rows of their own, on the CPU always and on the GPU where
/// the device writes timestamps inside a pass (`render.gpu_times_inside_passes`), and the
/// passes still add up to the frame. Passes with a note on a GPU without ray queries.
#[test]
fn the_ray_pass_reports_its_parts() {
    ray_pass_parts(false);
}

/// The same with every pixel's reflection (`trial_cut_more::RAY_REFLECTIONS`): its rows are
/// "ray reflections", the last of the ray pass, and "ray reflections resolve", in place of
/// "world reflections" and "reflections".
#[test]
fn every_pixels_reflection_reports_its_parts() {
    ray_pass_parts(true);
}

fn ray_pass_parts(full: bool) {
    use engine_render::settings::trial_cut_more::{RAY_REFLECTIONS, RAY_WORLD};
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 64;
        w.height = 64;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.perf_log_secs = 0.01;
        s.probe_gi = true;
        s.gi_hardware_rays = true;
        s.trial_cuts_more |= RAY_WORLD;
        if full {
            s.trial_cuts_more |= RAY_REFLECTIONS;
        }
        s.ray_sun_shadows = true;
    }
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 1.0, 4.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, -0.4), Vec3::Y),
    ));
    for (at, size) in [
        (DVec3::ZERO, [1.0, 1.0, 1.0]),
        (DVec3::new(0.0, -0.6, 0.0), [20.0, 0.2, 20.0]),
    ] {
        app.world.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(
                Shape::Cuboid { size },
                StandardMaterial::color(0.5, 0.5, 0.5),
            ),
        ));
    }
    for _ in 0..12 {
        app.update();
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let d = app.world.resource::<Diagnostics>();
    let get = |k: &str| d.metrics.get(k).copied();
    if get("render.ray.supported") != Some(1.0) {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return;
    }
    assert_eq!(get("render.ray.on"), Some(1.0), "the ray scene is on");
    assert_eq!(get("render.ray.losses"), Some(0.0));
    let (reflections, resolve, other) = if full {
        (
            "ray reflections",
            "ray reflections resolve",
            "world reflections",
        )
    } else {
        ("world reflections", "reflections", "ray reflections")
    };
    for step in [
        "ray scene",
        "ray scene build",
        "ray inputs",
        "probe gi",
        "ray shadows",
        reflections,
        resolve,
        "probe gi copies",
    ] {
        assert!(
            get(&format!("render.cpu.{step}")).is_some_and(|ms| ms >= 0.0),
            "no CPU time for {step}"
        );
    }
    // (A row that is gone reads 0.0: the publish keeps every row it has ever had.)
    assert!(
        get(&format!("render.cpu.{other}")).is_none_or(|ms| ms == 0.0),
        "{other}: not in this frame"
    );
    let Some(ms) = get("render.gpu_ms") else {
        eprintln!("no GPU timestamps here: the CPU rows only");
        return;
    };
    let passes: Vec<_> = d
        .metrics
        .iter()
        .filter(|(k, _)| k.starts_with("render.gpu."))
        .collect();
    eprintln!("GPU passes {passes:?}");
    let inside = get("render.gpu_times_inside_passes") == Some(1.0);
    eprintln!("timestamps inside a pass: {inside}");
    for pass in [
        "ray scene build",
        "ray inputs",
        "probe gi",
        "probe gi copies",
        resolve,
    ] {
        assert!(
            get(&format!("render.gpu.{pass}")).is_some_and(|ms| ms >= 0.0),
            "no GPU time for {pass}"
        );
    }
    // (Without such timestamps the two run under "probe gi"; the reflections still get a row
    // in a frame whose probe grid scrolled, where they have a pass of their own.)
    if inside {
        for pass in ["ray shadows", reflections] {
            assert!(
                get(&format!("render.gpu.{pass}")).is_some_and(|ms| ms >= 0.0),
                "{pass}: a GPU row of its own where the device times inside a pass"
            );
        }
    }
    let sum: f64 = passes.iter().map(|(_, ms)| **ms).sum();
    assert!(
        (sum - ms).abs() <= 0.05 * ms + 0.01,
        "passes add up to {sum} ms of {ms}"
    );
}
