//! The sky's hooks for a weather module: its flat cloud layer can be turned off
//! (`RenderSettings::sky_cloud_layer`), and night follows the main light's illuminance, so a
//! moon that is the brightest light still shows the stars. Needs a GPU or lavapipe; passes with
//! a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WeatherState;
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A camera on the ground looking toward -z, `pitch` degrees up, with a `fov` degree view; one
/// directional light of `lux` at `elevation` degrees straight ahead (negative: below the
/// horizon; past 90°: over the camera and behind it). Tone curve off so pixel values follow
/// radiance.
fn sky_app(pitch: f64, fov: f32, elevation: f64, lux: f32, ev100: f32) -> App {
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
    let w = &mut app.world;
    let p = pitch.to_radians();
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100 },
            ..Camera::perspective(fov)
        },
        Transform::from_xyz(0.0, 2.0, 0.0)
            .looking_at(DVec3::new(0.0, 2.0 + p.tan(), -1.0), Vec3::Y),
    ));
    let e = elevation.to_radians();
    // The light travels away from its source: down (or up) and toward the camera.
    w.spawn((
        DirectionalLight {
            illuminance: lux,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(0.0, -e.sin(), e.cos()), Vec3::Y),
    ));
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

/// Mean absolute difference of two images, per channel (0..255).
fn mean_difference(a: &RgbaImage, b: &RgbaImage) -> f32 {
    let mut sum = 0.0;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        for c in 0..3 {
            sum += (pa[c] as f32 - pb[c] as f32).abs();
        }
    }
    sum / (a.width() * a.height() * 3) as f32
}

/// A sunny sky looking 30° up past a white ball, under `cover` of the sky's own cloud layer.
fn cloudy(cover: f32, cloud_layer: bool, name: &str) -> Option<RgbaImage> {
    let mut app = sky_app(15.0, 70.0, 40.0, 110_000.0, 14.5);
    app.world.insert_resource(WeatherState {
        cloud_cover: cover,
        ..Default::default()
    });
    app.world.resource_mut::<RenderSettings>().sky_cloud_layer = cloud_layer;
    // Lit by the sun from above and by the sky (environment map) on the side facing the camera.
    app.world.spawn((
        Transform::from_xyz(0.0, 2.0, -6.0),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 1.0,
                segments: 48,
            },
            StandardMaterial::color(0.8, 0.8, 0.8).with_roughness(0.9),
        ),
    ));
    render(&mut app, name)
}

#[test]
fn turning_the_cloud_layer_off_clears_the_sky_and_its_light() {
    let Some(clear) = cloudy(0.0, true, "clouds_none") else {
        return;
    };
    let on = cloudy(0.9, true, "clouds_layer_on").unwrap();
    let off = cloudy(0.9, false, "clouds_layer_off").unwrap();
    let (with_layer, without_layer) = (mean_difference(&on, &clear), mean_difference(&off, &clear));
    eprintln!("cloud layer: on differs from a clear sky by {with_layer}, off by {without_layer}");
    assert!(
        with_layer > 4.0,
        "the cloud layer should show under 90% cover ({with_layer})"
    );
    // Off, both the sky and the light it casts (environment map) are as without clouds.
    assert!(
        without_layer < 0.5,
        "with the layer off the sky should look clear ({without_layer})"
    );
}

/// Pixels at least `contrast` (luma, 0..255) brighter than the sky two pixels around them
/// (stars), in a view of the sky.
fn stars(img: &RgbaImage, contrast: f32) -> usize {
    let luma = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
    };
    let mut n = 0;
    for y in 2..img.height() - 2 {
        for x in 2..img.width() - 2 {
            let around = [
                luma(x - 2, y),
                luma(x + 2, y),
                luma(x, y - 2),
                luma(x, y + 2),
            ]
            .into_iter()
            .fold(f32::MAX, f32::min);
            if luma(x, y) > around + contrast {
                n += 1;
            }
        }
    }
    n
}

/// A 24° view 60° up of a sky lit by one light of `lux` `elevation` degrees up behind the
/// camera (out of view), at 960x540: star cells are three pixels wide, and there are a few
/// hundred of them.
fn starry_sky(lux: f32, elevation: f64, ev100: f32, name: &str) -> Option<RgbaImage> {
    let mut app = sky_app(60.0, 24.0, 180.0 - elevation, lux, ev100);
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 960;
        w.height = 540;
    }
    render(&mut app, name)
}

#[test]
fn a_moon_overhead_still_shows_the_stars() {
    // A full moon is the only (so the brightest) light.
    let Some(moonlit) = starry_sky(0.3, 50.0, -3.0, "night_moon_overhead") else {
        return;
    };
    let n = stars(&moonlit, 10.0);
    eprintln!("stars under a moon overhead: {n}");
    assert!(
        n >= 10,
        "a moonlit sky is night: its stars should show ({n} star pixels)"
    );
    // The same light at a sun's illuminance is day: no stars.
    let day = starry_sky(110_000.0, 50.0, 14.5, "night_sun_overhead").unwrap();
    let n = stars(&day, 10.0);
    eprintln!("stars under a sun overhead: {n}");
    assert_eq!(n, 0, "a sunny sky has no stars");
}

#[test]
fn a_sun_dimmed_by_a_storm_deck_is_still_day() {
    // Under a full storm deck the weather dims the sun to about 12 lux at noon (the deck lights
    // the ground through the environment map). It is brighter than any moon: no stars. Exposed
    // for that dim sky, where stars at night strength stand out by several levels.
    let Some(storm) = starry_sky(12.0, 60.0, -2.0, "night_storm_noon") else {
        return;
    };
    let n = stars(&storm, 4.0);
    eprintln!("stars under a storm-dimmed sun at noon: {n}");
    assert_eq!(n, 0, "a storm at noon is day: no stars ({n} star pixels)");
}

/// Mean luma of the 9x9 pixels around (`cx`, `cy`) (0..255).
fn luma_around(img: &RgbaImage, cx: u32, cy: u32) -> f32 {
    let mut sum = 0.0;
    for y in cy - 4..=cy + 4 {
        for x in cx - 4..=cx + 4 {
            let p = img.get_pixel(x, y);
            sum += 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        }
    }
    sum / 81.0
}

#[test]
fn the_stand_in_moon_shows_only_opposite_a_sun() {
    // The light 25° below the horizon behind the camera, which looks 25° up with a 10° view:
    // right where a stand-in moon glows opposite it.
    let Some(sun_set) = render(
        &mut sky_app(25.0, 10.0, 205.0, 110_000.0, -4.0),
        "stand_in_moon_sun",
    ) else {
        return;
    };
    let moon_set = render(
        &mut sky_app(25.0, 10.0, 205.0, 0.3, -4.0),
        "stand_in_moon_moon",
    )
    .unwrap();
    // The middle of the view against the night sky in its corner.
    let glow = |img: &RgbaImage| luma_around(img, W / 2, H / 2) - luma_around(img, 10, 10);
    let (with_sun, with_moon) = (glow(&sun_set), glow(&moon_set));
    eprintln!(
        "stand-in moon over the night sky: opposite a set sun {with_sun}, a set moon {with_moon}"
    );
    assert!(
        with_sun > 100.0,
        "a sun below the horizon keeps its stand-in moon ({with_sun})"
    );
    assert!(
        with_moon.abs() < 5.0,
        "when the moon is the light there is no second, stand-in moon ({with_moon})"
    );
}
