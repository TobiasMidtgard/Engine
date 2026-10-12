//! Renders the water test area from several viewpoints into PNGs, without a window.
//!
//! ```text
//! cargo run -p engine_water --example water_gallery --release -- [out_dir] [seconds] [views]
//! ```
//!
//! `seconds` of simulation run before the first picture (default 12); `views` is a
//! comma-separated subset of: overview, falls, shore, under, pond, wake, aerial.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_water::WaterPlugin;
use engine_water::demo::{self, LAKE_LEVEL};
use engine_world::WorldPlugin;

/// (key, eye, target), local to the test area's center.
const VIEWS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("overview", [62.0, 18.0, 66.0], [0.0, 4.0, 0.0]),
    ("falls", [34.5, 3.6, 33.0], [27.0, 4.2, 27.4]),
    ("shore", [-1.5, 7.3, 30.5], [-2.0, 5.0, 0.0]),
    ("under", [-2.0, 3.6, 7.0], [5.0, 4.6, 17.5]),
    ("pond", [56.0, 2.0, 38.0], [46.7, 0.3, 46.7]),
    ("wake", [-3.0, 11.0, 26.0], [-5.0, 5.5, -3.0]),
    ("aerial", [0.0, 95.0, 70.0], [10.0, 0.0, 8.0]),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/water".into());
    let seconds: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(12.0);
    let only: Option<Vec<String>> = args
        .get(3)
        .map(|s| s.split(',').map(str::to_string).collect());
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
    app.add_plugins((RenderPlugin, WorldPlugin, WaterPlugin));

    let w = &mut app.world;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [2000.0, 2000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x55743a).with_roughness(0.95),
        ),
    ));
    // Late-afternoon sun, low over the far side of the lake from the main viewpoint.
    let to_sun = Vec3::new(-0.55, 0.36, -0.75).normalize();
    w.spawn((
        DirectionalLight {
            illuminance: 95_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-to_sun.as_dvec3(), Vec3::Y),
    ));
    let camera = w
        .spawn((Camera::default(), Transform::from_xyz(60.0, 18.0, 60.0)))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });

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
        // A few frames for exposure, the environment map and the ripples to follow the camera.
        let started = std::time::Instant::now();
        app.update_n(6);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!(
                "{path} ({:.1} s; water level {LAKE_LEVEL} m)",
                started.elapsed().as_secs_f32()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
    let stats = app.world.resource::<engine_render::RenderStats>();
    eprintln!("{stats:?}");
}
