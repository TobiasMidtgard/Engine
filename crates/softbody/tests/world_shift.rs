//! Cloth and ropes keep hanging from what holds them when the world is re-anchored (a
//! floating-origin move or a new planet's frame), the way `engine_space` does it: a
//! `WorldShift` written in `WorldShiftSet::Write`.

use engine_core::math::DQuat;
use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, ClothPins, Rope, RopeEnd, SimSettings, SoftBodyPlugin, SoftBodySim};
use engine_world::{WorldShift, WorldShiftSet};

#[derive(Resource, Default)]
struct PendingShift(Option<WorldShift>);

fn write_pending_shift(mut pending: ResMut<PendingShift>, mut shifts: MessageWriter<WorldShift>) {
    if let Some(s) = pending.0.take() {
        shifts.write(s);
    }
}

fn particles(app: &mut App) -> Vec<Vec<DVec3>> {
    let mut q = app.world.query::<&SoftBodySim>();
    let mut bodies: Vec<_> = q.iter(&app.world).collect();
    bodies.sort_by_key(|b| b.sim.len());
    bodies
        .iter()
        .map(|b| (0..b.sim.len()).map(|i| b.world_position(i)).collect())
        .collect()
}

/// No wind, so the bodies come to rest and any drift after the shift is the shift's fault.
fn still() -> SimSettings {
    SimSettings {
        wind_scale: 0.0,
        damping: 2.0,
        ..SimSettings::default()
    }
}

#[test]
fn cloth_and_ropes_follow_a_world_shift() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<PendingShift>()
        .add_systems(PostUpdate, write_pending_shift.in_set(WorldShiftSet::Write));
    // A curtain hanging from its top edge, a rope from its entity to a fixed world point.
    app.world.spawn((
        Transform::from_xyz(3.0, 4.0, -2.0),
        Cloth {
            size: [1.2, 1.0],
            resolution: [12, 10],
            pins: ClothPins::TopEdge { every: 1 },
            sim: still(),
            ..Cloth::default()
        },
    ));
    app.world.spawn((
        Transform::from_xyz(-2.0, 5.0, 1.0),
        Rope {
            end_a: RopeEnd::Here { offset: [0.0; 3] },
            end_b: RopeEnd::World {
                at: [-0.5, 5.0, 1.0],
            },
            length: 2.0,
            sim: still(),
            ..Rope::default()
        },
    ));
    app.update_n(240);
    let before = particles(&mut app);
    assert_eq!(before.len(), 2);

    // Re-anchor 40 km away and turn the world about the vertical (the default flat ground
    // stays at y = 0, so no vertical move).
    let shift = WorldShift {
        rotation: DQuat::from_rotation_y(0.9),
        translation: DVec3::new(4.0e4, 0.0, -1.5e4),
        ..WorldShift::IDENTITY
    };
    app.world.resource_mut::<PendingShift>().0 = Some(shift);
    app.update();
    let after = particles(&mut app);
    for (body, (b, a)) in before.iter().zip(&after).enumerate() {
        for (i, (b, a)) in b.iter().zip(a).enumerate() {
            let want = shift.point(*b);
            assert!(
                (*a - want).length() < 0.05,
                "body {body} particle {i}: {a} after the shift, expected about {want}"
            );
        }
    }
    // Still settled a second later: nothing yanked toward the old frame.
    app.update_n(60);
    let later = particles(&mut app);
    for (body, (b, l)) in before.iter().zip(&later).enumerate() {
        for (i, (b, l)) in b.iter().zip(l).enumerate() {
            let want = shift.point(*b);
            assert!(
                l.is_finite() && (*l - want).length() < 0.1,
                "body {body} particle {i}: {l} a second after the shift, expected about {want}"
            );
        }
    }
}

#[test]
fn the_unveiling_hoist_follows_a_world_shift() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<PendingShift>()
        .add_systems(PostUpdate, write_pending_shift.in_set(WorldShiftSet::Write));
    app.add_systems(Startup, |mut commands: Commands| {
        engine_softbody::demo::statue::spawn_unveiling(
            &mut commands,
            DVec3::ZERO,
            Vec3::new(0.5, 0.0, -0.87),
        );
    });
    let hook = |app: &mut App| {
        let mut q = app.world.query::<(&Name, &Transform)>();
        q.iter(&app.world)
            .find(|(n, _)| n.as_str() == "Unveiling sheet")
            .map(|(_, t)| t.translation)
            .expect("the sheet")
    };
    app.update_n(30);
    let before = hook(&mut app);
    let shift = WorldShift {
        rotation: DQuat::from_rotation_y(0.9),
        translation: DVec3::new(4.0e4, 0.0, -1.5e4),
        ..WorldShift::IDENTITY
    };
    app.world.resource_mut::<PendingShift>().0 = Some(shift);
    app.update_n(2);
    // The hook rides the crane's cord at a few centimeters a frame at most.
    let after = hook(&mut app);
    let want = shift.point(before);
    assert!(
        (after - want).length() < 0.05,
        "the sheet hangs at {after} after the shift, expected about {want}"
    );
}

#[test]
fn the_dinghy_waits_at_its_shifted_mooring() {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<PendingShift>()
        .add_systems(PostUpdate, write_pending_shift.in_set(WorldShiftSet::Write));
    // No water here, so the dinghy waits at its mooring (re-placed there every step).
    let home = DVec3::new(211.0, 0.0, -145.0);
    app.add_systems(Startup, move |mut commands: Commands| {
        engine_softbody::demo::spawn_sailboat(&mut commands, home, 22.0);
    });
    app.update_n(30);
    let mut boats = app
        .world
        .query_filtered::<&Transform, With<engine_softbody::demo::sailboat::Helm>>();
    let shift = WorldShift {
        rotation: DQuat::from_rotation_y(0.9),
        translation: DVec3::new(-3.0e3, 125.0, 4.0e4),
        ..WorldShift::IDENTITY
    };
    app.world.resource_mut::<PendingShift>().0 = Some(shift);
    app.update_n(10);
    let at = boats.single(&app.world).unwrap().translation;
    let want = shift.point(home);
    assert!(
        (at - want).length() < 0.05,
        "the dinghy is at {at} after the shift, its mooring moved to {want}"
    );
    let mut sails = app.world.query::<(&Cloth, &SoftBodySim)>();
    let (_, sail) = sails.single(&app.world).expect("the sail");
    let far = (0..sail.sim.len())
        .map(|i| (sail.world_position(i) - want).length())
        .fold(0.0, f64::max);
    assert!(far < 6.0, "sail particles {far} m from the boat");
}

/// Move every root entity by `s`, as the space module puts what it parked on a planet back
/// where it was on it after a trip (not a `WorldShift`).
fn unpark_all(app: &mut App, s: &WorldShift) {
    let mut roots = app
        .world
        .query_filtered::<&mut Transform, Without<ChildOf>>();
    for mut t in roots.iter_mut(&mut app.world) {
        *t = s.transform(&t);
    }
}

#[test]
fn the_yard_follows_its_anchor_put_back_after_a_trip() {
    let app = || {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.world.resource_mut::<FixedTime>().set_hz(60.0);
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn((
                Transform::IDENTITY,
                engine_softbody::systems::SurfaceAnchor::default(),
            ));
            engine_softbody::demo::statue::spawn_unveiling(
                &mut commands,
                DVec3::ZERO,
                Vec3::new(0.5, 0.0, -0.87),
            );
        });
        app.update_n(30);
        app
    };
    let sheet = |app: &mut App| {
        let mut q = app.world.query::<(&Name, &SoftBodySim)>();
        let (_, b) = q
            .iter(&app.world)
            .find(|(n, _)| n.as_str() == "Unveiling sheet")
            .expect("the sheet");
        (0..b.sim.len())
            .map(|i| b.world_position(i))
            .collect::<Vec<_>>()
    };
    // Back from the Moon: the yard's roots are put back 11 m and a hair of a turn from where
    // the shifts in between took them. Alongside, the same yard that stayed home.
    let s = WorldShift {
        rotation: DQuat::from_rotation_y(0.002),
        translation: DVec3::new(-11.0, 0.3, -1.9),
        ..WorldShift::IDENTITY
    };
    let (mut home, mut back) = (app(), app());
    unpark_all(&mut back, &s);
    home.update_n(30);
    back.update_n(30);
    // The sheet goes on as if the yard had always stood there, not dragged 11 m by its hook.
    let far = sheet(&mut home)
        .iter()
        .zip(sheet(&mut back))
        .map(|(h, b)| (s.point(*h) - b).length())
        .fold(0.0, f64::max);
    assert!(far < 0.05, "the moved yard's sheet is {far} m off");
}

#[test]
fn a_turning_world_shift_is_not_read_as_the_surface_moving() {
    use engine_softbody::systems::{SurfaceAnchor, SurfaceMove};
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<PendingShift>()
        .add_systems(PostUpdate, write_pending_shift.in_set(WorldShiftSet::Write));
    app.world.spawn((
        Transform::from_xyz(125.0, 0.0, -35.0),
        SurfaceAnchor::default(),
    ));
    app.update_n(10);
    // The walk audit's re-home: 45 km north and 0.405° of a turn, which leaves the anchor's
    // f32 orientation a hair off unit length.
    app.world.resource_mut::<PendingShift>().0 = Some(WorldShift {
        rotation: DQuat::from_rotation_x(0.405f64.to_radians()),
        translation: DVec3::new(-10.06, -99.0, -45_011.0),
        ..WorldShift::IDENTITY
    });
    for step in 0..30 {
        app.update();
        assert!(
            app.world.resource::<SurfaceMove>().0.is_none(),
            "step {step}: the anchor read as moved"
        );
    }
}
