//! Renders the virtual geometry gallery: a dense procedural granite boulder (1.3 million
//! triangles), a field of 60 of its instances and a 1.3 million triangle sandstone mesa, up
//! close, far away, and in the cluster and level-of-detail debug views.
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
use engine_render::{DebugView, RenderDisabled, RenderStats, Renderer, capture_now};

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

/// A granite boulder about 2.8 × 1.8 × 2.2 m (1,310,720 triangles): broken into flat facets
/// with sharp edges, crested, cracked and grainy.
fn boulder() -> Mesh {
    // Fracture planes (unit normal, distance from the center).
    let cuts: Vec<(Vec3, f32)> = (0..9)
        .map(|k| {
            let a = k as f32 * 2.399_963;
            let y = 1.0 - 2.0 * (k as f32 + 0.5) / 9.0;
            let s = (1.0 - y * y).sqrt();
            (
                Vec3::new(a.cos() * s, y, a.sin() * s),
                0.82 + 0.1 * hash(k, 3, 7),
            )
        })
        .collect();
    let mut m = engine_render::mesh::icosphere(1.0, 8);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let mut r =
            1.0 + 0.3 * (fbm(p * 1.3, 4) - 0.5) + 0.07 * ridged(p * 2.6 + Vec3::splat(11.0), 5);
        for &(n, d) in &cuts {
            let c = p.dot(n);
            if c > 0.05 {
                r = r.min(d / c + 0.04 * (fbm(p * 6.0, 2) - 0.5));
            }
        }
        // Weathered crests, pits and grain.
        r += 0.035 * ridged(p * 6.0 + Vec3::splat(2.0), 4);
        r -= 0.02 * fbm(p * 14.0, 3).powf(3.0);
        r += 0.006 * (fbm(p * 45.0, 3) - 0.5);
        v.position = (p * r * Vec3::new(1.4, 0.9, 1.1)).to_array();
    }
    m.compute_normals();
    m
}

/// A sandstone mesa about 40 m across and 9 m high (1,310,720 triangles): layered ledges,
/// gullies and weathered faces.
fn outcrop() -> Mesh {
    let mut m = engine_render::mesh::icosphere(1.0, 8);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        // Strata: ledges about every 1.2 m of height, wavy and broken up.
        let layer = p.y * 7.5 + 0.8 * fbm(p * 2.0, 3);
        let ledge = (layer - layer.floor()).powf(4.0);
        let r = 1.0 + 0.25 * (fbm(p * 1.2, 4) - 0.5) - 0.12 * ridged(p * 3.0, 4)
            + 0.035 * ledge
            + 0.01 * (fbm(p * 30.0, 3) - 0.5);
        // Flat top: a mesa.
        let mut q = p * r;
        q.y = q.y.min(0.55 + 0.03 * fbm(p * 4.0, 2));
        v.position = (q * Vec3::new(20.0, 16.0, 14.0)).to_array();
    }
    m.compute_normals();
    m
}

/// A point `gap` meters outside the surface of `mesh` (placed by `transform`), on the line from
/// its center toward `eye`.
fn near_surface(mesh: &Mesh, transform: &Transform, eye: DVec3, gap: f64) -> DVec3 {
    let center = transform.translation;
    let dir = (eye - center).normalize();
    let surface = mesh
        .vertices
        .iter()
        .map(|v| transform.transform_point(Vec3::from(v.position).as_dvec3()) - center)
        // Vertices within about 3 degrees of the line.
        .filter(|p| p.normalize().dot(dir) > 0.9986)
        .map(|p| p.dot(dir))
        .fold(0.0, f64::max);
    center + dir * (surface + gap)
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
    // Until the clusters are built the rocks are drawn instanced: skip the sky occlusion map
    // (and spawn the sun later) so that waiting stays cheap.
    app.world.resource_mut::<RenderSettings>().sky_occlusion = false;

    let boulder_mesh = boulder();
    let hero_transform = Transform::from_xyz(0.0, 1.2, 0.0)
        .with_rotation(Quat::from_rotation_y(0.6))
        .with_scale(Vec3::splat(2.2));
    let hero_eye = DVec3::new(-3.4, 2.4, 4.6);
    let detail_eye = near_surface(&boulder_mesh, &hero_transform, hero_eye, 1.3);
    let (boulder, outcrop) = {
        let mut meshes = app.world.resource_mut::<Assets<Mesh>>();
        (meshes.add(boulder_mesh), meshes.add(outcrop()))
    };
    let granite = StandardMaterial::color(0.17, 0.165, 0.155)
        .with_roughness(0.8)
        .with_detail(0.5, 0.03);
    let sandstone = StandardMaterial::color(0.46, 0.33, 0.23)
        .with_roughness(0.9)
        .with_detail(0.6, 0.12);
    let w = &mut app.world;
    // Looking at the sky while waiting.
    w.spawn((
        Camera::perspective(55.0),
        Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::new(0.0, 50.0, 0.0), Vec3::Z),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [800.0, 800.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.24, 0.2, 0.15)
                .with_roughness(0.95)
                .with_detail(0.8, 0.3)
                .with_detail_in_world(),
        ),
    ));
    // The hero boulder, two meters tall, and a field of boulders of every size around it,
    // half sunk into the ground.
    w.spawn((hero_transform, MeshRenderer::new(boulder, granite)));
    for k in 0..60 {
        let a = k as f64 * 2.399_963;
        let r = 9.0 + 9.0 * (k as f64).sqrt();
        let scale = 0.35 + 2.0 * (((k * 7919) % 100) as f32 / 100.0).powf(2.0);
        let rotation =
            Quat::from_rotation_y(a as f32 * 3.0) * Quat::from_rotation_x((k % 5) as f32 * 0.5);
        w.spawn((
            Transform::from_xyz(a.cos() * r, 0.4 * scale as f64, a.sin() * r - 10.0)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(scale)),
            MeshRenderer::new(boulder, granite),
        ));
    }
    w.spawn((
        Transform::from_xyz(-10.0, 0.0, -75.0).with_rotation(Quat::from_rotation_y(0.4)),
        MeshRenderer::new(outcrop, sandstone),
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
    app.world.resource_mut::<RenderSettings>().sky_occlusion = true;
    // Low afternoon sun from the left: raking light shows the relief.
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.75, -0.32, -0.55), Vec3::Y),
    ));

    let hero = (hero_eye, DVec3::new(0.0, 1.0, 0.0));
    let detail = (detail_eye, DVec3::new(0.0, 1.0, 0.0));
    let far = (DVec3::new(30.0, 16.0, 75.0), DVec3::new(-5.0, 2.0, -25.0));
    let shots = [
        ("01_boulder", hero, DebugView::None),
        ("02_boulder_detail", detail, DebugView::None),
        ("03_field_far", far, DebugView::None),
        ("04_boulder_clusters", hero, DebugView::Clusters),
        ("05_field_far_clusters", far, DebugView::Clusters),
        ("06_field_far_levels", far, DebugView::ClusterLevels),
    ];
    for (name, (eye, target), debug) in shots {
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
