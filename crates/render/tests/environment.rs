//! Sky light from the filtered environment map: rough metal reflects a blurred sky (still sky
//! above and ground below), and without a planet the ground below the horizon still lights
//! things. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;
/// The two spheres: left and right of the camera's view, 1 m radius, 1.5 m up.
const LEFT: DVec3 = DVec3::new(-1.3, 1.5, 0.0);
const RIGHT: DVec3 = DVec3::new(1.3, 1.5, 0.0);

/// Two spheres seen from 4 m with nothing else around: just the sky and the planet's ground
/// far below. The sun is low behind the spheres, so the sides facing the camera reflect only
/// the sky. Tone curve off so pixel values follow radiance.
fn spheres_app(left: StandardMaterial, right: StandardMaterial, atmosphere: bool) -> App {
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
        s.anti_aliasing = AntiAliasing::None;
    }
    app.world.resource_mut::<Atmosphere>().enabled = atmosphere;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.5, 4.0).looking_at(DVec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        // 12 degrees up, light traveling toward the camera.
        Transform::default().looking_at(DVec3::new(0.0, -0.208, 0.978), Vec3::Y),
    ));
    for (center, material) in [(LEFT, left), (RIGHT, right)] {
        w.spawn((
            Transform::from_translation(center),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 1.0,
                    segments: 64,
                },
                material,
            ),
        ));
    }
    app
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Where world point `p` lands on screen, in pixels.
fn screen(app: &App, p: DVec3) -> (i32, i32) {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    )
}

fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear RGB of the pixels within a 5-pixel radius around where `p` lands.
fn patch(app: &App, img: &RgbaImage, p: DVec3) -> Vec<[f32; 3]> {
    let (cx, cy) = screen(app, p);
    let mut out = Vec::new();
    for y in (cy - 5)..=(cy + 5) {
        for x in (cx - 5)..=(cx + 5) {
            if (x - cx).pow(2) + (y - cy).pow(2) > 25 {
                continue;
            }
            let p = img.get_pixel(
                x.clamp(0, W as i32 - 1) as u32,
                y.clamp(0, H as i32 - 1) as u32,
            );
            out.push([linear(p[0]), linear(p[1]), linear(p[2])]);
        }
    }
    out
}

fn mean(c: &[[f32; 3]]) -> [f32; 3] {
    let mut s = [0.0; 3];
    for p in c {
        for i in 0..3 {
            s[i] += p[i] / c.len() as f32;
        }
    }
    s
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Luma of every pixel of the sphere at `center`, inside 80% of its radius.
fn sphere_lumas(app: &App, img: &RgbaImage, center: DVec3) -> Vec<f32> {
    let (cx, cy) = screen(app, center);
    let (ex, _) = screen(app, center + DVec3::X);
    let r = ((ex - cx) as f32 * 0.8) as i32;
    let mut out = Vec::new();
    for y in (cy - r)..=(cy + r) {
        for x in (cx - r)..=(cx + r) {
            if (x - cx).pow(2) + (y - cy).pow(2) <= r * r {
                let p = img.get_pixel(x as u32, y as u32);
                out.push(luma([linear(p[0]), linear(p[1]), linear(p[2])]));
            }
        }
    }
    out
}

/// How sharp the reflection of the horizon is on the sphere at `center`: the steepest relative
/// step between vertical neighbors down the middle of the sphere.
fn horizon_sharpness(app: &App, img: &RgbaImage, center: DVec3) -> f32 {
    let (cx, cy) = screen(app, center);
    let (ex, _) = screen(app, center + DVec3::X);
    let r = ((ex - cx) as f32 * 0.8) as i32;
    let l = |x: i32, y: i32| {
        let p = img.get_pixel(x as u32, y as u32);
        luma([linear(p[0]), linear(p[1]), linear(p[2])])
    };
    let mut sum = 0.0;
    for x in (cx - 3)..=(cx + 3) {
        let steepest = ((cy - r)..(cy + r))
            .map(|y| (l(x, y) - l(x, y + 1)).abs() / (l(x, y) + l(x, y + 1) + 1e-3))
            .fold(0.0f32, f32::max);
        sum += steepest / 7.0;
    }
    sum
}

#[test]
fn rough_metal_reflects_a_blurred_sky() {
    let chrome = StandardMaterial::color(0.9, 0.9, 0.9).with_metallic(1.0);
    let mut app = spheres_app(
        chrome.with_roughness(0.05),
        chrome.with_roughness(0.7),
        true,
    );
    let Some(img) = render(&mut app, "env_rough_metal") else {
        return;
    };
    let (sharp, rough) = (
        horizon_sharpness(&app, &img, LEFT),
        horizon_sharpness(&app, &img, RIGHT),
    );
    let mean_of = |c: DVec3| {
        let v = sphere_lumas(&app, &img, c);
        v.iter().sum::<f32>() / v.len() as f32
    };
    let (sharp_mean, rough_mean) = (mean_of(LEFT), mean_of(RIGHT));
    eprintln!(
        "horizon sharpness {sharp:.3} smooth, {rough:.3} rough; mean luma {sharp_mean:.3} smooth, {rough_mean:.3} rough"
    );
    assert!(
        rough < sharp * 0.6,
        "the rough sphere's reflection isn't blurred: {rough:.4} vs {sharp:.4}"
    );
    // Blurring spreads the reflected light; it neither adds nor loses much of it.
    assert!(
        rough_mean > sharp_mean * 0.6 && rough_mean < sharp_mean * 1.5,
        "rough reflection brightness {rough_mean:.3} far from smooth {sharp_mean:.3}"
    );
}

#[test]
fn rough_metal_reflects_sky_above_and_ground_below() {
    // A fully rough metal sphere: however blurred, its top still reflects the blue sky and its
    // bottom the ground.
    let metal = StandardMaterial::color(0.9, 0.9, 0.9)
        .with_metallic(1.0)
        .with_roughness(1.0);
    let mut app = spheres_app(metal, metal, true);
    let Some(img) = render(&mut app, "env_rough_poles") else {
        return;
    };
    let top = mean(&patch(&app, &img, LEFT + DVec3::new(0.0, 0.8, 0.3)));
    let bottom = mean(&patch(&app, &img, LEFT + DVec3::new(0.0, -0.8, 0.3)));
    eprintln!("rough metal: top {top:?}, bottom {bottom:?}");
    assert!(
        top[2] > top[0] * 1.3,
        "the top doesn't reflect blue sky: {top:?}"
    );
    let blue = |c: [f32; 3]| c[2] / c[0].max(1e-4);
    assert!(
        blue(top) > blue(bottom) * 1.2,
        "sky and ground mixed: top {top:?}, bottom {bottom:?}"
    );
}

#[test]
fn without_a_planet_the_ground_below_still_lights_things() {
    // No atmosphere: a black sky, the sun, and a flat ground below the horizon. A matte sphere's
    // underside, in the sun's shadow, is lit only by light bounced off that ground.
    let matte = StandardMaterial::color(0.8, 0.8, 0.8).with_roughness(0.9);
    let mut app = spheres_app(matte, matte, false);
    let Some(img) = render(&mut app, "env_no_planet") else {
        return;
    };
    let bottom = mean(&patch(&app, &img, LEFT + DVec3::new(0.0, -0.85, 0.5)));
    eprintln!("no planet: underside {bottom:?}");
    assert!(
        luma(bottom) > 0.01,
        "the underside gets no light from the ground: {bottom:?}"
    );
}
