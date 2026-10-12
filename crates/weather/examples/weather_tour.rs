//! Renders the weather station through a day of changing weather and saves a screenshot of
//! each moment: `cargo run -p engine_weather --release --example weather_tour [out_dir] [only]`.
//! Runs headless (needs a GPU or lavapipe). `only` is a comma-separated list of shot names.

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_weather::WeatherPlugin;
use serde_json::{Value, json};

struct Shot {
    name: &'static str,
    /// Calls made before the shot: (api, args).
    setup: Vec<(&'static str, Value)>,
    eye: DVec3,
    target: DVec3,
    /// Frames to run before capturing (exposure settles, rain falls).
    frames: u32,
    /// Turn from `eye` toward the moon (it ends up in the upper part of the picture).
    face_moon: bool,
}

fn shots() -> Vec<Shot> {
    let set = |preset: &str| ("weather.set", json!({"preset": preset, "transition_s": 0}));
    let hour = |h: f32| ("weather.time", json!({"hour": h, "moon_phase": 0.5}));
    let advance = |s: f32| ("weather.advance", json!({"seconds": s}));
    let dry = || ("weather.ground", json!({"dry": true}));
    let station = DVec3::new(0.0, 1.0, 0.0);
    vec![
        Shot {
            name: "01_clear_morning",
            setup: vec![dry(), set("clear"), hour(9.0), advance(60.0)],
            eye: DVec3::new(-14.0, 3.0, 16.0),
            target: station,
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "02_fair_weather_clouds",
            setup: vec![set("partly_cloudy"), hour(14.0), advance(60.0)],
            eye: DVec3::new(0.0, 2.0, 18.0),
            target: DVec3::new(0.0, 60.0, -120.0),
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "03_storm_rolling_in",
            setup: vec![
                set("partly_cloudy"),
                hour(15.0),
                advance(30.0),
                (
                    "weather.set",
                    json!({"preset": "storm", "transition_s": 900, "wind_from_deg": 250}),
                ),
                advance(640.0),
            ],
            eye: DVec3::new(12.0, 2.5, -10.0),
            // Upwind: the wind comes from the west-southwest (+X, a little -Z).
            target: DVec3::new(200.0, 60.0, -70.0),
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "04_downpour",
            setup: vec![set("storm"), hour(15.5), advance(900.0)],
            eye: DVec3::new(-6.0, 1.6, 9.0),
            target: DVec3::new(2.0, 0.5, -2.0),
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "05_puddles_after_the_rain",
            setup: vec![
                set("partly_cloudy"),
                hour(16.5),
                advance(120.0),
                ("weather.ground", json!({"wetness": 1.0, "puddle_m": 0.012})),
            ],
            eye: DVec3::new(5.5, 0.6, 5.0),
            target: DVec3::new(-3.0, 1.2, -3.0),
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "06_night_rain_under_lamps",
            setup: vec![set("rain"), hour(22.5), advance(900.0)],
            eye: DVec3::new(-9.0, 1.7, 9.0),
            target: DVec3::new(-3.0, 1.5, 0.0),
            frames: 12,
            face_moon: false,
        },
        Shot {
            name: "07_lightning",
            setup: vec![
                set("storm"),
                hour(23.0),
                advance(300.0),
                ("weather.lightning", json!({"at": [60, 0, -160]})),
            ],
            eye: DVec3::new(0.0, 2.0, 14.0),
            target: DVec3::new(40.0, 60.0, -160.0),
            frames: 3,
            face_moon: false,
        },
        Shot {
            name: "08_foggy_morning",
            setup: vec![
                dry(),
                ("weather.ground", json!({"wetness": 0.3})),
                set("fog"),
                hour(7.0),
                advance(600.0),
            ],
            eye: DVec3::new(-14.0, 2.0, 14.0),
            target: station,
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "09_sunset",
            setup: vec![dry(), set("partly_cloudy"), hour(17.6), advance(120.0)],
            eye: DVec3::new(-10.0, 1.8, 6.0),
            target: DVec3::new(60.0, 10.0, 0.0),
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "10_above_the_clouds",
            setup: vec![set("partly_cloudy"), hour(10.0), advance(60.0)],
            eye: DVec3::new(0.0, 4200.0, 0.0),
            target: DVec3::new(0.0, 2600.0, -6000.0),
            frames: 10,
            face_moon: false,
        },
        Shot {
            name: "11_snow",
            setup: vec![dry(), set("snow"), hour(12.5), advance(4.0 * 3600.0)],
            eye: DVec3::new(-12.0, 2.5, 13.0),
            target: station,
            frames: 12,
            face_moon: false,
        },
        Shot {
            name: "12_clear_night_moon_and_stars",
            setup: vec![
                dry(),
                set("clear"),
                ("weather.time", json!({"hour": 19.5, "moon_phase": 0.4})),
                advance(60.0),
            ],
            eye: DVec3::new(2.0, 1.7, 14.0),
            target: DVec3::ZERO,
            frames: 12,
            face_moon: true,
        },
        Shot {
            name: "13_rainbow_in_the_showers",
            setup: vec![dry(), set("showers"), hour(17.0), advance(600.0)],
            // The camera turns to face away from the sun once the weather is right (see
            // `wait_for_rainbow`).
            eye: DVec3::new(6.0, 1.7, 8.0),
            target: DVec3::new(-100.0, 14.0, 10.0),
            frames: 10,
            face_moon: false,
        },
    ]
}

fn scene(app: &mut App) {
    let w = &mut app.world;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [150_000.0, 150_000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
        ),
    ));
    // Rolling hills and a few trees so the clouds have a landscape to sit over.
    let hill = StandardMaterial::hex(0x4a6b34).with_roughness(0.95);
    for (x, z, r) in [
        (-300.0, -500.0, 180.0),
        (250.0, -700.0, 260.0),
        (700.0, -300.0, 220.0),
        (-800.0, 100.0, 300.0),
        (500.0, 600.0, 200.0),
    ] {
        w.spawn((
            Transform::from_xyz(x, -r * 0.3, z).with_scale(Vec3::new(1.0, 0.55, 1.0)),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: r as f32,
                    segments: 48,
                },
                hill,
            ),
        ));
    }
    let bark = StandardMaterial::hex(0x5a4030).with_roughness(0.9);
    let leaves = StandardMaterial::hex(0x3d6b2a)
        .with_roughness(0.7)
        .with_subsurface(0.4);
    for k in 0..14 {
        let a = k as f64 * 2.4;
        let r = 22.0 + (k % 4) as f64 * 6.0;
        let (x, z) = (a.cos() * r, a.sin() * r);
        w.spawn((
            Transform::from_xyz(x, 1.5, z),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.2,
                    height: 3.0,
                    segments: 10,
                },
                bark,
            ),
        ));
        w.spawn((
            Transform::from_xyz(x, 4.2, z),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 2.0,
                    segments: 20,
                },
                leaves,
            ),
        ));
    }
    let mut commands = w.commands();
    engine_weather::demo::spawn_demo(&mut commands, DVec3::ZERO);
    w.flush();
}

/// Let the showers drift until the sun shines on the station while a shower falls a few
/// kilometers away on the opposite side, then face the shower (where the rainbow stands).
fn wait_for_rainbow(app: &mut App, camera: Entity, eye: DVec3) {
    for _ in 0..900 {
        let lights = engine_weather::sky::compute_lights(
            app.world.resource::<engine_weather::Sky>(),
            app.world.resource::<engine_weather::Climate>(),
            app.world.resource::<engine_weather::WeatherSim>(),
        );
        let sim = app.world.resource::<engine_weather::WeatherSim>();
        let sun = lights.sun_dir.as_dvec3();
        let sun_flat = DVec2::new(sun.x, sun.z).normalize_or(DVec2::X);
        let sunny = (0..5).all(|k| {
            let d = k as f64 * 2000.0;
            sim.cell_at(sun_flat * d).coverage < 0.45
        });
        // Rain close enough that the bow stands tall below the cloud base.
        let shower = (4..11)
            .map(|k| {
                sim.cell_at(-sun_flat * (k as f64 * 500.0))
                    .precipitation_mm_h
            })
            .fold(0.0f32, f32::max);
        if sunny && shower > 2.5 {
            let away = DVec3::new(-sun_flat.x, 0.0, -sun_flat.y);
            *app.world.get_mut::<Transform>(camera).unwrap() = Transform::from_translation(eye)
                .looking_at(eye + away * 100.0 + DVec3::Y * 18.0, Vec3::Y);
            return;
        }
        call_api(&mut app.world, "weather.advance", json!({"seconds": 60})).unwrap();
    }
    eprintln!("no rainbow weather found; shooting anyway");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "target/weather_tour".into());
    let only: Option<Vec<String>> = args
        .get(2)
        .map(|s| s.split(',').map(str::to_string).collect());
    std::fs::create_dir_all(&out_dir).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = 960;
        win.height = 540;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.05 });
    app.add_plugins((engine_world::WorldPlugin, RenderPlugin, WeatherPlugin));
    scene(&mut app);
    let camera = app
        .world
        .spawn((
            Camera::perspective(60.0),
            Transform::from_xyz(0.0, 2.0, 10.0),
        ))
        .id();
    {
        let mut settings = app.world.resource_mut::<RenderSettings>();
        settings.environment_interval = 1;
    }
    // The weather clock spawns its sun and moon on the second frame.
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    for shot in shots() {
        if let Some(only) = &only
            && !only.iter().any(|o| shot.name.contains(o.as_str()))
        {
            continue;
        }
        let started = std::time::Instant::now();
        *app.world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(shot.eye).looking_at(shot.target, Vec3::Y);
        if let Some(mut cam) = app.world.get_mut::<Camera>(camera)
            && let Exposure::Auto { speed, .. } = &mut cam.exposure
        {
            *speed = 40.0;
        }
        for (api, args) in &shot.setup {
            if let Err(e) = call_api(&mut app.world, api, args.clone()) {
                panic!("{api}: {e}");
            }
        }
        if shot.name.contains("rainbow") {
            wait_for_rainbow(&mut app, camera, shot.eye);
        }
        if shot.face_moon {
            // Where the moon is at the time the setup chose (the lights catch up next frame).
            let lights = engine_weather::sky::compute_lights(
                app.world.resource::<engine_weather::Sky>(),
                app.world.resource::<engine_weather::Climate>(),
                app.world.resource::<engine_weather::WeatherSim>(),
            );
            // Keep the horizon in view (aim at most 35° up), the moon above the middle.
            let m = lights.moon_dir.as_dvec3();
            let flat = DVec3::new(m.x, 0.0, m.z).normalize_or(DVec3::Z);
            let elevation = (m.y.asin() - 0.35).clamp(0.05, 0.6);
            let aim = flat * elevation.cos() + DVec3::Y * elevation.sin();
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(shot.eye).looking_at(shot.eye + aim * 100.0, Vec3::Y);
        }
        app.update_n(shot.frames as usize);
        let path = format!("{out_dir}/{}.png", shot.name);
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!(
                "{} ({:.1} s): {}",
                shot.name,
                started.elapsed().as_secs_f32(),
                call_api(&mut app.world, "weather.get", json!({}))
                    .map(|v| format!(
                        "{} {:.0}°C cover {:.2} rain {:.1} mm/h wind {:.1} m/s",
                        v["here"],
                        v["temperature_c"].as_f64().unwrap_or(0.0),
                        v["cloud_cover"].as_f64().unwrap_or(0.0),
                        v["precipitation_mm_h"].as_f64().unwrap_or(0.0),
                        v["wind_speed_m_s"].as_f64().unwrap_or(0.0)
                    ))
                    .unwrap_or_default()
            ),
            Err(e) => eprintln!("{}: {e}", shot.name),
        }
    }
}
