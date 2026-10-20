//! Coming home from orbit: how a vehicle's wings and hull lift and drag at any angle of
//! attack ([`AeroTable`]), a fast prediction of where a glide from here comes down
//! ([`predict`]), and the angles the [`AutopilotMode::Return`](crate::AutopilotMode::Return)
//! autopilot steers with: how far round the planet the pad is, and how far off the track.
//!
//! The prediction flies a point mass with the vehicle's own lift and drag (the same surface
//! coefficients and hull drag the simulation uses, tabulated once), the planet's gravity and
//! the world's air, at the angle of attack [`entry_alpha`] schedules and one bank angle. The
//! autopilot runs it a few times a second: to time the burn out of orbit, and through
//! re-entry to pick the bank that brings the glide down at the pad (less vertical lift falls
//! short, more stretches the glide).

use engine_core::math::{DVec3, Vec3};
use engine_world::{Atmospheres, Gravity, WeatherState, WindField};

use crate::aero::Aircraft;
use crate::chassis::AeroBody;

/// Lift and drag of a whole vehicle per pascal of dynamic pressure (m²), in level,
/// unbanked, unyawed flight at angles of attack from 0° to 90°, a degree apart.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AeroTable {
    lift: Vec<f32>,
    drag: Vec<f32>,
}

impl AeroTable {
    /// The table for these wings (control surfaces centered) and this hull.
    pub fn new(aircraft: Option<&Aircraft>, body: Option<&AeroBody>) -> Self {
        let mut table = Self::default();
        for deg in 0..=90 {
            let a = (deg as f32).to_radians();
            // The body's motion through the air (body axes: −Z nose, +Y top), and the lift
            // direction square to it.
            let flow = Vec3::new(0.0, -a.sin(), -a.cos());
            let lift_dir = Vec3::new(0.0, a.cos(), -a.sin());
            let mut force = Vec3::ZERO;
            for s in aircraft.map_or(&[][..], |a| &a.surfaces[..]) {
                // As `aero_surfaces_system` does, for unit airspeed and dynamic pressure.
                let local = s.rotation.inverse() * flow;
                let chordwise = Vec3::new(0.0, local.y, local.z);
                let speed = chordwise.length();
                if speed < 1e-3 {
                    continue;
                }
                let aoa = (-local.y).atan2(-local.z);
                let c = s.coefficients(aoa, 0.0);
                let area = speed * speed * s.area();
                let drag_dir = -chordwise / speed;
                let surface_lift = drag_dir.cross(Vec3::X);
                force += s.rotation * (surface_lift * (c.lift * area) + drag_dir * (c.drag * area));
            }
            if let Some(b) = body {
                // As `aero_body_system` does: drag area per body axis.
                force -= flow * b.drag_area;
            }
            table.lift.push(force.dot(lift_dir));
            table.drag.push(-force.dot(flow));
        }
        table
    }

    /// `(lift, drag)` areas at angle of attack `alpha` (rad, clamped to 0..90°), m².
    pub fn at(&self, alpha: f32) -> (f32, f32) {
        if self.lift.is_empty() {
            return (0.0, 0.0);
        }
        let x = alpha.to_degrees().clamp(0.0, (self.lift.len() - 1) as f32);
        let i = (x as usize).min(self.lift.len() - 2);
        let t = x - i as f32;
        let mix = |v: &[f32]| v[i] + (v[i + 1] - v[i]) * t;
        (mix(&self.lift), mix(&self.drag))
    }
}

/// Angle of attack through re-entry (rad): nose 40° up while the air is hottest, easing to
/// 15° (a long glide) as the ship slows from 4 km/s to 600 m/s.
pub fn entry_alpha(speed: f32) -> f32 {
    let x = ((speed - 600.0) / 3_400.0).clamp(0.0, 1.0);
    (15.0 + 25.0 * x).to_radians()
}

/// Below this speed a predicted glide flies wings level, m/s.
pub const LEVEL_BELOW: f32 = 1_500.0;

/// The world's air, for predictions.
#[derive(Clone, Copy)]
pub struct Air<'a> {
    pub atmospheres: &'a Atmospheres,
    pub weather: &'a WeatherState,
    pub wind: &'a WindField,
}

impl Air<'_> {
    fn density(&self, gravity: &Gravity, p: DVec3) -> f32 {
        self.atmospheres
            .sample(self.weather, gravity, self.wind, p)
            .density
    }
}

/// A glide to predict: the vehicle's lift and drag, its mass, and the bank it holds (rad;
/// only its size matters, as the bank reverses side to side) until it slows to
/// [`LEVEL_BELOW`]; slower, it glides wings level (the autopilot steers for the pad itself
/// from there).
#[derive(Clone, Copy)]
pub struct Glide<'a> {
    pub table: &'a AeroTable,
    pub mass: f32,
    pub bank: f32,
    /// Load limit, g: nearing it the glide rolls its lift up, as [`load_floor`] says.
    pub max_g: f32,
}

/// The least cos(bank) a glide may fly at aerodynamic load `g`: none until 0.7 g below
/// `max_g`, then rising to wings level at the limit (lift up stops the dive deepening).
pub fn load_floor(g: f32, max_g: f32) -> f32 {
    ((g - (max_g - 0.7)) / 0.7).clamp(0.0, 1.0)
}

/// Where a predicted glide ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prediction {
    pub position: DVec3,
    pub velocity: DVec3,
    pub seconds: f64,
    /// Highest aerodynamic load on the way, g.
    pub peak_g: f32,
}

/// The glide from `p`, `v` until it is `stop_altitude_m` up (or `max_s` seconds on).
pub fn predict(
    gravity: &Gravity,
    air: Air,
    glide: Glide,
    p: DVec3,
    v: DVec3,
    stop_altitude_m: f64,
    max_s: f64,
) -> Prediction {
    let (mut p, mut v) = (p, v);
    let mut t = 0.0;
    let mut peak_g = 0.0f32;
    let accel = |p: DVec3, v: DVec3, peak_g: &mut f32| -> (DVec3, f32) {
        let mut a = gravity.at(p).as_dvec3();
        let rho = air.density(gravity, p);
        let speed = v.length() as f32;
        if rho > 0.0 && speed > 1.0 {
            let dir = (v / speed as f64).as_vec3();
            let up = gravity.up_at(p);
            let up_perp = (up - dir * up.dot(dir)).normalize_or_zero();
            let bank = if speed > LEVEL_BELOW { glide.bank } else { 0.0 };
            let (cl, cd) = glide.table.at(entry_alpha(speed));
            let q = 0.5 * rho * speed * speed / glide.mass.max(1.0);
            let load = (cl * cl + cd * cd).sqrt() * q / 9.81;
            let c = bank.cos().max(load_floor(load, glide.max_g));
            // Only the lift's vertical part: the autopilot reverses the bank now and then
            // to stay on track, so the sideways part cancels out instead of curving the
            // glide away round a circle.
            *peak_g = peak_g.max(load);
            a += ((up_perp * (c * cl) - dir * cd) * q).as_dvec3();
        }
        (a, rho)
    };
    while t < max_s {
        let (alt, _) = engine_world::air::altitude(gravity, p);
        if alt < stop_altitude_m {
            break;
        }
        // Midpoint steps: long ones in vacuum, short ones in the air.
        let (a0, rho) = accel(p, v, &mut peak_g);
        let dt = if rho <= 1e-9 {
            4.0
        } else if alt > 50_000.0 {
            1.0
        } else {
            0.5
        };
        let (pm, vm) = (p + v * (dt * 0.5), v + a0 * (dt * 0.5));
        let (am, _) = accel(pm, vm, &mut peak_g);
        p += vm * dt;
        v += am * dt;
        t += dt;
    }
    Prediction {
        position: p,
        velocity: v,
        seconds: t,
        peak_g,
    }
}

/// How far round the body centered at `center` the point `to` lies ahead of `from`, in the
/// plane of motion with normal `normal` (`r × v`), radians in (−π, π].
pub fn track_angle(center: DVec3, from: DVec3, to: DVec3, normal: DVec3) -> f64 {
    let (a, b) = (
        (from - center).normalize_or_zero(),
        (to - center).normalize_or_zero(),
    );
    a.cross(b).dot(normal.normalize_or_zero()).atan2(a.dot(b))
}

/// How far `to` lies off the plane of motion (normal `r × v`), radians; + = to the right.
pub fn cross_angle(center: DVec3, to: DVec3, normal: DVec3) -> f64 {
    let b = (to - center).normalize_or_zero();
    (-b.dot(normal.normalize_or_zero())).clamp(-1.0, 1.0).asin()
}

/// Horizontal speed at `r` that puts the far side of the orbit `periapsis_r` from the
/// center (m/s): the speed to slow to for a burn out of orbit.
pub fn speed_for_periapsis(mu: f64, r: f64, periapsis_r: f64) -> f64 {
    (2.0 * mu * periapsis_r / (r * (r + periapsis_r))).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::GravityBody;

    fn earth() -> Gravity {
        Gravity {
            uniform: Vec3::ZERO,
            bodies: vec![GravityBody {
                center: DVec3::new(0.0, -6.371e6, 0.0),
                mu: 3.986e14,
                radius: 6.371e6,
            }],
        }
    }

    #[test]
    fn the_spaceplane_lifts_about_as_much_as_it_drags_nose_high() {
        let table = AeroTable::new(
            Some(&crate::presets::spaceplane_aircraft()),
            Some(&AeroBody {
                drag_area: Vec3::new(40.0, 110.0, 6.0),
                ..Default::default()
            }),
        );
        let (l40, d40) = table.at(40f32.to_radians());
        let (l15, d15) = table.at(15f32.to_radians());
        // Hypersonic gliders fly near L/D 1 at 40°; nose lower, the glide stretches.
        assert!(
            (0.7..1.6).contains(&(l40 / d40)),
            "L/D {} at 40°",
            l40 / d40
        );
        assert!(l15 / d15 > l40 / d40 + 0.3, "L/D {} at 15°", l15 / d15);
        let (l0, d0) = table.at(0.0);
        assert!(l0.abs() < 5.0 && d0 > 0.0, "{l0} {d0}");
    }

    #[test]
    fn a_ballistic_fall_from_orbit_lands_where_kepler_says() {
        // No air: a fall from a 200 km orbit slowed for a periapsis 100 km underground meets
        // the surface 108.2° on (Kepler: e = 0.0234, true anomaly 71.8° at r = 6 371 km).
        let gravity = earth();
        let atmospheres = Atmospheres {
            bodies: vec![engine_world::BodyAtmosphere {
                sea_level_density: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let weather = WeatherState::default();
        let wind = WindField::default();
        let air = Air {
            atmospheres: &atmospheres,
            weather: &weather,
            wind: &wind,
        };
        let table = AeroTable::default();
        let r = 6.571e6;
        let center = gravity.bodies[0].center;
        let p = center + DVec3::new(0.0, r, 0.0);
        let v = DVec3::new(0.0, 0.0, -speed_for_periapsis(3.986e14, r, 6.271e6));
        let glide = Glide {
            table: &table,
            mass: 1.0,
            bank: 0.0,
            max_g: 2.5,
        };
        let land = predict(&gravity, air, glide, p, v, 0.0, 10_000.0);
        let normal = (p - center).cross(v);
        let angle = track_angle(center, p, land.position, normal).to_degrees();
        assert!((angle - 108.2).abs() < 1.0, "{angle}°");
    }
}
