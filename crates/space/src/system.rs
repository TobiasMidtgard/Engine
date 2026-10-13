//! Real star systems as data.
//!
//! The solar system uses J2000 mean orbital elements (JPL "Approximate Positions of the
//! Planets", Standish), IAU radii and rotation periods. The epoch (space clock 0) is
//! 2000-01-01 12:00 TT.

use crate::orbit::{AU, DAY, Orbit};
use crate::{BodyAir, BodyAtmosphere, BodyHaze, BodyKind, BodyLook, BodyRings, CelestialBody};

/// Planet elements from semi-major axis (AU), eccentricity, inclination, longitude of the
/// ascending node, longitude of perihelion and mean longitude (degrees).
fn planet_orbit(a_au: f64, e: f64, i: f64, node: f64, peri_lon: f64, mean_lon: f64) -> Orbit {
    Orbit {
        semi_major_axis_m: a_au * AU,
        eccentricity: e,
        inclination_deg: i,
        ascending_node_deg: node,
        arg_periapsis_deg: (peri_lon - node).rem_euclid(360.0),
        mean_anomaly_deg: (mean_lon - peri_lon).rem_euclid(360.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn planet(
    name: &str,
    mu: f64,
    radius_m: f64,
    orbit: Orbit,
    rotation_period_s: f64,
    tilt_deg: f64,
    color: [f32; 3],
    atmosphere: Option<BodyAtmosphere>,
) -> CelestialBody {
    CelestialBody {
        name: name.into(),
        kind: BodyKind::Planet,
        mu,
        radius_m,
        parent: Some("Sun".into()),
        orbit,
        rotation_period_s,
        axial_tilt_deg: tilt_deg,
        atmosphere,
        color,
        ..Default::default()
    }
}

/// Air at the surface (gas giants: at 1 bar): density kg/m³, scale height m, top m, °C.
/// NASA planetary fact sheets. Earth is left to the weather-driven standard atmosphere.
fn air(density: f32, scale_height_m: f32, top_m: f32, temperature_c: f32) -> Option<BodyAir> {
    Some(BodyAir {
        surface_density: density,
        scale_height_m,
        top_m,
        temperature_c,
    })
}

/// The Sun, the eight planets and the Moon, with their far-view looks: a detailed portrait
/// of each body, haze on Venus, Earth and Mars, real polar flattening and Saturn's rings.
pub fn solar_system() -> Vec<CelestialBody> {
    let mut bodies = bodies();
    for b in &mut bodies {
        b.air = match b.name.as_str() {
            "Venus" => air(65.0, 15_900.0, 250_000.0, 464.0),
            "Mars" => air(0.020, 11_100.0, 200_000.0, -63.0),
            "Jupiter" => air(0.16, 27_000.0, 1_000_000.0, -108.0),
            "Saturn" => air(0.19, 59_500.0, 1_000_000.0, -139.0),
            "Uranus" => air(0.42, 27_700.0, 1_000_000.0, -197.0),
            "Neptune" => air(0.45, 19_700.0, 1_000_000.0, -201.0),
            _ => None,
        };
        match b.name.as_str() {
            "Mercury" => b.look = BodyLook::Mercury,
            "Venus" => {
                b.look = BodyLook::Clouds;
                // Haze above the cloud tops: a thin bright rim.
                b.haze = Some(BodyHaze {
                    optical_depth: 0.5,
                    extinction: [1.0, 1.0, 1.05],
                    albedo: [1.0, 0.96, 0.86],
                    scale_height_m: 5_000.0,
                    forward: 0.6,
                });
            }
            "Earth" => {
                b.look = BodyLook::Earthlike;
                // Greenwich sidereal time at J2000 (280.46°): space clock 0 is 12:00 UT on
                // 2000-01-01, noon at 0° E.
                b.rotation_at_epoch_deg = 280.460_618 + 90.0 - 360.0;
                // Used when the renderer's atmosphere is on another body.
                b.haze = Some(BodyHaze::default());
                b.flattening = 0.003_353;
            }
            "Moon" => b.look = BodyLook::Moon,
            "Mars" => {
                b.look = BodyLook::Mars;
                // Dust low down with a thin bluish haze above it.
                b.haze = Some(BodyHaze {
                    optical_depth: 0.3,
                    extinction: [0.9, 1.0, 1.15],
                    albedo: [0.95, 0.78, 0.66],
                    scale_height_m: 11_000.0,
                    forward: 0.6,
                });
                b.flattening = 0.005_89;
            }
            "Jupiter" => {
                b.look = BodyLook::Jupiter;
                b.flattening = 0.064_87;
            }
            "Saturn" => {
                b.look = BodyLook::Saturn;
                b.rings = Some(BodyRings::default());
                b.flattening = 0.097_96;
            }
            "Uranus" => {
                b.look = BodyLook::Uranus;
                b.flattening = 0.022_9;
            }
            "Neptune" => {
                b.look = BodyLook::Neptune;
                b.flattening = 0.017_1;
            }
            _ => {}
        }
    }
    bodies
}

fn bodies() -> Vec<CelestialBody> {
    let hour = 3_600.0;
    vec![
        CelestialBody {
            name: "Sun".into(),
            kind: BodyKind::Star,
            mu: 1.327_124_400_18e20,
            radius_m: 696_340_000.0,
            parent: None,
            rotation_period_s: 25.38 * DAY,
            axial_tilt_deg: 7.25,
            luminosity_w: 3.828e26,
            color: [1.0, 0.95, 0.85],
            look: BodyLook::None,
            ..Default::default()
        },
        planet(
            "Mercury",
            2.2032e13,
            2_439_700.0,
            planet_orbit(
                0.387_099_27,
                0.205_635_93,
                7.004_979_02,
                48.330_765_93,
                77.457_796_28,
                252.250_323_50,
            ),
            58.646 * DAY,
            0.034,
            [0.45, 0.42, 0.4],
            None,
        ),
        planet(
            "Venus",
            3.248_59e14,
            6_051_800.0,
            planet_orbit(
                0.723_335_66,
                0.006_776_72,
                3.394_676_05,
                76.679_842_55,
                131.602_467_18,
                181.979_099_50,
            ),
            243.025 * DAY,
            177.36,
            [0.85, 0.78, 0.6],
            None,
        ),
        planet(
            "Earth",
            3.986_004_418e14,
            6_371_000.0,
            planet_orbit(
                1.000_002_61,
                0.016_711_23,
                0.0,
                0.0,
                102.937_681_93,
                100.464_571_66,
            ),
            86_164.090_5,
            23.439,
            [0.25, 0.4, 0.65],
            Some(BodyAtmosphere::default()),
        ),
        CelestialBody {
            name: "Moon".into(),
            kind: BodyKind::Moon,
            mu: 4.904_869_5e12,
            radius_m: 1_737_400.0,
            parent: Some("Earth".into()),
            orbit: Orbit {
                semi_major_axis_m: 384_400_000.0,
                eccentricity: 0.0549,
                inclination_deg: 5.145,
                ascending_node_deg: 125.08,
                arg_periapsis_deg: 318.15,
                mean_anomaly_deg: 135.27,
            },
            rotation_period_s: 27.321_661 * DAY,
            axial_tilt_deg: 1.54,
            tidally_locked: true,
            color: [0.6, 0.58, 0.55],
            ..Default::default()
        },
        planet(
            "Mars",
            4.282_837e13,
            3_389_500.0,
            planet_orbit(
                1.523_710_34,
                0.093_394_10,
                1.849_691_42,
                49.559_538_91,
                -23.943_629_59,
                -4.553_432_05,
            ),
            88_642.66,
            25.19,
            [0.75, 0.42, 0.25],
            None,
        ),
        planet(
            "Jupiter",
            1.266_865_34e17,
            69_911_000.0,
            planet_orbit(
                5.202_887_00,
                0.048_386_24,
                1.304_396_95,
                100.473_909_09,
                14.728_479_83,
                34.396_440_51,
            ),
            9.925 * hour,
            3.13,
            [0.8, 0.7, 0.55],
            None,
        ),
        planet(
            "Saturn",
            3.793_118_7e16,
            58_232_000.0,
            planet_orbit(
                9.536_675_94,
                0.053_861_79,
                2.485_991_87,
                113.662_424_48,
                92.598_878_31,
                49.954_244_23,
            ),
            10.656 * hour,
            26.73,
            [0.85, 0.78, 0.6],
            None,
        ),
        planet(
            "Uranus",
            5.793_939e15,
            25_362_000.0,
            planet_orbit(
                19.189_164_64,
                0.047_257_44,
                0.772_637_83,
                74.016_925_03,
                170.954_276_30,
                313.238_104_51,
            ),
            17.24 * hour,
            97.77,
            [0.6, 0.8, 0.85],
            None,
        ),
        planet(
            "Neptune",
            6.836_529e15,
            24_622_000.0,
            planet_orbit(
                30.069_922_76,
                0.008_590_48,
                1.770_043_47,
                131.784_225_74,
                44.964_762_27,
                -55.120_029_69,
            ),
            16.11 * hour,
            28.32,
            [0.3, 0.45, 0.85],
            None,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planets_have_their_real_years() {
        let sys = solar_system();
        let sun = sys.iter().find(|b| b.name == "Sun").unwrap().mu;
        let year = |name: &str| {
            sys.iter()
                .find(|b| b.name == name)
                .unwrap()
                .orbit
                .period(sun)
                / (365.25 * DAY)
        };
        assert!((year("Earth") - 1.0).abs() < 0.001);
        assert!((year("Mars") - 1.881).abs() < 0.002);
        assert!((year("Jupiter") - 11.86).abs() < 0.02);
    }

    #[test]
    fn moon_month_and_sphere_of_influence() {
        let sys = solar_system();
        let earth = sys.iter().find(|b| b.name == "Earth").unwrap();
        let moon = sys.iter().find(|b| b.name == "Moon").unwrap();
        let month = moon.orbit.period(earth.mu + moon.mu) / DAY;
        assert!((month - 27.32).abs() < 0.1, "{month}");
        let soi = moon.orbit.sphere_of_influence(moon.mu, earth.mu);
        assert!((soi - 66_100_000.0).abs() < 1_000_000.0, "{soi}");
        let sun = sys.iter().find(|b| b.name == "Sun").unwrap();
        let earth_soi = earth.orbit.sphere_of_influence(earth.mu, sun.mu);
        assert!(
            (earth_soi - 924_000_000.0).abs() < 10_000_000.0,
            "{earth_soi}"
        );
    }
}
