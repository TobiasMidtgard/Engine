//! Distant hills under an overcast sky fade toward the grey of the air, not toward tan: the
//! clouds dim the sun, so the air is lit by the deck's grey light, and it scatters that back in
//! as much as its (bluish) extinction takes out. The deck's light here is a grey ambient fill
//! that doesn't dim with the sun, as the weather's clouds light the scene through the
//! environment map. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::WeatherState;
use image::RgbaImage;

const W: u32 = 240;
const H: u32 = 135;
/// The far ridge, 9 km away, and a near one of the same grey, 300 m away.
const FAR: DVec3 = DVec3::new(0.0, 450.0, -9000.0);
const NEAR: DVec3 = DVec3::new(-60.0, 15.0, -300.0);

/// The two ridges under `cover` cloud cover, with the sun dimmed by `dimming` as the weather
/// dims it under a deck and `deck` cd/m² of grey light from all around. Tone curve off so pixel
/// values follow radiance.
fn render(cover: f32, dimming: f32, deck: f32, name: &str) -> Option<(App, RgbaImage)> {
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
        s.white_balance = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.volumetric_fog = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world.insert_resource(WeatherState {
        cloud_cover: cover,
        ..Default::default()
    });
    if deck > 0.0 {
        app.world.insert_resource(AmbientLight {
            color: [1.0, 1.0, 1.0],
            brightness: deck,
        });
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Auto {
                min_ev100: -3.0,
                max_ev100: 18.0,
                speed: 50.0,
                compensation: 0.0,
            },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 5.0, 0.0).looking_at(DVec3::new(0.0, 60.0, -1000.0), Vec3::Y),
    ));
    let to_sun = DVec3::new(0.5, 0.7, 0.5).normalize();
    w.spawn((
        DirectionalLight {
            illuminance: 110_000.0 * dimming,
            ..Default::default()
        },
        Transform::default().looking_at(-to_sun, Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [30_000.0, 30_000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3).with_roughness(0.95),
        ),
    ));
    for (at, size) in [(FAR, [12_000.0, 900.0, 400.0]), (NEAR, [60.0, 30.0, 20.0])] {
        w.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(
                Shape::Cuboid { size },
                StandardMaterial::color(0.3, 0.3, 0.3).with_roughness(0.95),
            ),
        ));
    }
    app.update_n(12);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    Some((app, img))
}

/// Mean linear color of a 7x7 block around where world point `p` lands.
fn color_at(app: &App, img: &RgbaImage, p: DVec3) -> Vec3 {
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
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut sum = Vec3::ZERO;
    for dy in -3..=3 {
        for dx in -3..=3 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += Vec3::new(lin(px[0]), lin(px[1]), lin(px[2])) / 49.0;
        }
    }
    sum
}

/// Blue over red: below 1 is warm (tan), above 1 is cool.
fn coolness(c: Vec3) -> f32 {
    c.z / c.x.max(1e-4)
}

#[test]
fn far_hills_under_an_overcast_sky_stay_grey() {
    let Some((app, img)) = render(1.0, 0.0005, 30.0, "overcast_haze") else {
        return;
    };
    let near = color_at(&app, &img, NEAR + DVec3::new(0.0, 0.0, 10.0));
    let far = color_at(&app, &img, FAR + DVec3::new(0.0, -100.0, 200.0));
    eprintln!(
        "overcast: near ridge {near} (blue/red {:.3}), far ridge {far} (blue/red {:.3})",
        coolness(near),
        coolness(far)
    );
    assert!(
        coolness(far) > coolness(near) - 0.05,
        "the far ridge turns tan under the overcast: blue/red {:.3} far vs {:.3} near",
        coolness(far),
        coolness(near)
    );
}

#[test]
fn far_hills_under_a_clear_sky_still_turn_blue() {
    let Some((app, img)) = render(0.25, 1.0, 0.0, "clear_haze") else {
        return;
    };
    let near = color_at(&app, &img, NEAR + DVec3::new(0.0, 0.0, 10.0));
    let far = color_at(&app, &img, FAR + DVec3::new(0.0, -100.0, 200.0));
    eprintln!(
        "clear: near ridge {near} (blue/red {:.3}), far ridge {far} (blue/red {:.3})",
        coolness(near),
        coolness(far)
    );
    assert!(
        coolness(far) > coolness(near) + 0.1,
        "the far ridge should be bluer than the near one under a clear sky: {:.3} vs {:.3}",
        coolness(far),
        coolness(near)
    );
}
