//! What the NPC module costs per step in a big world: the arena fight and the wildlife meadow
//! among a valley's worth of static colliders, with terrain streaming in a new collider tile
//! every step, and the same content far from the camera. Ignored by default (it measures
//! time); run with `cargo test -p engine_npc --release --test perf -- --ignored --nocapture`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::physics::{Collider, ColliderShape};
use engine_world::{Ground, GroundQuery, GroundSample, SimulationFocus, SurfaceId, WorldPlugin};

const ARENA: DVec3 = DVec3::new(-70.0, 0.0, 170.0);
const MEADOW: DVec3 = DVec3::new(90.0, 0.0, 60.0);

/// Flat ground that takes about as long to ask as a planet's terrain does, and counts how
/// often it is asked.
struct SlowGround(Arc<AtomicU64>);

impl GroundQuery for SlowGround {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let mut h = 0.0;
        for i in 0..64 {
            h += (p.x * 0.01 * i as f64).sin() * (p.z * 0.013 * i as f64).cos();
        }
        Some(GroundSample {
            height: h * 1e-12,
            normal: Vec3::Y,
            surface: SurfaceId(3),
        })
    }
}

fn app(samples: &Arc<AtomicU64>) -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    app.insert_resource(Ground(Arc::new(SlowGround(samples.clone()))));
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, ARENA);
        demo::spawn_wildlife(&mut commands, MEADOW, 40.0);
        // Rocks, trunks and walls over a square kilometre, away from the two demos.
        let mut seed = 0x9e37_79b9_u64;
        let mut rand = move || {
            seed = engine_core::math::hash_u64(seed);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut placed = 0;
        while placed < 3000 {
            let p = DVec3::new(rand() * 1000.0 - 500.0, 0.0, rand() * 1000.0 - 500.0);
            if p.distance(ARENA) < 60.0 || p.distance(MEADOW) < 50.0 {
                continue;
            }
            let half = Vec3::new(
                0.3 + rand() as f32 * 2.0,
                0.5 + rand() as f32 * 2.0,
                0.3 + rand() as f32 * 2.0,
            );
            commands.spawn((
                Collider::cuboid(half),
                Transform::from_translation(p + DVec3::Y * half.y as f64),
            ));
            placed += 1;
        }
    }
    app.world.flush();
    app
}

fn focus(app: &mut App, at: DVec3) {
    app.world
        .spawn((SimulationFocus::default(), Transform::from_translation(at)));
}

/// A terrain tile's collider, as a streaming terrain adds one most steps.
fn stream_tile(app: &mut App, i: u32) {
    let n = 17;
    let x = (i % 40) as f64 * 64.0 - 1280.0;
    let z = (i / 40 % 40) as f64 * 64.0 - 1280.0;
    app.world.spawn((
        Collider {
            shape: ColliderShape::Heightfield {
                heights: vec![0.0; n * n],
                rows: n as u32,
                cols: n as u32,
                size: [64.0, 64.0],
            },
            ..Default::default()
        },
        Transform::from_translation(DVec3::new(x, 0.0, z)),
    ));
}

/// Mean and worst step, ms, and ground samples per step, over `steps` steps after a warm-up.
fn measure(app: &mut App, samples: &AtomicU64, steps: u32) -> (f64, f64, f64) {
    for i in 0..120 {
        stream_tile(app, i);
        app.update();
    }
    let (mut sum, mut worst) = (0.0f64, 0.0f64);
    let asked = samples.load(Ordering::Relaxed);
    for i in 0..steps {
        stream_tile(app, 1000 + i);
        let t = Instant::now();
        app.update();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        sum += ms;
        worst = worst.max(ms);
    }
    let asked = samples.load(Ordering::Relaxed) - asked;
    (sum / steps as f64, worst, asked as f64 / steps as f64)
}

#[test]
#[ignore]
fn npc_step_cost_near_and_far() {
    let samples = Arc::new(AtomicU64::new(0));
    let mut near = app(&samples);
    focus(&mut near, ARENA + DVec3::new(0.0, 2.0, -30.0));
    let (mean, worst, asked) = measure(&mut near, &samples, 600);
    println!(
        "camera at the arena: {mean:.2} ms per step on average, worst {worst:.2} ms, \
         {asked:.0} ground samples per step"
    );

    let samples = Arc::new(AtomicU64::new(0));
    let mut far = app(&samples);
    focus(&mut far, DVec3::new(2000.0, 2.0, 2000.0));
    let (mean, worst, asked) = measure(&mut far, &samples, 600);
    println!(
        "camera 2 km away: {mean:.2} ms per step on average, worst {worst:.2} ms, \
         {asked:.0} ground samples per step"
    );
}
