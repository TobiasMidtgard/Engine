//! Skies of two planets from the ground: Mars (`Atmosphere::mars()`: thin CO₂ and dust, a
//! butterscotch sky by day, a blue glow around the setting sun) and Earth (`Atmosphere::earth()`)
//! for comparison, each at midday and at sunset.
//!
//! ```text
//! cargo run -p engine_render --example planet_skies -- <out dir> [prefix] [WxH]
//! ```
//!
//! Writes `<prefix>mars_midday.png`, `<prefix>mars_sunset.png`, `<prefix>earth_midday.png` and
//! `<prefix>earth_sunset.png`; the size defaults to 960x540. Needs a GPU or a software Vulkan
//! driver (lavapipe).

use std::path::PathBuf;

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// Frames rendered before the capture: anti-aliasing and the environment settle.
const WARM_UP: usize = 12;

#[derive(Clone, Copy)]
enum Planet {
    Mars,
    Earth,
}

/// One shot: the planet, the sun's elevation and its azimuth from the view (degrees), the
/// camera's pitch (degrees) and exposure (EV100).
struct Shot {
    name: &'static str,
    planet: Planet,
    sun_elevation: f64,
    sun_azimuth: f64,
    pitch: f64,
    ev100: f32,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().map(String::as_str).unwrap_or("screenshots"));
    let prefix = args.get(1).cloned().unwrap_or_default();
    let (width, height) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((960, 540));
    std::fs::create_dir_all(&out).expect("output directory");
    let shots = [
        Shot {
            name: "mars_midday",
            planet: Planet::Mars,
            sun_elevation: 60.0,
            sun_azimuth: 150.0,
            pitch: 14.0,
            ev100: 13.0,
        },
        Shot {
            name: "mars_sunset",
            planet: Planet::Mars,
            sun_elevation: 2.0,
            sun_azimuth: 0.0,
            pitch: 8.0,
            ev100: 9.5,
        },
        Shot {
            name: "earth_midday",
            planet: Planet::Earth,
            sun_elevation: 60.0,
            sun_azimuth: 150.0,
            pitch: 14.0,
            ev100: 14.5,
        },
        Shot {
            name: "earth_sunset",
            planet: Planet::Earth,
            sun_elevation: 2.0,
            sun_azimuth: 0.0,
            pitch: 8.0,
            ev100: 11.0,
        },
    ];
    for shot in &shots {
        let path = out.join(format!("{prefix}{}.png", shot.name));
        let mut app = scene(shot, (width, height));
        app.update_n(WARM_UP);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}): nothing rendered", d.0);
            return;
        }
        capture_now(&mut app.world, &path).expect("screenshot");
        println!("{}", path.display());
    }
}

/// The planet's air, with the planet's surface at y = 0.
fn air(planet: Planet) -> Atmosphere {
    match planet {
        Planet::Mars => Atmosphere::mars(),
        Planet::Earth => Atmosphere::earth(),
    }
}

fn scene(shot: &Shot, (width, height): (u32, u32)) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(RenderPlugin);
    *app.world.resource_mut::<Atmosphere>() = air(shot.planet);
    // Thin, clear air near the ground; the atmosphere brings the haze.
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let (sun_lux, sun_size, ground) = match shot.planet {
        // Mars is 1.52 AU out: 43 % of Earth's sunlight, a disc of 0.35°.
        Planet::Mars => (
            47_000.0,
            0.35,
            StandardMaterial::color(0.42, 0.24, 0.13)
                .with_roughness(0.95)
                .with_detail(1.0, 4.0)
                .with_detail_in_world(),
        ),
        Planet::Earth => (
            110_000.0,
            0.53,
            StandardMaterial::color(0.2, 0.26, 0.11)
                .with_roughness(0.95)
                .with_detail(1.0, 4.0)
                .with_detail_in_world(),
        ),
    };
    let w = &mut app.world;
    let pitch = shot.pitch.to_radians();
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: shot.ev100 },
            ..Camera::perspective(70.0)
        },
        Transform::from_xyz(0.0, 1.7, 0.0)
            .looking_at(DVec3::new(0.0, 1.7 + pitch.tan(), -1.0), Vec3::Y),
    ));
    // The light travels away from the sun.
    let (e, a) = (
        shot.sun_elevation.to_radians(),
        shot.sun_azimuth.to_radians(),
    );
    let toward_sun = DVec3::new(-a.sin() * e.cos(), e.sin(), -a.cos() * e.cos());
    w.spawn((
        DirectionalLight {
            color: [1.0, 1.0, 1.0],
            illuminance: sun_lux,
            angular_size_deg: sun_size,
            ..Default::default()
        },
        Transform::default().looking_at(-toward_sun, Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4_000.0, 4_000.0],
                subdivisions: 8,
            },
            ground,
        ),
    ));
    // Boulders for scale.
    let stone = match shot.planet {
        Planet::Mars => StandardMaterial::color(0.36, 0.22, 0.14).with_roughness(0.85),
        Planet::Earth => StandardMaterial::color(0.42, 0.4, 0.37).with_roughness(0.85),
    };
    for i in 0..12u32 {
        let f = i as f64;
        let x = (f * 37.0) % 70.0 - 35.0;
        let z = -12.0 - (f * 53.0) % 110.0;
        let s = 0.6 + (f * 0.73) % 2.2;
        w.spawn((
            Transform::from_xyz(x, 0.0, z),
            MeshRenderer::new(
                Shape::Rock {
                    size: [s as f32 * 1.4, s as f32, s as f32 * 1.2],
                    seed: i,
                    detail: 0,
                },
                stone,
            ),
        ));
    }
    app
}
