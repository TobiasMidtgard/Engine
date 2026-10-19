//! Renders the yard's unveiling on its own through one cycle: the crane lowering the silk
//! sheet over the marble statue, the sheet resting on it, and lifting it off. Saves one PNG
//! per moment. Software Vulkan (lavapipe) is enough.
//!
//! ```sh
//! cargo run -p engine_softbody --example unveil --release -- [out_dir] [WIDTHxHEIGHT] [shot,...]
//! ```
//!
//! The first shots are close-ups of the bare statue, before the sheet comes down.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// The way the statue faces.
const FACING: Vec3 = Vec3::new(0.5, 0.0, -0.87);

/// Seconds into the cycle, and the camera: distance out in front, to the statue's right,
/// height; and the height it looks at.
const SHOTS: &[(&str, f32, [f64; 3], f64)] = &[
    ("statue", 1.0, [2.5, 0.5, 1.8], 1.85),
    ("statue_quarter", 1.0, [1.7, 1.7, 2.0], 1.9),
    ("statue_torso", 1.0, [1.25, 0.35, 2.15], 2.1),
    ("statue_head", 1.0, [0.62, 0.22, 2.66], 2.62),
    ("raised", 2.0, [6.5, 1.0, 2.6], 3.2),
    ("lowering", 5.5, [5.5, 0.8, 2.2], 2.6),
    ("settling", 8.5, [5.0, 0.6, 2.0], 2.0),
    ("covered", 15.0, [4.6, 1.2, 1.9], 1.9),
    ("covered_side", 16.0, [1.0, 4.4, 1.7], 1.9),
    ("covered_back", 16.3, [-4.2, -1.5, 1.8], 1.9),
    ("lifting", 17.3, [5.5, 0.8, 2.3], 2.6),
    ("lifted", 18.0, [6.0, 0.9, 2.6], 3.0),
    ("revealed", 20.0, [3.6, 0.9, 1.9], 1.9),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map_or("screenshots/unveil", String::as_str));
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
    // Bare earth around the plinth, meadow beyond.
    app.world.resource_mut::<Atmosphere>().ground_color = [0.11, 0.12, 0.07];
    // The statue's body is built before the first frame.
    app.world
        .resource_mut::<engine_softbody::demo::skin::BodyLibrary>()
        .blocking = true;
    // Afternoon sun over the statue's right shoulder, as at the showcase's tour stop.
    let sun = (FACING * 0.55 + Vec3::new(-FACING.z, 0.0, FACING.x) * 0.45 + Vec3::Y * 0.7)
        .normalize()
        .as_dvec3();
    app.world.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-sun, Vec3::Y),
    ));
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.6 },
                ..Camera::perspective(50.0)
            },
            Transform::from_xyz(0.0, 3.0, 8.0).looking_at(DVec3::ZERO, Vec3::Y),
        ))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        engine_softbody::demo::spawn_review_ground(&mut commands, DVec3::ZERO);
        engine_softbody::demo::statue::spawn_unveiling(&mut commands, DVec3::ZERO, FACING);
    });

    std::fs::create_dir_all(&out).expect("output directory");
    let front = FACING.as_dvec3();
    let right = DVec3::new(-front.z, 0.0, front.x);
    let mut elapsed = 0.0f32;
    for (i, (name, secs, [ahead, side, up], look)) in SHOTS.iter().enumerate().take(last + 1) {
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
        let eye = front * *ahead + right * *side + DVec3::Y * *up;
        let t = Transform::from_translation(eye).looking_at(DVec3::Y * *look, Vec3::Y);
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
