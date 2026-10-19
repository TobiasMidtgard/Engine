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
/// water turns blue-green); silt and algae scatter light back (murky water glows with their
/// color and hides the bottom).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterOptics {
    /// Light absorbed per meter, linear RGB, 1/m. Pure water ≈ [0.45, 0.07, 0.04].
    pub absorption: [f32; 3],
    /// Light scattered back per meter by particles (turbidity), 1/m. 0.02 crystal clear,
    /// 0.1 lake, 0.5 murky pond.
    pub scattering: f32,
    /// Color of what scatters (linear RGB): blue-green plankton, brown silt, green algae.
    pub scatter_color: [f32; 3],
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

impl WaterOptics {
    /// Clean mountain lake: see a few meters down, blue-green deep water.
    pub fn clear_lake() -> Self {
        Self {
            absorption: [0.42, 0.11, 0.075],
            scattering: 0.08,
            scatter_color: [0.32, 0.65, 0.62],
            roughness: 0.04,
            foam: 0.7,
            caustics: 0.8,
            refraction: 0.5,
        }
    }

    /// Open ocean: deep blue, clear.
    pub fn ocean() -> Self {
        Self {
            absorption: [0.45, 0.065, 0.03],
            scattering: 0.05,
            scatter_color: [0.12, 0.38, 0.62],
            roughness: 0.06,
            foam: 1.0,
            caustics: 0.6,
            refraction: 0.4,
        }
    }

    /// Tropical lagoon: turquoise over white sand.
    pub fn tropical() -> Self {
        Self {
            absorption: [0.38, 0.05, 0.05],
            scattering: 0.04,
            scatter_color: [0.25, 0.8, 0.78],
            roughness: 0.03,
            foam: 0.8,
            caustics: 1.0,
            refraction: 0.6,
        }
    }

    /// Murky green-brown pond: the bottom vanishes within a meter.
    pub fn murky_pond() -> Self {
        Self {
            absorption: [0.9, 0.55, 0.85],
            scattering: 0.8,
            scatter_color: [0.32, 0.36, 0.16],
            roughness: 0.05,
            foam: 0.4,
            caustics: 0.15,
            refraction: 0.3,
        }
    }

    /// Fast mountain stream: clear with a little silt.
    pub fn stream() -> Self {
        Self {
            absorption: [0.4, 0.1, 0.09],
            scattering: 0.15,
            scatter_color: [0.45, 0.6, 0.5],
            roughness: 0.06,
            foam: 1.0,
            caustics: 0.7,
            refraction: 0.5,
        }
    }

    /// A preset by name: `clear_lake`, `ocean`, `tropical`, `murky_pond`, `stream`.
    pub fn preset(name: &str) -> Option<Self> {
        Some(match name {
            "clear_lake" | "lake" => Self::clear_lake(),
            "ocean" | "sea" => Self::ocean(),
            "tropical" | "lagoon" => Self::tropical(),
            "murky_pond" | "pond" | "swamp" => Self::murky_pond(),
            "stream" | "river" => Self::stream(),
            _ => return None,
        })
    }

    /// Total extinction (absorption + scattering) per channel, 1/m.
    pub fn extinction(&self) -> Vec3 {
        Vec3::from(self.absorption) + Vec3::splat(self.scattering)
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
}

impl Default for WaterSettings {
    fn default() -> Self {
        Self {
            ripple_cells: 256,
            ripple_cell_m: 0.25,
            ripple_speed: 1.3,
            ripple_damping: 0.35,
            foam_life_s: 4.0,
            rain_drops: 0.02,
            puddles: true,
            puddle_radius_m: 64.0,
            wet_shores: true,
            noise_events: true,
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
