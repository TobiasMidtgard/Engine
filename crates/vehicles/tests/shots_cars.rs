//! Close-up screenshots of the sports car and the rally car (front and rear 3/4, profile,
//! wheel, headlight and interior details), plus both on the garage floor. Needs a GPU or a
//! software Vulkan driver (lavapipe); passes with a note when neither is available.
//!
//! The shots render only with `$VEHICLE_SHOTS` set to a folder for the PNGs (the budget
//! always runs).

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;
use support::shots::*;
use support::*;

fn sports(app: &mut App) {
    floor(app, 200.0);
    parked(app, "sports_car", DVec3::ZERO, 0.0);
}

fn rally(app: &mut App) {
    floor(app, 200.0);
    parked(app, "rally_car", DVec3::ZERO, 0.0);
}

fn rally_pod(app: &mut App) {
    floor(app, 200.0);
    let car = parked(app, "rally_car", DVec3::ZERO, 0.0);
    spawn_with(app, move |c| {
        engine_vehicles::models::cars::rally_light_pod(c, car);
        car
    });
}

/// Both cars on a polished garage floor in front of a wall.
fn garage(app: &mut App) {
    ground(app, DVec3::ZERO, 100.0, "concrete");
    plane(app, 200.0, 0.0, 0x6c6c6a, 0.38);
    spawn_with(app, |c| {
        c.spawn((
            Transform::from_xyz(0.0, 3.0, 6.5).with_rotation(
                engine_core::math::Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            ),
            MeshRenderer::new(
                Shape::Plane {
                    size: [24.0, 6.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x9c9a96).with_roughness(0.9),
            ),
        ))
        .id()
    });
    parked(app, "sports_car", DVec3::new(-1.6, 0.0, 0.0), -25.0);
    parked(app, "rally_car", DVec3::new(1.7, 0.0, 0.6), 25.0);
}

fn cam(from: [f64; 3], at: [f64; 3]) -> Transform {
    Transform::from_xyz(from[0], from[1], from[2]).looking_at(DVec3::from_array(at), Vec3::Y)
}

/// A scene plus its own camera with a chosen field of view (it outranks the 45° camera
/// `shoot` adds).
fn with_lens(
    scene: impl FnOnce(&mut App),
    fov_deg: f32,
    camera: Transform,
) -> impl FnOnce(&mut App) {
    move |app| {
        scene(app);
        spawn_with(app, |c| {
            c.spawn((
                Camera {
                    order: 1,
                    ..Camera::perspective(fov_deg)
                },
                camera,
            ))
            .id()
        });
    }
}

fn shot(name: &str, scene: impl FnOnce(&mut App), fov: f32, from: [f64; 3], at: [f64; 3]) {
    gallery_shot(
        name,
        1.0,
        with_lens(scene, fov, cam(from, at)),
        cam(from, at),
    );
}

/// Generation time and triangle budget per car (meshes counted as often as they are drawn).
#[test]
fn cars_kit_budget() {
    use engine_vehicles::models::cars;
    let mut over = Vec::new();
    for (name, make) in [
        ("sports car", cars::sports_car_kit as fn() -> cars::Kit),
        ("rally car", cars::rally_car_kit),
    ] {
        let cpu0 = cpu_seconds();
        let t = std::time::Instant::now();
        let kit = make();
        let secs = t.elapsed().as_secs_f64();
        let cpu = cpu_seconds() - cpu0;
        let mut drawn = 0;
        for (k, m) in &kit.meshes {
            let n = m.triangle_count();
            let times = cars::instances(k);
            eprintln!("  {k}: {n} triangles x{times}");
            drawn += n * times;
        }
        let texels: usize = kit
            .images
            .iter()
            .map(|(_, i)| (i.width * i.height) as usize)
            .sum();
        eprintln!(
            "{name}: {drawn} triangles drawn, {} textures ({:.1} Mtexels), generated in {secs:.3} s \
             ({cpu:.2} s CPU over all threads)",
            kit.images.len(),
            texels as f64 / 1e6
        );
        if drawn > 150_000 {
            over.push(format!("{name}: {drawn} triangles"));
        }
    }
    assert!(over.is_empty(), "over the triangle budget: {over:?}");
}

/// Process CPU time (user + system) in seconds, from /proc (0 elsewhere).
fn cpu_seconds() -> f64 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // Fields after the command name: utime is the 12th, stime the 13th (clock ticks).
    let rest = stat.rsplit(')').next().unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0
}

#[test]
fn cars_sports_front() {
    shot(
        "cars_sports_front",
        sports,
        40.0,
        [-3.7, 1.2, -4.4],
        [0.0, 0.5, -0.35],
    );
}

#[test]
fn cars_sports_rear() {
    shot(
        "cars_sports_rear",
        sports,
        40.0,
        [3.8, 1.35, 4.3],
        [0.0, 0.5, 0.35],
    );
}

#[test]
fn cars_sports_side() {
    shot(
        "cars_sports_side",
        sports,
        40.0,
        [-6.2, 0.7, 0.0],
        [0.0, 0.55, 0.0],
    );
}

#[test]
fn cars_sports_wheel() {
    shot(
        "cars_sports_wheel",
        sports,
        40.0,
        [-2.1, 0.5, -2.0],
        [-0.8, 0.33, -1.35],
    );
}

#[test]
fn cars_sports_headlight() {
    shot(
        "cars_sports_headlight",
        sports,
        30.0,
        [-1.4, 0.85, -3.35],
        [-0.64, 0.55, -2.15],
    );
}

#[test]
fn cars_sports_interior() {
    shot(
        "cars_sports_interior",
        sports,
        50.0,
        [-1.75, 1.0, 0.25],
        [0.1, 0.32, 0.25],
    );
}

#[test]
fn cars_rally_front() {
    shot(
        "cars_rally_front",
        rally,
        40.0,
        [-3.6, 1.3, -4.2],
        [0.0, 0.55, -0.3],
    );
}

#[test]
fn cars_rally_rear() {
    shot(
        "cars_rally_rear",
        rally,
        40.0,
        [3.7, 1.5, 4.1],
        [0.0, 0.6, 0.3],
    );
}

#[test]
fn cars_rally_side() {
    shot(
        "cars_rally_side",
        rally,
        40.0,
        [-6.0, 0.8, 0.0],
        [0.0, 0.62, 0.0],
    );
}

#[test]
fn cars_rally_wheel() {
    shot(
        "cars_rally_wheel",
        rally,
        40.0,
        [-2.0, 0.5, -1.9],
        [-0.79, 0.32, -1.25],
    );
}

#[test]
fn cars_rally_headlight() {
    shot(
        "cars_rally_headlight",
        rally_pod,
        34.0,
        [-1.6, 1.1, -3.4],
        [-0.35, 0.3, -1.85],
    );
}

#[test]
fn cars_rally_interior() {
    shot(
        "cars_rally_interior",
        rally,
        42.0,
        [-1.65, 1.38, 0.0],
        [0.1, 0.55, -0.05],
    );
}

#[test]
fn cars_garage() {
    shot(
        "cars_garage",
        garage,
        42.0,
        [-0.5, 2.1, -6.6],
        [0.0, 0.5, 0.4],
    );
}
