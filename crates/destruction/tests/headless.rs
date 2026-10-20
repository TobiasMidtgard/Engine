//! Renders a breached wall offscreen and checks the hole shows in the picture. Needs a GPU or
//! lavapipe; skips with a note when neither is available (CI sets `ENGINE_REQUIRE_GPU=1`, which
//! makes the renderer fail instead).

use engine_core::math::{DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_destruction::{DestructibleWall, DestructionPlugin, Hit, HitQueue, WallState};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;

#[test]
fn a_thermite_doorway_shows_the_world_behind_the_wall() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 320;
        w.height = 240;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin, DestructionPlugin));
    // No haze: the hole is judged by the sky's full brightness through it, whatever the fog
    // does near the horizon.
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 1.5, 6.0).looking_at(DVec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.6, -0.7), Vec3::Y),
    ));
    let wall = app
        .world
        .spawn((
            Transform::default(),
            DestructibleWall::reinforced(4.0, 3.0).with_paint([0.12, 0.12, 0.13]),
        ))
        .id();
    app.update_n(2);
    app.world.resource_mut::<HitQueue>().hits.push(Hit::Cut {
        wall,
        min: Vec2::new(-0.6, 0.6),
        max: Vec2::new(0.6, 2.4),
        front: true,
        melt: false,
        tip: Vec3::ZERO,
    });
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let s = app.world.get::<WallState>(wall).unwrap();
    assert!(!s.grid.solid[s.grid.cell_at(Vec2::new(0.0, 1.5)).unwrap()]);

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("destruction_doorway.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let lum = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        p[0] as f32 + p[1] as f32 + p[2] as f32
    };
    // The dark painted wall left of the doorway, and the bright sky through it.
    let wall_px = lum(90, 120);
    let hole_px = lum(160, 90);
    assert!(
        hole_px > wall_px + 60.0,
        "the doorway ({hole_px}) should be brighter than the dark wall ({wall_px}): {}",
        path.display()
    );
}
