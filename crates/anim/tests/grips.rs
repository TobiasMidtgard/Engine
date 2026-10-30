//! Held weapons in the paddock: the great sword's off hand closes on the grip below the main
//! hand, turned the same way round (with or without the fetched models), and on the
//! paddock's imported people (when fetched) each hand holding something has its palm on the
//! handle, its fingers round it without sinking in, and its thumb on the far side.

use engine_anim::build::weapons::{BLADE_AXIS, GREATSWORD_HANDS};
use engine_anim::demo::spawn_demo_world;
use engine_anim::grip::{GRIP, HeldItems, fist_correction};
use engine_anim::retarget::Retarget;
use engine_anim::xform::Xf;
use engine_anim::{AnimationPlugin, Animator};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app
}

#[test]
fn the_paddocks_people_hold_their_weapons_in_their_hands() {
    let mut app = app();
    let d = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    let people = [
        ("swordsman", d.swordsman),
        ("shield partner", d.shield_partner),
        ("great sword", d.greatsword),
        ("archer", d.archer),
    ];
    let mut checked = 0;
    let mut unopposed = 0;
    let mut two_handed = 0;
    for frame in 0..(10.0 * 60.0) as usize {
        app.update();
        // Past the first frames (the models are put on and the hands find their grips).
        if frame < 10 {
            continue;
        }
        // The great sword's left fist where the right one puts it on the grip.
        let w = &app.world;
        let a = w.get::<Animator>(d.greatsword).unwrap();
        if a.curve("grip") > 0.99 {
            let retargeter = w.get::<Retarget>(d.greatsword).map(|r| &*r.retargeter);
            let bone = |b: &str| a.model[a.skeleton.index_of(b).unwrap()];
            let fist = |b: &str, side| bone(b).mul(&fist_correction(retargeter, side));
            let want = fist("hand_r", 1)
                .mul(&Xf::from_t(-BLADE_AXIS * GREATSWORD_HANDS))
                .mul(&Xf::from_t(GRIP));
            let got = fist("hand_l", 0).mul(&Xf::from_t(GRIP));
            assert!(
                got.t.distance(want.t) < 0.01,
                "left fist {} off {}",
                got.t,
                want.t
            );
            let turn = got.r.angle_between(want.r).to_degrees();
            assert!(turn < 3.0, "the left fist is turned {turn}° off the grip");
            two_handed += 1;
        }
        for (who, e) in people {
            let w = &app.world;
            let (Some(a), Some(r), Some(held)) = (
                w.get::<Animator>(e),
                w.get::<Retarget>(e),
                w.get::<HeldItems>(e),
            ) else {
                continue;
            };
            for side in 0..2 {
                let Some(handle) = held.hands[side] else {
                    continue;
                };
                let Some(report) =
                    r.retargeter
                        .grip_report(&a.model, &a.skeleton, &r.model, side, &handle)
                else {
                    continue;
                };
                assert!(report.palm.abs() < 0.004, "{who} {side}: {report:?}");
                assert!(report.deepest < 0.005, "{who} {side}: {report:?}");
                assert!(report.widest < 0.006, "{who} {side}: {report:?}");
                unopposed += usize::from(!report.thumb_opposed);
                checked += 1;
            }
        }
    }
    assert!(
        two_handed > 60,
        "the great sword was held two-handed {two_handed} frames"
    );
    if checked == 0 {
        eprintln!("the paddock's people are not fetched: nothing to check");
        return;
    }
    assert!(
        unopposed * 10 < checked,
        "the thumb was off the far side in {unopposed} of {checked} frames"
    );
}
