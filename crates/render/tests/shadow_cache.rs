//! Far shadow cascades keep their image for a few frames (`RenderSettings::shadow_cascade_reuse`):
//! a camera walking through a field of pillars and a dense boulder sees the same shadows as with
//! every cascade redrawn each frame, while drawing less. Needs a GPU or lavapipe; passes with a
//! note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::rock::rock;
use engine_render::{Mesh, RenderDisabled, RenderStats, capture_now};

const W: u32 = 320;
const H: u32 = 180;
const FRAMES: u32 = 12;

/// Walk the camera for a few frames with cascades reused up to `reuse` frames: the draw calls
/// over the walk and the last frame's pixels.
fn walk(reuse: u32, shot: &str) -> Option<(u32, Vec<u8>)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.shadow_cascade_reuse = reuse;
    }
    let w = &mut app.world;
    let camera = w
        .spawn((
            Camera::perspective(60.0),
            Transform::from_xyz(0.0, 2.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -60.0), Vec3::Y),
        ))
        .id();
    // A low sun: long shadows reaching across the far cascades.
    let e = 18f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.7 * e.cos(), -e.sin(), -0.7 * e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [800.0, 800.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.5, 0.5, 0.45),
        ),
    ));
    let pillar = StandardMaterial::color(0.6, 0.55, 0.5);
    for i in 0..48 {
        let z = -6.0 - (i as f64).powf(1.6) * 1.5;
        let x = if i % 2 == 0 { -4.0 } else { 4.0 } * (1.0 + i as f64 * 0.15);
        w.spawn((
            Transform::from_xyz(x, 2.0, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.0, 4.0, 1.0],
                },
                pillar,
            ),
        ));
    }
    // Dense enough for the virtual geometry path, far enough for the third and fourth cascades.
    let boulder = w
        .resource_mut::<Assets<Mesh>>()
        .add(rock(Vec3::new(6.0, 4.0, 5.0), 3, 6));
    w.spawn((
        Transform::from_xyz(-10.0, 1.0, -120.0),
        MeshRenderer::new(boulder, StandardMaterial::color(0.4, 0.38, 0.35)),
    ));
    // Settle: the boulder's clusters are built in the background.
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 60 {
        app.update();
        if app.world.get_resource::<RenderDisabled>().is_some() {
            break;
        }
        if app
            .world
            .get_resource::<RenderStats>()
            .is_some_and(|s| s.clusters > 0)
        {
            break;
        }
    }
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let mut draws = 0;
    for _ in 0..FRAMES {
        app.world
            .get_mut::<Transform>(camera)
            .expect("camera")
            .translation
            .z -= 0.03;
        app.update();
        draws += app.world.resource::<RenderStats>().draw_calls;
    }
    let dir = std::env::temp_dir().join("engine-shadow-cache");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    let image = image::open(&path).expect("read the shot").to_rgb8();
    Some((draws, image.into_raw()))
}

#[test]
fn reused_far_cascades_cast_the_same_shadows_with_fewer_draws() {
    let Some((every, before)) = walk(0, "cascades_every_frame.png") else {
        return;
    };
    let Some((reused, after)) = walk(3, "cascades_reused.png") else {
        return;
    };
    eprintln!("draw calls over the walk: {every} redrawing every frame, {reused} reusing");
    assert!(reused < every, "{reused} draws reusing, {every} without");
    let diff = before
        .iter()
        .zip(&after)
        .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs())
        .sum::<u32>() as f32
        / before.len() as f32;
    eprintln!("mean difference {diff:.2} of 255");
    assert!(
        diff < 1.0,
        "the images differ by {diff:.2} of 255 on average"
    );
}
