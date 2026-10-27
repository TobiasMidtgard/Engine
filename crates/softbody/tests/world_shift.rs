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
