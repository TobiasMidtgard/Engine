//! A person struck flinches away from the blow and recovers, with or without the fetched
//! models.

use engine_anim::AnimationPlugin;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{BodyRequest, SkeletonPose};
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

/// A standing person, settled for a second.
fn person() -> (App, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let t = Transform::default();
    // The flinch alone: no knock-back step (tests/stagger.rs has that).
    let reaction = engine_anim::HitReaction {
        stagger_above: 0.0,
        ..Default::default()
    };
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::human(0), reaction))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    for _ in 0..60 {
        app.update();
    }
    (app, e)
}

fn head(app: &App, e: Entity) -> DVec3 {
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    let gt = app.world.get::<GlobalTransform>(e).unwrap();
    let i = pose
        .index_of("head")
        .or_else(|| pose.index_of("Bip01 Head"))
        .unwrap();
    pose.bone_world(gt, i).unwrap().0
}

#[test]
fn a_punched_head_snaps_back_and_recovers() {
    // The same person twice, one of them punched in the face from the front (they face −Z).
    let (mut hit, e) = person();
    let (mut calm, c) = person();
    let at = head(&hit, e);
    hit.world.write_message(DamageEvent {
        target: e,
        amount: 10.0,
        kind: DamageKind::Blunt,
        point: at + DVec3::new(0.0, 0.0, -0.1),
        impulse: Vec3::new(0.0, 0.0, 30.0),
        source: None,
    });
    let mut most = 0.0f64;
    let mut gap = 0.0;
    for frame in 0..120 {
        hit.update();
        calm.update();
        let d = head(&hit, e) - head(&calm, c);
        if frame < 20 {
            most = most.max(d.z);
        }
        gap = d.length();
    }
    assert!(most > 0.05, "the head went back only {most:.3} m");
    assert!(gap < 0.01, "the head is {gap:.3} m off after 2 s");
    assert!(hit.world.get::<engine_anim::HitReaction>(e).is_some());
}
