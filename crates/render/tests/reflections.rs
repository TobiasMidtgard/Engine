//! Screen-space reflections: a polished floor mirrors the objects standing on it, and turning
//! them off falls back to the sky. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;
/// A point on the floor between the camera and the red block, where the block's mirror image
/// appears.
const FLOOR_SPOT: DVec3 = DVec3::new(0.0, 0.0, 1.4);

/// A red block on a polished dark floor, seen from 6 m at a low angle, sun behind the camera.
fn floor_app(configure: impl FnOnce(&mut RenderSettings)) -> App {
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
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.sharpness = 0.0;
        configure(&mut s);
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.2, 6.0).looking_at(DVec3::new(0.0, 0.8, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.2, -0.7, -0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [60.0, 60.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.05, 0.05, 0.05)
                .with_metallic(1.0)
                .with_roughness(0.05),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [2.0, 2.0, 2.0],
            },
            StandardMaterial::color(0.8, 0.05, 0.03).with_roughness(0.6),
        ),
    ));
    app
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(8);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean sRGB color of a 5x5 block around where world point `p` lands.
fn color_at(app: &App, img: &RgbaImage, p: DVec3) -> [f32; 3] {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = ((ndc.x * 0.5 + 0.5) * W as f32) as i32;
    let y = ((0.5 - ndc.y * 0.5) * H as f32) as i32;
    let mut sum = [0.0; 3];
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            for c in 0..3 {
                sum[c] += px[c] as f32 / 25.0;
            }
        }
    }
    sum
}

#[test]
fn polished_floor_mirrors_the_block_standing_on_it() {
    let mut on = floor_app(|_| {});
    let mut off = floor_app(|s| s.screen_space_reflections = false);
    let (Some(with_ssr), Some(without)) = (render(&mut on, "ssr_on"), render(&mut off, "ssr_off"))
    else {
        return;
    };
    let a = color_at(&on, &with_ssr, FLOOR_SPOT);
    let b = color_at(&off, &without, FLOOR_SPOT);
    eprintln!("floor in front of the block: {a:?} with reflections, {b:?} without");
    // With reflections the floor shows the red block; without, the (blue) sky.
    assert!(
        a[0] > a[2] * 1.5,
        "the floor doesn't mirror the red block: {a:?}"
    );
    assert!(
        b[2] > b[0],
        "without reflections the floor should reflect the sky: {b:?}"
    );
}

#[test]
fn reflections_work_without_temporal_anti_aliasing() {
    let mut app = floor_app(|s| s.anti_aliasing = AntiAliasing::None);
    let Some(img) = render(&mut app, "ssr_no_taa") else {
        return;
    };
    let a = color_at(&app, &img, FLOOR_SPOT);
    eprintln!("floor in front of the block without TAA: {a:?}");
    assert!(
        a[0] > a[2] * 1.5,
        "the floor doesn't mirror the red block: {a:?}"
    );
}
