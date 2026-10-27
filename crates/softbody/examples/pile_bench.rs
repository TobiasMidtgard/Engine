//! The worst case for cloth self-collision: a 4 m silk sheet (the unveiling sheet's) dropped
//! from a corner onto the ground, where it lies crumpled in many layers. Prints how fast it
//! settles, how fast its particles still move, and what a step costs there.
//!
//! ```sh
//! cargo run -p engine_softbody --example pile_bench --release -- [seconds]
//! NOSELF=1 ...      # without self-collision
//! FRICTION=0 ...    # another friction (default 0.55); THICK=0.04 another thickness (m)
//! ```

use std::time::Instant;

use engine_core::prelude::*;
use engine_softbody::cloth::build;
use engine_softbody::{Cloth, ClothPins, SimSettings};

fn main() {
    let secs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(15);
    let spec = Cloth {
        size: [4.0, 4.0],
        resolution: [45, 45],
        pins: ClothPins::None,
        mass_per_m2: 0.16,
        shear_compliance: 1e-2,
        bend_compliance: 2e-2,
        sim: SimSettings {
            self_collide: std::env::var_os("NOSELF").is_none(),
            thickness: std::env::var("THICK")
                .ok()
                .and_then(|f| f.parse().ok())
                .unwrap_or(0.012),
            friction: std::env::var("FRICTION")
                .ok()
                .and_then(|f| f.parse().ok())
                .unwrap_or(0.55),
            damping: 0.12,
            ..SimSettings::default()
        },
        ..Cloth::default()
    };
    // Hanging from its top edge with the bottom 0.6 m up, then let go.
    let t = GlobalTransform::from(Transform::from_xyz(0.0, 4.6, 0.0));
    let mut body = build(&spec, Entity::PLACEHOLDER, &t, None, false).unwrap();
    let sim = &mut body.sim;
    sim.ground = vec![[0.0, 1.0, 0.0, 0.0]];
    let start = Instant::now();
    for _ in 0..secs * 60 {
        sim.step(1.0 / 60.0, &[]);
    }
    println!(
        "{secs} s simulated in {:.0} ms",
        start.elapsed().as_secs_f64() * 1000.0
    );
    let mut speeds = [0usize; 6];
    for v in &sim.vel {
        let s = v.length();
        speeds[[0.001, 0.01, 0.05, 0.2, 1.0]
            .iter()
            .position(|&l| s < l)
            .unwrap_or(5)] += 1;
    }
    println!("particles under 1 mm/s, 1 cm/s, 5 cm/s, 20 cm/s, 1 m/s, faster: {speeds:?}");
    // How far they really go in the next second (speeds can swing back and forth in place).
    let before = sim.pos.clone();
    let mut s = sim.clone();
    for _ in 0..60 {
        s.step(1.0 / 60.0, &[]);
    }
    let mut drift = [0usize; 6];
    for (a, b) in before.iter().zip(&s.pos) {
        drift[[0.001, 0.01, 0.05, 0.2, 1.0]
            .iter()
            .position(|&l| a.distance(*b) < l)
            .unwrap_or(5)] += 1;
    }
    let height = sim.pos.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    println!("moved in the next second, same bins (m): {drift:?}; pile {height:.2} m high");
    // The fastest of five runs, so other work on the machine doesn't count.
    let best = (0..5)
        .map(|_| {
            let mut s = sim.clone();
            let t = Instant::now();
            for _ in 0..30 {
                s.step(1.0 / 60.0, &[]);
            }
            t.elapsed().as_secs_f64() * 1000.0 / 30.0
        })
        .fold(f64::MAX, f64::min);
    println!(
        "a step: {best:.2} ms, {} self-collision pairs, {} of them solved",
        sim.self_pairs(),
        sim.self_pairs_solved()
    );
}
