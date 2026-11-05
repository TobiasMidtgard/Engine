//! Times the fire field's step on a big grass fire, without the ECS or a renderer.
//!
//! ```text
//! cargo run --release -p engine_fire --example field_bench -- [secs] [rain_mm_h]
//! ```
//!
//! A 300 m square of cured grass that regrows (so burnt tiles stay awake, as in the showcase),
//! lit along a 60 m line in a 3 m/s wind, stepped at the showcase's 20 Hz. Prints the mean and
//! worst step over the second half and the tile counts, for A/B runs of the field's code.

use engine_core::math::{DVec3, Vec3};
use engine_fire::field::{FireField, SpreadParams, StepInput};
use engine_fire::fuel::{CellFuel, FuelKind};

fn grass(_p: DVec3) -> (CellFuel, f64) {
    (
        CellFuel {
            kind: FuelKind::Grass,
            load_kg_m2: 0.35,
            moisture: 0.06,
            ignition_c: 300.0,
            regrow_secs: 120.0,
        },
        0.0,
    )
}

fn main() {
    engine_core::tasks::init_task_pools();
    let mut args = std::env::args().skip(1);
    let secs: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(60.0);
    let rain: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(0.0);
    let mut field = FireField::new(0.5);
    field.max_tiles = 4096;
    field.new_tiles_per_step = 2;
    let mut fuel = grass;
    // Make the square first, so the timed steps are the fire's, not the tiles' building.
    for x in -10..10 {
        for z in -10..10 {
            let key = engine_core::math::IVec2::new(x, z);
            field.ensure_tile(key, 20.0, &mut fuel);
        }
    }
    for k in -60..=60 {
        field.ignite(DVec3::new(k as f64 * 0.5, 0.0, -60.0), 1.0, &mut fuel);
    }
    let wind = |_p: DVec3| Vec3::new(1.0, 0.0, 3.0);
    let wet = |_p: DVec3| 0.0f32;
    let dt = 1.0 / 20.0;
    let steps = (secs / dt) as usize;
    let mut times = Vec::with_capacity(steps);
    for _ in 0..steps {
        let input = StepInput {
            dt,
            ambient_c: 20.0,
            rain_mm_h: rain,
            humidity: 0.4,
            wind: &wind,
            wetness: &wet,
            params: SpreadParams::default(),
            near: None,
        };
        let t = std::time::Instant::now();
        field.step(&input, &mut fuel);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let late = &times[steps / 2..];
    let mean = late.iter().sum::<f64>() / late.len() as f64;
    let worst = late.iter().cloned().fold(0.0, f64::max);
    // A checksum of every cell, to check that a faster step gives the same fire.
    let mut sum = 0u64;
    for t in &field.tiles {
        for c in &t.cells {
            for v in [c.burn, c.heat, c.temp, c.fuel, c.glow, c.scorch, c.hrr] {
                sum = sum.rotate_left(5) ^ v.to_bits() as u64;
            }
        }
    }
    let s = &field.stats;
    println!(
        "field_bench: {secs} s, mean step {mean:.2} ms, worst {worst:.2} ms over the second \
         half; tiles {} (active {}), burning {:.0} m2, cells {sum:016x}",
        s.tiles, s.active_tiles, s.burning_m2
    );
}
