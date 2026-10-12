//! How vehicles tell the rest of the world what they do, through `engine_world` messages
//! only: tire tracks and dust (`GroundContact`), engine noise AI can hear (`NoiseEvent`),
//! exhaust and tire smoke (`SmokeEmission`), jet and rocket wash (`WindField` gusts),
//! crashes (`WorldImpulse`, `DamageEvent`), wheels pushing what they stand on
//! (`ApplyImpulse`) and heat damage on re-entry. Every step each vehicle also carries
//! `EngineState` and `WheelContacts` components, which audio plays engines and tires from.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{ApplyImpulse, BodyKind, ContactEvent, ContactPhase, RigidBody};
use engine_world::{
    ContactKind, DamageEvent, DamageKind, EngineKind, EngineState, GroundContact, NoiseEvent,
    SmokeEmission, VehicleControls, WheelContact, WheelContacts, WindField, WorldImpulse,
};

use crate::aero::{Propulsion, PropulsorKind};
use crate::chassis::{Vehicle, VehicleForces, VehicleKind, VehicleSettings, VehicleTelemetry};
use crate::drivetrain::Drivetrain;
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
    settings: Res<VehicleSettings>,
    bodies: Query<&RigidBody>,
    mut q: Query<(Entity, &mut Wheels, &VehicleForces)>,
    mut contacts: MessageWriter<GroundContact>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut impulses: MessageWriter<ApplyImpulse>,
) {
    let g = 9.81;
    for (entity, mut wheels, forces) in &mut q {
        let dt = forces.frame.dt;
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

/// Seconds between rocket-wash smoke emissions.
const SMOKE_INTERVAL_S: f32 = 0.1;

/// Engine noise for AI, exhaust smoke and jet/rocket wash.
#[allow(clippy::type_complexity)]
pub fn engine_events_system(
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
    for (entity, vehicle, forces, tel, mut timers, propulsion, ship) in &mut q {
        let f = forces.frame;
        let dt = f.dt;
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
                        // Clean exhaust is nearly invisible; what shows is dust and steam
                        // kicked up off the pad: about 0.25 kg/s under a 1 MN hover (five smoke
                        // grenades), fading out as the ship climbs, mixed down to warm air.
                        let near = (1.0 - height / 150.0).clamp(0.0, 1.0);
                        let rate = 0.25 * (thrust / 1.0e6).sqrt() * near * near;
                        smoke.write(SmokeEmission {
                            position: ground,
                            velocity: exhaust * 6.0,
                            radius: 3.0 + thrust.sqrt() * 0.003,
                            mass_kg: rate * SMOKE_INTERVAL_S,
                            temperature_c: 45.0,
                            darkness: 0.03,
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
            timers.smoke = SMOKE_INTERVAL_S;
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
    mut q: Query<(Entity, &HeatShield, &VehicleForces, &mut EventTimers)>,
    mut damage: MessageWriter<DamageEvent>,
) {
    for (e, shield, forces, mut timers) in &mut q {
        timers.heat_damage -= forces.frame.dt;
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

/// What a vehicle's power plant is doing right now: a car's engine from its drivetrain, a
/// plane's from its engines, a ship's from its thrusters.
pub fn engine_state(
    controls: Option<&VehicleControls>,
    drivetrain: Option<&Drivetrain>,
    propulsion: Option<&Propulsion>,
    ship: Option<&Spacecraft>,
) -> Option<EngineState> {
    let throttle = controls.map_or(0.0, |c| c.throttle.max(0.0));
    if let Some(d) = drivetrain {
        let e = &d.engine;
        let rpm = d.state.rpm;
        let load = (d.state.engine_torque_nm / e.max_torque_at(rpm).max(1.0)).clamp(0.0, 1.0);
        let span = (e.redline_rpm - e.idle_rpm).max(1.0);
        let boost = if e.turbo {
            (((rpm - e.idle_rpm) / span) * 1.6).clamp(0.0, 1.0) * load
        } else {
            0.0
        };
        return Some(EngineState {
            kind: e.sound,
            running: true,
            rpm,
            idle_rpm: e.idle_rpm,
            max_rpm: e.redline_rpm,
            throttle,
            load,
            thrust_n: 0.0,
            boost,
        });
    }
    if let Some(p) = propulsion
        && let Some(first) = p.engines.first()
    {
        let level = p.engines.iter().map(|e| e.level).fold(0.0, f32::max);
        let thrust: f32 = p.engines.iter().map(|e| e.thrust_n).sum();
        let boost = controls.is_some_and(|c| c.boost);
        let (kind, idle, max) = match first.kind {
            // A light-aircraft flat four: 700 rpm idle, 2 700 rpm at full power.
            PropulsorKind::Propeller { .. } => {
                (EngineKind::Piston { cylinders: 4 }, 700.0, 2_700.0)
            }
            PropulsorKind::Jet => (EngineKind::Turbine, 3_000.0, 10_000.0),
            PropulsorKind::Rocket { .. } => (EngineKind::Rocket, 0.0, 0.0),
        };
        let rpm = if max > 0.0 {
            idle + (max - idle) * level
        } else {
            0.0
        };
        return Some(EngineState {
            kind,
            running: level > 0.0,
            rpm,
            idle_rpm: idle,
            max_rpm: max,
            throttle,
            load: level,
            thrust_n: thrust,
            boost: if boost && first.kind == PropulsorKind::Jet {
                1.0
            } else {
                0.0
            },
        });
    }
    if let Some(ship) = ship {
        let mut thrust = 0.0;
        let mut main = 0.0f32;
        let mut any = 0.0f32;
        for t in &ship.thrusters {
            thrust += t.level * t.max_thrust_n;
            any = any.max(t.level);
            if t.role != ThrusterRole::Rcs {
                main = main.max(t.level);
            }
        }
        return Some(EngineState {
            kind: if main > 0.01 || any <= 0.01 {
                EngineKind::Rocket
            } else {
                EngineKind::Thrusters
            },
            running: any > 0.01 && ship.propellant_kg > 0.0,
            throttle,
            load: main,
            thrust_n: thrust,
            ..Default::default()
        });
    }
    None
}

/// The wheels on the ground this step, for tire roll, squeal and gravel spray.
pub fn wheel_contacts(wheels: &Wheels) -> WheelContacts {
    WheelContacts {
        wheels: wheels
            .wheels
            .iter()
            .filter(|w| w.state.grounded)
            .map(|w| WheelContact {
                position: w.state.contact_point,
                surface: w.state.surface.unwrap_or_default(),
                rolling_speed: w.state.spin * w.radius,
                // Quiet while the tire grips; squeals as it nears the limit; 1 when sliding.
                slip: ((w.state.grip_used - 0.7) / 0.4).clamp(0.0, 1.0),
                load_kg: w.state.load_n / 9.81,
                radius_m: w.radius,
            })
            .collect(),
    }
}

/// Writes [`EngineState`] and [`WheelContacts`] on every vehicle each step (inserting them
/// the first time).
#[allow(clippy::type_complexity)]
pub fn engine_and_wheel_state_system(
    mut commands: Commands,
    mut q: Query<
        (
            Entity,
            Option<&VehicleControls>,
            Option<&Drivetrain>,
            Option<&Propulsion>,
            Option<&Spacecraft>,
            Option<&Wheels>,
            Option<&mut EngineState>,
            Option<&mut WheelContacts>,
        ),
        With<Vehicle>,
    >,
) {
    for (entity, controls, drivetrain, propulsion, ship, wheels, engine, contacts) in &mut q {
        if let Some(state) = engine_state(controls, drivetrain, propulsion, ship) {
            match engine {
                Some(mut e) => *e = state,
                None => {
                    commands.entity(entity).insert(state);
                }
            }
        }
        if let Some(w) = wheels {
            let now = wheel_contacts(w);
            match contacts {
                Some(mut c) => *c = now,
                None => {
                    commands.entity(entity).insert(now);
                }
            }
        }
    }
}
