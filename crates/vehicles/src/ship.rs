//! Spacecraft: thrusters anywhere on the hull, automatic thruster allocation, flight assist
//! (from raw Newtonian to "hover"), propellant that runs out and makes the ship lighter, and
//! heat shields that glow on re-entry.
//!
//! Pilots and AI ask for a motion ("roll right", "go up", "stop drifting") through
//! `VehicleControls`. Flight assist turns that into a force and torque the ship should make,
//! and the allocator ([`allocate`]) finds the throttle for every thruster that comes closest,
//! so any thruster layout (a shuttle, a lander, a damaged ship missing two RCS quads) flies
//! without per-ship code.

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_world::VehicleControls;
use engine_world::physics::RigidBody;

use crate::chassis::{Vehicle, VehicleForces, VehicleTelemetry};

/// Standard gravity for specific impulse, m/s².
pub const G0: f32 = 9.80665;
/// The allocator handles at most this many thrusters per ship.
pub const MAX_THRUSTERS: usize = 48;

/// What a thruster is for (drawing, sound and tuning; the allocator uses all of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThrusterRole {
    /// Big engines for acceleration.
    #[default]
    Main,
    /// Reaction control: small thrusters for attitude and fine translation.
    Rcs,
    /// Downward engines for vertical take-off and landing.
    Hover,
}

/// One thruster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thruster {
    pub name: String,
    /// Body local, meters.
    pub position: Vec3,
    /// Direction the thruster pushes the ship, body local (its exhaust goes the other way).
    pub direction: Vec3,
    /// N.
    pub max_thrust_n: f32,
    pub role: ThrusterRole,
    /// Specific impulse, seconds (chemical rocket ≈ 300–450, ion ≈ 3 000).
    pub isp_s: f32,
    /// Seconds to reach a new throttle.
    pub response_secs: f32,
    /// Current throttle 0..1.
    pub level: f32,
}

impl Default for Thruster {
    fn default() -> Self {
        Self {
            name: "thruster".into(),
            position: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            max_thrust_n: 1_000.0,
            role: ThrusterRole::Rcs,
            isp_s: 300.0,
            response_secs: 0.08,
            level: 0.0,
        }
    }
}

/// How much the flight computer helps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AssistMode {
    /// Raw: inputs are force and torque fractions. Pure Newtonian, nothing stops a spin.
    Off,
    /// Inputs are rotation rates; releasing the stick stops the rotation.
    Rotation,
    /// Rotation, and drift without input is killed ("inertia dampening"); `brake` stops, and
    /// held on at a stop backs the ship up.
    #[default]
    Full,
    /// Full, plus thrust that cancels gravity so the ship hangs in the air; `lift` climbs
    /// and descends.
    Hover,
}

/// Flight computer settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlightAssist {
    pub mode: AssistMode,
    /// Rotation rate at full stick: pitch, yaw, roll, degrees per second.
    pub max_rate_deg_s: Vec3,
    /// How quickly rotation follows the stick, seconds.
    pub rotation_response_secs: f32,
    /// How quickly drift is killed, seconds.
    pub damping_response_secs: f32,
}

impl Default for FlightAssist {
    fn default() -> Self {
        Self {
            mode: AssistMode::Full,
            max_rate_deg_s: Vec3::new(30.0, 25.0, 60.0),
            rotation_response_secs: 0.4,
            damping_response_secs: 1.5,
        }
    }
}

/// A spacecraft.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spacecraft {
    pub thrusters: Vec<Thruster>,
    pub assist: FlightAssist,
    /// Ship mass without propellant, kg.
    pub dry_mass_kg: f32,
    /// Propellant left, kg. Thrusters stop when it runs out.
    pub propellant_kg: f32,
    /// Force (N, body axes) and torque (N·m) the flight computer asked for last step.
    pub demand_force: Vec3,
    pub demand_torque: Vec3,
    /// Fraction of the demand the thrusters actually delivered (1 = all of it).
    pub authority: f32,
}

impl Default for Spacecraft {
    fn default() -> Self {
        Self {
            thrusters: Vec::new(),
            assist: FlightAssist::default(),
            dry_mass_kg: 20_000.0,
            propellant_kg: 10_000.0,
            demand_force: Vec3::ZERO,
            demand_torque: Vec3::ZERO,
            authority: 1.0,
        }
    }
}

/// Find thruster throttles `u` (0..1 each) whose combined push comes closest to the wanted
/// body-axis force and torque. Torque is weighted so attitude control wins when the
/// thrusters can't do everything at once. `u` holds the warm start and the result.
pub fn allocate(
    thrusters: &[Thruster],
    com: Vec3,
    force: Vec3,
    torque: Vec3,
    u: &mut [f32],
) -> f32 {
    let n = thrusters.len().min(MAX_THRUSTERS).min(u.len());
    if n == 0 {
        return 0.0;
    }
    // Columns of the 6×n effect matrix, scaled so force and torque rows are comparable.
    let mut cols = [[0.0f32; 6]; MAX_THRUSTERS];
    let (mut f_scale, mut t_scale) = (1.0f32, 1.0f32);
    for (i, t) in thrusters.iter().take(n).enumerate() {
        let d = t.direction.normalize_or_zero() * t.max_thrust_n;
        let r = (t.position - com).cross(d);
        cols[i] = [d.x, d.y, d.z, r.x, r.y, r.z];
        f_scale = f_scale.max(d.length());
        t_scale = t_scale.max(r.length());
    }
    let weight = [1.0, 1.0, 1.0, 3.0, 3.0, 3.0];
    let scale = [
        1.0 / f_scale,
        1.0 / f_scale,
        1.0 / f_scale,
        1.0 / t_scale,
        1.0 / t_scale,
        1.0 / t_scale,
    ];
    let target = [force.x, force.y, force.z, torque.x, torque.y, torque.z];
    let mut lipschitz = 0.0;
    for c in cols.iter().take(n) {
        for k in 0..6 {
            lipschitz += (c[k] * scale[k]).powi(2) * weight[k];
        }
    }
    let step = 1.0 / lipschitz.max(1e-9);
    let residual = |u: &[f32], out: &mut [f32; 6]| {
        for k in 0..6 {
            out[k] = -target[k] * scale[k];
        }
        for (i, c) in cols.iter().take(n).enumerate() {
            for k in 0..6 {
                out[k] += c[k] * scale[k] * u[i];
            }
        }
    };
    let mut r = [0.0f32; 6];
    // Accelerated projected gradient (FISTA) on the box-constrained least squares.
    let mut y = [0.0f32; MAX_THRUSTERS];
    y[..n].copy_from_slice(&u[..n]);
    let mut prev = y;
    let mut t_k = 1.0f32;
    for _ in 0..60 {
        residual(&y[..n], &mut r);
        let mut next = [0.0f32; MAX_THRUSTERS];
        for (i, c) in cols.iter().take(n).enumerate() {
            let mut g = 0.0;
            for k in 0..6 {
                g += c[k] * scale[k] * weight[k] * r[k];
            }
            // A tiny pull toward zero picks the cheapest of equally good solutions.
            next[i] = (y[i] - step * (g + 1e-4 * y[i])).clamp(0.0, 1.0);
        }
        let t_next = (1.0 + (1.0 + 4.0 * t_k * t_k).sqrt()) * 0.5;
        for i in 0..n {
            y[i] = (next[i] + (t_k - 1.0) / t_next * (next[i] - prev[i])).clamp(0.0, 1.0);
        }
        prev = next;
        t_k = t_next;
    }
    u[..n].copy_from_slice(&prev[..n]);
    residual(&prev[..n], &mut r);
    let err: f32 = r
        .iter()
        .zip(weight)
        .map(|(x, w)| x * x * w)
        .sum::<f32>()
        .sqrt();
    let want: f32 = target
        .iter()
        .zip(scale)
        .zip(weight)
        .map(|((t, s), w)| (t * s).powi(2) * w)
        .sum::<f32>()
        .sqrt();
    if want < 1e-6 {
        1.0
    } else {
        (1.0 - err / want).clamp(0.0, 1.0)
    }
}

/// Largest force each thruster set can make along ±x, ±y, ±z (body axes), N.
pub(crate) fn axis_capacity(thrusters: &[Thruster]) -> [f32; 6] {
    let mut cap = [0.0f32; 6];
    for t in thrusters {
        let d = t.direction.normalize_or_zero() * t.max_thrust_n;
        cap[0] += d.x.max(0.0);
        cap[1] += (-d.x).max(0.0);
        cap[2] += d.y.max(0.0);
        cap[3] += (-d.y).max(0.0);
        cap[4] += d.z.max(0.0);
        cap[5] += (-d.z).max(0.0);
    }
    cap
}

/// Flight assist + allocation + thrust, and propellant use.
pub fn spacecraft_system(
    mut q: Query<(
        &mut Spacecraft,
        &VehicleControls,
        &mut VehicleForces,
        &mut VehicleTelemetry,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(mut ship, controls, mut forces, mut tel)| {
            let ship = &mut *ship;
            let f = forces.frame;
            let dt = f.dt;
            if dt <= 0.0 {
                return;
            }
            let inv = f.rotation.inverse();
            let v_local = inv * f.linear;
            let w_local = inv * f.angular;
            let com_local = inv * (f.com - f.position).as_vec3();
            let cap = axis_capacity(&ship.thrusters);
            let axis_max = |input: f32, pos: usize| {
                if input >= 0.0 {
                    input * cap[pos]
                } else {
                    input * cap[pos + 1]
                }
            };
            let assist = ship.assist;
            // --- rotation
            let rate = Vec3::new(controls.pitch, -controls.yaw, -controls.steer)
                .clamp(Vec3::splat(-1.0), Vec3::splat(1.0));
            let torque = match assist.mode {
                AssistMode::Off => {
                    // Raw torque: a fraction of what the thrusters could make about each axis.
                    let mut max_t = Vec3::ZERO;
                    for t in &ship.thrusters {
                        let r = (t.position - com_local)
                            .cross(t.direction.normalize_or_zero() * t.max_thrust_n);
                        max_t += r.abs();
                    }
                    rate * max_t * 0.5
                }
                _ => {
                    let target = rate * assist.max_rate_deg_s * std::f32::consts::PI / 180.0;
                    (target - w_local) * f.inertia / assist.rotation_response_secs.max(0.05)
                }
            };
            // --- translation
            let input = Vec3::new(
                controls.strafe.clamp(-1.0, 1.0),
                controls.lift.clamp(-1.0, 1.0),
                controls.brake.clamp(0.0, 1.0) - controls.throttle.clamp(0.0, 1.0),
            );
            let boost = if controls.boost { 1.0 } else { 0.0 };
            let mut force = Vec3::new(
                axis_max(input.x, 0),
                axis_max(input.y, 2),
                axis_max(input.z, 4),
            );
            if controls.boost {
                force.z = -cap[5] * boost;
            }
            if matches!(assist.mode, AssistMode::Full | AssistMode::Hover) {
                let damp = f.mass / assist.damping_response_secs.max(0.1);
                if controls.strafe.abs() < 0.01 {
                    force.x = (-v_local.x * damp).clamp(-cap[1], cap[0]);
                }
                if controls.lift.abs() < 0.01 {
                    force.y = (-v_local.y * damp).clamp(-cap[3], cap[2]);
                }
                if controls.brake > 0.01 && controls.throttle < 0.01 && v_local.z < -0.5 {
                    // Brake: thrust against the forward motion until stopped; held on once
                    // stopped, it backs the ship up (as a car's brake pedal engages reverse).
                    force.z = (-v_local.z * damp * 2.0).clamp(-cap[5], cap[4]) * controls.brake;
                }
            }
            if assist.mode == AssistMode::Hover {
                let g_local = inv * f.gravity;
                force -= g_local * f.mass;
            }
            ship.demand_force = force;
            ship.demand_torque = torque;

            // --- allocate and fire
            let n = ship.thrusters.len().min(MAX_THRUSTERS);
            let mut u = [0.0f32; MAX_THRUSTERS];
            for (i, t) in ship.thrusters.iter().take(n).enumerate() {
                u[i] = t.level;
            }
            let has_fuel = ship.propellant_kg > 0.0;
            ship.authority = if has_fuel {
                allocate(&ship.thrusters[..n], com_local, force, torque, &mut u[..n])
            } else {
                u = [0.0; MAX_THRUSTERS];
                0.0
            };
            let mut flow = 0.0;
            let mut main_level: f32 = 0.0;
            for (i, t) in ship.thrusters.iter_mut().take(n).enumerate() {
                let k = (dt / t.response_secs.max(0.005)).min(1.0);
                t.level += (u[i] - t.level) * k;
                if !has_fuel {
                    t.level = 0.0;
                }
                let thrust = t.level * t.max_thrust_n;
                if thrust <= 0.0 {
                    continue;
                }
                let dir = f.rotation * t.direction.normalize_or_zero();
                forces.add_at(dir * thrust, f.to_world(t.position));
                flow += thrust / (t.isp_s.max(1.0) * G0);
                if t.role == ThrusterRole::Main {
                    main_level = main_level.max(t.level);
                }
            }
            ship.propellant_kg = (ship.propellant_kg - flow * dt).max(0.0);
            tel.throttle = main_level;
        });
}

/// Keep the physics mass in step with propellant use (updated when it changes by 0.5 %).
pub fn propellant_mass_system(mut q: Query<(&Spacecraft, &mut Vehicle, &mut RigidBody)>) {
    for (ship, mut vehicle, mut body) in &mut q {
        let mass = ship.dry_mass_kg + ship.propellant_kg;
        let current = body.mass_kg.unwrap_or(vehicle.mass_kg);
        if (mass - current).abs() > current * 0.005 {
            let ratio = mass / current.max(1.0);
            vehicle.inertia *= ratio;
            vehicle.mass_kg = mass;
            body.mass_kg = Some(mass);
            if let Some(inertia) = body.inertia.as_mut() {
                *inertia *= ratio;
            }
        }
    }
}

/// A heat shield: anything that flies fast through air heats up (Sutton–Graves stagnation
/// heating) and cools by radiating. Hot hulls glow; past `max_temperature_k` they take
/// damage.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeatShield {
    /// Heated area facing the airflow, m².
    pub area_m2: f32,
    /// Nose (or leading edge) radius, m. Blunter noses heat less.
    pub nose_radius_m: f32,
    /// Heat capacity of the shield, J/K.
    pub heat_capacity_j_k: f32,
    /// 0..1 (ceramic tiles ≈ 0.85).
    pub emissivity: f32,
    pub max_temperature_k: f32,
    /// Current temperature, K.
    pub temperature_k: f32,
    /// Heat flux at the nose right now, W/m².
    pub heat_flux_w_m2: f32,
}

impl Default for HeatShield {
    fn default() -> Self {
        Self {
            area_m2: 20.0,
            nose_radius_m: 1.5,
            heat_capacity_j_k: 4.0e6,
            emissivity: 0.85,
            max_temperature_k: 1_900.0,
            temperature_k: 288.0,
            heat_flux_w_m2: 0.0,
        }
    }
}

/// Sutton–Graves constant for air, SI.
const SUTTON_GRAVES: f32 = 1.7415e-4;
const STEFAN_BOLTZMANN: f32 = 5.670_374e-8;

/// Heat and cool every heat shield.
pub fn heating_system(mut q: Query<(&mut HeatShield, &VehicleForces, &mut VehicleTelemetry)>) {
    q.par_iter_mut().for_each(|(mut shield, forces, mut tel)| {
        let f = forces.frame;
        let dt = f.dt;
        let v = f.airspeed_vector().length();
        let rho = f.air.density.max(0.0);
        let flux = SUTTON_GRAVES * (rho / shield.nose_radius_m.max(0.05)).sqrt() * v * v * v;
        let ambient = if rho > 0.0 { f.air.temperature_k } else { 3.0 };
        let t = shield.temperature_k;
        let radiated =
            STEFAN_BOLTZMANN * shield.emissivity * shield.area_m2 * (t.powi(4) - ambient.powi(4));
        // Plain convection at low speed (wind over a warm hull).
        let convection =
            (10.0 + 4.0 * v.min(300.0)) * (rho / 1.225).min(1.0) * shield.area_m2 * (t - ambient);
        let heat_in = flux * shield.area_m2 * 0.5;
        // Stay stable for any step: heat capacity bounds the change.
        let delta = (heat_in - radiated - convection) * dt / shield.heat_capacity_j_k.max(1.0);
        let next = t + delta.min(2_000.0);
        // Cooling never overshoots below the surrounding air.
        shield.temperature_k = if delta < 0.0 {
            next.max(ambient.min(t))
        } else {
            next
        };
        shield.heat_flux_w_m2 = flux;
        tel.hull_temperature_k = shield.temperature_k;
    });
}

/// Glow color of a body at a temperature (blackbody, roughly), linear RGB, and a strength
/// in the renderer's emissive units (0 below ~800 K).
pub fn blackbody(temperature_k: f32) -> ([f32; 3], f32) {
    let t = temperature_k;
    if t < 800.0 {
        return ([1.0, 0.2, 0.05], 0.0);
    }
    // Rough fit of the Planckian locus from dull red to yellow-white.
    let x = ((t - 800.0) / 2_400.0).clamp(0.0, 1.0);
    let color = [
        1.0,
        (0.12 + 0.75 * x).min(0.95),
        (0.02 + 0.65 * x * x).min(0.85),
    ];
    // Visible radiance climbs steeply: a dull glow at 1 000 K, daylight-bright by 2 000 K.
    let strength = 10f32.powf(4.0 * ((t - 800.0) / 1_200.0).min(1.2)) - 1.0;
    (color, strength)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box ship with RCS quads front and back and one main engine.
    fn test_ship() -> Vec<Thruster> {
        let mut t = vec![Thruster {
            name: "main".into(),
            position: Vec3::new(0.0, 0.0, 5.0),
            direction: Vec3::NEG_Z,
            max_thrust_n: 100_000.0,
            role: ThrusterRole::Main,
            ..Default::default()
        }];
        for z in [-4.0f32, 4.0] {
            for (pos, dir) in [
                (Vec3::new(1.0, 0.0, z), Vec3::NEG_X),
                (Vec3::new(-1.0, 0.0, z), Vec3::X),
                (Vec3::new(0.0, 1.0, z), Vec3::NEG_Y),
                (Vec3::new(0.0, -1.0, z), Vec3::Y),
            ] {
                t.push(Thruster {
                    position: pos,
                    direction: dir,
                    max_thrust_n: 2_000.0,
                    ..Default::default()
                });
            }
        }
        t.push(Thruster {
            position: Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
            max_thrust_n: 4_000.0,
            ..Default::default()
        });
        t
    }

    fn net(thrusters: &[Thruster], u: &[f32]) -> (Vec3, Vec3) {
        let mut f = Vec3::ZERO;
        let mut tq = Vec3::ZERO;
        for (t, &u) in thrusters.iter().zip(u) {
            let d = t.direction * t.max_thrust_n * u;
            f += d;
            tq += t.position.cross(d);
        }
        (f, tq)
    }

    #[test]
    fn allocation_finds_pure_torque() {
        let ship = test_ship();
        let mut u = vec![0.0; ship.len()];
        // Pitch up without moving: front RCS pushes up, back pushes down.
        let want_t = Vec3::new(8_000.0, 0.0, 0.0);
        let authority = allocate(&ship, Vec3::ZERO, Vec3::ZERO, want_t, &mut u);
        let (f, t) = net(&ship, &u);
        assert!(authority > 0.9, "{authority}");
        assert!((t - want_t).length() < 800.0, "{t:?}");
        assert!(f.length() < 400.0, "no net force: {f:?}");
        assert!(u[0] < 0.01, "main engine stays off");
    }

    #[test]
    fn allocation_uses_the_main_engine_for_forward_thrust() {
        let ship = test_ship();
        let mut u = vec![0.0; ship.len()];
        let want = Vec3::new(0.0, 0.0, -80_000.0);
        allocate(&ship, Vec3::ZERO, want, Vec3::ZERO, &mut u);
        let (f, t) = net(&ship, &u);
        assert!((f - want).length() < 2_000.0, "{f:?}");
        assert!(t.length() < 500.0);
        assert!((u[0] - 0.8).abs() < 0.03, "{}", u[0]);
    }

    #[test]
    fn saturated_requests_keep_attitude_first() {
        let ship = test_ship();
        let mut u = vec![0.0; ship.len()];
        // Ask for far more sideways force than RCS has, plus a yaw.
        let authority = allocate(
            &ship,
            Vec3::ZERO,
            Vec3::new(50_000.0, 0.0, 0.0),
            Vec3::new(0.0, 6_000.0, 0.0),
            &mut u,
        );
        assert!(authority < 0.9);
        assert!(u.iter().all(|x| (0.0..=1.0).contains(x)));
        let (f, t) = net(&ship, &u);
        // The yaw is delivered; what's left of the RCS pushes sideways (at most 2.5 kN here
        // while still making the yaw).
        assert!((t.y - 6_000.0).abs() < 600.0, "{t:?}");
        assert!(f.x > 1_000.0, "still pushes sideways: {f:?}");
    }

    #[test]
    fn glow_starts_red_and_brightens() {
        assert_eq!(blackbody(500.0).1, 0.0);
        let (c1, s1) = blackbody(1_200.0);
        let (c2, s2) = blackbody(2_000.0);
        assert!(s2 > s1 * 3.0 && s1 > 0.0);
        assert!(c2[1] > c1[1] && c2[2] >= c1[2], "whiter when hotter");
    }
}
