//! Renders the smoke range from each of its viewpoints (`demo::VIEWS`) into PNGs, without a
//! window.
//!
//! ```text
//! cargo run --release -p engine_smoke --example smoke_gallery -- [out_dir] [views] [width] [height]
//! ```
//!
//! For every view the demo restarts (`smoke.demo` with `{"restart": true}`), runs the view's
//! `warmup_secs` of world time and then takes the picture, with the sun where the view's
//! `hour` puts it. `views` is a comma-separated subset of the view keys, plus the close-ups
//! `detail_turret`, `detail_fan`, `detail_cart`, `detail_targets`, `detail_vent`,
//! `detail_grenades` and `detail_berm` (default: every `demo::VIEWS` entry; `all` for every
//! view and close-up). Close-ups put the grenades out and clear the air first (except the
//! vent's, which shows its plume) and park the cart where `detail_cart` looks. Needs a GPU or a software Vulkan
//! driver (lavapipe). Time runs in fixed 1/30 s steps, so the pictures are the same every run.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_smoke::SmokePlugin;
use engine_smoke::demo::{self, DemoView};
use engine_world::{WeatherState, WorldPlugin};
use serde_json::json;

/// Where the range sits in the gallery world.
const ORIGIN: DVec3 = DVec3::new(0.0, 0.0, 0.0);

/// Close-ups of the props (not part of the demo's own view list).
const DETAILS: &[DemoView] = &[
    DemoView {
        key: "detail_turret",
        name: "Turret",
        eye: [2.3, 2.45, -21.6],
        target: [-0.2, 1.25, -18.7],
        hour: 10.5,
        warmup_secs: -1.0,
    },
    DemoView {
        key: "detail_fan",
        name: "Fan",
        eye: [-13.0, 2.5, -1.8],
        target: [-17.4, 2.1, 2.0],
        hour: 15.5,
        warmup_secs: -1.0,
    },
    DemoView {
        key: "detail_cart",
        name: "Cart",
        eye: [-4.6, 2.3, 0.8],
        target: [-8.6, 0.9, 5.5],
        hour: 15.0,
        warmup_secs: -1.0,
    },
    DemoView {
        key: "detail_targets",
        name: "Targets",
        eye: [3.0, 1.9, 9.0],
        target: [-1.0, 1.4, 16.5],
        hour: 15.0,
        warmup_secs: -1.0,
    },
    DemoView {
        key: "detail_vent",
        name: "Steam vent",
        eye: [17.6, 1.7, -13.6],
        target: [13.6, 1.0, -9.6],
        hour: 10.5,
        warmup_secs: 3.0,
    },
    DemoView {
        key: "detail_grenades",
        name: "Grenades",
        eye: [-2.3, 0.32, 0.62],
        target: [-2.5, 0.13, 1.0],
        hour: 10.5,
        warmup_secs: -1.0,
    },
    DemoView {
        key: "detail_berm",
        name: "Bullet trap",
        eye: [-9.0, 2.6, 8.5],
        target: [-14.0, 2.2, 19.0],
        hour: 15.0,
        warmup_secs: -1.0,
    },
];

/// Where the cart stands in the cart close-up (world x), so the camera finds it.
const CART_AT: f64 = -8.6;

/// Unit vector toward the sun at `hour` (the showcase's day cycle: rises in −X, sets in +X,
/// peaks about 66° up, a little toward −Z).
fn sun_direction(hour: f32) -> Vec3 {
    let theta = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    Vec3::new(theta.sin() * 0.9, theta.cos() * 0.8, -0.35).normalize()
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
        .unwrap_or_else(|| "screenshots/smoke".into());
    let all = args.get(2).is_some_and(|s| s == "all");
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
    app.add_plugins((RenderPlugin, WorldPlugin, SmokePlugin));

    let w = &mut app.world;
    // Short, dry grass around the range.
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x56603a).with_roughness(0.95),
        ),
    ));
    let sun = w
        .spawn((
            DirectionalLight {
                illuminance: 100_000.0,
                ..Default::default()
            },
            Transform::default(),
        ))
        .id();
    let camera = w
        .spawn((Camera::default(), Transform::from_xyz(0.0, 10.0, -40.0)))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, ORIGIN);
    });

    // Let the simulation run without drawing between pictures (much faster on a software GPU).
    let mut render = Some(swap_render(&mut app, Schedule::new(Render)));
    app.update_n(4);
    eprintln!(
        "built the range in {:.1} s",
        started.elapsed().as_secs_f32()
    );

    let views: Vec<&DemoView> = demo::VIEWS
        .iter()
        .chain(DETAILS.iter())
        .filter(|v| match &only {
            Some(o) => o.iter().any(|k| k == v.key),
            None => all || demo::VIEWS.iter().any(|d| d.key == v.key),
        })
        .collect();
    for view in views {
        let t0 = std::time::Instant::now();
        if view.warmup_secs >= 0.0 {
            engine_core::registry::call_api(&mut app.world, "smoke.demo", json!({"restart": true}))
                .expect("smoke.demo");
            app.update_n(2);
            app.update_n((view.warmup_secs * 30.0).round() as usize);
        } else {
            // A still moment for looking at the props: put the grenades out and clear the air.
            let mut q = app
                .world
                .query::<(&demo::DemoGrenade, &mut engine_smoke::SmokeSource)>();
            for (_, mut source) in q.iter_mut(&mut app.world) {
                source.active = false;
            }
            engine_core::registry::call_api(&mut app.world, "smoke.clear", json!({}))
                .expect("smoke.clear");
            app.update_n(2);
        }
        if view.key.starts_with("detail_") {
            // Park the cart where the camera looks (it eases from `from` to `to` and back).
            let mut q = app.world.query::<&mut demo::DemoCart>();
            for mut cart in q.iter_mut(&mut app.world) {
                let s = ((ORIGIN.x + CART_AT - cart.from.x) / (cart.to.x - cart.from.x))
                    .clamp(0.0, 1.0);
                cart.phase = ((1.0 - 2.0 * s).acos() / std::f64::consts::PI) as f32;
            }
        }
        let sim_secs = t0.elapsed().as_secs_f32();

        let dir = sun_direction(view.hour);
        {
            let mut weather = app.world.resource_mut::<WeatherState>();
            weather.time_of_day_h = view.hour;
            weather.sun_direction = dir;
        }
        *app.world.get_mut::<Transform>(sun).expect("sun") =
            Transform::default().looking_at(-dir.as_dvec3(), Vec3::Y);
        let eye = ORIGIN + DVec3::from_array(view.eye);
        let target = ORIGIN + DVec3::from_array(view.target);
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);

        // Draw a few frames for exposure, shadows and the environment map to settle.
        let empty = swap_render(&mut app, render.take().expect("render schedule"));
        app.update_n(8);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{}.png", view.key);
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!(
                "{path}: {} (simulated in {sim_secs:.1} s, {:.1} s in all)",
                view.name,
                t0.elapsed().as_secs_f32()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
        render = Some(swap_render(&mut app, empty));
    }
    let stats = app.world.resource::<engine_render::RenderStats>();
    eprintln!("{stats:?}");
}
