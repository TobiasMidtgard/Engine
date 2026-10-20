//! Measures what the physics level of detail saves: drums of rocks tumbling forever near the
//! focus, past the coarse distance and past the half-rate distance, stepped with the level of
//! detail on and then off. Headless, no GPU needed.
//!
//! ```sh
//! cargo run -p engine_physics --release --example lod_field -- [frames] [out.csv]
//! ```
//!
//! Prints the mean step time both ways, how many rocks each tier held, and how many rocks left
//! their drum (none should), and writes every frame's step time both ways to the CSV.
//! `LOD_ALL=coarse` (or `half`) puts every drum at that level of detail instead.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsSettings, PhysicsStats};
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{SimulationFocus, WorldPlugin};

/// Drum centers: two near the focus, two coarse, two at half rate.
const DRUMS: [f64; 6] = [25.0, -40.0, 300.0, -380.0, 900.0, -1100.0];
const ROCKS_PER_DRUM: usize = 150;
const RADIUS: f32 = 3.0;
const HALF_LENGTH: f32 = 2.0;
/// Frames before timing starts (the rocks fall into the drums and the regions split).
const WARMUP: usize = 120;

fn main() {
    let frames: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(600);
    let out = std::env::args().nth(2).unwrap_or("lod_field.csv".into());
    let mut runs = Vec::new();
    for lod in [true, false] {
        let mut app = field(lod);
        let (mut times, mut tiers) = (Vec::new(), (0, 0, 0));
        for frame in 0..WARMUP + frames {
            app.update();
            let s = *app.world.resource::<PhysicsStats>();
            if frame >= WARMUP {
                times.push(s.step_ms);
                tiers = (
                    s.awake - s.coarse_awake - s.half_rate_awake,
                    s.coarse_awake,
                    s.half_rate_awake,
                );
            }
        }
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let s = *app.world.resource::<PhysicsStats>();
        println!(
            "level of detail {}: {mean:6.2} ms per step; awake bodies full/coarse/half rate {}/{}/{}; {} regions; {} rocks out of their drums",
            if lod { "on " } else { "off" },
            tiers.0,
            tiers.1,
            tiers.2,
            s.regions,
            escaped(&mut app),
        );
        runs.push(times);
    }
    let mut csv = String::from("frame,lod_ms,full_ms\n");
    for (i, (a, b)) in runs[0].iter().zip(&runs[1]).enumerate() {
        csv += &format!("{i},{a:.4},{b:.4}\n");
    }
    std::fs::write(&out, csv).expect("write the CSV");
    println!("wrote {out}");
}

/// The rocks farther from their drum's axis than its wall, or past its ends.
fn escaped(app: &mut App) -> usize {
    let mut q = app
        .world
        .query_filtered::<&Transform, (With<RigidBody>, With<Collider>, Without<SimulationFocus>)>(
        );
    q.iter(&app.world)
        .filter(|t| {
            let p = t.translation;
            let Some(x) = DRUMS.iter().find(|x| (p.x - **x).abs() < 10.0) else {
                return true;
            };
            let r = DVec3::new(p.x - x, p.y - 3.5, 0.0).length();
            r > (RADIUS + 0.3) as f64 || p.z.abs() > (HALF_LENGTH + 0.3) as f64
        })
        .count()
}

fn field(lod: bool) -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    if let Ok(v) = std::env::var("LOD_ALL") {
        let mut s = app.world.resource_mut::<PhysicsSettings>();
        s.coarse_beyond_m = Some(10.0);
        s.half_rate_beyond_m = (v == "half").then_some(15.0);
    }
    if !lod {
        let mut s = app.world.resource_mut::<PhysicsSettings>();
        s.coarse_beyond_m = None;
        s.half_rate_beyond_m = None;
    }
    app.world
        .spawn((Transform::default(), SimulationFocus::default()));
    let mut seed = 7u64;
    let mut rnd = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f64 / (1u64 << 31) as f64) as f32
    };
    for (d, x) in DRUMS.iter().enumerate() {
        drum(
            &mut app,
            DVec3::new(*x, 3.5, 0.0),
            if d % 2 == 0 { 0.6 } else { -0.5 },
        );
        for i in 0..ROCKS_PER_DRUM {
            let cell = |k: usize| (k as f32 - 2.5) * 0.6;
            let at = Vec3::new(cell(i % 6), cell((i / 6) % 6), cell(i / 36) * 0.8);
            let size = Vec3::new(0.18 + 0.12 * rnd(), 0.12 + 0.1 * rnd(), 0.15 + 0.12 * rnd());
            let points: Vec<Vec3> = (0..12)
                .map(|_| {
                    Vec3::new(rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0).normalize()
                        * size
                })
                .collect();
            app.world.spawn((
                Transform::from_translation(DVec3::new(*x, 3.5, 0.0) + at.as_dvec3())
                    .with_rotation(Quat::from_rotation_y(rnd() * 6.0)),
                RigidBody::dynamic(),
                Collider {
                    shape: ColliderShape::ConvexHull { points },
                    ..Collider::cuboid(Vec3::ONE)
                }
                .with_density(2700.0),
                Velocity::default(),
            ));
        }
    }
    app
}

/// A kinematic drum turning about +Z at `spin` rad/s: ten staves and two end plates.
pub fn drum(app: &mut App, center: DVec3, spin: f32) {
    let body = app
        .world
        .spawn((
            Transform::from_translation(center),
            RigidBody::kinematic(),
            Velocity {
                linear: Vec3::ZERO,
                angular: Vec3::new(0.0, 0.0, spin),
            },
        ))
        .id();
    let staves = 10;
    let half_chord = RADIUS * (std::f32::consts::PI / staves as f32).tan() + 0.1;
    for k in 0..staves {
        let a = k as f32 / staves as f32 * std::f32::consts::TAU;
        app.world.spawn((
            Transform::from_translation(DVec3::new(
                (a.cos() * (RADIUS + 0.2)) as f64,
                (a.sin() * (RADIUS + 0.2)) as f64,
                0.0,
            ))
            .with_rotation(Quat::from_rotation_z(a)),
            Collider::cuboid(Vec3::new(0.2, half_chord, HALF_LENGTH + 0.3)),
            ChildOf(body),
        ));
    }
    for z in [-1.0, 1.0] {
        app.world.spawn((
            Transform::from_translation(DVec3::new(0.0, 0.0, z * (HALF_LENGTH + 0.15) as f64)),
            Collider::cuboid(Vec3::new(RADIUS + 0.4, RADIUS + 0.4, 0.15)),
            ChildOf(body),
        ));
    }
}
