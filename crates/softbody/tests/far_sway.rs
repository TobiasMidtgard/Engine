//! A far flag in the wind plays back its own recorded flapping instead of being stepped,
//! and is simulated again near, or when the wind turns.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, SoftBodyPlugin, SoftBodySim, SoftMaterial};
use engine_world::WeatherState;

#[derive(Resource)]
struct Camera(DVec3);

fn app(wind: Vec3) -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = wind;
    // The weather's default gusts: the loop plays faster and slower through them.
    weather.gustiness = 0.3;
    app.insert_resource(Camera(DVec3::new(0.0, 1.7, 90.0)));
    app.add_systems(
        PreUpdate,
        |cam: Res<Camera>, mut focus: ResMut<engine_world::FocusPoints>| {
            focus.points.clear();
            focus.points.push(cam.0);
        },
    );
    app
}

fn flag(app: &mut App) -> Entity {
    app.world
        .spawn((
            Transform::from_xyz(0.0, 6.0, 0.0),
            Cloth::flag([3.0, 2.0], SoftMaterial::default()),
        ))
        .id()
}

/// The tip (the particle farthest downwind) over `n` steps: its path's extent, and where
/// the flag reaches downwind on average.
fn watch(app: &mut App, e: Entity, n: usize) -> (f64, f64, bool) {
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    let mut reach = 0.0;
    let mut swaying = true;
    for _ in 0..n {
        app.update();
        let b = app.world.get::<SoftBodySim>(e).unwrap();
        let tip = (0..b.particle_count())
            .map(|i| b.world_position(i))
            .max_by(|a, b| a.x.total_cmp(&b.x))
            .unwrap();
        (lo, hi) = (lo.min(tip), hi.max(tip));
        reach += tip.x / n as f64;
        swaying &= b.swaying();
    }
    ((hi - lo).length(), reach, swaying)
}

#[test]
fn a_far_flag_sways_like_it_flapped_and_is_simulated_near_or_when_the_wind_turns() {
    let mut app = app(Vec3::new(7.0, 0.0, 0.0));
    let e = flag(&mut app);
    // Near: simulated, flapping downwind.
    app.insert_resource(Camera(DVec3::new(0.0, 6.0, 8.0)));
    app.update_n(240);
    let (flap, reach_near, swaying) = watch(&mut app, e, 120);
    assert!(!swaying);
    assert!(flap > 0.05, "a near flag should flap ({flap} m)");

    // 90 m off: it learns its sway, then sways without being stepped.
    app.insert_resource(Camera(DVec3::new(0.0, 6.0, 90.0)));
    app.update_n(240);
    let (sway, reach_far, swaying) = watch(&mut app, e, 120);
    assert!(swaying, "a far flag in the wind should sway");
    assert!(sway > 0.6 * flap, "it sways {sway} m, it flapped {flap} m");
    assert!(sway < 1.6 * flap, "it sways {sway} m, it flapped {flap} m");
    assert!(
        (reach_far - reach_near).abs() < 0.2,
        "it streams out {reach_far} m, it did {reach_near} m"
    );
    let b = app.world.get::<SoftBodySim>(e).unwrap();
    assert!(b.step_ms < 0.5, "swaying took {} ms", b.step_ms);

    // The wind turns: it is simulated again, and comes round.
    app.world.resource_mut::<WeatherState>().wind = Vec3::new(0.0, 0.0, 7.0);
    app.update_n(90);
    assert!(!app.world.get::<SoftBodySim>(e).unwrap().swaying());

    // Near again: simulated.
    app.update_n(400);
    app.insert_resource(Camera(DVec3::new(0.0, 6.0, 8.0)));
    app.update_n(2);
    assert!(!app.world.get::<SoftBodySim>(e).unwrap().swaying());
}

#[test]
fn a_far_flag_in_still_air_does_not_sway() {
    let mut app = app(Vec3::ZERO);
    let e = flag(&mut app);
    app.update_n(400);
    assert!(!app.world.get::<SoftBodySim>(e).unwrap().swaying());
}
