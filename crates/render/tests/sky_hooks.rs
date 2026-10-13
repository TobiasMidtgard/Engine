//! The sky's hooks for a weather module: its flat cloud layer can be turned off
//! (`RenderSettings::sky_cloud_layer`), and night follows the main light's illuminance, so a
//! moon that is the brightest light keeps the stars out. Needs a GPU or lavapipe; passes with
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
/// horizon). Tone curve off so pixel values follow radiance.
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
