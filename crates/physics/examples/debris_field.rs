//! Measures what a field of rock debris costs per physics step while it tumbles into a pit and
//! after it has settled and fallen asleep. Headless, no GPU needed.
//!
//! ```sh
//! cargo run -p engine_physics --release --example debris_field -- [rocks] [out.csv]
//! ```
//!
//! Prints each second's mean step time and how many rocks are awake, and writes every step's
//! time (ms) and awake count to the CSV.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::WorldPlugin;
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};

/// Fifteen seconds: the rocks pour in, settle and sleep.
const FRAMES: usize = 900;

fn main() {
    let rocks: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);
    let out = std::env::args().nth(2).unwrap_or("debris_field.csv".into());
    let mut app = pit(rocks);
    let mut csv = String::from("frame,step_ms,awake\n");
    let mut second = Vec::new();
    for frame in 0..FRAMES {
        app.update();
        let s = *app.world.resource::<PhysicsStats>();
        csv += &format!("{frame},{:.4},{}\n", s.step_ms, s.awake);
        second.push(s);
        if second.len() == 60 {
            let mean = second.iter().map(|s| s.step_ms).sum::<f64>() / 60.0;
            let awake = second.iter().map(|s| s.awake).max().unwrap_or(0);
            println!(
                "second {:2}: {mean:6.2} ms per step, {awake:4} of {rocks} rocks awake",
                frame / 60
            );
            second.clear();
        }
    }
    std::fs::write(&out, csv).expect("write the CSV");
    println!("wrote {out}");
}

/// A 10 m pit on a 60 m floor, and `rocks` irregular rocks (convex hulls of 16 points, 20 to
/// 55 cm across, granite) stacked above it in layers of 64.
fn pit(rocks: usize) -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let fixed = |app: &mut App, at: DVec3, half: Vec3| {
        app.world.spawn((
            Transform::from_translation(at),
            RigidBody::fixed(),
            Collider::cuboid(half),
        ));
    };
    fixed(
        &mut app,
        DVec3::new(0.0, -0.5, 0.0),
        Vec3::new(30.0, 0.5, 30.0),
    );
    for (x, z, hx, hz) in [
        (5.5, 0.0, 0.5, 6.0),
        (-5.5, 0.0, 0.5, 6.0),
        (0.0, 5.5, 6.0, 0.5),
        (0.0, -5.5, 6.0, 0.5),
    ] {
        fixed(&mut app, DVec3::new(x, 2.0, z), Vec3::new(hx, 2.0, hz));
    }
    let mut seed = 12345u64;
    let mut rnd = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f64 / (1u64 << 31) as f64) as f32
    };
    for i in 0..rocks {
        let at = DVec3::new(
            ((i % 8) as f64 - 3.5) * 1.2,
            1.0 + (i / 64) as f64 * 0.6,
            (((i / 8) % 8) as f64 - 3.5) * 1.2,
        );
        let size = Vec3::new(0.15 + 0.15 * rnd(), 0.1 + 0.1 * rnd(), 0.12 + 0.15 * rnd());
        let points: Vec<Vec3> = (0..16)
            .map(|_| {
                Vec3::new(rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0).normalize()
                    * size
            })
            .collect();
        let collider = Collider {
            shape: ColliderShape::ConvexHull { points },
            ..Collider::cuboid(Vec3::ONE)
        }
        .with_density(2700.0);
        app.world.spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(rnd() * 6.0)),
            RigidBody::dynamic(),
            collider,
            Velocity::default(),
        ));
    }
    app
}
