//! Cars driven end to end: acceleration, top speed, braking (and ABS), cornering, handbrake
//! turns, rally drifts on dirt, jumps, parking on a slope, surfaces and the messages cars send
//! to the world. Physics comes from the stand-in solver in `support`, through the
//! `engine_world` physics contract only.

mod support;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_vehicles::presets;
use engine_vehicles::{Autopilot, AutopilotMode};
use engine_world::physics::Velocity;
use engine_world::{GroundContact, NoiseEvent};
use support::*;

fn car_on(app: &mut App, at: DVec3, heading: Vec3) -> Entity {
    let t = Transform::from_translation(at + DVec3::Y * presets::ride_height("sports_car"))
        .with_rotation(engine_vehicles::demo::facing(heading));
    spawn_with(app, |c| presets::sports_car(c, t, 0xc4161c))
}

fn flat_app(surface: &str) -> App {
    let mut app = vehicle_app();
    ground(&mut app, DVec3::ZERO, 3_000.0, surface);
    app
}

#[test]
fn car_settles_on_its_suspension() {
    let mut app = flat_app("concrete");
    let car = car_on(&mut app, DVec3::ZERO, Vec3::NEG_Z);
    run_for(&mut app, 3.0);
    let t = transform(&app, car);
    let tel = telemetry(&app, car);
    assert_eq!(tel.wheels_grounded, 4, "{tel:?}");
    assert!(velocity(&app, car).linear.length() < 0.05, "at rest");
    let ride = t.translation.y;
    assert!(
        (ride - presets::ride_height("sports_car")).abs() < 0.08,
        "ride height {ride}"
    );
    assert!((t.rotation * Vec3::Y).y > 0.999, "level");
}

#[test]
fn sports_car_accelerates_shifts_and_tops_out() {
    let mut app = vehicle_app();
    ground(
        &mut app,
        DVec3::new(0.0, 0.0, -2_500.0),
        3_000.0,
        "concrete",
    );
    calm(&mut app);
    let car = car_on(&mut app, DVec3::ZERO, Vec3::NEG_Z);
    run_for(&mut app, 1.0);
    controls(&mut app, car, |c| c.throttle = 1.0);
    let to_100 = run_until(&mut app, 15.0, |app| {
        telemetry(app, car).speed > 100.0 / 3.6
    })
    .expect("reaches 100 km/h");
    assert!((4.0..6.5).contains(&to_100), "0-100 km/h in {to_100:.2} s");
    let mut top_gear = 0;
    let mut top = 0.0f32;
    run_until(&mut app, 55.0, |app| {
        let tel = telemetry(app, car);
        top_gear = top_gear.max(tel.gear);
        top = top.max(tel.forward_speed);
        false
    });
    assert_eq!(telemetry(&app, car).wheels_grounded, 4, "still on the road");
    let kmh = top * 3.6;
    assert!(top_gear >= 5, "shifted up to gear {top_gear}");
    // ≈ 280 km/h, where 250 kW meets drag (CdA 0.62 m²) and rolling resistance.
    assert!((250.0..310.0).contains(&kmh), "top speed {kmh:.0} km/h");
    // Went straight in calm air with the wheel centered.
    let p = transform(&app, car).translation;
    assert!(p.x.abs() < p.z.abs() * 0.002, "drifted sideways {p}");
}

fn brake_from_100(surface: &str) -> (f64, f64) {
    let mut app = flat_app(surface);
    calm(&mut app);
    let car = car_on(&mut app, DVec3::ZERO, Vec3::NEG_Z);
    run_for(&mut app, 1.0);
    set_velocity(&mut app, car, Vec3::NEG_Z * (100.0 / 3.6));
    run_for(&mut app, 0.1);
    let start = transform(&app, car).translation;
    controls(&mut app, car, |c| c.brake = 1.0);
    run_until(&mut app, 80.0, |app| telemetry(app, car).speed < 0.3).expect("stops");
    let end = transform(&app, car).translation;
    let d = end - start;
    (-d.z, d.x.abs())
}

#[test]
fn brakes_from_100_in_a_straight_line() {
    let (dist, sideways) = brake_from_100("concrete");
    // A performance car on a dry road: ~35–42 m.
    assert!(
        (30.0..48.0).contains(&dist),
        "stopping distance {dist:.1} m"
    );
    assert!(sideways < 1.0, "pulled {sideways:.2} m sideways");
}

#[test]
fn ice_takes_much_longer_to_stop() {
    let (dry, _) = brake_from_100("concrete");
    let (ice, _) = brake_from_100("ice");
    // Real cars need 5–10× as far on glare ice.
    assert!(ice > dry * 5.0, "ice {ice:.0} m vs dry {dry:.0} m");
}

#[test]
fn holds_a_skidpad_near_the_grip_limit() {
    let mut app = flat_app("concrete");
    let radius = 50.0;
    let path: Vec<DVec3> = (0..48)
        .map(|i| {
            let a = i as f64 / 48.0 * std::f64::consts::TAU;
            DVec3::new(a.cos() * radius, 0.0, a.sin() * radius)
        })
        .collect();
    let start = path[0];
    let car = car_on(&mut app, start, Vec3::Z);
    app.world
        .entity_mut(car)
        .insert(Autopilot::new(AutopilotMode::Drive {
            path,
            looped: true,
            max_speed: 40.0,
            lateral_g: 1.05,
        }));
    run_for(&mut app, 25.0);
    // Steady state: measure speed and how well the line is held over one more lap.
    let mut worst = 0.0f64;
    let mut speeds = Vec::new();
    run_until(&mut app, 12.0, |app| {
        let p = transform(app, car).translation;
        worst = worst.max(((p.x * p.x + p.z * p.z).sqrt() - radius).abs());
        speeds.push(telemetry(app, car).speed);
        false
    });
    let v = speeds.iter().sum::<f32>() / speeds.len() as f32;
    let lateral_g = v * v / radius as f32 / 9.81;
    assert!(
        lateral_g > 0.8,
        "cornering at {lateral_g:.2} g ({v:.1} m/s)"
    );
    assert!(worst < 4.0, "left the line by {worst:.1} m");
    assert!(
        (transform(&app, car).rotation * Vec3::Y).y > 0.95,
        "stayed on its wheels"
    );
}

#[test]
fn handbrake_turn_swings_the_tail_out() {
    let mut app = flat_app("concrete");
    let car = car_on(&mut app, DVec3::ZERO, Vec3::NEG_Z);
    run_for(&mut app, 1.0);
    set_velocity(&mut app, car, Vec3::NEG_Z * 16.0);
    run_for(&mut app, 0.2);
    controls(&mut app, car, |c| {
        c.steer = -1.0;
        c.handbrake = true;
    });
    let mut max_slip = 0.0f32;
    let mut skidded = false;
    run_until(&mut app, 2.5, |app| {
        let tel = telemetry(app, car);
        max_slip = max_slip.max(tel.sideslip_deg.abs());
        skidded |= tel.skidding;
        false
    });
    assert!(skidded, "rear tires slid");
    assert!(max_slip > 25.0, "body slip only {max_slip:.0}°");
    let fwd = transform(&app, car).rotation * Vec3::NEG_Z;
    let turned = fwd.x.atan2(-fwd.z).to_degrees().abs();
    assert!(turned > 60.0, "turned {turned:.0}°");
    assert!(
        (transform(&app, car).rotation * Vec3::Y).y > 0.9,
        "didn't roll over"
    );
}

#[test]
fn rally_car_holds_a_drift_on_dirt() {
    let mut app = flat_app("dirt");
    calm(&mut app);
    let radius = 15.0f32;
    let t = Transform::from_translation(DVec3::new(
        radius as f64,
        presets::ride_height("rally_car"),
        0.0,
    ))
    .with_rotation(engine_vehicles::demo::facing(Vec3::NEG_Z));
    let car = spawn_with(&mut app, |c| presets::rally_car(c, t));
    app.world.entity_mut(car).insert((
        Velocity {
            linear: Vec3::NEG_Z * 9.0,
            angular: Vec3::ZERO,
        },
        Autopilot::new(AutopilotMode::Drift {
            center: DVec3::ZERO,
            radius,
            speed: 10.0,
            slip_deg: 30.0,
            clockwise: false,
        }),
    ));
    run_for(&mut app, 15.0);
    let mut slips = Vec::new();
    let mut worst = 0.0f32;
    run_until(&mut app, 8.0, |app| {
        let tel = telemetry(app, car);
        slips.push(tel.sideslip_deg.abs());
        let p = transform(app, car).translation;
        worst = worst.max(((p.x * p.x + p.z * p.z).sqrt() as f32 - radius).abs());
        false
    });
    let slip = slips.iter().sum::<f32>() / slips.len() as f32;
    assert!((15.0..50.0).contains(&slip), "average slip {slip:.0}°");
    assert!(worst < 8.0, "wandered {worst:.1} m off the circle");
    assert!((transform(&app, car).rotation * Vec3::Y).y > 0.9, "upright");
}

#[test]
fn jumps_and_lands_on_its_wheels() {
    let mut app = flat_app("concrete");
    // A 7.5° kicker 6 m long, as on the showcase track, crossing the road at z = -60.
    let (len, angle, thick) = (6.0f32, 7.5f32.to_radians(), 0.6f32);
    let rot = Quat::from_rotation_x(angle);
    let base = DVec3::new(0.0, 0.0, -60.0);
    let center = base - (rot * Vec3::new(0.0, thick * 0.5, len * 0.5)).as_dvec3();
    static_box(
        &mut app,
        center,
        Vec3::new(6.0, thick * 0.5, len * 0.5),
        rot,
        "concrete",
    );
    let car = car_on(&mut app, DVec3::ZERO, Vec3::NEG_Z);
    run_for(&mut app, 1.0);
    set_velocity(&mut app, car, Vec3::NEG_Z * 30.0);
    controls(&mut app, car, |c| c.throttle = 0.4);
    let mut airborne = 0.0;
    let mut peak = 0.0f64;
    run_until(&mut app, 5.0, |app| {
        let tel = telemetry(app, car);
        if tel.wheels_grounded == 0 {
            airborne += DT;
        }
        peak = peak.max(transform(app, car).translation.y);
        false
    });
    let t = transform(&app, car);
    let tel = telemetry(&app, car);
    assert!(airborne > 0.4, "airborne {airborne:.2} s");
    assert!(peak > 1.2, "peak {peak:.2} m");
    assert_eq!(tel.wheels_grounded, 4, "landed on all four");
    assert!((t.rotation * Vec3::Y).y > 0.97, "landed upright");
    assert!(tel.speed > 20.0, "kept going at {:.1} m/s", tel.speed);
    assert!(t.translation.x.abs() < 3.0, "landed straight");
}

#[test]
fn parks_on_a_steep_slope() {
    let mut app = vehicle_app();
    let slope = 15f32.to_radians();
    let rot = Quat::from_rotation_x(slope);
    static_box(
        &mut app,
        DVec3::new(0.0, -1.0, 0.0),
        Vec3::new(20.0, 1.0, 40.0),
        rot,
        "concrete",
    );
    let up = rot * Vec3::Y;
    let at = DVec3::ZERO + (up * presets::ride_height("sports_car") as f32).as_dvec3();
    let t = Transform::from_translation(at).with_rotation(rot);
    let car = spawn_with(&mut app, |c| presets::sports_car(c, t, 0x1d4fb8));
    controls(&mut app, car, |c| c.handbrake = true);
    run_for(&mut app, 2.0);
    let before = transform(&app, car).translation;
    run_for(&mut app, 5.0);
    let moved = (transform(&app, car).translation - before).length();
    assert!(moved < 0.15, "slid {moved:.2} m down the slope");
}

#[derive(Resource, Default)]
struct Seen {
    contacts: Vec<GroundContact>,
    noises: usize,
}

fn record(
    mut seen: ResMut<Seen>,
    mut c: MessageReader<GroundContact>,
    mut n: MessageReader<NoiseEvent>,
) {
    seen.contacts.extend(c.read().copied());
    seen.noises += n.read().count();
}

#[test]
fn leaves_tracks_in_mud_and_makes_noise() {
    let mut app = flat_app("mud");
    app.init_resource::<Seen>().add_systems(Last, record);
    let car = car_on(&mut app, DVec3::ZERO, Vec3::NEG_Z);
    run_for(&mut app, 1.0);
    controls(&mut app, car, |c| c.throttle = 0.6);
    run_for(&mut app, 4.0);
    let mud = app
        .world
        .resource::<SurfaceMaterials>()
        .id("mud")
        .expect("mud");
    let seen = app.world.resource::<Seen>();
    let in_mud = seen
        .contacts
        .iter()
        .filter(|c| c.surface == Some(mud))
        .count();
    assert!(
        in_mud > 20,
        "{} tracks, {in_mud} in mud",
        seen.contacts.len()
    );
    assert!(seen.contacts.iter().all(|c| c.entity == Some(car)));
    assert!(seen.noises >= 5, "{} noise events", seen.noises);
    // Mud slows it down compared with concrete.
    let mud_speed = telemetry(&app, car).speed;
    let mut dry = flat_app("concrete");
    let car = car_on(&mut dry, DVec3::ZERO, Vec3::NEG_Z);
    run_for(&mut dry, 1.0);
    controls(&mut dry, car, |c| c.throttle = 0.6);
    run_for(&mut dry, 4.0);
    let dry_speed = telemetry(&dry, car).speed;
    assert!(
        mud_speed < dry_speed * 0.9,
        "mud {mud_speed:.1} vs dry {dry_speed:.1} m/s"
    );
}

use engine_world::SurfaceMaterials;
