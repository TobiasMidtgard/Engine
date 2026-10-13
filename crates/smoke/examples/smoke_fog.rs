//! A fog bank at dawn: a valley filled with ground fog up to a level, wooded hills standing
//! out of it, the low sun lighting its top. Renders a few views into PNGs, without a window.
//!
//! ```text
//! cargo run --release -p engine_smoke --example smoke_fog -- [out_dir] [views] [width] [height] [hour]
//! ```
//!
//! `views` is a comma-separated subset of the view keys (default: all). The valley is a
//! `Ground`, so the fog's volume finds its floor the way it does on real terrain. Needs a GPU or
//! a software Vulkan driver (lavapipe). Time runs in fixed 1/30 s steps, so the pictures are
//! the same every run.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{Mesh, RenderDisabled, Vertex, capture_now};
use engine_smoke::{FogBank, SmokePlugin, SmokeVolume};
use engine_world::{Ground, GroundQuery, GroundSample, SurfaceId, WeatherState, WorldPlugin};

/// Ground height: a valley winding along Z, its floor gently rolling around 0, hills rising
/// about 40 m on both sides.
fn land(x: f64, z: f64) -> f64 {
    let u = (x - 10.0 * (z / 60.0).sin()).abs();
    let t = ((u - 14.0) / 95.0).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    42.0 * s + 3.0 * (x * 0.05).sin() * (z * 0.043).cos() * s + 1.2 * (z * 0.021).sin()
}

struct Valley;

impl GroundQuery for Valley {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let e = 0.5;
        let dx = (land(p.x + e, p.z) - land(p.x - e, p.z)) / (2.0 * e);
        let dz = (land(p.x, p.z + e) - land(p.x, p.z - e)) / (2.0 * e);
        Some(GroundSample {
            height: land(p.x, p.z),
            normal: Vec3::new(-dx as f32, 1.0, -dz as f32).normalize(),
            surface: SurfaceId(3),
        })
    }
}

/// The valley as a mesh, meadow green on the floor and gentle slopes, browner and rockier
/// where it is steep.
fn valley_mesh() -> Mesh {
    let (x0, x1, z0, z1, step) = (-800.0, 800.0, -800.0, 800.0, 6.0);
    let nx = ((x1 - x0) / step) as u32 + 1;
    let nz = ((z1 - z0) / step) as u32 + 1;
    let meadow = Vec4::new(0.08, 0.16, 0.05, 1.0);
    let rough = Vec4::new(0.22, 0.2, 0.14, 1.0);
    let mut vertices = Vec::with_capacity((nx * nz) as usize);
    for j in 0..nz {
        for i in 0..nx {
            let x = x0 + i as f64 * step;
            let z = z0 + j as f64 * step;
            let e = 1.0;
            let slope = Vec2::new(
                ((land(x + e, z) - land(x - e, z)) / (2.0 * e)) as f32,
                ((land(x, z + e) - land(x, z - e)) / (2.0 * e)) as f32,
            )
            .length();
            let n = ((x * 0.37).sin() * (z * 0.29).cos()) as f32 * 0.5 + 0.5;
            let k = ((slope - 0.25) * 3.0).clamp(0.0, 1.0) * 0.7 + n * 0.3;
            vertices.push(
                Vertex::new(
                    Vec3::new(x as f32, land(x, z) as f32, z as f32),
                    Vec3::Y,
                    Vec2::new(x as f32, z as f32) * 0.1,
                )
                .with_color(meadow.lerp(rough, k)),
            );
        }
    }
    let mut indices = Vec::with_capacity(((nx - 1) * (nz - 1) * 6) as usize);
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = j * nx + i;
            let (b, c, d) = (a + 1, a + nx, a + nx + 1);
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mut mesh = Mesh::new(vertices, indices);
    mesh.compute_normals();
    mesh
}

/// A conifer standing on the ground at `foot`: a trunk under tiers of drooping boughs,
/// narrowing to the tip.
fn conifer(
    w: &mut World,
    rng: &mut Rng,
    foot: DVec3,
    tall: f32,
    wide: f32,
    trunk: StandardMaterial,
    needles: StandardMaterial,
) {
    w.spawn((
        Transform::from_translation(foot + DVec3::Y * (tall * 0.12) as f64),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: wide * 0.08,
                height: tall * 0.3,
                segments: 8,
            },
            trunk,
        ),
    ));
    let tiers = 6;
    for k in 0..tiers {
        let f = k as f32 / tiers as f32;
        w.spawn((
            Transform::from_translation(foot + DVec3::Y * (tall * (0.22 + 0.68 * f)) as f64)
                .with_rotation(Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU))),
            MeshRenderer::new(
                Shape::Cone {
                    radius: wide * (1.0 - 0.8 * f),
                    height: tall * (0.3 - 0.12 * f),
                    segments: 18,
                },
                needles,
            ),
        ));
    }
}

/// (key, eye, target).
const VIEWS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("fog_dawn", [92.0, 62.0, -96.0], [-20.0, 8.0, 40.0]),
    ("fog_valley", [-6.0, 46.0, -150.0], [4.0, 6.0, 40.0]),
    ("fog_inside", [4.0, 1.8, -40.0], [4.0, 3.0, 0.0]),
    ("fog_above", [30.0, 26.0, -60.0], [0.0, 8.0, 20.0]),
];

/// Unit vector toward the sun at `hour` (latitude 35°, midsummer, east = −X, north = +Z).
fn sun_direction(hour: f32) -> Vec3 {
    let (lat, decl) = (35.0f32.to_radians(), 23.44f32.to_radians());
    let h = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    let up = lat.sin() * decl.sin() + lat.cos() * decl.cos() * h.cos();
    let east = -decl.cos() * h.sin();
    let north = lat.cos() * decl.sin() - lat.sin() * decl.cos() * h.cos();
    Vec3::new(-east, up, north).normalize_or(Vec3::Y)
}

/// Put `schedule` in as the render schedule and return the one it replaces.
fn swap_render(app: &mut App, schedule: Schedule) -> Schedule {
    let mut schedules = app.world.resource_mut::<Schedules>();
    let old = schedules.remove(Render).expect("render schedule");
    schedules.insert(schedule);
    old
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshots/smoke".into());
    let only: Option<Vec<String>> = args
        .get(2)
        .filter(|s| !s.is_empty() && s.as_str() != "all")
        .map(|s| s.split(',').map(str::to_string).collect());
    let width: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(720);
    let hour: f32 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(7.2);
    std::fs::create_dir_all(&out).expect("output directory");

    let started = std::time::Instant::now();
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
    app.add_plugins((RenderPlugin, WorldPlugin, SmokePlugin));
    app.world.insert_resource(Ground(Arc::new(Valley)));
    let sun_dir = sun_direction(hour);
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::new(0.4, 0.0, 1.2);
        weather.time_of_day_h = hour;
        weather.sun_direction = sun_dir;
        weather.temperature_c = 9.0;
        weather.humidity = 0.97;
    }

    let mesh = app
        .world
        .resource_mut::<engine_assets::Assets<Mesh>>()
        .add(valley_mesh());
    let w = &mut app.world;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(mesh, StandardMaterial::hex(0xffffff).with_roughness(0.9)),
    ));
    // Conifers on the slopes, some standing in the fog, most above it.
    let mut rng = Rng::new(0xF06);
    let trunk = StandardMaterial::hex(0x3b2a1e).with_roughness(0.9);
    let needles = StandardMaterial::hex(0x1d3320).with_roughness(0.85);
    for _ in 0..700 {
        let x = rng.range(-230.0, 230.0) as f64;
        let z = rng.range(-230.0, 300.0) as f64;
        let h = land(x, z);
        let u = (x - 10.0 * (z / 60.0).sin()).abs();
        if u < 22.0 || rng.f32() > ((u - 22.0) / 30.0).clamp(0.0, 1.0) as f32 {
            continue;
        }
        let tall = rng.range(9.0, 17.0);
        let wide = tall * rng.range(0.2, 0.27);
        conifer(w, &mut rng, DVec3::new(x, h, z), tall, wide, trunk, needles);
    }
    // A few trees out on the valley floor, standing in the fog: they fade with distance.
    for _ in 0..60 {
        let z = rng.range(-120.0, 200.0) as f64;
        let side = rng.range(3.0, 20.0) as f64 * if rng.f32() < 0.5 { -1.0 } else { 1.0 };
        let x = 10.0 * (z / 60.0).sin() + side;
        // Keep clear of the camera standing in the fog.
        if (x - 4.0).hypot(z + 40.0) < 14.0 {
            continue;
        }
        let h = land(x, z);
        let tall = rng.range(8.0, 14.0);
        let wide = tall * rng.range(0.22, 0.3);
        conifer(w, &mut rng, DVec3::new(x, h, z), tall, wide, trunk, needles);
    }
    // The fog's air: the whole valley floor, 3 m voxels.
    w.spawn((
        Transform::from_xyz(0.0, -2.0, 60.0),
        SmokeVolume {
            size: [240.0, 42.0, 450.0],
            cell_m: 3.0,
        },
        FogBank {
            amount: 1.0,
            visibility_m: 28.0,
            depth_m: 14.0,
            top_fade_m: 2.5,
            ..Default::default()
        },
    ));
    w.spawn((
        DirectionalLight {
            illuminance: 60_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-sun_dir.as_dvec3(), Vec3::Y),
    ));
    let camera = w
        .spawn((
            Camera {
                exposure: Exposure::Auto {
                    min_ev100: -3.0,
                    max_ev100: 18.0,
                    speed: 25.0,
                    compensation: 0.0,
                },
                ..Default::default()
            },
            Transform::from_xyz(0.0, 40.0, -120.0),
        ))
        .id();

    // Let the fog form and drift a while without drawing (much faster on a software GPU).
    let mut render = Some(swap_render(&mut app, Schedule::new(Render)));
    app.update_n(4);
    app.update_n(20 * 30);
    let domains = app.world.resource::<engine_smoke::SmokeDomains>();
    eprintln!(
        "fog formed in {:.1} s: {:.0} kg in {} volume(s)",
        started.elapsed().as_secs_f32(),
        domains.mass_kg(),
        domains.list.len()
    );

    for &(key, eye, target) in VIEWS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|k| k == key)) {
            continue;
        }
        let t0 = std::time::Instant::now();
        *app.world.get_mut::<Transform>(camera).expect("camera") =
            Transform::from_translation(DVec3::from_array(eye))
                .looking_at(DVec3::from_array(target), Vec3::Y);
        let empty = swap_render(&mut app, render.take().expect("render schedule"));
        app.update_n(8);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = format!("{out}/{key}.png");
        match capture_now(&mut app.world, &path) {
            Ok(_) => eprintln!("{path} ({:.1} s)", t0.elapsed().as_secs_f32()),
            Err(e) => eprintln!("{path}: {e}"),
        }
        render = Some(swap_render(&mut app, empty));
    }
}
