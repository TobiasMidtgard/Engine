//! The sport plane flown end to end: takeoff from a short runway, a circuit holding height
//! and speed through banked turns, a stall and recovery, and a hands-off glide. Physics comes
//! from the stand-in solver in `support`, through the `engine_world` physics contract only.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_vehicles::presets;
use engine_vehicles::{Autopilot, AutopilotMode};
use engine_world::physics::Velocity;
use support::*;

/// The sport plane in flight at `altitude` m, heading −Z at `speed` m/s.
fn plane_in_air(app: &mut App, altitude: f64, speed: f32) -> Entity {
    let t = Transform::from_translation(DVec3::new(0.0, altitude, 0.0))
        .with_rotation(engine_vehicles::demo::facing(Vec3::NEG_Z));
    let plane = spawn_with(app, |c| presets::sport_plane(c, t));
    app.world.entity_mut(plane).insert(Velocity {
        linear: Vec3::NEG_Z * speed,
        angular: Vec3::ZERO,
    });
    plane
}

fn bank_deg(app: &App, plane: Entity) -> f32 {
    let right = transform(app, plane).rotation * Vec3::X;
    (-right.y).asin().to_degrees()
}

/// A 1.5 km square circuit at 150 m.
fn circuit() -> Vec<DVec3> {
    vec![
        DVec3::new(0.0, 150.0, -1500.0),
        DVec3::new(1500.0, 150.0, -1500.0),
        DVec3::new(1500.0, 150.0, 0.0),
        DVec3::new(0.0, 150.0, 0.0),
    ]
}

#[test]
fn takes_off_from_a_short_runway_and_climbs() {
    let mut app = vehicle_app();
    calm(&mut app);
    ground(&mut app, DVec3::new(0.0, 0.0, -1500.0), 3_000.0, "concrete");
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height("sport_plane"), 0.0))
        .with_rotation(engine_vehicles::demo::facing(Vec3::NEG_Z));
    let plane = spawn_with(&mut app, |c| presets::sport_plane(c, t));
    app.world
        .entity_mut(plane)
        .insert(Autopilot::new(AutopilotMode::Fly {
            waypoints: circuit(),
            looped: true,
            cruise_speed: 60.0,
            rotate_speed: 32.0,
        }));
    run_for(&mut app, 1.0);
    assert_eq!(
        telemetry(&app, plane).wheels_grounded,
        3,
        "sits on its gear"
    );

    let mut liftoff = None;
    let mut stalled = false;
    run_until(&mut app, 40.0, |app| {
        let tel = telemetry(app, plane);
        let p = transform(app, plane).translation;
        if liftoff.is_none() && tel.wheels_grounded == 0 && p.y > 3.0 {
            liftoff = Some(-p.z);
        }
        stalled |= tel.stalled;
        false
    });
    let liftoff = liftoff.expect("never left the ground");
    // The showcase runway is 240 m.
    assert!(liftoff < 230.0, "took {liftoff:.0} m of runway");
    let p = transform(&app, plane).translation;
    assert!(p.y > 120.0, "only {:.0} m up after 40 s", p.y);
    assert!(!stalled, "stalled on the climb-out");
    assert!(bank_deg(&app, plane).abs() < 60.0);
    let engine = *app.world.get::<engine_world::EngineState>(plane).unwrap();
    assert!(
        matches!(engine.kind, engine_world::EngineKind::Piston { .. })
            && engine.rpm > engine.idle_rpm + 500.0
            && engine.thrust_n > 500.0,
        "climbing engine {engine:?}"
    );
}

#[test]
fn flies_a_circuit_holding_height_and_speed() {
    let mut app = vehicle_app();
    calm(&mut app);
    let plane = plane_in_air(&mut app, 150.0, 60.0);
    app.world
        .entity_mut(plane)
        .insert(Autopilot::new(AutopilotMode::Fly {
            waypoints: circuit(),
            looped: true,
            cruise_speed: 60.0,
            rotate_speed: 32.0,
        }));
    run_for(&mut app, 30.0);
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    let (mut slow, mut fast) = (f32::MAX, f32::MIN);
    let mut max_bank = 0.0f32;
    let mut stalled = false;
    let mut waypoints = 0;
    let mut last = app.world.get::<Autopilot>(plane).unwrap().target;
    run_until(&mut app, 90.0, |app| {
        let tel = telemetry(app, plane);
        lo = lo.min(tel.altitude_m);
        hi = hi.max(tel.altitude_m);
        slow = slow.min(tel.airspeed);
        fast = fast.max(tel.airspeed);
        max_bank = max_bank.max(bank_deg(app, plane).abs());
        stalled |= tel.stalled;
        let target = app.world.get::<Autopilot>(plane).unwrap().target;
        if target != last {
            waypoints += 1;
            last = target;
        }
        false
    });
    assert!(lo > 135.0 && hi < 165.0, "altitude {lo:.0}..{hi:.0} m");
    assert!(
        slow > 52.0 && fast < 68.0,
        "airspeed {slow:.0}..{fast:.0} m/s"
    );
    assert!(max_bank > 25.0, "turns banked only {max_bank:.0}°");
    assert!(waypoints >= 3, "reached {waypoints} waypoints in 90 s");
    assert!(!stalled);
}

#[test]
fn stall_demo_stalls_and_recovers() {
    let mut app = vehicle_app();
    calm(&mut app);
    let plane = plane_in_air(&mut app, 600.0, 55.0);
    app.world
        .entity_mut(plane)
        .insert(Autopilot::new(AutopilotMode::StallDemo {
            altitude_m: 600.0,
            heading: Vec3::NEG_Z,
        }));
    let stall = run_until(&mut app, 30.0, |app| telemetry(app, plane).stalled)
        .expect("the wing never stalled");
    // The nose drops and the plane flies again.
    let recovered = run_until(&mut app, 12.0, |app| {
        let tel = telemetry(app, plane);
        !tel.stalled && tel.airspeed > 38.0
    })
    .expect("never recovered");
    let mut lowest = f64::MAX;
    let mut climbed = false;
    run_until(&mut app, 20.0, |app| {
        let tel = telemetry(app, plane);
        lowest = lowest.min(tel.altitude_m);
        climbed |= tel.vertical_speed > 2.0;
        false
    });
    assert!(
        lowest > 450.0,
        "lost too much height: down to {lowest:.0} m"
    );
    assert!(
        climbed,
        "never climbed back (stalled at {stall:.1} s, flying {recovered:.1} s later)"
    );
}

#[test]
fn glides_hands_off_with_the_engine_off() {
    let mut app = vehicle_app();
    calm(&mut app);
    let plane = plane_in_air(&mut app, 800.0, 55.0);
    controls(&mut app, plane, |c| c.throttle = 0.0);
    // Let it settle into its glide (a gentle phugoid).
    run_for(&mut app, 20.0);
    let start = transform(&app, plane).translation;
    let mut stalled = false;
    run_until(&mut app, 40.0, |app| {
        stalled |= telemetry(app, plane).stalled;
        false
    });
    let end = transform(&app, plane).translation;
    let dropped = start.y - end.y;
    let flown = DVec3::new(end.x - start.x, 0.0, end.z - start.z).length();
    let ratio = flown / dropped.max(1.0);
    assert!((7.0..16.0).contains(&ratio), "glide ratio {ratio:.1}");
    assert!(!stalled);
    assert!(bank_deg(&app, plane).abs() < 10.0, "wings stay level");
    assert!(
        (end.x).abs() < 50.0,
        "flies straight: drifted {:.0} m",
        end.x
    );
}
