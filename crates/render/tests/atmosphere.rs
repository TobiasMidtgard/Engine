//! The sky from the atmosphere tables: blue overhead and pale at the horizon by day, red
//! toward a setting sun, a glowing limb from orbit. Needs a GPU or lavapipe; passes with a note
//! when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 192;
const H: u32 = 108;

/// A camera `altitude` meters up, looking level toward -z (or `pitch` degrees up), with the sun
/// `sun_elevation` degrees above the horizon straight ahead. No geometry: just the sky and the
/// planet below.
fn sky_app(altitude: f64, pitch: f64, sun_elevation: f64, ev100: f32) -> App {
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
        s.anti_aliasing = AntiAliasing::None;
    }
    let w = &mut app.world;
    let p = pitch.to_radians();
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100 },
            ..Camera::perspective(90.0)
        },
        Transform::from_xyz(0.0, altitude, 0.0)
            .looking_at(DVec3::new(0.0, altitude + p.tan(), -1.0), Vec3::Y),
    ));
    let e = sun_elevation.to_radians();
    // The light travels away from the sun: down and toward the camera.
    w.spawn((
        DirectionalLight::default(),
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

/// Mean linear RGB of a block of rows `y0..y1` across the middle of the image.
fn mean_rgb(img: &RgbaImage, y0: u32, y1: u32) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for y in y0..y1 {
        for x in (W / 2 - 20)..(W / 2 + 20) {
            let p = img.get_pixel(x, y);
            for c in 0..3 {
                let v = p[c] as f32 / 255.0;
                sum[c] += if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                };
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[test]
fn day_sky_is_blue_overhead_and_pale_at_the_horizon() {
    // Looking 47° up with a 90° field of view: the top rows are near the zenith, the bottom
    // rows just above the horizon. The sun is behind the camera, 60° up.
    let mut app = sky_app(2.0, 47.0, 120.0, 15.0);
    let Some(img) = render(&mut app, "sky_day") else {
        return;
    };
    let high = mean_rgb(&img, 2, 10);
    let low = mean_rgb(&img, H - 10, H - 2);
    eprintln!("day sky: high {high:?}, low {low:?}");
    assert!(
        high[2] > high[0] * 1.8,
        "the sky overhead is not blue: {high:?}"
    );
    assert!(
        luma(low) > luma(high) * 1.2,
        "the horizon is not brighter than the sky above: {low:?} vs {high:?}"
    );
    let sat = |c: [f32; 3]| c[2] / c[0].max(1e-4);
    assert!(
        sat(low) < sat(high),
        "the horizon is not paler than the sky above: {low:?} vs {high:?}"
    );
}

#[test]
fn sunset_sky_is_red_toward_the_sun() {
    // Looking level at a sun 1° above the horizon.
    let mut app = sky_app(2.0, 0.0, 1.0, 13.0);
    let Some(img) = render(&mut app, "sky_sunset") else {
        return;
    };
    // Rows just above the horizon, beside the sun's disc.
    let glow = mean_rgb(&img, H / 2 - 8, H / 2 - 3);
    let high = mean_rgb(&img, 2, 8);
    eprintln!("sunset: horizon {glow:?}, high {high:?}");
    assert!(
        glow[0] > glow[2] * 1.5,
        "the sky around a setting sun is not red: {glow:?}"
    );
    assert!(
        high[2] / high[0].max(1e-4) > glow[2] / glow[0].max(1e-4),
        "the sky overhead should be bluer than the sunset glow: {high:?} vs {glow:?}"
    );
}

#[test]
fn from_orbit_the_limb_glows_above_a_dark_space() {
    // 400 km up, looking 10° down toward the horizon (the limb is ~20° below level).
    let mut app = sky_app(400_000.0, -17.0, 60.0, 13.0);
    let Some(img) = render(&mut app, "sky_orbit") else {
        return;
    };
    let space = mean_rgb(&img, 1, 6);
    let planet = mean_rgb(&img, H - 8, H - 2);
    eprintln!("orbit: space {space:?}, planet {planet:?}");
    assert!(luma(space) < 0.01, "space is not dark: {space:?}");
    assert!(luma(planet) > 0.02, "the planet is not lit: {planet:?}");
}

#[test]
fn from_orbit_haze_over_the_night_side_stays_dark() {
    // 400 km up, looking straight down at a sea where the sun set 15° ago; the camera itself
    // still sees the sun (the horizon is 20° below level up there). Exposed for a night side
    // (EV -1, as night photos from orbit are). Haze over the sea must not scatter the sunlight
    // or the sunlit sky that reach the camera, which are not the haze's. Lit at the camera, the
    // haze glowed about ten times brighter than this allows.
    let mut app = sky_app(400_000.0, -89.0, -15.0, -1.0);
    app.world.resource_mut::<FogSettings>().density = 0.00002;
    // A dark sea under the camera, so the haze is applied to a surface as it is over terrain.
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2_000_000.0, 2_000_000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.02, 0.04, 0.06),
        ),
    ));
    let Some(img) = render(&mut app, "sky_orbit_night") else {
        return;
    };
    let sea = mean_rgb(&img, H / 2 - 10, H / 2 + 10);
    eprintln!("orbit night side: {sea:?}");
    assert!(luma(sea) < 0.05, "the night side glows: {sea:?}");
}
