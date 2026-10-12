//! Renders the virtual geometry gallery: a dense procedural boulder (1.3 million triangles), a
//! field of its instances and a million-triangle cliff, up close, far away, and in the cluster
//! and level-of-detail debug views.
//!
//! ```text
//! cargo run -p engine_render --release --example virtual_geometry_gallery -- <output dir>
//! ```
//!
//! Needs a GPU or lavapipe. Prints the clusters and triangles drawn for every shot.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{DebugView, RenderDisabled, RenderStats, Renderer, Vertex, capture_now};

fn hash(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 16_777_215.0
}

/// Smooth value noise in [0, 1].
fn noise(p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let s = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx, dy, dz| hash(x + dx, y + dy, z + dz);
    lerp(
        lerp(
            lerp(c(0, 0, 0), c(1, 0, 0), s.x),
            lerp(c(0, 1, 0), c(1, 1, 0), s.x),
            s.y,
        ),
        lerp(
            lerp(c(0, 0, 1), c(1, 0, 1), s.x),
            lerp(c(0, 1, 1), c(1, 1, 1), s.x),
            s.y,
        ),
        s.z,
    )
}

fn fbm(mut p: Vec3, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut total) = (0.0, 0.5, 0.0);
    for _ in 0..octaves {
        sum += amp * noise(p);
        total += amp;
        p = Vec3::new(
            p.y * 1.6 + p.z * 1.2,
            p.z * 1.6 - p.x * 1.2,
            p.x * 1.6 + p.y * 1.2,
        ) + Vec3::splat(5.3);
        amp *= 0.5;
    }
    sum / total
}

/// Sharp-crested noise in [0, 1].
fn ridged(p: Vec3, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut q) = (0.0, 0.5, 0.0, p);
    for _ in 0..octaves {
        let r = 1.0 - (2.0 * noise(q) - 1.0).abs();
        sum += amp * r * r;
        total += amp;
        q = q * 2.1 + Vec3::new(3.1, 7.7, 1.3);
        amp *= 0.5;
    }
    sum / total
}

/// A boulder about 2.8 × 1.8 × 2.2 m (1,310,720 triangles).
fn boulder() -> Mesh {
    let mut m = engine_render::mesh::icosphere(1.0, 8);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let r = 1.0
            + 0.4 * (fbm(p * 1.4, 4) - 0.5)
            + 0.10 * ridged(p * 3.0 + Vec3::splat(11.0), 4)
            + 0.025 * (fbm(p * 22.0, 4) - 0.5);
        v.position = (p * r * Vec3::new(1.4, 0.9, 1.1)).to_array();
    }
    m.compute_normals();
    m
}

/// A cliff face 60 m wide and 24 m high (1,048,576 triangles), facing +Z: layered rock with
/// ledges, gullies and fine cracks.
fn cliff() -> Mesh {
    let (nx, ny) = (1024u32, 512u32);
    let (width, height) = (60.0f32, 24.0f32);
    let mut vertices = Vec::with_capacity(((nx + 1) * (ny + 1)) as usize);
    for j in 0..=ny {
        for i in 0..=nx {
            let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
            let (x, y) = ((u - 0.5) * width, v * height);
            let p = Vec3::new(x, y, 0.0);
            // Strata: terraces every ~1.6 m, broken up by noise.
            let layer = y / 1.6 + 1.5 * fbm(p * 0.08, 3);
            let terrace = (layer - layer.floor()).powf(3.0);
            let gullies = ridged(p * Vec3::new(0.09, 0.03, 1.0), 4);
            let depth = 2.5 * fbm(p * 0.05, 4)
                + 0.9 * terrace
                + 1.6 * gullies
                + 0.12 * ridged(p * 0.9 + Vec3::splat(4.0), 4)
                + 0.03 * fbm(p * 6.0, 3);
            // Lean back toward the top.
            let z = depth - 0.25 * y;
            vertices.push(Vertex::new(Vec3::new(x, y, z), Vec3::Z, Vec2::new(u, v)));
        }
    }
    let mut indices = Vec::with_capacity((nx * ny * 6) as usize);
    let row = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let a = j * row + i;
            indices.extend([a, a + 1, a + row + 1, a, a + row + 1, a + row]);
        }
    }
    let mut m = Mesh::new(vertices, indices);
    m.compute_normals();
    m
}

fn mesh_state(app: &App) -> serde_json::Value {
    let describe = app.world.resource::<Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
        .map(|f| f["state"]["virtual_geometry"].clone())
        .unwrap_or_default()
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "virtual_geometry_gallery".into()),
    );
    std::fs::create_dir_all(&out).expect("output directory");
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.add_plugins(RenderPlugin);

    let (boulder, cliff) = {
        let mut meshes = app.world.resource_mut::<Assets<Mesh>>();
        (meshes.add(boulder()), meshes.add(cliff()))
    };
    let stone = StandardMaterial::color(0.33, 0.30, 0.27)
        .with_roughness(0.9)
        .with_detail(0.6, 0.06);
    let cliff_stone = StandardMaterial::color(0.42, 0.36, 0.30)
        .with_roughness(0.92)
        .with_detail(0.5, 0.15);
    let w = &mut app.world;
    w.spawn((
        Camera::perspective(55.0),
        Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.6, -0.45, -0.65), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.22, 0.2, 0.15)
                .with_roughness(0.95)
                .with_detail(0.8, 0.3)
                .with_detail_in_world(),
        ),
    ));
    // The hero boulder, two meters tall.
    w.spawn((
        Transform::from_xyz(0.0, 1.3, 0.0).with_scale(Vec3::splat(2.2)),
        MeshRenderer::new(boulder, stone),
    ));
    // A field of boulders of every size, half sunk into the ground.
    for k in 0..60 {
        let a = k as f64 * 2.399_963;
        let r = 9.0 + 4.2 * (k as f64).sqrt() * 2.2;
        let scale = 0.4 + 2.2 * (((k * 7919) % 100) as f32 / 100.0).powf(2.0);
        let rotation =
            Quat::from_rotation_y(a as f32 * 3.0) * Quat::from_rotation_x((k % 5) as f32 * 0.4);
        w.spawn((
            Transform::from_xyz(a.cos() * r, 0.45 * scale as f64, a.sin() * r - 10.0)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(scale)),
            MeshRenderer::new(boulder, stone),
        ));
    }
    w.spawn((
        Transform::from_xyz(0.0, -1.0, -45.0),
        MeshRenderer::new(cliff, cliff_stone),
    ));

    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    let started = Instant::now();
    while mesh_state(&app)["meshes_ready"].as_u64().unwrap_or(0) < 2 {
        assert!(
            started.elapsed() < Duration::from_secs(900),
            "builds didn't finish: {}",
            mesh_state(&app)
        );
        std::thread::sleep(Duration::from_millis(50));
        app.update();
    }
    eprintln!("clusters built in {:.1} s", started.elapsed().as_secs_f32());

    let shots: [(&str, DVec3, DVec3, DebugView); 5] = [
        (
            "01_boulder_close",
            DVec3::new(1.2, 2.1, 3.6),
            DVec3::new(0.2, 1.5, 0.0),
            DebugView::None,
        ),
        (
            "02_field_far",
            DVec3::new(-30.0, 14.0, 70.0),
            DVec3::new(0.0, 2.0, -20.0),
            DebugView::None,
        ),
        (
            "03_boulder_close_clusters",
            DVec3::new(1.2, 2.1, 3.6),
            DVec3::new(0.2, 1.5, 0.0),
            DebugView::Clusters,
        ),
        (
            "04_field_far_clusters",
            DVec3::new(-30.0, 14.0, 70.0),
            DVec3::new(0.0, 2.0, -20.0),
            DebugView::Clusters,
        ),
        (
            "05_field_far_levels",
            DVec3::new(-30.0, 14.0, 70.0),
            DVec3::new(0.0, 2.0, -20.0),
            DebugView::ClusterLevels,
        ),
    ];
    for (name, eye, target, debug) in shots {
        {
            let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
            for mut t in q.iter_mut(&mut app.world) {
                *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
            }
        }
        app.world.resource_mut::<RenderSettings>().debug_view = debug;
        // Let exposure and temporal anti-aliasing settle.
        app.update_n(40);
        let path = out.join(format!("{name}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        let stats = app.world.resource::<RenderStats>().clone();
        eprintln!(
            "{name}: {} clusters, {} triangles drawn ({})",
            stats.clusters,
            stats.triangles,
            mesh_state(&app)["stats"]
        );
    }
}
