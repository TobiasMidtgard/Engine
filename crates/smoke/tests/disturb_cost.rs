//! What `smoke::disturb` costs over one round of the range with the camera far away (run by
//! hand: `cargo test --release -p engine_smoke --test disturb_cost -- --ignored --nocapture`).

use bevy_ecs::system::RunSystemOnce;
use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_smoke::SmokePlugin;
use engine_smoke::demo::spawn_demo;
use engine_world::FocusPoints;

#[test]
#[ignore]
fn disturb_cost() {
    engine_core::tasks::init_task_pools();
    for far in [false, true] {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(SmokePlugin);
        let origin = DVec3::new(100.0, 0.0, 50.0);
        {
            let mut commands = app.world.commands();
            spawn_demo(&mut commands, origin);
        }
        app.world.flush();
        let focus = if far {
            origin + DVec3::new(-360.0, 2.0, 15.0)
        } else {
            origin + DVec3::new(0.0, 2.0, -20.0)
        };
        app.world.resource_mut::<FocusPoints>().points = vec![focus];
        app.update_n(8);
        // One round of the range (50 s), in 5 s windows: what `disturb` alone costs a step.
        for window in 0..10 {
            let mut spent = 0.0;
            let mut worst = 0.0f64;
            for _ in 0..300 {
                app.update();
                // Again, alone and timed: the messages are read, so this is the fans and the
                // obstacle (the bullets and blasts were in the update's own run).
                let started = std::time::Instant::now();
                let _ = app.world.run_system_once(engine_smoke::sim::disturb);
                let ms = started.elapsed().as_secs_f64() * 1e3;
                spent += ms;
                worst = worst.max(ms);
            }
            println!(
                "far {far} {:>2}-{:>2} s: disturb {:.3} ms a step, worst {:.3}",
                window * 5,
                window * 5 + 5,
                spent / 300.0,
                worst
            );
        }
    }
}
