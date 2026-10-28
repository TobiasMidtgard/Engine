//! Pulling cloth by hand with the use key: E on a curtain takes hold of it, walking away
//! drags it along, E again lets go.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::grab::Held;
use engine_softbody::{Cloth, ClothPins, SoftBodyPlugin, SoftBodySim};
use engine_world::WeatherState;
use engine_world::interact::{Interact, Interactable, Using};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    app
}

fn press(app: &mut App, actor: Entity, target: Entity) {
    let from = app.world.get::<Transform>(actor).unwrap().translation;
    app.world.write_message(Interact {
        actor,
        target,
        from,
    });
}

#[test]
fn e_takes_hold_of_a_curtain_and_walking_away_drags_it_along() {
    let mut app = app();
    // A curtain hung from a rail by its top corners, its hem about a meter off the floor.
    let curtain = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 2.6, 0.0),
            Cloth {
                size: [1.2, 1.6],
                resolution: [12, 16],
                pins: ClothPins::TopCorners,
                ..Cloth::default()
            },
        ))
        .id();
    // The player stands in front of it, facing it.
    let player = app.world.spawn(Transform::from_xyz(0.0, 0.0, 1.0)).id();
    app.update_n(60);
    let offer = app
        .world
        .get::<Interactable>(curtain)
        .expect("offered to E");
    assert_eq!(offer.verb, "Pull");
    assert!(offer.reach_m > 1.6, "reach {}", offer.reach_m);

    press(&mut app, player, curtain);
    app.update_n(2);
    let held = *app.world.get::<Held>(curtain).expect("held");
    assert_eq!(
        app.world.get::<Using>(player).map(|u| u.target),
        Some(curtain)
    );
    assert_eq!(
        app.world.get::<Interactable>(curtain).unwrap().verb,
        "Let go"
    );

    // Backing off 1.5 m (at walking pace), the hand brings the curtain's hem along.
    let start = app
        .world
        .get::<SoftBodySim>(curtain)
        .unwrap()
        .world_position(held.particle as usize);
    for _ in 0..90 {
        app.world
            .get_mut::<Transform>(player)
            .unwrap()
            .translation
            .z += 1.5 / 90.0;
        app.update();
    }
    app.update_n(30);
    let now = app
        .world
        .get::<SoftBodySim>(curtain)
        .unwrap()
        .world_position(held.particle as usize);
    assert!(
        now.z - start.z > 0.8,
        "the held bit moved {} m toward the player",
        now.z - start.z
    );

    // E again lets go, and it swings back under its rail.
    press(&mut app, player, curtain);
    app.update_n(2);
    assert!(app.world.get::<Held>(curtain).is_none(), "still held");
    assert!(app.world.get::<Using>(player).is_none(), "still using");
    app.update_n(240);
    let back = app
        .world
        .get::<SoftBodySim>(curtain)
        .unwrap()
        .world_position(held.particle as usize);
    assert!(back.z.abs() < 0.3, "it didn't swing back: z = {}", back.z);
}
