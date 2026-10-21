//! The tarantula rears up in threat: its front legs leave the ground while the back legs stay
//! planted, and come back down onto their footholds when it settles.

use engine_anim::AnimationPlugin;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{AnimationParams, BodyRequest, SkeletonPose};

const STEP: f64 = 1.0 / 60.0;

fn tip(app: &App, e: Entity, side: &str, leg: usize) -> DVec3 {
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    let gt = app.world.get::<GlobalTransform>(e).unwrap();
    let i = pose.index_of(&format!("leg_{side}{leg}_tip")).unwrap();
    pose.bone_world(gt, i).unwrap().0
}

fn run(app: &mut App, secs: f64) {
    for _ in 0..(secs / STEP).round() as usize {
        app.update();
    }
}

#[test]
fn a_tarantula_rears_up_on_its_back_legs_and_comes_back_down() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let t = Transform::default();
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::new("spider")))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    // A short walk, then it stops.
    for _ in 0..90 {
        app.world.get_mut::<Transform>(e).unwrap().translation.z -= 0.8 * STEP;
        app.update();
    }
    run(&mut app, 1.0);
    let feet = |app: &App| -> Vec<DVec3> {
        ["l", "r"]
            .iter()
            .flat_map(|s| (1..=4).map(move |leg| (*s, leg)))
            .map(|(s, leg)| tip(app, e, s, leg))
            .collect()
    };
    let standing = feet(&app);
    for f in &standing {
        assert!(f.y < 0.06, "a foot off the ground before the threat: {f}");
    }
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set("threat", 1.0);
    run(&mut app, 1.0);
    let rearing = feet(&app);
    for (k, (now, was)) in rearing.iter().zip(&standing).enumerate() {
        let leg = k % 4 + 1;
        if leg == 1 {
            assert!(now.y > 0.5, "first leg {k} only up to {now}");
        } else if leg == 2 {
            assert!(now.y > 0.25, "second leg {k} only up to {now}");
        } else {
            assert!(
                now.distance(*was) < 0.03,
                "back leg {k} moved from {was} to {now}"
            );
        }
    }
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set("threat", 0.0);
    run(&mut app, 1.0);
    for (now, was) in feet(&app).iter().zip(&standing) {
        assert!(
            now.distance(*was) < 0.03,
            "a foot came down at {now}, not {was}"
        );
    }
}
