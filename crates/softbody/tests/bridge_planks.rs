//! A rope bridge's planks are root entities posed in the world, so a character standing on
//! one reads the deck's real motion from the plank's `Transform` (movement's moving-platform
//! code does: `crates/movement/src/platform.rs`). As children of the bridge, a plank tilting a
//! few milliradians under a walker read as metres per step on a bridge far from the origin.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{BridgePlank, RopeBridge, SoftBodyPlugin, SoftBodySim, SoftLoad};
use engine_world::WeatherState;

const DT: f32 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: DT as f64,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    app
}

/// The deck's velocity at `pos` from a plank's pose last step and now, as movement's
/// `platform::ride` computes it.
fn deck_velocity(before: &Transform, now: &Transform, pos: DVec3) -> Vec3 {
    let local = now.rotation.inverse() * (pos - now.translation).as_vec3();
    let was = before.translation + (before.rotation * local).as_dvec3();
    ((pos - was) / DT as f64).as_vec3()
}

#[test]
fn a_walker_on_a_far_bridge_reads_the_decks_real_speed() {
    let mut app = app();
    // Far from the origin and turned, as in the showcase valley.
    let at = DVec3::new(640.0, 85.0, -410.0);
    let bridge = app
        .world
        .spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(0.7)),
            RopeBridge::default(),
        ))
        .id();
    app.update_n(30);
    assert!(
        app.world.get::<SoftBodySim>(bridge).is_some(),
        "bridge built"
    );
    let mut planks = app.world.query::<(Entity, &BridgePlank, &Transform)>();
    let list: Vec<(Entity, u32, Transform)> = planks
        .iter(&app.world)
        .map(|(e, p, t)| (e, p.index, *t))
        .collect();
    assert!(!list.is_empty(), "no planks");
    for (e, _, t) in &list {
        assert!(app.world.get::<ChildOf>(*e).is_none(), "planks are roots");
        assert!(
            (t.translation - at).length() < 20.0,
            "a plank isn't on the bridge: {}",
            t.translation
        );
    }
    // Someone steps onto the middle plank: the deck sags and bounces.
    let (middle, _, t) = *list
        .iter()
        .min_by_key(|(_, k, _)| k.abs_diff(list.len() as u32 / 2))
        .unwrap();
    let feet = t.translation + DVec3::Y * 0.05;
    app.world.spawn((
        Transform::from_translation(feet),
        SoftLoad { mass_kg: 90.0 },
    ));
    // Let the drop of the load settle, then read the deck under the feet.
    app.update_n(30);
    let mut before = *app.world.get::<Transform>(middle).unwrap();
    let mut fastest = 0.0_f32;
    for _ in 0..60 {
        app.update();
        let now = *app.world.get::<Transform>(middle).unwrap();
        // The walker's feet, in the world.
        let feet = app
            .world
            .get::<GlobalTransform>(middle)
            .unwrap()
            .translation();
        let v = deck_velocity(&before, &now, feet + DVec3::Y * 0.1);
        fastest = fastest.max(v.length());
        before = now;
    }
    // A loaded deck sways at well under walking speed.
    assert!(
        fastest < 1.0,
        "the deck under the walker read {fastest} m/s"
    );

    // Planks go with their bridge.
    app.world.despawn(bridge);
    app.update_n(2);
    let left = planks.iter(&app.world).count();
    assert_eq!(left, 0, "planks outlived their bridge");
}
