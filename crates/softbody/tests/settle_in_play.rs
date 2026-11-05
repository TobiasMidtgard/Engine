//! In play (a real clock), bodies that arrive together settle over a few frames instead of
//! all settling in one: the settling count misses collisions, so a zone of capes and
//! curtains once settled 232 ms in one frame on the session VM. A manual clock (tests,
//! tours) settles by count alone, the same on every machine.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, ClothPins, SimSettings, SoftBodyPlugin, SoftBodySim};
use engine_world::WeatherState;

const BODIES: usize = 96;

fn app(manual: bool) -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    if manual {
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
    }
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    for i in 0..BODIES {
        let (x, z) = ((i % 12) as f64 * 2.0, (i / 12) as f64 * 2.0);
        app.world.spawn((
            Transform::from_xyz(x, 3.0, z),
            Cloth {
                size: [1.0, 1.0],
                resolution: [10, 10],
                pins: ClothPins::TopEdge { every: 1 },
                sim: SimSettings {
                    prewarm_secs: 4.0,
                    ..SimSettings::default()
                },
                ..Cloth::default()
            },
        ));
    }
    app
}

fn pending(app: &mut App) -> Vec<u32> {
    let mut q = app.world.query::<&SoftBodySim>();
    q.iter(&app.world).map(|b| b.pending_prewarm).collect()
}

/// Update until some body has settled a little; what each still has to settle then.
fn first_settling(app: &mut App, sleep: bool) -> (u32, Vec<u32>) {
    let full = 240;
    for _ in 0..500 {
        if sleep {
            std::thread::sleep(std::time::Duration::from_millis(17));
        }
        app.update();
        let now = pending(app);
        if now.len() == BODIES && now.iter().any(|&p| p < full) {
            return (full, now);
        }
    }
    panic!("nothing settled");
}

#[test]
fn in_play_bodies_that_arrive_together_settle_over_a_few_frames() {
    // By count: every curtain settles as much as it may in the first step.
    let mut manual = app(true);
    let (full, now) = first_settling(&mut manual, false);
    assert!(
        now.iter().all(|&p| p < full),
        "with a manual clock all settle at once: {now:?}"
    );

    // In play the step's settling stops at its time budget, so many wait for a later frame.
    let mut play = app(false);
    let (full, now) = first_settling(&mut play, true);
    let waiting = now.iter().filter(|&&p| p + 2 >= full).count();
    assert!(
        waiting >= BODIES / 4,
        "only {waiting} of {BODIES} curtains waited: {now:?}"
    );
    // And all of them settle in the end.
    for _ in 0..600 {
        if pending(&mut play).iter().all(|&p| p == 0) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(17));
        play.update();
    }
    panic!("not all settled: {:?}", pending(&mut play));
}
