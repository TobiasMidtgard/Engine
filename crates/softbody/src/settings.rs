//! Tuning shared by every kind of soft body: solver settings, the look, and module-wide
//! quality knobs.

use engine_core::prelude::*;
use engine_render::material::srgb_hex_to_linear;

/// Solver settings every soft body has. Defaults suit cloth; each kind (rope, jelly, hair)
/// picks its own defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimSettings {
    /// Substeps per 1/60 s. More = stiffer and more accurate, linearly more expensive.
    pub substeps: u32,
    /// Velocity damping, 1/s (air resistance for things without aerodynamics).
    pub damping: f32,
    /// Friction against colliders and the ground (0 = ice, 1 = rubber).
    pub friction: f32,
    /// Collision thickness, meters: how far particles stay from surfaces.
    pub thickness: f32,
    /// Multiplies gravity (0 = floats in place).
    pub gravity_scale: f32,
    /// Multiplies the wind it feels (0 = indoors).
    pub wind_scale: f32,
    /// How much of its anchor's motion the body follows rigidly (0..1). Capes and hair use
    /// 0.2–0.5 so a dash doesn't fling them; flags use 0.
    pub follow: f32,
    /// Collide with physics colliders, characters and bone capsules.
    pub collide: bool,
    /// Cloth keeps from passing through itself (folds stack instead of merging), at
    /// `thickness` × 1.5 between layers. Costs about as much as the rest of the step again.
    pub self_collide: bool,
    /// Cloth simulated on the GPU instead (flags, banners, pennants): visual only, so no
    /// collisions, self-collision, tearing, bullet holes, fire or pushing rigid bodies, and
    /// not listed in `SoftBodySim`. Grids up to 512 particles, pinned along edges or at the
    /// top corners; anything else stays on the CPU. Costs next to nothing on the CPU.
    pub gpu: bool,
    /// Simulate this many seconds immediately when built, so it starts settled instead of
    /// falling into place on screen.
    pub prewarm_secs: f32,
    /// Speed limit per particle, m/s.
    pub max_speed: f32,
    /// Under water: the lift on it fully submerged, as a fraction of its dry weight. Dry cloth
    /// holds air and floats (above 1) until it soaks through; hair and rope sink slowly.
    pub buoyancy: f32,
    /// Under water: how fast it takes on the current's velocity, 1/s (water is 800 times
    /// denser than air, so cloth and hair drift and sway slowly in it). For cloth this is
    /// moving across its face; sliding along it drags less.
    pub water_drag: f32,
    /// Water it soaks up: soaked through, it weighs (1 + soak) times its dry weight (cotton
    /// about 1.5, hair about 0.5, jelly and plastic 0). It soaks under water and in the rain, wicks
    /// a little way along itself and dries in the wind and warmth.
    pub soak: f32,
}

impl Default for SimSettings {
    fn default() -> Self {
        Self {
            substeps: 10,
            damping: 0.05,
            friction: 0.5,
            thickness: 0.015,
            gravity_scale: 1.0,
            wind_scale: 1.0,
            follow: 0.0,
            collide: true,
            self_collide: false,
            gpu: false,
            prewarm_secs: 0.0,
            max_speed: 60.0,
            buoyancy: 1.6,
            water_drag: 6.0,
            soak: 1.5,
        }
    }
}

/// A procedural pattern printed on cloth (flags, banners), in texture space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pattern {
    /// One color.
    #[default]
    Solid,
    /// `n` horizontal bands alternating `color`, `color2`, `color3`.
    HorizontalStripes(u32),
    /// `n` vertical bands alternating `color`, `color2`, `color3`.
    VerticalStripes(u32),
    /// A Nordic cross in `color2`, outlined in `color3`.
    Cross,
    /// A disc in `color2` in the middle.
    Circle,
    /// An `n`×`n` checkerboard of `color` and `color2`.
    Checker(u32),
    /// A border in `color2` around `color` (trimmed capes, curtains).
    Border,
    /// A diagonal band in `color2`.
    Diagonal,
    /// Three twisted plies (ropes and cables).
    Rope,
    /// Seigaiha, the Japanese "blue sea waves": overlapping fans of arcs in `color2` on
    /// `color`, each fan this many millimeters across.
    Waves(u32),
    /// Plum blossoms about this many millimeters apart: `color2` petals with `color3` hearts
    /// on `color`.
    Blossoms(u32),
    /// Tie-dye dots (kanoko shibori) this many millimeters apart: `color2` rings on `color`.
    Shibori(u32),
}

impl Pattern {
    /// Shader encoding: pattern id and parameter.
    pub fn encode(self) -> (u32, u32) {
        match self {
            Pattern::Solid => (0, 0),
            Pattern::HorizontalStripes(n) => (1, n.max(1)),
            Pattern::VerticalStripes(n) => (2, n.max(1)),
            Pattern::Cross => (3, 0),
            Pattern::Circle => (4, 0),
            Pattern::Checker(n) => (5, n.max(1)),
            Pattern::Border => (6, 0),
            Pattern::Diagonal => (7, 0),
            Pattern::Rope => (8, 0),
            Pattern::Waves(mm) => (9, mm.max(1)),
            Pattern::Blossoms(mm) => (10, mm.max(1)),
            Pattern::Shibori(mm) => (11, mm.max(1)),
        }
    }
}

/// How a soft body looks. Colors are linear RGB (use [`SoftMaterial::hex`] for sRGB hex).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoftMaterial {
    pub color: [f32; 3],
    pub color2: [f32; 3],
    pub color3: [f32; 3],
    pub pattern: Pattern,
    /// Perceptual roughness (0 = mirror, 1 = chalk).
    pub roughness: f32,
    pub metallic: f32,
    /// Fabric sheen: a soft bright rim at grazing angles (velvet 0.8, cotton 0.3, rubber 0).
    pub sheen: f32,
    /// Light passing through thin cloth or flesh (0..1).
    pub subsurface: f32,
    /// Strength of the fine wrinkles that appear where cloth is compressed (0 = none).
    pub wrinkles: f32,
    /// 1 = opaque; lower draws see-through (jelly, slime, sheer curtains).
    pub alpha: f32,
    /// Emitted light, cd/m² (glowing slime).
    pub emissive: [f32; 3],
}

impl Default for SoftMaterial {
    fn default() -> Self {
        Self {
            color: [0.6, 0.6, 0.6],
            color2: [0.9, 0.9, 0.9],
            color3: [0.1, 0.1, 0.1],
            pattern: Pattern::Solid,
            roughness: 0.8,
            metallic: 0.0,
            sheen: 0.3,
            subsurface: 0.3,
            wrinkles: 0.6,
            alpha: 1.0,
            emissive: [0.0; 3],
        }
    }
}

impl SoftMaterial {
    /// A material of sRGB color `rgb` (0xRRGGBB).
    pub fn hex(rgb: u32) -> Self {
        Self {
            color: srgb_hex_to_linear(rgb),
            ..Self::default()
        }
    }

    /// Print `pattern` using sRGB colors `c2` and `c3`.
    pub fn with_pattern(mut self, pattern: Pattern, c2: u32, c3: u32) -> Self {
        self.pattern = pattern;
        self.color2 = srgb_hex_to_linear(c2);
        self.color3 = srgb_hex_to_linear(c3);
        self
    }

    pub fn with_roughness(mut self, r: f32) -> Self {
        self.roughness = r;
        self
    }

    pub fn with_metallic(mut self, m: f32) -> Self {
        self.metallic = m;
        self
    }

    pub fn with_sheen(mut self, s: f32) -> Self {
        self.sheen = s;
        self
    }

    pub fn with_subsurface(mut self, s: f32) -> Self {
        self.subsurface = s;
        self
    }

    pub fn with_alpha(mut self, a: f32) -> Self {
        self.alpha = a;
        self
    }

    pub fn with_wrinkles(mut self, w: f32) -> Self {
        self.wrinkles = w;
        self
    }

    pub fn with_emissive(mut self, rgb: u32, cd_m2: f32) -> Self {
        let c = srgb_hex_to_linear(rgb);
        self.emissive = [c[0] * cd_m2, c[1] * cd_m2, c[2] * cd_m2];
        self
    }
}

/// Module-wide quality and level-of-detail settings.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoftBodyConfig {
    /// Master switch for the simulation (drawing continues).
    pub enabled: bool,
    /// Bodies farther than this from every `SimulationFocus` step every other fixed step, with
    /// half the substeps per second, m.
    pub lod_distance: f32,
    /// Bodies farther than this stop simulating and hold their shape, m.
    pub freeze_distance: f32,
    /// Multiplies every body's substeps (0.5 = faster and softer, 2 = stiffer).
    pub quality: f32,
}

impl Default for SoftBodyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            lod_distance: 30.0,
            freeze_distance: 400.0,
            quality: 1.0,
        }
    }
}
