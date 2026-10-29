//! Cruise (`engine_vehicles::cruise`): from over a planet out to a star 4.4 light years away
//! in a couple of minutes, slowing down by itself on the way in and dropping back into
//! physics near the surface. Physics comes from the stand-in solver in `support`.

mod support;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_vehicles::cruise::{self, Cruise};
use engine_vehicles::presets;
use engine_world::physics::{Collider, RigidBody, Velocity};
use engine_world::{Gravity, GravityBody, VehicleControls};
use support::*;

const EARTH_R: f64 = 6_371_000.0;
/// Alpha Centauri A: 4.37 light years straight up from the valley.
const STAR_D: f64 = 4.134e16;
const STAR_R: f64 = 8.5e8;

fn space_app() -> App {
    let mut app = vehicle_app();
    calm(&mut app);
    let mut g = app.world.resource_mut::<Gravity>();
    g.uniform = Vec3::ZERO;
    g.bodies = vec![
        GravityBody {
            center: DVec3::new(0.0, -EARTH_R, 0.0),
            mu: 3.986e14,
            radius: EARTH_R,
        },
        GravityBody {
            center: DVec3::new(0.0, STAR_D, 0.0),
            mu: 1.5e20,
            radius: STAR_R,
        },
    ];
    app
}

fn over_star(p: DVec3) -> f64 {
    (p - DVec3::new(0.0, STAR_D, 0.0)).length() - STAR_R
}

/// A spaceplane `altitude` m up, nose straight up, climbing at `speed`.
fn ship_at(app: &mut App, altitude: f64, speed: f32) -> Entity {
    let t = Transform::from_translation(DVec3::new(0.0, altitude, 0.0))
        .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
    let ship = spawn_with(app, |c| presets::spaceplane(c, t));
    app.world.entity_mut(ship).insert(Velocity {
        linear: Vec3::Y * speed,
        angular: Vec3::ZERO,
    });
    app.update();
    ship
}

#[test]
fn refuses_to_cruise_in_the_air() {
    let mut app = space_app();
    let ship = ship_at(&mut app, 3_000.0, 200.0);
    let err = cruise::engage(&mut app.world, ship).expect_err("cruised 3 km up");
    assert!(err.contains("too low"), "{err}");
    assert!(app.world.get::<RigidBody>(ship).is_some());
}

#[test]
fn flies_out_to_a_star_and_drops_out_over_it() {
    let mut app = space_app();
    let ship = ship_at(&mut app, 20_000.0, 900.0);
    cruise::engage(&mut app.world, ship).expect("engage");
    assert!(
        app.world.get::<RigidBody>(ship).is_none(),
        "still in physics"
    );
    assert!(app.world.get::<Collider>(ship).is_none());
    app.world
        .get_mut::<VehicleControls>(ship)
        .expect("controls")
        .throttle = 1.0;

    // Out: speed grows with the distance.
    let mut halfway_s = None;
    let mut fastest = 0.0f64;
    let mut lowest_over_star = f64::MAX;
    let mut t = 0.0;
    while t < 240.0 && app.world.get::<Cruise>(ship).is_some() {
        run_for(&mut app, 0.5);
        t += 0.5;
        let p = transform(&app, ship).translation;
        if halfway_s.is_none() && p.y > STAR_D * 0.5 {
            halfway_s = Some(t);
        }
        if let Some(c) = app.world.get::<Cruise>(ship) {
            fastest = fastest.max(c.speed);
        }
        lowest_over_star = lowest_over_star.min(over_star(p));
    }
    eprintln!("halfway after {halfway_s:?} s, there after {t} s, top speed {fastest:.2e} m/s");
    let halfway_s = halfway_s.expect("never got halfway");
    assert!(halfway_s < 90.0, "halfway after {halfway_s} s");
    assert!(fastest > 1.0e15, "top speed {:.2e} m/s", fastest);
    // In: dropped out by itself just over the star's surface, never through it.
    assert!(
        app.world.get::<Cruise>(ship).is_none(),
        "still cruising after {t} s"
    );
    let p = transform(&app, ship).translation;
    let h = over_star(p);
    assert!(
        h > 20_000.0 && h < 41_000.0,
        "dropped out {h:.0} m over the star"
    );
    assert!(lowest_over_star > 0.0, "went through the star");
    assert!(t < 200.0, "took {t} s");
    // Back in physics at a speed it can take.
    assert!(app.world.get::<RigidBody>(ship).is_some());
    assert!(app.world.get::<Collider>(ship).is_some());
    let v = velocity(&app, ship).linear.length();
    assert!(v <= 1_500.5 && v > 100.0, "{v:.0} m/s");
}

#[test]
fn steers_by_its_controls_and_drops_out_on_request() {
    let mut app = space_app();
    let ship = ship_at(&mut app, 50_000.0, 500.0);
    cruise::engage(&mut app.world, ship).expect("engage");
    // Pitch over (the nose was straight up).
    app.world
        .get_mut::<VehicleControls>(ship)
        .expect("controls")
        .pitch = 1.0;
    run_for(&mut app, 1.0);
    let fwd = transform(&app, ship).forward();
    assert!(fwd.y < 0.9 && fwd.y > 0.0, "nose {fwd:?}");
    let speed = app.world.get::<Cruise>(ship).expect("cruising").speed;
    assert!(speed > 1_000.0, "{speed:.0} m/s");
    let gauge = telemetry(&app, ship).speed as f64;
    assert!(
        (gauge - speed).abs() < speed * 1e-3,
        "telemetry {gauge:.0}, cruise {speed:.0}"
    );
    cruise::drop_out(&mut app.world, ship);
    assert!(app.world.get::<Cruise>(ship).is_none());
    run_for(&mut app, 0.5);
    let v = velocity(&app, ship).linear;
    assert!(
        v.length() <= 1_501.0 && v.normalize().dot(fwd) > 0.9,
        "{v:?}"
    );
}

/// Home again: from the Moon's distance, nose on the Earth, it slows by itself and drops out
/// skimming 30-odd km over the ground.
#[test]
fn comes_home_and_drops_out_over_the_ground() {
    let mut app = space_app();
    let t = Transform::from_translation(DVec3::new(0.0, 3.8e8, 0.0))
        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    app.update();
    cruise::engage(&mut app.world, ship).expect("engage");
    app.world
        .get_mut::<VehicleControls>(ship)
        .expect("controls")
        .throttle = 1.0;
    let ground = |app: &App| {
        (transform(app, ship).translation - DVec3::new(0.0, -EARTH_R, 0.0)).length() - EARTH_R
    };
    let mut lowest = f64::MAX;
    let mut t = 0.0;
    while t < 120.0 && app.world.get::<Cruise>(ship).is_some() {
        run_for(&mut app, 0.25);
        t += 0.25;
        lowest = lowest.min(ground(&app));
    }
    assert!(
        app.world.get::<Cruise>(ship).is_none(),
        "still cruising after {t} s"
    );
    assert!(t < 60.0, "took {t} s from the Moon");
    let h = ground(&app);
    assert!(h > 25_000.0 && h < 45_000.0, "dropped out {h:.0} m up");
    assert!(lowest > 25_000.0, "dipped to {lowest:.0} m");
    let v = velocity(&app, ship).linear.length();
    assert!(v <= 1_500.5, "{v:.0} m/s");
}

/// The target is named on the gauges when a space module answers `space.bodies`.
#[test]
fn names_the_body_it_homes_on() {
    let mut app = space_app();
    app.register_api("space.bodies", "test stand-in", "{}", |_, _| {
        Ok(serde_json::json!({"bodies": [
            {"name": "Earth", "radius_m": EARTH_R, "distance_m": 1.0e7},
            {"name": "Alpha Centauri A", "radius_m": STAR_R, "distance_m": 0.0},
        ]}))
    });
    let ship = ship_at(&mut app, 20_000.0, 900.0);
    cruise::engage(&mut app.world, ship).expect("engage");
    run_for(&mut app, 0.5);
    let c = app.world.get::<Cruise>(ship).expect("cruising");
    assert_eq!(c.target_name.as_deref(), Some("Alpha Centauri A"));
    assert!(
        (c.target_distance_m - STAR_D).abs() < STAR_D * 0.01,
        "{:.3e} m to go",
        c.target_distance_m
    );
}
