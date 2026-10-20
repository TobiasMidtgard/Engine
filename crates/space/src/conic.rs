//! Free flight around one body from a position and velocity: ellipses, the parabola and
//! escape hyperbolas alike (Kepler's problem in universal variables), exactly and in constant
//! time. Ships coasting through time warp fly on these (see [`crate::rails`]).
//!
//! Positions and velocities are relative to the body, in any fixed (non-turning) axes.

use engine_core::math::DVec3;
use std::f64::consts::TAU;

/// A path in free fall around a body of gravitational parameter `mu`: where the craft was at
/// `epoch` (space clock seconds) and how fast it moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conic {
    pub position: DVec3,
    pub velocity: DVec3,
    pub mu: f64,
    pub epoch: f64,
}

impl Conic {
    pub fn new(position: DVec3, velocity: DVec3, mu: f64, epoch: f64) -> Self {
        Self {
            position,
            velocity,
            mu,
            epoch,
        }
    }

    /// Reciprocal semi-major axis 1/a = 2/r − v²/μ (> 0 ellipse, 0 parabola, < 0 hyperbola).
    pub fn alpha(&self) -> f64 {
        2.0 / self.position.length() - self.velocity.length_squared() / self.mu
    }

    /// Specific orbital energy, J/kg (negative while bound).
    pub fn energy(&self) -> f64 {
        self.velocity.length_squared() * 0.5 - self.mu / self.position.length()
    }

    pub fn eccentricity(&self) -> f64 {
        let r = self.position;
        let v = self.velocity;
        let e = (r * (v.length_squared() - self.mu / r.length()) - v * r.dot(v)) / self.mu;
        e.length()
    }

    /// Closest distance to the body's center along the path, meters.
    pub fn periapsis(&self) -> f64 {
        let h2 = self.position.cross(self.velocity).length_squared();
        h2 / self.mu / (1.0 + self.eccentricity())
    }

    /// Farthest distance, meters (infinite on escape paths).
    pub fn apoapsis(&self) -> f64 {
        let alpha = self.alpha();
        let e = self.eccentricity();
        if alpha <= 0.0 || e >= 1.0 {
            f64::INFINITY
        } else {
            (1.0 + e) / alpha
        }
    }

    /// Seconds per revolution (`None` on escape paths).
    pub fn period(&self) -> Option<f64> {
        let alpha = self.alpha();
        (alpha > 0.0 && self.eccentricity() < 1.0).then(|| TAU / (self.mu * alpha.powi(3)).sqrt())
    }

    /// Fastest speed along the path (at periapsis), m/s.
    pub fn max_speed(&self) -> f64 {
        let rp = self.periapsis().max(1.0);
        (self.mu * (2.0 / rp - self.alpha())).max(0.0).sqrt()
    }

    /// Position and velocity at space clock time `t`.
    pub fn state_at(&self, t: f64) -> (DVec3, DVec3) {
        let mut dt = t - self.epoch;
        if let Some(p) = self.period() {
            // Whole turns change nothing; keep the solve within one.
            dt = dt.rem_euclid(p);
        }
        if dt == 0.0 {
            return (self.position, self.velocity);
        }
        let mu = self.mu;
        let sq = mu.sqrt();
        let r0 = self.position;
        let v0 = self.velocity;
        let r0n = r0.length();
        let vr0 = r0.dot(v0) / r0n;
        let alpha = self.alpha();
        // Time since the epoch as a function of the universal anomaly χ: rises monotonically.
        let time = |x: f64| {
            let z = alpha * x * x;
            (r0n * vr0 / sq * x * x * stumpff_c(z)
                + (1.0 - alpha * r0n) * x.powi(3) * stumpff_s(z)
                + r0n * x)
                / sq
        };
        let rate = |x: f64| {
            let z = alpha * x * x;
            (r0n * vr0 / sq * x * (1.0 - z * stumpff_s(z))
                + (1.0 - alpha * r0n) * x * x * stumpff_c(z)
                + r0n)
                / sq
        };
        // Bracket χ: within one turn on an ellipse; on escape paths, as far as cosh stays
        // finite.
        let limit = if alpha > 1e-15 {
            TAU / alpha.sqrt() * 1.0001
        } else if alpha < -1e-15 {
            700.0 / (-alpha).sqrt()
        } else {
            1e12
        };
        let (mut lo, mut hi) = if dt > 0.0 {
            (0.0, limit)
        } else {
            (-limit, 0.0)
        };
        if (dt > 0.0 && time(hi) < dt) || (dt < 0.0 && time(lo) > dt) {
            // Beyond the reach of the bracket (absurdly far out): stop at its end.
            let x = if dt > 0.0 { hi } else { lo };
            return self.at_anomaly(x, dt.signum() * time(x).abs());
        }
        let mut x = (sq * alpha.abs().max(1e-30) * dt).clamp(lo, hi);
        if !(lo..=hi).contains(&x) || x == 0.0 {
            x = 0.5 * (lo + hi);
        }
        for _ in 0..100 {
            let f = time(x) - dt;
            if f > 0.0 {
                hi = x;
            } else {
                lo = x;
            }
            if f.abs() <= 1e-12 * dt.abs().max(1.0) {
                break;
            }
            let step = f / rate(x);
            let next = x - step;
            x = if next > lo && next < hi {
                next
            } else {
                0.5 * (lo + hi)
            };
            if (hi - lo).abs() <= 1e-14 * hi.abs().max(lo.abs()).max(1.0) {
                break;
            }
        }
        self.at_anomaly(x, dt)
    }

    /// Position and velocity at universal anomaly `x`, `dt` seconds after the epoch
    /// (Lagrange coefficients).
    fn at_anomaly(&self, x: f64, dt: f64) -> (DVec3, DVec3) {
        let sq = self.mu.sqrt();
        let r0 = self.position;
        let v0 = self.velocity;
        let r0n = r0.length();
        let alpha = self.alpha();
        let z = alpha * x * x;
        let (c, s) = (stumpff_c(z), stumpff_s(z));
        let f = 1.0 - x * x / r0n * c;
        let g = dt - x.powi(3) / sq * s;
        let r = r0 * f + v0 * g;
        let rn = r.length().max(1e-9);
        let fdot = sq / (rn * r0n) * (z * s - 1.0) * x;
        let gdot = 1.0 - x * x / rn * c;
        (r, r0 * fdot + v0 * gdot)
    }

    /// The same path, re-based at `t`.
    pub fn at(&self, t: f64) -> Conic {
        let (p, v) = self.state_at(t);
        Conic::new(p, v, self.mu, t)
    }

    /// `n` points along the path from `t0` to `t1` (for drawing a trajectory).
    pub fn path(&self, t0: f64, t1: f64, n: usize) -> Vec<DVec3> {
        let n = n.max(2);
        (0..n)
            .map(|i| self.state_at(t0 + (t1 - t0) * i as f64 / (n - 1) as f64).0)
            .collect()
    }
}

/// Stumpff C(z) = (1 − cos √z) / z, continued to z ≤ 0.
pub fn stumpff_c(z: f64) -> f64 {
    if z > 1e-3 {
        (1.0 - z.sqrt().cos()) / z
    } else if z < -1e-3 {
        ((-z).sqrt().cosh() - 1.0) / -z
    } else {
        1.0 / 2.0 - z / 24.0 + z * z / 720.0 - z * z * z / 40_320.0
    }
}

/// Stumpff S(z) = (√z − sin √z) / √z³, continued to z ≤ 0.
pub fn stumpff_s(z: f64) -> f64 {
    if z > 1e-3 {
        let s = z.sqrt();
        (s - s.sin()) / (s * s * s)
    } else if z < -1e-3 {
        let s = (-z).sqrt();
        (s.sinh() - s) / (s * s * s)
    } else {
        1.0 / 6.0 - z / 120.0 + z * z / 5_040.0 - z * z * z / 362_880.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orbit::Orbit;

    const MU_EARTH: f64 = 3.986_004_418e14;

    /// Plain numerical integration (RK4) to check the closed form against.
    fn integrate(r: DVec3, v: DVec3, mu: f64, dt: f64, steps: usize) -> (DVec3, DVec3) {
        let acc = |p: DVec3| -p * mu / p.length().powi(3);
        let h = dt / steps as f64;
        let (mut r, mut v) = (r, v);
        for _ in 0..steps {
            let (k1r, k1v) = (v, acc(r));
            let (k2r, k2v) = (v + k1v * h * 0.5, acc(r + k1r * h * 0.5));
            let (k3r, k3v) = (v + k2v * h * 0.5, acc(r + k2r * h * 0.5));
            let (k4r, k4v) = (v + k3v * h, acc(r + k3r * h));
            r += (k1r + k2r * 2.0 + k3r * 2.0 + k4r) * h / 6.0;
            v += (k1v + k2v * 2.0 + k3v * 2.0 + k4v) * h / 6.0;
        }
        (r, v)
    }

    #[test]
    fn ellipses_match_the_moons_orbit() {
        let moon = Orbit {
            semi_major_axis_m: 384_400_000.0,
            eccentricity: 0.0549,
            inclination_deg: 5.145,
            ascending_node_deg: 125.08,
            arg_periapsis_deg: 318.15,
            mean_anomaly_deg: 135.27,
        };
        let t0 = 1.0e5;
        let (r, v) = moon.state_at(MU_EARTH, t0);
        let c = Conic::new(r, v, MU_EARTH, t0);
        assert!((c.period().unwrap() - moon.period(MU_EARTH)).abs() < 1e-3);
        for dt in [1.0, 3_600.0, 5.0e5, 2.0e6, 3.3e7, -7.0e5] {
            let (a, va) = moon.state_at(MU_EARTH, t0 + dt);
            let (b, vb) = c.state_at(t0 + dt);
            assert!((a - b).length() < 2.0, "dt {dt}: {} m", (a - b).length());
            assert!((va - vb).length() < 1e-5, "dt {dt}");
        }
        assert!((c.periapsis() - 384_400_000.0 * (1.0 - 0.0549)).abs() < 1.0);
        assert!((c.apoapsis() - 384_400_000.0 * (1.0 + 0.0549)).abs() < 1.0);
    }

    #[test]
    fn escape_paths_match_integration() {
        // Low Earth orbit with a burn past escape speed, and a fast flyby.
        for (r, v) in [
            (
                DVec3::new(6_771_000.0, 0.0, 0.0),
                DVec3::new(0.0, 300.0, -11_500.0),
            ),
            (
                DVec3::new(-2.0e8, 1.0e7, 5.0e7),
                DVec3::new(2_500.0, -100.0, -300.0),
            ),
        ] {
            let c = Conic::new(r, v, MU_EARTH, 0.0);
            assert!(c.alpha() < 0.0 && c.period().is_none());
            assert!(c.apoapsis().is_infinite());
            for dt in [600.0, 20_000.0, 300_000.0] {
                let (a, va) = integrate(r, v, MU_EARTH, dt, 20_000);
                let (b, vb) = c.state_at(dt);
                assert!((a - b).length() < 30.0, "dt {dt}: {} m", (a - b).length());
                assert!((va - vb).length() < 0.01, "dt {dt}");
                // Energy and angular momentum hold.
                let cb = Conic::new(b, vb, MU_EARTH, dt);
                assert!((cb.energy() - c.energy()).abs() < 1e-6 * c.energy().abs());
            }
            // And back again.
            let later = c.at(50_000.0);
            let (back, _) = later.state_at(0.0);
            assert!((back - r).length() < 1e-3, "{}", (back - r).length());
        }
    }

    #[test]
    fn parabolas_and_radial_drops_stay_finite() {
        let r = DVec3::new(7.0e6, 0.0, 0.0);
        let escape = (2.0 * MU_EARTH / r.length()).sqrt();
        let c = Conic::new(r, DVec3::new(0.0, 0.0, escape), MU_EARTH, 0.0);
        let (a, _) = integrate(r, c.velocity, MU_EARTH, 5_000.0, 20_000);
        let (b, _) = c.state_at(5_000.0);
        assert!((a - b).length() < 5.0, "{}", (a - b).length());
        // Dropped from rest: falls straight in (and stays a number all the way).
        let drop = Conic::new(r, DVec3::ZERO, MU_EARTH, 0.0);
        let (p, _) = drop.state_at(300.0);
        assert!(p.x < r.x && p.x.is_finite() && p.y.abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn apsides_of_a_low_orbit() {
        let r = DVec3::new(6_771_000.0, 0.0, 0.0);
        let v = (MU_EARTH / r.length()).sqrt();
        let c = Conic::new(r, DVec3::new(0.0, 0.0, -v), MU_EARTH, 0.0);
        assert!((c.periapsis() - 6_771_000.0).abs() < 1.0);
        assert!((c.apoapsis() - 6_771_000.0).abs() < 1.0);
        let p = c.period().unwrap();
        assert!((p / 60.0 - 92.4).abs() < 0.5, "{} min", p / 60.0);
        assert!((c.max_speed() - v).abs() < 1e-6);
        let path = c.path(0.0, p, 9);
        assert!((path[0] - path[8]).length() < 1e-3);
        assert!(path.iter().all(|q| (q.length() - 6_771_000.0).abs() < 1.0));
    }
}
