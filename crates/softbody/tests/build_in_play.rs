//! In play (a real clock), bodies that arrive together are built over a few frames instead of
//! all on one: a zone arriving with dozens of cloths, bridges and grooms once built them in
//! one frame (up to 135 ms on the cloud VM). A manual clock (tests, tours) builds them all at
//! once, the same on every machine.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, ClothPins, SoftBodyPlugin, SoftBodySim};
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
        let (x, z) = ((i % 8) as f64 * 3.0, (i / 8) as f64 * 3.0);
        app.world.spawn((
            Transform::from_xyz(x, 3.0, z),
            Cloth {
                size: [2.0, 2.0],
                resolution: [32, 32],
                pins: ClothPins::TopEdge { every: 1 },
                ..Cloth::default()
            },
        ));
    }
    app
}

fn built(app: &mut App) -> usize {
    let mut q = app.world.query::<&SoftBodySim>();
    q.iter(&app.world).count()
}

#[test]
fn in_play_many_new_bodies_are_built_over_a_few_frames() {
    let mut manual = app(true);
    manual.update();
    assert_eq!(
        built(&mut manual),
        BODIES,
        "a manual clock builds all at once"
    );

    let mut play = app(false);
    play.update();
    let first = built(&mut play);
    assert!(
        (1..BODIES).contains(&first),
        "the first frame in play built {first} of {BODIES}"
    );
    for _ in 0..600 {
        if built(&mut play) == BODIES {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(17));
        play.update();
    }
    panic!("only {} of {BODIES} were built", built(&mut play));
}

fn skeleton() -> engine_world::animation::SkeletonPose {
    use engine_world::animation::{BonePose, SkeletonPose};
    let at = |translation: Vec3| BonePose {
        translation,
        ..BonePose::default()
    };
    SkeletonPose {
        names: std::sync::Arc::from(vec!["pelvis".to_string(), "head".to_string()]),
        parents: std::sync::Arc::from(vec![None, Some(0)]),
        bones: vec![at(Vec3::new(0.0, 1.0, 0.0)), at(Vec3::new(0.0, 1.6, 0.0))],
    }
}

fn groom_app(manual: bool) -> (App, Entity) {
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
    let person = app.world.spawn((Transform::default(), skeleton())).id();
    let hair = app
        .world
        .spawn((
            Transform::default(),
            engine_softbody::HairGroom::long_hair("head", engine_softbody::HairStyle::default()),
            ChildOf(person),
        ))
        .id();
    (app, hair)
}

#[test]
fn in_play_a_groom_is_built_on_a_worker() {
    let (mut manual, hair) = groom_app(true);
    manual.update();
    assert!(
        manual.world.get::<SoftBodySim>(hair).is_some(),
        "a manual clock builds the groom on the frame"
    );

    let (mut play, hair) = groom_app(false);
    play.update();
    assert!(
        play.world.get::<SoftBodySim>(hair).is_none()
            && play
                .world
                .resource::<engine_softbody::systems::GroomBuilds>()
                .busy(),
        "in play the groom is handed to a worker, not built on the frame"
    );
    for _ in 0..600 {
        if play.world.get::<SoftBodySim>(hair).is_some() {
            assert!(
                !play
                    .world
                    .resource::<engine_softbody::systems::GroomBuilds>()
                    .busy()
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(17));
        play.update();
    }
    panic!("the groom was never built");
}
