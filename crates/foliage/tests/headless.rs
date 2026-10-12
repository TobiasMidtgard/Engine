//! Renders plants offscreen and checks the pictures: a sunlit oak has a green crown, and
//! glowcaps light up a dark scene. Needs a GPU or a software Vulkan driver (lavapipe); passes
//! with a note when neither is available.

use engine_core::prelude::*;
use engine_foliage::{FoliagePlugin, Genes, spawn_plant};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

fn app(width: u32, height: u32, sun_lux: f32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(engine_world::WorldPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(FoliagePlugin);
    let w = &mut app.world;
    w.spawn((
        Camera::default(),
        Transform::from_xyz(0.0, 2.0, 15.0).looking_at(DVec3::new(0.0, 5.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight {
            illuminance: sun_lux,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4a3b2a).with_roughness(0.95),
        ),
    ));
    app
}

fn capture(app: &mut App, name: &str) -> Option<image::RgbaImage> {
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    eprintln!("saved {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

#[test]
fn a_sunlit_oak_has_a_green_crown_and_a_trunk() {
    let mut app = app(320, 180, 60_000.0);
    spawn_plant(
        &mut app.world,
        "oak",
        DVec3::ZERO,
        0.7,
        Some(Genes::default()),
    )
    .unwrap();
    spawn_plant(&mut app.world, "fern", DVec3::new(2.0, 0.0, 4.0), 0.9, None).unwrap();
    let Some(img) = capture(&mut app, "foliage_oak.png") else {
        return;
    };
    // The crown fills a good part of the upper middle of the frame with green.
    let mut green = 0;
    let mut total = 0;
    for y in 10..110 {
        for x in 110..210 {
            let p = img.get_pixel(x, y);
            total += 1;
            if p[1] > p[0] && p[1] > p[2] && p[1] > 20 {
                green += 1;
            }
        }
    }
    assert!(
        green * 8 > total,
        "only {green}/{total} green pixels in the crown area"
    );
    let stats = app.world.resource::<engine_render::RenderStats>();
    assert!(stats.draw_calls >= 3, "{stats:?}");
}

#[test]
fn glowcaps_light_up_the_dark() {
    let mut app = app(320, 180, 0.0);
    {
        let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
        for mut t in q.iter_mut(&mut app.world) {
            *t = Transform::from_xyz(0.0, 1.2, 3.5).looking_at(DVec3::new(0.0, 0.2, 0.0), Vec3::Y);
        }
    }
    for (i, p) in [
        (-0.6, 0.0),
        (0.0, 0.3),
        (0.5, -0.2),
        (0.2, 0.6),
        (-0.3, -0.5),
    ]
    .iter()
    .enumerate()
    {
        let genes = Genes {
            variant: i as u8,
            ..Default::default()
        };
        spawn_plant(
            &mut app.world,
            "glowcap",
            DVec3::new(p.0, 0.0, p.1),
            1.0,
            Some(genes),
        )
        .unwrap();
    }
    let Some(img) = capture(&mut app, "foliage_glowcaps.png") else {
        return;
    };
    // Cyan glow: some pixels are bright, with green and blue well above red.
    let glowing = img
        .pixels()
        .filter(|p| p[1] > 90 && p[2] > 60 && p[1] as i32 > p[0] as i32 + 30)
        .count();
    assert!(glowing > 40, "only {glowing} glowing pixels");
}
