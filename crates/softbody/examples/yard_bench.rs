//! Times the soft-body yard's simulation without drawing it: the demo spawned as in the
//! showcase, the focus at `distance` m from it, and the soft-body systems' cost per frame.
//!
//! ```sh
//! cargo run -p engine_softbody --example yard_bench --release -- [frames] [distance]
//! SOLO=1 cargo run ...   # also times each body's solver with its colliders, and alone with
//!                        # and without self-collision
//! WORST=1 cargo run ...  # also prints the worst frame (hitches: restores, catch-up steps)
//! TRACE="Unveiling sheet" cargo run ...  # that body's step, prewarm and pairs each frame
//! SLOW_GROUND=1 cargo run ...  # ground that costs about what a planet's terrain does
//! RANGE_M=220 cargo run ...  # Ultra's simulation range (High's 150 m by default)
//! REALTIME=45 cargo run ...  # a real clock paced at 45 fps, as in play: main-thread ms a frame
//! ARRIVE=600 cargo run ...  # wait far off for 600 frames, then arrive (WORST shows the spike)
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
    // RANGE_M=220: the simulation range of Ultra (the default is High's 150 m).
    if let Some(m) = std::env::var("RANGE_M").ok().and_then(|m| m.parse().ok()) {
        app.world
            .resource_mut::<engine_world::SimulationRange>()
            .active_m = m;
    }
    if std::env::var("SLOW_GROUND").is_ok() {
        app.insert_resource(engine_world::Ground(std::sync::Arc::new(SlowGround)));
    }
    // The demo's people and the dog are skinned into meshes here (as in the showcase).
    app.world
        .insert_resource(engine_assets::Assets::<engine_render::Mesh>::default());
    let yard = DVec3::new(125.0, 0.0, -35.0);
    app.add_systems(Startup, move |mut commands: Commands| {
        engine_softbody::demo::spawn_demo(&mut commands, yard);
    });
    // ARRIVE=<frames>: the focus waits 400 m off for that many frames, then comes in (an
    // arrival: everything wakes from rest at once).
    let arrive: u32 = std::env::var("ARRIVE")
        .ok()
        .and_then(|a| a.parse().ok())
        .unwrap_or(0);
    app.add_systems(
        PreUpdate,
        move |mut focus: ResMut<FocusPoints>, mut frame: Local<u32>| {
            *frame += 1;
            let d = if *frame <= arrive { 400.0 } else { distance };
            focus.points.clear();
            focus.points.push(yard + DVec3::new(d, 1.7, 0.0));
        },
    );
    // REALTIME=<fps>: after the warm-up the clock turns real and frames are paced at fps, as
    // in play (bodies beside the frame, several fixed steps in a slow frame); reports the
    // soft bodies' main-thread time beside the physics step, summed over each frame's steps.
    let realtime: Option<f64> = std::env::var("REALTIME").ok().and_then(|f| f.parse().ok());
    if realtime.is_some() {
        app.world
            .resource_mut::<engine_softbody::SoftBodyConfig>()
            .beside_frame = Some(true);
    }
    app.init_resource::<MainThread>();
    app.add_systems(
        engine_core::schedule::FixedPostUpdate,
        |d: Res<Diagnostics>, mut m: ResMut<MainThread>| {
            m.0 += d.get("softbody.beside_physics_ms").unwrap_or(0.0);
        },
    );
    // Build the bodies (and their first steps) before timing.
    let warm = Instant::now();
    app.update_n(30);
    if realtime.is_some() {
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Realtime);
    }
    app.world.resource_mut::<MainThread>().0 = 0.0;
    let mut main_ms = Vec::with_capacity(frames);
    println!("warm-up: {:.0} ms", warm.elapsed().as_secs_f64() * 1000.0);
    let start = Instant::now();
    let mut worst = (0.0, 0);
    // The app's update alone (the bench's own bookkeeping below is not the engine's cost).
    let mut updating = 0.0;
    let mut stepping = 0.0;
    // Skinned meshes uploaded (changed) each frame, bytes.
    let mut versions = std::collections::HashMap::new();
    let mut uploaded = 0usize;
    // And bones for the GPU skinning.
    let mut last_bones = std::collections::HashMap::new();
    let mut bone_bytes = 0usize;
    for f in 0..frames {
        let t = Instant::now();
        app.update();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if let Some(fps) = realtime
            && let Some(rest) =
                std::time::Duration::from_secs_f64(1.0 / fps).checked_sub(t.elapsed())
        {
            std::thread::sleep(rest);
        }
        main_ms.push(std::mem::take(
            &mut app.world.resource_mut::<MainThread>().0,
        ));
        updating += ms;
        stepping += app
            .world
            .resource::<Diagnostics>()
            .get("softbody.step_ms")
            .unwrap_or(0.0);
        {
            let mut q = app.world.query_filtered::<
                (Entity, &engine_render::MeshRenderer),
                With<engine_softbody::demo::skin::SkinnedPart>,
            >();
            let shown: Vec<_> = q
                .iter(&app.world)
                .filter_map(|(e, r)| match &r.mesh {
                    engine_render::MeshRef::Handle(h) => Some((e, *h)),
                    _ => None,
                })
                .collect();
            let meshes = app
                .world
                .resource::<engine_assets::Assets<engine_render::Mesh>>();
            for (e, h) in shown {
                let v = meshes.version(h);
                if versions.insert((e, h), v).is_some_and(|old| old != v) {
                    let n = meshes.get(h).map_or(0, |m| m.vertices.len());
                    uploaded += n * std::mem::size_of::<engine_render::Vertex>();
                }
            }
            let mut q = app.world.query::<(Entity, &engine_render::GpuSkin)>();
            for (e, skin) in q.iter(&app.world) {
                if last_bones.get(&e) != Some(&skin.bones) {
                    bone_bytes += std::mem::size_of_val(&skin.bones[..]);
                    last_bones.insert(e, skin.bones.clone());
                }
            }
        }
        // TRACE=<body name>: that body's step each frame.
        if let Ok(name) = std::env::var("TRACE") {
            let mut q = app.world.query::<(&Name, &engine_softbody::SoftBodySim)>();
            for (n, b) in q.iter(&app.world) {
                if n.as_str() == name {
                    let fields = b
                        .last_colliders()
                        .iter()
                        .filter(|c| {
                            matches!(c.shape, engine_softbody::collide::LocalShape::Field(_))
                        })
                        .count();
                    println!(
                        "{f:5} frame {ms:6.1} ms  step {:6.2} ms  prewarm {:4}  pairs {:6} solved {:6}  colliders {} fields {fields} asleep {} fastest {:.3} m/s radius {:.1} env {:.2} ms",
                        b.step_ms,
                        b.pending_prewarm,
                        b.sim.self_pairs(),
                        b.sim.self_pairs_solved(),
                        b.last_colliders().len(),
                        b.asleep(),
                        b.sim
                            .vel
                            .iter()
                            .zip(&b.sim.inv_mass)
                            .filter(|(_, w)| **w > 0.0)
                            .fold(0.0f32, |m, (v, _)| m.max(v.length())),
                        b.radius,
                        b.env_ms,
                    );
                }
            }
        }
        if ms > worst.0 {
            worst = (ms, f);
        }
    }
    if realtime.is_some() {
        let mut sorted = main_ms.clone();
        sorted.sort_by(f64::total_cmp);
        println!(
            "beside the physics step on the main thread: {:.2} ms a frame on average, {:.2} at the 95th percentile, {:.2} worst",
            main_ms.iter().sum::<f64>() / main_ms.len().max(1) as f64,
            sorted.get(sorted.len() * 95 / 100).copied().unwrap_or(0.0),
            sorted.last().copied().unwrap_or(0.0),
        );
    }
    if std::env::var("WORST").is_ok() {
        println!("worst frame {:.1} ms (frame {})", worst.0, worst.1);
    }
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
    let asleep: Vec<String> = q
        .iter(&app.world)
        .filter(|(_, b)| b.asleep())
        .map(|(n, _)| n.map_or("?".to_string(), |n| n.as_str().to_string()))
        .collect();
    println!("asleep: {asleep:?}");
    println!(
        "skinned meshes uploaded: {:.2} MB a frame, GPU skinning bones {:.1} KB a frame",
        uploaded as f64 / frames as f64 / 1e6,
        bone_bytes as f64 / frames as f64 / 1e3
    );
    // Each body's solver alone, with no colliders, with and without self-collision.
    if std::env::var("SOLO").is_ok() {
        for (n, b) in q.iter(&app.world) {
            // The fastest of five runs, so other work on the machine doesn't count.
            let time = |sim: &engine_softbody::solver::Sim, colliders| {
                (0..5)
                    .map(|_| {
                        let mut sim = sim.clone();
                        let t = Instant::now();
                        for _ in 0..10 {
                            sim.step(1.0 / 60.0, colliders);
                        }
                        t.elapsed().as_secs_f64() * 100.0
                    })
                    .fold(f64::MAX, f64::min)
            };
            let met = time(&b.sim, b.last_colliders());
            let kinds: Vec<&str> = b
                .last_colliders()
                .iter()
                .map(|c| match c.shape {
                    engine_softbody::collide::LocalShape::Sphere { .. } => "sphere",
                    engine_softbody::collide::LocalShape::Capsule { .. } => "capsule",
                    engine_softbody::collide::LocalShape::Cuboid { .. } => "box",
                    engine_softbody::collide::LocalShape::Cylinder { .. } => "cylinder",
                    engine_softbody::collide::LocalShape::Field(_) => "field",
                    engine_softbody::collide::LocalShape::Height(_) => "height",
                })
                .collect();
            println!("  with colliders {met:6.2} ms: {kinds:?}");
            let full = time(&b.sim, &[]);
            let mut plain = b.sim.clone();
            plain.params.self_collide = 0.0;
            let plain = time(&plain, &[]);
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
        }
    }
    println!(
        "render geometry on one thread: {:.2} ms per frame, {} vertices",
        t.elapsed().as_secs_f64() * 100.0,
        geo.vertices.len(),
    );
    // Hair and fur go to the strand renderer: their grooms are built once, on a worker.
    for b in bodies
        .iter()
        .filter(|b| b.strands.is_some() && !b.drawn_elsewhere)
    {
        let t = Instant::now();
        let Some(input) = engine_softbody::strand_draw::GroomInput::of(b) else {
            continue;
        };
        let groom = engine_softbody::strand_draw::build_groom(&input);
        println!(
            "groom: {} strands, {} points, {} guide points, built in {:.1} ms",
            groom.strand_count(),
            groom.points.len(),
            groom.guides.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
    // Resting bodies keep their mesh from frame to frame (`KeptMesh`), as the prepare does.
    let mut kept: Vec<engine_softbody::render::KeptMesh> =
        bodies.iter().map(|_| Default::default()).collect();
    let mut frame = |geo: &mut Geometry| {
        geo.clear();
        for (b, k) in bodies.iter().zip(&mut kept) {
            let base = (b.sim.origin - yard).as_vec3();
            geo.add_body_reusing(b, base, 0.5, px(b), Some(k));
        }
    };
    let t = Instant::now();
    for _ in 0..10 {
        geo.clear();
        for b in &bodies {
            geo.add_body(b, (b.sim.origin - yard).as_vec3(), 0.5, px(b));
        }
    }
    let fresh = t.elapsed().as_secs_f64() * 100.0;
    frame(&mut geo);
    let t = Instant::now();
    for _ in 0..10 {
        frame(&mut geo);
    }
    println!(
        "render geometry without hair: {fresh:.2} ms per frame built, {:.2} with resting \
         bodies kept ({} of {} resting)",
        t.elapsed().as_secs_f64() * 100.0,
        bodies.iter().filter(|b| b.frozen).count(),
        bodies.len(),
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
        "focus {distance} m from the yard: {ms:.2} ms per frame ({:.2} updating, {:.2} \
         stepping soft bodies); last step {:.2} ms, {} bodies ({} frozen), {} particles",
        updating / frames as f64,
        stepping / frames as f64,
        d.get("softbody.step_ms").unwrap_or(0.0),
        d.get("softbody.bodies").unwrap_or(0.0),
        d.get("softbody.frozen").unwrap_or(0.0),
        d.get("softbody.particles").unwrap_or(0.0),
    );
}

/// Flat ground that takes about as long to ask as a planet's terrain (as NPC's perf test).
struct SlowGround;

impl engine_world::GroundQuery for SlowGround {
    fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
        let mut h = 0.0;
        for i in 0..64 {
            h += (p.x * 0.01 * i as f64).sin() * (p.z * 0.013 * i as f64).cos();
        }
        Some(engine_world::GroundSample {
            height: h * 1e-12,
            normal: Vec3::Y,
            surface: engine_world::SurfaceId(3),
        })
    }
}

/// Soft bodies' main-thread time beside the physics step this frame, ms (`REALTIME`).
#[derive(Resource, Default)]
struct MainThread(f64);
