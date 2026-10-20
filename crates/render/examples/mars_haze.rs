//! Mars' dust haze over terrain stand-ins: procedural cratered ground with dark and bright
//! albedo regions, seen with `Atmosphere::mars()` straight down from 400 km, obliquely from
//! orbit onto a 21 km shield volcano, from a canyon floor 6 km below datum toward walls 15–30
//! km away, and from datum toward a crater rim 30 km away.
//!
//! ```text
//! cargo run -p engine_render --example mars_haze -- <out dir> [prefix] [WxH]
//! ```
//!
//! Writes `<prefix>orbit_down.png`, `<prefix>orbit_volcano.png`, `<prefix>canyon_floor.png`
//! and `<prefix>crater_rim.png`; the size defaults to 960x540. Needs a GPU or a software Vulkan
//! driver (lavapipe).

use std::path::PathBuf;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::mesh::{Mesh, Vertex};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// Frames rendered before the capture: anti-aliasing and the environment settle.
const WARM_UP: usize = 12;
/// Mars' mean sunlight (lux) and the sun's angular size there (degrees).
const SUN_LUX: f32 = 47_000.0;
const SUN_SIZE: f32 = 0.35;

/// One view: the terrain, the camera and where it looks, the sun, the exposure.
struct Shot {
    name: &'static str,
    terrain: Terrain,
    eye: DVec3,
    target: DVec3,
    fov: f32,
    sun_elevation: f64,
    /// Degrees from the view's direction, clockwise seen from above.
    sun_azimuth: f64,
    ev100: f32,
}

#[derive(Clone, Copy)]
enum Terrain {
    /// Cratered plains with dark and bright albedo regions, 400 km across.
    Plains,
    /// A shield volcano 21 km high with a summit caldera and a basal scarp.
    Volcano,
    /// A canyon 6 km deep and about 45 km wide, running along z.
    Canyon,
    /// Plains inside a large crater whose rim rises 2 km about 30 km from the camera.
    CraterRim,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().map(String::as_str).unwrap_or("screenshots"));
    let prefix = args.get(1).cloned().unwrap_or_default();
    let (width, height) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((960, 540));
    std::fs::create_dir_all(&out).expect("output directory");
    let air = Atmosphere::mars();
    // Straight down from 400 km; obliquely onto the volcano's summit from 400 km up.
    let summit = DVec3::new(0.0, 21_000.0, 0.0);
    let zenith = 58f64.to_radians();
    let u = DVec3::new(0.0, zenith.cos(), zenith.sin());
    let oc = summit - air.planet_center;
    let r = air.planet_radius + 400_000.0;
    let b = oc.dot(u);
    let t = -b + (b * b - (oc.length_squared() - r * r)).sqrt();
    let mut shots = [
        Shot {
            name: "orbit_down",
            terrain: Terrain::Plains,
            eye: DVec3::new(0.0, 400_000.0, 0.0),
            target: DVec3::ZERO,
            fov: 36.0,
            sun_elevation: 35.0,
            sun_azimuth: 120.0,
            ev100: 12.0,
        },
        Shot {
            name: "orbit_volcano",
            terrain: Terrain::Volcano,
            eye: summit + u * t,
            target: summit - DVec3::new(0.0, 0.0, 60_000.0),
            fov: 30.0,
            sun_elevation: 30.0,
            sun_azimuth: 70.0,
            ev100: 12.0,
        },
        Shot {
            name: "canyon_floor",
            terrain: Terrain::Canyon,
            eye: DVec3::new(-6_000.0, -5_960.0, 0.0),
            target: DVec3::new(19_000.0, -4_200.0, -18_000.0),
            fov: 60.0,
            sun_elevation: 45.0,
            sun_azimuth: 150.0,
            ev100: 12.0,
        },
        Shot {
            name: "crater_rim",
            terrain: Terrain::CraterRim,
            eye: DVec3::new(0.0, 40.0, 0.0),
            target: DVec3::new(0.0, 900.0, -30_000.0),
            fov: 50.0,
            sun_elevation: 40.0,
            sun_azimuth: 140.0,
            ev100: 12.0,
        },
    ];
    // From the ground, the camera stands 30 m above it.
    for shot in shots.iter_mut().skip(2) {
        let (x, z) = (shot.eye.x as f32, shot.eye.z as f32);
        shot.eye.y = sample(shot.terrain, x, z).0 as f64 + 30.0;
    }
    for shot in &shots {
        let path = out.join(format!("{prefix}{}.png", shot.name));
        let mut app = scene(shot, (width, height));
        app.update_n(WARM_UP);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}): nothing rendered", d.0);
            return;
        }
        capture_now(&mut app.world, &path).expect("screenshot");
        println!("{}", path.display());
    }
}

fn scene(shot: &Shot, (width, height): (u32, u32)) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(RenderPlugin);
    app.world.resource_mut::<RenderSettings>().sky_cloud_layer = false;
    *app.world.resource_mut::<Atmosphere>() = Atmosphere::mars();
    // The atmosphere brings the haze.
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    let view = (shot.target - shot.eye).normalize();
    let up = if view.y.abs() > 0.95 {
        DVec3::NEG_Z
    } else {
        DVec3::Y
    };
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: shot.ev100 },
            ..Camera::perspective(shot.fov)
        },
        Transform::from_translation(shot.eye).looking_at(shot.target, up.as_vec3()),
    ));
    // The sun, by elevation and azimuth from the view's horizontal direction.
    let forward = DVec3::new(view.x, 0.0, view.z).normalize_or(DVec3::NEG_Z);
    let right = forward.cross(DVec3::Y);
    let (e, a) = (
        shot.sun_elevation.to_radians(),
        shot.sun_azimuth.to_radians(),
    );
    let toward_sun = (forward * a.cos() + right * a.sin()) * e.cos() + DVec3::Y * e.sin();
    w.spawn((
        DirectionalLight {
            color: [1.0, 1.0, 1.0],
            illuminance: SUN_LUX,
            angular_size_deg: SUN_SIZE,
            ..Default::default()
        },
        Transform::default().looking_at(-toward_sun, Vec3::Y),
    ));
    let eye = Vec2::new(shot.eye.x as f32, shot.eye.z as f32);
    let mesh = terrain_mesh(shot.terrain, Atmosphere::mars().planet_radius, eye);
    let handle = w.resource_mut::<Assets<Mesh>>().add(mesh);
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            handle,
            StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.95),
        ),
    ));
    app
}

// ------------------------------------------------------------------ procedural terrain

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (u, v) = (s(fx), s(fy));
    let a = hash(ix, iy, seed);
    let b = hash(ix + 1, iy, seed);
    let c = hash(ix, iy + 1, seed);
    let d = hash(ix + 1, iy + 1, seed);
    (a + (b - a) * u) + ((c + (d - c) * u) - (a + (b - a) * u)) * v
}

/// Fractal noise in about -0.5..0.5 at `scale` meters per feature, `octaves` deep.
fn fbm(x: f32, y: f32, scale: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.5, 1.0 / scale);
    for o in 0..octaves {
        sum += (value_noise(x * freq, y * freq, seed + o) - 0.5) * amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum * 1.9
}

/// Bowl-shaped craters with raised rims: height and albedo factor at (x, z).
fn craters(x: f32, z: f32, extent: f32, count: u32, max_radius: f32, seed: u32) -> (f32, f32) {
    let (mut h, mut tint) = (0.0f32, 1.0f32);
    for i in 0..count {
        let cx = (hash(i as i32, 1, seed) - 0.5) * extent;
        let cz = (hash(i as i32, 2, seed) - 0.5) * extent;
        // Many small craters, few large ones.
        let r = max_radius * hash(i as i32, 3, seed).powf(3.0).max(0.03);
        let d = ((x - cx).powi(2) + (z - cz).powi(2)).sqrt() / r;
        if d < 1.0 {
            h += r * (0.12 * (d * d * d * d - 1.0) + 0.035);
            tint *= 0.9 + 0.1 * d;
        } else if d < 2.5 {
            h += r * 0.035 * ((2.5 - d) / 1.5).powi(3);
        }
    }
    (h, tint)
}

/// Mars' regolith: bright dust and dark basaltic sand, linear albedo.
fn albedo(dark: f32) -> Vec3 {
    let bright = Vec3::new(0.34, 0.19, 0.10);
    let basalt = Vec3::new(0.13, 0.085, 0.06);
    bright.lerp(basalt, dark.clamp(0.0, 1.0))
}

/// Height (meters above datum) and albedo at (x, z), with ground-level detail.
fn sample(terrain: Terrain, x: f32, z: f32) -> (f32, Vec3) {
    let (h, color) = sample_large(terrain, x, z);
    let (c, tint) = craters(x, z, 30_000.0, 400, 300.0, 53);
    let bumps = fbm(x, z, 400.0, 5, 59) * 12.0 + c;
    let speckle = 1.0 + fbm(x, z, 150.0, 4, 61) * 0.35;
    (h + bumps, color * speckle * tint)
}

fn sample_large(terrain: Terrain, x: f32, z: f32) -> (f32, Vec3) {
    match terrain {
        Terrain::Plains => {
            let (c, tint) = craters(x, z, 760_000.0, 500, 30_000.0, 7);
            let h = fbm(x, z, 60_000.0, 6, 3) * 900.0 + c;
            // Large dark regions (like Syrtis Major) and streaks.
            let dark = (fbm(x, z, 160_000.0, 5, 11) * 3.0 + 0.1).clamp(0.0, 1.0);
            (h, albedo(dark) * tint)
        }
        Terrain::Volcano => {
            let r = (x * x + z * z).sqrt();
            let shield = (1.0 - r / 300_000.0).max(0.0);
            let mut h = 21_000.0 * shield.powf(1.3);
            // Basal scarp, caldera.
            h += 5_000.0 * smoothstep(300_000.0, 285_000.0, r);
            h -= 3_000.0 * smoothstep(32_000.0, 26_000.0, r);
            let (c, tint) = craters(x, z, 900_000.0, 160, 25_000.0, 5);
            h += c * smoothstep(250_000.0, 320_000.0, r) + fbm(x, z, 40_000.0, 5, 9) * 700.0;
            // Lava flows down the flanks.
            let flows = fbm(x * 0.4, z * 0.4, 30_000.0, 4, 13);
            let dark = (0.35 + flows * 1.2 + (1.0 - shield) * 0.2).clamp(0.0, 1.0);
            (h, albedo(dark) * tint)
        }
        Terrain::Canyon => {
            let wobble = fbm(0.0, z, 30_000.0, 4, 21) * 6_000.0;
            let d = (x - wobble).abs();
            // Spurs and gullies down the walls.
            let gullies = fbm(x * 0.3, z * 1.0, 3_000.0, 5, 17) * 1_800.0;
            let wall = smoothstep(18_000.0, 30_000.0, d + gullies);
            let mut h = -6_000.0 + 7_000.0 * wall;
            h += fbm(x, z, 8_000.0, 6, 19) * 500.0 * (1.0 - wall)
                + fbm(x, z, 20_000.0, 5, 23) * 400.0;
            // Dark sand on the floor, bright layered walls.
            let layers = (h / 600.0).sin() * 0.15;
            let dark = (0.55 * (1.0 - wall) + layers + fbm(x, z, 15_000.0, 4, 29)).clamp(0.0, 1.0);
            (h, albedo(dark))
        }
        Terrain::CraterRim => {
            // The camera stands 30 km inside the rim of a crater 150 km across.
            let (cx, cz) = (0.0, 45_000.0);
            let r = ((x - cx).powi(2) + (z - cz).powi(2)).sqrt();
            let rim = 75_000.0 + fbm(x, z, 20_000.0, 4, 31) * 6_000.0;
            let gullies = fbm(x * 0.35, z * 0.35, 2_500.0, 6, 37) * 1_400.0;
            let mut h = 2_500.0 * smoothstep(rim - 9_000.0, rim, r)
                - 600.0 * smoothstep(rim, rim + 30_000.0, r);
            h += gullies * smoothstep(rim - 10_000.0, rim - 2_000.0, r);
            let (c, tint) = craters(x, z, 200_000.0, 160, 6_000.0, 41);
            h += c + fbm(x, z, 10_000.0, 6, 43) * 150.0;
            let slopes = smoothstep(rim - 12_000.0, rim - 4_000.0, r);
            let dark = (0.3 + 0.35 * slopes + fbm(x, z, 25_000.0, 4, 47) * 1.5).clamp(0.0, 1.0);
            (h, albedo(dark) * tint)
        }
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The terrain as a grid draped over the planet (center at y = -radius). For views from the
/// ground (`eye`) the grid is spaced quadratically away from the camera: meters apart at its
/// feet, hundreds of meters at the horizon.
fn terrain_mesh(terrain: Terrain, radius: f64, eye: Vec2) -> Mesh {
    let (size, n, center, warp) = match terrain {
        Terrain::Plains => (760_000.0, 400, Vec2::ZERO, false),
        Terrain::Volcano => (900_000.0, 400, Vec2::new(0.0, -100_000.0), false),
        Terrain::Canyon => (90_000.0, 500, eye, true),
        Terrain::CraterRim => (90_000.0, 500, eye, true),
    };
    let mut vertices = Vec::with_capacity((n + 1) * (n + 1));
    let along = |k: usize| {
        let u = k as f32 / n as f32 * 2.0 - 1.0;
        if warp {
            u * u.abs() * size * 0.5
        } else {
            u * size * 0.5
        }
    };
    for j in 0..=n {
        for i in 0..=n {
            let x = center.x + along(i);
            let z = center.y + along(j);
            let (h, color) = sample(terrain, x, z);
            // Down to the sphere: the planet curves away by about d^2 / 2R.
            let d2 = (x as f64).powi(2) + (z as f64).powi(2);
            let drop = (radius * radius - d2).max(0.0).sqrt() - radius;
            let p = Vec3::new(x, h + drop as f32, z);
            vertices.push(
                Vertex::new(p, Vec3::Y, Vec2::new(x / 1000.0, z / 1000.0))
                    .with_color(color.extend(1.0)),
            );
        }
    }
    let mut indices = Vec::with_capacity(n * n * 6);
    let row = (n + 1) as u32;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * row + i;
            indices.extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    let mut mesh = Mesh::new(vertices, indices);
    mesh.compute_normals();
    mesh
}
