//! Renders the float test (`demo::spawn_floaters`) in a concrete pool, headless, and saves
//! screenshots as the things dropped in settle, then as they ride the water down and up again
//! when the pool is drained 1 m and filled again (asleep by then: the water wakes them).
//!
//! ```sh
//! cargo run -p engine_physics --release --example float_test -- [out_dir] [width]x[height]
//! ```
//!
//! The pool's water is a plain `Water` provider (still, surface at y = 0 but for the drain) drawn as a clear
//! sheet, so what floats under the surface shows; in the showcase the float test sits on the
//! water module's lake instead. `PHYSICS_DEBUG=1` draws the physics debug view too.
//!
//! Needs a GPU or a software Vulkan driver (lavapipe: `apt-get install mesa-vulkan-drivers`).

use std::sync::{Arc, Mutex};

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsDebug, PhysicsPlugin, demo};
use engine_render::prelude::*;
use engine_render::{MaterialPreset, RenderDisabled, Renderer, capture_now};
use engine_world::physics::{Collider, RigidBody};
use engine_world::{Water, WaterQuery, WaterSample, WorldPlugin};

/// The pool's inside, half extents (x, z), meters; its floor depth and how far its coping
/// stands above the water.
const POOL: [f64; 2] = [8.5, 5.5];
const DEPTH: f64 = 2.5;
const COPING: f64 = 0.35;
const WALL: f64 = 0.4;

/// Still water filling the pool, its surface at `level` meters.
struct Pool(Arc<Mutex<f64>>);

impl WaterQuery for Pool {
    fn sample(&self, p: DVec3) -> Option<WaterSample> {
        let level = *self.0.lock().unwrap();
        (p.x.abs() < POOL[0] && p.z.abs() < POOL[1]).then_some(WaterSample {
            surface_height: level,
            depth: (DEPTH + level) as f32,
            velocity: Vec3::ZERO,
            normal: Vec3::Y,
        })
    }
}

/// The pool's water level at world time `t`: full until everything has settled (and fallen
/// asleep), then drained 1 m and filled again, which the sleeping floaters ride down and up.
fn level_at(t: f32) -> f64 {
    let ramp = |from: f32, to: f32| ((t - from) / (to - from)).clamp(0.0, 1.0) as f64;
    -DRAIN * (ramp(27.0, 32.0) - ramp(40.0, 45.0))
}
const DRAIN: f64 = 1.0;

/// (world seconds, file name, camera position, look-at target).
const SHOTS: &[(f32, &str, [f64; 3], [f64; 3])] = &[
    (0.05, "30_float_drop", OVERVIEW.0, OVERVIEW.1),
    (0.7, "31_float_splash", OVERVIEW.0, OVERVIEW.1),
    (2.5, "32_float_tipping", OVERVIEW.0, OVERVIEW.1),
    (25.0, "33_float_settled", OVERVIEW.0, OVERVIEW.1),
    (
        25.5,
        "34_float_waterline",
        [-7.6, 0.32, 4.6],
        [1.5, -0.05, -1.2],
    ),
    (
        26.0,
        "35_float_back_row",
        [2.0, 1.6, 3.2],
        [0.5, -0.2, -2.0],
    ),
    (
        26.5,
        "36_float_front_row",
        [-3.0, 1.5, 5.2],
        [1.2, -0.5, 1.6],
    ),
    (36.0, "37_float_drained", OVERVIEW.0, OVERVIEW.1),
    (50.0, "38_float_refilled", OVERVIEW.0, OVERVIEW.1),
];

/// Where the overview shots look from and at.
const OVERVIEW: ([f64; 3], [f64; 3]) = ([-7.5, 4.2, 9.5], [0.3, -0.6, -0.3]);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).cloned().unwrap_or("physics_shots".into()));
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
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
            centers_of_mass: true,
            radius_m: 40.0,
            ..Default::default()
        };
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    let level = Arc::new(Mutex::new(0.0));
    app.world
        .insert_resource(Water(Arc::new(Pool(level.clone()))));
    app.world.resource_mut::<AmbientLight>().brightness = 400.0;
    let camera = app
        .world
        .spawn((
            Camera::perspective(50.0),
            Transform::from_xyz(-7.5, 4.2, 9.5),
        ))
        .id();
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.4, -0.7, -0.6), Vec3::Y),
    ));
    let sheets = pool(&mut app.world);
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_floaters(&mut commands, DVec3::ZERO);
    });

    let mut t = 0.0f32;
    for (at, name, eye, target) in SHOTS {
        let mut frames = 0;
        let mut until = t;
        while until < at - 4.0 / 60.0 {
            frames += 1;
            until += 1.0 / 60.0;
        }
        for _ in 0..frames {
            frame(&mut app, &level, &sheets, &mut t, false);
        }
        {
            let mut tr = app.world.get_mut::<Transform>(camera).unwrap();
            *tr = Transform::from_translation(DVec3::from_array(*eye))
                .looking_at(DVec3::from_array(*target), Vec3::Y);
        }
        for _ in 0..4 {
            frame(&mut app, &level, &sheets, &mut t, true);
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let suffix = if debug { "_debug" } else { "" };
        let path = out.join(format!("{name}{suffix}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("t = {t:5.2} s  {}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}

/// One frame at world time `t`: the water at its level (its sheets with it), then a step,
/// drawn or not.
fn frame(app: &mut App, level: &Mutex<f64>, sheets: &[(Entity, f64)], t: &mut f32, draw: bool) {
    let y = level_at(*t);
    *level.lock().unwrap() = y;
    for &(e, depth) in sheets {
        app.world.get_mut::<Transform>(e).unwrap().translation.y = y + depth;
    }
    if draw {
        app.update();
    } else {
        simulate(app, 1);
    }
    *t += 1.0 / 60.0;
}

/// A concrete pool sunk in a paved apron: floor, four walls (colliders and drawn), the apron
/// around it, and the water as a clear sheet at y = 0. Returns the water's sheets, each with
/// its depth below the surface.
fn pool(world: &mut World) -> Vec<(Entity, f64)> {
    let concrete = StandardMaterial::preset(MaterialPreset::Concrete).with_triplanar_world();
    let [px, pz] = POOL;
    let slab = |world: &mut World, center: DVec3, half: Vec3| {
        world.spawn((
            Transform::from_translation(center),
            RigidBody::fixed(),
            Collider::cuboid(half),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: (half * 2.0).into(),
                },
                concrete,
            ),
        ));
    };
    let floor_top = -DEPTH;
    slab(
        world,
        DVec3::new(0.0, floor_top - 0.25, 0.0),
        Vec3::new((px + WALL) as f32, 0.25, (pz + WALL) as f32),
    );
    let wall_h = (DEPTH + COPING) * 0.5;
    let wall_y = floor_top + wall_h;
    for s in [-1.0, 1.0] {
        slab(
            world,
            DVec3::new(0.0, wall_y, s * (pz + WALL * 0.5)),
            Vec3::new((px + WALL) as f32, wall_h as f32, (WALL * 0.5) as f32),
        );
        slab(
            world,
            DVec3::new(s * (px + WALL * 0.5), wall_y, 0.0),
            Vec3::new((WALL * 0.5) as f32, wall_h as f32, pz as f32),
        );
    }
    // A paved apron around the pool, flush with the coping, in a lawn.
    let apron = StandardMaterial::preset(MaterialPreset::Cobblestone).with_triplanar_world();
    let reach = 5.0f32;
    let (ox, oz) = ((px + WALL) as f32, (pz + WALL) as f32);
    for s in [-1.0f32, 1.0] {
        world.spawn((
            Transform::from_xyz(0.0, COPING - 0.1, (s * (oz + reach * 0.5)) as f64),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [(ox + reach) * 2.0, 0.2, reach],
                },
                apron,
            ),
        ));
        world.spawn((
            Transform::from_xyz((s * (ox + reach * 0.5)) as f64, COPING - 0.1, 0.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [reach, 0.2, oz * 2.0],
                },
                apron,
            ),
        ));
    }
    // The lawn beyond the apron: four strips around the paving's outer edge.
    let lawn = StandardMaterial::preset(MaterialPreset::Grass).with_triplanar_world();
    let (ex, ez, far) = (ox + reach, oz + reach, 300.0f32);
    for s in [-1.0f32, 1.0] {
        for (center, size) in [
            ([0.0, s * (ez + far * 0.5)], [(ex + far) * 2.0, far]),
            ([s * (ex + far * 0.5), 0.0], [far, ez * 2.0]),
        ] {
            world.spawn((
                Transform::from_xyz(center[0] as f64, COPING - 0.02, center[1] as f64),
                MeshRenderer::new(
                    Shape::Plane {
                        size,
                        subdivisions: 1,
                    },
                    lawn,
                ),
            ));
        }
    }
    // The water: a clear sheet at the surface, and two fainter ones deeper down, so the
    // deeper something is the bluer it looks (as light is absorbed).
    let sheet = |world: &mut World, y: f64, color: u32, alpha: f32, roughness: f32| {
        let e = world.spawn((
            Transform::from_xyz(0.0, y, 0.0),
            MeshRenderer::new(
                Shape::Plane {
                    size: [(px * 2.0) as f32, (pz * 2.0) as f32],
                    subdivisions: 1,
                },
                StandardMaterial::hex(color)
                    .with_alpha(alpha)
                    .with_roughness(roughness),
            ),
        ));
        (e.id(), y)
    };
    vec![
        sheet(world, 0.0, 0x1f6573, 0.32, 0.02),
        sheet(world, -0.9, 0x0f4b58, 0.22, 0.6),
        sheet(world, -1.8, 0x0b3f4c, 0.28, 0.6),
    ]
}

/// Run `frames` frames without drawing them (see the proving ground example).
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
