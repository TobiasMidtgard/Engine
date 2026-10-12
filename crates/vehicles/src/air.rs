//! The air a vehicle moves through: density, pressure, temperature, speed of sound and wind at
//! any point, from the shared `engine_world` weather, wind and gravity.
//!
//! Flat worlds measure altitude as world `y`. Planetary scenes (where [`Gravity::bodies`] is
//! not empty) measure it from the surface of the body that pulls hardest, so the same plane
//! and the same spaceship work on every planet. Each body can have its own atmosphere
//! ([`VehicleAir::bodies`]); bodies without an entry get the default (Earth-like) one, and an
//! entry with `sea_level_density: 0` is airless (the Moon).
//!
//! The profile is the International Standard Atmosphere scaled to the current weather: sea
//! level temperature and pressure come from [`WeatherState`], so a hot day really does make
//! planes need more runway.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::{Gravity, WeatherState, WindField};

/// Specific gas constant of dry air, J/(kg·K).
pub const R_AIR: f32 = 287.05;
/// Ratio of specific heats for air.
pub const GAMMA_AIR: f32 = 1.4;
/// Kelvin at 0 °C.
pub const KELVIN: f32 = 273.15;

/// The atmosphere of one gravity body.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyAtmosphere {
    /// Which gravity body this is, by its gravitational parameter μ (m³/s²); matched within
    /// 0.1 %. Earth ≈ 3.986e14, Moon ≈ 4.905e12, Mars ≈ 4.283e13.
    pub mu: f64,
    /// Air density at the surface, kg/m³ (Earth 1.225, Mars 0.020, Moon 0).
    pub sea_level_density: f32,
    /// Altitude over which density falls by e (×0.37), meters (Earth ≈ 8 500, Mars ≈ 11 100).
    pub scale_height_m: f32,
    /// No air above this altitude, meters (Earth ≈ 140 000).
    pub top_m: f32,
    /// Surface temperature, °C.
    pub temperature_c: f32,
}

impl Default for BodyAtmosphere {
    fn default() -> Self {
        Self {
            mu: 3.986e14,
            sea_level_density: 1.225,
            scale_height_m: 8_500.0,
            top_m: 140_000.0,
            temperature_c: 15.0,
        }
    }
}

/// How vehicles see the air. A resource; tune it per game or planet.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VehicleAir {
    /// Atmospheres of specific gravity bodies. Bodies not listed use the weather-driven
    /// standard atmosphere.
    pub bodies: Vec<BodyAtmosphere>,
    /// No air above this altitude for the default atmosphere, meters.
    pub top_m: f32,
}

impl Default for VehicleAir {
    fn default() -> Self {
        Self {
            bodies: Vec::new(),
            top_m: 140_000.0,
        }
    }
}

/// Air at one point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AirSample {
    /// kg/m³.
    pub density: f32,
    /// Pa.
    pub pressure_pa: f32,
    /// K.
    pub temperature_k: f32,
    /// m/s.
    pub speed_of_sound: f32,
    /// Wind velocity, m/s (world).
    pub wind: Vec3,
    /// Height above the surface (sea level) of the dominant body, meters.
    pub altitude_m: f64,
}

impl AirSample {
    /// Vacuum (space): no density, no wind.
    pub const VACUUM: AirSample = AirSample {
        density: 0.0,
        pressure_pa: 0.0,
        temperature_k: 3.0,
        speed_of_sound: 1.0,
        wind: Vec3::ZERO,
        altitude_m: f64::INFINITY,
    };

    /// Dynamic pressure ½ρv² for an airspeed, Pa.
    pub fn dynamic_pressure(&self, airspeed: f32) -> f32 {
        0.5 * self.density * airspeed * airspeed
    }
}

/// Which body dominates gravity at `p` and how high above its surface `p` is.
/// Returns `(altitude, Some(index))` on planets and `(p.y, None)` in flat worlds.
pub fn altitude(gravity: &Gravity, p: DVec3) -> (f64, Option<usize>) {
    let mut best: Option<(usize, f64)> = None;
    for (i, b) in gravity.bodies.iter().enumerate() {
        let r = (p - b.center).length().max(1.0);
        let pull = b.mu / (r * r);
        if best.is_none_or(|(_, best_pull)| pull > best_pull) {
            best = Some((i, pull));
        }
    }
    match best {
        Some((i, _)) => {
            let b = &gravity.bodies[i];
            ((p - b.center).length() - b.radius, Some(i))
        }
        None => (p.y, None),
    }
}

/// International Standard Atmosphere, scaled to a sea-level temperature (K) and pressure (Pa).
/// Returns `(temperature_k, pressure_pa, density)` at an altitude in meters.
pub fn standard_atmosphere(altitude_m: f64, t0_k: f32, p0_pa: f32) -> (f32, f32, f32) {
    const G0: f32 = 9.80665;
    const LAPSE: f32 = 0.0065;
    let h = altitude_m.max(-500.0) as f32;
    let t11 = (t0_k - LAPSE * 11_000.0).max(150.0);
    let (t, p) = if h <= 11_000.0 {
        let t = t0_k - LAPSE * h;
        (t, p0_pa * (t / t0_k).powf(G0 / (LAPSE * R_AIR)))
    } else {
        let p11 = p0_pa * (t11 / t0_k).powf(G0 / (LAPSE * R_AIR));
        if h <= 20_000.0 {
            (t11, p11 * (-(G0 * (h - 11_000.0)) / (R_AIR * t11)).exp())
        } else {
            // Above the tropopause: exponential with a ~6.8 km scale height fits the ISA
            // densities at 50 km (1e-3), 80 km (2e-5) and 100 km (6e-7) closely enough for
            // flight and re-entry.
            let p20 = p11 * (-(G0 * 9_000.0) / (R_AIR * t11)).exp();
            let rho = p20 / (R_AIR * t11) * (-(h - 20_000.0) / 6_800.0).exp();
            let t = (t11 + 0.001 * (h - 20_000.0)).min(270.0);
            return (t, rho * R_AIR * t, rho);
        }
    };
    (t, p, p / (R_AIR * t))
}

impl VehicleAir {
    /// Air at `p`.
    pub fn sample(
        &self,
        weather: &WeatherState,
        gravity: &Gravity,
        wind: &WindField,
        p: DVec3,
    ) -> AirSample {
        let (alt, body) = altitude(gravity, p);
        let custom = body.and_then(|i| {
            let mu = gravity.bodies[i].mu;
            self.bodies
                .iter()
                .find(|a| ((a.mu - mu) / mu.max(1.0)).abs() < 1e-3)
        });
        let (temperature_k, pressure_pa, density) = match custom {
            Some(a) => {
                if a.sea_level_density <= 0.0 || alt > a.top_m as f64 {
                    return AirSample {
                        altitude_m: alt,
                        ..AirSample::VACUUM
                    };
                }
                let t = a.temperature_c + KELVIN;
                let rho = a.sea_level_density * (-(alt.max(0.0) as f32) / a.scale_height_m).exp();
                (t, rho * R_AIR * t, rho)
            }
            None => {
                if alt > self.top_m as f64 {
                    return AirSample {
                        altitude_m: alt,
                        ..AirSample::VACUUM
                    };
                }
                // Fade the last 10 % to zero so drag and heating never step.
                let fade = ((self.top_m as f64 - alt) / (self.top_m as f64 * 0.1)).clamp(0.0, 1.0);
                let (t, p, rho) = standard_atmosphere(
                    alt,
                    weather.temperature_c + KELVIN,
                    weather.pressure_hpa * 100.0,
                );
                (t, p * fade as f32, rho * fade as f32)
            }
        };
        // Weather wind only exists in the lower atmosphere.
        let wind = if alt < 20_000.0 && density > 0.0 {
            wind.sample(p)
        } else {
            Vec3::ZERO
        };
        AirSample {
            density,
            pressure_pa,
            temperature_k,
            speed_of_sound: (GAMMA_AIR * R_AIR * temperature_k.max(1.0)).sqrt(),
            wind,
            altitude_m: alt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::GravityBody;

    #[test]
    fn standard_atmosphere_matches_reference_values() {
        let t0 = 288.15;
        let p0 = 101_325.0;
        let (_, _, rho0) = standard_atmosphere(0.0, t0, p0);
        assert!((rho0 - 1.225).abs() < 0.002, "{rho0}");
        let (t5, _, rho5) = standard_atmosphere(5_000.0, t0, p0);
        assert!((rho5 - 0.736).abs() < 0.01, "{rho5}");
        assert!((t5 - 255.65).abs() < 0.1);
        let (_, _, rho11) = standard_atmosphere(11_000.0, t0, p0);
        assert!((rho11 - 0.364).abs() < 0.01, "{rho11}");
        let (_, _, rho20) = standard_atmosphere(20_000.0, t0, p0);
        assert!((rho20 - 0.0889).abs() < 0.005, "{rho20}");
        let (_, _, rho50) = standard_atmosphere(50_000.0, t0, p0);
        assert!(rho50 > 4e-4 && rho50 < 2e-3, "{rho50}");
        let (_, _, rho100) = standard_atmosphere(100_000.0, t0, p0);
        assert!(rho100 > 1e-7 && rho100 < 3e-6, "{rho100}");
    }

    #[test]
    fn hot_days_have_thinner_air() {
        let (_, _, cold) = standard_atmosphere(0.0, KELVIN - 10.0, 101_325.0);
        let (_, _, hot) = standard_atmosphere(0.0, KELVIN + 40.0, 101_325.0);
        assert!(hot < cold * 0.9, "hot {hot} cold {cold}");
    }

    #[test]
    fn planets_measure_altitude_from_their_surface_and_can_be_airless() {
        let gravity = Gravity {
            uniform: Vec3::ZERO,
            bodies: vec![
                GravityBody {
                    center: DVec3::ZERO,
                    mu: 3.986e14,
                    radius: 6.371e6,
                },
                GravityBody {
                    center: DVec3::new(3.844e8, 0.0, 0.0),
                    mu: 4.905e12,
                    radius: 1.7374e6,
                },
            ],
        };
        let air = VehicleAir {
            bodies: vec![BodyAtmosphere {
                mu: 4.905e12,
                sea_level_density: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let weather = WeatherState::default();
        let wind = WindField::default();
        let ground = air.sample(&weather, &gravity, &wind, DVec3::new(0.0, 6.371e6, 0.0));
        assert!(ground.altitude_m.abs() < 1.0);
        assert!(ground.density > 1.0);
        let orbit = air.sample(
            &weather,
            &gravity,
            &wind,
            DVec3::new(0.0, 6.371e6 + 4e5, 0.0),
        );
        assert_eq!(orbit.density, 0.0);
        let moon = air.sample(
            &weather,
            &gravity,
            &wind,
            DVec3::new(3.844e8, 1.7374e6 + 10.0, 0.0),
        );
        assert!((moon.altitude_m - 10.0).abs() < 1.0, "{}", moon.altitude_m);
        assert_eq!(moon.density, 0.0);
    }

    #[test]
    fn speed_of_sound_at_sea_level() {
        let s = VehicleAir::default().sample(
            &WeatherState {
                temperature_c: 15.0,
                ..Default::default()
            },
            &Gravity::default(),
            &WindField::default(),
            DVec3::ZERO,
        );
        assert!(
            (s.speed_of_sound - 340.3).abs() < 1.0,
            "{}",
            s.speed_of_sound
        );
    }
}
