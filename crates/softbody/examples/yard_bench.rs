//! Times the soft-body yard's simulation without drawing it: the demo spawned as in the
//! showcase, the focus at `distance` m from it, and the soft-body systems' cost per frame.
//!
//! ```sh
//! cargo run -p engine_softbody --example yard_bench --release -- [frames] [distance]
//! SOLO=1 cargo run ...   # also times each body's solver alone, with and without self-collision
//! ```

use std::time::Instant;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_softbody::render::Geometry;
use engine_world::FocusPoints;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(240);
    let distance: f64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(20.0);
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_world::WorldPlugin, engine_softbody::SoftBodyPlugin));
    let yard = DVec3::new(125.0, 0.0, -35.0);
    app.add_systems(Startup, move |mut commands: Commands| {
        engine_softbody::demo::spawn_demo(&mut commands, yard);
    });
    app.add_systems(PreUpdate, move |mut focus: ResMut<FocusPoints>| {
        focus.points.clear();
        focus.points.push(yard + DVec3::new(distance, 1.7, 0.0));
    });
    // Build the bodies (and their first steps) before timing.
    let warm = Instant::now();
    app.update_n(30);
    println!("warm-up: {:.0} ms", warm.elapsed().as_secs_f64() * 1000.0);
    let start = Instant::now();
    app.update_n(frames);
    let ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    let mut q = app
        .world
        .query::<(Option<&Name>, &engine_softbody::SoftBodySim)>();
    let mut costs: Vec<(f32, String, usize)> = q
        .iter(&app.world)
        .map(|(n, b)| {
            let name = n.map_or("?".to_string(), |n| n.as_str().to_string());
            (b.step_ms, name, b.particle_count())
        })
        .collect();
    costs.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Each body's solver alone, with no colliders, with and without self-collision.
    if std::env::var("SOLO").is_ok() {
        for (n, b) in q.iter(&app.world) {
            let time = |mut sim: engine_softbody::solver::Sim| {
                let t = Instant::now();
                for _ in 0..10 {
                    sim.step(1.0 / 60.0, &[]);
                }
                t.elapsed().as_secs_f64() * 100.0
            };
            let full = time(b.sim.clone());
            let mut plain = b.sim.clone();
            plain.params.self_collide = 0.0;
            let plain = time(plain);
            println!(
                "  solo {full:6.2} ms, {plain:6.2} without self-collision, {} substeps: {}",
                b.sim.params.substeps,
                n.map_or("?", |n| n.as_str())
            );
        }
    }
    // What the render prepare builds each frame for every body seen from the focus, without
    // a GPU.
    let mut geo = Geometry::default();
    // Each body's radius on screen from the focus, for a 1080p view 60 degrees high.
    let eye = yard + DVec3::new(distance, 1.7, 0.0);
    let px = |b: &engine_softbody::SoftBodySim| {
        let d = (b.center.distance(eye) as f32).max(b.radius + 0.1);
        b.radius / d * 540.0 / (30f32).to_radians().tan()
    };
    let bodies: Vec<_> = q.iter(&app.world).map(|(_, b)| b).collect();
    let t = Instant::now();
    for _ in 0..10 {
        geo.clear();
        for b in &bodies {
            let base = (b.sim.origin - yard).as_vec3();
            geo.add_body(b, base, 0.5, px(b));
            geo.add_hair(b, base, 0.5, 1.0, 1.0);
        }
    }
    println!(
        "render geometry on one thread: {:.2} ms per frame, {} vertices, {} guide points",
        t.elapsed().as_secs_f64() * 100.0,
        geo.vertices.len(),
        geo.points.len()
    );
    // The same split over the compute pool the way the render prepare does it.
    let pool = engine_core::tasks::ComputeTaskPool::get();
    let run = bodies.len().div_ceil(pool.thread_num().max(1) * 2).max(1);
    let mut parts: Vec<Geometry> = (0..bodies.len().div_ceil(run))
        .map(|_| Geometry::default())
        .collect();
    let mut joined = Geometry::default();
    let mut join = 0.0;
    let (mut material, mut hole) = (0, 0);
    for (chunk, part) in bodies.chunks(run).zip(parts.iter_mut()) {
        (part.material_base, part.hole_base) = (material, hole);
        for b in chunk {
            let (m, h) = Geometry::slots(b);
            (material, hole) = (material + m, hole + h);
        }
    }
    let t = Instant::now();
    for _ in 0..10 {
        pool.scope(|scope| {
            for (chunk, part) in bodies.chunks(run).zip(parts.iter_mut()) {
                scope.spawn(async move {
                    part.clear();
                    for b in chunk {
                        let base = (b.sim.origin - yard).as_vec3();
                        part.add_body(b, base, 0.5, px(b));
                        part.add_hair(b, base, 0.5, 1.0, 1.0);
                    }
                });
            }
        });
        let j = Instant::now();
        joined.clear();
        for part in &parts {
            joined.append(part);
        }
        join += j.elapsed().as_secs_f64() * 100.0;
    }
    println!(
        "render geometry on {} threads: {:.2} ms per frame, {join:.2} of it joining, {} vertices",
        pool.thread_num(),
        t.elapsed().as_secs_f64() * 100.0,
        joined.vertices.len()
    );
    let mut each: Vec<(f64, String, usize)> = q
        .iter(&app.world)
        .map(|(n, b)| {
            let base = (b.sim.origin - yard).as_vec3();
            let t = Instant::now();
            for _ in 0..10 {
                geo.clear();
                geo.add_body(b, base, 0.5, px(b));
            }
            let name = n.map_or("?".to_string(), |n| n.as_str().to_string());
            (t.elapsed().as_secs_f64() * 100.0, name, geo.vertices.len())
        })
        .collect();
    each.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (ms, name, v) in each.iter().take(6) {
        println!("  geometry {ms:6.2} ms  {v:6} vertices  {name}");
    }
    for (ms, name, n) in costs.iter().take(12) {
        println!("  {ms:6.2} ms  {n:6} particles  {name}");
    }
    let d = app.world.resource::<Diagnostics>();
    println!(
        "focus {distance} m from the yard: {ms:.2} ms per frame; last step {:.2} ms, {} bodies \
         ({} frozen), {} particles",
        d.get("softbody.step_ms").unwrap_or(0.0),
        d.get("softbody.bodies").unwrap_or(0.0),
        d.get("softbody.frozen").unwrap_or(0.0),
        d.get("softbody.particles").unwrap_or(0.0),
    );
}
