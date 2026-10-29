//! Water bodies (oceans, lakes, pools), how they look, how they wave, and module settings.

use engine_core::prelude::*;

/// Where a [`WaterBody`] is, in the horizontal plane around its entity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WaterExtent {
    /// Endless water (an ocean or sea). The surface is drawn out to the horizon.
    Infinite,
    /// A disc of `radius` meters around the entity.
    Circle { radius: f32 },
    /// A rectangle `half_size` meters (local x, z) around the entity, turned by its yaw.
    Rect { half_size: [f32; 2] },
}

impl Default for WaterExtent {
    fn default() -> Self {
        WaterExtent::Circle { radius: 20.0 }
    }
}

impl WaterExtent {
    /// Is the local point (meters from the body's center, already un-rotated) inside?
    pub fn contains_local(&self, local: Vec2) -> bool {
        match *self {
            WaterExtent::Infinite => true,
            WaterExtent::Circle { radius } => local.length_squared() <= radius * radius,
            WaterExtent::Rect { half_size } => {
                local.x.abs() <= half_size[0] && local.y.abs() <= half_size[1]
            }
        }
    }

    /// Distance from a local point inward to the edge (negative outside). Infinite = huge.
    pub fn inset(&self, local: Vec2) -> f32 {
        match *self {
            WaterExtent::Infinite => 1e9,
            WaterExtent::Circle { radius } => radius - local.length(),
            WaterExtent::Rect { half_size } => {
                (half_size[0] - local.x.abs()).min(half_size[1] - local.y.abs())
            }
        }
    }

    /// Radius of a circle that holds the whole extent (infinite for oceans).
    pub fn bounding_radius(&self) -> f32 {
        match *self {
            WaterExtent::Infinite => f32::INFINITY,
            WaterExtent::Circle { radius } => radius,
            WaterExtent::Rect { half_size } => Vec2::from(half_size).length(),
        }
    }
}

/// What lies under a [`WaterBody`]: how deep it is at each point.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum WaterBed {
    /// Ask the terrain (`engine_world::Ground`). Points where the ground is above the surface
    /// are dry.
    #[default]
    Ground,
    /// A flat bottom `depth_m` below the still surface (pools, tanks, open ocean).
    Flat { depth_m: f32 },
    /// A basin: `depth_m` deep in the middle, rising smoothly to the surface over the last
    /// `shore_m` meters before the edge of the extent (ponds and lakes without terrain).
    Bowl { depth_m: f32, shore_m: f32 },
}

/// How light behaves in the water: its color, clarity and surface.
///
/// Absorption and scattering are per meter of water. Clear water absorbs red first (so deep
/// water turns blue); dissolved tannins absorb blue (tea-brown bogs); algae absorb blue and red
/// (green lakes); silt and rock flour scatter light back (milky turquoise glacier lakes, brown
/// rivers) and hide the bottom. The color a deep body shows is what its particles scatter
/// back against what the water absorbs (Gordon's reflectance model, in the shader). Build one
/// from what is in the water with [`WaterOptics::from_constituents`], or take a preset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterOptics {
    /// Light absorbed per meter, linear RGB, 1/m. Pure water ≈ [0.40, 0.05, 0.012].
    pub absorption: [f32; 3],
    /// Light scattered per meter by particles (turbidity), 1/m: how fast the bottom fades.
    /// 0.03 iceberg-clear, 0.3 a clean lake, 1-2 a glacier lake, 5+ a swamp, 20 a flood.
    pub scattering: f32,
    /// Color of what scatters (linear RGB, about 0..1): white rock flour, tan silt, green
    /// algae.
    pub scatter_color: [f32; 3],
    /// Fraction of the scattered light sent back toward where it came from: 0.005 algae,
    /// 0.02 mineral silt (bright, milky water), 0.06 pure water.
    pub backscatter: f32,
    /// Roughness of the calm surface (0 mirror .. 1 matte). Wind adds to it.
    pub roughness: f32,
    /// How readily whitecaps, wakes and splashes leave foam, 0..1.
    pub foam: f32,
    /// Strength of the sun's caustic network on the bottom, 0..1.
    pub caustics: f32,
    /// How much the surface bends the view of the bottom, 0..1.
    pub refraction: f32,
}

impl Default for WaterOptics {
    fn default() -> Self {
        Self::clear_lake()
    }
}

/// What is in the water, from which its color follows ([`WaterOptics::from_constituents`]).
/// The bio-optical model of ocean and lake science, reduced to three color bands: pure water
/// after Pope & Fry (1997); dissolved organic matter after Bricaud, Morel & Prieur (1981);
/// phytoplankton absorption after Bricaud et al. (1998) and its scattering after Gordon &
/// Morel (1983); mineral particles after Babin et al. (2003).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterConstituents {
    /// Chlorophyll (algae, plankton), mg/m³: 0.05 open ocean, 1-5 lakes, 50+ a bloom.
    pub chlorophyll: f32,
    /// Dissolved organic matter ("yellow substance": tannins from peat and leaves), as its
    /// absorption at 440 nm, 1/m: 0.01 ocean, 0.05-0.3 clear lakes, 4-10 bogs and blackwater.
    pub cdom_440: f32,
    /// Mineral particles (silt, clay, glacial rock flour), g/m³: 0.02 iceberg melt, 2-5
    /// glacier lakes, 40+ a river in flood.
    pub sediment: f32,
    /// What the particles look like (linear RGB reflectance): white rock flour, tan silt,
    /// red clay.
    pub sediment_color: [f32; 3],
}

impl Default for WaterConstituents {
    fn default() -> Self {
        Self {
            chlorophyll: 1.0,
            cdom_440: 0.15,
            sediment: 0.3,
            sediment_color: [0.8, 0.75, 0.65],
        }
    }
}

/// Pure water's absorption in the red, green and blue bands (600-700, 500-570, 420-490 nm),
/// 1/m (Pope & Fry 1997).
const PURE_WATER: [f32; 3] = [0.40, 0.050, 0.012];
/// Dissolved organic matter's absorption in each band per unit of its 440 nm absorption:
/// exp(-0.015 (λ - 440)) at 640, 535 and 465 nm.
const CDOM_BANDS: [f32; 3] = [0.050, 0.240, 0.687];
/// Phytoplankton absorption per (mg/m³)^0.65 of chlorophyll in each band (blue and red
/// peaks, a green gap).
const ALGAE_BANDS: [f32; 3] = [0.010, 0.006, 0.032];
/// Mineral particles' absorption per g/m³ in each band (0.031 m²/g at 443 nm, slope 0.0123).
const MINERAL_BANDS: [f32; 3] = [0.0027, 0.0096, 0.023];
/// The light algae scatter back (green-tinted cells), linear RGB.
const ALGAE_COLOR: [f32; 3] = [0.35, 0.75, 0.45];

impl WaterOptics {
    /// Optics of water holding `c`: a calm surface, and caustics and a bent view of the
    /// bottom that fade as the water clouds.
    pub fn from_constituents(c: WaterConstituents) -> Self {
        let chl = c.chlorophyll.max(0.0);
        let sediment = c.sediment.max(0.0);
        let mut absorption = [0.0; 3];
        for (k, a) in absorption.iter_mut().enumerate() {
            *a = PURE_WATER[k]
                + c.cdom_440.max(0.0) * CDOM_BANDS[k]
                + ALGAE_BANDS[k] * chl.powf(0.65)
                + MINERAL_BANDS[k] * sediment;
        }
        // Scattering: algae (Gordon & Morel), mineral particles (about 0.5 m²/g) and the
        // water itself; each sends a different share back.
        let (b_algae, b_mineral, b_water) = (0.3 * chl.powf(0.62), 0.5 * sediment, 0.003);
        let back = (0.007 * b_algae, 0.019 * b_mineral, 0.5 * b_water);
        let scattering = b_algae + b_mineral + b_water;
        let total_back = back.0 + back.1 + back.2;
        let mut color = [0.0; 3];
        for (k, col) in color.iter_mut().enumerate() {
            *col = (back.0 * ALGAE_COLOR[k] + back.1 * c.sediment_color[k] + back.2)
                / total_back.max(1e-9);
        }
        // Murky water hides the bottom: the caustics and the bent view of it go too.
        let clarity = (-scattering * 0.6).exp();
        Self {
            absorption,
            scattering,
            scatter_color: color,
            backscatter: total_back / scattering.max(1e-9),
            roughness: 0.04,
            foam: 0.7,
            caustics: 0.15 + 0.85 * clarity,
            refraction: 0.3 + 0.3 * clarity,
        }
    }

    /// Meltwater off an iceberg: almost pure water, deep sapphire blue, the bottom visible
    /// tens of meters down.
    pub fn iceberg() -> Self {
        Self::from_constituents(WaterConstituents {
            chlorophyll: 0.01,
            cdom_440: 0.003,
            sediment: 0.02,
            sediment_color: [1.0, 1.0, 1.0],
        })
    }

    /// A glacier-fed lake: rock flour turns it milky turquoise (Lake Louise, Peyto).
    pub fn glacial() -> Self {
        Self::from_constituents(WaterConstituents {
            chlorophyll: 0.1,
            cdom_440: 0.02,
            sediment: 2.0,
            sediment_color: [0.92, 0.94, 0.95],
        })
    }

    /// A high mountain lake: clear, blue-green.
    pub fn alpine() -> Self {
        Self::from_constituents(WaterConstituents {
            chlorophyll: 0.3,
            cdom_440: 0.05,
            sediment: 0.2,
            sediment_color: [0.85, 0.85, 0.8],
        })
    }

    /// Clean lake: see a few meters down, green-blue deep water.
    pub fn clear_lake() -> Self {
        Self::from_constituents(WaterConstituents {
            chlorophyll: 1.0,
            cdom_440: 0.15,
            sediment: 0.3,
            sediment_color: [0.8, 0.75, 0.65],
        })
    }

    /// Open ocean: deep blue, clear.
    pub fn ocean() -> Self {
        Self {
            roughness: 0.06,
            foam: 1.0,
            ..Self::from_constituents(WaterConstituents {
                chlorophyll: 0.05,
                cdom_440: 0.01,
                sediment: 0.05,
                sediment_color: [0.9, 0.9, 0.9],
            })
        }
    }

    /// Tropical lagoon: turquoise over white sand, clear for tens of meters.
    pub fn tropical() -> Self {
        Self {
            roughness: 0.03,
            foam: 0.8,
            ..Self::from_constituents(WaterConstituents {
                chlorophyll: 0.1,
                cdom_440: 0.01,
                sediment: 0.1,
                sediment_color: [0.95, 0.93, 0.85],
            })
        }
    }

    /// Fast mountain stream: clear with a little silt.
    pub fn stream() -> Self {
        Self {
            roughness: 0.06,
            foam: 1.0,
            ..Self::from_constituents(WaterConstituents {
                chlorophyll: 0.5,
                cdom_440: 0.1,
                sediment: 0.6,
                sediment_color: [0.8, 0.75, 0.65],
            })
        }
    }

    /// A river in spate: opaque with brown silt.
    pub fn silty_river() -> Self {
        Self {
            foam: 1.0,
            ..Self::from_constituents(WaterConstituents {
                chlorophyll: 1.0,
                cdom_440: 0.4,
                sediment: 40.0,
                sediment_color: [0.56, 0.48, 0.38],
            })
        }
    }

    /// Peat-stained blackwater: clear but tea-brown, black where deep.
    pub fn peat() -> Self {
        Self::from_constituents(WaterConstituents {
            chlorophyll: 1.0,
            cdom_440: 8.0,
            // Fine humic flocs: what little light comes back is tea-brown.
            sediment: 2.0,
            sediment_color: [0.5, 0.3, 0.1],
        })
    }

    /// A swamp: brown-olive, tannins, algae and mud; the bottom gone within a hand's depth.
    pub fn swamp() -> Self {
        Self {
            foam: 0.4,
            ..Self::from_constituents(WaterConstituents {
                chlorophyll: 25.0,
                cdom_440: 4.0,
                sediment: 6.0,
                sediment_color: [0.45, 0.42, 0.3],
            })
        }
    }

    /// An algal bloom: pea-soup green.
    pub fn algal_bloom() -> Self {
        Self::from_constituents(WaterConstituents {
            chlorophyll: 60.0,
            cdom_440: 0.5,
            sediment: 2.0,
            sediment_color: [0.6, 0.6, 0.5],
        })
    }

    /// Murky green-brown pond: the bottom vanishes within a meter.
    pub fn murky_pond() -> Self {
        Self {
            foam: 0.4,
            ..Self::from_constituents(WaterConstituents {
                chlorophyll: 12.0,
                cdom_440: 1.5,
                sediment: 4.0,
                sediment_color: [0.55, 0.5, 0.38],
            })
        }
    }

    /// Every preset by name, clearest first (the showcase's row of pools).
    pub const PRESETS: [&'static str; 12] = [
        "iceberg",
        "ocean",
        "tropical",
        "alpine",
        "glacial",
        "clear_lake",
        "stream",
        "algal_bloom",
        "murky_pond",
        "peat",
        "swamp",
        "silty_river",
    ];

    /// A preset by name (see [`Self::PRESETS`]; also `lake`, `sea`, `lagoon`, `pond`,
    /// `river`, `blackwater`, `bloom`, `silt`).
    pub fn preset(name: &str) -> Option<Self> {
        Some(match name {
            "iceberg" | "meltwater" => Self::iceberg(),
            "glacial" | "glacier" => Self::glacial(),
            "alpine" => Self::alpine(),
            "clear_lake" | "lake" => Self::clear_lake(),
            "ocean" | "sea" => Self::ocean(),
            "tropical" | "lagoon" => Self::tropical(),
            "stream" | "river" => Self::stream(),
            "silty_river" | "silt" | "flood" => Self::silty_river(),
            "peat" | "blackwater" => Self::peat(),
            "swamp" => Self::swamp(),
            "algal_bloom" | "bloom" => Self::algal_bloom(),
            "murky_pond" | "pond" => Self::murky_pond(),
            _ => return None,
        })
    }

    /// Total extinction (absorption + scattering) per channel, 1/m.
    pub fn extinction(&self) -> Vec3 {
        Vec3::from(self.absorption) + Vec3::splat(self.scattering)
    }

    /// Backscattering per meter in each band (what the shader's reflectance model uses).
    pub fn backscattering(&self) -> [f32; 3] {
        self.scatter_color
            .map(|c| c * self.scattering * self.backscatter)
    }
}

/// What drives a body's waves.
///
/// Wind waves follow the real sea-state relations: the longer and harder the wind blows over
/// open water (`fetch_m`), the higher and longer the waves (a 10 m/s wind raises ~2 m waves on
/// the open ocean but only ~5 cm ripples on a 150 m lake). Waves react to wind changes over
/// `response_s` seconds and slow down with the world's time dilation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveSettings {
    /// Raise waves from the wind (`WindField` at the body). Off = calm except for swell.
    pub wind_waves: bool,
    /// Distance the wind blows over the water before reaching here, meters. Small for ponds
    /// and lakes (short choppy waves), 0 = open ocean (fully developed sea).
    pub fetch_m: f32,
    /// Multiplies wave height (1 = physical, 0 = mirror calm). Exaggerate for drama.
    pub height_scale: f32,
    /// Crest sharpness: 0 round swells .. 1 sharp peaked crests (never loops over).
    pub choppiness: f32,
    /// Long waves from distant storms (oceans), significant height in meters (0 = none).
    pub swell_height_m: f32,
    /// Swell period, seconds (8–16 typical).
    pub swell_period_s: f32,
    /// Direction the swell travels toward, degrees (0 = +X, 90 = +Z).
    pub swell_direction_deg: f32,
    /// How long waves take to follow a wind change, seconds.
    pub response_s: f32,
    /// Seed for wave directions and phases.
    pub seed: u64,
}

impl Default for WaveSettings {
    fn default() -> Self {
        Self {
            wind_waves: true,
            fetch_m: 500.0,
            height_scale: 1.0,
            choppiness: 0.6,
            swell_height_m: 0.0,
            swell_period_s: 11.0,
            swell_direction_deg: 0.0,
            response_s: 8.0,
            seed: 7,
        }
    }
}

impl WaveSettings {
    /// Open ocean: unlimited fetch, a moderate swell.
    pub fn ocean() -> Self {
        Self {
            fetch_m: 0.0,
            choppiness: 0.75,
            swell_height_m: 1.2,
            swell_period_s: 12.0,
            response_s: 30.0,
            ..Self::default()
        }
    }

    /// A lake of about `size_m` across.
    pub fn lake(size_m: f32) -> Self {
        Self {
            fetch_m: size_m.max(1.0),
            ..Self::default()
        }
    }
}

/// A body of water: ocean, sea, lake, pond, pool, or a still river reach.
///
/// The still surface is at the entity's world height (`Transform` y); the extent is centered on
/// the entity and turned by its yaw. Everything about it is data, so designers and AI agents
/// can change it live (`entity.set`): make a lake murky, raise a storm swell, drain a pool.
///
/// ```ignore
/// commands.spawn((
///     Name::new("Lake"),
///     Transform::from_xyz(170.0, 4.0, -140.0),
///     WaterBody::lake(40.0).with_bed(WaterBed::Bowl { depth_m: 6.0, shore_m: 25.0 }),
/// ));
/// ```
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct WaterBody {
    pub extent: WaterExtent,
    pub bed: WaterBed,
    pub optics: WaterOptics,
    pub waves: WaveSettings,
    /// Steady surface current (rivers, tides), m/s along world x and z. Floating things drift.
    pub current: [f32; 2],
    /// Simulate ripples, wakes and splashes near the player/camera.
    pub interactive: bool,
    /// Draw the surface.
    pub visible: bool,
}

impl Default for WaterBody {
    fn default() -> Self {
        Self {
            extent: WaterExtent::default(),
            bed: WaterBed::Ground,
            optics: WaterOptics::default(),
            waves: WaveSettings::default(),
            current: [0.0, 0.0],
            interactive: true,
            visible: true,
        }
    }
}

impl WaterBody {
    /// A round lake of `radius` meters with a clear-lake look and lake-sized wind waves.
    pub fn lake(radius: f32) -> Self {
        Self {
            extent: WaterExtent::Circle { radius },
            waves: WaveSettings::lake(radius * 2.0),
            ..Self::default()
        }
    }

    /// An endless ocean with swell, `depth_m` deep unless the terrain says otherwise.
    pub fn ocean() -> Self {
        Self {
            extent: WaterExtent::Infinite,
            bed: WaterBed::Flat { depth_m: 200.0 },
            optics: WaterOptics::ocean(),
            waves: WaveSettings::ocean(),
            ..Self::default()
        }
    }

    /// A rectangular pool `half_x` × `half_z` meters, `depth_m` deep, without wind waves.
    pub fn pool(half_x: f32, half_z: f32, depth_m: f32) -> Self {
        Self {
            extent: WaterExtent::Rect {
                half_size: [half_x, half_z],
            },
            bed: WaterBed::Flat { depth_m },
            optics: WaterOptics::tropical(),
            waves: WaveSettings {
                wind_waves: true,
                fetch_m: half_x.max(half_z) * 2.0,
                ..WaveSettings::default()
            },
            ..Self::default()
        }
    }

    pub fn with_bed(mut self, bed: WaterBed) -> Self {
        self.bed = bed;
        self
    }

    pub fn with_optics(mut self, optics: WaterOptics) -> Self {
        self.optics = optics;
        self
    }

    pub fn with_waves(mut self, waves: WaveSettings) -> Self {
        self.waves = waves;
        self
    }

    pub fn with_current(mut self, x: f32, z: f32) -> Self {
        self.current = [x, z];
        self
    }
}

/// Module-wide quality and behavior settings.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterSettings {
    /// Interactive ripple grid size (cells per side) around the focus. 256 × 0.25 m = 64 m.
    pub ripple_cells: u32,
    /// Ripple grid cell size, meters.
    pub ripple_cell_m: f32,
    /// Speed of ripples in deep water, m/s (real ripples 0.3–1.5 m long travel 0.7–1.6 m/s).
    /// Shallow water slows them (√(g·depth)), so rings bend toward the shore.
    pub ripple_speed: f32,
    /// How fast ripples die out, 1/s.
    pub ripple_damping: f32,
    /// Seconds for foam to fade to a third.
    pub foam_life_s: f32,
    /// Seconds for the currents bodies stir up (a boat's wash, the eddies behind a swimmer)
    /// to fade to a third.
    pub current_life_s: f32,
    /// How tightly those currents curl into swirls, 0 (smooth, they blur out) .. 1.
    pub swirl: f32,
    /// Simulated raindrop impacts per m² per second for each mm/h of rain. The renderer adds
    /// the countless small rings procedurally; these are the ones that make real waves.
    pub rain_drops: f32,
    /// Rain fills puddles on the ground (`SurfaceLayers::puddle_m`) and they drain after.
    pub puddles: bool,
    /// Puddles are simulated within this distance of the focus, meters.
    pub puddle_radius_m: f32,
    /// Water wets the ground it reaches (`SurfaceLayers::wetness`): a band of wet sand above a
    /// lake's waterline, stream banks, the rock round a waterfall's foot.
    pub wet_shores: bool,
    /// Interactors and splashes write `NoiseEvent`s AI can hear.
    pub noise_events: bool,
    /// Run the dispersive ripples ([`crate::surface`]) on the CPU when no GPU runs them: a
    /// headless server whose boats should ride the same Kelvin wakes, a gallery that
    /// simulates without drawing. A few milliseconds a step; off by default.
    pub cpu_surface: bool,
}

impl Default for WaterSettings {
    fn default() -> Self {
        Self {
            ripple_cells: 256,
            ripple_cell_m: 0.25,
            ripple_speed: 1.3,
            ripple_damping: 0.35,
            foam_life_s: 6.0,
            current_life_s: 9.0,
            swirl: 0.5,
            rain_drops: 0.02,
            puddles: true,
            puddle_radius_m: 64.0,
            wet_shores: true,
            noise_events: true,
            cpu_surface: false,
        }
    }
}

/// Local rain over water and ground: a shower, a fountain's spray, a leaking roof, a
/// sprinkler. Adds to the weather's rain inside `radius_m` (rings on water, filling puddles).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct RainShower {
    pub radius_m: f32,
    /// Rain rate, mm/h (light 1, heavy 10, cloudburst 50).
    pub mm_h: f32,
}

impl Default for RainShower {
    fn default() -> Self {
        Self {
            radius_m: 10.0,
            mm_h: 8.0,
        }
    }
}
