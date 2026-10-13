//! Close-up screenshots of the sport plane and the spaceplane, and the build budget of their
//! models (triangles, generation time).
//!
//! The shots render only with `$VEHICLE_SHOTS` set (the budgets always run):
//! `VEHICLE_SHOTS=<dir> ENGINE_REQUIRE_GPU=1 cargo test -p engine_vehicles --test shots_aircraft -- --nocapture`

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use support::shots::*;

fn sport(name: &str, eye: [f64; 3], target: [f64; 3]) {
    gallery_shot(
        name,
        0.6,
        |app| {
            floor(app, 200.0);
            parked(app, "sport_plane", DVec3::ZERO, 0.0);
        },
        Transform::from_xyz(eye[0], eye[1], eye[2])
            .looking_at(DVec3::new(target[0], target[1], target[2]), Vec3::Y),
    );
}

#[test]
fn aircraft_sport_plane_front() {
    sport(
        "aircraft_sport_plane_front",
        [-7.5, 3.0, -8.0],
        [0.0, 0.9, -0.3],
    );
}

/// Run-up on the brakes: the propeller turns too fast to see, so its blur disc shows.
#[test]
fn aircraft_sport_plane_runup() {
    gallery_shot(
        "aircraft_sport_plane_runup",
        0.8,
        |app| {
            floor(app, 200.0);
            let e = parked(app, "sport_plane", DVec3::ZERO, 0.0);
            support::controls(app, e, |c| {
                c.throttle = 0.7;
                c.brake = 1.0;
            });
        },
        Transform::from_xyz(-5.0, 1.7, -7.0).looking_at(DVec3::new(0.0, 1.0, -2.6), Vec3::Y),
    );
}

#[test]
fn aircraft_sport_plane_rear() {
    sport(
        "aircraft_sport_plane_rear",
        [7.0, 3.2, 8.5],
        [0.0, 1.0, 0.6],
    );
}

#[test]
fn aircraft_sport_plane_side() {
    sport(
        "aircraft_sport_plane_side",
        [11.0, 1.5, 0.4],
        [0.0, 0.9, 0.4],
    );
}

#[test]
fn aircraft_sport_plane_top() {
    sport(
        "aircraft_sport_plane_top",
        [0.01, 14.0, 0.6],
        [0.0, 0.9, 0.6],
    );
}

#[test]
fn aircraft_sport_plane_cockpit() {
    sport(
        "aircraft_sport_plane_cockpit",
        [1.5, 2.5, -1.4],
        [0.0, 1.1, -0.7],
    );
}

#[test]
fn aircraft_sport_plane_engine() {
    sport(
        "aircraft_sport_plane_engine",
        [-1.6, 1.3, -5.4],
        [0.0, 0.85, -3.2],
    );
}

#[test]
fn aircraft_sport_plane_panel() {
    sport(
        "aircraft_sport_plane_panel",
        [0.25, 2.3, 0.6],
        [0.0, 1.05, -1.36],
    );
}

#[test]
fn aircraft_sport_plane_gear() {
    sport(
        "aircraft_sport_plane_gear",
        [3.2, 0.55, -1.4],
        [1.2, 0.3, 0.3],
    );
}

fn ship(name: &str, eye: [f64; 3], target: [f64; 3]) {
    gallery_shot(
        name,
        0.6,
        |app| {
            floor(app, 300.0);
            parked(app, "spaceplane", DVec3::ZERO, 0.0);
        },
        Transform::from_xyz(eye[0], eye[1], eye[2])
            .looking_at(DVec3::new(target[0], target[1], target[2]), Vec3::Y),
    );
}

#[test]
fn aircraft_spaceplane_front() {
    ship(
        "aircraft_spaceplane_front",
        [-15.0, 6.5, -21.0],
        [0.0, 2.6, -0.5],
    );
}

#[test]
fn aircraft_spaceplane_rear() {
    ship(
        "aircraft_spaceplane_rear",
        [15.0, 7.5, 23.0],
        [0.0, 2.8, 3.5],
    );
}

#[test]
fn aircraft_spaceplane_side() {
    ship(
        "aircraft_spaceplane_side",
        [27.0, 3.2, 0.8],
        [0.0, 3.0, 0.8],
    );
}

#[test]
fn aircraft_spaceplane_top() {
    ship(
        "aircraft_spaceplane_top",
        [0.01, 36.0, 0.6],
        [0.0, 2.5, 0.6],
    );
}

#[test]
fn aircraft_spaceplane_belly() {
    ship(
        "aircraft_spaceplane_belly",
        [11.0, 0.3, -12.0],
        [0.5, 1.7, 1.0],
    );
}

#[test]
fn aircraft_spaceplane_cockpit() {
    ship(
        "aircraft_spaceplane_cockpit",
        [-4.5, 6.8, -13.0],
        [0.0, 4.3, -6.6],
    );
}

#[test]
fn aircraft_spaceplane_engines() {
    ship(
        "aircraft_spaceplane_engines",
        [5.0, 3.6, 17.5],
        [0.0, 2.7, 11.0],
    );
}

#[test]
fn aircraft_spaceplane_gear() {
    ship(
        "aircraft_spaceplane_gear",
        [6.5, 0.7, -0.5],
        [2.0, 0.8, 4.0],
    );
}

/// Main and hover engines firing with the ship hanging a few meters up, for the plumes.
#[test]
fn aircraft_spaceplane_firing() {
    gallery_shot(
        "aircraft_spaceplane_firing",
        0.5,
        |app| {
            floor(app, 300.0);
            let e = parked(app, "spaceplane", DVec3::new(0.0, 7.0, 0.0), 0.0);
            support::controls(app, e, |c| {
                c.throttle = 1.0;
                c.lift = 0.5;
            });
        },
        Transform::from_xyz(22.0, 5.0, 30.0).looking_at(DVec3::new(0.0, 7.0, 6.0), Vec3::Y),
    );
}

/// The heat shield hot (as after re-entry): tiles and leading edges glow through the heat
/// glow's emission masks.
#[test]
fn aircraft_spaceplane_heat() {
    gallery_shot(
        "aircraft_spaceplane_heat",
        0.3,
        |app| {
            floor(app, 300.0);
            let e = parked(app, "spaceplane", DVec3::ZERO, 0.0);
            if let Some(mut shield) = app.world.get_mut::<engine_vehicles::HeatShield>(e) {
                shield.temperature_k = 1900.0;
            }
        },
        Transform::from_xyz(-14.0, 2.2, -16.0).looking_at(DVec3::new(0.0, 2.2, -2.0), Vec3::Y),
    );
}

/// Both aircraft parked on the apron.
#[test]
fn aircraft_apron() {
    gallery_shot(
        "aircraft_apron",
        2.0,
        |app| {
            floor(app, 300.0);
            parked(app, "sport_plane", DVec3::new(-9.0, 0.0, 4.0), 30.0);
            parked(app, "spaceplane", DVec3::new(10.0, 0.0, -8.0), -30.0);
        },
        Transform::from_xyz(-24.0, 7.0, 26.0).looking_at(DVec3::new(2.0, 2.0, -3.0), Vec3::Y),
    );
}

/// Where the wheels settle when parked (the drawn gear legs and fairings are built around
/// these axle heights).
#[test]
fn aircraft_rest_pose() {
    for kind in ["sport_plane", "spaceplane"] {
        let mut app = support::vehicle_app();
        support::calm(&mut app);
        support::ground(&mut app, DVec3::ZERO, 100.0, "concrete");
        let e = parked(&mut app, kind, DVec3::ZERO, 0.0);
        support::run_for(&mut app, 3.0);
        let wheels = app
            .world
            .get::<engine_vehicles::wheels::Wheels>(e)
            .expect("wheels");
        let t = support::transform(&app, e);
        for w in &wheels.wheels {
            eprintln!(
                "{kind}: {} spring {:.3} (axle y {:.3}), grounded {}; body y {:.3}, pitch {:.2}°",
                w.name,
                w.state.spring_length,
                w.mount.y - w.state.spring_length,
                w.state.grounded,
                t.translation.y,
                t.rotation
                    .to_euler(engine_core::math::EulerRot::YXZ)
                    .1
                    .to_degrees()
            );
        }
    }
}

/// CPU seconds this process has used (Linux; 0 elsewhere).
fn cpu_secs() -> f64 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // Fields after the command name (which is in parentheses), from the state on: utime
    // and stime are the 12th and 13th, in clock ticks of 1/100 s, summed over all threads.
    let rest = stat.rsplit_once(')').map_or("", |r| r.1);
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0
}

/// Mesh and texture generation time and triangle count of one model: builds it three times
/// (the first build is what a game pays; the best of three shows the cost without the noise
/// of other processes on a shared machine), prints the numbers and the biggest meshes.
fn budget(
    name: &str,
    build: fn() -> engine_vehicles::models::aircraft::toolkit::ModelAssets,
) -> usize {
    let mut times = Vec::new();
    let mut cpu = Vec::new();
    let mut model = None;
    for _ in 0..3 {
        let (t, c) = (std::time::Instant::now(), cpu_secs());
        model = Some(build());
        times.push(t.elapsed().as_secs_f64());
        cpu.push(cpu_secs() - c);
    }
    let model = model.expect("built");
    let tris = model.triangles();
    let best = times.iter().copied().fold(f64::MAX, f64::min);
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    eprintln!(
        "{name}: {tris} triangles in {} meshes, {} images; generated in {:.3} s (first), \
         {best:.3} s (best of 3), {:.2} CPU s per build on {cores} cores",
        model.meshes.len(),
        model.images.len(),
        times[0],
        cpu.iter().sum::<f64>() / 3.0,
    );
    let mut by_mesh: Vec<(usize, &str)> = model
        .meshes
        .iter()
        .map(|(k, m)| (m.triangle_count(), k.as_str()))
        .collect();
    by_mesh.sort_unstable_by(|a, b| b.cmp(a));
    for (n, k) in by_mesh.iter().take(12) {
        eprintln!("  {n:>7}  {k}");
    }
    tris
}

/// Mesh and texture generation time and triangle counts (printed; the budgets are 0.6 s per
/// model in the dev profile, 150 k triangles for the sport plane and 250 k for the
/// spaceplane).
#[test]
fn aircraft_model_budgets() {
    use engine_vehicles::models::aircraft::{build_spaceplane, build_sport_plane};
    let plane = budget("sport plane", build_sport_plane);
    let ship = budget("spaceplane", build_spaceplane);
    assert!(plane <= 150_000, "sport plane has {plane} triangles");
    assert!(ship <= 250_000, "spaceplane has {ship} triangles");
}
