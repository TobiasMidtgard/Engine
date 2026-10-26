//! Viewports: a camera with a `Viewport` draws only into its rectangle of the window, with the
//! rectangle's aspect ratio, and the scene there looks as it does filling a window of that
//! size. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// Render a ball on a floor into a `window`-sized window, the camera drawing into `viewport`
/// (all of the window without one): the shot's pixels.
fn render(window: UVec2, viewport: Option<Viewport>, shot: &str) -> Option<image::RgbImage> {
    shoot(window, viewport, 1.0, shot)
}

/// The same scene filling the window at a render scale.
fn render_scaled(window: UVec2, scale: f32, shot: &str) -> Option<image::RgbImage> {
    shoot(window, None, scale, shot)
}

fn shoot(
    window: UVec2,
    viewport: Option<Viewport>,
    scale: f32,
    shot: &str,
) -> Option<image::RgbImage> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = window.x;
        w.height = window.y;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.render_scale = scale;
    }
    let w = &mut app.world;
    let camera = w
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.0 },
                ..Camera::perspective(60.0)
            },
            Transform::from_xyz(0.0, 1.5, 4.0).looking_at(DVec3::new(0.0, 0.8, 0.0), Vec3::Y),
        ))
        .id();
    if let Some(v) = viewport {
        w.entity_mut(camera).insert(v);
    }
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.4, -0.8, -0.45), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.35, 0.4, 0.3),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.8, 0.0),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.8,
                segments: 48,
            },
            StandardMaterial::color(0.8, 0.25, 0.2),
        ),
    ));
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-viewport");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    Some(image::open(&path).expect("read the shot").to_rgb8())
}

#[test]
fn a_camera_draws_only_into_its_viewport_as_into_a_window_that_size() {
    let viewport = Viewport::new(UVec2::new(200, 30), UVec2::new(160, 120));
    let Some(window) = render(UVec2::new(400, 180), Some(viewport), "viewport_window.png") else {
        return;
    };
    let Some(alone) = render(viewport.size, None, "viewport_alone.png") else {
        return;
    };
    assert_eq!(
        window.dimensions(),
        (400, 180),
        "the shot covers the window"
    );
    // Outside the rectangle: black.
    let lit_outside = window
        .enumerate_pixels()
        .filter(|(x, y, _)| {
            let inside = (200..360).contains(x) && (30..150).contains(y);
            !inside
        })
        .filter(|(_, _, p)| p.0.iter().any(|&c| c > 0))
        .count();
    assert_eq!(
        lit_outside, 0,
        "{lit_outside} pixels lit outside the viewport"
    );
    // Inside: the same image as a window of the rectangle's size (same aspect ratio).
    let mut diff = 0u64;
    for (x, y, p) in alone.enumerate_pixels() {
        let q = window.get_pixel(200 + x, 30 + y);
        diff +=
            p.0.iter()
                .zip(q.0)
                .map(|(a, b)| a.abs_diff(b) as u64)
                .sum::<u64>();
    }
    let mean = diff as f64 / (160.0 * 120.0 * 3.0);
    eprintln!("viewport vs. window of its size: mean difference {mean:.2} of 255");
    assert!(
        mean < 1.5,
        "the viewport's image differs by {mean:.2} of 255"
    );
}

#[test]
fn a_viewport_is_kept_inside_the_window() {
    let v = Viewport::new(UVec2::new(300, 100), UVec2::new(500, 500));
    assert_eq!(
        v.within(UVec2::new(400, 300)),
        (UVec2::new(300, 100), UVec2::new(100, 200))
    );
    assert_eq!(
        Viewport::default().within(UVec2::new(640, 360)),
        (UVec2::ZERO, UVec2::new(640, 360))
    );
    let off = Viewport::new(UVec2::new(900, 900), UVec2::new(10, 10));
    assert_eq!(
        off.within(UVec2::new(400, 300)),
        (UVec2::new(399, 299), UVec2::ONE)
    );
}

/// A render scale under 1 draws the scene smaller and upscales it into the viewport: the
/// output keeps the window's size and looks like the full-resolution image, softer.
#[test]
fn a_lower_render_scale_upscales_to_the_viewport() {
    let window = UVec2::new(320, 180);
    let Some(full) = render(window, None, "render_scale_full.png") else {
        return;
    };
    let Some(half) = render_scaled(window, 0.5, "render_scale_half.png") else {
        return;
    };
    assert_eq!(half.dimensions(), (320, 180), "the shot covers the window");
    let diff = full
        .as_raw()
        .iter()
        .zip(half.as_raw())
        .map(|(a, b)| a.abs_diff(*b) as u64)
        .sum::<u64>() as f64
        / full.as_raw().len() as f64;
    eprintln!("half resolution vs. full: mean difference {diff:.2} of 255");
    assert!(diff < 4.0, "the upscaled image differs by {diff:.2} of 255");
}
