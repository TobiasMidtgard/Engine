//! Pipelined rendering (`RenderSettings::pipelining`): the renderer records and submits on its
//! own thread while the next frame simulates. Its results (stats, diagnostics) arrive a frame
//! later, `capture_now` still returns this frame's picture, and the picture matches an inline
//! renderer's. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::mesh::icosphere;
use engine_render::prelude::*;
use engine_render::{
    Mesh, Pipelining, RenderDisabled, RenderPaused, RenderStats, RenderThread, Renderer,
    capture_now, with_renderer,
};

fn app(pipelining: Pipelining) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 160;
        w.height = 90;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins(RenderPlugin);
    app.world.resource_mut::<RenderSettings>().pipelining = pipelining;
    for i in 0..5 {
        let mesh = app
            .world
            .resource_mut::<Assets<Mesh>>()
            .add(icosphere(0.5, 3));
        app.world.spawn((
            Transform::from_xyz(i as f64 * 1.5 - 3.0, 0.5, 0.0),
            MeshRenderer::new(
                mesh,
                StandardMaterial::color(0.8, 0.3 + 0.1 * i as f32, 0.2),
            ),
        ));
    }
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 3.0, 9.0).looking_at(DVec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, -0.2), Vec3::Y),
    ));
    app
}

fn pixels(path: &std::path::Path) -> Vec<u8> {
    image::open(path).expect("screenshot").to_rgba8().into_raw()
}

#[test]
fn pipelined_frames_return_a_frame_late_and_capture_matches_inline() {
    let mut piped = app(Pipelining::Always);
    piped.update();
    if let Some(d) = piped.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU ({}), skipping", d.0);
        return;
    }
    // The renderer is on its thread; the first frame's results come with the second.
    assert!(piped.world.contains_resource::<RenderThread>());
    assert!(!piped.world.contains_resource::<Renderer>());
    assert_eq!(piped.world.resource::<RenderStats>().instances, 0);
    piped.update();
    assert!(piped.world.resource::<RenderStats>().instances > 0);
    let metrics = &piped.world.resource::<Diagnostics>().metrics;
    for name in [
        "render.extract_ms",
        "render.record_ms",
        "render.wait_ms",
        "render.finish_ms",
        "render.submit_ms",
        "render.screenshot_ms",
        "render.record_other_ms",
        "render.record_total_ms",
        "render.record_cpu_ms",
        "render.backend",
        "render.cpu_ms",
    ] {
        assert!(metrics.contains_key(name), "{name} missing");
    }
    // The render thread's parts add up to its total.
    let parts: f64 = [
        "render.record_ms",
        "render.acquire_ms",
        "render.finish_ms",
        "render.submit_ms",
        "render.present_ms",
        "render.screenshot_ms",
        "render.record_other_ms",
    ]
    .iter()
    .map(|n| metrics.get(*n).copied().unwrap_or(0.0))
    .sum();
    let total = metrics["render.record_total_ms"];
    assert!(
        (parts - total).abs() < 0.01,
        "parts {parts} ms, total {total} ms"
    );
    let described = with_renderer(&mut piped.world, |r, _| r.describe()).expect("renderer");
    assert!(described.is_object());

    let mut inline = app(Pipelining::Off);
    for _ in 0..2 {
        inline.update();
    }
    assert!(inline.world.contains_resource::<Renderer>());

    // Same frames, same picture: capture_now renders inline either way.
    let dir = std::env::temp_dir().join("engine_pipelined_test");
    std::fs::create_dir_all(&dir).unwrap();
    for _ in 0..8 {
        piped.update();
        inline.update();
    }
    // The same scene counts the same draws either way (nothing counted twice).
    let draws = |app: &App| app.world.resource::<RenderStats>().draw_calls;
    assert_eq!(draws(&piped), draws(&inline));
    let a = dir.join("pipelined.png");
    let b = dir.join("inline.png");
    capture_now(&mut piped.world, &a).expect("pipelined capture");
    capture_now(&mut inline.world, &b).expect("inline capture");
    let (a, b) = (pixels(&a), pixels(&b));
    assert_eq!(a.len(), b.len());
    let diff = a
        .iter()
        .zip(&b)
        .map(|(x, y)| x.abs_diff(*y) as u64)
        .sum::<u64>() as f64
        / a.len() as f64;
    eprintln!("mean difference {diff:.3}");
    assert!(diff < 1.0, "pictures differ by {diff:.3} on average");

    // Turning it off brings the renderer home.
    piped.world.resource_mut::<RenderSettings>().pipelining = Pipelining::Off;
    piped.update();
    assert!(piped.world.contains_resource::<Renderer>());
    assert!(!piped.world.contains_resource::<RenderThread>());
}

#[test]
fn paused_frames_draw_nothing_inline_or_pipelined() {
    for pipelining in [Pipelining::Always, Pipelining::Off] {
        let mut app = app(pipelining);
        for _ in 0..2 {
            app.update();
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}), skipping", d.0);
            return;
        }
        // Results inserted while paused would mean frames were drawn.
        app.world.insert_resource(RenderPaused);
        app.world.remove_resource::<RenderStats>();
        for _ in 0..3 {
            app.update();
        }
        assert!(
            !app.world.contains_resource::<RenderStats>(),
            "{pipelining:?} drew while paused"
        );
        app.world.remove_resource::<RenderPaused>();
        for _ in 0..2 {
            app.update();
        }
        assert!(app.world.resource::<RenderStats>().instances > 0);
    }
}
