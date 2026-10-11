//! Renders a small scene offscreen and checks the picture. Needs a GPU or a software Vulkan
//! driver (lavapipe); passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

fn scene_app(width: u32, height: u32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(RenderPlugin);
    let w = &mut app.world;
    w.spawn((
        Camera::default(),
        Transform::from_xyz(0.0, 2.5, 9.0).looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane { size: [60.0, 60.0], subdivisions: 1 },
            StandardMaterial::hex(0x4f7a3a).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Transform::from_xyz(-2.0, 1.0, 0.0),
        MeshRenderer::new(
            Shape::Sphere { radius: 1.0, segments: 48 },
            StandardMaterial::hex(0xd94f30).with_roughness(0.35),
        ),
    ));
    w.spawn((
        Transform::from_xyz(1.5, 0.75, 0.5),
        MeshRenderer::new(
            Shape::Cuboid { size: [1.5, 1.5, 1.5] },
            StandardMaterial::hex(0xc0c4cc).with_metallic(1.0).with_roughness(0.25),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 1.0, -3.0),
        MeshRenderer::new(
            Shape::Torus { radius: 1.0, tube: 0.3, segments: 48 },
            StandardMaterial::hex(0x3060d0).with_emissive([0.3, 0.5, 1.0], 20_000.0),
        ),
    ));
    w.spawn((
        Transform::from_xyz(2.5, 0.5, 2.5),
        PointLight {
            intensity_lm: 20_000.0,
            ..Default::default()
        },
    ));
    app
}

#[test]
fn renders_a_lit_scene_to_png() {
    let mut app = scene_app(480, 270);
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let dir = env!("CARGO_TARGET_TMPDIR");
    let path = std::path::Path::new(&dir).join("headless_scene.png");
    let info = capture_now(&mut app.world, &path).expect("screenshot");
    assert_eq!((info.width, info.height), (480, 270));

    let img = image::open(&path).expect("png").to_rgba8();
    // Sky at the top is bright and blue-ish.
    let sky = img.get_pixel(240, 5);
    assert!(sky[2] > 60 && sky[2] >= sky[0], "sky pixel {sky:?}");
    // The scene isn't uniform: the ground and objects differ from the sky.
    let ground = img.get_pixel(240, 260);
    assert!(ground[1] > 10, "ground pixel {ground:?}");
    let mut distinct = std::collections::HashSet::new();
    for (_, _, p) in img.enumerate_pixels() {
        distinct.insert([p[0] / 16, p[1] / 16, p[2] / 16]);
    }
    assert!(distinct.len() > 40, "image looks flat ({} colors)", distinct.len());

    let stats = app.world.resource::<engine_render::RenderStats>();
    assert!(stats.draw_calls >= 4, "{stats:?}");
    eprintln!("rendered on {} ({}): {:?} -> {}", stats.adapter, stats.backend, stats, path.display());
}

#[test]
fn night_scene_with_lamp_is_dark_but_not_black() {
    let mut app = scene_app(320, 180);
    {
        let mut q = app.world.query::<&mut DirectionalLight>();
        for mut l in q.iter_mut(&mut app.world) {
            l.illuminance = 0.25; // moonlight
        }
    }
    app.update_n(2);
    if app.world.contains_resource::<RenderDisabled>() {
        return;
    }
    let dir = env!("CARGO_TARGET_TMPDIR");
    let path = std::path::Path::new(&dir).join("headless_night.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let mean: f32 = img.pixels().map(|p| p[0] as f32 + p[1] as f32 + p[2] as f32).sum::<f32>()
        / (img.width() * img.height() * 3) as f32;
    assert!(mean > 2.0, "night is pitch black (mean {mean})");
}
