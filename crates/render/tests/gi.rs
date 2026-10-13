//! Screen-space global illumination: a sunlit red wall bounces red light onto the white floor in
//! front of it, and an emissive panel lights the dark floor below it. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

fn base_app(configure: impl FnOnce(&mut RenderSettings)) -> App {
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
    app
}

fn floor(w: &mut World, albedo: f32) {
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [60.0, 60.0],
                subdivisions: 1,
            },
            StandardMaterial::color(albedo, albedo, albedo).with_roughness(0.9),
        ),
    ));
}

/// A white floor with a red wall across it, lit by the sun from behind the camera.
fn red_wall_app(configure: impl FnOnce(&mut RenderSettings)) -> App {
    let mut app = base_app(configure);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 15.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.6, 4.5).looking_at(DVec3::new(0.0, 0.4, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.35, -0.55, -0.75), Vec3::Y),
    ));
    floor(w, 0.8);
    w.spawn((
        Transform::from_xyz(0.0, 1.25, -0.6),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [6.0, 2.5, 0.2],
            },
            StandardMaterial::color(0.85, 0.04, 0.03).with_roughness(0.8),
        ),
    ));
    app
}

/// A dark floor under a large glowing panel, at night (no sun, no sky light to speak of).
fn glowing_panel_app(configure: impl FnOnce(&mut RenderSettings)) -> App {
    let mut app = base_app(configure);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 6.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 2.2, 4.0).looking_at(DVec3::new(0.0, 0.3, 0.0), Vec3::Y),
    ));
    floor(w, 0.5);
    // A panel standing up, facing the camera and the floor in front of it.
    w.spawn((
        Transform::from_xyz(0.0, 1.2, -0.5),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.0, 2.0, 0.1],
            },
            StandardMaterial::color(0.0, 0.0, 0.0).with_emissive([1.0, 0.6, 0.2], 400.0),
        ),
    ));
    app
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    // GI builds up over frames.
    app.update_n(24);
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
fn a_sunlit_red_wall_tints_the_floor_in_front_of_it() {
    let mut on = red_wall_app(|_| {});
    let mut off = red_wall_app(|s| s.screen_space_gi = false);
    let (Some(with_gi), Some(without)) = (render(&mut on, "gi_on"), render(&mut off, "gi_off"))
    else {
        return;
    };
    let spot = DVec3::new(0.3, 0.0, -0.2);
    let a = color_at(&on, &with_gi, spot);
    let b = color_at(&off, &without, spot);
    eprintln!("floor at the wall's foot: {a:?} with GI, {b:?} without");
    let warmth = |c: [f32; 3]| c[0] / c[2].max(1.0);
    assert!(
        warmth(a) > warmth(b) * 1.1,
        "the red wall doesn't tint the floor: {a:?} with GI, {b:?} without"
    );
    // Far from the wall the floor looks the same either way.
    let far = DVec3::new(0.0, 0.0, 3.0);
    let (fa, fb) = (color_at(&on, &with_gi, far), color_at(&off, &without, far));
    eprintln!("floor far from the wall: {fa:?} with GI, {fb:?} without");
    assert!(
        (warmth(fa) - warmth(fb)).abs() < 0.1,
        "GI tints the floor far from the wall: {fa:?} vs {fb:?}"
    );
}

#[test]
fn a_glowing_panel_lights_the_floor_below_it() {
    let mut on = glowing_panel_app(|_| {});
    let mut off = glowing_panel_app(|s| s.screen_space_gi = false);
    let (Some(with_gi), Some(without)) = (
        render(&mut on, "gi_panel_on"),
        render(&mut off, "gi_panel_off"),
    ) else {
        return;
    };
    let spot = DVec3::new(0.0, 0.0, 0.3);
    let a = color_at(&on, &with_gi, spot);
    let b = color_at(&off, &without, spot);
    eprintln!("floor below the panel: {a:?} with GI, {b:?} without");
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    assert!(
        luma(a) > luma(b) + 20.0,
        "the glowing panel doesn't light the floor: {a:?} with GI, {b:?} without"
    );
}
