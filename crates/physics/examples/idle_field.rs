//! Measures what physics costs a step when almost nothing moves: crates resting in piles near
//! the focus and far from it, beside a few hundred static boxes, the way a streamed-in zone
//! sits once it has settled. Headless, no GPU needed.
//!
//! ```sh
//! cargo run -p engine_physics --release --example idle_field -- [frames]
//! PHYSICS_REPORT=600 cargo run -p engine_physics --release --example idle_field
//! ```
//!
//! Prints the mean and worst step time (physics's own work) and how many bodies are awake.
//! Under callgrind, pass few frames and `--toggle-collect='*timed_frame*'`: the warm-up
//! settles the piles first.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::physics::{Collider, RigidBody, Velocity};
use engine_world::{SimulationFocus, WorldPlugin};

/// Pile centers along X: near the focus, past the coarse distance, far.
const PILES: [f64; 6] = [20.0, -30.0, 180.0, -260.0, 700.0, -950.0];
/// Crates per pile (a 10 x 10 grid, this many layers).
const LAYERS: usize = 5;
/// Static boxes per pile (walls, kerbs, props).
const STATICS: usize = 60;
/// Frames before timing starts: the piles settle and fall asleep.
const WARMUP: usize = 300;

fn main() {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(600);
    engine_core::log::init("info");
    let mut app = field();
    let mut times = Vec::new();
    for _ in 0..WARMUP {
        app.update();
    }
    eprintln!("idle field: warmed up");
    for _ in 0..frames {
        timed_frame(&mut app);
        times.push(app.world.resource::<PhysicsStats>().step_ms);
    }
    let s = *app.world.resource::<PhysicsStats>();
    let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
    let worst = times.iter().copied().fold(0.0, f64::max);
    println!(
        "idle field: {mean:.3} ms per step (worst {worst:.2}) over {frames} steps; {} bodies, {} awake, {} regions",
        s.bodies, s.awake, s.regions
    );
    if std::env::var("IDLE_PILES").is_ok() {
        let mut q = app.world.query::<(&Transform, &Velocity, &RigidBody)>();
        for x in PILES {
            let (mut n, mut moving, mut top, mut low) = (0, 0, 0.0f32, f64::MAX);
            for (t, v, _) in q.iter(&app.world) {
                if (t.translation.x - x).abs() < 20.0 {
                    n += 1;
                    let sp = v.linear.length() + v.angular.length();
                    moving += usize::from(sp > 0.01);
                    top = top.max(sp);
                    low = low.min(t.translation.y);
                }
            }
            println!("pile at {x}: {n} crates, {moving} moving, fastest {top:.3}, lowest {low:.3}");
        }
    }
}

/// One timed frame, kept apart so a profiler can count only these
/// (`valgrind --tool=callgrind --toggle-collect='*timed_frame*'`).
#[inline(never)]
fn timed_frame(app: &mut App) {
    app.update();
}

fn field() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world
        .spawn((Transform::default(), SimulationFocus::default()));
    for x in PILES {
        // The ground under the pile.
        app.world.spawn((
            Transform::from_translation(DVec3::new(x, -0.5, 0.0)),
            Collider::cuboid(Vec3::new(20.0, 0.5, 20.0)),
        ));
        for i in 0..STATICS {
            let (a, r) = (i as f64 * 0.7, 9.0 + (i % 5) as f64);
            app.world.spawn((
                Transform::from_translation(DVec3::new(x + a.cos() * r, 0.5, a.sin() * r)),
                Collider::cuboid(Vec3::new(0.4, 0.5, 0.2)),
            ));
        }
        for layer in 0..LAYERS {
            for k in 0..100 {
                let at = DVec3::new(
                    x + (k % 10) as f64 * 0.7 - 3.15,
                    0.26 + layer as f64 * 0.52,
                    (k / 10) as f64 * 0.7 - 3.15,
                );
                app.world.spawn((
                    Transform::from_translation(at),
                    RigidBody::dynamic(),
                    Collider::cuboid(Vec3::splat(0.25)).with_density(400.0),
                    Velocity::default(),
                ));
            }
        }
    }
    app
}
