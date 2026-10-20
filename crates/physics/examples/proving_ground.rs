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
use engine_world::{SimulationFocus, WorldPlugin};

/// (world seconds, file name, camera position, look-at target), yard-local.
const SHOTS: &[(f32, &str, [f64; 3], [f64; 3])] = &[
    // The pool table: the cue ball rolling at the rack, the rack opening, and the balls at rest,
    // seen from above, a few of them down the pockets.
    (
        0.1,
        "26_pool_cue_ball_rolling",
        [8.6, 1.9, -6.6],
        [9.6, 0.8, -8.6],
    ),
    (
        0.25,
        "01_overview_start",
        [0.0, 24.0, 44.0],
        [0.0, 2.0, -3.0],
    ),
    // The Newton's cradle: the first ball swinging in, the last flying out with the middle three
    // still, and the first flying out again.
    (
        0.3,
        "20_cradle_swinging_in",
        [17.3, 2.3, -2.2],
        [16.0, 1.75, -4.5],
    ),
    // The catapult: cocked, the arm slamming into its crossbar as the ball leaves, the ball at
    // the top of its flight over the yard, the crate wall as it hits, and the wall after.
    (
        0.3,
        "33_catapult_cocked",
        [-8.6, 2.3, 9.8],
        [-4.4, 2.1, 3.0],
    ),
    (0.35, "27_pool_break", [8.6, 1.9, -6.6], [9.6, 0.8, -8.6]),
    // The wrecking ball's chain close up under the swinging ball's load (and later, below).
    (
        0.7,
        "16_wrecking_chain_swinging",
        [-31.5, 5.5, 16.0],
        [-26.5, 4.6, 10.0],
    ),
    (
        0.95,
        "21_cradle_last_ball_out",
        [17.3, 2.3, -2.2],
        [16.0, 1.75, -4.5],
    ),
    (
        0.97,
        "34_catapult_release",
        [-8.6, 2.3, 9.8],
        [-4.4, 2.6, 3.0],
    ),
    (
        1.0,
        "02_ragdolls_on_stairs",
        [33.0, 4.5, -4.0],
        [24.0, 1.3, -12.0],
    ),
    // Ten dummies on the stairs: halfway down, then a heap at the foot, all asleep.
    (
        1.5,
        "14_dummies_tumbling",
        [30.0, 3.2, -5.5],
        [24.0, 1.6, -12.0],
    ),
    (
        1.7,
        "35_catapult_ball_flying",
        [0.0, 7.0, -11.0],
        [2.0, 4.0, 3.0],
    ),
    (
        1.8,
        "22_cradle_first_ball_out_again",
        [17.3, 2.3, -2.2],
        [16.0, 1.75, -4.5],
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
    // The drums on the barrel run's ramp, then rolling abreast down the runway, then stopped in
    // the sand.
    (
        2.2,
        "11_barrel_run_ramp",
        [-19.0, 2.4, 24.2],
        [-30.0, 1.3, 19.5],
    ),
    // The rockfall: rocks pouring off the chute, then the heap.
    (
        2.4,
        "18_rockfall_pouring",
        [-15.0, 4.0, 7.0],
        [-24.5, 2.0, -2.0],
    ),
    (
        2.6,
        "05_pile_and_tower",
        [-8.0, 9.0, 4.0],
        [-18.0, 3.0, -15.0],
    ),
    // The Galton board: the hopper emptying through the funnel, the balls pouring through the
    // pins (and close up), and the bins filled in a bell curve.
    (
        0.6,
        "29_galton_pour_starts",
        [10.35, 1.3, -15.85],
        [10.0, 1.22, -18.0],
    ),
    (
        2.5,
        "31_galton_pins_close",
        [10.12, 1.08, -17.6],
        [10.0, 0.98, -18.0],
    ),
    // The pendulum wave: nearly in step, in snakes, and in two alternate rows at half a minute.
    (2.8, "36_catapult_impact", [3.0, 2.5, -2.0], [7.6, 1.6, 3.0]),
    (
        3.0,
        "23_pendulum_wave_start",
        [16.0, 6.2, -3.4],
        [16.0, 1.5, -8.6],
    ),
    (
        5.0,
        "30_galton_pouring",
        [10.35, 1.3, -15.85],
        [10.0, 1.22, -18.0],
    ),
    (5.0, "06_dominoes", [8.0, 4.5, -4.0], [5.0, 0.4, -16.0]),
    (6.0, "07_bridge", [27.5, 4.0, 10.0], [18.0, 2.6, 10.0]),
    (6.5, "08_turntable", [37.0, 6.5, 20.0], [30.0, 0.5, 10.0]),
    (
        7.0,
        "37_catapult_wall_after",
        [0.0, 7.0, -11.0],
        [2.0, 2.5, 3.0],
    ),
    (7.9, "12_barrel_run", [0.0, 1.7, 23.2], [-9.0, 0.35, 18.9]),
    (
        8.5,
        "15_dummies_at_rest",
        [28.0, 2.4, -5.0],
        [24.0, 0.5, -9.8],
    ),
    (
        10.0,
        "19_rockfall_heap",
        [-18.0, 2.4, 3.0],
        [-23.0, 0.7, -2.0],
    ),
    (
        10.0,
        "24_pendulum_wave_snakes",
        [16.0, 6.2, -3.4],
        [16.0, 1.5, -8.6],
    ),
    (
        12.0,
        "28_pool_after_the_break",
        [9.0, 4.4, -6.6],
        [9.0, 0.8, -8.5],
    ),
    (
        12.0,
        "09_overview_after",
        [0.0, 24.0, 44.0],
        [0.0, 2.0, -3.0],
    ),
    // Where the chain meets the ball, close up, as it swings on.
    (
        14.0,
        "17_wrecking_chain_end",
        [-21.5, 5.2, 15.0],
        [-26.0, 4.8, 10.0],
    ),
    (
        20.0,
        "13_barrel_run_sand",
        [22.5, 1.6, 23.9],
        [29.0, 0.4, 19.0],
    ),
    (
        28.0,
        "32_galton_bell_curve",
        [10.35, 1.3, -15.85],
        [10.0, 1.22, -18.0],
    ),
    (
        30.0,
        "25_pendulum_wave_two_rows",
        [16.0, 6.2, -3.4],
        [16.0, 1.5, -8.6],
    ),
    // The dominoes replay every 30 s: standing again, the first one tipping.
    (
        30.6,
        "10_dominoes_replayed",
        [8.0, 4.5, -4.0],
        [5.0, 0.4, -16.0],
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
    // `PHYSICS_FOCUS=x,y,z` simulates as if the player stood there (far away, the yard
    // simulates at a coarser level of detail).
    let focus: Option<DVec3> = std::env::var("PHYSICS_FOCUS").ok().and_then(|v| {
        let p: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (p.len() == 3).then(|| DVec3::new(p[0], p[1], p[2]))
    });
    if let Some(at) = focus {
        app.world
            .spawn((Transform::from_translation(at), SimulationFocus::default()));
    }
    if debug {
        app.add_plugins(engine_debug::DebugPlugin);
        *app.world.resource_mut::<PhysicsDebug>() = PhysicsDebug {
            colliders: true,
            contacts: true,
            joints: true,
            centers_of_mass: true,
            // Drawn around the focus, so around it and the yard both when it is far.
            radius_m: focus.map_or(80.0, |f| f.length() + 80.0),
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
        let suffix = match (debug, focus) {
            (true, Some(_)) => "_debug_far",
            (true, None) => "_debug",
            (false, Some(_)) => "_far",
            (false, None) => "",
        };
        let path = out.join(format!("{name}{suffix}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => {
                let s = *app.world.resource::<PhysicsStats>();
                println!(
                    "t = {t:5.2} s  {}  ({} bodies, {} awake ({} coarse, {} half rate, {} fine), {} regions, step {:.2} ms)",
                    path.display(),
                    s.bodies,
                    s.awake,
                    s.coarse_awake,
                    s.half_rate_awake,
                    s.fine_awake,
                    s.regions,
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
