//! A bench for the look of smoke: white and colored smoke over a field with a few blocks, seen
//! with the sun behind, beside and in front of the camera, rendered into PNGs without a window.
//!
//! ```text
//! cargo run -p engine_smoke --example smoke_look --release -- [out_dir] [seconds] [views] [hour]
//! ```
//!
//! `seconds` of simulation run before the first picture (default 14); `views` is a
//! comma-separated subset of: front, side, back, close, high; `hour` sets the sun (default 15).

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_smoke::{SmokePlugin, SmokeSource, SmokeVolume};
use engine_world::WorldPlugin;

/// (key, eye, target).
const VIEWS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("front", [-2.0, 3.0, -26.0], [0.0, 3.0, 0.0]),
    ("side", [-26.0, 4.0, 2.0], [0.0, 3.0, 0.0]),
    ("back", [3.0, 3.0, 26.0], [0.0, 3.5, 0.0]),
    ("close", [-6.0, 1.8, -9.0], [0.0, 2.5, 0.0]),
    ("high", [-24.0, 18.0, -24.0], [0.0, 2.0, 0.0]),
];

/// Unit vector toward the sun for an hour of the day (as in the showcase: rises in -X, sets
/// in +X, peaks a little to the -Z side).
fn sun_direction(hour: f32) -> Vec3 {
    let theta = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    Vec3::new(theta.sin() * 0.9, theta.cos() * 0.8, -0.35).normalize()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/smoke_look".into());
    let seconds: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(14.0);
    let only: Option<Vec<String>> = args
        .get(3)
        .filter(|s| !s.is_empty() && *s != "all")
        .map(|s| s.split(',').map(str::to_string).collect());
    let hour: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(15.0);
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin, SmokePlugin));

    let w = &mut app.world;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5d7a3e).with_roughness(0.95),
        ),
    ));
    // Blocks to catch the smoke's shadow and show scale (a 2 m person-sized post among them).
    for (x, z, size) in [
        (-6.0, -4.0, [2.0, 1.0, 1.0]),
        (5.0, -6.0, [1.2, 2.0, 1.2]),
        (8.0, 4.0, [3.0, 1.5, 0.6]),
        (-3.0, 7.0, [0.3, 2.0, 0.3]),
    ] {
        w.spawn((
            Transform::from_xyz(x, size[1] as f64 * 0.5, z),
            MeshRenderer::new(
                Shape::Cuboid { size },
                StandardMaterial::hex(0x9a9387).with_roughness(0.8),
            ),
        ));
    }
    let to_sun = sun_direction(hour);
    w.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-to_sun.as_dvec3(), Vec3::Y),
    ));
    let camera = w
        .spawn((Camera::default(), Transform::from_xyz(-2.0, 3.0, -26.0)))
        .id();
    // The volume's Transform is the center of its floor.
    w.spawn((
        Transform::default(),
        SmokeVolume {
            size: [40.0, 16.0, 40.0],
            cell_m: 0.5,
        },
    ));
    for (x, z, color, secs) in [
        (-1.0, 0.0, [0.9, 0.9, 0.9], 40.0),
        (5.0, 6.0, [0.85, 0.12, 0.08], 40.0),
        (-6.0, 5.0, [0.15, 0.6, 0.2], 40.0),
    ] {
        w.spawn((
            Transform::from_xyz(x, 0.15, z),
            SmokeSource::grenade(color, secs),
        ));
    }

    // Let the simulation run without drawing (much faster on a software GPU).
    let render = app
        .world
        .resource_mut::<Schedules>()
        .remove(Render)
        .expect("render schedule");
    app.world
        .resource_mut::<Schedules>()
        .insert(Schedule::new(Render));
    let frames = (seconds * 30.0) as usize;
    let started = std::time::Instant::now();
    app.update_n(frames.max(2));
    eprintln!(
        "simulated {seconds} s in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    app.world.resource_mut::<Schedules>().insert(render);

    for (key, eye, target) in VIEWS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|k| k == key)) {
            continue;
        }
        let eye = DVec3::from_array(*eye);
        let target = DVec3::from_array(*target);
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(eye).looking_at(target, Vec3::Y);
        // A few frames for exposure and the environment map to settle.
        let started = std::time::Instant::now();
        app.update_n(6);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!("{path} ({:.1} s)", started.elapsed().as_secs_f32()),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}
