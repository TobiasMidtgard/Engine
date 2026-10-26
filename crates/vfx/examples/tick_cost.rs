//! Measures what the particle module costs the CPU per frame: the Spell court played next to
//! the camera and seven more copies of it out of the camera's reach (as the showcase's other
//! zones are), with no GPU. Prints milliseconds per frame for the module's systems, how many
//! effects play and rest, and how often they ask the ground under them, which on a real
//! terrain is the expensive part.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example tick_cost
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_vfx::VfxRuntime;

const STEP: f64 = 1.0 / 60.0;
const WARMUP: usize = 300;
const FRAMES: usize = 600;

fn main() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, engine_vfx::VfxPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    let lookups = Arc::new(AtomicU64::new(0));
    app.insert_resource(engine_world::Ground(Arc::new(Counting(lookups.clone()))));
    // The camera stands in the court; the other courts are 400 m and more away.
    let near = DVec3::new(0.0, 0.0, -150.0);
    app.world.spawn((
        Transform::from_translation(near + DVec3::new(6.0, 1.7, 12.0)),
        engine_world::SimulationFocus::default(),
    ));
    app.add_systems(Startup, move |mut commands: Commands| {
        engine_vfx::demo::spawn_demo(&mut commands, near);
        for k in 0..7 {
            let a = k as f64 * std::f64::consts::TAU / 7.0;
            let far = near + DVec3::new(a.cos(), 0.0, a.sin()) * (400.0 + 150.0 * k as f64);
            engine_vfx::demo::spawn_demo(&mut commands, far);
        }
    });
    app.update_n(WARMUP);

    lookups.store(0, Ordering::Relaxed);
    let t = std::time::Instant::now();
    app.update_n(FRAMES);
    let ms = t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
    let stats = app.world.resource::<VfxRuntime>().stats;
    println!(
        "{ms:.3} ms per frame for the whole app ({FRAMES} frames), {:.1} ground lookups per \
         frame; {} effects playing, {} resting, {} emitters drawn",
        lookups.load(Ordering::Relaxed) as f64 / FRAMES as f64,
        stats.effects,
        stats.resting,
        stats.emitters,
    );
}

/// Flat ground that counts how often it is asked.
struct Counting(Arc<AtomicU64>);

impl engine_world::GroundQuery for Counting {
    fn sample(&self, _p: DVec3) -> Option<engine_world::GroundSample> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Some(engine_world::GroundSample {
            height: 0.0,
            normal: Vec3::Y,
            surface: engine_world::SurfaceId(0),
        })
    }
}
