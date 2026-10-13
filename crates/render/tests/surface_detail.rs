//! Procedural surface detail (`StandardMaterial::detail`): bumps and mottling break up plain
//! surfaces up close and calm down far away. Needs a GPU or lavapipe; passes with a note when
//! neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A stone-colored wall seen from 4 m, or (`wall` false) a floor stretching to the horizon,
/// with or without detail.
fn wall_app(detail: f32, wall: bool, configure: impl FnOnce(&mut RenderSettings)) -> App {
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
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        configure(&mut s);
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(60.0)
        },
        if wall {
            Transform::from_xyz(0.0, 1.6, 4.0).looking_at(DVec3::new(0.0, 1.2, 0.0), Vec3::Y)
        } else {
            Transform::from_xyz(0.0, 1.6, 4.0).looking_at(DVec3::new(0.0, 1.6, 0.0), Vec3::Y)
        },
    ));
    // Low sun grazing the wall from the side: bumps show best in raking light.
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(1.0, -0.5, -0.4), Vec3::Y),
    ));
    let stone = StandardMaterial::hex(0x9c8670)
        .with_roughness(0.85)
        .with_detail(detail, 0.15);
    if wall {
        w.spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [8.0, 4.0, 0.4],
                },
                stone,
            ),
        ));
    }
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            stone.with_detail_in_world(),
        ),
    ));
    app
}

fn render(app: &mut App, frames: usize, name: &str) -> Option<RgbaImage> {
    app.update_n(frames);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

fn luma(img: &RgbaImage, x: u32, y: u32) -> f32 {
    let p = img.get_pixel(x, y);
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// Mean absolute difference between neighboring pixels in a block: how busy the surface looks.
fn busyness(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += (luma(img, x, y) - luma(img, x + 1, y)).abs()
                + (luma(img, x, y) - luma(img, x, y + 1)).abs();
            n += 2.0;
        }
    }
    sum / n
}

/// Standard deviation of luma in a block: how mottled the surface is.
fn spread(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
    let values: Vec<f32> = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .map(|(x, y)| luma(img, x, y))
        .collect();
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
}

#[test]
fn detail_breaks_up_a_plain_wall() {
    let no_aa = |s: &mut RenderSettings| s.anti_aliasing = AntiAliasing::None;
    let (Some(plain), Some(detailed)) = (
        render(&mut wall_app(0.0, true, no_aa), 2, "detail_off"),
        render(&mut wall_app(1.0, true, no_aa), 2, "detail_on"),
    ) else {
        return;
    };
    // The wall fills the middle of the frame.
    let block = (W / 4, H / 4, W * 3 / 4, H / 2);
    let (a, b) = (
        spread(&plain, block.0, block.1, block.2, block.3),
        spread(&detailed, block.0, block.1, block.2, block.3),
    );
    eprintln!("wall luma spread: {a:.2} plain, {b:.2} with detail");
    assert!(b > a + 3.0, "detail doesn't show: spread {b:.2} vs {a:.2}");
}

#[test]
fn distant_detail_does_not_sparkle() {
    // The floor toward the horizon: many detail bumps per pixel. With anti-aliasing the
    // octaves finer than a pixel fade out, so it stays about as calm as a plain floor.
    let (Some(plain), Some(detailed)) = (
        render(&mut wall_app(0.0, false, |_| {}), 12, "detail_far_off"),
        render(&mut wall_app(1.0, false, |_| {}), 12, "detail_far_on"),
    ) else {
        return;
    };
    // A band of floor just below the horizon.
    let band = (W / 4, H / 2 + 2, W * 3 / 4, H / 2 + 12);
    let (a, b) = (
        busyness(&plain, band.0, band.1, band.2, band.3),
        busyness(&detailed, band.0, band.1, band.2, band.3),
    );
    eprintln!("far floor busyness: {a:.2} plain, {b:.2} with detail");
    assert!(b < a + 4.0, "distant detail sparkles: {b:.2} vs {a:.2}");
}
