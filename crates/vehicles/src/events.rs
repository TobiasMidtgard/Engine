//! How vehicles tell the rest of the world what they do, through `engine_world` messages
//! only: tire tracks and dust (`GroundContact`), engine noise AI can hear (`NoiseEvent`),
//! exhaust and tire smoke (`SmokeEmission`), jet and rocket wash (`WindField` gusts),
//! crashes (`WorldImpulse`, `DamageEvent`), wheels pushing what they stand on
//! (`ApplyImpulse`) and heat damage on re-entry.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{ApplyImpulse, BodyKind, ContactEvent, ContactPhase, RigidBody};
use engine_world::{
    ContactKind, DamageEvent, DamageKind, GroundContact, NoiseEvent, SmokeEmission, WindField,
    WorldImpulse,
};

use crate::aero::{Propulsion, PropulsorKind};
use crate::chassis::{Vehicle, VehicleForces, VehicleKind, VehicleSettings, VehicleTelemetry};
use crate::ship::{HeatShield, Spacecraft, ThrusterRole};
use crate::wheels::Wheels;

/// Per-vehicle timers for periodic events.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct EventTimers {
    pub noise: f32,
    pub smoke: f32,
    pub heat_damage: f32,
}

/// Wheel tracks, tire smoke and pushes on whatever the wheels stand on.
pub fn wheel_events_system(
    time: Res<Time>,
    settings: Res<VehicleSettings>,
    bodies: Query<&RigidBody>,
    mut q: Query<(Entity, &mut Wheels, &VehicleForces)>,
    mut contacts: MessageWriter<GroundContact>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut impulses: MessageWriter<ApplyImpulse>,
) {
    let dt = time.delta_secs();
    let g = 9.81;
    for (entity, mut wheels, forces) in &mut q {
        let yaw_of = |dir: Vec3| (-dir.z).atan2(dir.x);
        let rotation = forces.frame.rotation;
        for w in &mut wheels.wheels {
            let width = w.width;
            let radius = w.radius;
            let st = &mut w.state;
            if !st.grounded {
                continue;
            }
            if st.since_track_m >= settings.track_spacing_m
                || (st.skidding && st.since_track_m > 0.1)
            {
                st.since_track_m = 0.0;
                let dir =
                    rotation * engine_core::math::Quat::from_rotation_y(-st.steer) * Vec3::NEG_Z;
                contacts.write(GroundContact {
                    position: st.contact_point,
                    normal: st.contact_normal,
                    size: [width, (radius * 0.35).max(0.08)],
                    yaw: yaw_of(dir),
                    load_kg: st.load_n / g,
                    speed: st.ground_speed,
                    kind: ContactKind::Wheel,
                    surface: st.surface,
                    entity: Some(entity),
                });
            }
            if st.skidding && st.ground_speed > 4.0 {
                // Burning rubber: white-grey tire smoke.
                smoke.write(SmokeEmission {
                    position: st.contact_point + DVec3::new(0.0, 0.1, 0.0),
                    velocity: forces.frame.linear * 0.3 + Vec3::Y * 0.5,
                    radius: width * 1.5,
                    mass_kg: 0.004 * dt * st.grip_used.min(2.0),
                    temperature_c: 120.0,
                    darkness: 0.15,
                });
            }
            if let Some(other) = st.contact_entity
                && other != entity
                && bodies.get(other).is_ok_and(|b| b.kind == BodyKind::Dynamic)
                && st.reaction.length_squared() > 1.0
            {
                impulses.write(ApplyImpulse {
                    entity: other,
                    impulse: st.reaction * dt,
                    at: Some(st.contact_point),
                    angular: Vec3::ZERO,
                });
            }
        }
    }
}

/// Engine noise for AI, exhaust smoke and jet/rocket wash.
#[allow(clippy::type_complexity)]
pub fn engine_events_system(
    time: Res<Time>,
    settings: Res<VehicleSettings>,
    mut wind: ResMut<WindField>,
    mut q: Query<(
        Entity,
        &Vehicle,
        &VehicleForces,
        &VehicleTelemetry,
        &mut EventTimers,
        Option<&Propulsion>,
        Option<&Spacecraft>,
    )>,
    mut noise: MessageWriter<NoiseEvent>,
    mut smoke: MessageWriter<SmokeEmission>,
) {
    let dt = time.delta_secs();
    for (entity, vehicle, forces, tel, mut timers, propulsion, ship) in &mut q {
        let f = forces.frame;
        timers.noise -= dt;
        timers.smoke -= dt;
        // How loud: idling car ≈ 40 m for AI hearing, racing car ≈ 300 m, jet ≈ 3 km,
        // rocket ≈ 10 km.
        let base = match vehicle.kind {
            VehicleKind::Car | VehicleKind::Boat | VehicleKind::Hovercraft => 40.0,
            VehicleKind::Truck => 70.0,
            VehicleKind::Aircraft => 300.0,
            VehicleKind::Spacecraft => 200.0,
        };
        let mut radius: f32 = base * (0.4 + tel.throttle);
        if let Some(p) = propulsion {
            for e in &p.engines {
                let loud = match e.kind {
                    PropulsorKind::Propeller { .. } => 800.0,
                    PropulsorKind::Jet => 3_000.0,
                    PropulsorKind::Rocket { .. } => 10_000.0,
                };
                radius = radius.max(loud * e.level);
            }
        }
        if let Some(ship) = ship {
            for t in &ship.thrusters {
                if t.level <= 0.01 {
                    continue;
                }
                let at = f.to_world(t.position);
                let exhaust = -(f.rotation * t.direction.normalize_or_zero());
                let thrust = t.level * t.max_thrust_n;
                if t.role != ThrusterRole::Rcs {
                    radius = radius.max(10_000.0 * (thrust / 1.0e6).sqrt().min(1.0));
                }
                // Rocket wash near the ground fans out along it, blowing dust and smoke
                // around the pad (outward, not back up into the ship's own wings).
                let height = f.air.altitude_m as f32;
                if thrust > 20_000.0 && height < 150.0 && f.air.density > 0.1 {
                    let exhaust_speed = (thrust / 1.0e5).sqrt() * 30.0;
                    let ground = at + (exhaust * height.clamp(1.0, 60.0)).as_dvec3();
                    // Steady state ≈ the exhaust speed (capped) against the field's 1.5/s decay.
                    let strength = exhaust_speed.min(30.0) * 1.5 * dt;
                    let spread = 6.0 + thrust.sqrt() * 0.005;
                    let (a, b) = exhaust.any_orthonormal_pair();
                    for k in 0..6 {
                        let angle = k as f32 * std::f32::consts::TAU / 6.0;
                        let out = a * angle.cos() + b * angle.sin();
                        wind.add_gust(
                            ground + (out * spread).as_dvec3(),
                            spread,
                            strength,
                            Some(out),
                        );
                    }
                    if timers.smoke <= 0.0 {
                        smoke.write(SmokeEmission {
                            position: ground,
                            velocity: exhaust * 15.0,
                            radius: 4.0 + thrust.sqrt() * 0.004,
                            mass_kg: thrust * 2e-6,
                            temperature_c: 400.0,
                            darkness: 0.08,
                        });
                    }
                }
            }
        }
        if timers.noise <= 0.0 {
            timers.noise = settings.noise_interval_s;
            noise.write(NoiseEvent {
                position: f.com,
                radius,
                source: Some(entity),
            });
        }
        if timers.smoke <= 0.0 {
            timers.smoke = 0.1;
        }
    }
}

/// Crashes: hard contacts become a `WorldImpulse` (dust, debris, nearby AI flinch) and a
/// `DamageEvent` on the vehicle.
pub fn crash_events_system(
    vehicles: Query<(&Vehicle, &VehicleForces)>,
    mut contacts: MessageReader<ContactEvent>,
    mut impulses: MessageWriter<WorldImpulse>,
    mut damage: MessageWriter<DamageEvent>,
) {
    for c in contacts.read() {
        if c.phase != ContactPhase::Started || c.sensor {
            continue;
        }
        for (me, sign) in [(c.a, -1.0f32), (c.b, 1.0)] {
            let Ok((_, forces)) = vehicles.get(me) else {
                continue;
            };
            let dv = c.impulse / forces.frame.mass.max(1.0);
            // Below ~4 m/s change of speed it's a bump, not a crash.
            if dv < 4.0 {
                continue;
            }
            let energy = 0.5 * forces.frame.mass * dv * dv;
            impulses.write(WorldImpulse {
                center: c.point,
                radius: 2.0 + dv * 0.4,
                strength: dv * 1.5,
                energy_j: energy,
                heat_c: 0.0,
                source: Some(me),
            });
            damage.write(DamageEvent {
                target: me,
                amount: energy / 2_000.0,
                kind: DamageKind::Blunt,
                point: c.point,
                impulse: c.normal * c.impulse * sign,
                source: Some(if me == c.a { c.b } else { c.a }),
            });
        }
    }
}

/// Overheating hulls take fire damage.
pub fn heat_damage_system(
    time: Res<Time>,
    mut q: Query<(Entity, &HeatShield, &VehicleForces, &mut EventTimers)>,
    mut damage: MessageWriter<DamageEvent>,
) {
    let dt = time.delta_secs();
    for (e, shield, forces, mut timers) in &mut q {
        timers.heat_damage -= dt;
        if shield.temperature_k <= shield.max_temperature_k || timers.heat_damage > 0.0 {
            continue;
        }
        timers.heat_damage = 0.5;
        damage.write(DamageEvent {
            target: e,
            amount: (shield.temperature_k - shield.max_temperature_k) * 0.05,
            kind: DamageKind::Fire,
            point: forces.frame.com,
            impulse: Vec3::ZERO,
            source: None,
        });
    }
}
