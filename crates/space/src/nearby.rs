//! The stars nearest the Sun and their planets: the real ones surveys found, and seeded,
//! physically plausible ones filling each system, so a ship can fly to another star and land.
//!
//! Stars: J2000 right ascension and declination, distance, and RECONS / SIMBAD masses, radii,
//! luminosities and temperatures, updated from Bond et al. 2015 and 2017 (Procyon and Sirius),
//! Pineda 2021 (Wolf 359) and González Hernández 2024 (Barnard's Star). Real planets:
//! Suárez Mascareño 2025 and Faria 2022 (Proxima), Basant 2025 (Barnard's Star), Thompson 2025
//! (Epsilon Eridani b). Candidates (Alpha Centauri A's JWST giant, Proxima c, Wolf 359 b,
//! Tau Ceti g, h and f) come only with `include_candidates`; refuted ones (Alpha Centauri Bb,
//! the 233-day Barnard b, Wolf 359 c, Tau Ceti e) are never added.
//!
//! The generator follows how Elite Dangerous treats real data: it constrains the seeded system
//! instead of replacing it. Each system is seeded from its star's name: real planets first,
//! then a compact inner chain of neighbours spaced 10 to 60 mutual Hill radii apart (Weiss
//! 2018: about 20 is most common, period ratios above 1.2) and of similar mass ("peas in a
//! pod"), and a giant or ice world or two past the snow line,
//! inside the binary stability limit (Holman & Wiegert 1999), away from debris belts and below
//! what radial-velocity surveys would already have found. Radii follow Chen & Kipping 2017;
//! classes follow the equilibrium temperature (SpaceEngine's 90/170/250/330/1000 K edges), the
//! snow line (2.7 AU·√L) and whether the planet keeps air (the "cosmic shoreline", Zahnle &
//! Catling 2017, fitted to the Solar System: airless when insolation exceeds (v_esc/6 km/s)⁴,
//! ten times stricter around M dwarfs); planets lock to their star when the tidal locking time
//! is shorter than the star's age.
//!
//! Orbital phases are seeded: radial-velocity periods are too uncertain to carry the measured
//! phases back to the J2000 epoch.

use crate::orbit::{AU, Orbit};
use crate::system::{LIGHT_YEAR, OBLIQUITY_DEG, ecliptic, star_color};
use crate::{BodyAir, BodyAtmosphere, BodyHaze, BodyKind, BodyLook, CelestialBody};
use engine_core::math::DVec3;
use serde::{Deserialize, Serialize};

const SUN_MU: f64 = 1.327_124_400_18e20;
const SUN_RADIUS: f64 = 695_700_000.0;
const SUN_LUMINOSITY: f64 = 3.828e26;
const SUN_MASS_KG: f64 = 1.988_47e30;
const EARTH_MU: f64 = 3.986_004_418e14;
const EARTH_RADIUS: f64 = 6_371_000.0;
const EARTH_MASS_KG: f64 = 5.972_2e24;
const G: f64 = 6.674_30e-11;
/// Bump when a change would reshuffle the seeded systems.
const GENERATOR_VERSION: u64 = 2;
/// Most planets a system gets.
const MAX_PLANETS: usize = 8;
/// Earth masses where Chen & Kipping's relation turns Neptunian and Jovian.
const NEPTUNIAN_M: f64 = 2.04;
const JOVIAN_M: f64 = 131.6;

/// What a planet's surface is like, for the terrain a game puts on it when a ship comes close
/// (see [`SurfaceRecipe`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum WorldClass {
    /// Bare, cratered rock with no air (the Moon, Mercury).
    #[default]
    Airless,
    /// Rock hot enough to melt (above about 1000 K on the day side).
    Lava,
    /// Hot and dry, 330-1000 K: ochre rock and dust, or under thick clouds like Venus.
    Desert,
    /// Temperate with seas and continents (250-330 K).
    Terra,
    /// Temperate and mostly sea.
    Ocean,
    /// Cold and dry (170-250 K), like Mars.
    ColdDesert,
    /// Frozen (below 170 K): ice plains and frozen seas.
    Ice,
    /// Frozen under a thick orange haze with dark hydrocarbon lakes (Titan).
    Hazy,
}

/// How to build a planet's surface: picked by the generator from its temperature, mass and air.
/// Space knows nothing of terrain; a game maps this to a terrain preset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SurfaceRecipe {
    pub class: WorldClass,
    /// Seed of the terrain's shapes and colors.
    pub seed: u64,
    /// Share of the surface under sea (or frozen sea), 0..1.
    pub sea_fraction: f64,
    /// 0..1 how crowded its craters are.
    pub crater_density: f64,
    /// Surface pressure, bar (0 = airless).
    pub pressure_bar: f64,
    /// 0..1 cloud cover.
    pub cloud_cover: f32,
    /// Mean surface temperature, kelvin (equilibrium temperature plus the air's greenhouse).
    pub temperature_k: f64,
    /// Surface temperature of the star it circles, kelvin: the color of its daylight, and of
    /// plants that grew under it.
    pub star_kelvin: f64,
}

/// A planet the surveys found.
struct Known {
    host: &'static str,
    letter: char,
    candidate: bool,
    /// Earth masses: the minimum (m sin i) unless `true_mass`.
    mass: f64,
    true_mass: bool,
    a_au: f64,
    e: f64,
    omega_deg: f64,
    /// Inclination and node against the sky, degrees, when measured.
    sky: Option<(f64, f64)>,
}

const fn known(host: &'static str, letter: char, mass: f64, a_au: f64, e: f64) -> Known {
    Known {
        host,
        letter,
        candidate: false,
        mass,
        true_mass: false,
        a_au,
        e,
        omega_deg: 0.0,
        sky: None,
    }
}

const fn candidate(host: &'static str, letter: char, mass: f64, a_au: f64, e: f64) -> Known {
    Known {
        candidate: true,
        ..known(host, letter, mass, a_au, e)
    }
}

const KNOWN: &[Known] = &[
    known("Proxima Centauri", 'd', 0.260, 0.028_81, 0.0),
    known("Proxima Centauri", 'b', 1.055, 0.048_48, 0.0),
    known("Barnard's Star", 'd', 0.263, 0.018_8, 0.0),
    known("Barnard's Star", 'b', 0.299, 0.022_9, 0.0),
    known("Barnard's Star", 'c', 0.335, 0.027_4, 0.0),
    known("Barnard's Star", 'e', 0.193, 0.038_1, 0.0),
    Known {
        true_mass: true,
        omega_deg: 268.0,
        sky: Some((41.0, 191.0)),
        ..known("Epsilon Eridani", 'b', 311.5, 3.53, 0.06)
    },
    candidate("Alpha Centauri A", 'b', 120.0, 1.62, 0.4),
    candidate("Proxima Centauri", 'c', 5.8, 1.48, 0.0),
    candidate("Wolf 359", 'b', 43.9, 1.85, 0.0),
    candidate("Tau Ceti", 'g', 1.75, 0.133, 0.06),
    candidate("Tau Ceti", 'h', 1.83, 0.243, 0.23),
    candidate("Tau Ceti", 'f', 3.93, 1.334, 0.16),
];

/// Letters of refuted planets: never reused for a seeded one.
const REFUTED: &[(&str, char)] = &[
    ("Alpha Centauri B", 'b'),
    ("Wolf 359", 'c'),
    ("Tau Ceti", 'e'),
];

/// A star near the Sun and what its planets must respect.
struct Star {
    name: &'static str,
    ra_deg: f64,
    dec_deg: f64,
    light_years: f64,
    /// Solar masses, radii and luminosities; surface temperature, kelvin.
    mass: f64,
    radius: f64,
    luminosity: f64,
    kelvin: f64,
    age_gyr: f64,
    /// Outermost stable planet orbit around a binary member (AU), else 0 (30 AU·M).
    outer_au: f64,
    /// Lowest inclination of the system's plane against the sky, degrees.
    min_inclination_deg: f64,
    /// (from AU, to AU, most m sin i in Earth masses) the surveys would have found.
    limits: &'static [(f64, f64, f64)],
    /// Debris belts, AU: no planets there.
    belts: &'static [(f64, f64)],
    /// Planets of its own (white dwarfs have none).
    planets: bool,
}

const STARS: &[Star] = &[
    Star {
        name: "Alpha Centauri A",
        ra_deg: 219.902_1,
        dec_deg: -60.834_0,
        light_years: 4.344,
        mass: 1.0788,
        radius: 1.2175,
        luminosity: 1.5059,
        kelvin: 5790.0,
        age_gyr: 5.3,
        outer_au: 2.7,
        min_inclination_deg: 20.0,
        limits: &[],
        belts: &[],
        planets: true,
    },
    Star {
        name: "Proxima Centauri",
        ra_deg: 217.429_0,
        dec_deg: -62.679_5,
        light_years: 4.2465,
        mass: 0.1221,
        radius: 0.1542,
        luminosity: 0.001_55,
        kelvin: 3042.0,
        age_gyr: 4.85,
        outer_au: 0.0,
        min_inclination_deg: 20.0,
        // NIRPS: nothing above a few Earth masses close in besides b and d.
        limits: &[(0.0, 0.3, 3.0)],
        belts: &[],
        planets: true,
    },
    Star {
        name: "Barnard's Star",
        ra_deg: 269.452_1,
        dec_deg: 4.693_4,
        light_years: 5.963,
        mass: 0.162,
        radius: 0.19,
        luminosity: 0.003_55,
        kelvin: 3195.0,
        age_gyr: 10.0,
        outer_au: 0.0,
        // Its four planets are stable only seen at least 24° from face-on (Basant 2025).
        min_inclination_deg: 30.0,
        // Nothing above 0.57 M⊕ in the habitable zone (periods 10-42 days).
        limits: &[(0.0, 0.05, 0.4), (0.05, 0.13, 0.57)],
        belts: &[],
        planets: true,
    },
    Star {
        name: "Wolf 359",
        ra_deg: 164.120_5,
        dec_deg: 7.014_7,
        light_years: 7.86,
        mass: 0.110,
        radius: 0.144,
        luminosity: 0.001_06,
        kelvin: 2749.0,
        age_gyr: 0.3,
        outer_au: 0.0,
        min_inclination_deg: 20.0,
        // No planet over half a Jupiter within 1 AU, no brown dwarf within 10.
        limits: &[(0.0, 1.0, 159.0), (1.0, 10.0, 4000.0)],
        belts: &[],
        planets: true,
    },
    Star {
        name: "Sirius",
        ra_deg: 101.287_2,
        dec_deg: -16.716_1,
        light_years: 8.60,
        mass: 2.063,
        radius: 1.7144,
        luminosity: 24.74,
        kelvin: 9845.0,
        age_gyr: 0.24,
        outer_au: 2.2,
        min_inclination_deg: 20.0,
        limits: &[(0.0, 5.0, 3178.0)],
        belts: &[],
        planets: true,
    },
    Star {
        name: "Epsilon Eridani",
        ra_deg: 53.232_7,
        dec_deg: -9.458_3,
        light_years: 10.475,
        mass: 0.82,
        radius: 0.735,
        luminosity: 0.34,
        kelvin: 5084.0,
        age_gyr: 0.6,
        outer_au: 0.0,
        min_inclination_deg: 20.0,
        // JWST sees no gaps a giant would carve between 3 and 50 AU.
        limits: &[(2.9, 50.0, 10.0)],
        belts: &[(1.7, 2.1), (63.0, 76.0)],
        planets: true,
    },
    Star {
        name: "Procyon",
        ra_deg: 114.825_5,
        dec_deg: 5.225_0,
        light_years: 11.46,
        mass: 1.478,
        radius: 2.043,
        luminosity: 7.049,
        kelvin: 6582.0,
        age_gyr: 1.9,
        outer_au: 2.8,
        min_inclination_deg: 20.0,
        limits: &[],
        belts: &[],
        planets: true,
    },
    Star {
        name: "Tau Ceti",
        ra_deg: 26.017_0,
        dec_deg: -15.937_5,
        light_years: 11.912,
        mass: 0.78,
        radius: 0.793,
        luminosity: 0.45,
        kelvin: 5320.0,
        age_gyr: 10.0,
        outer_au: 0.0,
        min_inclination_deg: 25.0,
        // ESPRESSO: 1.7 M⊕ within 100-day periods, 2-5 M⊕ in the habitable zone.
        limits: &[(0.0, 0.39, 1.7), (0.39, 1.3, 3.5)],
        belts: &[(6.0, 52.0)],
        planets: true,
    },
];

/// A star's companion on its real visual orbit around it (elements against the sky).
struct Companion {
    star: Star,
    host: &'static str,
    a_au: f64,
    e: f64,
    i_deg: f64,
    node_deg: f64,
    omega_deg: f64,
    period_yr: f64,
    /// Year of periastron.
    periastron_yr: f64,
}

const COMPANIONS: &[Companion] = &[
    // Kervella et al. 2016.
    Companion {
        star: Star {
            name: "Alpha Centauri B",
            mass: 0.9092,
            radius: 0.8591,
            luminosity: 0.4981,
            kelvin: 5260.0,
            outer_au: 2.5,
            ..STARS[0]
        },
        host: "Alpha Centauri A",
        a_au: 23.3,
        e: 0.5208,
        i_deg: 79.32,
        node_deg: 205.064,
        omega_deg: 232.006,
        period_yr: 79.929,
        periastron_yr: 1955.604,
    },
    // Bond et al. 2017: a white dwarf.
    Companion {
        star: Star {
            name: "Sirius B",
            mass: 1.018,
            radius: 0.008_098,
            luminosity: 0.024_48,
            kelvin: 25_369.0,
            planets: false,
            ..STARS[4]
        },
        host: "Sirius",
        a_au: 19.78,
        e: 0.591_42,
        i_deg: 136.336,
        node_deg: 45.4,
        omega_deg: 149.161,
        period_yr: 50.1284,
        periastron_yr: 1994.5715,
    },
    // Bond et al. 2015: a white dwarf.
    Companion {
        star: Star {
            name: "Procyon B",
            mass: 0.592,
            radius: 0.012_34,
            luminosity: 0.000_49,
            kelvin: 7740.0,
            planets: false,
            ..STARS[6]
        },
        host: "Procyon",
        a_au: 15.137,
        e: 0.397_85,
        i_deg: 31.408,
        node_deg: 100.683,
        omega_deg: 89.23,
        period_yr: 40.84,
        periastron_yr: 1968.076,
    },
];

fn star_body(s: &Star) -> CelestialBody {
    CelestialBody {
        name: s.name.into(),
        kind: BodyKind::Star,
        mu: SUN_MU * s.mass,
        radius_m: SUN_RADIUS * s.radius,
        luminosity_w: SUN_LUMINOSITY * s.luminosity,
        color: star_color(s.kelvin),
        look: BodyLook::None,
        ..Default::default()
    }
}

/// The stars nearest the Sun at their true distances and directions. Each is placed on a circle
/// around the Sun through its position that takes millions of years to go around: it stands
/// still for any flight. Alpha Centauri B, Sirius B and Procyon B circle their primaries on their
/// real orbits.
pub fn stars() -> Vec<CelestialBody> {
    let mut out = Vec::new();
    for s in STARS {
        let (lon, lat) = ecliptic(s.ra_deg, s.dec_deg);
        out.push(CelestialBody {
            parent: Some("Sun".into()),
            // A circle tipped upright (inclination 90°) through ecliptic longitude `lon`: the
            // star sits `lat` along it from the node.
            orbit: Orbit {
                semi_major_axis_m: s.light_years * LIGHT_YEAR,
                eccentricity: 0.0,
                inclination_deg: 90.0,
                ascending_node_deg: lon,
                arg_periapsis_deg: 0.0,
                mean_anomaly_deg: lat.rem_euclid(360.0),
            },
            ..star_body(s)
        });
        for c in COMPANIONS.iter().filter(|c| c.host == s.name) {
            let (i, node, omega) =
                sky_to_ecliptic(s.ra_deg, s.dec_deg, c.i_deg, c.node_deg, c.omega_deg);
            // Mean anomaly at J2000 from the year of periastron.
            let m = 360.0 * (2000.0 - c.periastron_yr) / c.period_yr;
            out.push(CelestialBody {
                parent: Some(c.host.into()),
                orbit: Orbit {
                    semi_major_axis_m: c.a_au * AU,
                    eccentricity: c.e,
                    inclination_deg: i,
                    ascending_node_deg: node,
                    arg_periapsis_deg: omega,
                    mean_anomaly_deg: m.rem_euclid(360.0),
                },
                ..star_body(&c.star)
            });
        }
    }
    out
}

/// Orbital elements against the sky at a star (inclination, node from north through east,
/// argument of periastron; i < 90° turns counterclockwise on the sky) as J2000 ecliptic
/// elements, degrees.
pub fn sky_to_ecliptic(
    ra_deg: f64,
    dec_deg: f64,
    i_deg: f64,
    node_deg: f64,
    omega_deg: f64,
) -> (f64, f64, f64) {
    let (sa, ca) = ra_deg.to_radians().sin_cos();
    let (sd, cd) = dec_deg.to_radians().sin_cos();
    let (se, ce) = OBLIQUITY_DEG.to_radians().sin_cos();
    let to_ecl = |v: DVec3| DVec3::new(v.x, v.y * ce + v.z * se, -v.y * se + v.z * ce);
    // At the star (equatorial): away from us, north and east on the sky.
    let away = to_ecl(DVec3::new(cd * ca, cd * sa, sd));
    let north = to_ecl(DVec3::new(-sd * ca, -sd * sa, cd));
    let east = to_ecl(DVec3::new(-sa, ca, 0.0));
    // The orbit's periastron and its 90°-ahead direction in the (north, east, away) frame,
    // as the Thiele-Innes constants have them.
    let (so, co) = node_deg.to_radians().sin_cos();
    let (si, ci) = i_deg.to_radians().sin_cos();
    let (sw, cw) = omega_deg.to_radians().sin_cos();
    let p = DVec3::new(co * cw - so * sw * ci, so * cw + co * sw * ci, sw * si);
    let q = DVec3::new(-co * sw - so * cw * ci, -so * sw + co * cw * ci, cw * si);
    let world = |v: DVec3| north * v.x + east * v.y + away * v.z;
    elements_of(world(p), world(q))
}

/// Inclination, node and argument of periapsis (degrees) of an orbit whose periapsis points
/// along `p` and moves toward `q` (ecliptic, z north).
fn elements_of(p: DVec3, q: DVec3) -> (f64, f64, f64) {
    let p = p.normalize();
    let h = p.cross(q).normalize();
    let i = h.z.clamp(-1.0, 1.0).acos();
    let node = if h.x.abs() + h.y.abs() < 1e-12 {
        DVec3::X
    } else {
        DVec3::new(-h.y, h.x, 0.0).normalize()
    };
    let big_omega = node.y.atan2(node.x);
    let w = h.dot(node.cross(p)).atan2(node.dot(p));
    (
        i.to_degrees(),
        big_omega.to_degrees().rem_euclid(360.0),
        w.to_degrees().rem_euclid(360.0),
    )
}

/// SplitMix64: small, fast and the same everywhere.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// A whole number below `n`.
    fn below(&mut self, n: usize) -> usize {
        ((self.unit() * n as f64) as usize).min(n.saturating_sub(1))
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    /// Log-uniform in [lo, hi].
    fn log_range(&mut self, lo: f64, hi: f64) -> f64 {
        (lo.ln() + (hi.ln() - lo.ln()) * self.unit()).exp()
    }
    fn normal(&mut self) -> f64 {
        let u = self.unit().max(1e-12);
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * self.unit()).cos()
    }
    fn rayleigh(&mut self, sigma: f64) -> f64 {
        sigma * (-2.0 * self.unit().max(1e-12).ln()).sqrt()
    }
}

/// FNV-1a.
fn hash(s: &str, salt: u64) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64 ^ salt, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// A planet of a system being built.
#[derive(Debug, Clone)]
struct Slot {
    letter: char,
    /// True mass, Earth masses.
    mass: f64,
    a_au: f64,
    e: f64,
    omega_deg: f64,
    /// Against the sky, degrees.
    sky: (f64, f64),
}

/// Spacing of two neighbours in mutual Hill radii.
fn hill_spacing(a1: f64, m1: f64, a2: f64, m2: f64, star_earths: f64) -> f64 {
    let rh = ((m1 + m2) / (3.0 * star_earths)).cbrt() * (a1 + a2) * 0.5;
    (a2 - a1).abs() / rh.max(1e-12)
}

/// The planets of every nearby star with planets: real ones (and `include_candidates`'
/// unconfirmed ones), then seeded ones filling each system.
pub fn planets(include_candidates: bool) -> Vec<CelestialBody> {
    let mut out = Vec::new();
    for star in STARS
        .iter()
        .chain(COMPANIONS.iter().map(|c| &c.star))
        .filter(|s| s.planets)
    {
        out.extend(system_of(star, include_candidates));
    }
    out
}

fn system_of(star: &Star, include_candidates: bool) -> Vec<CelestialBody> {
    let seed = hash(star.name, GENERATOR_VERSION);
    let mut layout = Rng::new(seed ^ 0x1a7);
    let star_earths = star.mass * SUN_MASS_KG / EARTH_MASS_KG;
    // The system's plane against the sky (a measured one where a planet has it).
    let known: Vec<&Known> = KNOWN
        .iter()
        .filter(|k| k.host == star.name && (include_candidates || !k.candidate))
        .collect();
    let plane = known.iter().find_map(|k| k.sky).unwrap_or_else(|| {
        (
            layout.range(star.min_inclination_deg.max(20.0), 90.0),
            layout.range(0.0, 360.0),
        )
    });
    let mut slots: Vec<Slot> = known
        .iter()
        .map(|k| {
            let sky = k.sky.unwrap_or(plane);
            Slot {
                letter: k.letter,
                mass: if k.true_mass {
                    k.mass
                } else {
                    k.mass / sky.0.to_radians().sin()
                },
                a_au: k.a_au,
                e: k.e,
                omega_deg: k.omega_deg,
                sky,
            }
        })
        .collect();
    let real = slots.len();

    // Where planets may go.
    let a_in = (10.0 * star.radius * SUN_RADIUS / AU).max(0.01);
    let a_out = if star.outer_au > 0.0 {
        star.outer_au
    } else {
        30.0 * star.mass
    };
    let snow = 2.7 * star.luminosity.sqrt();
    let min_mass = |a: f64| {
        star.limits
            .iter()
            .filter(|l| a >= l.0 && a < l.1)
            .map(|l| l.2)
            .fold(f64::INFINITY, f64::min)
    };
    let in_belt = |a: f64| star.belts.iter().any(|b| a > b.0 * 0.9 && a < b.1 * 1.1);
    let fits = |a: f64, m: f64, slots: &[Slot]| {
        a > a_in
            && a < a_out
            && !in_belt(a)
            && m * plane.0.to_radians().sin() <= min_mass(a)
            && slots.iter().all(|s| {
                let lo = a.min(s.a_au) / a.max(s.a_au);
                hill_spacing(s.a_au, s.mass, a, m, star_earths) >= 10.0 && lo < 0.885
            })
    };

    let place = |slots: &mut Vec<Slot>, layout: &mut Rng, a: f64, mass: f64| {
        let limit = min_mass(a) / plane.0.to_radians().sin();
        let mass = if mass > limit { limit * 0.8 } else { mass };
        let ok = mass >= 0.03 && fits(a, mass, slots);
        if ok {
            slots.push(Slot {
                letter: ' ',
                mass,
                a_au: a,
                e: layout
                    .rayleigh(if mass > 10.0 { 0.15 } else { 0.03 })
                    .min(0.4),
                omega_deg: layout.range(0.0, 360.0),
                sky: plane,
            });
        }
        ok.then_some(mass)
    };

    // The inner chain (Kepler's compact multis): a few planets of similar mass from an inner
    // period of 2-15 days outward, 10-60 mutual Hill radii apart (log-normal around 20). M
    // dwarfs hold more of them (Dressing & Charbonneau 2015: about 2.5 per star inside 200
    // days); hotter stars fewer.
    let inner_target = if star.mass < 0.6 {
        3 + layout.below(3)
    } else if star.mass < 1.5 {
        2 + layout.below(3)
    } else {
        1 + layout.below(3)
    };
    let inner_real = slots.iter().filter(|s| s.a_au < snow).count();
    let period_days = layout.log_range(3.0, 25.0);
    let mut a = (period_days / 365.25).powf(2.0 / 3.0) * star.mass.cbrt();
    let mut m = layout.log_range(0.3, 6.0) * star.mass.sqrt().clamp(0.4, 1.2);
    let mut added = 0;
    let mut tries = 0;
    while inner_real + added < inner_target && a < snow.min(a_out) && tries < 60 {
        tries += 1;
        let mut placed = m;
        if let Some(mass) = place(&mut slots, &mut layout, a, m) {
            added += 1;
            placed = mass;
            m = (mass * 10f64.powf(0.3 * layout.normal())).clamp(0.05, 8.0);
        }
        // Next orbit Δ mutual Hill radii out, past any planet in the way.
        let spacing = (20.0 * (0.35 * layout.normal()).exp()).clamp(10.0, 60.0);
        let k = ((placed + m) / (3.0 * star_earths)).cbrt();
        let mut next = (a * (2.0 + spacing * k) / (2.0 - spacing * k).max(0.05)).max(a * 1.13);
        for s in &slots {
            if s.a_au > a && s.a_au < next * 1.3 {
                let ks = ((s.mass + m) / (3.0 * star_earths)).cbrt();
                next = next.max(s.a_au * (2.0 + 10.5 * ks) / (2.0 - 10.5 * ks).max(0.05));
            }
        }
        a = next;
    }

    // A temperate planet in the habitable zone (Kopparapu 2014, 0.95-1.67 AU·√L) for about
    // half the stars: Bryson 2021 puts η⊕ between 0.37 and 0.60 for Sun-like stars.
    let hz = (0.95 * star.luminosity.sqrt(), 1.67 * star.luminosity.sqrt());
    let in_hz = slots.iter().any(|s| s.a_au > hz.0 && s.a_au < hz.1);
    if !in_hz && layout.unit() < 0.5 {
        for _ in 0..12 {
            let a = layout.log_range(hz.0, hz.1);
            let m = layout.log_range(0.3, 4.0) * star.mass.sqrt().clamp(0.4, 1.2);
            if place(&mut slots, &mut layout, a, m).is_some() {
                break;
            }
        }
    }

    // Past the snow line: giants (about one Sun-like star in ten has one, M dwarfs few;
    // Fulton 2021) or ice worlds, spread log-uniformly out to the system's edge.
    let outer_target = if star.mass < 0.6 {
        layout.below(2)
    } else {
        1 + layout.below(2)
    };
    let outer_real = slots.iter().filter(|s| s.a_au >= snow).count();
    let mut giants = slots
        .iter()
        .filter(|s| s.mass > 10.0 && s.a_au >= snow)
        .count();
    let (lo, hi) = (snow.max(a_in).max(a), a_out);
    let mut added = 0;
    for _ in 0..40 {
        if outer_real + added >= outer_target || lo >= hi || slots.len() >= MAX_PLANETS {
            break;
        }
        let a = layout.log_range(lo, hi);
        let mass = if giants < 2 && layout.unit() < 0.25 * star.mass.min(1.5) {
            layout.log_range(15.0, 900.0)
        } else {
            layout.log_range(0.3, 15.0)
        };
        if let Some(mass) = place(&mut slots, &mut layout, a, mass) {
            added += 1;
            giants += usize::from(mass > 10.0);
        }
    }
    slots.sort_by(|x, y| x.a_au.total_cmp(&y.a_au));

    // Letters for the seeded planets: after every letter surveys used, in order outward.
    let taken: Vec<char> = KNOWN
        .iter()
        .filter(|k| k.host == star.name)
        .map(|k| k.letter)
        .chain(REFUTED.iter().filter(|r| r.0 == star.name).map(|r| r.1))
        .collect();
    let mut letter = 'b';
    for s in slots.iter_mut().filter(|s| s.letter == ' ') {
        while taken.contains(&letter) {
            letter = (letter as u8 + 1) as char;
        }
        s.letter = letter;
        letter = (letter as u8 + 1) as char;
    }
    debug_assert!(real <= slots.len());

    slots
        .iter()
        .map(|s| planet_body(star, s, seed, snow))
        .collect()
}

/// Radius (Earth radii) of a planet of `m` Earth masses: Chen & Kipping 2017, giants capped
/// at 1.1 Jupiter radii unless hot (their fit is to inflated hot Jupiters).
fn radius_earths(m: f64, t_eq: f64) -> f64 {
    let r = if m < NEPTUNIAN_M {
        1.008 * m.powf(0.279)
    } else if m < JOVIAN_M {
        1.008 * NEPTUNIAN_M.powf(0.279) * (m / NEPTUNIAN_M).powf(0.589)
    } else {
        1.008
            * NEPTUNIAN_M.powf(0.279)
            * (JOVIAN_M / NEPTUNIAN_M).powf(0.589)
            * (m / JOVIAN_M).powf(-0.044)
    };
    if t_eq < 500.0 { r.min(12.3) } else { r }
}

/// Equilibrium temperature (K) at `a` AU from a star of `luminosity` L☉, albedo `albedo`.
fn equilibrium_k(luminosity: f64, a_au: f64, albedo: f64) -> f64 {
    278.0 * luminosity.powf(0.25) * a_au.powf(-0.5) * (1.0 - albedo).powf(0.25)
}

/// Mean surface temperature (K) under `pressure_bar` of air over an equilibrium temperature
/// `t_eq`: a grey greenhouse, T⁴ = T_eq⁴ (1 + ¾τ), with the optical depth τ = 0.84 p^1.1 fitted
/// to Mars (none), Earth (288 K over 255 K) and Venus (737 K over 232 K).
fn greenhouse_k(t_eq: f64, pressure_bar: f64) -> f64 {
    t_eq * (1.0 + 0.75 * 0.84 * pressure_bar.powf(1.1)).powf(0.25)
}

fn planet_body(star: &Star, s: &Slot, system_seed: u64, snow: f64) -> CelestialBody {
    let name = format!("{} {}", star.name, s.letter);
    let seed = hash(&name, system_seed);
    let mut rng = Rng::new(seed);
    let t0 = equilibrium_k(star.luminosity, s.a_au, 0.3);
    let r_earths = radius_earths(s.mass, t0) * (1.0 + 0.04 * rng.normal()).clamp(0.9, 1.1);
    let radius_m = r_earths * EARTH_RADIUS;
    let mu = s.mass * EARTH_MU;
    let mass_kg = s.mass * EARTH_MASS_KG;
    let star_kg = star.mass * SUN_MASS_KG;
    // Tidal locking time (Gladman et al. 1996 as Wikipedia gives it), rock rigidity.
    let a_m = s.a_au * AU;
    let lock_yr = 6.0e10 * a_m.powi(6) * radius_m * 3.0e10 / (mass_kg * star_kg * star_kg);
    let locked = lock_yr < star.age_gyr * 1e9;
    let period_s = std::f64::consts::TAU * (a_m.powi(3) / (G * (star_kg + mass_kg))).sqrt();

    // Mutual inclination of a degree or two off the system's plane.
    let tilt = rng.rayleigh(1.5);
    let turn = rng.range(0.0, 360.0);
    let sky = (
        s.sky.0 + tilt * turn.to_radians().cos(),
        s.sky.1 + tilt * turn.to_radians().sin() / s.sky.0.to_radians().sin().max(0.2),
    );
    let (i, node, omega) = sky_to_ecliptic(star.ra_deg, star.dec_deg, sky.0, sky.1, s.omega_deg);
    let orbit = Orbit {
        semi_major_axis_m: a_m,
        eccentricity: s.e,
        inclination_deg: i,
        ascending_node_deg: node,
        arg_periapsis_deg: omega,
        mean_anomaly_deg: rng.range(0.0, 360.0),
    };

    let mut body = CelestialBody {
        name,
        kind: BodyKind::Planet,
        mu,
        radius_m,
        parent: Some(star.name.into()),
        orbit,
        tidally_locked: locked,
        rotation_period_s: if locked {
            period_s
        } else {
            rng.range(9.0, 40.0) * 3600.0
        },
        axial_tilt_deg: if locked { 0.0 } else { rng.range(0.0, 35.0) },
        rotation_at_epoch_deg: rng.range(0.0, 360.0),
        ..Default::default()
    };

    // Gas and ice giants.
    if s.mass >= 10.0 || (s.mass >= NEPTUNIAN_M && (s.a_au > snow || rng.unit() < 0.5)) {
        let t = equilibrium_k(star.luminosity, s.a_au, 0.35);
        giant_look(&mut body, s.mass, t, &mut rng);
        return body;
    }

    // Rock: does it keep air? (Insolation against escape speed, M dwarfs' flares count 10×.)
    let v_esc = (2.0 * mu / radius_m).sqrt() / 1000.0;
    let insolation = star.luminosity / (s.a_au * s.a_au);
    let shoreline = (v_esc / 6.0).powi(4) * if star.mass < 0.5 { 0.1 } else { 1.0 };
    let margin = insolation / shoreline;
    let pressure = if margin > 2.0 {
        0.0
    } else if margin > 0.5 {
        rng.log_range(0.006, 0.1)
    } else {
        rng.log_range(0.5, 2.5)
    };
    // Inside the habitable zone the carbonate-silicate cycle (Walker 1981; Kasting 1993) piles
    // up carbon dioxide until the planet thaws (up to 2.5 bar here, beyond which the sky
    // turns to a white veil): most planets there with real air come out temperate.
    let ts = greenhouse_k(t0, pressure);
    let pressure = if pressure >= 0.5 && (200.0..273.0).contains(&ts) && rng.unit() < 0.7 {
        let target = rng.range(278.0, 300.0) / t0;
        ((target.powi(4) - 1.0) / 0.63)
            .powf(1.0 / 1.1)
            .clamp(pressure, 2.5)
    } else {
        pressure
    };
    let day_side = if locked { 1.28 } else { 1.0 };
    let t_day = t0 * day_side;
    let ts = greenhouse_k(t0, pressure);
    let class = if t_day > 1000.0 {
        WorldClass::Lava
    } else if pressure == 0.0 {
        WorldClass::Airless
    } else if ts > 340.0 {
        WorldClass::Desert
    } else if ts > 273.0 {
        if pressure < 0.1 {
            WorldClass::ColdDesert
        } else if rng.unit() < 0.5 {
            WorldClass::Ocean
        } else {
            WorldClass::Terra
        }
    } else if ts > 200.0 {
        if rng.unit() < 0.6 {
            WorldClass::ColdDesert
        } else {
            WorldClass::Ice
        }
    } else if pressure > 0.5 && s.mass < 1.5 && rng.unit() < 0.4 {
        WorldClass::Hazy
    } else {
        WorldClass::Ice
    };
    // A hot desert with real air may be a Venus under its clouds.
    let pressure = if class == WorldClass::Desert && pressure > 0.5 && rng.unit() < 0.5 {
        rng.log_range(10.0, 90.0)
    } else {
        pressure
    };
    let albedo = match class {
        WorldClass::Lava => 0.05,
        WorldClass::Airless => 0.1,
        WorldClass::Desert => {
            if pressure > 10.0 {
                0.75
            } else {
                0.25
            }
        }
        WorldClass::Terra | WorldClass::Ocean => 0.3,
        WorldClass::ColdDesert => 0.25,
        WorldClass::Ice => 0.6,
        WorldClass::Hazy => 0.3,
    };
    let temperature_k = greenhouse_k(equilibrium_k(star.luminosity, s.a_au, albedo), pressure);
    let recipe = SurfaceRecipe {
        class,
        seed,
        sea_fraction: match class {
            WorldClass::Ocean => rng.range(0.75, 0.95),
            WorldClass::Terra => rng.range(0.3, 0.7),
            WorldClass::Ice => rng.range(0.2, 0.7),
            WorldClass::Hazy => rng.range(0.05, 0.2),
            _ => 0.0,
        },
        crater_density: if pressure == 0.0 {
            rng.range(0.8, 1.0)
        } else if pressure < 0.1 {
            rng.range(0.4, 0.6)
        } else if class == WorldClass::Ocean {
            0.05
        } else {
            0.1
        },
        pressure_bar: pressure,
        cloud_cover: match class {
            WorldClass::Terra | WorldClass::Ocean => rng.range(0.4, 0.7) as f32,
            WorldClass::Desert if pressure > 10.0 => 1.0,
            WorldClass::Hazy => 1.0,
            WorldClass::Desert | WorldClass::ColdDesert => rng.range(0.0, 0.1) as f32,
            _ => 0.0,
        },
        temperature_k,
        star_kelvin: star.kelvin,
    };
    rock_look(&mut body, &recipe, &mut rng);
    body.surface = Some(recipe);
    body
}

/// Far-view look, sky and air of a rocky planet.
fn rock_look(body: &mut CelestialBody, r: &SurfaceRecipe, rng: &mut Rng) {
    let jitter = |rng: &mut Rng, c: [f32; 3]| {
        let k = rng.range(0.85, 1.15) as f32;
        c.map(|v| (v * k).min(1.0))
    };
    let (look, color) = match r.class {
        WorldClass::Airless => {
            let brown = rng.unit() as f32;
            (
                BodyLook::Rocky,
                jitter(
                    rng,
                    [0.36 + 0.06 * brown, 0.34 + 0.0 * brown, 0.32 - 0.06 * brown],
                ),
            )
        }
        WorldClass::Lava => (BodyLook::Rocky, [0.09, 0.075, 0.07]),
        WorldClass::Desert if r.pressure_bar > 10.0 => {
            (BodyLook::Clouds, jitter(rng, [0.85, 0.74, 0.52]))
        }
        WorldClass::Desert => (BodyLook::Rocky, jitter(rng, [0.62, 0.42, 0.26])),
        WorldClass::ColdDesert => (BodyLook::Rocky, jitter(rng, [0.58, 0.44, 0.36])),
        WorldClass::Terra | WorldClass::Ocean => (BodyLook::Earthlike, [0.3, 0.45, 0.7]),
        WorldClass::Ice => (BodyLook::Rocky, jitter(rng, [0.84, 0.87, 0.92])),
        WorldClass::Hazy => (BodyLook::Clouds, jitter(rng, [0.78, 0.52, 0.24])),
    };
    body.look = look;
    body.color = color;
    if r.pressure_bar <= 0.0 {
        body.air = Some(BodyAir::NONE);
        return;
    }
    // Air: N₂/O₂ (29 u) on temperate and icy worlds, CO₂ (44 u) on deserts; H = kT/(m g).
    let molar = match r.class {
        WorldClass::Desert | WorldClass::ColdDesert => 0.044,
        _ => 0.029,
    };
    let g = body.mu / (body.radius_m * body.radius_m);
    let t = r.temperature_k.max(40.0);
    let scale_height = 8.314 * t / (molar * g);
    let density = r.pressure_bar * 1.0e5 * molar / (8.314 * t);
    body.air = Some(BodyAir {
        surface_density: density as f32,
        scale_height_m: scale_height as f32,
        top_m: (scale_height * 12.0) as f32,
        temperature_c: (t - 273.15) as f32,
    });
    // The sky: Earth's Rayleigh scattering per molecule (2.4 times more for carbon dioxide;
    // Sneep & Ubachs 2005) at the surface's number density; ozone only where life made oxygen.
    // Dry worlds carry Mars' dust and hazy ones a thicker orange smog, as much as the winds
    // keep up whatever the pressure.
    let mars = BodyAtmosphere::mars();
    let mut sky = BodyAtmosphere::earth();
    sky.density_scale = (r.pressure_bar * 288.0 / t).clamp(0.005, 10.0) as f32;
    if molar > 0.04 {
        sky.rayleigh_scattering = sky.rayleigh_scattering.map(|v| v * 2.4);
    }
    if !matches!(r.class, WorldClass::Terra | WorldClass::Ocean) {
        sky.ozone_absorption = [0.0; 3];
    }
    let dust = match r.class {
        WorldClass::Desert | WorldClass::ColdDesert => rng.range(0.4, 1.2) as f32,
        WorldClass::Hazy => 4.0,
        _ => 0.0,
    };
    if dust > 0.0 {
        let k = dust / sky.density_scale;
        sky.mie_scattering = mars.mie_scattering.map(|v| v * k);
        sky.mie_absorption = mars.mie_absorption.map(|v| v * k);
        sky.mie_g = mars.mie_g;
        sky.mie_g_offset = mars.mie_g_offset;
        sky.mie_forward_peak = mars.mie_forward_peak;
        sky.mie_scale_height_m = (scale_height * 1.4) as f32;
    }
    sky.rayleigh_scale_height_m = scale_height as f32;
    sky.thickness_m = (scale_height * 12.0) as f32;
    body.atmosphere = Some(sky);
    body.haze = Some(match r.class {
        WorldClass::Terra | WorldClass::Ocean | WorldClass::Ice => BodyHaze {
            scale_height_m: scale_height,
            ..BodyHaze::default()
        },
        _ => BodyHaze {
            optical_depth: if r.pressure_bar > 10.0 {
                0.6
            } else {
                0.05 + 0.12 * dust
            },
            extinction: [0.9, 1.0, 1.15],
            albedo: [color[0].max(0.6), color[1].max(0.5), color[2].max(0.4)],
            scale_height_m: scale_height,
            forward: 0.6,
        },
    });
}

/// Bands of a gas or ice giant by temperature (Sudarsky et al. 2000's cloud classes).
fn giant_look(body: &mut CelestialBody, mass: f64, t_eq: f64, rng: &mut Rng) {
    body.kind = BodyKind::Planet;
    let colors: Vec<[f32; 3]> = if mass < JOVIAN_M && t_eq < 350.0 {
        // Ice giants and cold sub-Neptunes: methane blue-green.
        if rng.unit() < 0.5 {
            vec![[0.55, 0.78, 0.82], [0.5, 0.72, 0.78], [0.6, 0.82, 0.85]]
        } else {
            vec![[0.25, 0.4, 0.82], [0.3, 0.46, 0.86], [0.22, 0.36, 0.76]]
        }
    } else if t_eq < 150.0 {
        // Ammonia clouds: Jupiter's beige and brown bands.
        vec![
            [0.85, 0.75, 0.6],
            [0.62, 0.47, 0.35],
            [0.92, 0.88, 0.78],
            [0.7, 0.55, 0.42],
        ]
    } else if t_eq < 350.0 {
        // Water clouds: bright white.
        vec![[0.93, 0.93, 0.91], [0.82, 0.84, 0.86], [0.88, 0.88, 0.9]]
    } else if t_eq < 900.0 {
        // Clear of clouds: deep blue.
        vec![[0.2, 0.3, 0.62], [0.25, 0.36, 0.68], [0.18, 0.27, 0.56]]
    } else if t_eq < 1500.0 {
        // Alkali metals absorb: dark.
        vec![[0.16, 0.13, 0.12], [0.21, 0.17, 0.15], [0.13, 0.11, 0.1]]
    } else {
        // Silicate clouds: grey and bright.
        vec![[0.62, 0.6, 0.6], [0.5, 0.49, 0.5], [0.7, 0.68, 0.66]]
    };
    body.color = colors[0];
    body.look = BodyLook::Banded {
        colors,
        bands: rng.range(5.0, 14.0) as f32,
        storm: (rng.unit() < 0.3).then_some([0.75, 0.42, 0.3]),
    };
    body.flattening = if mass >= JOVIAN_M {
        rng.range(0.03, 0.08)
    } else {
        rng.range(0.01, 0.03)
    };
    body.haze = Some(BodyHaze {
        optical_depth: 0.3,
        extinction: [0.6, 0.8, 1.0],
        albedo: [0.9, 0.9, 0.95],
        scale_height_m: 30_000.0,
        forward: 0.4,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(name: &str) -> Vec<CelestialBody> {
        planets(false)
            .into_iter()
            .filter(|b| b.parent.as_deref() == Some(name))
            .collect()
    }

    #[test]
    fn the_same_seed_builds_the_same_systems() {
        assert_eq!(planets(false), planets(false));
        assert_eq!(planets(true), planets(true));
    }

    #[test]
    fn real_planets_keep_their_orbits() {
        let proxima = system("Proxima Centauri");
        let b = proxima
            .iter()
            .find(|p| p.name == "Proxima Centauri b")
            .unwrap();
        assert!((b.orbit.semi_major_axis_m / AU - 0.04848).abs() < 1e-9);
        let d = proxima
            .iter()
            .find(|p| p.name == "Proxima Centauri d")
            .unwrap();
        assert!((d.orbit.semi_major_axis_m / AU - 0.02881).abs() < 1e-9);
        // Its true mass is at least the minimum the radial velocities give.
        assert!(b.mu / EARTH_MU >= 1.055);
        let barnard = system("Barnard's Star");
        for l in ['b', 'c', 'd', 'e'] {
            assert!(
                barnard
                    .iter()
                    .any(|p| p.name == format!("Barnard's Star {l}"))
            );
        }
        // Epsilon Eridani b: a cold Jupiter at 3.53 AU, its orbit 41° from face-on.
        let eps = system("Epsilon Eridani");
        let b = eps.iter().find(|p| p.name == "Epsilon Eridani b").unwrap();
        assert!((b.mu / EARTH_MU - 311.5).abs() < 1e-6);
        assert!(matches!(b.look, BodyLook::Banded { .. }));
        // Candidates only when asked for, refuted ones never.
        assert!(!planets(false).iter().any(|p| p.name == "Tau Ceti g"));
        assert!(planets(true).iter().any(|p| p.name == "Tau Ceti g"));
        assert!(!planets(true).iter().any(|p| p.name == "Tau Ceti e"));
        assert!(!planets(true).iter().any(|p| p.name == "Alpha Centauri B b"));
    }

    #[test]
    fn seeded_planets_are_spaced_stably_and_stay_inside_their_limits() {
        for star in STARS.iter().chain(COMPANIONS.iter().map(|c| &c.star)) {
            let sys = system(star.name);
            if !star.planets {
                assert!(sys.is_empty());
                continue;
            }
            assert!(!sys.is_empty(), "{} has no planets", star.name);
            assert!(sys.len() <= MAX_PLANETS);
            let star_earths = star.mass * SUN_MASS_KG / EARTH_MASS_KG;
            let a = |b: &CelestialBody| b.orbit.semi_major_axis_m / AU;
            let m = |b: &CelestialBody| b.mu / EARTH_MU;
            for w in sys.windows(2) {
                assert!(a(&w[0]) < a(&w[1]), "{} out of order", star.name);
                let real = KNOWN.iter().any(|k| {
                    w.iter()
                        .any(|b| b.name == format!("{} {}", k.host, k.letter))
                });
                let spacing = hill_spacing(a(&w[0]), m(&w[0]), a(&w[1]), m(&w[1]), star_earths);
                // Real neighbours are as found (Barnard's are tight); seeded ones keep 10.
                assert!(
                    real || spacing >= 10.0,
                    "{} {} {spacing}",
                    w[0].name,
                    w[1].name
                );
            }
            for b in &sys {
                if star.outer_au > 0.0 {
                    assert!(a(b) < star.outer_au, "{} beyond the binary limit", b.name);
                }
                assert!(
                    !star.belts.iter().any(|x| a(b) > x.0 && a(b) < x.1),
                    "{} in a belt",
                    b.name
                );
            }
        }
        // Nothing heavier than 0.57 M⊕ (m sin i) in Barnard's Star's habitable zone.
        for b in system("Barnard's Star") {
            let a = b.orbit.semi_major_axis_m / AU;
            if (0.05..0.13).contains(&a) {
                assert!(b.mu / EARTH_MU * 0.5 <= 0.57 * 1.01, "{}", b.name);
            }
        }
    }

    #[test]
    fn classes_follow_temperature_and_air() {
        let all = planets(false);
        let rocky: Vec<_> = all
            .iter()
            .filter_map(|b| b.surface.map(|s| (b, s)))
            .collect();
        assert!(rocky.len() >= 10, "{}", rocky.len());
        for (b, s) in &rocky {
            match s.class {
                WorldClass::Airless => assert_eq!(s.pressure_bar, 0.0, "{}", b.name),
                WorldClass::Terra | WorldClass::Ocean => {
                    assert!(
                        (240.0..340.0).contains(&s.temperature_k),
                        "{} {}",
                        b.name,
                        s.temperature_k
                    );
                    assert!(s.sea_fraction > 0.2);
                    assert!(b.atmosphere.is_some());
                }
                WorldClass::Ice | WorldClass::Hazy => {
                    assert!(s.temperature_k < 260.0, "{}", b.name)
                }
                _ => {}
            }
        }
        // Proxima b is so close to its flaring star that it is locked to it.
        let b = all.iter().find(|p| p.name == "Proxima Centauri b").unwrap();
        assert!(b.tidally_locked);
        // A few kinds of world to land on.
        let kinds: std::collections::HashSet<_> = rocky.iter().map(|(_, s)| s.class).collect();
        assert!(kinds.len() >= 4, "{kinds:?}");
    }

    #[test]
    fn sky_elements_turn_into_ecliptic_ones() {
        // A face-on orbit turning counterclockwise on the sky has its pole pointing at us.
        let (ra, dec) = (101.287_2, -16.716_1);
        let (i, node, _) = sky_to_ecliptic(ra, dec, 0.0, 0.0, 0.0);
        let (lon, lat) = ecliptic(ra, dec);
        let pole = DVec3::new(
            i.to_radians().sin() * node.to_radians().sin(),
            -i.to_radians().sin() * node.to_radians().cos(),
            i.to_radians().cos(),
        );
        let toward_us = -DVec3::new(
            lat.to_radians().cos() * lon.to_radians().cos(),
            lat.to_radians().cos() * lon.to_radians().sin(),
            lat.to_radians().sin(),
        );
        assert!(
            pole.angle_between(toward_us) < 1e-9,
            "{}",
            pole.angle_between(toward_us)
        );
        // An edge-on orbit's pole lies in the sky plane.
        let (i, node, _) = sky_to_ecliptic(ra, dec, 90.0, 30.0, 0.0);
        let pole = DVec3::new(
            i.to_radians().sin() * node.to_radians().sin(),
            -i.to_radians().sin() * node.to_radians().cos(),
            i.to_radians().cos(),
        );
        assert!(pole.dot(toward_us).abs() < 1e-9);
    }

    #[test]
    fn white_dwarfs_circle_their_stars() {
        let stars = stars();
        let b = stars.iter().find(|s| s.name == "Sirius B").unwrap();
        assert_eq!(b.parent.as_deref(), Some("Sirius"));
        assert!((b.radius_m - 5_634_000.0).abs() < 10_000.0);
        let sirius = stars.iter().find(|s| s.name == "Sirius").unwrap();
        let years = b.orbit.period(sirius.mu + b.mu) / (365.25 * crate::orbit::DAY);
        assert!((years - 50.13).abs() < 0.5, "{years}");
        let procyon_b = stars.iter().find(|s| s.name == "Procyon B").unwrap();
        let procyon = stars.iter().find(|s| s.name == "Procyon").unwrap();
        let years =
            procyon_b.orbit.period(procyon.mu + procyon_b.mu) / (365.25 * crate::orbit::DAY);
        assert!((years - 40.84).abs() < 0.5, "{years}");
    }
}
