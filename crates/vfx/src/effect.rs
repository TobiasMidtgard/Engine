//! Effects as data: what an effect looks like, written as plain JSON.
//!
//! An [`EffectDesc`] is a named list of [`EmitterDesc`] layers (a fireball is a hot core, licking
//! flames, sparks and a smoke tail), plus an optional [`EffectLight`] that lights the
//! surroundings and an optional [`Trail`] ribbon. Every field has a default, so a JSON effect
//! only lists what differs:
//!
//! ```json
//! {
//!   "name": "my.sparks",
//!   "duration": 0.2,
//!   "emitters": [{
//!     "bursts": [{ "count": 80 }],
//!     "shape": { "kind": "point" },
//!     "spread_deg": 70, "speed": [4, 12], "lifetime": [0.4, 0.9],
//!     "gravity": 1, "drag": 0.4, "collide": { "kind": "bounce", "restitution": 0.4 },
//!     "sprite": "spark", "facing": "velocity", "stretch": 0.03,
//!     "color": [[0, [1, 0.7, 0.3, 1]], [1, [1, 0.2, 0.0, 0]]],
//!     "emissive": 40000, "additive": 1
//!   }]
//! }
//! ```
//!
//! Units: meters, seconds, m/s, m/s², degrees in fields named `_deg`, emissive luminance in
//! cd/m² (a candle flame ≈ 10 000, a welding arc ≈ 10⁷), lights in lumens.

use serde::{Deserialize, Serialize};

use crate::trail::Trail;

/// A color gradient over a particle's life (0 = born, 1 = dies), as `[t, [r, g, b, a]]` keys.
///
/// RGB is linear. For glowing particles it is multiplied by [`EmitterDesc::emissive`], so
/// values above 1 make a hotter core. Alpha is coverage: 0 is invisible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Gradient(pub Vec<(f32, [f32; 4])>);

impl Default for Gradient {
    fn default() -> Self {
        Self(vec![
            (0.0, [1.0, 1.0, 1.0, 1.0]),
            (1.0, [1.0, 1.0, 1.0, 0.0]),
        ])
    }
}

impl Gradient {
    /// The same color for the whole life.
    pub fn constant(color: [f32; 4]) -> Self {
        Self(vec![(0.0, color)])
    }

    /// A gradient from keys in any order.
    pub fn new(keys: &[(f32, [f32; 4])]) -> Self {
        let mut keys = keys.to_vec();
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self(keys)
    }

    /// Color at `t` (0..1), linearly interpolated between keys.
    pub fn sample(&self, t: f32) -> [f32; 4] {
        let keys = &self.0;
        let Some(first) = keys.first() else {
            return [1.0; 4];
        };
        if t <= first.0 {
            return first.1;
        }
        for pair in keys.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.0 {
                let f = if b.0 > a.0 {
                    (t - a.0) / (b.0 - a.0)
                } else {
                    1.0
                };
                return std::array::from_fn(|i| a.1[i] + (b.1[i] - a.1[i]) * f);
            }
        }
        keys[keys.len() - 1].1
    }

    /// `N` evenly spaced samples from t = 0 to t = 1 (what the GPU reads).
    pub fn bake<const N: usize>(&self) -> [[f32; 4]; N] {
        std::array::from_fn(|i| self.sample(i as f32 / (N - 1).max(1) as f32))
    }
}

/// A number over a particle's life (0 = born, 1 = dies), as `[t, value]` keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Curve(pub Vec<(f32, f32)>);

impl Default for Curve {
    fn default() -> Self {
        Self(vec![(0.0, 1.0)])
    }
}

impl Curve {
    /// The same value for the whole life.
    pub fn constant(v: f32) -> Self {
        Self(vec![(0.0, v)])
    }

    /// A curve from keys in any order.
    pub fn new(keys: &[(f32, f32)]) -> Self {
        let mut keys = keys.to_vec();
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self(keys)
    }

    /// Value at `t` (0..1), linearly interpolated between keys.
    pub fn sample(&self, t: f32) -> f32 {
        let keys = &self.0;
        let Some(first) = keys.first() else {
            return 1.0;
        };
        if t <= first.0 {
            return first.1;
        }
        for pair in keys.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.0 {
                let f = if b.0 > a.0 {
                    (t - a.0) / (b.0 - a.0)
                } else {
                    1.0
                };
                return a.1 + (b.1 - a.1) * f;
            }
        }
        keys[keys.len() - 1].1
    }

    /// `N` evenly spaced samples from t = 0 to t = 1.
    pub fn bake<const N: usize>(&self) -> [f32; N] {
        std::array::from_fn(|i| self.sample(i as f32 / (N - 1).max(1) as f32))
    }

    /// Largest value of the curve.
    pub fn max(&self) -> f32 {
        self.0.iter().map(|k| k.1).fold(0.0, f32::max)
    }
}

/// Where new particles appear, in the emitter's local space (+Y is "up" / "forward").
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmitShape {
    /// One point. Particles fly along `direction`, randomized by `spread_deg`.
    #[default]
    Point,
    /// Inside a sphere (or on its surface). Particles fly outward from the center.
    Sphere {
        #[serde(default = "one")]
        radius: f32,
        #[serde(default)]
        surface: bool,
    },
    /// The upper half of a sphere (domes, ground bursts). Particles fly outward.
    Hemisphere {
        #[serde(default = "one")]
        radius: f32,
    },
    /// A disc of `radius` at the base; particles fly up inside a cone of `angle_deg` around
    /// `direction` (fountains, jets, flames).
    Cone {
        #[serde(default = "default_cone_angle")]
        angle_deg: f32,
        #[serde(default)]
        radius: f32,
    },
    /// A flat ring in the local XZ plane. Particles fly outward along the ground
    /// (shockwaves, ripples, splash crowns).
    Ring {
        #[serde(default = "one")]
        radius: f32,
        #[serde(default)]
        thickness: f32,
    },
    /// A filled disc in the local XZ plane. Particles fly outward along it.
    Disc {
        #[serde(default = "one")]
        radius: f32,
    },
    /// Inside a box. Particles fly along `direction` with `spread_deg`.
    Box {
        #[serde(default = "half_meter")]
        half_extents: [f32; 3],
    },
    /// Along a line of `length` meters on the local Y axis, centered on the emitter.
    Line {
        #[serde(default = "one")]
        length: f32,
    },
}

fn one() -> f32 {
    1.0
}
fn half_meter() -> [f32; 3] {
    [0.5; 3]
}
fn default_cone_angle() -> f32 {
    25.0
}

impl EmitShape {
    /// GPU code of the shape and its three parameters.
    pub fn gpu(&self) -> [f32; 4] {
        match *self {
            EmitShape::Point => [0.0, 0.0, 0.0, 0.0],
            EmitShape::Sphere { radius, surface } => {
                [1.0, radius, if surface { 1.0 } else { 0.0 }, 0.0]
            }
            EmitShape::Hemisphere { radius } => [2.0, radius, 0.0, 0.0],
            EmitShape::Cone { angle_deg, radius } => {
                [3.0, radius, angle_deg.to_radians().cos(), 0.0]
            }
            EmitShape::Ring { radius, thickness } => [4.0, radius, thickness, 0.0],
            EmitShape::Disc { radius } => [5.0, radius, 0.0, 0.0],
            EmitShape::Box { half_extents } => {
                [6.0, half_extents[0], half_extents[1], half_extents[2]]
            }
            EmitShape::Line { length } => [7.0, length, 0.0, 0.0],
        }
    }

    /// Largest distance from the emitter a new particle can start at, meters.
    pub fn extent(&self) -> f32 {
        match *self {
            EmitShape::Point => 0.0,
            EmitShape::Sphere { radius, .. }
            | EmitShape::Hemisphere { radius }
            | EmitShape::Cone { radius, .. }
            | EmitShape::Disc { radius } => radius,
            EmitShape::Ring { radius, thickness } => radius + thickness,
            EmitShape::Box { half_extents } => len3(half_extents),
            EmitShape::Line { length } => length * 0.5,
        }
    }
}

/// How each particle is drawn (procedural, no textures needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sprite {
    /// A soft round glow with a hot center (magic motes, embers, light orbs).
    #[default]
    Glow,
    /// A thin hot streak; use with `facing: velocity` (sparks, tracers, rain).
    Spark,
    /// A billowing noisy puff, lit by the sun and sky (smoke, steam, mist).
    Smoke,
    /// A licking tongue of flame with a hot core (fire spells, torches).
    Flame,
    /// A shiny water drop with a highlight (splashes, fountains, rain).
    Droplet,
    /// A thin ring (shockwaves, ripples; use with `facing: horizontal` for ripples).
    Ring,
    /// A four-point star flare (glints, arcane sparks, impacts).
    Star,
    /// A rotating magic circle with runes (spell circles; use with `facing: horizontal`).
    Rune,
    /// An angular chunk (rock, wood, ice and glass debris).
    Shard,
    /// A small soft dust puff (footsteps, impacts, sand).
    Dust,
    /// Heat haze: bends the scene behind it and adds nothing of its own (fire, explosions,
    /// engines). Color alpha sets the strength.
    Haze,
    /// A billow of fire: a dense puff glowing from within, hot cracks cooling to dark smoke over
    /// its life (explosions, fire impacts, dragon breath). Use `emissive` and a low `additive`.
    Billow,
}

impl Sprite {
    /// GPU code.
    pub fn code(self) -> f32 {
        self as u32 as f32
    }

    /// How much light passes through it (smoke 1, solid chunks 0): picks how sunlight wraps.
    pub fn translucency(self) -> f32 {
        match self {
            Sprite::Smoke | Sprite::Dust => 1.0,
            Sprite::Droplet => 0.4,
            Sprite::Shard | Sprite::Haze => 0.0,
            _ => 0.6,
        }
    }
}

/// Which way the particle quad faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Facing {
    /// Always faces the camera (most particles).
    #[default]
    Camera,
    /// Faces the camera but stretches along the particle's velocity (sparks, rain, streaks).
    Velocity,
    /// Lies flat, perpendicular to the emitter's up axis (ripples, ground rings, magic circles).
    Horizontal,
}

impl Facing {
    /// GPU code.
    pub fn code(self) -> f32 {
        self as u32 as f32
    }
}

/// What happens when a particle reaches the ground (or a water surface).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Collide {
    /// Pass through everything.
    #[default]
    None,
    /// Bounce off with `restitution` (0 dead stop .. 1 perfect bounce) and lose `friction`
    /// (0..1) of the sliding speed on each contact.
    Bounce {
        #[serde(default = "default_restitution")]
        restitution: f32,
        #[serde(default = "default_friction")]
        friction: f32,
    },
    /// Disappear on contact (rain drops, droplets falling into water).
    Die,
    /// Stop and stay where it landed (embers, debris settling).
    Stick,
}

fn default_restitution() -> f32 {
    0.35
}
fn default_friction() -> f32 {
    0.3
}

impl Collide {
    /// GPU code and parameters.
    pub fn gpu(&self) -> [f32; 3] {
        match *self {
            Collide::None => [0.0, 0.0, 0.0],
            Collide::Bounce {
                restitution,
                friction,
            } => [1.0, restitution, friction],
            Collide::Die => [2.0, 0.0, 0.0],
            Collide::Stick => [3.0, 0.0, 0.0],
        }
    }
}

/// A burst of particles at a moment of the emitter's life.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Burst {
    /// Seconds after the emitter starts.
    pub time: f32,
    /// Particles in the burst.
    pub count: u32,
    /// Repeat every this many seconds (0 = once).
    pub repeat: f32,
    /// How many times it fires when repeating (0 = forever).
    pub cycles: u32,
}

impl Default for Burst {
    fn default() -> Self {
        Self {
            time: 0.0,
            count: 10,
            repeat: 0.0,
            cycles: 1,
        }
    }
}

impl Burst {
    /// Total particles this burst can make (repeating forever counts one cycle).
    pub fn total(&self) -> u32 {
        if self.repeat > 0.0 && self.cycles > 1 {
            self.count.saturating_mul(self.cycles)
        } else {
            self.count
        }
    }
}

/// One layer of an effect: a population of particles with its own spawning, motion and look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmitterDesc {
    /// Optional label ("core", "sparks", "smoke").
    pub name: String,

    // ---- spawning
    /// Particles per second while emitting.
    pub rate: f32,
    /// Extra particles per meter the emitter travels (dense trails behind fast projectiles).
    pub rate_per_meter: f32,
    /// Bursts at given moments.
    pub bursts: Vec<Burst>,
    /// Seconds after the effect starts before this layer begins.
    pub delay: f32,
    /// Seconds this layer emits for (none = as long as the effect plays).
    pub duration: Option<f32>,
    /// Most particles alive at once (0 = worked out from rate, bursts and lifetime).
    pub max_particles: u32,

    // ---- birth
    /// Where particles start.
    pub shape: EmitShape,
    /// Local offset of the shape from the emitter, meters.
    pub offset: [f32; 3],
    /// Main direction, local space (default up, +Y).
    pub direction: [f32; 3],
    /// Random spread around `direction` for point, box and line shapes, degrees (180 = all
    /// directions).
    pub spread_deg: f32,
    /// Speed along the shape's direction at birth, m/s (random in range).
    pub speed: [f32; 2],
    /// Extra velocity at birth, local space, m/s.
    pub velocity: [f32; 3],
    /// Speed around the local up axis at birth, m/s (swirls, vortices, auras).
    pub orbit_speed: f32,
    /// Fraction of the emitter's own velocity particles keep (0..1).
    pub inherit_velocity: f32,
    /// Life of each particle, seconds (random in range).
    pub lifetime: [f32; 2],
    /// Diameter at birth, meters (random in range).
    pub size: [f32; 2],
    /// Starting rotation, degrees (random in range).
    pub rotation_deg: [f32; 2],
    /// Spin, degrees per second (random in range).
    pub spin_deg: [f32; 2],

    // ---- motion
    /// Multiplier on world gravity (1 = falls like a stone, 0 = floats, negative = rises).
    pub gravity: f32,
    /// Constant acceleration, m/s², as on flat ground in sea-level air (buoyancy of flames and
    /// smoke: `[0, 3, 0]`). Its +Y turns to the local up on a planet, and it scales with the
    /// air's density, so smoke barely rises in thin air and not at all in a vacuum.
    pub acceleration: [f32; 3],
    /// How fast particles take on the motion of the surrounding air, per second, in sea-level
    /// air (0 = no air resistance, 1 = embers, 4 = light smoke). Scales with the air's density,
    /// as does `turbulence`.
    pub drag: f32,
    /// How much of the wind the surrounding air carries for this layer (0..1).
    pub wind: f32,
    /// Strength of swirling turbulence, m/s².
    pub turbulence: f32,
    /// Size of the turbulence swirls, meters.
    pub turbulence_scale: f32,
    /// Tangential acceleration around the emitter's up axis, m/s² (whirlwinds, portals).
    pub vortex: f32,
    /// Pull toward the emitter, m/s² (negative pushes away). Charging spells, black holes.
    pub attract: f32,
    /// Ground and water collision.
    pub collide: Collide,
    /// Particles move with the emitter instead of being left behind (auras, orbiting runes).
    pub local_space: bool,

    // ---- look
    /// Procedural sprite.
    pub sprite: Sprite,
    /// Which way the quad faces.
    pub facing: Facing,
    /// Streak length per m/s of speed, seconds (velocity facing; 0.02 = a 20 m/s spark is 0.4 m).
    pub stretch: f32,
    /// Color over life (see [`Gradient`]).
    pub color: Gradient,
    /// Size multiplier over life.
    pub size_over_life: Curve,
    /// Glow: luminance for color 1.0, cd/m² (0 = does not glow).
    pub emissive: f32,
    /// How much scene light (sun, sky, lamps) lights the particle (0 unlit .. 1 fully lit; the
    /// color is then its albedo).
    pub lit: f32,
    /// 0 = alpha blended (smoke, dust, droplets) .. 1 = additive (fire, magic, sparks).
    pub additive: f32,
    /// Fade out this close to scene geometry, meters (soft particles; 0 = hard edges).
    pub soft: f32,
}

impl Default for EmitterDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            rate: 0.0,
            rate_per_meter: 0.0,
            bursts: Vec::new(),
            delay: 0.0,
            duration: None,
            max_particles: 0,
            shape: EmitShape::Point,
            offset: [0.0; 3],
            direction: [0.0, 1.0, 0.0],
            spread_deg: 0.0,
            speed: [1.0, 1.0],
            velocity: [0.0; 3],
            orbit_speed: 0.0,
            inherit_velocity: 0.0,
            lifetime: [1.0, 1.0],
            size: [0.1, 0.1],
            rotation_deg: [0.0, 360.0],
            spin_deg: [0.0, 0.0],
            gravity: 0.0,
            acceleration: [0.0; 3],
            drag: 0.0,
            wind: 1.0,
            turbulence: 0.0,
            turbulence_scale: 1.0,
            vortex: 0.0,
            attract: 0.0,
            collide: Collide::None,
            local_space: false,
            sprite: Sprite::Glow,
            facing: Facing::Camera,
            stretch: 0.0,
            color: Gradient::default(),
            size_over_life: Curve::default(),
            emissive: 0.0,
            lit: 0.0,
            additive: 0.0,
            soft: 0.2,
        }
    }
}

/// Assumed top speed of a moving emitter when sizing per-meter spawning, m/s.
const ASSUMED_EMITTER_SPEED: f32 = 30.0;

impl EmitterDesc {
    /// Longest a particle lives, seconds.
    pub fn max_lifetime(&self) -> f32 {
        self.lifetime[0].max(self.lifetime[1]).max(0.0)
    }

    /// Particles this layer needs room for at once.
    pub fn capacity(&self, rate_scale: f32) -> u32 {
        if self.max_particles > 0 {
            return self.max_particles;
        }
        let life = self.max_lifetime();
        let steady = (self.rate + self.rate_per_meter * ASSUMED_EMITTER_SPEED) * life * rate_scale;
        let bursts: u32 = self.bursts.iter().map(Burst::total).sum();
        let bursts = (bursts as f32 * rate_scale).ceil() as u32;
        let n = (steady * 1.15).ceil() as u32 + bursts;
        n.clamp(16, 1 << 22).next_multiple_of(64)
    }

    /// How far particles get from the emitter, meters (for culling).
    pub fn reach(&self) -> f32 {
        let life = self.max_lifetime();
        let speed = self.speed[0].abs().max(self.speed[1].abs())
            + len3(self.velocity)
            + self.orbit_speed.abs();
        let accel = len3(self.acceleration)
            + self.gravity.abs() * 9.81
            + self.turbulence.abs()
            + self.vortex.abs();
        let size = self.size[0].max(self.size[1]) * self.size_over_life.max().max(1.0);
        let travel = speed * life + 0.5 * accel * life * life;
        // Drag caps how far particles coast; wind can carry them further.
        let travel = if self.drag > 0.0 {
            travel.min(speed / self.drag + accel * life) + 12.0 * self.wind * life
        } else {
            travel
        };
        self.shape.extent() + len3(self.offset) + travel + size
    }

    /// Check values that would make the layer invisible or broken. Returns a sentence saying
    /// what to fix.
    pub fn validate(&self) -> Result<(), String> {
        let label = if self.name.is_empty() {
            "an emitter".to_string()
        } else {
            format!("emitter \"{}\"", self.name)
        };
        if !(self.lifetime[0] > 0.0 && self.lifetime[1] > 0.0) {
            return Err(format!(
                "{label} has lifetime {:?}; both values must be above 0 seconds.",
                self.lifetime
            ));
        }
        if self.size[0] < 0.0 || self.size[1] < 0.0 {
            return Err(format!(
                "{label} has a negative size {:?}; sizes are diameters in meters.",
                self.size
            ));
        }
        if self.rate < 0.0 || self.rate_per_meter < 0.0 {
            return Err(format!("{label} has a negative spawn rate."));
        }
        if self.rate == 0.0 && self.rate_per_meter == 0.0 && self.bursts.is_empty() {
            return Err(format!(
                "{label} never spawns anything; give it a rate, rate_per_meter or bursts."
            ));
        }
        if self.color.0.is_empty() {
            return Err(format!("{label} has an empty color gradient."));
        }
        let all = [
            self.rate,
            self.rate_per_meter,
            self.gravity,
            self.drag,
            self.wind,
            self.turbulence,
            self.turbulence_scale,
            self.vortex,
            self.attract,
            self.stretch,
            self.emissive,
            self.lit,
            self.additive,
            self.soft,
            self.spread_deg,
            self.orbit_speed,
            self.inherit_velocity,
        ];
        if all.iter().any(|v| !v.is_finite()) {
            return Err(format!("{label} has a value that is not a finite number."));
        }
        Ok(())
    }
}

fn len3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// A point light that follows an effect and lights its surroundings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectLight {
    /// Linear RGB.
    pub color: [f32; 3],
    /// Luminous power, lumens (candle ≈ 12, campfire ≈ 2 000, a fireball ≈ 20 000).
    pub intensity_lm: f32,
    /// No light beyond this distance, meters.
    pub range: f32,
    /// Random flicker, 0 (steady) .. 1 (wild).
    pub flicker: f32,
    /// Local offset from the effect, meters.
    pub offset: [f32; 3],
    /// Seconds to fade out after the effect stops emitting.
    pub fade_secs: f32,
}

impl Default for EffectLight {
    fn default() -> Self {
        Self {
            color: [1.0, 0.6, 0.3],
            intensity_lm: 4000.0,
            range: 12.0,
            flicker: 0.2,
            offset: [0.0; 3],
            fade_secs: 0.4,
        }
    }
}

/// A complete effect: emitter layers plus an optional light and trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectDesc {
    /// Name used to spawn it (`"spell.fireball"`).
    pub name: String,
    /// One line for humans and AI agents.
    pub description: String,
    /// Seconds the effect emits (none = until stopped). One-shot effects (impacts, splashes)
    /// set this and are removed once their last particle dies.
    pub duration: Option<f32>,
    /// Seconds to simulate ahead when it starts, so ambient effects (smoke columns, fountains)
    /// appear fully formed instead of growing from nothing.
    pub prewarm: f32,
    pub emitters: Vec<EmitterDesc>,
    pub light: Option<EffectLight>,
    /// A ribbon trail behind the effect while it moves.
    pub trail: Option<Trail>,
}

impl Default for EffectDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            duration: None,
            prewarm: 0.0,
            emitters: Vec::new(),
            light: None,
            trail: None,
        }
    }
}

impl EffectDesc {
    /// Check every layer. Returns a sentence saying what to fix.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("The effect needs a \"name\", like \"spell.fireball\".".into());
        }
        if self.emitters.is_empty() && self.light.is_none() && self.trail.is_none() {
            return Err(format!(
                "Effect \"{}\" has no emitters, light or trail, so it would show nothing.",
                self.name
            ));
        }
        if let Some(d) = self.duration
            && !(d >= 0.0 && d.is_finite())
        {
            return Err(format!(
                "Effect \"{}\" has duration {d}; use seconds (0 or more) or leave it out to loop.",
                self.name
            ));
        }
        if !(self.prewarm >= 0.0 && self.prewarm <= 30.0) {
            return Err(format!(
                "Effect \"{}\" has prewarm {}; use 0 to 30 seconds.",
                self.name, self.prewarm
            ));
        }
        for e in &self.emitters {
            e.validate()
                .map_err(|err| format!("Effect \"{}\": {err}", self.name))?;
        }
        Ok(())
    }

    /// True if the effect stops by itself (has a duration).
    pub fn is_one_shot(&self) -> bool {
        self.duration.is_some()
    }

    /// Seconds from start until the last particle can still be alive (none = loops).
    pub fn total_secs(&self) -> Option<f32> {
        let d = self.duration?;
        let tail = self
            .emitters
            .iter()
            .map(|e| e.delay + e.duration.unwrap_or(d).min(d) + e.max_lifetime())
            .fold(0.0, f32::max);
        Some(tail.max(d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_interpolates_and_bakes() {
        let g = Gradient::new(&[(1.0, [0.0, 0.0, 0.0, 0.0]), (0.0, [1.0, 2.0, 4.0, 1.0])]);
        assert_eq!(g.sample(-1.0), [1.0, 2.0, 4.0, 1.0]);
        assert_eq!(g.sample(0.5), [0.5, 1.0, 2.0, 0.5]);
        assert_eq!(g.sample(2.0), [0.0; 4]);
        let baked: [[f32; 4]; 3] = g.bake();
        assert_eq!(baked[1], [0.5, 1.0, 2.0, 0.5]);
        assert_eq!(Gradient::constant([0.1; 4]).sample(0.7), [0.1; 4]);
    }

    #[test]
    fn curve_interpolates_and_bakes() {
        let c = Curve::new(&[(0.0, 0.0), (0.25, 1.0), (1.0, 0.0)]);
        assert!((c.sample(0.125) - 0.5).abs() < 1e-6);
        assert!((c.sample(0.625) - 0.5).abs() < 1e-6);
        let baked: [f32; 5] = c.bake();
        for (got, want) in baked.iter().zip([0.0, 1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0]) {
            assert!((got - want).abs() < 1e-6, "{baked:?}");
        }
        assert_eq!(c.max(), 1.0);
    }

    #[test]
    fn json_with_only_some_fields_parses() {
        let json = r#"{
            "name": "test.sparks",
            "duration": 0.2,
            "emitters": [{
                "bursts": [{ "count": 80 }],
                "shape": { "kind": "cone", "angle_deg": 40 },
                "speed": [4, 12], "lifetime": [0.4, 0.9],
                "collide": { "kind": "bounce" },
                "sprite": "spark", "facing": "velocity",
                "color": [[0, [1, 0.7, 0.3, 1]], [1, [1, 0.2, 0.0, 0]]],
                "emissive": 40000, "additive": 1
            }]
        }"#;
        let e: EffectDesc = serde_json::from_str(json).unwrap();
        e.validate().unwrap();
        let em = &e.emitters[0];
        assert_eq!(em.bursts[0].count, 80);
        assert_eq!(em.bursts[0].cycles, 1);
        assert_eq!(
            em.shape,
            EmitShape::Cone {
                angle_deg: 40.0,
                radius: 0.0
            }
        );
        assert_eq!(
            em.collide,
            Collide::Bounce {
                restitution: 0.35,
                friction: 0.3
            }
        );
        assert_eq!(em.sprite, Sprite::Spark);
        assert_eq!(em.direction, [0.0, 1.0, 0.0]);
        assert!(e.is_one_shot());
        // Round trip.
        let back: EffectDesc = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn validation_explains_problems() {
        let mut e = EffectDesc {
            name: "x".into(),
            emitters: vec![EmitterDesc {
                rate: 10.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        e.validate().unwrap();
        e.emitters[0].lifetime = [0.0, 1.0];
        assert!(e.validate().unwrap_err().contains("lifetime"));
        e.emitters[0].lifetime = [1.0, 1.0];
        e.emitters[0].rate = 0.0;
        assert!(e.validate().unwrap_err().contains("never spawns"));
        e.emitters.clear();
        assert!(e.validate().unwrap_err().contains("show nothing"));
        e.name.clear();
        assert!(e.validate().unwrap_err().contains("name"));
    }

    #[test]
    fn capacity_covers_rate_times_lifetime_plus_bursts() {
        let e = EmitterDesc {
            rate: 100.0,
            lifetime: [0.5, 2.0],
            bursts: vec![Burst {
                count: 50,
                ..Default::default()
            }],
            ..Default::default()
        };
        let c = e.capacity(1.0);
        assert!(c >= 200 + 50, "{c}");
        assert_eq!(c % 64, 0);
        assert!(e.capacity(2.0) > c);
        let fixed = EmitterDesc {
            max_particles: 77,
            ..e
        };
        assert_eq!(fixed.capacity(1.0), 77);
    }

    #[test]
    fn total_secs_includes_the_last_particle() {
        let e = EffectDesc {
            name: "x".into(),
            duration: Some(1.0),
            emitters: vec![EmitterDesc {
                rate: 1.0,
                delay: 0.5,
                lifetime: [1.0, 3.0],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(e.total_secs(), Some(4.5));
        assert_eq!(
            EffectDesc {
                duration: None,
                ..e
            }
            .total_secs(),
            None
        );
    }
}
