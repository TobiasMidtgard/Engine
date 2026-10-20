//! Per-planet air: Mars' butterscotch sky by day and blue glow around the setting sun, from the
//! scattering coefficients on `Atmosphere`; Earth unchanged; the atmosphere tables follow a
//! change of air. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 192;
const H: u32 = 108;
/// Vertical field of view, degrees.
const FOV: f64 = 90.0;

/// A camera 2 m up on `air`'s planet, looking `pitch` degrees up toward -z, with a white sun
/// `sun_elevation` degrees above the horizon and `sun_azimuth` degrees to the left of the view
/// (180 = behind the camera). No geometry: just the sky and the planet below.
fn sky_app(air: Atmosphere, pitch: f64, sun_elevation: f64, sun_azimuth: f64, ev100: f32) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    // Frames a fixed time apart, so two apps rendering the same sky give the same pixels (the
    // output dither follows the clock).
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
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
        s.sky_cloud_layer = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    *app.world.resource_mut::<Atmosphere>() = air;
    let w = &mut app.world;
    let p = pitch.to_radians();
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100 },
            ..Camera::perspective(FOV as f32)
        },
        Transform::from_xyz(0.0, 2.0, 0.0)
            .looking_at(DVec3::new(0.0, 2.0 + p.tan(), -1.0), Vec3::Y),
    ));
    let (e, a) = (sun_elevation.to_radians(), sun_azimuth.to_radians());
    let toward_sun = DVec3::new(-a.sin() * e.cos(), e.sin(), -a.cos() * e.cos());
    // The light travels away from the sun.
    w.spawn((
        DirectionalLight {
            color: [1.0, 1.0, 1.0],
            ..Default::default()
        },
        Transform::default().looking_at(-toward_sun, Vec3::Y),
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

fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Mean linear RGB of the pixels `x0..x1`, `y0..y1`.
fn mean_rgb(img: &RgbaImage, (x0, x1): (u32, u32), (y0, y1): (u32, u32)) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y);
            for (c, s) in sum.iter_mut().enumerate() {
                *s += linear(p[c]);
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

/// The image row `degrees` above the view's center (positive up).
fn row_at(degrees: f64) -> u32 {
    let half = (FOV * 0.5).to_radians().tan();
    let y = 0.5 - degrees.to_radians().tan() / half * 0.5;
    (y * H as f64).round().clamp(0.0, H as f64 - 1.0) as u32
}

/// Blue over red: how blue a color is.
fn blueness(c: [f32; 3]) -> f32 {
    c[2] / c[0].max(1e-5)
}

#[test]
fn mars_midday_sky_is_butterscotch() {
    // Looking 37° up, away from a sun 60° up behind the camera: rows 30–45° up.
    let mut app = sky_app(Atmosphere::mars(), 37.0, 60.0, 180.0, 13.0);
    let Some(img) = render(&mut app, "mars_midday") else {
        return;
    };
    let sky = mean_rgb(&img, (W / 2 - 30, W / 2 + 30), (row_at(8.0), row_at(-7.0)));
    eprintln!(
        "mars midday sky 30-45 deg up: {sky:?}, blue/red {}",
        blueness(sky)
    );
    assert!(sky[0] > 0.02, "the sky is dark: {sky:?}");
    assert!(
        sky[0] > sky[1] && sky[1] > sky[2],
        "the sky is not butterscotch (red > green > blue): {sky:?}"
    );
    assert!(
        blueness(sky) < 0.7,
        "the sky is not clearly warm (blue/red {}): {sky:?}",
        blueness(sky)
    );
}

#[test]
fn mars_sunset_glows_blue_around_the_sun() {
    // A sun 2° up: the sky 4–8° up within 5° to either side of it (2–8° from the sun), against
    // the sky at the same height 90° to the side. The glow is far brighter than the sky away
    // from it, so it gets a darker exposure (blue over red doesn't depend on exposure).
    let band = (row_at(8.0), row_at(4.0));
    let mut toward = sky_app(Atmosphere::mars(), 0.0, 2.0, 0.0, 16.5);
    let Some(toward) = render(&mut toward, "mars_sunset_toward_sun") else {
        return;
    };
    let mut side = sky_app(Atmosphere::mars(), 0.0, 2.0, 90.0, 12.5);
    let Some(side) = render(&mut side, "mars_sunset_side") else {
        return;
    };
    let near = mean_rgb(&toward, (W / 2 - 6, W / 2 + 6), band);
    let away = mean_rgb(&side, (W / 2 - 6, W / 2 + 6), band);
    eprintln!(
        "mars sunset: near the sun {near:?} (blue/red {}), 90° away {away:?} (blue/red {})",
        blueness(near),
        blueness(away)
    );
    assert!(
        near.iter().all(|&c| c < 0.95) && away[0] > 0.01,
        "the exposure doesn't fit the sky: {near:?}, {away:?}"
    );
    assert!(
        blueness(near) > blueness(away) * 1.5,
        "the sky near the setting sun is not bluer than the sky 90° away: {near:?} vs {away:?}"
    );
    assert!(
        near[2] > near[0],
        "the glow around the setting sun is not blue: {near:?}"
    );
}

#[test]
fn earth_preset_is_the_default_air() {
    assert_eq!(Atmosphere::earth(), Atmosphere::default());
    let mut default = sky_app(Atmosphere::default(), 20.0, 30.0, 40.0, 14.0);
    let Some(default) = render(&mut default, "earth_default") else {
        return;
    };
    let mut earth = sky_app(Atmosphere::earth(), 20.0, 30.0, 40.0, 14.0);
    let Some(earth) = render(&mut earth, "earth_preset") else {
        return;
    };
    assert!(
        default.as_raw() == earth.as_raw(),
        "Atmosphere::earth() renders differently from the default"
    );
    // And it is still Earth's blue sky.
    let sky = mean_rgb(&earth, (W / 2 - 30, W / 2 + 30), (2, 12));
    assert!(sky[2] > sky[0] * 1.5, "Earth's sky is not blue: {sky:?}");
}

#[test]
fn changing_the_air_rebuilds_the_tables() {
    // The same planet, radius, heights and ground with Earth's air, then Mars': only the
    // scattering coefficients change, and the tables must follow them.
    let mars = Atmosphere::mars();
    let earthlike = Atmosphere {
        rayleigh_scattering: Atmosphere::earth().rayleigh_scattering,
        mie_scattering: Atmosphere::earth().mie_scattering,
        mie_absorption: Atmosphere::earth().mie_absorption,
        ozone_absorption: Atmosphere::earth().ozone_absorption,
        ..mars
    };
    let mut switched = sky_app(earthlike, 20.0, 12.0, 30.0, 12.0);
    let Some(_) = render(&mut switched, "air_before_switch") else {
        return;
    };
    *switched.world.resource_mut::<Atmosphere>() = mars;
    let Some(switched) = render(&mut switched, "air_after_switch") else {
        return;
    };
    // Mars' air from the start, rendered for as many frames (the same clock, so the same
    // output dither).
    let mut fresh = sky_app(mars, 20.0, 12.0, 30.0, 12.0);
    let Some(_) = render(&mut fresh, "air_fresh_first") else {
        return;
    };
    let Some(fresh) = render(&mut fresh, "air_fresh") else {
        return;
    };
    let diff = switched
        .as_raw()
        .iter()
        .zip(fresh.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    eprintln!("largest difference after switching the air: {diff}/255");
    assert!(
        diff <= 1,
        "the sky after switching to Mars' air differs from Mars' sky by up to {diff}/255: \
         stale atmosphere tables"
    );
}
