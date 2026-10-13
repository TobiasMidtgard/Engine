//! Measures how far the proving ground's crash test dummies' joints pull apart as they tumble
//! down the stairs, with constraint joints and with articulated ones
//! (`PhysicsSettings::articulated_joints`). Headless, no GPU needed.
//!
//! ```sh
//! cargo run -p engine_physics --release --example joint_gap -- [out.csv]
//! ```
//!
//! Prints each run's widest gap and writes the widest gap of every frame (mm) to the CSV.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsSettings, demo};
use engine_world::WorldPlugin;
use engine_world::physics::{Joint, JointKind};

/// Ten seconds: the dummies' fall down the stairs and their settling at the bottom.
const FRAMES: usize = 600;

fn main() {
    let out = std::env::args().nth(1).unwrap_or("joint_gap.csv".into());
    let runs = [false, true].map(|articulated| {
        let mut app = proving_ground(articulated);
        let gaps: Vec<f64> = (0..FRAMES)
            .map(|_| {
                app.update();
                widest_gap(&mut app.world)
            })
            .collect();
        let (frame, widest) = gaps
            .iter()
            .enumerate()
            .fold((0, 0.0), |a, (f, g)| if *g > a.1 { (f + 1, *g) } else { a });
        let name = if articulated {
            "articulated"
        } else {
            "constraint"
        };
        println!(
            "{name} joints: widest gap {:.1} mm (frame {frame})",
            widest * 1000.0
        );
        gaps
    });
    let mut csv = String::from("frame,seconds,constraint_mm,articulated_mm\n");
    for f in 0..FRAMES {
        csv += &format!(
            "{},{:.4},{:.2},{:.2}\n",
            f + 1,
            (f + 1) as f64 / 60.0,
            runs[0][f] * 1000.0,
            runs[1][f] * 1000.0
        );
    }
    std::fs::write(&out, csv).expect("write the CSV");
    println!("{out}");
}

/// The proving ground at 60 Hz, headless.
fn proving_ground(articulated: bool) -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<PhysicsSettings>()
        .articulated_joints = articulated;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    app
}

/// The widest gap now between the anchors of a dummy's joints, meters.
fn widest_gap(world: &mut World) -> f64 {
    let joints: Vec<Joint> = world.query::<&Joint>().iter(world).cloned().collect();
    let station = demo::Station::Ragdolls.base();
    let mut widest = 0.0f64;
    for j in joints {
        let held = matches!(
            j.kind,
            JointKind::Fixed | JointKind::Spherical { .. } | JointKind::Revolute { .. }
        );
        let (Some(b), true) = (j.body_b, held) else {
            continue;
        };
        let (Some(ta), Some(tb)) = (world.get::<Transform>(j.body_a), world.get::<Transform>(b))
        else {
            continue;
        };
        if ta.translation.distance(station) > 12.0 {
            continue;
        }
        let pa = ta.translation + (ta.rotation * j.anchor_a).as_dvec3();
        let pb = tb.translation + (tb.rotation * j.anchor_b.as_vec3()).as_dvec3();
        widest = widest.max(pa.distance(pb));
    }
    widest
}
