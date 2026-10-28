//! The yard's pedestal turret on the use key: E fires a burst at what the player looks at,
//! and the turret holds off its own pattern while the player is at it.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::SoftBodyPlugin;
use engine_softbody::demo::{DemoClock, DemoTurret};
use engine_world::{Interact, Interactable, ProjectileTrace};

#[derive(Resource, Default)]
struct Shots(Vec<(DVec3, DVec3)>);

fn record(mut shots: ResMut<Shots>, mut traces: MessageReader<ProjectileTrace>) {
    shots.0.extend(traces.read().map(|t| (t.from, t.to)));
}

#[test]
fn e_fires_a_burst_where_the_player_looks() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<Shots>().add_systems(Last, record);
    // Its own pattern aims at a curtain ahead (−Z); the player looks off to the right (+X).
    let turret = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.2, 0.0),
            DemoTurret::new(DVec3::new(0.0, 3.0, -8.0), Vec2::splat(1.0), 0.7, 0.5),
            DemoClock::default(),
            Interactable::new("Fire").reach(2.5),
        ))
        .id();
    let eye = DVec3::new(0.0, 1.7, 1.5);
    let player = app.world.spawn(Transform::from_translation(eye)).id();
    app.world.spawn((
        engine_render::Camera::perspective(60.0),
        Transform::from_translation(eye).looking_at(eye + DVec3::new(10.0, 0.0, 0.0), Vec3::Y),
    ));
    app.update_n(60);
    assert!(
        !app.world.resource::<Shots>().0.is_empty(),
        "its own pattern fires"
    );
    app.world.resource_mut::<Shots>().0.clear();

    app.world.write_message(Interact {
        actor: player,
        target: turret,
        from: eye,
    });
    // A second and a half: the burst, and none of its own shots meanwhile.
    app.update_n(90);
    let shots = std::mem::take(&mut app.world.resource_mut::<Shots>().0);
    assert_eq!(shots.len(), 6, "a six-round burst");
    for (from, to) in &shots {
        let dir = (*to - *from).normalize();
        assert!(
            dir.x > 0.95,
            "a shot went {dir}, not where the player looks"
        );
    }
    assert!(app.world.get::<Interactable>(turret).unwrap().enabled);
    // It waits for the player a while before going back to its pattern.
    app.update_n(240);
    assert!(
        app.world.resource::<Shots>().0.is_empty(),
        "it went back too soon"
    );
    app.update_n(360);
    let own = &app.world.resource::<Shots>().0;
    assert!(!own.is_empty(), "it never went back to its pattern");
    assert!(
        own.iter().all(|(f, t)| (*t - *f).z < 0.0),
        "its own shots go at the curtain"
    );
}
