//! The statue's crane on the use key: E on its winch pulls the sheet up off the statue or
//! lowers it back on, and the hoist stays where the player leaves it.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::SoftBodyPlugin;
use engine_softbody::demo::statue::{self, Winch};
use engine_softbody::demo::{DemoMotion, HoistByHand};
use engine_world::{Interact, Interactable};

#[test]
fn e_on_the_winch_pulls_the_sheet_up_and_lowers_it() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.add_systems(Startup, |mut commands: Commands| {
        statue::spawn_unveiling(&mut commands, DVec3::ZERO, Vec3::new(0.0, 0.0, -1.0));
    });
    app.update_n(30);
    let mut q = app.world.query::<(Entity, &Winch)>();
    let (winch, &Winch { sheet }) = q.single(&app.world).expect("one winch");
    let DemoMotion::Hoist { low, high, .. } = *app.world.get::<DemoMotion>(sheet).unwrap() else {
        panic!("the sheet hangs on a hoist");
    };
    let hook = |app: &App| app.world.get::<Transform>(sheet).unwrap().translation;
    let verb = |app: &App| app.world.get::<Interactable>(winch).unwrap().verb.clone();
    // Half a second in, its own cycle still holds the sheet up.
    assert!((hook(&app) - high).length() < 0.01);
    assert_eq!(verb(&app), statue::LOWER);
    let press = |app: &mut App| {
        app.world.write_message(Interact {
            actor: winch,
            target: winch,
            from: DVec3::ZERO,
        });
    };

    press(&mut app);
    app.update_n(2);
    assert_eq!(verb(&app), statue::PULL_UP);
    app.update_n(6 * 60);
    assert!(
        (hook(&app) - low).length() < 0.01,
        "lowered: the hook is {} m from the crown",
        (hook(&app) - low).length()
    );

    press(&mut app);
    app.update_n(3 * 60);
    assert!((hook(&app) - high).length() < 0.01, "pulled up");
    assert_eq!(verb(&app), statue::LOWER);
    // It stays up: its own cycle would have lowered it again by now.
    app.update_n(20 * 60);
    assert!(
        (hook(&app) - high).length() < 0.01,
        "it went back to its cycle"
    );
    assert!(!app.world.get::<HoistByHand>(sheet).unwrap().cover);
}

#[test]
fn taking_over_the_hoist_does_not_jump() {
    for k in [0.0, 0.2, 0.5, 0.8, 1.0] {
        let h = HoistByHand::take_over(k);
        let eased = h.down * h.down * (3.0 - 2.0 * h.down);
        assert!((eased - k).abs() < 1e-4, "{k} -> {eased}");
    }
}
