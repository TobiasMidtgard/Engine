//! The interactive map: one continuous zoom from a flat surface map, out to a 3D globe, out to
//! the star system and out to the galaxy.
//!
//! The map draws [`MapData`]: a galaxy of star systems joined by hyperlanes, their planets
//! (procedural looks by [`PlanetKind`]), points of interest on the planets, where the player
//! is and the waypoint. It is plain JSON (`map.data`, `map.add_system`, `map.add_poi`), so a
//! game or an AI agent can build its universe at run time.
//!
//! `ui.show {"screen": "map", "zoom": 1.4}` opens it. Zoom runs from 0 to 4: 0-1 surface
//! (flat map, 1 = the whole world), 1-2 planet (globe), 2-3 star system, 3-4 galaxy.
//! Mouse wheel / triggers zoom, drag / right stick pans or turns the globe, click selects,
//! Q/E or LB/RB jump a level. The info card on the right sets waypoints and starts travel
//! (`map.travel:<id>` actions for the game to handle).

use std::f32::consts::{PI, TAU};

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::math::Vec2;
use engine_core::prelude::*;
use engine_input::{
    ButtonInput, GamepadAxis, GamepadButton, Gamepads, KeyCode, Mouse, MouseButton,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::data::UiData;
use crate::draw::{DrawCtx, Painters, PlanetLook};
use crate::el::{self, El};
use crate::input::{UiEvent, UiInputState};
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Justify, Pick, Position, Shadow};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets;

// ---------------------------------------------------------------------------------- data

/// Planet presets: each picks colors, sea level, clouds, ice and bands for the procedural
/// look (`seed` varies the continents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanetKind {
    #[default]
    Terran,
    Ocean,
    Jungle,
    Desert,
    Ice,
    Lava,
    Barren,
    Gas,
}

impl PlanetKind {
    pub fn label(self) -> &'static str {
        match self {
            PlanetKind::Terran => "Terran world",
            PlanetKind::Ocean => "Ocean world",
            PlanetKind::Jungle => "Jungle world",
            PlanetKind::Desert => "Desert world",
            PlanetKind::Ice => "Ice world",
            PlanetKind::Lava => "Volcanic world",
            PlanetKind::Barren => "Barren rock",
            PlanetKind::Gas => "Gas giant",
        }
    }

    /// The look for this kind with `seed` (any number).
    pub fn look(self, seed: f32) -> PlanetLook {
        use crate::draw::surface;
        let c = |hex: u32| Color::hex(hex).to_array();
        let mut l = PlanetLook::default();
        l.shape[0] = seed;
        match self {
            PlanetKind::Terran => {}
            PlanetKind::Ocean => {
                l.colors = [
                    c(0x051a3a),
                    c(0x1286a6),
                    c(0x3a7a30),
                    c(0xd6c696),
                    c(0x6f6a60),
                    c(0xf4f6fa),
                ];
                l.shape = [seed, 0.64, 1.1, 0.86];
                l.style[0] = 0.15;
                l.clouds[3] = 0.54;
                l.lights[3] = 0.3;
            }
            PlanetKind::Jungle => {
                l.colors = [
                    c(0x08283a),
                    c(0x178a8a),
                    c(0x1f4f18),
                    c(0x6f7a34),
                    c(0x5a5442),
                    c(0xe8ecef),
                ];
                l.shape = [seed, 0.45, 1.2, 0.95];
                l.clouds = [0.95, 0.98, 1.0, 0.56];
                l.atmosphere = [0.42, 0.72, 0.95, 1.0];
                l.style[0] = 0.0;
                l.lights[3] = 0.15;
            }
            PlanetKind::Desert => {
                l.colors = [
                    c(0x16405a),
                    c(0x4a9a96),
                    c(0x7a6a3a),
                    c(0xd29a5a),
                    c(0x8a4a30),
                    c(0xf0e0c8),
                ];
                l.shape = [seed, 0.3, 1.3, 0.96];
                l.style = [1.0, 0.0, 0.0, 0.0];
                l.atmosphere = [0.95, 0.62, 0.38, 0.7];
                l.clouds = [1.0, 0.96, 0.9, 0.1];
                l.lights = [1.0, 0.7, 0.35, 0.35];
            }
            PlanetKind::Ice => {
                l.colors = [
                    c(0x0e2a44),
                    c(0x8fb4d0),
                    c(0xd8e4ee),
                    c(0x9fb8cc),
                    c(0x6a6e78),
                    c(0xf8fbff),
                ];
                l.colors[0][3] = surface::ICE;
                l.shape = [seed, 0.47, 1.0, 0.0];
                l.atmosphere = [0.6, 0.8, 1.0, 0.6];
                l.clouds = [0.95, 0.97, 1.0, 0.25];
                l.lights[3] = 0.0;
            }
            PlanetKind::Lava => {
                l.colors = [
                    c(0xffc04a),
                    c(0xff4a12),
                    c(0x231a18),
                    c(0x3a2a24),
                    c(0x140f0e),
                    c(0x6a5a52),
                ];
                l.colors[0][3] = surface::VOLCANIC;
                l.shape = [seed, 0.42, 1.4, 1.0];
                l.atmosphere = [1.0, 0.45, 0.2, 0.55];
                l.clouds = [0.3, 0.25, 0.23, 0.3];
                l.lights = [1.0, 0.4, 0.08, 1.0];
            }
            PlanetKind::Barren => {
                l.colors = [
                    c(0x2a2a2c),
                    c(0x3c3a38),
                    c(0x7a7570),
                    c(0x948f88),
                    c(0x5e5a56),
                    c(0xc8c4be),
                ];
                l.colors[0][3] = surface::BARREN;
                l.shape = [seed, 0.0, 1.5, 1.2];
                l.atmosphere = [0.6, 0.6, 0.6, 0.0];
                l.clouds[3] = 0.0;
                l.lights[3] = 0.0;
            }
            PlanetKind::Gas => {
                l.colors = [
                    c(0x000000),
                    c(0x000000),
                    c(0xe6d2b0),
                    c(0xf2e6cc),
                    c(0xa0643e),
                    c(0xb8482c),
                ];
                l.style = [0.0, 14.0, 1.0, 1.0];
                l.atmosphere = [0.95, 0.8, 0.6, 0.6];
                l.clouds[3] = 0.0;
                l.lights[3] = 0.0;
            }
        }
        l
    }
}

/// Kinds of places on a planet (icon and color).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoiKind {
    #[default]
    Outpost,
    City,
    Spaceport,
    Cave,
    Quest,
    Vendor,
    Danger,
    Npc,
}

impl PoiKind {
    pub fn icon(self) -> &'static str {
        match self {
            PoiKind::Outpost => "marker_outpost",
            PoiKind::City => "marker_city",
            PoiKind::Spaceport => "marker_spaceport",
            PoiKind::Cave => "marker_cave",
            PoiKind::Quest => "marker_quest",
            PoiKind::Vendor => "marker_vendor",
            PoiKind::Danger => "marker_danger",
            PoiKind::Npc => "marker_npc",
        }
    }

    pub fn color(self) -> Color {
        match self {
            PoiKind::Outpost => Color::hex(0x5fd0ff),
            PoiKind::City => Color::hex(0xf7d999),
            PoiKind::Spaceport => Color::hex(0x9fb8ff),
            PoiKind::Cave => Color::hex(0xb89a7a),
            PoiKind::Quest => Color::hex(0xe8b65a),
            PoiKind::Vendor => Color::hex(0x6fdc8c),
            PoiKind::Danger => Color::hex(0xff6b6b),
            PoiKind::Npc => Color::hex(0xc8a8ff),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PoiKind::Outpost => "Outpost",
            PoiKind::City => "City",
            PoiKind::Spaceport => "Spaceport",
            PoiKind::Cave => "Cave",
            PoiKind::Quest => "Objective",
            PoiKind::Vendor => "Trader",
            PoiKind::Danger => "Danger",
            PoiKind::Npc => "Contact",
        }
    }
}

/// A place on a planet. `lon`/`lat` in degrees (east and north positive).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Poi {
    pub id: String,
    pub name: String,
    pub kind: PoiKind,
    pub lon: f64,
    pub lat: f64,
    pub discovered: bool,
    pub description: String,
}

/// A planet orbiting a star.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Planet {
    pub id: String,
    pub name: String,
    pub kind: PlanetKind,
    pub seed: f32,
    pub radius_km: f64,
    /// Orbit radius (AU) and where on it the planet is (degrees).
    pub orbit_au: f32,
    pub phase_deg: f32,
    /// Year length, days (sets how fast it moves on the system map).
    pub period_days: f32,
    pub rings: bool,
    pub moons: u32,
    pub gravity_g: f32,
    pub atmosphere: String,
    pub population: String,
    pub description: String,
    pub pois: Vec<Poi>,
}

impl Default for Planet {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: PlanetKind::Terran,
            seed: 1.0,
            radius_km: 6000.0,
            orbit_au: 1.0,
            phase_deg: 0.0,
            period_days: 365.0,
            rings: false,
            moons: 0,
            gravity_g: 1.0,
            atmosphere: "Breathable".into(),
            population: String::new(),
            description: String::new(),
            pois: Vec::new(),
        }
    }
}

impl Planet {
    pub fn look(&self) -> PlanetLook {
        let mut l = self.kind.look(self.seed);
        if self.rings {
            l.style[3] = 1.0;
        }
        l
    }
}

/// A star system. `x`/`y` place it in the galaxy disk (-1..1, the disk's radius is 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StarSystem {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub star_color: Color,
    /// Spectral class shown in the info card ("G2", "M4", ...).
    pub star_class: String,
    /// Star size relative to the Sun.
    pub star_size: f32,
    pub faction: String,
    pub faction_color: Color,
    pub discovered: bool,
    pub description: String,
    pub planets: Vec<Planet>,
}

impl Default for StarSystem {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            x: 0.0,
            y: 0.0,
            star_color: Color::hex(0xfff1d0),
            star_class: "G2".into(),
            star_size: 1.0,
            faction: String::new(),
            faction_color: Color::hex(0x8a94a8),
            discovered: true,
            description: String::new(),
            planets: Vec::new(),
        }
    }
}

/// Where someone is: a system, optionally a planet and a spot on it (degrees).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MapLocation {
    pub system: String,
    pub planet: String,
    pub lon: f64,
    pub lat: f64,
}

/// The navigation target (drawn on every map level, the compass and the minimap).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Waypoint {
    pub system: String,
    pub planet: String,
    pub lon: f64,
    pub lat: f64,
    pub label: String,
}

/// The galaxy's name and how it is drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Galaxy {
    pub name: String,
    /// Turn of the spiral, radians.
    pub rotation: f32,
    /// 1 = seen face on, smaller = tilted.
    pub tilt: f32,
}

impl Default for Galaxy {
    fn default() -> Self {
        Self {
            name: "Galaxy".into(),
            rotation: 0.0,
            tilt: 0.62,
        }
    }
}

/// Everything the map shows.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MapData {
    pub galaxy: Galaxy,
    pub systems: Vec<StarSystem>,
    /// Hyperlanes between systems, by id.
    pub lanes: Vec<[String; 2]>,
    pub player: MapLocation,
    pub waypoint: Option<Waypoint>,
}

impl MapData {
    pub fn system(&self, id: &str) -> Option<&StarSystem> {
        self.systems.iter().find(|s| s.id == id)
    }

    /// A planet by id, with its system.
    pub fn planet(&self, id: &str) -> Option<(&StarSystem, &Planet)> {
        self.systems
            .iter()
            .find_map(|s| s.planets.iter().find(|p| p.id == id).map(|p| (s, p)))
    }

    pub fn player_planet(&self) -> Option<&Planet> {
        self.planet(&self.player.planet).map(|(_, p)| p)
    }

    /// A place by id, with its planet.
    pub fn poi(&self, id: &str) -> Option<(&Planet, &Poi)> {
        self.systems
            .iter()
            .flat_map(|s| s.planets.iter())
            .find_map(|p| p.pois.iter().find(|q| q.id == id).map(|q| (p, q)))
    }

    /// Meters and bearing (degrees) from the player to the waypoint when both are on the same
    /// planet.
    pub fn waypoint_nav(&self) -> Option<(f64, f64)> {
        let w = self.waypoint.as_ref()?;
        if w.planet.is_empty() || w.planet != self.player.planet {
            return None;
        }
        let planet = self.player_planet()?;
        let (d, b) = great_circle(self.player.lon, self.player.lat, w.lon, w.lat);
        Some((d * planet.radius_km * 1000.0, b))
    }
}

/// Angular distance (radians) and initial bearing (degrees, 0 = north, clockwise) between two
/// points given in degrees.
pub fn great_circle(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> (f64, f64) {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let a = ((p2 - p1) * 0.5).sin().powi(2) + p1.cos() * p2.cos() * (dl * 0.5).sin().powi(2);
    let d = 2.0 * a.sqrt().min(1.0).asin();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    (d, y.atan2(x).to_degrees().rem_euclid(360.0))
}

/// The demo universe: the Veil galaxy, the player on Veyra's northern ridge.
pub fn demo_map() -> MapData {
    let poi = |id: &str, name: &str, kind: PoiKind, lon: f64, lat: f64, desc: &str| Poi {
        id: id.into(),
        name: name.into(),
        kind,
        lon,
        lat,
        discovered: true,
        description: desc.into(),
    };
    let veyra = Planet {
        id: "veyra".into(),
        name: "Veyra".into(),
        kind: PlanetKind::Terran,
        seed: 160.8,
        radius_km: 1180.0,
        orbit_au: 1.0,
        phase_deg: 205.0,
        period_days: 410.0,
        moons: 2,
        gravity_g: 0.82,
        atmosphere: "Breathable, thin".into(),
        population: "1.4 million".into(),
        description: "A cold, bright world of ridges and inland seas. The relay network that tied the Veil together runs from its northern highlands.".into(),
        pois: vec![
            poi("hollow_reach", "Hollow Reach", PoiKind::Vendor, 12.0, 48.0, "A trading post in the lee of the ridge. Old Brannoc keeps the stall."),
            poi("relay_tower", "Relay Tower", PoiKind::Quest, 14.6, 50.4, "The silent relay. Something is drawing power from it."),
            poi("ridge_lookout", "Ridge Lookout", PoiKind::Outpost, 9.4, 49.6, "A survey station with a view over the northern sea."),
            poi("caldera_port", "Caldera Port", PoiKind::Spaceport, 31.0, 22.0, "Veyra's only deep-space port, built into a dead volcano."),
            poi("lumen", "Lumen", PoiKind::City, -18.0, 12.0, "The capital, lit day and night by the geothermal vents."),
            poi("glass_caves", "Glass Caves", PoiKind::Cave, 4.0, 41.0, "Caverns of fused sand. Bring a torch."),
            poi("scar", "The Scar", PoiKind::Danger, 22.0, 56.0, "Storm fields. Ships don't come back."),
            poi("old_mill", "Ostra Mill", PoiKind::Npc, 16.0, 44.5, "Ostra Vale trades rumors for fuel cells."),
            poi("south_camp", "Saltmarsh Camp", PoiKind::Outpost, -60.0, -28.0, "Salt harvesters on the southern flats."),
            poi("ice_station", "Polar Station 9", PoiKind::Outpost, 120.0, -71.0, "An ice-core drilling station."),
        ],
        ..Default::default()
    };
    let home = StarSystem {
        id: "veyra_system".into(),
        name: "Veyra".into(),
        x: 0.34,
        y: -0.42,
        star_color: Color::hex(0xffe6b0),
        star_class: "G4".into(),
        star_size: 1.0,
        faction: "Free Relay League".into(),
        faction_color: Color::hex(0x5fd0ff),
        discovered: true,
        description: "A quiet yellow star on the Veil's outer arm.".into(),
        planets: vec![
            Planet {
                id: "ashfall".into(),
                name: "Ashfall".into(),
                kind: PlanetKind::Lava,
                seed: 8.1,
                radius_km: 2400.0,
                orbit_au: 0.42,
                phase_deg: 80.0,
                period_days: 98.0,
                gravity_g: 0.6,
                atmosphere: "Sulfur, toxic".into(),
                description: "Molten and restless. Miners work its night side.".into(),
                ..Default::default()
            },
            veyra,
            Planet {
                id: "morrow".into(),
                name: "Morrow".into(),
                kind: PlanetKind::Ocean,
                seed: 5.2,
                radius_km: 5600.0,
                orbit_au: 1.55,
                phase_deg: 320.0,
                period_days: 720.0,
                moons: 1,
                gravity_g: 1.1,
                atmosphere: "Breathable, humid".into(),
                population: "Floating cities, 3.2 million".into(),
                description: "One endless ocean with a scatter of reef islands.".into(),
                ..Default::default()
            },
            Planet {
                id: "thessaly".into(),
                name: "Thessaly".into(),
                kind: PlanetKind::Gas,
                seed: 2.0,
                radius_km: 61000.0,
                orbit_au: 3.1,
                phase_deg: 150.0,
                period_days: 2400.0,
                rings: true,
                moons: 14,
                gravity_g: 2.4,
                atmosphere: "Hydrogen, helium".into(),
                description: "A banded giant with a great storm. Skimmers harvest fuel from its upper clouds.".into(),
                ..Default::default()
            },
            Planet {
                id: "kell".into(),
                name: "Kell".into(),
                kind: PlanetKind::Ice,
                seed: 11.0,
                radius_km: 3100.0,
                orbit_au: 4.6,
                phase_deg: 30.0,
                period_days: 4200.0,
                gravity_g: 0.4,
                atmosphere: "Nitrogen traces".into(),
                description: "Frozen, quiet, and full of old wrecks.".into(),
                ..Default::default()
            },
        ],
    };
    // The rest of the Veil: systems along the two spiral arms.
    let names = [
        "Kestrel Reach",
        "Orun",
        "Halcyon",
        "Brightwater",
        "Teshar",
        "Cinder Gate",
        "Mirae",
        "Vashti",
        "Low Harbor",
        "Sable",
        "Ithren",
        "Corvid",
        "Aster Nine",
        "Pellucid",
        "Rhune",
        "Dawnmere",
        "Kaskade",
        "Ombra",
        "Lanthe",
        "Quietus",
        "Eldra",
        "Marrow",
        "Solace",
        "Tarn",
    ];
    let classes = [
        ("M2", 0xffb27a, 0.6),
        ("K5", 0xffd2a0, 0.8),
        ("G2", 0xfff1d0, 1.0),
        ("F7", 0xfffaf0, 1.2),
        ("A1", 0xdfe8ff, 1.6),
        ("B3", 0xb8ccff, 2.1),
    ];
    let factions = [
        ("Free Relay League", 0x5fd0ff),
        ("Concord of Lanthe", 0xe8b65a),
        ("Ashen Compact", 0xff6b6b),
        ("Unclaimed", 0x8a94a8),
    ];
    let kinds = [
        PlanetKind::Barren,
        PlanetKind::Desert,
        PlanetKind::Terran,
        PlanetKind::Ice,
        PlanetKind::Gas,
        PlanetKind::Jungle,
        PlanetKind::Ocean,
        PlanetKind::Lava,
    ];
    let mut systems = vec![home];
    let hash = |i: u32, k: u32| {
        let mut h = i.wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        (h & 0xffff) as f32 / 65535.0
    };
    for (i, name) in names.iter().enumerate() {
        let i = i as u32;
        // Along an arm of the drawn spiral: theta = (2 pi k + 4.2 ln r) / 2.
        let r = 0.2 + 0.75 * hash(i, 1);
        let arm = (i % 2) as f32;
        let theta = (TAU * arm + 4.2 * r.ln()) / 2.0 + (hash(i, 2) - 0.5) * 0.5;
        let (cls, col, size) =
            classes[(hash(i, 3) * classes.len() as f32) as usize % classes.len()];
        let (faction, fcol) = factions[(hash(i, 4) * 4.0) as usize % 4];
        let n_planets = 1 + (hash(i, 5) * 5.0) as u32;
        let planets = (0..n_planets)
            .map(|k| {
                let kind = kinds[(hash(i * 7 + k, 6) * kinds.len() as f32) as usize % kinds.len()];
                Planet {
                    id: format!("{}_{}", name.to_lowercase().replace(' ', "_"), k + 1),
                    name: format!(
                        "{} {}",
                        name,
                        ["I", "II", "III", "IV", "V", "VI"][k as usize]
                    ),
                    kind,
                    seed: hash(i, 10 + k) * 20.0,
                    radius_km: if kind == PlanetKind::Gas {
                        50_000.0
                    } else {
                        2000.0 + 5000.0 * hash(i, 20 + k) as f64
                    },
                    orbit_au: 0.4 + k as f32 * 0.9 + hash(i, 30 + k) * 0.5,
                    phase_deg: hash(i, 40 + k) * 360.0,
                    period_days: 200.0 * (1.0 + k as f32 * 2.0),
                    rings: kind == PlanetKind::Gas && hash(i, 50 + k) > 0.5,
                    description: format!("{} in the {} system.", kind.label(), name),
                    ..Default::default()
                }
            })
            .collect();
        systems.push(StarSystem {
            id: name.to_lowercase().replace(' ', "_"),
            name: name.to_string(),
            x: r * theta.cos(),
            y: r * theta.sin(),
            star_color: Color::hex(col),
            star_class: cls.into(),
            star_size: size,
            faction: faction.into(),
            faction_color: Color::hex(fcol),
            discovered: hash(i, 7) > 0.3,
            description: format!("A {cls}-class star held by the {faction}."),
            planets,
        });
    }
    // Hyperlanes: each system to its two nearest neighbors.
    let mut lanes: Vec<[String; 2]> = Vec::new();
    for a in &systems {
        let mut near: Vec<(f32, &StarSystem)> = systems
            .iter()
            .filter(|b| b.id != a.id)
            .map(|b| (((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt(), b))
            .collect();
        near.sort_by(|x, y| x.0.total_cmp(&y.0));
        for (_, b) in near.into_iter().take(2) {
            let key = if a.id < b.id {
                [a.id.clone(), b.id.clone()]
            } else {
                [b.id.clone(), a.id.clone()]
            };
            if !lanes.contains(&key) {
                lanes.push(key);
            }
        }
    }
    MapData {
        galaxy: Galaxy {
            name: "The Veil".into(),
            rotation: 0.35,
            tilt: 0.62,
        },
        systems,
        lanes,
        player: MapLocation {
            system: "veyra_system".into(),
            planet: "veyra".into(),
            lon: 12.0,
            lat: 48.0,
        },
        waypoint: Some(Waypoint {
            system: "veyra_system".into(),
            planet: "veyra".into(),
            lon: 14.6,
            lat: 50.4,
            label: "Relay Tower".into(),
        }),
    }
}

/// Install the demo universe (unless a map is already loaded) and the location data the HUD
/// shows.
pub fn add_demo_map(world: &mut World) {
    let empty = world
        .get_resource::<MapData>()
        .is_none_or(|m| m.systems.is_empty());
    if empty {
        world.insert_resource(demo_map());
    }
    if let Some(mut data) = world.get_resource_mut::<UiData>() {
        data.set(
            "location",
            json!({"name": "Hollow Reach", "region": "Veyra · Northern Ridge", "time": "14:32"}),
        );
    }
}

// ------------------------------------------------------------------------------ the view

/// Something on the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum MapItem {
    System(String),
    Planet(String),
    Poi(String),
}

/// Map levels.
pub const LEVELS: [&str; 4] = ["Surface", "Planet", "System", "Galaxy"];

/// The map canvas's view state (on the canvas node, `paint: "map"`).
#[derive(Component, Debug, Clone, PartialEq)]
pub struct MapView {
    /// Current zoom (0..4) and where it eases to.
    pub zoom: f32,
    pub target: f32,
    /// The system and planet the inner levels show.
    pub system: String,
    pub planet: String,
    /// Surface map center, degrees.
    pub lon: f64,
    pub lat: f64,
    /// Globe rotation, radians.
    pub yaw: f32,
    pub pitch: f32,
    /// Extra pan on the system (AU) and galaxy (disk units) levels.
    pub system_pan: Vec2,
    pub galaxy_pan: Vec2,
    pub selected: Option<MapItem>,
    pub hovered: Option<MapItem>,
    /// Things drawn last frame: center, pick radius (physical px).
    pub hits: Vec<(Vec2, f32, MapItem)>,
    /// Seconds since the last pan or zoom (the globe turns by itself when idle).
    pub idle: f32,
    pub dragging: bool,
    pub drag_from: Vec2,
}

impl Default for MapView {
    fn default() -> Self {
        Self {
            zoom: 1.4,
            target: 1.4,
            system: String::new(),
            planet: String::new(),
            lon: 0.0,
            lat: 0.0,
            yaw: 0.0,
            pitch: 0.35,
            system_pan: Vec2::ZERO,
            galaxy_pan: Vec2::ZERO,
            selected: None,
            hovered: None,
            hits: Vec::new(),
            idle: 10.0,
            dragging: false,
            drag_from: Vec2::ZERO,
        }
    }
}

impl MapView {
    pub fn level(&self) -> usize {
        (self.zoom.clamp(0.0, 3.999)) as usize
    }
}

/// The map screen's state (on its root): which item the info card shows.
#[derive(Component, Debug, Clone, Default)]
pub struct MapScreen {
    pub canvas: Option<Entity>,
    pub card_for: Option<MapItem>,
    pub level: usize,
}

/// Layer switches (UiData `map.layers.*`).
fn layer(world: &World, name: &str) -> bool {
    world
        .get_resource::<UiData>()
        .and_then(|d| d.get(&format!("map.layers.{name}")))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Weight of `level` at `zoom`: 1 inside the level, crossfading over 0.15 at each boundary.
fn level_weight(zoom: f32, level: usize) -> f32 {
    let l = level as f32;
    let w = 0.15;
    let lo = if level == 0 {
        1.0
    } else {
        ((zoom - (l - w)) / (2.0 * w)).clamp(0.0, 1.0)
    };
    let hi = if level == 3 {
        1.0
    } else {
        (((l + 1.0 + w) - zoom) / (2.0 * w)).clamp(0.0, 1.0)
    };
    let k = lo.min(hi);
    k * k * (3.0 - 2.0 * k)
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Globe radius at planet-level zoom `u` (0..1), physical px.
fn globe_radius(u: f32, h: f32) -> f32 {
    h * 0.42 * 2f32.powf(-u * 1.35)
}

/// System-level magnification at zoom `u` (0..1): 16x on the focused planet at 0, the whole
/// system at 1, easing so most of the level shows the full system.
fn system_zoom(u: f32) -> f32 {
    let k = (1.0 - u.clamp(-0.2, 1.2)).max(0.0);
    2f32.powf(4.0 * k * k)
}

/// Rotate a planet-frame direction into view space (inverse of the shader's mapping).
fn to_view(d: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    // n = rot_y(rot_x(v, -pitch), yaw)  =>  v = rot_x(rot_y(n, -yaw), pitch)
    let (c, s) = ((-yaw).cos(), (-yaw).sin());
    let a = [c * d[0] + s * d[2], d[1], -s * d[0] + c * d[2]];
    let (c, s) = (pitch.cos(), pitch.sin());
    [a[0], c * a[1] - s * a[2], s * a[1] + c * a[2]]
}

fn lonlat_dir(lon: f64, lat: f64) -> [f32; 3] {
    let (lo, la) = (lon.to_radians() as f32, lat.to_radians() as f32);
    [la.cos() * lo.sin(), la.sin(), la.cos() * lo.cos()]
}

/// Sun direction for the planet shader from a screen-space direction toward the star.
fn sun_from(dir: Vec2) -> (f32, f32) {
    let v = engine_core::math::Vec3::new(dir.x, -dir.y, 0.55).normalize_or_zero();
    (v.x.atan2(v.z), v.y.asin())
}

/// Screen position of a galaxy-disk point.
fn galaxy_point(g: Vec2, center: Vec2, radius: f32, rotation: f32, tilt: f32) -> Vec2 {
    // The shader maps screen p0 to disk p = R(a) (p0.x, p0.y / tilt).
    let (c, s) = (rotation.cos(), rotation.sin());
    let x0 = c * g.x + s * g.y;
    let y0 = -s * g.x + c * g.y;
    center + Vec2::new(x0, y0 * tilt) * radius
}

fn planet_base_radius(p: &Planet, px: f32) -> f32 {
    let r = (p.radius_km as f32 / 6000.0).powf(0.35);
    (7.0 + 9.0 * r.min(2.2)) * px
}

/// Planet position on the system map (AU, orbit plane) at real time `t`.
fn orbit_pos(p: &Planet, t: f32) -> Vec2 {
    let speed = 0.6 / p.period_days.max(10.0).sqrt();
    let a = p.phase_deg.to_radians() + t * speed * 0.05;
    Vec2::new(a.cos(), a.sin()) * p.orbit_au
}

// ------------------------------------------------------------------------------- painter

struct Painted<'a> {
    hits: Vec<(Vec2, f32, MapItem)>,
    view: &'a MapView,
    selected: Option<&'a MapItem>,
    hovered: Option<&'a MapItem>,
}

impl Painted<'_> {
    fn is_sel(&self, it: &MapItem) -> bool {
        self.selected == Some(it)
    }
    fn is_hot(&self, it: &MapItem) -> bool {
        self.hovered == Some(it) || self.selected == Some(it)
    }
}

/// A selection reticle: four corner brackets.
fn reticle(ctx: &mut DrawCtx, c: Vec2, r: f32, color: Color) {
    let px = ctx.px;
    let k = r + 6.0 * px;
    let l = 7.0 * px;
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let corner = c + Vec2::new(sx * k, sy * k);
        ctx.line(
            corner,
            corner - Vec2::new(sx * l, 0.0),
            2.0 * px,
            color,
            0.0,
        );
        ctx.line(
            corner,
            corner - Vec2::new(0.0, sy * l),
            2.0 * px,
            color,
            0.0,
        );
    }
}

fn label(
    ctx: &mut DrawCtx,
    text: &str,
    size: f32,
    weight: u16,
    color: Color,
    pos: Vec2,
    anchor: Vec2,
) -> Rect {
    // A soft dark pad behind map labels keeps them readable on bright terrain.
    let m = ctx.measure(&crate::text::TextSpec {
        text: text.to_string(),
        size: size * ctx.px,
        weight,
        color,
        wrap: false,
        ..Default::default()
    });
    let tl = pos - m * anchor;
    let pad = 4.0 * ctx.px;
    ctx.rect(
        Rect::new(
            tl.x - pad * 1.5,
            tl.y - pad * 0.5,
            m.x + pad * 3.0,
            m.y + pad,
        ),
        Color::hexa(0x05080e8c),
        (m.y + pad) * 0.5,
        0.0,
        Color::TRANSPARENT,
    );
    ctx.text(text, size, weight, color, pos, anchor)
}

/// Keeps map labels from piling up: each one tries below, above, right and left of its marker
/// and takes the first side that covers no marker or label placed before it.
#[derive(Default)]
struct Declutter(Vec<Rect>);

impl Declutter {
    fn block(&mut self, r: Rect) {
        self.0.push(r);
    }

    /// Where to put a label `gap` px from the marker at `q`, as (position, anchor). `None`
    /// when every side is taken, unless `keep` (then it goes below anyway).
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        ctx: &mut DrawCtx,
        text: &str,
        size: f32,
        weight: u16,
        q: Vec2,
        gap: f32,
        keep: bool,
    ) -> Option<(Vec2, Vec2)> {
        let m = ctx.measure(&crate::text::TextSpec {
            text: text.to_string(),
            size: size * ctx.px,
            weight,
            wrap: false,
            ..Default::default()
        });
        let pad = 4.0 * ctx.px;
        let side = gap + pad;
        let spots = [
            (Vec2::new(0.0, gap), Vec2::new(0.5, 0.0)),
            (Vec2::new(0.0, -gap), Vec2::new(0.5, 1.0)),
            (Vec2::new(side, 0.0), Vec2::new(0.0, 0.5)),
            (Vec2::new(-side, 0.0), Vec2::new(1.0, 0.5)),
        ];
        let rect = |(off, anchor): (Vec2, Vec2)| {
            let tl = q + off - m * anchor;
            Rect::new(
                tl.x - pad * 1.5,
                tl.y - pad * 0.5,
                m.x + pad * 3.0,
                m.y + pad,
            )
        };
        let free = |r: &Rect| self.0.iter().all(|o| o.intersect(r).is_empty());
        let spot = spots
            .into_iter()
            .find(|s| free(&rect(*s)))
            .or(keep.then_some(spots[0]))?;
        self.0.push(rect(spot));
        Some((q + spot.0, spot.1))
    }
}

fn paint_map(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(view) = world.get::<MapView>(e).cloned() else {
        return;
    };
    let Some(map) = world.get_resource::<MapData>().cloned() else {
        ctx.text(
            "No map loaded",
            16.0,
            700,
            Color::WHITE,
            ctx.rect.center(),
            Vec2::splat(0.5),
        );
        return;
    };
    let p = world.resource::<Theme>().palette.clone();
    let layers = (
        layer(world, "places"),
        layer(world, "lanes"),
        layer(world, "labels"),
        layer(world, "grid"),
        layer(world, "clouds"),
    );
    let r = ctx.rect;
    let t = ctx.time;
    let z = view.zoom.clamp(0.0, 3.999);

    // Deep space under everything, drifting with the pan.
    ctx.rect(r, Color::hex(0x03050a), 0.0, 0.0, Color::TRANSPARENT);
    let drift = Vec2::new(view.galaxy_pan.x, view.galaxy_pan.y) * 400.0 * ctx.px
        + Vec2::new(z * 120.0, view.yaw * 60.0) * ctx.px;
    ctx.starfield(r, drift, 1.0, 7.0, 0.0);

    let mut out = Painted {
        hits: Vec::new(),
        view: &view,
        selected: view.selected.as_ref(),
        hovered: view.hovered.as_ref(),
    };
    for level in [3usize, 2, 1, 0] {
        let w = level_weight(z, level);
        if w <= 0.001 {
            continue;
        }
        let u = z - level as f32;
        ctx.faded(w, |ctx| match level {
            3 => draw_galaxy(ctx, &map, &p, &mut out, u, t, layers),
            2 => draw_system(ctx, &map, &p, &mut out, u, t, layers),
            1 => draw_globe(ctx, &map, &p, &mut out, u, t, layers),
            _ => draw_surface(ctx, &map, &p, &mut out, u, t, layers),
        });
    }
    // Vignette.
    ctx.rect_fill(
        r,
        Fill::Radial {
            inner: Color::hexa(0x00000000),
            outer: Color::hexa(0x000000a0),
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    let hits = out.hits;
    if let Some(mut v) = world.get_mut::<MapView>(e) {
        v.hits = hits;
    }
}

type Layers = (bool, bool, bool, bool, bool);

fn draw_galaxy(
    ctx: &mut DrawCtx,
    map: &MapData,
    p: &crate::theme::Palette,
    out: &mut Painted,
    u: f32,
    t: f32,
    layers: Layers,
) {
    let r = ctx.rect;
    let px = ctx.px;
    let view = out.view;
    let g = &map.galaxy;
    let focus = map
        .system(&view.system)
        .map(|s| Vec2::new(s.x, s.y))
        .unwrap_or(Vec2::ZERO);
    let base = r.h * 0.44;
    let zoomed = 2f32.powf((1.0 - u.clamp(-0.2, 1.2)) * 3.2);
    let radius = base * zoomed;
    // Center the focus system when zoomed in, the whole disk when zoomed out.
    let k = smooth(u);
    let look_at = focus.lerp(Vec2::ZERO, k) + view.galaxy_pan;
    let center0 = r.center() + Vec2::new(-r.w * 0.04, 0.0);
    let off = galaxy_point(look_at, Vec2::ZERO, radius, g.rotation, g.tilt);
    let center = center0 - off;
    ctx.galaxy(r, center, radius, g.rotation + t * 0.004, g.tilt);
    let gp = |s: &StarSystem| {
        galaxy_point(
            Vec2::new(s.x, s.y),
            center,
            radius,
            g.rotation + t * 0.004,
            g.tilt,
        )
    };
    // Hyperlanes.
    if layers.1 {
        for [a, b] in &map.lanes {
            let (Some(sa), Some(sb)) = (map.system(a), map.system(b)) else {
                continue;
            };
            let known = sa.discovered && sb.discovered;
            let col = if known {
                p.info.with_alpha(0.32)
            } else {
                Color::WHITE.with_alpha(0.1)
            };
            ctx.line(
                gp(sa),
                gp(sb),
                1.2 * px,
                col,
                if known { 0.0 } else { 6.0 * px },
            );
        }
    }
    // Route to the waypoint's system.
    if let Some(w) = &map.waypoint
        && w.system != map.player.system
        && let (Some(a), Some(b)) = (map.system(&map.player.system), map.system(&w.system))
    {
        ctx.line(gp(a), gp(b), 2.0 * px, p.accent.with_alpha(0.8), 10.0 * px);
    }
    for s in &map.systems {
        let q = gp(s);
        if !r.inflate(40.0 * px).contains(q) {
            continue;
        }
        let item = MapItem::System(s.id.clone());
        let hot = out.is_hot(&item);
        let size = (2.6 + s.star_size * 1.6) * px * zoomed.powf(0.25);
        let col = if s.discovered {
            s.star_color
        } else {
            Color::hex(0x8a94a8)
        };
        ctx.glow(
            q,
            size * 5.0,
            col.with_alpha(if s.discovered { 0.55 } else { 0.25 }),
            1.8,
        );
        ctx.glow(q, size * 1.4, Color::WHITE.with_alpha(0.95), 2.5);
        if hot {
            ctx.ring(
                q,
                Vec2::splat(size * 3.0),
                1.5 * px,
                p.accent_light,
                0.0,
                TAU,
                0.0,
            );
        }
        if s.id == map.player.system {
            let pulse = (t * 2.0).fract();
            ctx.ring(
                q,
                Vec2::splat(size * (2.5 + pulse * 5.0)),
                1.5 * px,
                p.info.with_alpha(1.0 - pulse),
                0.0,
                TAU,
                0.0,
            );
        }
        if map
            .waypoint
            .as_ref()
            .is_some_and(|w| w.system == s.id && w.system != map.player.system)
        {
            let d = 12.0 * px;
            ctx.icon(
                "marker_quest",
                Rect::new(q.x - d * 0.5, q.y - size * 3.0 - d, d, d),
                Color::WHITE,
            );
        }
        if out.is_sel(&item) {
            reticle(ctx, q, size * 2.2, p.accent_light);
        }
        if layers.2 && (s.discovered || hot) {
            let name = if s.discovered {
                s.name.as_str()
            } else {
                "Uncharted"
            };
            let color = if s.id == map.player.system {
                p.info
            } else if hot {
                p.accent_light
            } else {
                Color::WHITE.with_alpha(0.82)
            };
            ctx.text(
                name,
                12.0,
                700,
                color,
                q + Vec2::new(0.0, size * 2.4 + 4.0 * px),
                Vec2::new(0.5, 0.0),
            );
            if hot && s.discovered && !s.faction.is_empty() {
                ctx.text(
                    &s.faction,
                    10.0,
                    600,
                    s.faction_color,
                    q + Vec2::new(0.0, size * 2.4 + 20.0 * px),
                    Vec2::new(0.5, 0.0),
                );
            }
        }
        out.hits.push((q, (size * 3.0).max(14.0 * px), item));
    }
}

fn draw_system(
    ctx: &mut DrawCtx,
    map: &MapData,
    p: &crate::theme::Palette,
    out: &mut Painted,
    u: f32,
    t: f32,
    layers: Layers,
) {
    let r = ctx.rect;
    let px = ctx.px;
    let view = out.view;
    let Some(sys) = map
        .system(&view.system)
        .or_else(|| map.system(&map.player.system))
    else {
        return;
    };
    let outer = sys
        .planets
        .iter()
        .map(|p| p.orbit_au)
        .fold(1.0f32, f32::max)
        * 1.08;
    let tilt = 0.5;
    let zf = system_zoom(u);
    let au = r.h * 0.43 / outer * zf;
    let focus = sys
        .planets
        .iter()
        .find(|p| p.id == view.planet)
        .map(|pl| orbit_pos(pl, t))
        .unwrap_or(Vec2::ZERO);
    let look_at = focus.lerp(Vec2::ZERO, smooth(u)) + view.system_pan;
    let center = r.center() + Vec2::new(-r.w * 0.04, 0.0);
    let to_screen = |a: Vec2| center + Vec2::new(a.x - look_at.x, (a.y - look_at.y) * tilt) * au;
    let star = to_screen(Vec2::ZERO);
    // Star.
    let star_r = (16.0 + 10.0 * sys.star_size) * px * zf.powf(0.45);
    ctx.glow(star, star_r * 9.0, sys.star_color.with_alpha(0.22), 1.2);
    ctx.glow(star, star_r * 3.0, sys.star_color.with_alpha(0.75), 1.6);
    ctx.glow(star, star_r * 1.1, Color::WHITE, 3.0);
    out.hits
        .push((star, star_r * 1.5, MapItem::System(sys.id.clone())));
    let star_label = star + Vec2::new(0.0, star_r * 1.6 + 6.0 * px);
    let mut star_label_alpha = 1.0f32;
    // Asteroid belt between the 2nd and 3rd orbits.
    if sys.planets.len() >= 3 {
        let belt = (sys.planets[1].orbit_au + sys.planets[2].orbit_au) * 0.5;
        for (k, a) in [(0.0f32, 0.32f32), (0.06, 0.22), (-0.05, 0.18)] {
            ctx.ring(
                star,
                Vec2::new(belt + k, (belt + k) * tilt) * au,
                (2.0 + k.abs() * 20.0) * px,
                Color::hex(0xa89a88).with_alpha(a),
                0.0,
                TAU,
                220.0,
            );
        }
    }
    // Orbits, then planets back to front.
    let mut order: Vec<(f32, &Planet, Vec2)> = Vec::new();
    for pl in &sys.planets {
        let pos = orbit_pos(pl, t);
        let item = MapItem::Planet(pl.id.clone());
        let col = if out.is_hot(&item) {
            p.accent.with_alpha(0.7)
        } else {
            Color::WHITE.with_alpha(0.16)
        };
        ctx.ring(
            star,
            Vec2::new(pl.orbit_au, pl.orbit_au * tilt) * au,
            1.2 * px,
            col,
            0.0,
            TAU,
            0.0,
        );
        order.push((pos.y, pl, pos));
    }
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, pl, pos) in order {
        let q = to_screen(pos);
        let base = planet_base_radius(pl, px);
        let pr = (base * zf.powf(0.9)).min(globe_radius(1.0, r.h) * 1.02);
        if !r.inflate(pr * 3.0).contains(q) {
            continue;
        }
        // Fade the star's label while a planet (or its label below it) passes over it.
        let gap =
            (star_label + Vec2::new(0.0, 8.0 * px)) - (q + Vec2::new(0.0, pr * 0.5 + 14.0 * px));
        let near = (gap.x.abs() / (90.0 * px + pr)).max(gap.y.abs() / (26.0 * px + pr * 0.8));
        star_label_alpha = star_label_alpha.min(smooth((near - 0.8) / 0.4));
        let (az, el) = sun_from((star - q).normalize_or_zero());
        let item = MapItem::Planet(pl.id.clone());
        let look = pl.look();
        let halo = if look.atmosphere[3] > 0.0 { 0.08 } else { 0.0 };
        if pl.rings {
            ring_half(ctx, q, pr, true);
        }
        ctx.planet(q, pr, look, t * 0.05 + pl.seed, 0.3, az, el, halo);
        if pl.rings {
            ring_half(ctx, q, pr, false);
        }
        // Moons as tiny dots.
        for m in 0..pl.moons.min(4) {
            let a = t * (0.3 + m as f32 * 0.17) + m as f32 * 2.1;
            let d = pr * (1.8 + m as f32 * 0.45);
            let mq = q + Vec2::new(a.cos() * d, a.sin() * d * 0.45);
            ctx.glow(mq, 2.6 * px, Color::hex(0xd8dce4), 3.0);
        }
        if pl.id == map.player.planet {
            let pulse = (t * 1.6).fract();
            ctx.ring(
                q,
                Vec2::splat(pr * (1.25 + pulse * 0.6)),
                1.5 * px,
                p.info.with_alpha(1.0 - pulse),
                0.0,
                TAU,
                0.0,
            );
        }
        if out.is_sel(&item) {
            reticle(ctx, q, pr, p.accent_light);
        }
        if layers.2 && pr < r.h * 0.25 {
            let hot = out.is_hot(&item);
            ctx.text(
                &pl.name,
                13.0,
                700,
                if pl.id == map.player.planet {
                    p.info
                } else if hot {
                    p.accent_light
                } else {
                    Color::WHITE.with_alpha(0.88)
                },
                q + Vec2::new(0.0, pr + 8.0 * px),
                Vec2::new(0.5, 0.0),
            );
            ctx.text(
                pl.kind.label(),
                10.0,
                600,
                Color::WHITE.with_alpha(0.5),
                q + Vec2::new(0.0, pr + 24.0 * px),
                Vec2::new(0.5, 0.0),
            );
        }
        out.hits.push((q, pr.max(12.0 * px) * 1.2, item));
    }
    if layers.2 && u > 0.35 && star_label_alpha > 0.01 {
        ctx.faded(star_label_alpha, |ctx| {
            ctx.text(
                &format!("{}  ·  {}", sys.name, sys.star_class),
                12.0,
                700,
                sys.star_color.lerp(Color::WHITE, 0.4),
                star_label,
                Vec2::new(0.5, 0.0),
            );
        });
    }
}

/// Half of a planet's ring system: the far half (drawn before the planet) or the near half.
fn ring_half(ctx: &mut DrawCtx, c: Vec2, pr: f32, back: bool) {
    let (start, sweep) = if back { (PI * 1.5, PI) } else { (PI * 0.5, PI) };
    for (k, a, w) in [
        (1.55f32, 0.55f32, 0.18f32),
        (1.85, 0.4, 0.22),
        (2.15, 0.25, 0.12),
    ] {
        ctx.ring(
            c,
            Vec2::new(pr * k, pr * k * 0.3),
            pr * w,
            Color::hex(0xe0cfae).with_alpha(a),
            start,
            sweep,
            0.0,
        );
    }
}

fn draw_globe(
    ctx: &mut DrawCtx,
    map: &MapData,
    p: &crate::theme::Palette,
    out: &mut Painted,
    u: f32,
    t: f32,
    layers: Layers,
) {
    let r = ctx.rect;
    let px = ctx.px;
    let view = out.view;
    let Some((_, pl)) = map
        .planet(&view.planet)
        .or_else(|| map.planet(&map.player.planet))
    else {
        return;
    };
    let rad = globe_radius(u.clamp(-0.3, 1.3), r.h);
    let c = r.center() + Vec2::new(-r.w * 0.04, 0.0);
    let (yaw, pitch) = (view.yaw, view.pitch);
    let look = pl.look();
    let halo = if look.atmosphere[3] > 0.0 { 0.07 } else { 0.0 };
    let (az, el) = (-0.75, 0.3);
    if pl.rings {
        ring_half(ctx, c, rad, true);
    }
    // Sci-fi frame: a thin ring with ticks around the globe.
    ctx.ring(
        c,
        Vec2::splat(rad * 1.16),
        1.0 * px,
        p.info.with_alpha(0.22),
        0.0,
        TAU,
        0.0,
    );
    ctx.ring(
        c,
        Vec2::splat(rad * 1.2),
        6.0 * px,
        p.info.with_alpha(0.12),
        0.0,
        TAU,
        120.0,
    );
    ctx.planet(c, rad, look, yaw, pitch, az, el, halo);
    if pl.rings {
        ring_half(ctx, c, rad, false);
    }
    // Places on the visible side.
    let project = |lon: f64, lat: f64| {
        let v = to_view(lonlat_dir(lon, lat), yaw, pitch);
        (c + Vec2::new(v[0], -v[1]) * rad, v[2])
    };
    // Labels go on last, so none covers a marker or another label.
    let mut placer = Declutter::default();
    let mut labels: Vec<(String, Vec2, f32, f32)> = Vec::new();
    // The player and the waypoint first, so places merge around them.
    if pl.id == map.player.planet {
        let (q, depth) = project(map.player.lon, map.player.lat);
        if depth > 0.05 {
            let pulse = (t * 1.6).fract();
            ctx.ring(
                q,
                Vec2::splat((5.0 + pulse * 14.0) * px),
                1.5 * px,
                p.info.with_alpha(1.0 - pulse),
                0.0,
                TAU,
                0.0,
            );
            ctx.glow(q, 6.0 * px, p.info, 2.0);
            placer.block(Rect::new(
                q.x - 6.0 * px,
                q.y - 6.0 * px,
                12.0 * px,
                12.0 * px,
            ));
        }
    }
    if layers.0 {
        // Places closer than a marker apart merge into the most important one, with a count.
        let rank = |poi: &Poi| {
            let item = MapItem::Poi(poi.id.clone());
            if out.is_sel(&item) {
                0
            } else if out.is_hot(&item) {
                1
            } else {
                match poi.kind {
                    PoiKind::Quest | PoiKind::City | PoiKind::Spaceport => 2,
                    PoiKind::Vendor => 3,
                    _ => 4,
                }
            }
        };
        let mut seen: Vec<(&Poi, Vec2, f32)> = pl
            .pois
            .iter()
            .filter(|q| q.discovered)
            .filter_map(|poi| {
                let (q, depth) = project(poi.lon, poi.lat);
                (depth >= 0.08).then_some((poi, q, smooth((depth - 0.08) / 0.25)))
            })
            .collect();
        seen.sort_by_key(|(poi, _, _)| rank(poi));
        let merge = 32.0 * px;
        let mut leaders: Vec<(&Poi, Vec2, f32, usize)> = Vec::new();
        for (poi, q, a) in seen {
            match leaders.iter_mut().find(|l| l.1.distance(q) < merge) {
                Some(l) => l.3 += 1,
                None => leaders.push((poi, q, a, 0)),
            }
        }
        for (poi, q, a, more) in leaders {
            let item = MapItem::Poi(poi.id.clone());
            let s = (18.0 + 6.0 * a) * px;
            ctx.faded(a, |ctx| {
                ctx.glow(q, s * 0.9, poi.kind.color().with_alpha(0.4), 1.6);
                let icon = Rect::new(q.x - s * 0.5, q.y - s * 0.5, s, s);
                ctx.icon(poi.kind.icon(), icon, Color::WHITE);
                placer.block(icon.inflate(-2.0 * px));
                if out.is_sel(&item) {
                    reticle(ctx, q, s * 0.5, p.accent_light);
                }
                if more > 0 {
                    // "+2" badge on the bottom right of the marker, clear of a waypoint pin.
                    let b = q + Vec2::new(s * 0.5, s * 0.2);
                    let d = 16.0 * px;
                    let badge = Rect::new(b.x - d * 0.5, b.y - d * 0.5, d, d);
                    ctx.rect(badge, Color::hexa(0x0b1220e6), d * 0.5, 1.0 * px, p.info);
                    ctx.text(
                        &format!("+{more}"),
                        9.0,
                        800,
                        Color::WHITE,
                        b,
                        Vec2::new(0.5, 0.5),
                    );
                    placer.block(badge);
                }
            });
            if layers.2
                && (out.is_hot(&item)
                    || poi.kind == PoiKind::City
                    || poi.kind == PoiKind::Spaceport)
            {
                labels.push((poi.name.clone(), q, s * 0.6 + 2.0 * px, a));
            }
            out.hits.push((q, s * 0.8, item));
        }
    }
    if let Some(w) = &map.waypoint
        && w.planet == pl.id
    {
        let (q, depth) = project(w.lon, w.lat);
        if depth > 0.05 {
            let s = 22.0 * px;
            ctx.glow(q, s, p.accent.with_alpha(0.5), 1.4);
            let icon = Rect::new(q.x - s * 0.5, q.y - s - 4.0 * px, s, s);
            ctx.icon("marker_quest", icon, Color::WHITE);
            placer.block(icon.inflate(-2.0 * px));
        }
    }
    for (text, q, gap, a) in labels {
        if let Some((pos, anchor)) = placer.place(ctx, &text, 11.0, 700, q, gap, true) {
            ctx.faded(a, |ctx| {
                label(ctx, &text, 11.0, 700, Color::WHITE, pos, anchor);
            });
        }
    }
    out.hits.push((c, rad, MapItem::Planet(pl.id.clone())));
    if layers.2 && u > 0.2 {
        ctx.text(
            &pl.name.to_uppercase(),
            13.0,
            800,
            Color::WHITE.with_alpha(0.8),
            c + Vec2::new(0.0, rad * 1.26 + 6.0 * px),
            Vec2::new(0.5, 0.0),
        );
    }
}

fn draw_surface(
    ctx: &mut DrawCtx,
    map: &MapData,
    p: &crate::theme::Palette,
    out: &mut Painted,
    u: f32,
    t: f32,
    layers: Layers,
) {
    let r = ctx.rect;
    let px = ctx.px;
    let view = out.view;
    let Some((_, pl)) = map
        .planet(&view.planet)
        .or_else(|| map.planet(&map.player.planet))
    else {
        return;
    };
    // World width in px: the whole map fits at u = 1, 16x closer at u = 0.
    let w1 = (r.w * 0.86).min(r.h * 1.7);
    let ww = w1 * 2f32.powf((1.0 - u.clamp(-0.3, 1.3)) * 4.0);
    let wh = ww * 0.5;
    // Centered on the focus at surface zooms, easing out to the whole world as it zooms out.
    let k = smooth(((u - 0.5) * 2.0).clamp(0.0, 1.0));
    let clon = view.lon * (1.0 - k as f64);
    let clat = view.lat * (1.0 - k as f64);
    let c = r.center() + Vec2::new(-r.w * 0.04 * k, 0.0);
    let world_rect = Rect::new(
        c.x - ww * 0.5 - (clon / 360.0) as f32 * ww,
        c.y - wh * 0.5 + (clat / 180.0) as f32 * wh,
        ww,
        wh,
    );
    let vis = world_rect.intersect(&r);
    if vis.is_empty() {
        return;
    }
    let lon_at = |x: f32| -180.0 + ((x - world_rect.x) / ww) as f64 * 360.0;
    let lat_at = |y: f32| 90.0 - ((y - world_rect.y) / wh) as f64 * 180.0;
    let to_px = |lon: f64, lat: f64| {
        Vec2::new(
            world_rect.x + ((lon + 180.0) / 360.0) as f32 * ww,
            world_rect.y + ((90.0 - lat) / 180.0) as f32 * wh,
        )
    };
    let rounded = if vis == world_rect { 14.0 * px } else { 0.0 };
    ctx.shadow(vis, rounded, 30.0 * px, Color::hexa(0x000000a0));
    ctx.planet_flat(
        vis,
        pl.look(),
        [
            lon_at(vis.x).to_radians() as f32,
            lon_at(vis.x + vis.w).to_radians() as f32,
        ],
        [
            lat_at(vis.y).to_radians() as f32,
            lat_at(vis.y + vis.h).to_radians() as f32,
        ],
        if layers.4 { 1.0 } else { 0.0 },
        if layers.3 { 1.0 } else { 0.0 },
        rounded,
    );
    if rounded > 0.0 {
        ctx.rect(
            vis,
            Color::TRANSPARENT,
            rounded,
            1.0 * px,
            p.info.with_alpha(0.35),
        );
    }
    ctx.with_clip(vis, |ctx| {
        // Labels go on last, around the markers, so none covers a marker or another label.
        let mut placer = Declutter::default();
        let mut labels: Vec<(String, f32, Color, Vec2, f32, bool)> = Vec::new();
        // Route from the player to the waypoint.
        if let Some(w) = &map.waypoint
            && w.planet == pl.id
            && map.player.planet == pl.id
        {
            let a = to_px(map.player.lon, map.player.lat);
            let b = to_px(w.lon, w.lat);
            ctx.line(a, b, 2.5 * px, Color::hexa(0x00000080), 0.0);
            ctx.line(a, b, 1.6 * px, p.accent.with_alpha(0.9), 9.0 * px);
            let s = 26.0 * px;
            ctx.glow(b, s, p.accent.with_alpha(0.5), 1.4);
            let icon = Rect::new(b.x - s * 0.5, b.y - s - 2.0 * px, s, s);
            ctx.icon("marker_quest", icon, Color::WHITE);
            placer.block(icon.inflate(-3.0 * px));
        }
        if layers.0 {
            for poi in pl.pois.iter().filter(|q| q.discovered) {
                let q = to_px(poi.lon, poi.lat);
                if !vis.inflate(20.0 * px).contains(q) {
                    continue;
                }
                let item = MapItem::Poi(poi.id.clone());
                let hot = out.is_hot(&item);
                let sel = out.is_sel(&item);
                let s = if hot { 30.0 } else { 24.0 } * px;
                ctx.glow(q, s * 0.9, poi.kind.color().with_alpha(0.45), 1.6);
                let icon = Rect::new(q.x - s * 0.5, q.y - s * 0.5, s, s);
                ctx.icon(poi.kind.icon(), icon, Color::WHITE);
                placer.block(icon.inflate(-3.0 * px));
                if sel {
                    reticle(ctx, q, s * 0.5, p.accent_light);
                }
                if layers.2 && (u < 0.75 || hot || poi.kind == PoiKind::City) {
                    let color = if hot { p.accent_light } else { Color::WHITE };
                    labels.push((
                        poi.name.clone(),
                        12.0,
                        color,
                        q,
                        s * 0.55 + 3.0 * px,
                        hot || sel,
                    ));
                }
                out.hits.push((q, s * 0.75, item));
            }
        }
        if map.player.planet == pl.id {
            let q = to_px(map.player.lon, map.player.lat);
            let pulse = (t * 1.6).fract();
            ctx.ring(
                q,
                Vec2::splat((7.0 + pulse * 16.0) * px),
                1.5 * px,
                p.info.with_alpha(1.0 - pulse),
                0.0,
                TAU,
                0.0,
            );
            let s = 26.0 * px;
            ctx.glow(q, s * 0.7, p.info.with_alpha(0.5), 1.6);
            let icon = Rect::new(q.x - s * 0.5, q.y - s * 0.5, s, s);
            ctx.icon("marker_player", icon, Color::WHITE);
            placer.block(icon.inflate(-3.0 * px));
            if layers.2 {
                // After the hovered or selected place, before the others.
                labels.sort_by_key(|l| !l.5);
                let at = labels.iter().take_while(|l| l.5).count();
                labels.insert(
                    at,
                    ("You".into(), 11.0, p.info, q, s * 0.6 + 2.0 * px, false),
                );
            }
        }
        labels.sort_by_key(|l| !l.5);
        for (text, size, color, q, gap, keep) in labels {
            let weight = if text == "You" { 800 } else { 700 };
            if let Some((pos, anchor)) = placer.place(ctx, &text, size, weight, q, gap, keep) {
                label(ctx, &text, size, weight, color, pos, anchor);
            }
        }
    });
    // Scale bar (bottom left of the map).
    if u < 0.95 {
        let km_per_px = pl.radius_km * std::f64::consts::TAU / ww as f64;
        let target = km_per_px * 160.0 * px as f64;
        let mag = 10f64.powf(target.log10().floor());
        let nice = [1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .map(|m| m * mag)
            .find(|v| *v >= target * 0.6)
            .unwrap_or(mag * 10.0);
        let len = (nice / km_per_px) as f32;
        let a = Vec2::new(r.x + 340.0 * px, r.y + r.h - 96.0 * px);
        ctx.line(
            a,
            a + Vec2::new(len, 0.0),
            3.0 * px,
            Color::hexa(0x00000080),
            0.0,
        );
        ctx.line(a, a + Vec2::new(len, 0.0), 1.5 * px, Color::WHITE, 0.0);
        for x in [0.0, len] {
            ctx.line(
                a + Vec2::new(x, -5.0 * px),
                a + Vec2::new(x, 5.0 * px),
                1.5 * px,
                Color::WHITE,
                0.0,
            );
        }
        ctx.text(
            &crate::hud::format_distance(nice * 1000.0),
            11.0,
            700,
            Color::WHITE,
            a + Vec2::new(len * 0.5, -8.0 * px),
            Vec2::new(0.5, 1.0),
        );
    }
}

/// Register the map painter.
pub fn add_painters(p: &mut Painters) {
    p.add("map", paint_map);
}

// ------------------------------------------------------------------------------- screen

fn info_card(world: &World, item: Option<&MapItem>) -> Vec<El> {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let Some(map) = world.get_resource::<MapData>() else {
        return Vec::new();
    };
    let stat = |k: &str, v: String| widgets::stat_row(k, &v, None);
    let mut out = Vec::new();
    let head = |icon: &str, title: &str, sub: &str, color: Color| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(12.0))
            .child(
                El::new()
                    .class("header_icon")
                    .with_style(|s| s.border_color(color.fade(0.6)))
                    .child(
                        El::icon(icon)
                            .with_style(|s| s.size(26.0, 26.0))
                            .pick_ignore(),
                    ),
            )
            .child(
                El::new()
                    .class("col")
                    .with_style(|s| s.gap(2.0).grow(1.0).shrink(1.0))
                    .child(El::text(title).class("heading").text_style(|t| t.nowrap()))
                    .child(El::text(sub).class("caption").text_style(|t| t.nowrap())),
            )
    };
    let desc = |d: &str| {
        El::text(d)
            .class("body")
            .text_style(|t| t.line_height(1.45))
            .with_style(|s| s.font_size(14.0))
    };
    match item {
        Some(MapItem::Poi(id)) => {
            let Some((pl, poi)) = map.poi(id) else {
                return out;
            };
            let (dist, bearing) = great_circle(map.player.lon, map.player.lat, poi.lon, poi.lat);
            let here = map.player.planet == pl.id;
            out.push(head(
                poi.kind.icon(),
                &poi.name,
                &format!("{}  ·  {}", poi.kind.label(), pl.name),
                poi.kind.color(),
            ));
            out.push(desc(&poi.description));
            out.push(widgets::divider());
            if here {
                out.push(stat(
                    "Distance",
                    crate::hud::format_distance(dist * pl.radius_km * 1000.0),
                ));
                out.push(stat("Bearing", format!("{bearing:03.0}°")));
            }
            out.push(stat(
                "Coordinates",
                format!(
                    "{:.1}° {}, {:.1}° {}",
                    poi.lat.abs(),
                    if poi.lat >= 0.0 { "N" } else { "S" },
                    poi.lon.abs(),
                    if poi.lon >= 0.0 { "E" } else { "W" }
                ),
            ));
            let is_wp = map.waypoint.as_ref().is_some_and(|w| {
                w.planet == pl.id
                    && (w.lon - poi.lon).abs() < 1e-6
                    && (w.lat - poi.lat).abs() < 1e-6
            });
            out.push(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(8.0).margin([6.0, 0.0, 0.0, 0.0]))
                    .child(
                        El::button(
                            if is_wp {
                                "Clear waypoint"
                            } else {
                                "Set waypoint"
                            },
                            "map.waypoint",
                        )
                        .id("map_waypoint")
                        .class(if is_wp { "ghost" } else { "primary" })
                        .with_style(|s| s.grow(1.0)),
                    )
                    .child(
                        El::button("Fast travel", format!("map.travel:{}", poi.id))
                            .id("map_travel")
                            .with_style(|s| s.grow(1.0)),
                    ),
            );
        }
        Some(MapItem::Planet(id)) => {
            let Some((sys, pl)) = map.planet(id) else {
                return out;
            };
            out.push(head(
                "planet",
                &pl.name,
                &format!("{}  ·  {} system", pl.kind.label(), sys.name),
                p.info,
            ));
            out.push(desc(&pl.description));
            out.push(widgets::divider());
            out.push(stat(
                "Radius",
                format!(
                    "{} km",
                    crate::data::group_thousands(format!("{:.0}", pl.radius_km))
                ),
            ));
            out.push(stat("Gravity", format!("{:.2} g", pl.gravity_g)));
            out.push(stat("Atmosphere", pl.atmosphere.clone()));
            out.push(stat(
                "Orbit",
                format!("{:.2} AU · {:.0} days", pl.orbit_au, pl.period_days),
            ));
            if pl.moons > 0 {
                out.push(stat("Moons", pl.moons.to_string()));
            }
            if !pl.population.is_empty() {
                out.push(stat("Population", pl.population.clone()));
            }
            let places = pl.pois.iter().filter(|q| q.discovered).count();
            if places > 0 {
                out.push(stat("Known places", places.to_string()));
            }
            out.push(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(8.0).margin([6.0, 0.0, 0.0, 0.0]))
                    .child(
                        El::button("View planet", format!("map.focus:planet:{}", pl.id))
                            .id("map_focus")
                            .class("primary")
                            .with_style(|s| s.grow(1.0)),
                    )
                    .child(
                        El::button("Plot course", format!("map.travel:{}", pl.id))
                            .id("map_travel")
                            .with_style(|s| s.grow(1.0)),
                    ),
            );
        }
        Some(MapItem::System(id)) => {
            let Some(sys) = map.system(id) else {
                return out;
            };
            if !sys.discovered {
                out.push(head(
                    "ui_lock",
                    "Uncharted system",
                    "No survey data",
                    Color::hex(0x8a94a8),
                ));
                out.push(desc("Travel here or buy survey data to reveal it."));
                out.push(
                    El::button("Plot course", format!("map.travel:{}", sys.id))
                        .id("map_travel")
                        .class("primary"),
                );
                return out;
            }
            out.push(head(
                "star",
                &sys.name,
                &format!(
                    "{}-class star  ·  {} planets",
                    sys.star_class,
                    sys.planets.len()
                ),
                sys.star_color,
            ));
            out.push(desc(&sys.description));
            out.push(widgets::divider());
            out.push(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(8.0))
                    .child(El::new().with_style(|s| {
                        s.size(10.0, 10.0)
                            .radius(Corners::all(5.0))
                            .bg(sys.faction_color)
                    }))
                    .child(El::text(sys.faction.clone()).with_style(|s| {
                        s.font_size(14.0).font_weight(600).color(sys.faction_color)
                    })),
            );
            for pl in sys.planets.iter().take(6) {
                out.push(
                    El::new()
                        .class("list_item")
                        .id(format!("map_planet:{}", pl.id))
                        .on_click(format!("map.select:planet:{}", pl.id))
                        .with_style(|s| s.padding([7.0, 10.0]))
                        .child(
                            El::icon("planet")
                                .with_style(|s| s.size(20.0, 20.0))
                                .pick_ignore(),
                        )
                        .child(
                            El::text(pl.name.clone())
                                .with_style(|s| s.font_size(14.0).grow(1.0))
                                .pick_ignore(),
                        )
                        .child(
                            El::text(pl.kind.label())
                                .class("caption")
                                .text_style(|t| t.nowrap())
                                .pick_ignore(),
                        ),
                );
            }
            out.push(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(8.0).margin([6.0, 0.0, 0.0, 0.0]))
                    .child(
                        El::button("View system", format!("map.focus:system:{}", sys.id))
                            .id("map_focus")
                            .class("primary")
                            .with_style(|s| s.grow(1.0)),
                    )
                    .child(
                        El::button("Plot course", format!("map.travel:{}", sys.id))
                            .id("map_travel")
                            .with_style(|s| s.grow(1.0)),
                    ),
            );
        }
        None => {
            let here = map
                .player_planet()
                .map(|p| p.name.clone())
                .unwrap_or_default();
            out.push(head(
                "ui_galaxy",
                &map.galaxy.name,
                &format!(
                    "{} systems charted",
                    map.systems.iter().filter(|s| s.discovered).count()
                ),
                p.accent,
            ));
            out.push(desc(&format!(
                "You are on {here}. Scroll to zoom from the surface out to the galaxy, drag to look around, click anything to learn more."
            )));
            if let Some(w) = &map.waypoint {
                out.push(widgets::divider());
                out.push(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(10.0))
                        .child(El::icon("marker_quest").with_style(|s| s.size(22.0, 22.0)))
                        .child(
                            El::new()
                                .class("col")
                                .with_style(|s| s.gap(1.0))
                                .child(El::text("WAYPOINT").class("label_caps"))
                                .child(El::text(w.label.clone()).with_style(|s| {
                                    s.font_size(15.0).font_weight(600).color(p.accent_light)
                                })),
                        ),
                );
            }
        }
    }
    out
}

fn breadcrumb(world: &World, view: &MapView) -> String {
    let Some(map) = world.get_resource::<MapData>() else {
        return String::new();
    };
    let level = view.level();
    let mut parts = vec![map.galaxy.name.clone()];
    if level <= 2
        && let Some(s) = map.system(&view.system)
    {
        parts.push(format!("{} system", s.name));
    }
    if level <= 1
        && let Some((_, p)) = map.planet(&view.planet)
    {
        parts.push(p.name.clone());
    }
    if level == 0 {
        parts.push("Surface".into());
    }
    parts.join("  ›  ")
}

fn build(world: &World, view: &MapView) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let level = view.level();
    let layer_toggle = |label: &str, key: &str| {
        widgets::toggle_row(
            label,
            &format!("map.layers.{key}"),
            &format!("map_layer_{key}"),
        )
    };
    El::new()
        .class("screen")
        .child(
            El::new()
                .id("map_canvas")
                .paint("map")
                .with_style(|s| s.fill_parent().pick(Pick::Block)),
        )
        // Top bar.
        .child(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(0.0)
                        .right(0.0)
                        .top(0.0)
                        .padding([24.0, 36.0, 40.0, 36.0])
                        .gap(24.0)
                        .align_items(Align::Center)
                        .background(Fill::vertical(
                            Color::hexa(0x03050ae0),
                            Color::hexa(0x03050a00),
                        ))
                        .pick(Pick::Ignore)
                })
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.grow(1.0).shrink(1.0).gap(0.0))
                        .child(widgets::header(Some("ui_galaxy"), "Star Map", "", false))
                        .child(
                            El::text(breadcrumb(world, view))
                                .id("map_breadcrumb")
                                .text_style(|t| t.nowrap())
                                .with_style(|s| {
                                    s.font_size(13.0)
                                        .font_weight(600)
                                        .color(p.info)
                                        .margin([-12.0, 0.0, 0.0, 58.0])
                                }),
                        ),
                )
                .child(widgets::tabs(&LEVELS, level, "map.level"))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.grow(1.0).shrink(1.0).justify(Justify::End))
                        .child(widgets::close_button()),
                ),
        )
        // Layers.
        .child(
            El::new()
                .class("glass")
                .id("map_layers")
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(36.0)
                        .top(150.0)
                        .width(250.0)
                        .column()
                        .padding(12.0)
                        .gap(4.0)
                })
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(8.0).padding([2.0, 4.0, 6.0, 4.0]))
                        .child(
                            El::icon("ui_layers")
                                .with_style(|s| s.size(16.0, 16.0).color(p.text_dim)),
                        )
                        .child(El::text("LAYERS").class("label_caps")),
                )
                .child(layer_toggle("Places", "places"))
                .child(layer_toggle("Hyperlanes", "lanes"))
                .child(layer_toggle("Labels", "labels"))
                .child(layer_toggle("Grid", "grid"))
                .child(layer_toggle("Clouds", "clouds")),
        )
        // Zoom buttons.
        .child(
            El::new()
                .class("glass")
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(36.0)
                        .top(500.0)
                        .column()
                        .padding(6.0)
                        .gap(4.0)
                        .radius(Corners::all(12.0))
                })
                .child(
                    El::new()
                        .class("icon_button")
                        .id("map_zoom_in")
                        .on_click("map.zoom:-0.5")
                        .tooltip("Zoom in", "Mouse wheel or right trigger")
                        .child(
                            El::icon("ui_plus")
                                .with_style(|s| s.size(16.0, 16.0))
                                .pick_ignore(),
                        ),
                )
                .child(
                    El::new()
                        .class("icon_button")
                        .id("map_zoom_out")
                        .on_click("map.zoom:0.5")
                        .tooltip("Zoom out", "Mouse wheel or left trigger")
                        .child(
                            El::icon("ui_minus")
                                .with_style(|s| s.size(16.0, 16.0))
                                .pick_ignore(),
                        ),
                ),
        )
        // Info card.
        .child(
            El::new()
                .class("panel")
                .id("map_card")
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .right(36.0)
                        .top(150.0)
                        .width(360.0)
                        .gap(12.0)
                        .shadow(Shadow::new(Color::hexa(0x00000099), 30.0, 8.0))
                })
                .children(info_card(world, view.selected.as_ref())),
        )
        // Hints.
        .child(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(0.0)
                        .right(0.0)
                        .bottom(28.0)
                        .justify(Justify::Center)
                        .gap(26.0)
                        .pick(Pick::Ignore)
                })
                .child(
                    // A glass strip keeps the hints readable over bright terrain and ice.
                    El::new()
                        .class("glass")
                        .class("row")
                        .with_style(|s| {
                            s.padding([8.0, 20.0])
                                .gap(26.0)
                                .radius(Corners::all(21.0))
                                .pick(Pick::Ignore)
                        })
                        .child(widgets::key_hint("Wheel", "RT", "Zoom"))
                        .child(widgets::key_hint("Drag", "RS", "Pan"))
                        .child(widgets::key_hint("Click", "A", "Select"))
                        .child(widgets::key_hint2(["Q", "E"], ["LB", "RB"], "Level"))
                        .child(widgets::key_hint("Esc", "B", "Close")),
                ),
        )
}

/// Builder for `ui.show {"screen": "map"}`: `zoom` 0..4, `system`, `planet`, `select`
/// ({"type": "poi", "id": "relay_tower"}).
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
    if map.systems.is_empty() {
        return Err(
            "There is no map yet. Load one with map.data, or add systems with map.add_system."
                .into(),
        );
    }
    if let Some(mut d) = world.get_resource_mut::<UiData>()
        && d.get("map.layers").is_none()
    {
        d.set(
            "map.layers",
            json!({"places": true, "lanes": true, "labels": true, "grid": true, "clouds": true}),
        );
    }
    let zoom = args
        .get("zoom")
        .and_then(Value::as_f64)
        .unwrap_or(1.4)
        .clamp(0.0, 3.999) as f32;
    let mut view = MapView {
        zoom,
        target: zoom,
        system: args
            .get("system")
            .and_then(Value::as_str)
            .unwrap_or(&map.player.system)
            .to_string(),
        planet: args
            .get("planet")
            .and_then(Value::as_str)
            .unwrap_or(&map.player.planet)
            .to_string(),
        lon: map.player.lon,
        lat: map.player.lat,
        // Face the player's side of the planet.
        yaw: (map.player.lon as f32).to_radians(),
        pitch: (map.player.lat as f32).to_radians() * 0.6,
        ..Default::default()
    };
    if let Some(sel) = args.get("select") {
        view.selected = serde_json::from_value(sel.clone()).ok();
    }
    let el = build(world, &view);
    let sp = screens::spawn_screen(
        world,
        Screen {
            key: "map".into(),
            modal: true,
            layer: 40,
            closable: true,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("The map is empty.")?;
    let canvas = screens::find_in(world, root, "map_canvas");
    if let Some(c) = canvas {
        world.entity_mut(c).insert(view.clone());
    }
    world.entity_mut(root).insert(MapScreen {
        canvas,
        card_for: view.selected.clone(),
        level: view.level(),
    });
    Ok(root)
}

fn open_map(world: &mut World) -> Option<(Entity, MapScreen)> {
    let mut q = world.query::<(Entity, &MapScreen, &Screen)>();
    q.iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, m, _)| (e, m.clone()))
        .next()
}

fn parse_item(s: &str) -> Option<MapItem> {
    let (kind, id) = s.split_once(':')?;
    match kind {
        "system" => Some(MapItem::System(id.into())),
        "planet" => Some(MapItem::Planet(id.into())),
        "poi" => Some(MapItem::Poi(id.into())),
        _ => None,
    }
}

/// Zoom that frames `item`, and the view focus for it.
fn focus_on(view: &mut MapView, map: &MapData, item: &MapItem) {
    match item {
        MapItem::System(id) => {
            view.system = id.clone();
            if let Some(s) = map.system(id) {
                view.planet = s.planets.first().map(|p| p.id.clone()).unwrap_or_default();
            }
            view.system_pan = Vec2::ZERO;
            view.galaxy_pan = Vec2::ZERO;
            view.target = 2.6;
        }
        MapItem::Planet(id) => {
            if let Some((s, _)) = map.planet(id) {
                view.system = s.id.clone();
            }
            view.planet = id.clone();
            view.system_pan = Vec2::ZERO;
            view.target = 1.35;
        }
        MapItem::Poi(id) => {
            if let Some((pl, poi)) = map.poi(id) {
                view.planet = pl.id.clone();
                view.lon = poi.lon;
                view.lat = poi.lat;
                view.yaw = (poi.lon as f32).to_radians();
                view.pitch = (poi.lat as f32).to_radians() * 0.8;
            }
            view.target = 0.3;
        }
    }
    view.idle = 0.0;
}

/// Input, easing and the info card for the open map.
pub fn map_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let Some((root, mut ms)) = open_map(world) else {
        return;
    };
    let Some(canvas) = ms.canvas else {
        return;
    };
    let Some(mut view) = world.get::<MapView>(canvas).cloned() else {
        return;
    };
    let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let vp = world
        .get_resource::<crate::layout::UiViewport>()
        .copied()
        .unwrap_or_default();
    let is_top = screens::open_screens(world)
        .into_iter()
        .find(|(k, ..)| !matches!(k.as_str(), "toasts" | "tooltip" | "drag"))
        .is_some_and(|(_, e, ..)| e == root);
    let mut rebuild_card = false;

    for ev in &events {
        match ev {
            UiEvent::Action { action, entity, .. } => {
                if entity.is_some_and(|e| !screens::is_descendant(world, e, root)) {
                    continue;
                }
                if let Some(l) = action
                    .strip_prefix("map.level:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    view.target = [0.45, 1.35, 2.55, 3.6][l.min(3)];
                    view.idle = 0.0;
                } else if let Some(d) = action
                    .strip_prefix("map.zoom:")
                    .and_then(|s| s.parse::<f32>().ok())
                {
                    view.target = (view.target + d).clamp(0.0, 3.999);
                    view.idle = 0.0;
                } else if let Some(it) = action.strip_prefix("map.select:").and_then(parse_item) {
                    view.selected = Some(it);
                    rebuild_card = true;
                } else if let Some(it) = action.strip_prefix("map.focus:").and_then(parse_item) {
                    focus_on(&mut view, &map, &it);
                } else if action == "map.waypoint" {
                    if let Some(MapItem::Poi(id)) = &view.selected {
                        toggle_waypoint(world, id);
                        rebuild_card = true;
                    }
                } else if let Some(id) = action.strip_prefix("map.travel:") {
                    let name = map
                        .poi(id)
                        .map(|(_, q)| q.name.clone())
                        .or_else(|| map.planet(id).map(|(_, p)| p.name.clone()))
                        .or_else(|| map.system(id).map(|s| s.name.clone()))
                        .unwrap_or_else(|| id.to_string());
                    toast::notify(
                        world,
                        ToastKind::Info,
                        "ui_target",
                        "Course plotted",
                        &format!("Travel to [b]{name}[/b]"),
                        3.0,
                    );
                }
            }
            UiEvent::Tab { screen, delta } if screen == "map" => {
                let l = (view.target as i32 + delta).clamp(0, 3) as usize;
                view.target = [0.45, 1.35, 2.55, 3.6][l];
                view.idle = 0.0;
            }
            _ => {}
        }
    }

    // Pointer and gamepad.
    let mouse = world.get_resource::<Mouse>().cloned().unwrap_or_default();
    let buttons = world.get_resource::<ButtonInput<MouseButton>>().cloned();
    let keys = world.get_resource::<ButtonInput<KeyCode>>().cloned();
    let pads = world.get_resource::<Gamepads>().cloned();
    let pad_buttons = world.get_resource::<ButtonInput<GamepadButton>>().cloned();
    let (hit, focused) = world
        .get_resource::<UiInputState>()
        .map(|st| (st.hit, st.focused))
        .unwrap_or_default();
    let over_canvas = hit == Some(canvas);
    let level = view.zoom.clamp(0.0, 3.999).floor() as usize;
    let h = vp.height.max(1.0);
    let mut pan = Vec2::ZERO;
    if is_top {
        if over_canvas && mouse.scroll.y.abs() > 0.0 {
            view.target = (view.target - mouse.scroll.y * 0.2).clamp(0.0, 3.999);
            view.idle = 0.0;
        }
        let left_down = buttons
            .as_ref()
            .is_some_and(|b| b.pressed(MouseButton::Left));
        if let Some(b) = &buttons {
            if b.just_pressed(MouseButton::Left) && over_canvas {
                view.dragging = true;
                view.drag_from = mouse.position.unwrap_or_default();
            }
            if view.dragging && left_down {
                pan -= mouse.delta;
            }
            if b.just_released(MouseButton::Left) && view.dragging {
                view.dragging = false;
                let moved = mouse
                    .position
                    .map(|p| (p - view.drag_from).length())
                    .unwrap_or(0.0);
                if moved < 6.0 * vp.scale && over_canvas {
                    let picked = view.hovered.clone();
                    if picked != view.selected {
                        view.selected = picked;
                        rebuild_card = true;
                    }
                }
            }
        }
        if let Some(pads) = &pads {
            let rx = pads.any_axis(GamepadAxis::RightStickX);
            let ry = pads.any_axis(GamepadAxis::RightStickY);
            if rx.abs() > 0.15 || ry.abs() > 0.15 {
                pan += Vec2::new(rx, -ry) * 900.0 * dt * vp.scale;
            }
            let zoom =
                pads.any_axis(GamepadAxis::LeftTrigger) - pads.any_axis(GamepadAxis::RightTrigger);
            if zoom.abs() > 0.1 {
                view.target = (view.target + zoom * dt * 1.5).clamp(0.0, 3.999);
                view.idle = 0.0;
            }
        }
        if let Some(k) = &keys {
            let mut kz = 0.0;
            if k.pressed(KeyCode::Equal) || k.pressed(KeyCode::NumpadAdd) {
                kz -= 1.0;
            }
            if k.pressed(KeyCode::Minus) || k.pressed(KeyCode::NumpadSubtract) {
                kz += 1.0;
            }
            if kz != 0.0 {
                view.target = (view.target + kz * dt * 1.5).clamp(0.0, 3.999);
                view.idle = 0.0;
            }
        }
        if pad_buttons
            .as_ref()
            .is_some_and(|b| b.just_pressed(GamepadButton::South))
            && focused.is_none()
            && view.hovered.is_some()
        {
            view.selected = view.hovered.clone();
            rebuild_card = true;
        }
    }
    if pan != Vec2::ZERO {
        view.idle = 0.0;
        let u = view.zoom - level as f32;
        match level {
            3 => {
                let radius = h * 0.44 * 2f32.powf((1.0 - u) * 3.2);
                let g = &map.galaxy;
                // Screen delta back to disk units.
                let d = Vec2::new(pan.x, pan.y / g.tilt.max(0.05)) / radius;
                let (c, s) = (g.rotation.cos(), g.rotation.sin());
                view.galaxy_pan += Vec2::new(c * d.x - s * d.y, s * d.x + c * d.y);
                view.galaxy_pan = view.galaxy_pan.clamp_length_max(1.2);
            }
            2 => {
                let outer = map
                    .system(&view.system)
                    .map(|s| s.planets.iter().map(|p| p.orbit_au).fold(1.0f32, f32::max))
                    .unwrap_or(1.0)
                    * 1.08;
                let au = h * 0.43 / outer * system_zoom(u);
                view.system_pan += Vec2::new(pan.x, pan.y / 0.5) / au;
                view.system_pan = view.system_pan.clamp_length_max(outer * 1.2);
            }
            1 => {
                let rad = globe_radius(u, h);
                view.yaw -= pan.x / rad;
                view.pitch = (view.pitch - pan.y / rad).clamp(-1.3, 1.3);
            }
            _ => {
                let ww = (vp.width * 0.86).min(h * 1.7) * 2f32.powf((1.0 - u) * 4.0);
                view.lon =
                    (view.lon + (pan.x / ww * 360.0) as f64 + 540.0).rem_euclid(360.0) - 180.0;
                view.lat = (view.lat - (pan.y / ww * 360.0) as f64).clamp(-85.0, 85.0);
            }
        }
    }
    // Ease the zoom; turn an idle globe slowly.
    view.idle += dt;
    let k = 1.0 - (-dt * 7.0).exp();
    view.zoom += (view.target - view.zoom) * k;
    if (view.target - view.zoom).abs() < 0.0005 {
        view.zoom = view.target;
    }
    if view.idle > 4.0 && level == 1 {
        view.yaw += dt * 0.06;
    }
    // Hover from last frame's hits.
    view.hovered = if over_canvas && !view.dragging {
        mouse.position.and_then(|m| {
            view.hits
                .iter()
                .filter(|(c, r, _)| (*c - m).length() <= *r)
                .min_by(|a, b| (a.0 - m).length().total_cmp(&(b.0 - m).length()))
                .map(|(_, _, it)| it.clone())
        })
    } else {
        None
    };
    // Level-dependent chrome: tabs and the breadcrumb.
    let new_level = view.level();
    if new_level != ms.level {
        ms.level = new_level;
        for i in 0..4 {
            if let Some(e) = screens::find_in(world, root, &format!("map.level:{i}"))
                && let Some(mut it) = world.get_mut::<Interaction>(e)
            {
                it.selected = i == new_level;
            }
        }
        let crumb = breadcrumb(world, &view);
        if let Some(e) = screens::find_in(world, root, "map_breadcrumb")
            && let Some(mut t) = world.get_mut::<Text>(e)
        {
            t.text = crumb;
        }
    }
    if rebuild_card || ms.card_for != view.selected {
        ms.card_for = view.selected.clone();
        let els = info_card(world, view.selected.as_ref());
        if let Some(card) = screens::find_in(world, root, "map_card")
            && let Err(e) = el::replace_children(world, card, &els)
        {
            tracing::warn!("map card: {e}");
        }
    }
    if let Some(mut v) = world.get_mut::<MapView>(canvas) {
        let hits = std::mem::take(&mut v.hits);
        *v = view;
        v.hits = hits;
    }
    world.entity_mut(root).insert(ms);
}

fn toggle_waypoint(world: &mut World, poi_id: &str) {
    let Some(mut map) = world.get_resource_mut::<MapData>() else {
        return;
    };
    let Some((pl, poi)) = map.poi(poi_id).map(|(p, q)| (p.clone(), q.clone())) else {
        return;
    };
    let system = map
        .planet(&pl.id)
        .map(|(s, _)| s.id.clone())
        .unwrap_or_default();
    let same = map.waypoint.as_ref().is_some_and(|w| {
        w.planet == pl.id && (w.lon - poi.lon).abs() < 1e-6 && (w.lat - poi.lat).abs() < 1e-6
    });
    if same {
        map.waypoint = None;
        toast::notify(
            world,
            ToastKind::Info,
            "marker_quest",
            "Waypoint cleared",
            "",
            2.0,
        );
    } else {
        map.waypoint = Some(Waypoint {
            system,
            planet: pl.id.clone(),
            lon: poi.lon,
            lat: poi.lat,
            label: poi.name.clone(),
        });
        toast::notify(
            world,
            ToastKind::Good,
            "marker_quest",
            "Waypoint set",
            &poi.name,
            2.4,
        );
    }
}

// ----------------------------------------------------------------------------------- API

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// `map.*` APIs.
pub fn register_api(app: &mut App) {
    app.register_api(
        "map.data",
        "Get the whole map (no args) or replace it (pass the map: galaxy, systems [{id, name, x, y, star_color, star_class, faction, discovered, planets [{id, name, kind, seed, radius_km, orbit_au, rings, moons, pois [{id, name, kind, lon, lat}]}]}], lanes, player, waypoint).",
        r#"{}"#,
        |world, args| {
            if args.as_object().is_some_and(|o| !o.is_empty()) {
                let m: MapData = serde_json::from_value(args).map_err(|e| format!("The map doesn't parse: {e}."))?;
                world.insert_resource(m);
                return Ok(json!({"ok": true}));
            }
            serde_json::to_value(world.get_resource::<MapData>().cloned().unwrap_or_default()).map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "map.systems",
        "Systems with their planets and places (ids and names only).",
        "{}",
        |world, _| {
            let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
            Ok(json!(map.systems.iter().map(|s| json!({
                "id": s.id, "name": s.name, "discovered": s.discovered,
                "planets": s.planets.iter().map(|p| json!({"id": p.id, "name": p.name, "kind": p.kind, "places": p.pois.iter().map(|q| json!({"id": q.id, "name": q.name, "kind": q.kind})).collect::<Vec<_>>()})).collect::<Vec<_>>()
            })).collect::<Vec<_>>()))
        },
    );
    app.register_api(
        "map.add_system",
        "Add or replace a star system (same fields as in map.data); `lanes_to` lists systems to connect it to.",
        r#"{"id": "nyx", "name": "Nyx", "x": -0.3, "y": 0.5, "star_class": "M1", "planets": [{"id": "nyx_1", "name": "Nyx I", "kind": "ice"}], "lanes_to": ["orun"]}"#,
        |world, args| {
            let lanes: Vec<String> = args
                .get("lanes_to")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default();
            let s: StarSystem = serde_json::from_value(args).map_err(|e| format!("The system doesn't parse: {e}."))?;
            if s.id.is_empty() {
                return Err("Give the system an \"id\".".into());
            }
            let mut map = world.get_resource_or_insert_with(MapData::default);
            for to in lanes {
                let key = if s.id < to { [s.id.clone(), to] } else { [to, s.id.clone()] };
                if !map.lanes.contains(&key) {
                    map.lanes.push(key);
                }
            }
            map.systems.retain(|x| x.id != s.id);
            let id = s.id.clone();
            map.systems.push(s);
            Ok(json!({"id": id}))
        },
    );
    app.register_api(
        "map.add_poi",
        "Add or replace a place on a planet: planet (id), id, name, kind (outpost, city, spaceport, cave, quest, vendor, danger, npc), lon, lat (degrees), description.",
        r#"{"planet": "veyra", "id": "wreck", "name": "Crashed Hauler", "kind": "danger", "lon": 18.2, "lat": 47.0}"#,
        |world, args| {
            let planet = str_arg(&args, "planet").ok_or("Pass the \"planet\" id.")?.to_string();
            let mut poi: Poi = serde_json::from_value(args).map_err(|e| format!("The place doesn't parse: {e}."))?;
            if poi.id.is_empty() {
                return Err("Give the place an \"id\".".into());
            }
            poi.discovered = true;
            let mut map = world.get_resource_or_insert_with(MapData::default);
            let pl = map
                .systems
                .iter_mut()
                .flat_map(|s| s.planets.iter_mut())
                .find(|p| p.id == planet)
                .ok_or_else(|| format!("There is no planet \"{planet}\". map.systems lists them."))?;
            pl.pois.retain(|q| q.id != poi.id);
            pl.pois.push(poi);
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "map.set_player",
        "Where the player is: system, planet (ids), lon, lat (degrees). Omitted fields keep their value.",
        r#"{"planet": "veyra", "lon": 12.4, "lat": 48.2}"#,
        |world, args| {
            let mut map = world.get_resource_or_insert_with(MapData::default);
            if let Some(s) = str_arg(&args, "system") {
                map.player.system = s.into();
            }
            if let Some(p) = str_arg(&args, "planet") {
                map.player.planet = p.into();
                if let Some((s, _)) = map.planet(p) {
                    let sid = s.id.clone();
                    map.player.system = sid;
                }
            }
            if let Some(v) = args.get("lon").and_then(Value::as_f64) {
                map.player.lon = v;
            }
            if let Some(v) = args.get("lat").and_then(Value::as_f64) {
                map.player.lat = v;
            }
            Ok(serde_json::to_value(&map.player).unwrap_or_default())
        },
    );
    app.register_api(
        "map.waypoint",
        "Set the waypoint to a place (`poi`: id), a spot (planet, lon, lat, label), or clear it (`clear`: true).",
        r#"{"poi": "relay_tower"}"#,
        |world, args| {
            let mut map = world.get_resource_or_insert_with(MapData::default);
            if args.get("clear").and_then(Value::as_bool) == Some(true) {
                map.waypoint = None;
                return Ok(json!({"ok": true}));
            }
            let w = if let Some(id) = str_arg(&args, "poi") {
                let (pl, poi) = map.poi(id).ok_or_else(|| format!("There is no place \"{id}\". map.systems lists them."))?;
                let system = map.planet(&pl.id).map(|(s, _)| s.id.clone()).unwrap_or_default();
                Waypoint {
                    system,
                    planet: pl.id.clone(),
                    lon: poi.lon,
                    lat: poi.lat,
                    label: poi.name.clone(),
                }
            } else {
                let planet = str_arg(&args, "planet").ok_or("Pass a \"poi\" id, or \"planet\" with lon and lat.")?;
                let system = map.planet(planet).map(|(s, _)| s.id.clone()).ok_or_else(|| format!("There is no planet \"{planet}\"."))?;
                Waypoint {
                    system,
                    planet: planet.into(),
                    lon: args.get("lon").and_then(Value::as_f64).unwrap_or(0.0),
                    lat: args.get("lat").and_then(Value::as_f64).unwrap_or(0.0),
                    label: str_arg(&args, "label").unwrap_or("Waypoint").into(),
                }
            };
            map.waypoint = Some(w.clone());
            Ok(serde_json::to_value(w).unwrap_or_default())
        },
    );
    app.register_api(
        "map.view",
        "Move the open map: zoom (0-4), focus ({\"type\": \"system\"|\"planet\"|\"poi\", \"id\": ...}), select (same form, or null).",
        r#"{"focus": {"type": "planet", "id": "thessaly"}}"#,
        |world, args| {
            let (_, ms) = open_map(world).ok_or("The map isn't open. ui.show {\"screen\": \"map\"} opens it.")?;
            let canvas = ms.canvas.ok_or("The map has no canvas.")?;
            let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
            let mut view = world.get::<MapView>(canvas).cloned().ok_or("The map has no view.")?;
            if let Some(f) = args.get("focus") {
                let it: MapItem = serde_json::from_value(f.clone()).map_err(|e| format!("focus: {e}"))?;
                focus_on(&mut view, &map, &it);
            }
            if let Some(z) = args.get("zoom").and_then(Value::as_f64) {
                view.target = (z as f32).clamp(0.0, 3.999);
                if args.get("instant").and_then(Value::as_bool) == Some(true) {
                    view.zoom = view.target;
                }
            }
            if let Some(s) = args.get("select") {
                view.selected = serde_json::from_value(s.clone()).ok();
            }
            let out = json!({"zoom": view.target, "level": LEVELS[view.level()], "system": view.system, "planet": view.planet});
            if let Some(mut v) = world.get_mut::<MapView>(canvas) {
                *v = view;
            }
            Ok(out)
        },
    );
    app.register_api(
        "map.state",
        "The open map: zoom, level, focus, selected and hovered items, and what is under the cursor.",
        "{}",
        |world, _| {
            let (_, ms) = open_map(world).ok_or("The map isn't open.")?;
            let view = ms.canvas.and_then(|c| world.get::<MapView>(c).cloned()).ok_or("The map has no view.")?;
            Ok(json!({
                "zoom": view.zoom, "level": LEVELS[view.level()], "system": view.system, "planet": view.planet,
                "selected": view.selected, "hovered": view.hovered, "visible": view.hits.len()
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn great_circle_matches_known_values() {
        let (d, b) = great_circle(0.0, 0.0, 90.0, 0.0);
        assert!((d - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert!((b - 90.0).abs() < 1e-6);
        let (_, north) = great_circle(10.0, 10.0, 10.0, 20.0);
        assert!(north.abs() < 1e-6 || (north - 360.0).abs() < 1e-6);
    }

    #[test]
    fn demo_map_is_consistent() {
        let m = demo_map();
        assert!(m.systems.len() > 20);
        assert!(m.player_planet().is_some());
        for [a, b] in &m.lanes {
            assert!(m.system(a).is_some() && m.system(b).is_some());
        }
        let (dist, _) = m.waypoint_nav().expect("waypoint on the player's planet");
        assert!(dist > 1000.0, "{dist}");
        let json = serde_json::to_value(&m).unwrap();
        let back: MapData = serde_json::from_value(json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn demo_places_stand_on_land() {
        use crate::planet_noise::is_water;
        let m = demo_map();
        let (_, veyra) = m.planet("veyra").unwrap();
        let look = veyra.look();
        for poi in &veyra.pois {
            assert!(!is_water(poi.lon, poi.lat, &look), "{} is at sea", poi.name);
        }
        assert!(
            !is_water(m.player.lon, m.player.lat, &look),
            "the player is at sea"
        );
        let w = m.waypoint.as_ref().unwrap();
        assert!(!is_water(w.lon, w.lat, &look), "the waypoint is at sea");
    }

    #[test]
    fn level_weights_crossfade() {
        assert_eq!(level_weight(1.5, 1), 1.0);
        assert_eq!(level_weight(1.5, 2), 0.0);
        let a = level_weight(2.0, 1);
        let b = level_weight(2.0, 2);
        assert!(a > 0.3 && b > 0.3);
        assert_eq!(level_weight(3.9, 3), 1.0);
        assert_eq!(level_weight(0.0, 0), 1.0);
    }

    #[test]
    fn globe_projection_inverts_the_shader_rotation() {
        // A point at lon = yaw, lat = 0 faces the viewer when pitch is 0.
        let v = to_view(lonlat_dir(40.0, 0.0), 40f32.to_radians(), 0.0);
        assert!((v[2] - 1.0).abs() < 1e-4, "{v:?}");
    }
}
