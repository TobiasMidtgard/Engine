//! Times boulders tumbling onto and resting on a bumpy triangle-mesh floor cut into 4 m
//! chunks (as a voxel cliff's are: 0.25 m cells), headless. Big hulls tilted on a fine mesh
//! are the case of a contact manifold per triangle under their box.
//!
//! ```sh
//! cargo run -p engine_physics --release --example mesh_rocks_bench -- [boulders] [seconds]
//! ```
//!
//! `PHYSICS_AXIS_CACHE=0` turns off skipping pairs a cached axis holds apart
//! (`mesh_contacts`), for an A/B. Prints the mean, p95 and slowest step; with
//! `PHYSICS_REPORT=N PHYSICS_STEP_LOG=<file>` the narrow phase of each step goes to the file.
//! `MESH_ROCKS_STEPS=<file>` writes each step's time (ms), one per line, for a chart.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsSettings, PhysicsStats};
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{SimulationFocus, WorldPlugin};
use std::time::Instant;

/// Cells along a chunk's side, and the cell size (m).
const CELLS: usize = 16;
const CELL_M: f32 = 0.25;
/// Chunks along each side of the floor.
const CHUNKS: usize = 6;

fn main() {
    let mut args = std::env::args().skip(1);
    let boulders: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(150);
    let seconds: f32 = args.next().and_then(|n| n.parse().ok()).unwrap_or(12.0);
    if std::env::var_os("PHYSICS_REPORT").is_some() {
        engine_core::log::init("info");
    }
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.resource_mut::<PhysicsSettings>().step_budget_ms = None;
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 2.0, 0.0)),
        SimulationFocus::default(),
    ));
    let side = CHUNKS as f32 * CELLS as f32 * CELL_M;
    let height = |x: f32, z: f32| {
        0.12 * (x * 2.1).sin() * (z * 1.7).cos() + 0.06 * (x * 5.3 + z * 4.1).sin()
            - 0.04 * (x * 0.6).cos() * (z * 0.9).sin()
    };
    for cz in 0..CHUNKS {
        for cx in 0..CHUNKS {
            let x0 = cx as f32 * CELLS as f32 * CELL_M - side * 0.5;
            let z0 = cz as f32 * CELLS as f32 * CELL_M - side * 0.5;
            let mut vertices = Vec::new();
            for j in 0..=CELLS {
                for i in 0..=CELLS {
                    let (x, z) = (x0 + i as f32 * CELL_M, z0 + j as f32 * CELL_M);
                    vertices.push(Vec3::new(x, height(x, z), z));
                }
            }
            let mut indices = Vec::new();
            let at = |i: usize, j: usize| (j * (CELLS + 1) + i) as u32;
            for j in 0..CELLS {
                for i in 0..CELLS {
                    indices.push([at(i, j), at(i, j + 1), at(i + 1, j)]);
                    indices.push([at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)]);
                }
            }
            app.world.spawn((
                Transform::IDENTITY,
                RigidBody::fixed(),
                Collider {
                    shape: ColliderShape::TriMesh {
                        vertices: vertices.into(),
                        indices: indices.into(),
                    },
                    ..Collider::cuboid(Vec3::ONE)
                },
            ));
        }
    }
    let mut seed = 977u64;
    let mut rnd = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f64 / (1u64 << 31) as f64) as f32
    };
    let across = (boulders as f32).sqrt().ceil() as usize;
    let spacing = (side - 4.0) / across as f32;
    for b in 0..boulders {
        let (i, j) = (b % across, b / across);
        let at = DVec3::new(
            (i as f32 * spacing - side * 0.5 + 2.0) as f64,
            1.5 + rnd() as f64 * 1.5,
            (j as f32 * spacing - side * 0.5 + 2.0) as f64,
        );
        let r = 0.25 + 0.75 * rnd();
        let size = Vec3::new(r, r * (0.5 + 0.4 * rnd()), r * (0.7 + 0.3 * rnd()));
        let points: Vec<Vec3> = (0..14)
            .map(|_| {
                Vec3::new(rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0).normalize()
                    * size
            })
            .collect();
        app.world.spawn((
            Transform::from_translation(at).with_rotation(Quat::from_euler(
                engine_core::math::EulerRot::XYZ,
                rnd() * 6.0,
                rnd() * 6.0,
                rnd() * 6.0,
            )),
            RigidBody::dynamic(),
            Collider {
                shape: ColliderShape::ConvexHull {
                    points: points.into(),
                },
                ..Collider::cuboid(Vec3::ONE)
            }
            .with_density(2600.0),
            Velocity::default(),
        ));
    }
    // Let the floor's meshes build (off the frame) before timing.
    app.update_n(30);
    let frames = (seconds * 60.0) as usize;
    let mut steps = Vec::with_capacity(frames);
    let start = Instant::now();
    for _ in 0..frames {
        app.update();
        steps.push(app.world.resource::<PhysicsStats>().step_ms);
    }
    if let Some(path) = std::env::var_os("MESH_ROCKS_STEPS") {
        let lines: Vec<String> = steps.iter().map(|ms| format!("{ms:.3}")).collect();
        std::fs::write(path, lines.join("\n") + "\n").expect("write the step times");
    }
    let mean = steps.iter().sum::<f64>() / steps.len() as f64;
    let mut sorted = steps.clone();
    sorted.sort_by(f64::total_cmp);
    let s = *app.world.resource::<PhysicsStats>();
    println!(
        "mesh_rocks_bench: {boulders} boulders on {} triangles, {seconds} s: step mean {mean:.2} ms, p95 {:.2}, slowest {:.2}; {} awake at the end; {:.1} s wall",
        CHUNKS * CHUNKS * CELLS * CELLS * 2,
        sorted[sorted.len() * 95 / 100],
        sorted[sorted.len() - 1],
        s.awake,
        start.elapsed().as_secs_f64(),
    );
}
