//! A boat floats on the water around it, not in the trough its own wake digs under it: the
//! `Water` query under a moving interactor stays at the lake's level while the wake spreads
//! behind it. A boat moved without sailing (respawned, or shifted with the world) throws no
//! spray. A sheet of cloth dropped in (`WaterDisturbance`) sends rings out and throws spray.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::{WaterBody, WaterInteractor, WaterPlugin};
use engine_world::{Water, WaterDisturbance, WaterSplash, WorldPlugin};

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

#[derive(Resource, Default)]
struct Splashes(Vec<WaterSplash>);

fn collect(mut splashes: MessageReader<WaterSplash>, mut out: ResMut<Splashes>) {
    out.0.extend(splashes.read().copied());
}

#[test]
fn a_teleported_boat_throws_no_spray() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.init_resource::<Splashes>();
    app.add_systems(Update, collect);
    let mut lake = WaterBody::lake(600.0);
    lake.waves.wind_waves = false;
    lake.bed = engine_water::WaterBed::Flat { depth_m: 4.0 };
    app.world
        .spawn((Transform::from_translation(DVec3::ZERO), lake));
    // A motorboat-sized hull moored at rest, then moved across the lake in one step (a respawn,
    // or the world shifting before its transform catches up).
    let hull = app
        .world
        .spawn((
            Transform::from_translation(DVec3::new(-200.0, 0.0, 0.0)),
            WaterInteractor::new(0.9).with_wake(2.5),
        ))
        .id();
    for _ in 0..30 {
        app.update();
    }
    app.world.get_mut::<Transform>(hull).unwrap().translation.x = 200.0;
    for _ in 0..30 {
        app.update();
    }
    let thrown = &app.world.resource::<Splashes>().0;
    assert!(
        thrown.is_empty(),
        "a hull that never moved through the water threw {} splashes: {:?}",
        thrown.len(),
        thrown.first()
    );
}

/// A 1.2 m sheet dropped flat onto the lake from half a meter, as the softbody module reports
/// it: four 0.6 m patches of 0.1 kg slapping in at 3 m/s for a few steps, then lying still.
fn drop_sheet(mut step: Local<u32>, mut out: MessageWriter<WaterDisturbance>) {
    *step += 1;
    if !(30..36).contains(&*step) {
        return;
    }
    for (x, z) in [(-0.3, -0.3), (0.3, -0.3), (-0.3, 0.3), (0.3, 0.3)] {
        out.write(WaterDisturbance {
            position: DVec3::new(x, 0.0, z),
            velocity: Vec3::new(0.2, -3.0, 0.0),
            radius: 0.3,
            mass_kg: 0.1,
            source: None,
        });
    }
}

#[test]
fn a_sheet_dropped_in_sends_rings_and_spray() {
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
    app.world.insert_resource(Splashes::default());
    app.add_systems(Update, (drop_sheet, collect));
    let mut ring = 0.0f64;
    for _ in 0..90 {
        app.update();
        // The rings spread out past the sheet's edge.
        let water = app.world.resource::<Water>().0.clone();
        for at in [DVec3::new(2.0, 0.0, 0.0), DVec3::new(0.0, 0.0, -2.0)] {
            ring = ring.max(water.sample(at).unwrap().surface_height.abs());
        }
    }
    assert!(ring > 0.002, "no rings from the sheet ({ring:.4} m)");
    let splashes = &app.world.resource::<Splashes>().0;
    assert!(!splashes.is_empty(), "the sheet threw no spray");
    let thrown: f32 = splashes.iter().map(|s| s.mass_kg).sum();
    assert!(thrown < 2.0, "a light sheet threw {thrown:.2} kg of spray");
}
