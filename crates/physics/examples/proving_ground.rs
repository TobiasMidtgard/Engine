//! Renders the physics proving ground headless and saves screenshots as it plays out.
//!
//! ```sh
//! cargo run -p engine_physics --release --example proving_ground -- [out_dir] [width]x[height] [only]
//! ```
//!
//! `only` renders just the shots whose name contains it (the rest are still simulated).
//! `PHYSICS_DEBUG=1` draws the physics debug view over the scene (colliders, contacts, joints,
//! centers of mass) and adds `_debug` to the file names.
//!
//! Needs a GPU or a software Vulkan driver (lavapipe: `apt-get install mesa-vulkan-drivers`).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsDebug, PhysicsPlugin, PhysicsStats, demo};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::WorldPlugin;

/// (world seconds, file name, camera position, look-at target), yard-local.
const SHOTS: &[(f32, &str, [f64; 3], [f64; 3])] = &[
    (
        0.25,
        "01_overview_start",
        [0.0, 24.0, 44.0],
        [0.0, 2.0, -3.0],
    ),
    (
        1.0,
        "02_ragdolls_on_stairs",
        [33.0, 4.5, -4.0],
        [24.0, 1.3, -12.0],
    ),
    (
        1.9,
        "03_wrecking_ball",
        [-23.0, 4.0, 27.0],
        [-24.5, 2.5, 10.0],
    ),
    (
        2.0,
        "04_seesaw_and_wheel",
        [-1.0, 6.0, 25.0],
        [-4.0, 2.5, 9.0],
    ),
    (
        2.6,
        "05_pile_and_tower",
        [-8.0, 9.0, 4.0],
        [-18.0, 3.0, -15.0],
    ),
    (5.0, "06_dominoes", [8.0, 4.5, -4.0], [5.0, 0.4, -16.0]),
    (6.0, "07_bridge", [27.5, 4.0, 10.0], [18.0, 2.6, 10.0]),
    (6.5, "08_turntable", [37.0, 6.5, 20.0], [30.0, 0.5, 10.0]),
    (
        12.0,
        "09_overview_after",
        [0.0, 24.0, 44.0],
        [0.0, 2.0, -3.0],
    ),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).cloned().unwrap_or("physics_shots".into()));
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let only = args.get(3).cloned().unwrap_or_default();
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.add_plugins(RenderPlugin);
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    let debug = std::env::var("PHYSICS_DEBUG").is_ok_and(|v| v != "0");
    if debug {
        app.add_plugins(engine_debug::DebugPlugin);
        *app.world.resource_mut::<PhysicsDebug>() = PhysicsDebug {
            colliders: true,
            contacts: true,
            joints: true,
            centers_of_mass: true,
            radius_m: 80.0,
            ..Default::default()
        };
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<AmbientLight>().brightness = 400.0;
    let camera = app
        .world
        .spawn((
            Camera::perspective(55.0),
            Transform::from_xyz(0.0, 24.0, 44.0),
        ))
        .id();
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.45, -0.75, -0.5), Vec3::Y),
    ));
    // Grass around the yard.
    app.world.spawn((
        Transform::from_xyz(0.0, -0.02, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f7a3a).with_roughness(0.95),
        ),
    ));
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });

    let mut t = 0.0f32;
    for (at, name, eye, target) in SHOTS.iter().filter(|s| s.1.contains(only.as_str())) {
        // Simulate up to a few frames before the shot without drawing (fast on lavapipe),
        // then draw a few frames so auto exposure settles.
        let mut frames = 0;
        while t < at - 4.0 / 60.0 {
            frames += 1;
            t += 1.0 / 60.0;
        }
        simulate(&mut app, frames);
        {
            let mut tr = app.world.get_mut::<Transform>(camera).unwrap();
            *tr = Transform::from_translation(DVec3::from_array(*eye))
                .looking_at(DVec3::from_array(*target), Vec3::Y);
        }
        for _ in 0..4 {
            app.update();
            t += 1.0 / 60.0;
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let suffix = if debug { "_debug" } else { "" };
        let path = out.join(format!("{name}{suffix}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => {
                let s = *app.world.resource::<PhysicsStats>();
                println!(
                    "t = {t:5.2} s  {}  ({} bodies, {} awake, step {:.2} ms)",
                    path.display(),
                    s.bodies,
                    s.awake,
                    s.step_ms
                );
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}

/// Run `frames` frames without drawing them: the renderer draws every frame (from the origin
/// when no camera is active), so it is parked meanwhile.
fn simulate(app: &mut App, frames: usize) {
    let renderer = app.world.remove_resource::<Renderer>();
    app.world
        .insert_resource(RenderDisabled("simulating without drawing".into()));
    app.update_n(frames);
    app.world.remove_resource::<RenderDisabled>();
    if let Some(r) = renderer {
        app.world.insert_resource(r);
    }
}
