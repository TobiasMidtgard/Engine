//! Mesh pipeline variants and material presets' generator pipelines compiled ahead on worker
//! threads (`RenderSettings::pipeline_warmup`): kinds of material first seen later in the scene
//! draw the same picture as without the warm-up, and their first frame doesn't wait for a
//! shader compile. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::mesh::icosphere;
use engine_render::prelude::*;
use engine_render::{MaterialPreset, Mesh, PipelineWarmup, RenderDisabled, capture_now};

fn app(warmup: PipelineWarmup) -> App {
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
    app.world.resource_mut::<RenderSettings>().pipeline_warmup = warmup;
    let mesh = app
        .world
        .resource_mut::<Assets<Mesh>>()
        .add(icosphere(0.6, 3));
    app.world.spawn((
        Transform::from_xyz(-1.0, 0.6, 0.0),
        // A preset from the start: the shader parts presets need are on from the first frame.
        MeshRenderer::new(mesh, StandardMaterial::preset(MaterialPreset::Brick)),
    ));
    app.world.spawn((
        // Auto exposure adapts over real time, which the slower cold frames would change.
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 2.0, 5.0).looking_at(DVec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, -0.2), Vec3::Y),
    ));
    app
}

fn metric(app: &App, name: &str) -> f64 {
    app.world.resource::<Diagnostics>().get(name).unwrap_or(0.0)
}

#[test]
fn materials_first_seen_later_draw_alike_without_a_compile() {
    let dir = std::env::temp_dir().join("engine_pipeline_warmup_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut shots = Vec::new();
    let mut compile_ms = Vec::new();
    let mut presets_compiled = Vec::new();
    for warmup in [PipelineWarmup::Off, PipelineWarmup::Always] {
        let mut app = app(warmup);
        app.update();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}), skipping", d.0);
            return;
        }
        // Let the workers finish (they build every variant), drawing the same frames either
        // way so both pictures have the same history (TAA, exposure).
        let warming = |app: &App| {
            metric(app, "render.meshes.pipelines_warming")
                + metric(app, "render.materials.pipelines_warming")
        };
        for _ in 0..40 {
            if warming(&app) > 0.0 {
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            app.update();
        }
        if warmup == PipelineWarmup::Always {
            assert_eq!(warming(&app), 0.0);
        }
        // A blended, double-sided material: two variants the scene hasn't drawn with.
        let mesh = app
            .world
            .resource_mut::<Assets<Mesh>>()
            .add(icosphere(0.5, 2));
        let glass = StandardMaterial {
            double_sided: true,
            ..StandardMaterial::color(0.3, 0.6, 0.9).with_alpha(0.5)
        };
        app.world.spawn((
            Transform::from_xyz(1.0, 0.6, 0.0),
            MeshRenderer::new(mesh, glass),
        ));
        // And a material preset, whose textures are generated on first sight.
        app.world.spawn((
            Transform::from_xyz(0.0, 0.6, -1.0),
            MeshRenderer::new(mesh, StandardMaterial::preset(MaterialPreset::Marble)),
        ));
        app.update();
        let ms = metric(&app, "render.cpu.meshes: pipelines");
        // Generating the preset's textures takes long under lavapipe either way; what the
        // warm-up saves is compiling its pipelines (150 ms there).
        presets_compiled.push(metric(&app, "render.materials.pipelines_built_inline"));
        eprintln!("{warmup:?}: first frame with the new material compiled for {ms:.1} ms");
        compile_ms.push(ms);
        for _ in 0..3 {
            app.update();
        }
        let path = dir.join(format!("{warmup:?}.png"));
        capture_now(&mut app.world, &path).expect("capture");
        shots.push(
            image::open(&path)
                .expect("screenshot")
                .to_rgba8()
                .into_raw(),
        );
    }
    let (a, b) = (&shots[0], &shots[1]);
    assert_eq!(a.len(), b.len());
    let max = a
        .iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0);
    assert!(max <= 1, "pictures differ by up to {max}");
    assert_eq!(
        presets_compiled,
        // Cold, brick and marble each compiled on first use; warmed, neither.
        [2.0, 0.0],
        "presets compiled on first use"
    );
    // A compile takes hundreds of milliseconds under lavapipe, taking a built one microseconds.
    if compile_ms[0] > 50.0 {
        assert!(
            compile_ms[1] < compile_ms[0] / 4.0,
            "warmed: {:.1} ms, cold: {:.1} ms",
            compile_ms[1],
            compile_ms[0]
        );
    }
}
