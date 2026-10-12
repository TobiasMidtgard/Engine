//! The VTOL spaceplane flown end to end: hops off a pad and lands again, climbs from the
//! ground into orbit around a small planet, and heats its shield on re-entry. Physics comes
//! from the stand-in solver in `support`, through the `engine_world` physics contract only.

mod support;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_vehicles::presets;
use engine_vehicles::{
    AssistMode, Autopilot, AutopilotMode, BodyAtmosphere, HeatShield, Spacecraft,
};
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{Gravity, GravityBody, SurfaceMaterials};
use support::*;

fn phase(app: &App, e: Entity) -> String {
    app.world.get::<Autopilot>(e).unwrap().phase.clone()
}

#[test]
fn hops_off_the_pad_hovers_and_lands() {
    let mut app = vehicle_app();
    calm(&mut app);
    ground(&mut app, DVec3::ZERO, 500.0, "concrete");
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height("spaceplane"), 0.0));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    app.world
        .entity_mut(ship)
        .insert(Autopilot::new(AutopilotMode::Hop {
            pad: DVec3::ZERO,
            height_m: 18.0,
            hover_s: 5.0,
            rest_s: 2.0,
        }));
    run_for(&mut app, 1.0);
    assert_eq!(phase(&app, ship), "on the pad");
    let rest = transform(&app, ship).translation.y;

    let mut highest = 0.0f64;
    let mut hovered = false;
    let mut touchdown = None;
    let mut airborne = false;
    run_until(&mut app, 40.0, |app| {
        let tel = telemetry(app, ship);
        let y = transform(app, ship).translation.y;
        highest = highest.max(y - rest);
        hovered |= phase(app, ship) == "hover";
        if tel.wheels_grounded == 0 && y - rest > 2.0 {
            airborne = true;
        }
        if airborne && tel.wheels_grounded > 0 && touchdown.is_none() {
            touchdown = Some(tel.vertical_speed);
        }
        airborne && phase(app, ship) == "on the pad"
    })
    .expect("never landed back on the pad");
    assert!(hovered, "never hovered");
    assert!((15.0..24.0).contains(&highest), "hopped {highest:.1} m");
    let sink = touchdown.unwrap();
    assert!(sink > -1.5, "hard landing at {sink:.1} m/s");
    let p = transform(&app, ship).translation;
    let drift = (p.x * p.x + p.z * p.z).sqrt();
    assert!(drift < 6.0, "landed {drift:.1} m off the pad's center");
    assert!((transform(&app, ship).rotation * Vec3::Y).y > 0.99, "level");
}

/// A 60 km rock with Earth's surface gravity and a thin, 15 km deep atmosphere. The planet
/// surface is at y = 0 under the origin.
fn small_planet(app: &mut App) {
    let radius = 60_000.0f64;
    let mu = 9.81 * radius * radius;
    let center = DVec3::new(0.0, -radius, 0.0);
    {
        let mut g = app.world.resource_mut::<Gravity>();
        g.uniform = Vec3::ZERO;
        g.bodies = vec![GravityBody { center, mu, radius }];
    }
    app.world
        .resource_mut::<engine_vehicles::VehicleAir>()
        .bodies = vec![BodyAtmosphere {
        mu,
        sea_level_density: 1.0,
        scale_height_m: 2_500.0,
        top_m: 15_000.0,
        temperature_c: 10.0,
    }];
    let rock = app
        .world
        .resource::<SurfaceMaterials>()
        .id_or_default("rock");
    app.world.spawn((
        Transform::from_translation(center),
        RigidBody::fixed(),
        Collider {
            shape: ColliderShape::Sphere {
                radius: radius as f32,
            },
            surface: rock,
            ..Default::default()
        },
    ));
}

#[test]
fn climbs_from_the_ground_into_orbit_around_a_small_planet() {
    let mut app = vehicle_app();
    calm(&mut app);
    small_planet(&mut app);
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height("spaceplane"), 0.0));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    let target = 20_000.0;
    app.world
        .entity_mut(ship)
        .insert(Autopilot::new(AutopilotMode::Ascend {
            target_altitude_m: target,
            turn_start_m: 1_000.0,
            turn_end_m: 12_000.0,
            heading: Vec3::NEG_Z,
        }));
    let mut phases = Vec::<String>::new();
    let took = run_until(&mut app, 240.0, |app| {
        let p = phase(app, ship);
        if phases.last() != Some(&p) {
            phases.push(p.clone());
        }
        p == "orbit"
    })
    .unwrap_or_else(|| panic!("no orbit; phases {phases:?}"));
    for want in [
        "vertical lift",
        "pitch up",
        "gravity turn",
        "coast",
        "circularize",
    ] {
        assert!(
            phases.iter().any(|p| p == want),
            "skipped {want}: {phases:?}"
        );
    }
    let tel = telemetry(&app, ship);
    assert!(
        tel.periapsis_m > target * 0.85 && tel.apoapsis_m < target * 1.15,
        "orbit {:.0} × {:.0} m after {took:.0} s",
        tel.periapsis_m,
        tel.apoapsis_m
    );
    // Engines off, it stays up: a third of an orbit later it is still well above the air.
    let (mut low, mut high) = (f64::MAX, f64::MIN);
    run_until(&mut app, 250.0, |app| {
        let alt = telemetry(app, ship).altitude_m;
        low = low.min(alt);
        high = high.max(alt);
        false
    });
    assert!(
        low > target * 0.85 && high < target * 1.15,
        "altitude {low:.0}..{high:.0} m"
    );
    let fuel = app.world.get::<Spacecraft>(ship).unwrap().propellant_kg;
    assert!(fuel > 10_000.0, "only {fuel:.0} kg propellant left");
}

#[test]
fn reentry_heats_the_shield_without_burning_through() {
    let mut app = vehicle_app();
    calm(&mut app);
    let r = 6.371e6f64;
    {
        let mut g = app.world.resource_mut::<Gravity>();
        g.uniform = Vec3::ZERO;
        g.bodies = vec![GravityBody {
            center: DVec3::new(0.0, -r, 0.0),
            mu: 3.986e14,
            radius: r,
        }];
    }
    // Coming in from orbit: 75 km up at 7.6 km/s, nose 35° up, engines off.
    let t = Transform::from_translation(DVec3::new(0.0, 75_000.0, 0.0))
        .with_rotation(engine_vehicles::demo::facing(Vec3::NEG_Z) * Quat::from_rotation_x(0.6));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    app.world.entity_mut(ship).insert(Velocity {
        linear: Vec3::new(0.0, -60.0, -7_600.0),
        angular: Vec3::ZERO,
    });
    app.world.get_mut::<Spacecraft>(ship).unwrap().assist.mode = AssistMode::Off;
    let mut peak = 0.0f32;
    let mut flux = 0.0f32;
    run_until(&mut app, 180.0, |app| {
        let hs = app.world.get::<HeatShield>(ship).unwrap();
        peak = peak.max(hs.temperature_k);
        flux = flux.max(hs.heat_flux_w_m2);
        false
    });
    let shield = *app.world.get::<HeatShield>(ship).unwrap();
    assert!(flux > 500_000.0, "peak heat flux only {flux:.0} W/m²");
    assert!(peak > 1_200.0, "shield only reached {peak:.0} K");
    assert!(
        peak < shield.max_temperature_k,
        "burned through at {peak:.0} K"
    );
    assert!(
        telemetry(&app, ship).altitude_m < 60_000.0,
        "dived into the air"
    );
}
