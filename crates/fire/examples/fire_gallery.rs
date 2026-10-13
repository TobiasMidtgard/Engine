//! Renders the burning-meadow demo from each of its viewpoints (`demo::VIEWS`) into PNGs,
//! without a window.
//!
//! ```text
//! cargo run --release -p engine_fire --example fire_gallery -- [out_dir] [views] [width] [height]
//! ```
//!
//! Like the showcase tour, each stop starts a fresh fire its view's `at_secs` along
//! (`fire.demo`) and watches it for the view's `warmup_secs`, in the showcase's default wind
//! (5 m/s from 250°), with the sun and moon where the view's `hour` puts them (the showcase's
//! sky: latitude 35°, the March equinox). `views` is a comma-separated subset of the stop keys (default: every
//! `demo::VIEWS` entry plus `fire_before`, the meadow before the fire reaches the trees);
//! the close-ups `detail_*` are listed in `DETAILS`; `key@secs` takes a view at another moment.
//! Needs a GPU or a software Vulkan driver (lavapipe). Time runs in fixed 1/30 s steps, so the
//! pictures are the same every run.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_fire::demo::{self, DemoView};
use engine_fire::{FirePlugin, GrassMeadow, GroundKind, GroundPaint, PaintedGround, PatchShape};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::{WeatherState, WorldPlugin};

/// Where the demo sits in the gallery world.
const ORIGIN: DVec3 = DVec3::new(0.0, 0.0, 0.0);

/// Extra stops: the meadow before the fire reaches the trees, and close-ups of the art.
const DETAILS: &[DemoView] = &[
    DemoView {
        key: "fire_before",
        name: "Meadow before the fire reaches the trees",
        eye: [24.0, 3.2, -16.0],
        target: [0.0, 2.0, 4.0],
        hour: 17.4,
        at_secs: 10.0,
        warmup_secs: 0.0,
    },
    DemoView {
        key: "detail_grass",
        name: "Grass close-up",
        eye: [-2.0, 0.9, -15.5],
        target: [-6.0, 0.4, -9.0],
        hour: 17.0,
        at_secs: 1.0,
        warmup_secs: 0.0,
    },
    DemoView {
        key: "detail_tree",
        name: "Tree close-up",
        eye: [9.5, 1.8, 1.5],
        target: [3.0, 3.6, 7.5],
        hour: 16.0,
        at_secs: 1.0,
        warmup_secs: 0.0,
    },
    DemoView {
        key: "detail_shed",
        name: "Shed close-up",
        eye: [9.0, 1.7, -9.0],
        target: [4.0, 1.3, -3.0],
        hour: 16.0,
        at_secs: 1.0,
        warmup_secs: 0.0,
    },
    DemoView {
        key: "detail_bales",
        name: "Hay bales across the road",
        eye: [17.0, 1.6, -0.5],
        target: [21.0, 0.6, 4.5],
        hour: 16.0,
        at_secs: 1.0,
        warmup_secs: 0.0,
    },
    DemoView {
        key: "detail_road",
        name: "Gravel road and border",
        eye: [11.0, 1.7, -24.0],
        target: [15.0, 0.0, -17.0],
        hour: 16.0,
        at_secs: 1.0,
        warmup_secs: 0.0,
    },
    DemoView {
        key: "detail_campfire",
        name: "Campfire by day",
        eye: [-23.7, 1.05, -22.4],
        target: [-22.0, 0.2, -20.5],
        hour: 16.0,
        at_secs: 1.0,
        warmup_secs: 0.0,
    },
    DemoView {
        key: "detail_burnt",
        name: "Burnt trees and shed",
        eye: [10.0, 2.2, -8.0],
        target: [1.0, 2.6, 6.0],
        hour: 16.5,
        at_secs: 150.0,
        warmup_secs: 0.0,
    },
];

/// Unit vector toward a body on the sky at `hour` with declination `decl` (the showcase's
/// climate: latitude 35°, east = −X, north = +Z).
fn celestial_direction(hour: f32, decl: f32) -> Vec3 {
    let lat = 35.0f32.to_radians();
    let h = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    let up = lat.sin() * decl.sin() + lat.cos() * decl.cos() * h.cos();
    let east = -decl.cos() * h.sin();
    let north = lat.cos() * decl.sin() - lat.sin() * decl.cos() * h.cos();
    Vec3::new(-east, up, north).normalize_or(Vec3::Y)
}

/// Put `schedule` in as the render schedule and return the one it replaces.
fn swap_render(app: &mut App, schedule: Schedule) -> Schedule {
    let mut schedules = app.world.resource_mut::<Schedules>();
    let old = schedules.remove(Render).expect("render schedule");
    schedules.insert(schedule);
    old
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/fire".into());
    let only: Option<Vec<String>> = args
        .get(2)
        .filter(|s| !s.is_empty() && s.as_str() != "all")
        .map(|s| s.split(',').map(str::to_string).collect());
    let width: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(720);
    std::fs::create_dir_all(&out).expect("output directory");

    let started = std::time::Instant::now();
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin, FirePlugin));
    // The showcase's default weather: 5 m/s from 250° (blowing toward −X, a little +Z).
    let from = 250.0f32.to_radians();
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(from.sin(), 0.0, -from.cos()) * 5.0;

    let w = &mut app.world;
    // Stand-in for the showcase's land around the demo: dry pasture litter with a trodden
    // track close by, and a plain plane out to the horizon.
    w.spawn((
        Transform::from_xyz(0.0, -0.002, 0.0),
        PaintedGround {
            half_extents: [160.0, 160.0],
            base: GroundKind::Litter,
            edge_m: 0.0,
            lift_m: 0.0,
            paints: vec![
                GroundPaint::new(
                    GroundKind::Dirt,
                    [-30.0, -40.0],
                    PatchShape::Rect {
                        half_extents: [60.0, 6.0],
                    },
                )
                .with_edge(6.0, 4.0)
                .with_strength(0.6),
            ],
        },
    ));
    // ... with dry pasture grass all around, like the showcase's field edge, thinning out
    // farther away.
    for (i, (center, half, density)) in [
        ([0.0, -31.0], [48.0, 9.0], 80.0),
        ([0.0, 31.0], [48.0, 9.0], 80.0),
        ([-40.0, 0.0], [8.0, 22.0], 80.0),
        ([40.0, 0.0], [8.0, 22.0], 80.0),
        ([0.0, -60.0], [80.0, 20.0], 18.0),
        ([0.0, 60.0], [80.0, 20.0], 18.0),
        ([-64.0, 0.0], [16.0, 40.0], 18.0),
        ([64.0, 0.0], [16.0, 40.0], 18.0),
    ]
    .into_iter()
    .enumerate()
    {
        w.spawn((
            Transform::from_xyz(center[0], 0.0, center[1]),
            GrassMeadow {
                half_extents: half,
                density,
                height: [0.3, 0.7],
                seed: 0x0ED6 + i as u32,
                ..Default::default()
            },
        ));
    }
    w.spawn((
        Transform::from_xyz(0.0, -0.02, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6e6040).with_roughness(0.95),
        ),
    ));
    let sun = w
        .spawn((DirectionalLight::default(), Transform::default()))
        .id();
    let moon = w
        .spawn((
            DirectionalLight {
                color: [0.78, 0.84, 1.0],
                illuminance: 0.0,
                cast_shadows: true,
                angular_size_deg: 0.52,
            },
            Transform::default(),
        ))
        .id();
    let camera = w
        .spawn((
            // Adapts fast: each picture gets only a few frames to settle after the last one.
            Camera {
                exposure: Exposure::Auto {
                    min_ev100: -3.0,
                    max_ev100: 18.0,
                    speed: 25.0,
                    compensation: 0.0,
                },
                ..Default::default()
            },
            Transform::from_xyz(0.0, 10.0, -40.0),
        ))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, ORIGIN);
    });

    // Let the simulation run without drawing between pictures (much faster on a software GPU).
    let mut render = Some(swap_render(&mut app, Schedule::new(Render)));
    // The demo lights its first fire (fast-forwarded by its warm-up) on the third frame.
    app.update_n(4);
    eprintln!("built the demo in {:.1} s", started.elapsed().as_secs_f32());

    // Stops: (view, seconds into the fire, file name).
    let all = || demo::VIEWS.iter().chain(DETAILS.iter());
    let views: Vec<(&DemoView, f32, String)> = match &only {
        Some(keys) => keys
            .iter()
            .filter_map(|k| {
                let (key, at) = match k.split_once('@') {
                    Some((key, t)) => (key, t.parse::<f32>().ok()),
                    None => (k.as_str(), None),
                };
                let view = all().find(|v| v.key == key)?;
                Some(match at {
                    Some(t) => (view, t, format!("{key}_{t:.0}s")),
                    None => (view, view.at_secs, key.to_string()),
                })
            })
            .collect(),
        None => all()
            .filter(|v| demo::VIEWS.iter().any(|d| d.key == v.key) || v.key == "fire_before")
            .map(|v| (v, v.at_secs, v.key.to_string()))
            .collect(),
    };
    for (view, at, file) in views {
        let t0 = std::time::Instant::now();
        engine_core::registry::call_api(
            &mut app.world,
            "fire.demo",
            serde_json::json!({ "at_secs": at }),
        )
        .expect("fire.demo");
        app.update_n(1 + (view.warmup_secs * 30.0).round() as usize);
        let sim_secs = t0.elapsed().as_secs_f32();

        // Sun and moon for the hour (day 80, moon phase 0.42 as in the showcase's sky).
        let decl = 0.0f32;
        let sun_dir = celestial_direction(view.hour, decl);
        let elevation = sun_dir.y.asin().to_degrees();
        // The showcase's sky fades the sun out over nautical twilight and lets its planet's
        // atmosphere hide it below the horizon. There is no planet here, so the sun goes out
        // as it sets: shading with a sun from below the horizon blacks out the scene.
        let sun_lux =
            120_000.0 * smoothstep(-14.0, -5.0, elevation) * smoothstep(0.0, 1.0, elevation);
        let phase = 0.42f32;
        let moon_dir = celestial_direction(
            (view.hour - phase * 24.0).rem_euclid(24.0),
            decl * (std::f32::consts::TAU * phase).cos(),
        );
        let lit = 0.5 * (1.0 - (std::f32::consts::TAU * phase).cos());
        let moon_lux = 0.3 * lit * smoothstep(-0.05, 0.1, moon_dir.y);
        {
            let mut weather = app.world.resource_mut::<WeatherState>();
            weather.time_of_day_h = view.hour;
            weather.sun_direction = sun_dir;
        }
        {
            let night = smoothstep(0.05, -0.2, sun_dir.y);
            let mut ambient = app.world.resource_mut::<AmbientLight>();
            ambient.color = Vec3::new(0.85, 0.9, 1.0)
                .lerp(Vec3::new(0.35, 0.45, 0.8), night)
                .to_array();
            ambient.brightness = 0.05 + night * 0.1;
        }
        app.world
            .get_mut::<DirectionalLight>(sun)
            .unwrap()
            .illuminance = sun_lux;
        *app.world.get_mut::<Transform>(sun).unwrap() =
            Transform::default().looking_at(-sun_dir.as_dvec3(), Vec3::Y);
        app.world
            .get_mut::<DirectionalLight>(moon)
            .unwrap()
            .illuminance = moon_lux;
        *app.world.get_mut::<Transform>(moon).unwrap() =
            Transform::default().looking_at(-moon_dir.as_dvec3(), Vec3::Y);
        let eye = ORIGIN + DVec3::from_array(view.eye);
        let target = ORIGIN + DVec3::from_array(view.target);
        // A cut the renderer sees as one: it keeps the last picture's history (anti-aliasing,
        // fog) across a jump of less than 25 m, and the stops are closer together than that,
        // so the camera looks from far off for a frame first. Without it, a stop smears
        // ghosts of the one before over its sky.
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye + DVec3::Y * 1000.0).looking_at(target, Vec3::Y);
        let empty = swap_render(&mut app, render.take().expect("render schedule"));
        app.update();
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);

        // Draw a few frames for exposure, shadows and the environment map to settle.
        app.update_n(6);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{file}.png");
        let status =
            engine_core::registry::call_api(&mut app.world, "fire.status", serde_json::json!({}))
                .map(|s| {
                    format!(
                        "{:.0} m² burning, {} objects",
                        s["burning_m2"].as_f64().unwrap_or(0.0),
                        s["objects"].as_array().map_or(0, |o| o.len())
                    )
                })
                .unwrap_or_default();
        let sample = engine_core::registry::call_api(
            &mut app.world,
            "fire.sample",
            serde_json::json!({"at": [target.x, 0.0, target.z]}),
        )
        .map(|s| s.to_string())
        .unwrap_or_default();
        eprintln!("  at the target: {sample}");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!(
                "{path}: {} at +{:.0} s, {status}, EV {:.1} (simulated in {sim_secs:.1} s, {:.1} s in all)",
                view.name,
                at + view.warmup_secs,
                app.world.resource::<engine_render::RenderStats>().ev100,
                t0.elapsed().as_secs_f32()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
        render = Some(swap_render(&mut app, empty));
    }
    let stats = app.world.resource::<engine_render::RenderStats>();
    eprintln!("{stats:?}");
}
