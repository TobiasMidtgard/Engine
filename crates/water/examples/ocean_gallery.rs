//! Renders the open ocean in three winds from a few viewpoints into PNGs, without a window: the
//! sea's look at a glance (fine waves, glitter, whitecaps, reflections).
//!
//! ```text
//! cargo run -p engine_water --example ocean_gallery --release -- [out_dir] [views]
//! ```
//!
//! `views` is a comma-separated subset of: glitter, close, horizon, down; each is rendered in a
//! light breeze (4 m/s), a fresh breeze (10 m/s) and a gale (18 m/s).

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_water::{WaterBody, WaterPlugin};
use engine_world::{WeatherState, WorldPlugin};

/// (key, eye, target): toward the low sun across its glitter, a few meters over the swell, along
/// the wind away from the sun, and straight down at the water.
const VIEWS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("glitter", [0.0, 6.0, 0.0], [-60.0, 0.0, -100.0]),
    ("close", [0.0, 3.0, 0.0], [12.0, 0.0, 9.0]),
    ("horizon", [0.0, 25.0, 0.0], [400.0, 0.0, 300.0]),
    ("down", [0.0, 9.0, 0.0], [2.0, 0.0, 1.5]),
];

const WINDS: &[f32] = &[4.0, 10.0, 18.0];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/ocean".into());
    let only: Option<Vec<String>> = args
        .get(2)
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
    // A low sun ahead of the glitter view.
    let to_sun = Vec3::new(-0.5, 0.3, -0.8).normalize();
    w.spawn((
        DirectionalLight {
            illuminance: 95_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-to_sun.as_dvec3(), Vec3::Y),
    ));
    w.spawn((Name::new("Ocean"), Transform::default(), WaterBody::ocean()));
    let camera = w
        .spawn((Camera::default(), Transform::from_xyz(0.0, 6.0, 0.0)))
        .id();

    for &wind in WINDS {
        // Blowing away from the sun, across the glitter view.
        app.world.resource_mut::<WeatherState>().wind = Vec3::new(0.8, 0.0, 0.6) * wind;
        // Let the sea build without drawing (much faster on a software GPU).
        let render = app
            .world
            .resource_mut::<Schedules>()
            .remove(Render)
            .expect("render schedule");
        app.world
            .resource_mut::<Schedules>()
            .insert(Schedule::new(Render));
        app.update_n(300);
        app.world.resource_mut::<Schedules>().insert(render);

        for (key, eye, target) in VIEWS {
            if only.as_ref().is_some_and(|o| !o.iter().any(|k| k == key)) {
                continue;
            }
            *app.world.get_mut::<Transform>(camera).expect("camera") =
                Transform::from_translation(DVec3::from_array(*eye))
                    .looking_at(DVec3::from_array(*target), Vec3::Y);
            let started = std::time::Instant::now();
            // A few frames for exposure and the environment map.
            app.update_n(6);
            if let Some(d) = app.world.get_resource::<RenderDisabled>() {
                eprintln!("no GPU: {}", d.0);
                return;
            }
            let path = format!("{out}/{key}_{wind:.0}.png");
            match capture_now(&mut app.world, &path) {
                Ok(_) => eprintln!("{path} ({:.1} s)", started.elapsed().as_secs_f32()),
                Err(e) => eprintln!("{path}: {e}"),
            }
        }
    }
}
