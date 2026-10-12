//! Kepler orbits on rails: positions and velocities of bodies at any time, exactly and in
//! constant time (no integration drift, any time warp).
//!
//! Axes follow the engine: +Y is the ecliptic north pole, orbits lie near the X/Z plane, and
//! longitudes are measured from +X (the vernal equinox) counter-clockwise seen from +Y.

use engine_core::math::DVec3;
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

/// One astronomical unit, meters.
pub const AU: f64 = 149_597_870_700.0;
/// One day, seconds.
pub const DAY: f64 = 86_400.0;

/// Classical orbital elements around a parent body.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Orbit {
    pub semi_major_axis_m: f64,
    /// 0 circle .. <1 ellipse.
    pub eccentricity: f64,
    /// Tilt of the orbit against the ecliptic, degrees.
    pub inclination_deg: f64,
    /// Longitude of the ascending node, degrees.
    pub ascending_node_deg: f64,
    /// Argument of periapsis, degrees.
    pub arg_periapsis_deg: f64,
    /// Mean anomaly at the epoch (space clock = 0), degrees.
    pub mean_anomaly_deg: f64,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            semi_major_axis_m: AU,
            eccentricity: 0.0,
            inclination_deg: 0.0,
            ascending_node_deg: 0.0,
            arg_periapsis_deg: 0.0,
            mean_anomaly_deg: 0.0,
        }
    }
}

/// Solve Kepler's equation `M = E − e sin E` for the eccentric anomaly `E` (elliptic orbits).
pub fn solve_kepler(mean_anomaly: f64, e: f64) -> f64 {
    let m = (mean_anomaly + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI;
    let mut big_e = if e < 0.8 {
        m
    } else {
        std::f64::consts::PI * m.signum()
    };
    for _ in 0..30 {
        let f = big_e - e * big_e.sin() - m;
        let d = f / (1.0 - e * big_e.cos());
        big_e -= d;
        if d.abs() < 1e-15 {
            break;
        }
    }
    big_e
}

/// Standard (z-up) to engine (y-up) axes.
fn to_engine(v: DVec3) -> DVec3 {
    DVec3::new(v.x, v.z, -v.y)
}

fn from_engine(v: DVec3) -> DVec3 {
    DVec3::new(v.x, -v.z, v.y)
}

impl Orbit {
    pub fn circular(radius_m: f64) -> Self {
        Self {
            semi_major_axis_m: radius_m,
            ..Default::default()
        }
    }

    /// Mean motion, radians per second.
    pub fn mean_motion(&self, mu: f64) -> f64 {
        (mu / self.semi_major_axis_m.powi(3)).sqrt()
    }

    /// Seconds per revolution.
    pub fn period(&self, mu: f64) -> f64 {
        TAU / self.mean_motion(mu)
    }

    /// Mean anomaly at time `t` (seconds since the epoch), radians.
    pub fn mean_anomaly(&self, mu: f64, t: f64) -> f64 {
        self.mean_anomaly_deg.to_radians() + self.mean_motion(mu) * t
    }

    /// Mean longitude (node + periapsis + mean anomaly) at `t`, radians.
    pub fn mean_longitude(&self, mu: f64, t: f64) -> f64 {
        self.ascending_node_deg.to_radians()
            + self.arg_periapsis_deg.to_radians()
            + self.mean_anomaly(mu, t)
    }

    /// Position and velocity relative to the parent at `t` seconds since the epoch.
    pub fn state_at(&self, mu: f64, t: f64) -> (DVec3, DVec3) {
        let a = self.semi_major_axis_m;
        let e = self.eccentricity.clamp(0.0, 0.999_999);
        let n = self.mean_motion(mu);
        let big_e = solve_kepler(self.mean_anomaly(mu, t), e);
        let (sin_e, cos_e) = big_e.sin_cos();
        let b = a * (1.0 - e * e).sqrt();
        // Perifocal frame: x toward periapsis, y 90° ahead along the motion.
        let p = DVec3::new(a * (cos_e - e), b * sin_e, 0.0);
        let rate = n / (1.0 - e * cos_e);
        let v = DVec3::new(-a * sin_e * rate, b * cos_e * rate, 0.0);
        let rot = self.perifocal_to_ecliptic();
        (to_engine(rot(p)), to_engine(rot(v)))
    }

    /// Rotation from the perifocal frame to the (z-up) ecliptic frame.
    fn perifocal_to_ecliptic(&self) -> impl Fn(DVec3) -> DVec3 {
        let (so, co) = self.ascending_node_deg.to_radians().sin_cos();
        let (si, ci) = self.inclination_deg.to_radians().sin_cos();
        let (sw, cw) = self.arg_periapsis_deg.to_radians().sin_cos();
        // R = Rz(Ω) · Rx(i) · Rz(ω)
        let c1 = DVec3::new(co * cw - so * sw * ci, so * cw + co * sw * ci, sw * si);
        let c2 = DVec3::new(-co * sw - so * cw * ci, -so * sw + co * cw * ci, cw * si);
        move |p: DVec3| c1 * p.x + c2 * p.y
    }

    /// Elements of the (elliptic) orbit through position `r` with velocity `v` relative to
    /// a parent with gravitational parameter `mu`, at time `t`. `None` when the path is not
    /// a bound ellipse (escape trajectory or radial fall).
    pub fn from_state(r: DVec3, v: DVec3, mu: f64, t: f64) -> Option<Orbit> {
        let r = from_engine(r);
        let v = from_engine(v);
        let rl = r.length();
        let h = r.cross(v);
        if h.length() < 1e-9 || rl < 1e-9 {
            return None;
        }
        let energy = v.length_squared() * 0.5 - mu / rl;
        if energy >= 0.0 {
            return None;
        }
        let a = -mu / (2.0 * energy);
        let e_vec = v.cross(h) / mu - r / rl;
        let e = e_vec.length();
        let i = (h.z / h.length()).clamp(-1.0, 1.0).acos();
        let node = DVec3::Z.cross(h);
        let omega = if node.length() > 1e-12 {
            let o = (node.x / node.length()).clamp(-1.0, 1.0).acos();
            if node.y < 0.0 { TAU - o } else { o }
        } else {
            0.0
        };
        // Reference direction for the periapsis: the node line, or +X in the ecliptic plane.
        let node_dir = if node.length() > 1e-12 {
            node / node.length()
        } else {
            DVec3::X
        };
        let w = if e > 1e-10 {
            let mut w = (node_dir.dot(e_vec) / e).clamp(-1.0, 1.0).acos();
            let sign = if node.length() > 1e-12 {
                e_vec.z
            } else {
                h.z * e_vec.y
            };
            if sign < 0.0 {
                w = TAU - w;
            }
            w
        } else {
            0.0
        };
        // True → eccentric → mean anomaly.
        let nu = if e > 1e-10 {
            let mut nu = (e_vec.dot(r) / (e * rl)).clamp(-1.0, 1.0).acos();
            if r.dot(v) < 0.0 {
                nu = TAU - nu;
            }
            nu
        } else {
            // Circular: angle from the periapsis reference (node line) along the motion.
            let mut u = (node_dir.dot(r) / rl).clamp(-1.0, 1.0).acos();
            let ahead = h.cross(node_dir);
            if ahead.dot(r) < 0.0 {
                u = TAU - u;
            }
            u
        };
        let big_e = 2.0 * (((1.0 - e) / (1.0 + e)).sqrt() * (nu * 0.5).tan()).atan();
        let m = big_e - e * big_e.sin();
        let n = (mu / a.powi(3)).sqrt();
        Some(Orbit {
            semi_major_axis_m: a,
            eccentricity: e,
            inclination_deg: i.to_degrees(),
            ascending_node_deg: omega.to_degrees(),
            arg_periapsis_deg: w.to_degrees(),
            mean_anomaly_deg: (m - n * t).to_degrees().rem_euclid(360.0),
        })
    }

    /// Radius of the parent's sphere of influence for a body of `mu` on this orbit.
    pub fn sphere_of_influence(&self, mu: f64, parent_mu: f64) -> f64 {
        self.semi_major_axis_m * (mu / parent_mu).powf(0.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MU_SUN: f64 = 1.327_124_400_18e20;
    const MU_EARTH: f64 = 3.986_004_418e14;

    #[test]
    fn kepler_solves_high_eccentricity() {
        for &e in &[0.0, 0.1, 0.5, 0.9, 0.99] {
            for k in 0..50 {
                let m = k as f64 * 0.13 - 3.0;
                let big_e = solve_kepler(m, e);
                let back = big_e - e * big_e.sin();
                let wrapped =
                    (back - m + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI;
                assert!(wrapped.abs() < 1e-12, "e={e} m={m}");
            }
        }
    }

    #[test]
    fn earth_year_and_distance() {
        let earth = Orbit {
            semi_major_axis_m: AU,
            eccentricity: 0.0167,
            ..Default::default()
        };
        let year = earth.period(MU_SUN) / DAY;
        assert!((year - 365.25).abs() < 0.1, "{year}");
        let (p0, v0) = earth.state_at(MU_SUN, 0.0);
        // Starts at perihelion: closest, fastest.
        assert!((p0.length() - AU * (1.0 - 0.0167)).abs() < 1.0);
        assert!((v0.length() - 30_290.0).abs() < 30.0, "{}", v0.length());
        // Half a year later at aphelion; a full year later back where it started.
        let (p1, _) = earth.state_at(MU_SUN, earth.period(MU_SUN) * 0.5);
        assert!((p1.length() - AU * (1.0 + 0.0167)).abs() < 1.0);
        let (p2, _) = earth.state_at(MU_SUN, earth.period(MU_SUN));
        assert!((p2 - p0).length() < 1e-3 * AU * 1e-6);
    }

    #[test]
    fn prograde_orbits_turn_counter_clockwise_seen_from_north() {
        let o = Orbit::circular(7_000_000.0);
        let (p, v) = o.state_at(MU_EARTH, 0.0);
        // Angular momentum points to +Y (ecliptic north).
        assert!(p.cross(v).normalize().y > 0.999);
    }

    #[test]
    fn velocity_is_the_derivative_of_position() {
        let o = Orbit {
            semi_major_axis_m: 384_400_000.0,
            eccentricity: 0.0549,
            inclination_deg: 5.145,
            ascending_node_deg: 125.08,
            arg_periapsis_deg: 318.15,
            mean_anomaly_deg: 135.27,
        };
        let t = 123_456.0;
        let (_, v) = o.state_at(MU_EARTH, t);
        let (a, _) = o.state_at(MU_EARTH, t - 0.5);
        let (b, _) = o.state_at(MU_EARTH, t + 0.5);
        assert!(((b - a) - v).length() < 1e-3, "{:?} vs {:?}", b - a, v);
    }

    #[test]
    fn state_round_trips_through_elements() {
        let o = Orbit {
            semi_major_axis_m: 1.52 * AU,
            eccentricity: 0.0934,
            inclination_deg: 1.85,
            ascending_node_deg: 49.56,
            arg_periapsis_deg: 286.5,
            mean_anomaly_deg: 19.39,
        };
        let t = 3.0e7;
        let (r, v) = o.state_at(MU_SUN, t);
        let back = Orbit::from_state(r, v, MU_SUN, t).unwrap();
        for dt in [0.0, 1.0e6, 5.0e7] {
            let (r1, _) = o.state_at(MU_SUN, t + dt);
            let (r2, _) = back.state_at(MU_SUN, t + dt);
            assert!((r1 - r2).length() < 10.0, "dt {dt}: {}", (r1 - r2).length());
        }
        // Escape speed: no ellipse.
        assert!(Orbit::from_state(r, v * 1.5, MU_SUN, t).is_none());
    }
}
