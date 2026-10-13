//! A boat floats on the water around it, not in the trough its own wake digs under it: the
//! `Water` query under a moving interactor stays at the lake's level while the wake spreads
//! behind it.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::{WaterBody, WaterInteractor, WaterPlugin};
use engine_world::{Water, WorldPlugin};

const SPEED: f64 = 1.8;

#[derive(Component)]
struct Hull;

fn sail(time: Res<Time>, mut hulls: Query<&mut Transform, With<Hull>>) {
    for mut tf in &mut hulls {
        tf.translation.x += SPEED * time.delta_secs() as f64;
    }
}

#[test]
fn a_moving_hull_floats_at_the_lakes_level() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    let mut lake = WaterBody::lake(60.0);
    lake.waves.wind_waves = false;
    lake.bed = engine_water::WaterBed::Flat { depth_m: 4.0 };
    app.world
        .spawn((Transform::from_translation(DVec3::ZERO), lake));
    let hull = app
        .world
        .spawn((
            Hull,
            Transform::from_translation(DVec3::new(-20.0, 0.0, 0.0)),
            WaterInteractor::new(0.8).with_wake(1.0),
        ))
        .id();
    app.add_systems(Update, sail);
    let mut lowest = 0.0f64;
    let mut wake = 0.0f64;
    for frame in 0..300 {
        app.update();
        if frame < 60 {
            continue;
        }
        let p = app
            .world
            .get::<GlobalTransform>(hull)
            .unwrap()
            .translation();
        let water = app.world.resource::<Water>().0.clone();
        let under = water.sample(p).expect("on the lake").surface_height;
        lowest = lowest.min(under);
        // Two meters behind it, the wake is still there.
        for back in [2.0, 3.0, 4.0] {
            let s = water.sample(p - DVec3::X * back).unwrap().surface_height;
            wake = wake.max(s.abs());
        }
    }
    assert!(
        lowest > -0.04,
        "the hull sits {lowest:.3} m in its own trough"
    );
    assert!(wake > 0.01, "no wake behind the hull ({wake:.4} m)");
}
