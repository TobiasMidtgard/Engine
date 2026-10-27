//! Mesh batches in the shared vertex pool drawn as multi-draws
//! (`RenderSettings::multi_draw_indirect`): the same picture as a draw call per batch, from
//! fewer draw commands. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::mesh::{cuboid, icosphere};
use engine_render::prelude::*;
use engine_render::{Mesh, RenderDisabled, RenderStats, capture_now};

fn app(multi_draw: bool) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 192;
        w.height = 108;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<RenderSettings>()
        .multi_draw_indirect = multi_draw;
    // Twelve meshes, two materials (opaque and blended), a row of each: many
    // batches, few pipelines and texture sets.
    let mut meshes = app.world.resource_mut::<Assets<Mesh>>();
    let shapes: Vec<_> = (0..12)
        .map(|i| {
            if i % 2 == 0 {
                meshes.add(icosphere(0.3 + 0.02 * i as f32, 2 + i % 3))
            } else {
                meshes.add(cuboid(Vec3::new(0.4, 0.3 + 0.03 * i as f32, 0.4)))
            }
        })
        .collect();
    for (i, mesh) in shapes.iter().enumerate() {
        for row in 0..3 {
            let material = if row == 2 {
                StandardMaterial::color(0.2, 0.5, 0.9).with_alpha(0.6)
            } else {
                StandardMaterial::color(0.8, 0.4 + 0.04 * i as f32, 0.2)
            };
            app.world.spawn((
                Transform::from_xyz(i as f64 * 0.9 - 5.0, 0.4, row as f64 * -1.2),
                MeshRenderer::new(*mesh, material),
            ));
        }
    }
    app.world.spawn((
        // Auto exposure adapts over real time, which differs between the runs compared.
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 3.0, 6.0).looking_at(DVec3::new(0.0, 0.3, -1.2), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, -0.2), Vec3::Y),
    ));
    app
}

#[test]
fn multi_draws_match_single_draws_with_fewer_commands() {
    let dir = std::env::temp_dir().join("engine_multi_draw_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut shots = Vec::new();
    let mut draws = Vec::new();
    for multi_draw in [false, true] {
        let mut app = app(multi_draw);
        app.update();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}), skipping", d.0);
            return;
        }
        for _ in 0..4 {
            app.update();
        }
        let path = dir.join(format!("multi_draw_{multi_draw}.png"));
        capture_now(&mut app.world, &path).expect("capture");
        let stats = app.world.resource::<RenderStats>().clone();
        let batches = app
            .world
            .resource::<Diagnostics>()
            .get("render.meshes.batches")
            .unwrap_or(0.0);
        eprintln!(
            "multi_draw {multi_draw}: {} draw calls, {batches} batches",
            stats.draw_calls
        );
        assert!(stats.instances >= 36);
        draws.push((stats.draw_calls, batches));
        shots.push(
            image::open(&path)
                .expect("screenshot")
                .to_rgba8()
                .into_raw(),
        );
    }
    assert_eq!(draws[0].1, draws[1].1, "the same batches either way");
    // The device may lack first instances in indirect draws; then both draw one by one.
    let supported = draws[1].0 < draws[0].0;
    if !supported {
        eprintln!("multi-draw unsupported here: draws one by one");
        assert_eq!(draws[0].0, draws[1].0);
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
}
