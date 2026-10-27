//! CPU cost of drawing many instanced meshes: `count` entities (50 000 by default) using 200
//! meshes and 20 materials, scattered over a 600 m square under a sun with shadows, a quarter of
//! them turning every frame. Runs `frames` frames at a small resolution and prints the render
//! system's CPU steps (`render.cpu.*`, averaged), the mesh feature's own steps among them, so a
//! change to the gather or submit shows its gain. Needs a GPU or lavapipe.
//!
//! ```sh
//! cargo run -p engine_render --release --example mesh_gather -- 50000 300
//! ```
//!
//! A third number sets the frames run before timing (60), a fourth the number of distinct meshes
//! (200; thousands make a draw-heavy scene like the showcase fortress). `--single-draws` turns multi-draw
//! off (`RenderSettings::multi_draw_indirect`), for a before and after.

use std::collections::BTreeMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::mesh::{cuboid, icosphere};
use engine_render::prelude::*;
use engine_render::{Mesh, RenderDisabled, RenderStats};

const MESHES: usize = 200;
const MATERIALS: usize = 20;
const EXTENT: f64 = 600.0;

/// Turns a little every frame (a quarter of the entities).
#[derive(Component)]
struct Spin(f32);

fn spin(time: Res<Time>, mut q: Query<(&mut Transform, &Spin)>) {
    let dt = time.delta_secs();
    for (mut t, s) in &mut q {
        t.rotate_y(s.0 * dt);
    }
}

fn main() {
    let mut args = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .map(|a| {
            a.parse::<usize>()
                .expect("numbers: count frames settle meshes")
        });
    let count = args.next().unwrap_or(50_000);
    let frames = args.next().unwrap_or(300);
    let settle = args.next().unwrap_or(60);
    let mesh_count = args.next().unwrap_or(MESHES).max(1);
    engine_core::log::init("warn");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 480;
        w.height = 270;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<RenderSettings>()
        .multi_draw_indirect = !std::env::args().any(|a| a == "--single-draws");
    app.add_systems(Update, spin);

    let meshes: Vec<Handle<Mesh>> = {
        let mut assets = app.world.resource_mut::<Assets<Mesh>>();
        (0..mesh_count)
            .map(|i| {
                let size = 0.5 + (i % 7) as f32 * 0.4;
                assets.add(if i % 2 == 0 {
                    icosphere(size * 0.5, 1 + (i % 3) as u32)
                } else {
                    cuboid(Vec3::new(size, size * 1.6, size * 0.8))
                })
            })
            .collect()
    };
    let materials: Vec<Handle<StandardMaterial>> = {
        let mut assets = app.world.resource_mut::<Assets<StandardMaterial>>();
        (0..MATERIALS)
            .map(|i| {
                let c = 0x404040 + (i as u32 * 0x0a0703);
                assets.add(StandardMaterial::hex(c).with_roughness(0.4 + 0.02 * i as f32))
            })
            .collect()
    };
    // A fixed scatter (a hash of the index), so every run draws the same scene.
    let hash = |i: u64, salt: u64| {
        let mut x = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ salt;
        x ^= x >> 31;
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x ^= x >> 29;
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    for i in 0..count as u64 {
        let x = (hash(i, 1) - 0.5) * EXTENT;
        let z = (hash(i, 2) - 0.5) * EXTENT;
        let transform = Transform::from_xyz(x, 1.0, z).with_rotation(Quat::from_rotation_y(
            hash(i, 3) as f32 * std::f32::consts::TAU,
        ));
        let mesh = meshes[(hash(i, 4) * mesh_count as f64) as usize % mesh_count];
        let material = materials[(hash(i, 5) * MATERIALS as f64) as usize % MATERIALS];
        let mut e = app
            .world
            .spawn((transform, MeshRenderer::new(mesh, material)));
        if i % 4 == 0 {
            e.insert(Spin(0.5 + hash(i, 6) as f32));
        }
    }
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -1.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [EXTENT as f32, EXTENT as f32],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6f6a62).with_roughness(0.9),
        ),
    ));
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 25.0, EXTENT * 0.5).looking_at(DVec3::new(0.0, 0.0, 0.0), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.4, -0.8, -0.3), Vec3::Y),
    ));

    // Settle: uploads, levels of detail, impostors, pipelines.
    let start = std::time::Instant::now();
    app.update();
    let first = start.elapsed();
    // The first frame's slowest steps (uploads, pipeline compiles).
    let mut steps: Vec<(String, f64)> = app
        .world
        .resource::<Diagnostics>()
        .metrics
        .iter()
        .filter_map(|(name, v)| Some((name.strip_prefix("render.cpu.")?.to_string(), *v)))
        .collect();
    steps.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("first frame:");
    for (step, ms) in steps.iter().take(8) {
        println!("  {ms:>8.1} ms  {step}");
    }
    app.update_n(settle);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU ({}), nothing measured", d.0);
        return;
    }

    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    let mut counts: BTreeMap<String, f64> = BTreeMap::new();
    let start = std::time::Instant::now();
    for _ in 0..frames {
        app.update();
        let diagnostics = app.world.resource::<Diagnostics>();
        for (name, v) in &diagnostics.metrics {
            if let Some(step) = name.strip_prefix("render.cpu.") {
                *sums.entry(step.to_string()).or_default() += v;
            } else if name.starts_with("render.meshes.")
                || name.starts_with("render.record")
                || name == "render.submit_ms"
                || name == "render.finish_ms"
                || name == "render.wait_ms"
                || name == "render.extract_ms"
            {
                *counts.entry(name.clone()).or_default() += v;
            }
        }
    }
    let frame_ms = start.elapsed().as_secs_f64() * 1e3 / frames as f64;
    let stats = app.world.resource::<RenderStats>();
    println!(
        "{count} entities, {frames} frames: {frame_ms:.2} ms a frame (first frame {:.0} ms); \
         {} draws, {} instances",
        first.as_secs_f64() * 1e3,
        stats.draw_calls,
        stats.instances
    );
    for (name, total) in &counts {
        println!("  {name}: {:.2} (average)", total / frames as f64);
    }
    let mut rows: Vec<_> = sums.into_iter().collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (step, total) in rows.iter().take(16) {
        println!("  {:>8.3} ms  {step}", total / frames as f64);
    }
}
