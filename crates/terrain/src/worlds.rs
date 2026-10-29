//! Terrains for made-up planets of every kind, from a short recipe: the space module's planets
//! of other stars carry one, so a ship that comes close finds ground to land on that matches
//! what it saw from afar.
//!
//! Each kind starts from the closest world we know and turns its knobs: airless rock from the
//! Moon (craters, worn massifs, dark plains); lava worlds from Io and the predicted surfaces
//! of hot rocky exoplanets (a black crust over molten rock that shows through fissures, rifts
//! and pools where it is hot enough); hot and cold deserts from Mars (dust, dark rock, few
//! fresh craters under air, polar frost); temperate worlds from Earth (their own share of
//! ocean, plants colored by their star's light, no towns); frozen worlds as ice plains
//! with scattered craters; hazy ones from Titan (dark dune fields, black hydrocarbon lakes).

use serde::{Deserialize, Serialize};

use crate::landmarks::LandmarkField;
use crate::rocks::RockField;
use crate::shape::{Climate, PlanetShape};
use crate::{PlanetTerrain, TerrainPalette};

/// The kind of world a [`WorldRecipe`] makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum WorldKind {
    #[default]
    Airless,
    Lava,
    Desert,
    Terra,
    Ocean,
    ColdDesert,
    Ice,
    Hazy,
}

/// What a world is like, for [`PlanetTerrain::world`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldRecipe {
    pub kind: WorldKind,
    pub seed: u64,
    pub radius_m: f64,
    /// Share of the surface under sea (or frozen sea), 0..1.
    pub sea_fraction: f64,
    /// 0..1 how crowded its craters are.
    pub crater_density: f64,
    /// Surface pressure, bar.
    pub pressure_bar: f64,
    /// Mean surface temperature, kelvin.
    pub temperature_k: f64,
    /// Surface temperature of its star, kelvin (plant colors).
    pub star_kelvin: f64,
    /// The world's color seen from afar (linear): the ground takes its hue.
    pub color: [f32; 3],
    /// Luminance of exposed melt, cd/m² (lava worlds; 0 = no glow).
    pub lava_glow_nits: f32,
    /// Locked to its star (the star over planet-local +Z): warm and wet under the star, the
    /// night side frozen.
    pub locked: bool,
}

impl Default for WorldRecipe {
    fn default() -> Self {
        Self {
            kind: WorldKind::Airless,
            seed: 1,
            radius_m: 6_371_000.0,
            sea_fraction: 0.0,
            crater_density: 0.8,
            pressure_bar: 0.0,
            temperature_k: 250.0,
            star_kelvin: 5772.0,
            color: [0.36, 0.34, 0.32],
            lava_glow_nits: 0.0,
            locked: false,
        }
    }
}

/// `palette` with every color pulled toward `color`'s hue by `amount` (0..1), keeping each
/// color's own brightness.
fn tinted(mut palette: TerrainPalette, color: [f32; 3], amount: f32) -> TerrainPalette {
    let mean = (color[0] + color[1] + color[2]) / 3.0;
    let hue = color.map(|c| c / mean.max(1e-4));
    let tint = |c: [f32; 3]| {
        let m = (c[0] + c[1] + c[2]) / 3.0;
        [0, 1, 2].map(|i| c[i] + (m * hue[i] - c[i]) * amount)
    };
    for c in [
        &mut palette.lush,
        &mut palette.dry,
        &mut palette.soil,
        &mut palette.rock,
        &mut palette.sand,
        &mut palette.dark,
        &mut palette.mud,
    ] {
        *c = tint(*c);
    }
    palette
}

/// Forest and grass colors (linear) under a star of `kelvin`: Earth's green under the Sun,
/// olive under K stars, nearly black under M dwarfs, yellow to orange under F and A stars
/// (Kiang et al. 2007). Matches the far view's plants.
fn plants_under(kelvin: f64) -> ([f32; 3], [f32; 3]) {
    let stops: [(f64, [f32; 3]); 6] = [
        (3000.0, [0.026, 0.018, 0.026]),
        (3900.0, [0.032, 0.034, 0.018]),
        (5000.0, [0.060, 0.120, 0.022]),
        (5800.0, [0.085, 0.160, 0.035]),
        (6600.0, [0.150, 0.130, 0.020]),
        (8000.0, [0.190, 0.090, 0.018]),
    ];
    let lush = between(kelvin, &stops);
    // Dry grass: paler, toward straw; under cool stars a dark plum-brown, the dead leaves
    // keeping much of their dark pigment.
    let straw = between(
        kelvin,
        &[
            (3200.0, [0.06, 0.04, 0.045]),
            (3900.0, [0.13, 0.095, 0.06]),
            (5000.0, [0.16, 0.14, 0.07]),
        ],
    );
    let dry = [0, 1, 2].map(|c| lush[c] * 0.5 + straw[c]);
    (lush, dry)
}

/// Linear interpolation of colors between (kelvin, color) stops, clamped at both ends.
fn between(kelvin: f64, stops: &[(f64, [f32; 3])]) -> [f32; 3] {
    let k = kelvin.clamp(stops[0].0, stops[stops.len() - 1].0);
    let i = stops
        .iter()
        .rposition(|s| s.0 <= k)
        .unwrap_or(0)
        .min(stops.len() - 2);
    let t = ((k - stops[i].0) / (stops[i + 1].0 - stops[i].0).max(1e-9)) as f32;
    [0, 1, 2].map(|c| stops[i].1[c] + (stops[i + 1].1[c] - stops[i].1[c]) * t)
}

/// Microbial mats on the shores and wet lowlands of a world under a star of `kelvin`: their
/// color, and how thickly they cover (none under Sun-like stars, where plants took the
/// shores). Purple bacteria under M dwarfs, orange and red under K stars, yellow under F stars
/// (Coelho et al. 2024; engine-design/exoplanet-surface-research.md).
fn mats_under(kelvin: f64) -> ([f32; 3], f32) {
    let color = between(
        kelvin,
        &[
            (3200.0, [0.12, 0.035, 0.09]),
            (4200.0, [0.17, 0.06, 0.035]),
            (5200.0, [0.17, 0.11, 0.03]),
            (6600.0, [0.2, 0.15, 0.03]),
        ],
    );
    let sunlike =
        ((kelvin - 4800.0) / 500.0).clamp(0.0, 1.0) * ((6500.0 - kelvin) / 500.0).clamp(0.0, 1.0);
    (color, 0.8 * (1.0 - sunlike as f32))
}

impl PlanetTerrain {
    /// A planet made from `r`: its kind picks the closest preset, the rest tunes it.
    pub fn world(r: &WorldRecipe) -> Self {
        let seed = r.seed;
        let t_c = (r.temperature_k - 273.15) as f32;
        let air = r.pressure_bar > 0.05;
        let climate = |swing_up: f32, swing_down: f32, wetness: f32, circulation: f32| {
            let (equator_c, pole_c) = match (r.locked, air) {
                // The air carries some of the day side's heat around, but the night side
                // still freezes harder than a spinning world's poles.
                (true, true) => (t_c + swing_up * 1.3, t_c - swing_down * 1.5),
                // Bare rock keeps its heat where the star shines: √2 × the mean temperature
                // straight under it, the night side near the cold of space (Koll & Abbot 2015).
                (true, false) => (
                    r.temperature_k as f32 * std::f32::consts::SQRT_2 - 273.15,
                    (t_c - swing_down * 2.0).max(-230.0),
                ),
                (false, _) => (t_c + swing_up, t_c - swing_down),
            };
            Climate {
                equator_c,
                pole_c,
                lapse_c_per_m: if air { 0.0065 } else { 0.0 },
                wetness,
                circulation,
                substellar: r.locked,
            }
        };
        let airless_shape = |crater_density: f64| PlanetShape {
            crater_density,
            ..PlanetShape::moon(seed)
        };
        let mut t = match r.kind {
            WorldKind::Airless => Self {
                shape: PlanetShape {
                    maria: 0.15 + 0.3 * (seed % 7) as f64 / 6.0,
                    climate: climate(40.0, 120.0, 0.0, 0.0),
                    ..airless_shape(r.crater_density.clamp(0.3, 1.0))
                },
                palette: tinted(TerrainPalette::moon(), r.color, 0.8),
                ..Self::moon(seed)
            },
            WorldKind::Lava => Self {
                shape: PlanetShape {
                    // Resurfaced: few craters, broad young lava plains, low massifs.
                    maria: 0.6,
                    lava_basins: 1.0,
                    crater_density: 0.15,
                    crater_max_radius_m: 20_000.0,
                    mountain_height_m: 2_500.0,
                    worn: 0.3,
                    climate: climate(250.0, 400.0, 0.0, 0.0),
                    ..airless_shape(0.15)
                },
                palette: TerrainPalette {
                    lush: [0.045, 0.040, 0.037],
                    dry: [0.055, 0.048, 0.042],
                    soil: [0.04, 0.034, 0.03],
                    rock: [0.06, 0.054, 0.05],
                    sand: [0.05, 0.045, 0.04],
                    // Sulfur frost on the coolest ground.
                    snow: [0.55, 0.5, 0.22],
                    dark: [0.025, 0.022, 0.02],
                    mud: [0.03, 0.025, 0.022],
                    regolith: 0.2,
                    snow_below_c: -300.0,
                    lava_glow: r.lava_glow_nits,
                    ground_material: "rock".into(),
                    bare_material: "rock".into(),
                    ..TerrainPalette::moon()
                },
                rocks: RockField {
                    color: [0.05, 0.045, 0.04],
                    ..RockField::mars()
                },
                ..Self::moon(seed)
            },
            WorldKind::Desert | WorldKind::ColdDesert => {
                let air = (r.pressure_bar / 0.5).clamp(0.0, 1.0);
                Self {
                    shape: PlanetShape {
                        landforms: Default::default(),
                        crater_density: r.crater_density.clamp(0.05, 0.8),
                        crater_min_radius_m: 20.0 + 200.0 * air,
                        erosion: 0.3 * air,
                        worn: 0.6 + 0.3 * air,
                        // Frost caps where it is cold enough (the palette's frost line).
                        climate: if r.kind == WorldKind::Desert {
                            climate(30.0, 50.0, 0.0, 0.0)
                        } else {
                            climate(25.0, 45.0, 0.02, 0.0)
                        },
                        ..PlanetShape::mars(seed)
                    },
                    palette: TerrainPalette {
                        snow_below_c: if r.kind == WorldKind::Desert {
                            -300.0
                        } else {
                            -60.0
                        },
                        ..tinted(TerrainPalette::mars(), r.color, 0.9)
                    },
                    landmarks: LandmarkField::mars().tinted(r.color, 0.7),
                    ..Self::mars(seed)
                }
            }
            WorldKind::Terra | WorldKind::Ocean => {
                let (lush, dry) = plants_under(r.star_kelvin);
                let (mats, shore_mats) = mats_under(r.star_kelvin);
                let ocean = r.kind == WorldKind::Ocean;
                Self {
                    shape: PlanetShape {
                        ocean_fraction: r.sea_fraction.clamp(0.1, 0.97),
                        // Ocean worlds: island chains rather than continents.
                        continent_scale_m: if ocean { 1_400_000.0 } else { 3_200_000.0 },
                        climate: climate(15.0, 37.0, if ocean { 0.75 } else { 0.58 }, 1.0),
                        ..PlanetShape::earthlike(seed)
                    },
                    palette: TerrainPalette {
                        lush,
                        dry,
                        night_lights: 0.0,
                        mud: if shore_mats > 0.0 {
                            mats
                        } else {
                            TerrainPalette::earth().mud
                        },
                        shore_mats,
                        ..TerrainPalette::earth()
                    },
                    // No farmers have cleared the fields: stones and boulders lie in the
                    // grass, thickest on rocky patches and slopes.
                    rocks: RockField {
                        per_100m2: 8.0,
                        coverage: 0.35,
                        color: [0.17, 0.155, 0.14],
                        ..RockField::mars()
                    },
                    // Granite tors on the uplands, sandstone in the deserts.
                    landmarks: LandmarkField::terra(),
                    ..Self::earthlike(seed)
                }
            }
            WorldKind::Ice => Self {
                shape: PlanetShape {
                    // Ice plains: smooth, softly cratered, low ridged uplands.
                    crater_density: 0.25,
                    crater_max_radius_m: 30_000.0,
                    maria: 0.0,
                    lava_basins: 0.0,
                    mountain_height_m: 1_200.0,
                    plateau_height_m: 800.0,
                    worn: 0.7,
                    climate: climate(15.0, 40.0, 0.3, 0.0),
                    ..airless_shape(0.25)
                },
                palette: TerrainPalette {
                    lush: [0.78, 0.8, 0.84],
                    dry: [0.72, 0.74, 0.78],
                    soil: [0.55, 0.56, 0.6],
                    rock: [0.32, 0.35, 0.4],
                    sand: [0.7, 0.72, 0.76],
                    snow: [0.86, 0.89, 0.93],
                    dark: [0.42, 0.36, 0.33],
                    mud: [0.5, 0.5, 0.52],
                    regolith: 0.4,
                    snow_below_c: -2.0,
                    rock_slope: 0.35,
                    ground_material: "snow".into(),
                    bare_material: "ice".into(),
                    ..TerrainPalette::moon()
                },
                rocks: RockField::default(),
                landmarks: LandmarkField::ice(),
                ..Self::moon(seed)
            },
            WorldKind::Hazy => Self {
                shape: PlanetShape {
                    ocean_fraction: r.sea_fraction.clamp(0.03, 0.3),
                    mountain_height_m: 800.0,
                    plateau_height_m: 300.0,
                    erosion: 0.6,
                    climate: Climate {
                        // Methane rain carves channels; the land stays below every frost line.
                        equator_c: t_c + 2.0,
                        pole_c: t_c - 4.0,
                        lapse_c_per_m: 0.001,
                        wetness: 0.2,
                        circulation: 0.5,
                        substellar: r.locked,
                    },
                    rivers: true,
                    ..PlanetShape::earthlike(seed)
                },
                palette: TerrainPalette {
                    // Organic sand: the dunes are dark, the uplands paler orange-brown.
                    lush: [0.16, 0.1, 0.05],
                    dry: [0.2, 0.13, 0.06],
                    soil: [0.12, 0.08, 0.045],
                    rock: [0.22, 0.17, 0.12],
                    sand: [0.07, 0.05, 0.035],
                    snow: [0.6, 0.55, 0.45],
                    dark: [0.05, 0.04, 0.03],
                    ocean: [0.003, 0.003, 0.004],
                    mud: [0.06, 0.04, 0.025],
                    vegetation: false,
                    regolith: 0.0,
                    night_lights: 0.0,
                    snow_below_c: -300.0,
                    rock_slope: 0.3,
                    ground_material: "sand".into(),
                    bare_material: "dirt".into(),
                    lava_glow: 0.0,
                    shore_mats: 0.0,
                },
                landmarks: LandmarkField::default(),
                ..Self::earthlike(seed)
            },
        };
        t.shape.radius_m = r.radius_m;
        // Seas show from afar; up close the water module draws them where there is one.
        t.ocean_surface = t.shape.ocean;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::DVec3;

    #[test]
    fn every_kind_makes_a_terrain_of_its_own_size() {
        let kinds = [
            WorldKind::Airless,
            WorldKind::Lava,
            WorldKind::Desert,
            WorldKind::Terra,
            WorldKind::Ocean,
            WorldKind::ColdDesert,
            WorldKind::Ice,
            WorldKind::Hazy,
        ];
        for kind in kinds {
            let r = WorldRecipe {
                kind,
                radius_m: 5_000_000.0,
                sea_fraction: 0.7,
                temperature_k: 290.0,
                ..Default::default()
            };
            let t = PlanetTerrain::world(&r);
            assert_eq!(t.shape.radius_m, 5_000_000.0, "{kind:?}");
            let wet = matches!(kind, WorldKind::Terra | WorldKind::Ocean | WorldKind::Hazy);
            assert_eq!(t.shape.ocean, wet, "{kind:?}");
            assert_eq!(t.palette.night_lights, 0.0, "{kind:?}: nobody lives there");
            // The ground is solid where it is sampled.
            let s = t
                .shape
                .sample_with(DVec3::new(0.3, 0.5, 0.8).normalize(), 100.0, None);
            assert!(s.height.is_finite(), "{kind:?}");
        }
    }

    #[test]
    fn plants_darken_under_red_dwarfs() {
        let (sun, _) = plants_under(5772.0);
        let (red, _) = plants_under(3050.0);
        assert!(
            sun[1] > 0.15 && red.iter().all(|&c| c < 0.03),
            "{sun:?} {red:?}"
        );
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        assert!(luma(plants_under(6600.0).0) > luma(red));
    }

    #[test]
    fn red_dwarf_shores_carry_purple_mats_and_sunlike_ones_none() {
        let world = |star_kelvin| {
            PlanetTerrain::world(&WorldRecipe {
                kind: WorldKind::Terra,
                sea_fraction: 0.5,
                star_kelvin,
                ..Default::default()
            })
        };
        let red = world(3050.0);
        let sun = world(5772.0);
        let p = &red.palette;
        assert!(
            p.shore_mats > 0.5 && p.mud[2] > p.mud[1] && p.mud[0] > p.mud[1],
            "{p:?}"
        );
        assert_eq!(sun.palette.shore_mats, 0.0);
        assert_eq!(sun.palette.mud, TerrainPalette::earth().mud);
        // Dry grass under the red dwarf is a dark plum-brown, under the Sun straw.
        let (_, dry_red) = plants_under(3050.0);
        let (_, dry_sun) = plants_under(5772.0);
        assert!(
            dry_red[0] > dry_red[1]
                && dry_red.iter().sum::<f32>() < 0.5 * dry_sun.iter().sum::<f32>()
        );
        // Stones lie in their grass.
        assert!(red.rocks.per_100m2 > 0.0);
    }

    #[test]
    fn a_big_world_finds_its_rayed_craters_at_once() {
        // Small craters on a world half again Earth's size: the search scans the shell of cells
        // around the surface, not the whole cube (1.7 billion cells, a minute's stall when the
        // terrain was first drawn).
        let lava = PlanetTerrain::world(&WorldRecipe {
            kind: WorldKind::Lava,
            radius_m: 9_500_000.0,
            ..Default::default()
        });
        let started = std::time::Instant::now();
        let rayed = lava.shape.rayed_craters();
        let secs = started.elapsed().as_secs_f64();
        assert!(secs < 1.5, "{secs} s");
        assert!(!rayed.is_empty());
    }

    #[test]
    fn lava_glows_only_on_lava_worlds() {
        let lava = PlanetTerrain::world(&WorldRecipe {
            kind: WorldKind::Lava,
            temperature_k: 1500.0,
            lava_glow_nits: 5000.0,
            ..Default::default()
        });
        assert_eq!(lava.palette.lava_glow, 5000.0);
        let desert = PlanetTerrain::world(&WorldRecipe {
            kind: WorldKind::Desert,
            lava_glow_nits: 5000.0,
            ..Default::default()
        });
        assert_eq!(desert.palette.lava_glow, 0.0);
    }

    #[test]
    fn a_locked_world_is_warm_under_its_star_and_frozen_behind() {
        use engine_core::math::DVec3;
        let world = |locked| {
            PlanetTerrain::world(&WorldRecipe {
                kind: WorldKind::Terra,
                sea_fraction: 0.6,
                pressure_bar: 1.0,
                temperature_k: 285.0,
                locked,
                ..Default::default()
            })
        };
        // Sea-level climate only: the lapse rate would mix in the heights.
        let at = |t: &PlanetTerrain, d: DVec3| {
            let s = t.shape.sample(d, 0.0);
            s.temperature_c + t.shape.climate.lapse_c_per_m * s.height.max(0.0) as f32
        };
        let (under, behind, pole) = (DVec3::Z, -DVec3::Z, DVec3::Y);
        let locked = world(true);
        assert!(locked.shape.climate.substellar);
        assert!(at(&locked, under) > 20.0, "{}", at(&locked, under));
        assert!(at(&locked, behind) < -25.0, "{}", at(&locked, behind));
        assert!(at(&locked, pole) < at(&locked, under) - 25.0);
        // Spinning, the same world is as warm all round its equator (sea currents aside).
        let spinning = world(false);
        assert!((at(&spinning, under) - at(&spinning, behind)).abs() < 20.0);
        assert!(at(&spinning, pole) < -10.0);
    }
}
