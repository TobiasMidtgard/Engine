//! Far beyond the simulation range a creature rests: it keeps its pose and takes a fresh one
//! only every few seconds. Brought back in range it poses every frame again, with its feet
//! back on the ground.

use engine_anim::{AnimationPlugin, Animator};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{BodyRequest, SkeletonPose};
use engine_world::{SimulationFocus, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

fn pose(app: &App, e: Entity) -> Vec<Vec3> {
    app.world
        .get::<SkeletonPose>(e)
        .unwrap()
        .bones
        .iter()
        .map(|b| b.translation)
        .collect()
}

#[test]
fn a_far_creature_rests_and_wakes_when_the_focus_comes_near() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let far = Transform::from_xyz(0.0, 0.0, -400.0);
    let focus = app
        .world
        .spawn((
            Transform::default(),
            GlobalTransform::default(),
            SimulationFocus::default(),
        ))
        .id();
    let e = app
        .world
        .spawn((far, GlobalTransform::from(far), BodyRequest::new("spider")))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    // It walks along, out there.
    let walk = |app: &mut App| {
        app.world.get_mut::<Transform>(e).unwrap().translation.z -= 0.8 * STEP;
        app.update();
    };
    for _ in 0..30 {
        walk(&mut app);
    }
    let mut poses = 0;
    let mut last = pose(&app, e);
    for _ in 0..(5.0 / STEP) as usize {
        walk(&mut app);
        let now = pose(&app, e);
        if now != last {
            poses += 1;
        }
        last = now;
        assert!(app.world.get::<Animator>(e).unwrap().resting());
    }
    assert!(
        (1..=3).contains(&poses),
        "{poses} poses in 5 s at rest (one every 2 s expected)"
    );

    // The focus comes over: it poses every frame from the next one on.
    let at = app.world.get::<Transform>(e).unwrap().translation + DVec3::new(0.0, 0.0, 20.0);
    app.world.get_mut::<Transform>(focus).unwrap().translation = at;
    app.update();
    app.update();
    let animator = app.world.get::<Animator>(e).unwrap();
    assert!(!animator.resting() && animator.posed());
    let mut last = pose(&app, e);
    for _ in 0..60 {
        walk(&mut app);
        let now = pose(&app, e);
        assert_ne!(now, last, "a pose missed while walking in range");
        last = now;
    }
    for _ in 0..60 {
        app.update();
    }
    // Standing again with every foot on the ground (the gait planted them afresh).
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    let gt = app.world.get::<GlobalTransform>(e).unwrap();
    for side in ["l", "r"] {
        for leg in 1..=4 {
            let i = pose.index_of(&format!("leg_{side}{leg}_tip")).unwrap();
            let tip = pose.bone_world(gt, i).unwrap().0;
            assert!(
                tip.y < 0.06,
                "leg {side}{leg} off the ground after waking: {tip}"
            );
        }
    }
}
