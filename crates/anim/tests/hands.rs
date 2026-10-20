//! A requested person's hands close as their clips and what they hold say (the
//! `HandControls` contract), with or without the fetched models.

use engine_anim::AnimationPlugin;
use engine_anim::retarget::HandGrip;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{AnimationParams, BodyRequest};
use engine_world::character::HandControls;

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
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::human(0)))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    for _ in 0..10 {
        app.update();
    }
    (app, e)
}

fn grip(app: &App, e: Entity) -> [f32; 2] {
    app.world.get::<HandControls>(e).expect("HandControls").grip
}

#[test]
fn hands_close_on_what_they_hold_and_into_a_guard() {
    let (mut app, e) = person();
    let open = grip(&app, e);
    assert!(open[0] < 0.2 && open[1] < 0.2, "standing hands {open:?}");

    app.world.entity_mut(e).insert(HandGrip {
        left: 0.0,
        right: 1.0,
    });
    app.update();
    let held = grip(&app, e);
    assert!(
        held[1] > 0.99 && held[0] < 0.2,
        "holding in the right {held:?}"
    );

    app.world.entity_mut(e).remove::<HandGrip>();
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set("guarding", 1.0);
    for _ in 0..60 {
        app.update();
    }
    let fists = grip(&app, e);
    assert!(fists[0] > 0.8 && fists[1] > 0.8, "guard fists {fists:?}");

    app.world.entity_mut(e).remove::<BodyRequest>();
    app.update();
    assert!(app.world.get::<HandControls>(e).is_none());
}
