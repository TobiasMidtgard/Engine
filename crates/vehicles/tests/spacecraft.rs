//! The VTOL spaceplane flown end to end: hops off a pad and lands again, climbs from the
//! ground into orbit around a small planet, and heats its shield on re-entry. Physics comes
//! from the stand-in solver in `support`, through the `engine_world` physics contract only.

mod support;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_vehicles::presets;
use engine_vehicles::{
    Aircraft, AssistMode, Autopilot, AutopilotMode, BodyAtmosphere, HeatShield, Spacecraft,
};
use engine_world::physics::{Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{Gravity, GravityBody, SmokeEmission, SurfaceMaterials};
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
    let mut hover_thrust = 0.0f32;
    let mut touchdown = None;
    let mut airborne = false;
    let (mut tucked, mut gear_at_touchdown) = (1.0f32, 0.0f32);
    run_until(&mut app, 40.0, |app| {
        let tel = telemetry(app, ship);
        let y = transform(app, ship).translation.y;
        let gear = app.world.get::<Aircraft>(ship).unwrap().gear;
        tucked = tucked.min(gear);
        highest = highest.max(y - rest);
        hovered |= phase(app, ship) == "hover";
        if phase(app, ship) == "hover" {
            let e = app.world.get::<engine_world::EngineState>(ship).unwrap();
            // Holding 80 t up takes about 780 kN.
            hover_thrust = hover_thrust.max(e.thrust_n);
        }
        if tel.wheels_grounded == 0 && y - rest > 2.0 {
            airborne = true;
        }
        if airborne && tel.wheels_grounded > 0 && touchdown.is_none() {
            touchdown = Some(tel.vertical_speed);
            gear_at_touchdown = gear;
        }
        airborne && phase(app, ship) == "on the pad"
    })
    .expect("never landed back on the pad");
    assert!(hovered, "never hovered");
    assert!(
        tucked < 0.3,
        "the gear stayed out in the hover ({tucked:.2})"
    );
    assert_eq!(
        gear_at_touchdown, 1.0,
        "touched down before the gear was down"
    );
    assert!(
        (600_000.0..1_200_000.0).contains(&hover_thrust),
        "hover thrust {hover_thrust:.0} N"
    );
    assert!((15.0..24.0).contains(&highest), "hopped {highest:.1} m");
    let sink = touchdown.unwrap();
    assert!(sink > -1.5, "hard landing at {sink:.1} m/s");
    let p = transform(&app, ship).translation;
    let drift = (p.x * p.x + p.z * p.z).sqrt();
    assert!(drift < 6.0, "landed {drift:.1} m off the pad's center");
    assert!((transform(&app, ship).rotation * Vec3::Y).y > 0.99, "level");
}

#[derive(Resource, Default)]
struct Wash {
    kg: f32,
    darkest: f32,
    hottest: f32,
}

fn record_wash(mut wash: ResMut<Wash>, mut smoke: MessageReader<SmokeEmission>) {
    for s in smoke.read() {
        wash.kg += s.mass_kg;
        wash.darkest = wash.darkest.max(s.darkness);
        wash.hottest = wash.hottest.max(s.temperature_c);
    }
}

#[test]
fn rocket_wash_kicks_up_a_light_dust_cloud() {
    let mut app = vehicle_app();
    calm(&mut app);
    ground(&mut app, DVec3::ZERO, 500.0, "concrete");
    app.init_resource::<Wash>().add_systems(Last, record_wash);
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height("spaceplane"), 0.0));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    app.world
        .entity_mut(ship)
        .insert(Autopilot::new(AutopilotMode::Hop {
            pad: DVec3::ZERO,
            height_m: 18.0,
            hover_s: 30.0,
            rest_s: 2.0,
        }));
    run_until(&mut app, 30.0, |app| phase(app, ship) == "hover").expect("never hovered");
    run_for(&mut app, 2.0);
    app.world.resource_mut::<Wash>().kg = 0.0;
    run_for(&mut app, 10.0);
    let wash = app.world.resource::<Wash>();
    // An 80 t ship hovering at 18 m blows dust and steam off the pad: a few smoke grenades'
    // worth (0.05 kg/s each), pale and only warm, so it rolls along the ground.
    let rate = wash.kg / 10.0;
    eprintln!("{rate:.3} kg/s of wash at 18 m");
    assert!((0.05..0.6).contains(&rate), "{rate:.3} kg/s of wash");
    assert!(wash.darkest < 0.1, "darkness {:.2}", wash.darkest);
    assert!(wash.hottest < 100.0, "{:.0} °C", wash.hottest);
}

/// High above the pad the jets don't reach the ground: no dust (it used to be released in
/// mid-air below the ship, a trail of puffs along its path).
#[test]
fn hovering_high_kicks_up_no_dust() {
    let mut app = vehicle_app();
    calm(&mut app);
    ground(&mut app, DVec3::ZERO, 500.0, "concrete");
    app.init_resource::<Wash>().add_systems(Last, record_wash);
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height("spaceplane"), 0.0));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    app.world
        .entity_mut(ship)
        .insert(Autopilot::new(AutopilotMode::Hop {
            pad: DVec3::ZERO,
            height_m: 110.0,
            hover_s: 30.0,
            rest_s: 2.0,
        }));
    run_until(&mut app, 60.0, |app| phase(app, ship) == "hover").expect("never hovered");
    run_for(&mut app, 2.0);
    let y = transform(&app, ship).translation.y;
    app.world.resource_mut::<Wash>().kg = 0.0;
    run_for(&mut app, 5.0);
    let wash = app.world.resource::<Wash>();
    assert!(y > 100.0, "hovering at {y:.0} m");
    assert_eq!(wash.kg, 0.0, "{:.3} kg of dust from {y:.0} m up", wash.kg);
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
    app.world.resource_mut::<engine_world::Atmospheres>().bodies = vec![BodyAtmosphere {
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

/// On a full-size Earth (as in the showcase) the climb never stalls back into thick air, and
/// the ship coasts to its 200 km apoapsis and circularizes there. Before, the pitch program
/// levelled off at 54 km with orbital speed far off, and the ship sank back to 37 km at
/// Mach 16.
#[test]
fn climbs_to_a_200_km_orbit_around_a_full_size_earth() {
    let mut app = vehicle_app();
    calm(&mut app);
    {
        let mut g = app.world.resource_mut::<Gravity>();
        g.uniform = Vec3::ZERO;
        g.bodies = vec![GravityBody {
            center: DVec3::new(0.0, -6.371e6, 0.0),
            mu: 3.986e14,
            radius: 6.371e6,
        }];
    }
    ground(&mut app, DVec3::ZERO, 400.0, "concrete");
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height("spaceplane"), 0.0));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    let target = 200_000.0;
    app.world
        .entity_mut(ship)
        .insert(Autopilot::new(AutopilotMode::Ascend {
            target_altitude_m: target,
            turn_start_m: 1_000.0,
            turn_end_m: 45_000.0,
            heading: Vec3::NEG_Z,
        }));
    let mut phases = Vec::<String>::new();
    let (mut highest, mut sank) = (0.0f64, 0.0f64);
    let took = run_until(&mut app, 900.0, |app| {
        let p = phase(app, ship);
        if !p.is_empty() && phases.last() != Some(&p) {
            phases.push(p.clone());
        }
        let alt = telemetry(app, ship).altitude_m;
        highest = highest.max(alt);
        sank = sank.max(highest - alt);
        p == "orbit"
    })
    .unwrap_or_else(|| panic!("no orbit; phases {phases:?}"));
    assert_eq!(
        phases,
        [
            "vertical lift",
            "pitch up",
            "liftoff",
            "gravity turn",
            "coast",
            "circularize",
            "orbit"
        ]
    );
    assert!(sank < 500.0, "sank {sank:.0} m on the way up");
    // A moment for the engines to spool down, then the orbit it is left in.
    run_for(&mut app, 5.0);
    let tel = telemetry(&app, ship);
    assert!(
        tel.periapsis_m > target * 0.9 && tel.apoapsis_m < target * 1.1,
        "orbit {:.0} × {:.0} km after {took:.0} s",
        tel.periapsis_m / 1e3,
        tel.apoapsis_m / 1e3
    );
    let fuel = app.world.get::<Spacecraft>(ship).unwrap().propellant_kg;
    assert!(fuel > 20_000.0, "only {fuel:.0} kg propellant left");
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

/// A steady wind pushes on the hovering ship; it still hovers over the pad's center and lands
/// back on it hop after hop instead of walking off the pad.
#[test]
fn hops_in_a_steady_wind_and_lands_on_its_spot() {
    let mut app = vehicle_app();
    let wind = Vec3::new(6.0, 0.0, 0.0);
    {
        let mut weather = app.world.resource_mut::<engine_world::WeatherState>();
        weather.wind = wind;
        weather.gustiness = 0.0;
        let mut w = app.world.resource_mut::<engine_world::WindField>();
        w.prevailing = wind;
        w.gustiness = 0.0;
    }
    ground(&mut app, DVec3::ZERO, 500.0, "concrete");
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height("spaceplane"), 0.0));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    app.world
        .entity_mut(ship)
        .insert(Autopilot::new(AutopilotMode::Hop {
            pad: DVec3::ZERO,
            height_m: 18.0,
            hover_s: 14.0,
            rest_s: 2.0,
        }));
    let (mut landings, mut worst_landing, mut worst_hover) = (0, 0.0f64, 0.0f64);
    let mut was_on_pad = true;
    run_until(&mut app, 120.0, |app| {
        let p = transform(app, ship).translation;
        let off = (p.x * p.x + p.z * p.z).sqrt();
        let on_pad = phase(app, ship) == "on the pad";
        if phase(app, ship) == "hover" && app.world.get::<Autopilot>(ship).unwrap().timer > 8.0 {
            worst_hover = worst_hover.max(off);
        }
        if on_pad && !was_on_pad {
            landings += 1;
            worst_landing = worst_landing.max(off);
        }
        was_on_pad = on_pad;
        landings == 3
    })
    .expect("didn't land three times");
    assert!(
        telemetry(&app, ship).airspeed > 4.0,
        "the wind blew ({:.1} m/s of air)",
        telemetry(&app, ship).airspeed
    );
    assert!(
        worst_hover < 0.5,
        "hovered {worst_hover:.1} m off the pad's center"
    );
    assert!(
        worst_landing < 0.3,
        "landed {worst_landing:.1} m off the pad's center"
    );
}

/// `vehicles.spawn` puts a ship in the air with its gear stowed, and `vehicles.control`
/// switches its flight assist and gear: with the assist off, a ship falling free coasts with
/// every thruster idle, where the full assist fires against the fall; the gear comes down.
#[test]
fn the_control_api_turns_the_flight_assist_off_to_coast() {
    let mut app = vehicle_app();
    calm(&mut app);
    let spawned = engine_core::registry::call_api(
        &mut app.world,
        "vehicles.spawn",
        serde_json::json!({
            "preset": "spaceplane",
            "name": "Coaster",
            "at": [0.0, 500.0, 0.0],
            "on_ground": false,
            "gear": "up",
        }),
    )
    .unwrap();
    let ship = Entity::from_bits(spawned["id"].as_u64().unwrap());
    let gear = |app: &App| app.world.get::<Aircraft>(ship).unwrap().gear;
    assert_eq!(gear(&app), 0.0, "spawned in flight with the gear stowed");
    run_for(&mut app, 1.0);
    let firing = |app: &App| {
        let ship = app.world.get::<Spacecraft>(ship).unwrap();
        ship.thrusters
            .iter()
            .map(|t| t.level)
            .fold(0.0f32, f32::max)
    };
    assert!(firing(&app) > 0.05, "the full assist fights the fall");
    assert_eq!(gear(&app), 0.0, "the gear stays up");

    let set = engine_core::registry::call_api(
        &mut app.world,
        "vehicles.control",
        serde_json::json!({"vehicle": "Coaster", "assist": "Off", "gear": "down"}),
    );
    let set = set.unwrap();
    assert_eq!(
        (&set["assist"], &set["gear"]),
        (&"Off".into(), &"down".into())
    );
    assert_eq!(
        app.world.get::<Spacecraft>(ship).unwrap().assist.mode,
        AssistMode::Off
    );
    // The engines spool down (a quarter second for the hover wells) and stay off.
    run_for(&mut app, 2.0);
    assert!(firing(&app) < 1e-3, "coasting at {}", firing(&app));
    // The gear comes down over six seconds.
    run_for(&mut app, 4.5);
    assert_eq!(gear(&app), 1.0);

    for bad in [
        serde_json::json!({"vehicle": "Coaster", "assist": "Warp"}),
        serde_json::json!({"vehicle": "Coaster", "gear": "half"}),
    ] {
        let r = engine_core::registry::call_api(&mut app.world, "vehicles.control", bad);
        assert!(r.is_err(), "{r:?}");
    }
}

/// Re-entry plasma: at 7 km/s and 70 km the shock layer and wake (each a hot and an outer
/// part) and the wingtip streamers show, trailing behind and above a ship flying nose 40° up; held still at the same height,
/// or in vacuum, neither does.
#[test]
fn reentry_air_glows_and_trails_the_ship() {
    use engine_render::MeshRenderer;
    use engine_vehicles::visuals::PlasmaVisual;
    let plasma = |altitude: f64, speed: f32| {
        let mut app = vehicle_app();
        calm(&mut app);
        let t = Transform::from_translation(DVec3::new(0.0, altitude, 0.0))
            .with_rotation(Quat::from_rotation_x(40f32.to_radians()));
        let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
        app.world.entity_mut(ship).insert(Velocity {
            linear: Vec3::NEG_Z * speed,
            angular: Vec3::ZERO,
        });
        app.world.get_mut::<Spacecraft>(ship).unwrap().assist.mode = AssistMode::Off;
        run_for(&mut app, 0.2);
        let flux = app.world.get::<HeatShield>(ship).unwrap().heat_flux_w_m2;
        let mut q = app
            .world
            .query::<(&ChildOf, &PlasmaVisual, &MeshRenderer, &Transform)>();
        // (is a wake, is the main wake, shown, placed)
        let parts: Vec<(bool, bool, bool, Transform)> = q
            .iter(&app.world)
            .filter(|(p, ..)| p.parent() == ship)
            .map(|(_, v, r, t)| {
                let main = v.wake.is_some_and(|w| w.from.is_none());
                (v.wake.is_some(), main, r.visible, *t)
            })
            .collect();
        eprintln!("{altitude} m at {speed} m/s: nose flux {flux:.0} W/m²");
        assert_eq!(
            parts.len(),
            6,
            "a shock layer and a wake (hot and outer parts) and two wingtip streamers"
        );
        parts
    };
    for (wake, main, visible, t) in plasma(70_000.0, 7_000.0) {
        assert!(visible);
        if wake {
            let along = t.rotation * Vec3::Z;
            let a = 40f32.to_radians();
            eprintln!(
                "wake {:.0} m along {along:.2} from {:.1}",
                t.scale.z, t.translation
            );
            assert!(along.distance(Vec3::new(0.0, a.sin(), a.cos())) < 0.05);
            assert!(t.scale.z > if main { 100.0 } else { 40.0 });
        }
    }
    for (altitude, speed) in [(70_000.0, 0.0), (200_000.0, 7_800.0), (2_000.0, 250.0)] {
        assert!(
            plasma(altitude, speed)
                .iter()
                .all(|(.., visible, _)| !visible)
        );
    }
}

/// `vehicles.place` sets a ship nose up and banked, with its shield as hot as re-entry
/// leaves it (the re-entry tour shot holds the ship like this).
#[test]
fn the_place_api_tilts_a_ship_and_heats_its_shield() {
    let mut app = vehicle_app();
    let spawned = engine_core::registry::call_api(
        &mut app.world,
        "vehicles.spawn",
        serde_json::json!({"preset": "spaceplane", "name": "Hot", "at": [0.0, 900.0, 0.0],
            "on_ground": false}),
    )
    .unwrap();
    let ship = Entity::from_bits(spawned["id"].as_u64().unwrap());
    engine_core::registry::call_api(
        &mut app.world,
        "vehicles.place",
        serde_json::json!({"vehicle": "Hot", "at": [0.0, 900.0, 0.0], "on_ground": false,
            "yaw_deg": 90.0, "pitch_deg": 40.0, "roll_deg": 30.0,
            "hull_temperature_k": 1_390.0}),
    )
    .unwrap();
    let r = transform(&app, ship).rotation;
    let (nose, right) = (r * Vec3::NEG_Z, r * Vec3::X);
    assert!((nose.y - 40f32.to_radians().sin()).abs() < 1e-4, "{nose}");
    assert!(nose.x > 0.7, "heading +X: {nose}");
    // Rolled about the nose: the right wing drops 30° of the way round from level.
    let down = 30f32.to_radians().sin() * 40f32.to_radians().cos();
    assert!((right.y + down).abs() < 1e-4, "right wing down: {right}");
    let shield = app.world.get::<HeatShield>(ship).unwrap();
    assert_eq!(shield.temperature_k, 1_390.0);
}

const EARTH_R: f64 = 6.371e6;
const EARTH_MU: f64 = 3.986e14;

/// How a flight home went.
struct Homecoming {
    phases: Vec<String>,
    peak_k: f32,
    peak_g: f32,
    /// Where the ship ended, and the seconds it took to land (None: it never did).
    at: DVec3,
    took: Option<f64>,
}

/// The spaceplane flying home (the Return autopilot) to a pad at the origin of a full-size
/// Earth: `lead` round the planet before the pad, `altitude_m` up, at `speed` along its
/// path (horizontal, toward the pad, −Z over it) with `propellant_kg` aboard, for at most
/// `max_s` seconds.
fn fly_home(
    lead_deg: f64,
    altitude_m: f64,
    speed: f64,
    propellant_kg: f32,
    max_s: f64,
) -> Homecoming {
    let mut app = vehicle_app();
    calm(&mut app);
    let center = DVec3::new(0.0, -EARTH_R, 0.0);
    {
        let mut g = app.world.resource_mut::<Gravity>();
        g.uniform = Vec3::ZERO;
        g.bodies = vec![GravityBody {
            center,
            mu: EARTH_MU,
            radius: EARTH_R,
        }];
    }
    let pad = DVec3::ZERO;
    ground(&mut app, pad, 200.0, "concrete");
    let lead = lead_deg.to_radians();
    let at = center + DVec3::new(0.0, lead.cos(), lead.sin()) * (EARTH_R + altitude_m);
    let along = Vec3::new(0.0, lead.sin() as f32, -lead.cos() as f32);
    let up = (at - center).normalize().as_vec3();
    let t = Transform::from_translation(at).with_rotation(Quat::from_mat3(
        &engine_core::math::Mat3::from_cols(-along.cross(up), up, -along),
    ));
    let ship = spawn_with(&mut app, |c| presets::spaceplane(c, t));
    app.world.entity_mut(ship).insert(Velocity {
        linear: along * speed as f32,
        angular: Vec3::ZERO,
    });
    app.world.get_mut::<Spacecraft>(ship).unwrap().propellant_kg = propellant_kg;
    {
        let mut a = app.world.get_mut::<Aircraft>(ship).unwrap();
        a.gear_down = false;
        a.gear = 0.0;
    }
    app.world
        .entity_mut(ship)
        .insert(Autopilot::new(AutopilotMode::Return { pad }));
    let mut phases = Vec::<String>::new();
    let (mut peak_k, mut peak_g) = (0.0f32, 0.0f32);
    let mut last_print = -100.0;
    let mut clock = 0.0;
    let took = run_until(&mut app, max_s, |app| {
        clock += 1.0 / 60.0;
        let p = phase(app, ship);
        if !p.is_empty() && phases.last() != Some(&p) {
            eprintln!("{clock:7.1} s: {p}");
            phases.push(p.clone());
        }
        let tel = telemetry(app, ship);
        peak_k = peak_k.max(tel.hull_temperature_k);
        if tel.altitude_m < 150_000.0 {
            peak_g = peak_g.max(tel.g_load);
        }
        let every = if p.starts_with("landing") { 2.0 } else { 20.0 };
        if clock - last_print > every {
            last_print = clock;
            let ap = app.world.get::<Autopilot>(ship).unwrap();
            let x = transform(app, ship);
            eprintln!(
                "{clock:7.1} s {p:12} alt {:7.0} m speed {:6.0} aoa {:5.1} bank {:6.1} over {:6.0} km hull {:5.0} K g {:4.1} to pad {:8.2} km fuel {:6.0} kg upright {:5.2}",
                tel.altitude_m,
                tel.speed,
                tel.aoa_deg,
                ap.entry.bank.to_degrees(),
                ap.entry.overshoot_m / 1e3,
                tel.hull_temperature_k,
                tel.g_load,
                (x.translation - pad).length() / 1e3,
                app.world.get::<Spacecraft>(ship).unwrap().propellant_kg,
                (x.rotation * Vec3::Y)
                    .as_dvec3()
                    .dot((x.translation - center).normalize()),
            );
        }
        p == "landed"
    });
    let at = transform(&app, ship).translation;
    eprintln!("took {took:?}, phases {phases:?}, peak {peak_k:.0} K {peak_g:.1} g, at {at:?}");
    let shield = *app.world.get::<HeatShield>(ship).unwrap();
    assert!(peak_k < shield.max_temperature_k, "{peak_k:.0} K");
    Homecoming {
        phases,
        peak_k,
        peak_g,
        at,
        took,
    }
}

fn assert_on_the_pad(home: &Homecoming) {
    let took = home
        .took
        .unwrap_or_else(|| panic!("never landed; phases {:?}", home.phases));
    let off = home.at - DVec3::Y * home.at.y;
    assert!(
        off.length() < 10.0,
        "landed {:.1} m off the pad after {took:.0} s",
        off.length()
    );
}

/// Home from a 200 km orbit around a full-size Earth: the spaceplane waits for the moment to
/// burn out of orbit, re-enters nose high with its heat shield glowing but intact, banks to
/// come down at the pad, glides to it and lands on its hover engines.
#[test]
fn returns_from_orbit_and_lands_on_the_pad() {
    let orbit = EARTH_R + 200_000.0;
    let home = fly_home(
        190.0,
        200_000.0,
        (EARTH_MU / orbit).sqrt(),
        25_000.0,
        5_000.0,
    );
    assert_eq!(
        home.phases,
        [
            "deorbit wait",
            "deorbit burn",
            "entry coast",
            "entry",
            "approach",
            "landing burn",
            "landing",
            "landed"
        ],
    );
    assert!(home.peak_k > 1_000.0, "{:.0} K", home.peak_k);
    assert!(home.peak_g < 4.0, "{:.1} g", home.peak_g);
    assert_on_the_pad(&home);
}

/// The last of the way home: gliding in high and fast 40 km out, the spaceplane comes down
/// to the pad, brakes on its hover engines and sets down on it.
#[test]
fn glides_in_and_lands_on_the_pad() {
    let home = fly_home(0.36, 12_000.0, 400.0, 6_000.0, 600.0);
    assert_eq!(
        home.phases,
        ["approach", "landing burn", "landing", "landed"],
    );
    assert_on_the_pad(&home);
}
