//! A world shift (floating origin, planet travel) moves the paddock without changing anything
//! anyone sees: two identical paddocks, one shifted halfway through, keep the same poses
//! (planted feet, swinging hair and tails, jiggling muscles, scripted swings) and the same
//! places relative to the world.

use engine_anim::demo::{DemoEntities, spawn_demo_world};
use engine_anim::{AnimationPlugin, Animator};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::{WorldPlugin, WorldShift};

const STEP: f64 = 1.0 / 60.0;

fn app() -> (App, DemoEntities) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let demo = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    (app, demo)
}

fn run(app: &mut App, seconds: f64) {
    for _ in 0..(seconds / STEP).round() as usize {
        app.update();
    }
}

#[test]
fn a_world_shift_changes_nothing_anyone_sees() {
    let (mut still, d) = app();
    let (mut moved, _) = app();
    // Mid-swing for the athlete, mid-stride for the walker, horse, spider and car, the
    // stuntman on the stairs, the shuttle runner down his lane.
    run(&mut still, 3.0);
    run(&mut moved, 3.0);
    let shift = DVec3::new(-250.0, 0.0, 400.0);
    moved.world.write_message(WorldShift::translate(shift));
    run(&mut still, 2.0);
    run(&mut moved, 2.0);

    let people = [
        ("athlete", d.athlete),
        ("lifter", d.lifter),
        ("reacher", d.reacher),
        ("walker", d.walker),
        ("horse", d.horse),
        ("spider", d.spider),
        ("car", d.car),
        ("stuntman", d.stuntman),
        ("shuttle runner", d.runner),
        ("coach", d.coach),
    ];
    for (name, e) in people {
        let a = still.world.get::<GlobalTransform>(e).unwrap();
        let b = moved.world.get::<GlobalTransform>(e).unwrap();
        let drift = (b.translation() - shift).distance(a.translation());
        assert!(
            drift < 2e-3,
            "{name} drifted {drift} m from where it should be"
        );
        let pose_a = &still.world.get::<Animator>(e).unwrap().model;
        let pose_b = &moved.world.get::<Animator>(e).unwrap().model;
        let skeleton = &still.world.get::<Animator>(e).unwrap().skeleton;
        for (bone, (x, y)) in pose_a.iter().zip(pose_b).enumerate() {
            let name_b = &skeleton.names[bone];
            assert!(
                x.t.distance(y.t) < 2e-3 && x.r.angle_between(y.r) < 2e-2,
                "{name}'s {name_b} moved in the shift: {} vs {} m, {:.3} rad",
                x.t,
                y.t,
                x.r.angle_between(y.r)
            );
        }
    }
}
