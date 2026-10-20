//! Renders the yard's silk rinse on its own: dyed silks tied to a bamboo pole across a stream,
//! streaming out in the current. The stream is a stand-in (`rinse::TestStream` and a
//! see-through plane over the review ground); in the showcase it is the valley's real stream.
//!
//! ```sh
//! cargo run -p engine_softbody --example rinse --release -- [out_dir] [WIDTHxHEIGHT] [shot,...]
//! ```

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_softbody::demo::rinse::{TestStream, spawn_rinse};

/// Water surface over the bed, m, and the current, m/s.
const LEVEL: f64 = 0.45;
const CURRENT: f32 = 0.7;

/// Seconds after the start, the camera and where it looks (x across the stream, z downstream).
const SHOTS: &[(&str, f32, [f64; 3], [f64; 3])] = &[
    ("downstream", 6.0, [1.6, 1.7, 8.5], [-0.2, 0.3, 2.0]),
    ("bank", 8.0, [4.6, 1.5, 3.2], [0.0, 0.4, 3.0]),
    ("pole", 9.0, [1.9, 1.0, 1.6], [0.2, 0.55, 0.2]),
    ("above", 10.0, [0.5, 6.0, 4.5], [0.0, 0.4, 3.6]),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map_or("screenshots/rinse", String::as_str));
    let (width, height) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let only: Option<Vec<String>> = args
        .get(3)
        .map(|s| s.split(',').map(str::to_string).collect());
    let wanted = |name: &str| only.as_ref().is_none_or(|o| o.iter().any(|n| n == name));
    let last = SHOTS
        .iter()
        .rposition(|(name, ..)| wanted(name))
        .expect("no such shot");
    engine_core::log::init("info,wgpu=warn,naga=warn");

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
    app.add_plugins((
        RenderPlugin,
        engine_world::WorldPlugin,
        engine_softbody::SoftBodyPlugin,
    ));
    app.world.resource_mut::<Atmosphere>().ground_color = [0.08, 0.11, 0.05];
    {
        let mut weather = app.world.resource_mut::<engine_world::WeatherState>();
        weather.wind = Vec3::new(0.8, 0.0, 0.6);
        weather.gustiness = 0.2;
    }
    app.world
        .insert_resource(engine_world::Water(Arc::new(TestStream {
            level: LEVEL,
            half_width: 2.6,
            current: Vec3::Z * CURRENT,
        })));
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.4, -0.75, 0.5), Vec3::Y),
    ));
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.6 },
                ..Camera::perspective(50.0)
            },
            Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::ZERO, Vec3::Y),
        ))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        engine_softbody::demo::spawn_review_ground(&mut commands, DVec3::ZERO);
        // The stream: clear water over the bed.
        commands.spawn((
            Transform::from_xyz(0.0, LEVEL, 6.0),
            MeshRenderer::new(
                Shape::Plane {
                    size: [5.2, 24.0],
                    subdivisions: 1,
                },
                StandardMaterial {
                    base_color: [0.03, 0.09, 0.08, 0.55],
                    perceptual_roughness: 0.04,
                    alpha_mode: AlphaMode::Blend,
                    ..StandardMaterial::default()
                },
            )
            .without_shadows(),
        ));
        spawn_rinse(&mut commands, DVec3::new(0.0, 1.0, 0.0), Vec3::Z, 5.8);
    });

    std::fs::create_dir_all(&out).expect("output directory");
    let mut elapsed = 0.0f32;
    for (i, (name, secs, eye, look)) in SHOTS.iter().enumerate().take(last + 1) {
        if !wanted(name) {
            continue;
        }
        while elapsed < *secs {
            app.update();
            elapsed += 1.0 / 30.0;
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}); nothing to capture", d.0);
            return;
        }
        let t = Transform::from_translation(DVec3::from_array(*eye))
            .looking_at(DVec3::from_array(*look), Vec3::Y);
        *app.world.get_mut::<Transform>(camera).unwrap() = t;
        *app.world.get_mut::<GlobalTransform>(camera).unwrap() = GlobalTransform::from(t);
        let path = out.join(format!("{:02}_{name}.png", i + 1));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{} ({elapsed:.1} s)", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
    let d = app.world.resource::<Diagnostics>();
    println!(
        "particles: {:?}, step: {:?} ms",
        d.get("softbody.particles"),
        d.get("softbody.step_ms"),
    );
}
