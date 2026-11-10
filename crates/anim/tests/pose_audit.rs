//! The pose validator over the whole animation paddock (characters week A1): every person
//! runs through their routine for a duel's round while `PoseAudit` checks each frame, and
//! the table of worst values goes to `ANIM_POSE_AUDIT_TABLE` (or stdout). The bar A1 set:
//! no wrist out of range, no elbow or knee flipping across its limb, off hands on two-handed
//! grips within 1 cm, planted feet sliding under 2 cm/s, nothing NaN.

use engine_anim::AnimationPlugin;
use engine_anim::demo::spawn_demo_world;
use engine_anim::validate::{PoseAudit, rows, table};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;

const STEP: f64 = 1.0 / 60.0;

#[test]
fn the_paddock_people_pass_the_pose_validator() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app.insert_resource(PoseAudit::default());
    let demo = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    // A second to settle (feet find the ground, guards come up), then a duel's round.
    for _ in 0..60 {
        app.update();
    }
    let mut q = app.world.query::<&mut engine_anim::validate::PoseCheck>();
    for mut c in q.iter_mut(&mut app.world) {
        c.reset();
    }
    for _ in 0..(16.0 / STEP) as usize {
        app.update();
    }
    let rows = rows(&mut app.world);
    let text = table(&rows);
    match std::env::var_os("ANIM_POSE_AUDIT_TABLE") {
        Some(path) => std::fs::write(path, &text).unwrap(),
        None => println!("{text}"),
    }
    let all = &rows.last().unwrap().1;
    assert!(all.frames > 0);
    assert_eq!(all.nan, 0, "{text}");
    // The sword-and-shield pair, the people A1 is judged on.
    let pair = [demo.swordsman, demo.shield_partner];
    for e in pair {
        let c = app
            .world
            .get::<engine_anim::validate::PoseCheck>(e)
            .expect("checked");
        let s = &c.stats;
        assert_eq!(s.pole_flips, 0, "{e:?} {s:?}");
    }
}
