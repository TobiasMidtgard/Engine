//! Renders the weather station through a day of changing weather and saves a screenshot of
//! each moment: `cargo run -p engine_weather --release --example weather_tour [out_dir] [only]`.
//! Runs headless (needs a GPU or lavapipe). `only` is a comma-separated list of shot names.
//!
//! The station stands in a procedural landscape (see `landscape.rs`): a terrain with a meadow,
//! wooded hills and a mountain range, procedural trees, rocks, grass and a farm track, all
//! with textures painted on the CPU at startup.

mod landscape;
mod noise;
mod rocks;
mod terrain;
mod texgen;
mod trees;

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
    ]
}

fn scene(app: &mut App) {
    landscape::build(&mut app.world);
    let mut commands = app.world.commands();
    engine_weather::demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
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
