//! Procedural footsteps and impacts.
//!
//! Every step can be synthesized from a few physically motivated layers, mixed by the
//! surface's [`SurfaceSound`]. Where recorded takes of the surface are fetched
//! ([`crate::samples`]), a footstep plays one of them and the layers only add what the
//! recording lacks ([`FootstepSound::with_sample`]); without them the layers make the whole
//! step. Grass rustles, gravel rattles (each pebble a tiny
//! resonator), snow crunches as it compacts (and squeaks when it is colder than about −5 °C),
//! mud squelches through a closing resonance and pops on lift-off with a suction bubble,
//! puddles splash with bubbles that ring at their Minnaert pitch, planks knock with wood
//! modes, metal grates ring. Every step is different (seeded randomness), so a walk never
//! sounds like a loop.
//!
//! The surface comes from [`engine_world::SurfaceMaterials`] and the ground's live state from
//! [`engine_world::SurfaceLayers`]: rain makes dirt and grass squish, puddles add splashes,
//! fresh snow covers whatever is under it, scorched grass crackles like ash, and frozen mud
//! turns hard.

use std::collections::BTreeMap;

use engine_core::prelude::*;
use engine_world::{ContactKind, Deformation, SurfaceCell, SurfaceMaterial};

use crate::dsp::{Biquad, Bubble, Noise, Resonator, db, decay_coef};
use crate::mixer::Sound;
use crate::samples::{SampleVoice, StepSample};

/// How a surface sounds underfoot: how much of each layer it has (0 = none, 1 = typical) and
/// its tone. Tune these per material in [`FootstepSurfaces`] (by JSON, from the console or an
/// AI agent) to change how the world sounds without touching code.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceSound {
    /// Low body thud of the foot landing.
    pub thump: f32,
    /// Hard heel transient (concrete, stone, ice, planks).
    pub click: f32,
    /// Toe scrape as the foot rolls off.
    pub scuff: f32,
    /// Pebbles rattling (gravel, dirt with stones).
    pub grit: f32,
    /// Dense compaction crunch (snow, glass shards).
    pub crunch: f32,
    /// Blades and leaves brushing (grass, crops).
    pub rustle: f32,
    /// Sparse dry cracks (dead leaves, twigs, ash).
    pub crackle: f32,
    /// Wet resonant squish (mud, soaked soil).
    pub squelch: f32,
    /// Suction pop when the foot pulls out (mud).
    pub suction: f32,
    /// Water: bubbles and spray (puddles, shallows).
    pub splash: f32,
    /// Wooden plank modes.
    pub hollow: f32,
    /// Metal modes (grates, ship decks).
    pub ring: f32,
    /// Fine granular hiss (sand).
    pub hiss: f32,
    /// Squeak of very cold snow (set automatically below about −5 °C).
    pub squeak: f32,
    /// 0 dull .. 1 bright: filters and grain resonances.
    pub brightness: f32,
    /// Pitch of resonant parts (planks, metal, thump): lower for bigger, heavier things.
    pub pitch: f32,
    /// Overall level, dB.
    pub gain_db: f32,
}

impl Default for SurfaceSound {
    fn default() -> Self {
        Self {
            thump: 0.8,
            click: 0.4,
            scuff: 0.3,
            grit: 0.0,
            crunch: 0.0,
            rustle: 0.0,
            crackle: 0.0,
            squelch: 0.0,
            suction: 0.0,
            splash: 0.0,
            hollow: 0.0,
            ring: 0.0,
            hiss: 0.0,
            squeak: 0.0,
            brightness: 0.5,
            pitch: 1.0,
            gain_db: 0.0,
        }
    }
}

macro_rules! surface {
    ($($field:ident: $v:expr),* $(,)?) => {
        SurfaceSound { $($field: $v,)* ..SurfaceSound::SILENT }
    };
}

impl SurfaceSound {
    /// No layers at all (presets start from this).
    pub const SILENT: SurfaceSound = SurfaceSound {
        thump: 0.0,
        click: 0.0,
        scuff: 0.0,
        grit: 0.0,
        crunch: 0.0,
        rustle: 0.0,
        crackle: 0.0,
        squelch: 0.0,
        suction: 0.0,
        splash: 0.0,
        hollow: 0.0,
        ring: 0.0,
        hiss: 0.0,
        squeak: 0.0,
        brightness: 0.5,
        pitch: 1.0,
        gain_db: 0.0,
    };

    /// Built-in sounds for the default [`engine_world::SurfaceMaterials`] names (plus
    /// `gravel`). `None` for names it doesn't know.
    pub fn preset(name: &str) -> Option<Self> {
        Some(match name {
            "default" => SurfaceSound::default(),
            "rock" | "stone" => {
                surface!(thump: 0.6, click: 0.9, scuff: 0.5, grit: 0.25, brightness: 0.75)
            }
            "concrete" | "asphalt" | "tile" => {
                surface!(thump: 0.6, click: 1.0, scuff: 0.6, grit: 0.05, brightness: 0.7)
            }
            "dirt" | "soil" => {
                surface!(thump: 1.0, click: 0.1, scuff: 0.35, grit: 0.35, brightness: 0.35, gain_db: -1.0)
            }
            "grass" => {
                surface!(thump: 0.55, rustle: 1.0, crackle: 0.08, scuff: 0.1, brightness: 0.6, gain_db: -2.0)
            }
            "mud" => {
                surface!(thump: 0.8, squelch: 1.0, suction: 0.9, splash: 0.12, brightness: 0.25, pitch: 0.85)
            }
            "sand" => surface!(thump: 0.55, hiss: 1.0, scuff: 0.25, brightness: 0.4, gain_db: -2.0),
            "snow" => surface!(thump: 0.35, crunch: 1.0, brightness: 0.65, gain_db: -1.0),
            "ice" => {
                surface!(thump: 0.35, click: 1.0, scuff: 0.35, ring: 0.04, brightness: 1.0, pitch: 1.6)
            }
            "water" => surface!(thump: 0.2, splash: 1.0, brightness: 0.5),
            "wood" | "planks" => {
                surface!(thump: 0.45, click: 0.6, hollow: 1.0, scuff: 0.2, brightness: 0.55)
            }
            "leaves" => {
                surface!(thump: 0.4, rustle: 0.7, crackle: 1.0, brightness: 0.7, gain_db: -1.0)
            }
            "metal" => surface!(thump: 0.35, click: 0.8, ring: 1.0, scuff: 0.2, brightness: 0.8),
            "glass" => {
                surface!(thump: 0.3, click: 0.6, crunch: 0.6, ring: 0.3, brightness: 1.0, pitch: 2.5)
            }
            "fabric" | "carpet" => {
                surface!(thump: 0.7, scuff: 0.15, brightness: 0.1, gain_db: -4.0)
            }
            "flesh" => surface!(thump: 0.9, squelch: 0.3, brightness: 0.2, pitch: 0.8),
            "gravel" => surface!(thump: 0.55, grit: 1.0, crunch: 0.2, scuff: 0.2, brightness: 0.6),
            "ash" => surface!(thump: 0.5, crackle: 0.6, hiss: 0.4, grit: 0.2, brightness: 0.4),
            _ => return None,
        })
    }

    /// A best guess from physical properties, for materials other modules add that have no
    /// preset: how it deforms, how dense and how flammable it is.
    pub fn from_material(m: &SurfaceMaterial) -> Self {
        if let Some(p) = Self::preset(&m.name) {
            return p;
        }
        let base = match m.deformation {
            Deformation::Snow => Self::preset("snow"),
            Deformation::Plastic { depth_m } if depth_m >= 0.03 => Self::preset("mud"),
            Deformation::Plastic { .. } => Self::preset("dirt"),
            Deformation::Granular { .. } => Self::preset("sand"),
            Deformation::Flatten { .. } if m.flammability > 0.8 => Self::preset("leaves"),
            Deformation::Flatten { .. } => Self::preset("grass"),
            Deformation::None if m.density > 5_000.0 => Self::preset("metal"),
            Deformation::None if m.flammability > 0.5 && m.density < 1_000.0 => {
                Self::preset("wood")
            }
            Deformation::None if m.friction < 0.15 => Self::preset("ice"),
            Deformation::None if m.density < 600.0 => Self::preset("fabric"),
            Deformation::None => Self::preset("concrete"),
        };
        base.unwrap_or_default()
    }

    /// The 14 layer weights, in field order (thump … squeak).
    fn layers(&self) -> [f32; 14] {
        [
            self.thump,
            self.click,
            self.scuff,
            self.grit,
            self.crunch,
            self.rustle,
            self.crackle,
            self.squelch,
            self.suction,
            self.splash,
            self.hollow,
            self.ring,
            self.hiss,
            self.squeak,
        ]
    }

    fn with_layers(mut self, l: [f32; 14]) -> Self {
        [
            self.thump,
            self.click,
            self.scuff,
            self.grit,
            self.crunch,
            self.rustle,
            self.crackle,
            self.squelch,
            self.suction,
            self.splash,
            self.hollow,
            self.ring,
            self.hiss,
            self.squeak,
        ] = l;
        self
    }

    /// How much of `dry`'s character this sound still has (0..1): the share of `dry`'s layer
    /// weights it keeps. A wet path keeps most of its gravel; frozen mud keeps little of mud.
    pub fn kept(&self, dry: &SurfaceSound) -> f32 {
        let (a, b) = (self.layers(), dry.layers());
        let total: f32 = b.iter().sum();
        if total <= 1e-6 {
            return 1.0;
        }
        a.iter().zip(&b).map(|(x, y)| x.min(*y)).sum::<f32>() / total
    }

    /// The layers to synthesize over a recording of `dry`: everything this sound has beyond
    /// `dry` (rain's squelch, a puddle's splash, snow's squeak), plus `bed` (0..1) of what
    /// they share, for per-step variation. Tone, pitch and gain stay.
    pub fn over_recording(&self, dry: &SurfaceSound, bed: f32) -> SurfaceSound {
        let (a, b) = (self.layers(), dry.layers());
        let mut l = [0.0; 14];
        for k in 0..14 {
            l[k] = (a[k] - b[k]).max(0.0) + a[k].min(b[k]) * bed;
        }
        self.with_layers(l)
    }

    pub fn blend(&self, o: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self {
            thump: l(self.thump, o.thump),
            click: l(self.click, o.click),
            scuff: l(self.scuff, o.scuff),
            grit: l(self.grit, o.grit),
            crunch: l(self.crunch, o.crunch),
            rustle: l(self.rustle, o.rustle),
            crackle: l(self.crackle, o.crackle),
            squelch: l(self.squelch, o.squelch),
            suction: l(self.suction, o.suction),
            splash: l(self.splash, o.splash),
            hollow: l(self.hollow, o.hollow),
            ring: l(self.ring, o.ring),
            hiss: l(self.hiss, o.hiss),
            squeak: l(self.squeak, o.squeak),
            brightness: l(self.brightness, o.brightness),
            pitch: l(self.pitch, o.pitch),
            gain_db: l(self.gain_db, o.gain_db),
        }
    }

    /// Change the sound for the ground's current state: wetness (0..1, scaled by how much the
    /// material soaks up), puddles and snow depth (m), scorch (0..1), air temperature (°C) and
    /// how deep the foot is in standing water (m).
    pub fn with_state(
        mut self,
        cell: &SurfaceCell,
        absorbency: f32,
        temperature_c: f32,
        wading_m: f32,
        snow: &SurfaceSound,
    ) -> Self {
        let w = (cell.wetness * absorbency.clamp(0.0, 1.0) * 1.4).clamp(0.0, 1.0);
        if w > 0.0 {
            self.squelch += 0.55 * w * (1.0 - self.click.min(1.0) * 0.7);
            self.rustle *= 1.0 - 0.45 * w;
            self.crackle *= 1.0 - 0.9 * w;
            self.grit *= 1.0 - 0.4 * w;
            self.hiss *= 1.0 - 0.6 * w;
            self.splash += 0.12 * w * w;
            self.brightness *= 1.0 - 0.3 * w;
        }
        let puddle = smooth(0.002, 0.03, cell.puddle_m);
        if puddle > 0.0 {
            self.splash += 1.1 * puddle;
            self.thump *= 1.0 - 0.3 * puddle;
        }
        if cell.scorch > 0.0 {
            let s = cell.scorch.clamp(0.0, 1.0);
            self.crackle += 0.5 * s;
            self.grit += 0.2 * s;
            self.hiss += 0.3 * s;
            self.rustle *= 1.0 - s;
        }
        // Fresh snow covers whatever is underneath.
        let cover = smooth(0.005, 0.07, cell.snow_m);
        if cover > 0.0 && self.crunch < snow.crunch {
            self = self.blend(snow, cover);
        }
        // Frozen ground: mud and wet soil turn hard.
        if temperature_c < -2.0 && self.squelch > 0.0 {
            let f = smooth(-2.0, -8.0, temperature_c);
            self.click += 0.6 * f * self.squelch;
            self.grit += 0.3 * f * self.squelch;
            self.squelch *= 1.0 - 0.85 * f;
            self.suction *= 1.0 - f;
            self.splash *= 1.0 - f;
        }
        if self.crunch > 0.0 && temperature_c < -4.0 {
            self.squeak = self
                .squeak
                .max(smooth(-4.0, -15.0, temperature_c) * self.crunch);
        }
        if wading_m > 0.01 {
            let k = (wading_m / 0.3).clamp(0.0, 1.0);
            self.splash += 0.6 + 0.9 * k;
            self.thump *= 1.0 - 0.5 * k;
            self.click *= 1.0 - k;
            self.rustle *= 1.0 - k;
        }
        self
    }
}

pub(crate) fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Footstep sound per surface material name. Starts with presets for every built-in
/// material; materials not listed get a guess from their physical properties.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FootstepSurfaces {
    pub surfaces: BTreeMap<String, SurfaceSound>,
}

impl Default for FootstepSurfaces {
    fn default() -> Self {
        let names = [
            "default", "rock", "concrete", "dirt", "grass", "mud", "sand", "snow", "ice", "water",
            "wood", "leaves", "metal", "glass", "fabric", "flesh", "gravel", "ash",
        ];
        Self {
            surfaces: names
                .iter()
                .map(|n| (n.to_string(), SurfaceSound::preset(n).unwrap_or_default()))
                .collect(),
        }
    }
}

impl FootstepSurfaces {
    /// The sound for `material`: its entry if there is one, else a guess from its properties.
    pub fn get(&self, material: &SurfaceMaterial) -> SurfaceSound {
        self.surfaces
            .get(&material.name)
            .copied()
            .unwrap_or_else(|| SurfaceSound::from_material(material))
    }

    pub fn snow(&self) -> SurfaceSound {
        self.surfaces
            .get("snow")
            .copied()
            .or_else(|| SurfaceSound::preset("snow"))
            .unwrap_or_default()
    }
}

/// One footstep (or paw, hoof, wheel bump, body fall) to synthesize.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub surface: SurfaceSound,
    pub kind: ContactKind,
    /// How hard: about 1 for an adult walking, 2 running, 0.3 a cat.
    pub intensity: f32,
    /// Seconds from heel strike to toe strike (shorter when running).
    pub roll: f32,
    pub seed: u64,
}

impl Step {
    /// Intensity and timing from a ground contact's load (kg) and speed (m/s).
    pub fn from_contact(
        surface: SurfaceSound,
        kind: ContactKind,
        load_kg: f32,
        speed: f32,
        seed: u64,
    ) -> Self {
        let weight = (load_kg.max(1.0) / 75.0).sqrt();
        let pace = (speed.max(0.0) / 1.4).clamp(0.0, 3.5);
        let intensity = (weight * (0.6 + 0.4 * pace)).clamp(0.12, 3.0);
        let run = ((speed - 1.6) / 3.4).clamp(0.0, 1.0);
        Self {
            surface,
            kind,
            intensity,
            roll: 0.11 - 0.06 * run,
            seed,
        }
    }

    /// A single hit of `strength` (≈ 1 a dropped crate, 3 a heavy slam).
    pub fn impact(surface: SurfaceSound, strength: f32, seed: u64) -> Self {
        Self {
            surface,
            kind: ContactKind::Body,
            intensity: strength.clamp(0.05, 4.0),
            roll: 0.0,
            seed,
        }
    }
}

// ---------------------------------------------------------------- layers

/// Attack–decay envelope (linear attack, exponential decay).
#[derive(Debug, Clone, Copy, Default)]
struct Env {
    level: f32,
    peak: f32,
    attack_left: u32,
    attack_step: f32,
    decay: f32,
}

impl Env {
    fn trigger(&mut self, fs: f32, peak: f32, attack: f32, t60: f32) {
        let a = (attack * fs).max(1.0) as u32;
        self.peak = self.level.max(peak);
        self.attack_left = a;
        self.attack_step = (self.peak - self.level) / a as f32;
        self.decay = decay_coef(fs, t60);
    }

    #[inline]
    fn next(&mut self) -> f32 {
        if self.attack_left > 0 {
            self.level += self.attack_step;
            self.attack_left -= 1;
        } else {
            self.level *= self.decay;
        }
        self.level
    }

    fn active(&self) -> bool {
        self.attack_left > 0 || self.level > 1e-4
    }
}

struct Thump {
    phase: f32,
    f0: f32,
    f: f32,
    env: Env,
    noise_env: Env,
    lp: Biquad,
    t60: f32,
}

impl Thump {
    fn new(fs: f32, s: &SurfaceSound, kind: ContactKind, rng: &mut Noise) -> Self {
        let body = if kind == ContactKind::Body { 0.6 } else { 1.0 };
        Self {
            phase: 0.0,
            f0: (58.0 + 45.0 * s.brightness) * s.pitch * body * rng.range(0.92, 1.08),
            f: 0.0,
            env: Env::default(),
            noise_env: Env::default(),
            lp: Biquad::lowpass(fs, 180.0 + 260.0 * s.brightness, 0.8),
            t60: (0.16 - 0.08 * s.brightness) * if kind == ContactKind::Body { 1.8 } else { 1.0 },
        }
    }

    fn strike(&mut self, fs: f32, s: f32) {
        self.env.trigger(fs, s, 0.002, self.t60);
        self.noise_env.trigger(fs, s * 0.8, 0.001, self.t60 * 0.6);
        self.f = self.f0 * 1.7;
    }

    #[inline]
    fn next(&mut self, fs: f32, rng: &mut Noise) -> f32 {
        self.f += (self.f0 - self.f) * 0.004;
        self.phase += std::f32::consts::TAU * self.f / fs;
        if self.phase > std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }
        self.env.next() * self.phase.sin() + self.lp.process(rng.white()) * self.noise_env.next()
    }

    fn active(&self) -> bool {
        self.env.active() || self.noise_env.active()
    }
}

/// A short noise burst for transients.
#[derive(Debug, Clone, Copy, Default)]
struct Burst {
    left: u32,
    len: u32,
    amp: f32,
}

impl Burst {
    fn trigger(&mut self, fs: f32, secs: f32, amp: f32) {
        self.len = (secs * fs).max(1.0) as u32;
        self.left = self.len;
        self.amp = amp;
    }

    #[inline]
    fn next(&mut self, rng: &mut Noise) -> f32 {
        if self.left == 0 {
            return 0.0;
        }
        let e = self.left as f32 / self.len as f32;
        self.left -= 1;
        rng.white() * self.amp * e * e
    }
}

struct Click {
    burst: Burst,
    hp: Biquad,
    knock: Resonator,
    knock2: Resonator,
    len: f32,
}

impl Click {
    fn new(fs: f32, s: &SurfaceSound, rng: &mut Noise) -> Self {
        let b = s.brightness;
        Self {
            burst: Burst::default(),
            hp: Biquad::highpass(fs, 700.0 + 3_500.0 * b, 0.7),
            knock: Resonator::new(
                fs,
                (380.0 + 500.0 * b) * s.pitch * rng.range(0.9, 1.1),
                0.035,
            ),
            knock2: Resonator::new(
                fs,
                (1_300.0 + 1_800.0 * b) * s.pitch * rng.range(0.9, 1.1),
                0.018,
            ),
            len: 0.0025 - 0.0012 * b,
        }
    }

    fn strike(&mut self, fs: f32, s: f32) {
        self.burst.trigger(fs, self.len, s);
    }

    #[inline]
    fn next(&mut self, rng: &mut Noise) -> f32 {
        let x = self.burst.next(rng);
        self.hp.process(x) + self.knock.process(x) * 1.1 + self.knock2.process(x) * 0.7
    }

    fn active(&self) -> bool {
        self.burst.left > 0 || !self.knock.quiet() || !self.knock2.quiet()
    }
}

struct Scuff {
    env: Env,
    bp: Biquad,
    hp: Biquad,
    t60: f32,
}

impl Scuff {
    fn new(fs: f32, s: &SurfaceSound, rng: &mut Noise) -> Self {
        Self {
            env: Env::default(),
            bp: Biquad::bandpass(
                fs,
                (1_400.0 + 3_000.0 * s.brightness) * rng.range(0.85, 1.15),
                0.9,
            ),
            hp: Biquad::highpass(fs, 400.0, 0.7),
            t60: rng.range(0.05, 0.11),
        }
    }

    fn strike(&mut self, fs: f32, s: f32) {
        self.env.trigger(fs, s, 0.006, self.t60);
    }

    #[inline]
    fn next(&mut self, rng: &mut Noise) -> f32 {
        let e = self.env.next();
        if e < 1e-5 {
            return 0.0;
        }
        self.bp.process(self.hp.process(rng.white())) * e
    }

    fn active(&self) -> bool {
        self.env.active()
    }
}

/// How a grain layer's density evolves after a strike.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// Builds up as the material compacts, then stops (snow).
    Compact,
    /// Dies away (gravel settling).
    Decay,
    /// Rises and falls (leaves, sand).
    Swell,
}

const GRAIN_SLOTS: usize = 4;

/// Stochastic grains through a small resonant filter bank (Cook's PhISEM idea): snow, gravel,
/// leaves, sand and glass are all "many tiny collisions" with different densities, sizes and
/// resonances.
struct Grains {
    filters: [Biquad; 3],
    tuning: [(f32, f32); 3],
    jitter: f32,
    weights: [f32; 3],
    rate: f32,
    burst: (f32, f32),
    power: f32,
    shape: Shape,
    dur: f32,
    gain: f32,
    // state
    t: u32,
    dur_samples: u32,
    strength: f32,
    slots: [(Burst, usize); GRAIN_SLOTS],
    next_slot: usize,
    running: bool,
}

impl Grains {
    #[allow(clippy::too_many_arguments)]
    fn new(
        fs: f32,
        freqs: [(f32, f32); 3],
        weights: [f32; 3],
        rate: f32,
        burst: (f32, f32),
        power: f32,
        shape: Shape,
        dur: f32,
        gain: f32,
    ) -> Self {
        Self {
            filters: freqs.map(|(f, q)| Biquad::bandpass(fs, f, q)),
            tuning: freqs,
            jitter: 0.25,
            weights,
            rate,
            burst,
            power,
            shape,
            dur,
            gain,
            t: 0,
            dur_samples: 1,
            strength: 0.0,
            slots: [(Burst::default(), 0); GRAIN_SLOTS],
            next_slot: 0,
            running: false,
        }
    }

    fn strike(&mut self, fs: f32, s: f32, dur_scale: f32) {
        self.t = 0;
        self.dur_samples = (self.dur * dur_scale * fs).max(1.0) as u32;
        self.strength = s;
        self.running = true;
    }

    fn density(&self) -> f32 {
        let x = self.t as f32 / self.dur_samples as f32;
        match self.shape {
            Shape::Compact => {
                if x < 1.0 {
                    0.25 + 0.75 * x.powf(0.6)
                } else {
                    (-(x - 1.0) * 25.0).exp()
                }
            }
            Shape::Decay => (-x * 3.0).exp(),
            Shape::Swell => {
                if x < 1.0 {
                    (x * std::f32::consts::PI).sin().max(0.0)
                } else {
                    0.0
                }
            }
        }
    }

    #[inline]
    fn next(&mut self, fs: f32, rng: &mut Noise) -> f32 {
        if self.running {
            let d = self.density();
            if d < 1e-3 && self.t > self.dur_samples {
                self.running = false;
            } else {
                // Poisson arrivals: a grain this sample with probability rate / fs.
                let rate = self.rate * d * (0.6 + 0.4 * self.strength.min(2.0));
                if rng.uniform() * fs < rate {
                    let amp = rng.uniform().powf(self.power) * self.strength;
                    let len = rng.range(self.burst.0, self.burst.1);
                    let slot = &mut self.slots[self.next_slot];
                    slot.0.trigger(fs, len, amp);
                    let k = (rng.next_u32() % 3) as usize;
                    slot.1 = k;
                    // Every pebble or crystal rings a little differently.
                    if self.jitter > 0.0 && amp > 0.05 * self.strength {
                        let (f, q) = self.tuning[k];
                        let j = rng.range(1.0 - self.jitter, 1.0 + self.jitter);
                        self.filters[k].retune(Biquad::bandpass(fs, f * j, q));
                    }
                    self.next_slot = (self.next_slot + 1) % GRAIN_SLOTS;
                }
            }
            self.t += 1;
        }
        let mut x = [0.0f32; 3];
        for (b, f) in &mut self.slots {
            if b.left > 0 {
                x[*f] += b.next(rng);
            }
        }
        let mut y = 0.0;
        for ((f, x), w) in self.filters.iter_mut().zip(x).zip(self.weights) {
            y += f.process(x) * w;
        }
        y * self.gain
    }

    fn active(&self) -> bool {
        self.running || self.slots.iter().any(|(b, _)| b.left > 0)
    }
}

struct Rustle {
    env: Env,
    flutter: Biquad,
    hp: Biquad,
    bp: Biquad,
    body: Biquad,
}

impl Rustle {
    fn new(fs: f32, s: &SurfaceSound, rng: &mut Noise) -> Self {
        Self {
            env: Env::default(),
            flutter: Biquad::lowpass(fs, 35.0, 0.7),
            hp: Biquad::highpass(fs, 1_600.0, 0.7),
            bp: Biquad::bandpass(
                fs,
                (3_200.0 + 2_500.0 * s.brightness) * rng.range(0.85, 1.15),
                0.55,
            ),
            body: Biquad::bandpass(fs, 850.0 * rng.range(0.8, 1.2), 0.8),
        }
    }

    fn strike(&mut self, fs: f32, s: f32, t60: f32) {
        self.env.trigger(fs, s, 0.02, t60);
    }

    #[inline]
    fn next(&mut self, rng: &mut Noise) -> f32 {
        let e = self.env.next();
        let am = 0.25 + 9.0 * self.flutter.process(rng.white()).abs();
        if e < 1e-5 {
            return 0.0;
        }
        let n = rng.white();
        (self.bp.process(self.hp.process(n)) * 1.6 + self.body.process(n) * 0.35) * e * am
    }

    fn active(&self) -> bool {
        self.env.active()
    }
}

struct Squelch {
    env: Env,
    lp: Biquad,
    slap: Burst,
    slap_bp: Biquad,
    t: u32,
    f_start: f32,
    f_end: f32,
    q: f32,
}

impl Squelch {
    fn new(fs: f32, s: &SurfaceSound, rng: &mut Noise) -> Self {
        let f_end = (220.0 + 120.0 * s.brightness) * s.pitch * rng.range(0.85, 1.15);
        Self {
            env: Env::default(),
            lp: Biquad::lowpass(fs, 1_000.0, 6.0),
            slap: Burst::default(),
            slap_bp: Biquad::bandpass(fs, 1_300.0 * rng.range(0.8, 1.2), 1.0),
            t: u32::MAX,
            f_start: f_end * rng.range(3.5, 5.0),
            f_end,
            q: rng.range(5.0, 9.0),
        }
    }

    fn strike(&mut self, fs: f32, s: f32) {
        self.env.trigger(fs, s, 0.006, 0.28);
        self.slap.trigger(fs, 0.012, s * 0.8);
        self.t = 0;
    }

    #[inline]
    fn next(&mut self, fs: f32, rng: &mut Noise) -> f32 {
        if self.t != u32::MAX {
            if self.t.is_multiple_of(16) {
                let tt = self.t as f32 / fs;
                let f = self.f_end + (self.f_start - self.f_end) * (-tt / 0.045).exp();
                self.lp.retune(Biquad::lowpass(fs, f, self.q));
            }
            self.t = self.t.saturating_add(1);
        }
        let e = self.env.next();
        let wet = self.slap_bp.process(self.slap.next(rng));
        if e < 1e-5 {
            return wet;
        }
        self.lp.process(rng.white() * e) * 0.35 + wet
    }

    fn active(&self) -> bool {
        self.env.active() || self.slap.left > 0
    }
}

const MAX_BUBBLES: usize = 24;

/// Bubbles: splashes, mud gloops and suction pops.
struct Bubbles {
    list: Vec<Bubble>,
}

impl Bubbles {
    fn new() -> Self {
        Self {
            list: Vec::with_capacity(MAX_BUBBLES),
        }
    }

    fn add(&mut self, b: Bubble) {
        if self.list.len() < MAX_BUBBLES {
            self.list.push(b);
        } else if let Some(slot) = self.list.iter_mut().find(|b| b.done()) {
            *slot = b;
        }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let mut y = 0.0;
        for b in &mut self.list {
            if !b.done() {
                y += b.process();
            }
        }
        y
    }

    fn active(&self) -> bool {
        self.list.iter().any(|b| !b.done())
    }
}

struct Splash {
    spray: Env,
    spray_bp: Biquad,
    slosh: Env,
    slosh_lp: Biquad,
}

impl Splash {
    fn new(fs: f32, rng: &mut Noise) -> Self {
        Self {
            spray: Env::default(),
            spray_bp: Biquad::bandpass(fs, 2_800.0 * rng.range(0.8, 1.25), 0.7),
            slosh: Env::default(),
            slosh_lp: Biquad::lowpass(fs, 520.0, 0.9),
        }
    }

    /// `bubble_gain` scales the bubbles (the spray and slosh are scaled by the layer weight
    /// where they are mixed).
    fn strike(
        &mut self,
        fs: f32,
        s: f32,
        deep: f32,
        bubble_gain: f32,
        bubbles: &mut Bubbles,
        rng: &mut Noise,
    ) {
        self.spray.trigger(fs, s, 0.002, 0.11);
        self.slosh
            .trigger(fs, s * (0.3 + deep), 0.015, 0.25 + 0.3 * deep);
        let n = (3.0 + 9.0 * s.min(1.5)) as usize;
        for _ in 0..n {
            let r = rng.range(0.0012, 0.006) * (1.0 + deep);
            let amp = rng.uniform().powi(2) * s * 0.5 * bubble_gain;
            bubbles.add(Bubble::new(fs, r, amp, 0.1, rng.range(0.0, 0.09)));
        }
    }

    #[inline]
    fn next(&mut self, rng: &mut Noise) -> f32 {
        let a = self.spray.next();
        let b = self.slosh.next();
        if a < 1e-5 && b < 1e-5 {
            return 0.0;
        }
        let n = rng.white();
        self.spray_bp.process(n) * a * 0.9 + self.slosh_lp.process(n) * b * 0.7
    }

    fn active(&self) -> bool {
        self.spray.active() || self.slosh.active()
    }
}

/// A bank of resonant modes struck by a short burst: planks, metal, hooves.
struct Modes {
    modes: Vec<(Resonator, f32)>,
    burst: Burst,
    lp: Biquad,
}

impl Modes {
    fn new(fs: f32, f0: f32, ratios: &[f32], t60s: &[f32], gains: &[f32], rng: &mut Noise) -> Self {
        let modes = ratios
            .iter()
            .zip(t60s)
            .zip(gains)
            .map(|((r, t), g)| {
                (
                    Resonator::new(
                        fs,
                        f0 * r * rng.range(0.97, 1.03),
                        t * rng.range(0.85, 1.15),
                    ),
                    *g,
                )
            })
            .collect();
        Self {
            modes,
            burst: Burst::default(),
            lp: Biquad::lowpass(fs, 5_000.0, 0.7),
        }
    }

    fn strike(&mut self, fs: f32, s: f32) {
        self.burst.trigger(fs, 0.0015, s);
    }

    #[inline]
    fn next(&mut self, rng: &mut Noise) -> f32 {
        let x = self.lp.process(self.burst.next(rng));
        let mut y = 0.0;
        for (r, g) in &mut self.modes {
            y += r.process(x) * *g;
        }
        y
    }

    fn active(&self) -> bool {
        self.burst.left > 0 || self.modes.iter().any(|(r, _)| !r.quiet())
    }
}

struct Squeak {
    phase: f32,
    f0: f32,
    env: Env,
    wobble: Biquad,
    bp: Biquad,
}

impl Squeak {
    fn new(fs: f32, rng: &mut Noise) -> Self {
        Self {
            phase: 0.0,
            f0: rng.range(650.0, 1_150.0),
            env: Env::default(),
            wobble: Biquad::lowpass(fs, 18.0, 0.7),
            bp: Biquad::bandpass(fs, 1_500.0, 0.8),
        }
    }

    fn strike(&mut self, fs: f32, s: f32, dur: f32) {
        self.env.trigger(fs, s, dur * 0.7, 0.12);
    }

    #[inline]
    fn next(&mut self, fs: f32, rng: &mut Noise) -> f32 {
        let e = self.env.next();
        if e < 1e-5 {
            return 0.0;
        }
        let f = self.f0 * (1.0 + 6.0 * self.wobble.process(rng.white()));
        self.phase = (self.phase + f / fs).fract();
        let p = self.phase * std::f32::consts::TAU;
        let x = p.sin() + 0.5 * (2.0 * p).sin() + 0.33 * (3.0 * p).sin();
        self.bp.process(x) * e
    }

    fn active(&self) -> bool {
        self.env.active()
    }
}

// ---------------------------------------------------------------- the footstep

#[derive(Debug, Clone, Copy, PartialEq)]
enum Strike {
    Heel,
    Toe,
    Lift,
}

/// A synthesized footstep or impact. See the [module docs](self).
pub struct FootstepSound {
    fs: f32,
    rng: Noise,
    s: SurfaceSound,
    kind: ContactKind,
    intensity: f32,
    gain: f32,
    strikes: [(u32, Strike, f32); 4],
    strike_count: usize,
    t: u32,
    end_after: u32,
    max_len: u32,
    thump: Thump,
    click: Click,
    scuff: Scuff,
    grit: Grains,
    crunch: Grains,
    crackle: Grains,
    hiss: Grains,
    rustle: Rustle,
    squelch: Squelch,
    splash: Splash,
    bubbles: Bubbles,
    hollow: Modes,
    ring: Modes,
    squeak: Squeak,
    hp: Biquad,
    sample: Option<SampleVoice>,
    /// How loud the splash's bubbles are: 1 when synthesized whole (the splash layer's weight
    /// only scales its spray), the splash weight over a recording, which has its own.
    splash_bubbles: f32,
}

impl FootstepSound {
    /// A footstep that plays a recorded take, with `step`'s layers synthesized on top (pass
    /// the step [`FootstepSamples::recorded`](crate::samples::FootstepSamples::recorded)
    /// returns, whose layers are only what the recording lacks).
    pub fn with_sample(fs: f32, step: &Step, sample: &StepSample) -> Self {
        let mut me = Self::new(fs, step);
        me.splash_bubbles = step.surface.splash.min(1.0);
        let voice = SampleVoice::new(fs, sample);
        me.max_len = me.max_len.max(voice.remaining() + 64);
        me.sample = Some(voice);
        me
    }

    pub fn new(fs: f32, step: &Step) -> Self {
        let mut rng = Noise::new(step.seed);
        let mut s = step.surface;
        let kind = step.kind;
        let mut hollow_pitch = s.pitch;
        match kind {
            ContactKind::Paw => {
                s.click *= 0.25;
                s.scuff *= 0.3;
                s.thump *= 0.6;
                s.suction *= 0.5;
            }
            ContactKind::Hoof => {
                s.click = s.click.max(0.6) * 1.6;
                s.hollow += 0.6;
                hollow_pitch *= 4.0;
                s.scuff *= 0.5;
            }
            ContactKind::Wheel | ContactKind::Track => {
                s.thump *= 1.4;
                s.grit *= 1.5;
                s.crunch *= 1.2;
                s.click *= 0.4;
                s.rustle *= 0.6;
            }
            ContactKind::Body => {
                s.thump *= 2.0;
                s.scuff *= 0.3;
            }
            ContactKind::Foot => {}
        }
        let b = s.brightness;
        let bright = 0.7 + 0.6 * b;
        let i = step.intensity.max(0.05);
        let roll = (step.roll * rng.range(0.85, 1.15) * fs) as u32;
        let mut strikes = [(0u32, Strike::Heel, 0.0f32); 4];
        let count;
        match kind {
            ContactKind::Foot => {
                strikes[0] = (0, Strike::Heel, 1.0);
                strikes[1] = (roll, Strike::Toe, rng.range(0.45, 0.65));
                strikes[2] = (roll + (rng.range(0.12, 0.2) * fs) as u32, Strike::Lift, 1.0);
                count = 3;
            }
            ContactKind::Paw => {
                let gap = (rng.range(0.03, 0.05) * fs) as u32;
                strikes[0] = (0, Strike::Heel, 0.7);
                strikes[1] = (gap, Strike::Toe, 0.55);
                strikes[2] = (gap + (0.1 * fs) as u32, Strike::Lift, 0.6);
                count = 3;
            }
            ContactKind::Body => {
                strikes[0] = (0, Strike::Heel, 1.0);
                if step.roll > 0.0 || i > 1.0 {
                    strikes[1] = ((rng.range(0.07, 0.11) * fs) as u32, Strike::Toe, 0.3);
                    count = 2;
                } else {
                    count = 1;
                }
            }
            _ => {
                strikes[0] = (0, Strike::Heel, 1.0);
                count = 1;
            }
        }
        let last = strikes[..count].iter().map(|s| s.0).max().unwrap_or(0);
        let body = if kind == ContactKind::Body { 1.6 } else { 1.0 };
        Self {
            thump: Thump::new(fs, &s, kind, &mut rng),
            click: Click::new(fs, &s, &mut rng),
            scuff: Scuff::new(fs, &s, &mut rng),
            grit: Grains::new(
                fs,
                [
                    (1_400.0 * bright, 6.0),
                    (2_600.0 * bright, 7.0),
                    (4_300.0 * bright, 8.0),
                ],
                [1.0, 0.9, 0.7],
                420.0,
                (0.00015, 0.0005),
                2.0,
                Shape::Decay,
                0.13 * body,
                16.0,
            ),
            crunch: Grains::new(
                fs,
                [
                    (1_600.0 * bright * s.pitch.sqrt(), 2.0),
                    (3_200.0 * bright * s.pitch.sqrt(), 2.5),
                    (5_800.0 * bright, 2.0),
                ],
                [0.8, 1.0, 0.6],
                700.0,
                (0.0006, 0.0025),
                2.2,
                Shape::Compact,
                0.14 * body,
                3.0,
            ),
            crackle: Grains::new(
                fs,
                [
                    (2_400.0 * bright, 0.9),
                    (4_800.0 * bright, 1.1),
                    (8_500.0, 1.0),
                ],
                [0.8, 1.0, 0.6],
                170.0,
                (0.0001, 0.0004),
                3.5,
                Shape::Swell,
                0.26 * body,
                3.5,
            ),
            hiss: Grains::new(
                fs,
                [
                    (900.0 * bright, 0.7),
                    (2_200.0 * bright, 0.8),
                    (4_000.0 * bright, 0.8),
                ],
                [0.7, 1.0, 0.5],
                4_000.0,
                (0.0002, 0.0006),
                1.0,
                Shape::Swell,
                0.18 * body,
                1.0,
            ),
            rustle: Rustle::new(fs, &s, &mut rng),
            squelch: Squelch::new(fs, &s, &mut rng),
            splash: Splash::new(fs, &mut rng),
            bubbles: Bubbles::new(),
            hollow: Modes::new(
                fs,
                165.0 * hollow_pitch * rng.range(0.9, 1.1),
                &[1.0, 2.57, 4.21, 6.32],
                &[0.17, 0.11, 0.07, 0.045],
                &[1.0, 0.7, 0.5, 0.35],
                &mut rng,
            ),
            ring: Modes::new(
                fs,
                430.0 * s.pitch * rng.range(0.9, 1.1),
                &[1.0, 2.76, 5.40, 8.93, 13.34, 18.64],
                &[0.36, 0.27, 0.21, 0.15, 0.11, 0.07],
                &[0.5, 1.0, 0.8, 0.6, 0.4, 0.3],
                &mut rng,
            ),
            squeak: Squeak::new(fs, &mut rng),
            hp: Biquad::highpass(fs, 35.0, 0.7),
            sample: None,
            splash_bubbles: 1.0,
            fs,
            gain: db(s.gain_db) * 0.42 * i.powf(0.8) * rng.range(0.85, 1.15),
            rng,
            s,
            kind,
            intensity: i,
            strikes,
            strike_count: count,
            t: 0,
            end_after: last + (0.06 * fs) as u32,
            max_len: last + (2.5 * fs) as u32,
        }
    }

    fn strike(&mut self, which: Strike, k: f32) {
        let fs = self.fs;
        let s = self.s;
        let i = self.intensity;
        let k = k * i.min(2.0).sqrt();
        let rng = &mut self.rng;
        let (heel, toe) = (which == Strike::Heel, which == Strike::Toe);
        if which == Strike::Lift {
            if s.suction > 0.01 {
                let amp = s.suction * rng.range(0.6, 1.0) * 0.6;
                self.bubbles.add(
                    Bubble::new(
                        fs,
                        rng.range(0.004, 0.008) * s.pitch.recip(),
                        amp,
                        rng.range(0.8, 1.6),
                        0.0,
                    )
                    .damped(2.5, fs),
                );
                self.bubbles.add(Bubble::new(
                    fs,
                    rng.range(0.002, 0.004),
                    amp * 0.4,
                    0.6,
                    rng.range(0.01, 0.03),
                ));
                self.squelch.strike(fs, s.suction * 0.35);
            }
            return;
        }
        if s.thump > 0.0 {
            self.thump.strike(fs, k * if heel { 1.0 } else { 0.45 });
        }
        if s.click > 0.0 {
            self.click.strike(fs, k);
        }
        if s.scuff > 0.0 && (toe || self.kind != ContactKind::Foot) {
            self.scuff.strike(fs, k);
        }
        let grain_scale = if heel { 1.0 } else { 0.7 };
        if s.grit > 0.0 {
            self.grit.strike(fs, k, grain_scale);
        }
        if s.crunch > 0.0 {
            self.crunch.strike(fs, k, grain_scale);
            if s.squeak > 0.0 {
                self.squeak.strike(fs, k, 0.13 * grain_scale);
            }
        }
        if s.crackle > 0.0 {
            self.crackle.strike(fs, k, grain_scale);
        }
        if s.hiss > 0.0 {
            self.hiss.strike(fs, k, grain_scale);
        }
        if s.rustle > 0.0 {
            self.rustle.strike(fs, k, if heel { 0.2 } else { 0.16 });
        }
        if s.squelch > 0.0 {
            self.squelch.strike(fs, k);
            if heel {
                let n = (1.0 + 3.0 * s.squelch.min(1.5)) as usize;
                for _ in 0..n {
                    let amp = rng.uniform() * 0.25 * k * s.squelch;
                    self.bubbles.add(
                        Bubble::new(
                            fs,
                            rng.range(0.003, 0.009),
                            amp,
                            0.3,
                            rng.range(0.005, 0.12),
                        )
                        .damped(3.0, fs),
                    );
                }
            }
        }
        if s.splash > 0.0 {
            let deep = (s.splash - 1.0).clamp(0.0, 1.0);
            self.splash
                .strike(fs, k, deep, self.splash_bubbles, &mut self.bubbles, rng);
        }
        if s.hollow > 0.0 {
            self.hollow.strike(fs, k);
        }
        if s.ring > 0.0 {
            self.ring.strike(fs, k);
        }
    }

    fn active(&self) -> bool {
        self.sample.as_ref().is_some_and(|s| s.active())
            || self.thump.active()
            || self.click.active()
            || self.scuff.active()
            || self.grit.active()
            || self.crunch.active()
            || self.crackle.active()
            || self.hiss.active()
            || self.rustle.active()
            || self.squelch.active()
            || self.splash.active()
            || self.bubbles.active()
            || self.hollow.active()
            || self.ring.active()
            || self.squeak.active()
    }
}

impl Sound for FootstepSound {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        let s = self.s;
        for (n, o) in out.iter_mut().enumerate() {
            if self.t >= self.max_len
                || (self.t > self.end_after && self.t.is_multiple_of(64) && !self.active())
            {
                return n;
            }
            for k in 0..self.strike_count {
                let (at, which, strength) = self.strikes[k];
                if at == self.t {
                    self.strike(which, strength);
                }
            }
            let rng = &mut self.rng;
            let mut y = 0.0;
            if s.thump > 0.0 {
                y += self.thump.next(fs, rng) * s.thump * 0.9;
            }
            if s.click > 0.0 {
                y += self.click.next(rng) * s.click * 0.45;
            }
            if s.scuff > 0.0 {
                y += self.scuff.next(rng) * s.scuff * 1.5;
            }
            if s.grit > 0.0 {
                y += self.grit.next(fs, rng) * s.grit;
            }
            if s.crunch > 0.0 {
                y += self.crunch.next(fs, rng) * s.crunch;
            }
            if s.crackle > 0.0 {
                y += self.crackle.next(fs, rng) * s.crackle;
            }
            if s.hiss > 0.0 {
                y += self.hiss.next(fs, rng) * s.hiss;
            }
            if s.rustle > 0.0 {
                y += self.rustle.next(rng) * s.rustle * 0.75;
            }
            if s.squelch > 0.0 || s.suction > 0.0 {
                y += self.squelch.next(fs, rng) * s.squelch.max(s.suction * 0.5) * 1.4;
            }
            if s.splash > 0.0 {
                y += self.splash.next(rng) * s.splash.min(2.0) * 0.6;
            }
            y += self.bubbles.next();
            if s.hollow > 0.0 {
                y += self.hollow.next(rng) * s.hollow * 0.22;
            }
            if s.ring > 0.0 {
                y += self.ring.next(rng) * s.ring * 0.1;
            }
            if s.squeak > 0.0 {
                y += self.squeak.next(fs, rng) * s.squeak * 0.15;
            }
            *o = self.hp.process(y) * self.gain;
            if let Some(sample) = &mut self.sample {
                *o += sample.next();
            }
            self.t += 1;
        }
        out.len()
    }
}

/// Loudness ([`crate::samples::loudness`]) of a synthesized reference step on `surface`: a
/// 75 kg walker at 1.4 m/s, averaged over four steps. Recorded takes are levelled to it, so a
/// surface sounds as loud recorded as synthesized.
pub fn reference_loudness(surface: &SurfaceSound, fs: f32) -> f32 {
    let mut total = 0.0;
    for seed in 1..=4u64 {
        let step = Step::from_contact(*surface, ContactKind::Foot, 75.0, 1.4, seed * 7_919);
        let x = render_mono(&mut FootstepSound::new(fs, &step), 1.0, fs);
        total += crate::samples::loudness(&x, fs);
    }
    total / 4.0
}

/// Render a whole sound into a buffer (tests, tools and offline previews).
pub fn render_mono(sound: &mut dyn Sound, max_secs: f32, fs: f32) -> Vec<f32> {
    let max = (max_secs * fs) as usize;
    let mut out = vec![0.0; max];
    let mut done = 0;
    while done < max {
        let end = (done + 256).min(max);
        let n = sound.fill(&mut out[done..end]);
        done += n;
        if n < end - (done - n) {
            break;
        }
    }
    out.truncate(done);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::SurfaceMaterials;

    const FS: f32 = 48_000.0;

    fn step(name: &str, seed: u64) -> Vec<f32> {
        let s = SurfaceSound::preset(name).unwrap();
        let st = Step::from_contact(s, ContactKind::Foot, 75.0, 1.4, seed);
        render_mono(&mut FootstepSound::new(FS, &st), 3.0, FS)
    }

    /// Energy-weighted mean frequency from zero crossings of the derivative-free signal
    /// (cheap brightness estimate), and fraction of energy below 400 Hz via a low-pass.
    fn brightness(x: &[f32]) -> f32 {
        let energy: f32 = x.iter().map(|v| v * v).sum();
        let diff: f32 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        // For a sine of frequency f: diff/energy ≈ (2π f / fs)².
        (diff / energy.max(1e-12)).sqrt() * FS / std::f32::consts::TAU
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn every_surface_makes_a_sound_of_reasonable_level_and_length() {
        for name in FootstepSurfaces::default().surfaces.keys() {
            let x = step(name, 3);
            let p = peak(&x);
            let len = x.len() as f32 / FS;
            assert!((0.03..0.95).contains(&p), "{name}: peak {p}");
            assert!((0.08..2.6).contains(&len), "{name}: {len} s");
            assert!(x.iter().all(|v| v.is_finite()), "{name}");
        }
    }

    #[test]
    fn grass_mud_and_snow_sound_different() {
        let avg = |name: &str| (0..6).map(|s| brightness(&step(name, s))).sum::<f32>() / 6.0;
        let (grass, mud, snow) = (avg("grass"), avg("mud"), avg("snow"));
        // Mud is dark and wet, grass airy, snow crisp.
        assert!(mud < grass * 0.6, "mud {mud} grass {grass}");
        assert!(mud < snow * 0.6, "mud {mud} snow {snow}");
        // Mud lasts longer (suction pop after lift-off).
        let len = |name: &str| step(name, 1).len();
        assert!(len("mud") > len("concrete"));
    }

    #[test]
    fn steps_vary() {
        let a = step("gravel", 1);
        let b = step("gravel", 2);
        let n = a.len().min(b.len());
        let diff: f32 = a[..n].iter().zip(&b[..n]).map(|(x, y)| (x - y).abs()).sum();
        assert!(diff > 1.0);
        // …and are reproducible.
        assert_eq!(step("gravel", 1), a);
    }

    #[test]
    fn running_is_louder_than_walking_and_heavier_is_louder() {
        let s = SurfaceSound::preset("concrete").unwrap();
        let render = |load: f32, speed: f32| {
            let st = Step::from_contact(s, ContactKind::Foot, load, speed, 9);
            let x = render_mono(&mut FootstepSound::new(FS, &st), 2.0, FS);
            x.iter().map(|v| v * v).sum::<f32>()
        };
        assert!(render(75.0, 5.0) > render(75.0, 1.4) * 1.5);
        assert!(render(150.0, 1.4) > render(40.0, 1.4) * 1.5);
    }

    #[test]
    fn ground_state_changes_the_sound() {
        let snow = SurfaceSound::preset("snow").unwrap();
        let grass = SurfaceSound::preset("grass").unwrap();
        let wet = grass.with_state(
            &SurfaceCell {
                wetness: 1.0,
                ..Default::default()
            },
            0.6,
            15.0,
            0.0,
            &snow,
        );
        assert!(wet.squelch > 0.2 && wet.rustle < grass.rustle);
        let puddle = grass.with_state(
            &SurfaceCell {
                puddle_m: 0.05,
                ..Default::default()
            },
            0.6,
            15.0,
            0.0,
            &snow,
        );
        assert!(puddle.splash > 0.9);
        let snowed = grass.with_state(
            &SurfaceCell {
                snow_m: 0.2,
                ..Default::default()
            },
            0.6,
            -1.0,
            0.0,
            &snow,
        );
        assert!((snowed.crunch - snow.crunch).abs() < 1e-3 && snowed.rustle < 0.01);
        let cold = snow.with_state(&SurfaceCell::default(), 0.2, -15.0, 0.0, &snow);
        assert!(cold.squeak > 0.9);
        let frozen_mud = SurfaceSound::preset("mud").unwrap().with_state(
            &SurfaceCell::default(),
            1.0,
            -10.0,
            0.0,
            &snow,
        );
        assert!(frozen_mud.click > 0.5 && frozen_mud.squelch < 0.2);
    }

    #[test]
    fn unknown_materials_get_a_sensible_guess() {
        let mats = SurfaceMaterials::default();
        let mut slime = mats.get(mats.id("mud").unwrap()).clone();
        slime.name = "slime".into();
        assert_eq!(
            SurfaceSound::from_material(&slime),
            SurfaceSound::preset("mud").unwrap()
        );
        let mut steel = mats.get(mats.id("metal").unwrap()).clone();
        steel.name = "steel_deck".into();
        assert!(SurfaceSound::from_material(&steel).ring > 0.5);
    }

    #[test]
    fn recordings_get_only_the_layers_they_lack() {
        let grass = SurfaceSound::preset("grass").unwrap();
        let snow = SurfaceSound::preset("snow").unwrap();
        // Dry grass over a grass recording: a faint bed of every layer, nothing more.
        let over = grass.over_recording(&grass, 0.1);
        assert!((over.rustle - 0.1).abs() < 1e-6 && over.squelch == 0.0);
        assert_eq!(grass.kept(&grass), 1.0);
        // Wet grass keeps most of its grass and adds squelch.
        let wet = grass.with_state(
            &SurfaceCell {
                wetness: 1.0,
                ..Default::default()
            },
            0.6,
            15.0,
            0.0,
            &snow,
        );
        let over = wet.over_recording(&grass, 0.0);
        assert!(over.squelch > 0.3 && over.rustle == 0.0);
        assert!(
            (0.6..1.0).contains(&wet.kept(&grass)),
            "{}",
            wet.kept(&grass)
        );
        // Frozen mud keeps little of mud.
        let mud = SurfaceSound::preset("mud").unwrap();
        let frozen = mud.with_state(&SurfaceCell::default(), 1.0, -12.0, 0.0, &snow);
        assert!(frozen.kept(&mud) < 0.4, "{}", frozen.kept(&mud));
    }

    #[test]
    fn a_step_with_a_recording_plays_it_and_is_reproducible() {
        use crate::samples::{Gait, StepSample, Take};
        let pcm: Vec<f32> = (0..24_000)
            .map(|i| (i as f32 * 0.05).sin() * (-(i as f32) / 4_000.0).exp())
            .collect();
        let sample = StepSample {
            take: Take {
                pcm: pcm.into(),
                sample_rate: 24_000.0,
                gait: Gait::Walk,
                source: "test".into(),
            },
            speed: 1.0,
            gain: 0.5,
            shelf_db: 0.0,
            lowpass_hz: 20_000.0,
        };
        let silent = Step::from_contact(SurfaceSound::SILENT, ContactKind::Foot, 75.0, 1.4, 3);
        let render = || {
            render_mono(
                &mut FootstepSound::with_sample(FS, &silent, &sample),
                3.0,
                FS,
            )
        };
        let x = render();
        // The take (one second at 24 kHz) plays at the mixer's rate, two seconds of samples.
        assert!(
            (x.len() as f32 / FS - 1.0).abs() < 0.05,
            "{} s",
            x.len() as f32 / FS
        );
        assert!(peak(&x) > 0.3);
        assert_eq!(render(), x);
        // With layers on top it is louder than the take alone.
        let grass = Step::from_contact(
            SurfaceSound::preset("grass").unwrap(),
            ContactKind::Foot,
            75.0,
            1.4,
            3,
        );
        let y = render_mono(
            &mut FootstepSound::with_sample(FS, &grass, &sample),
            3.0,
            FS,
        );
        let e = |v: &[f32]| v.iter().map(|s| s * s).sum::<f32>();
        assert!(e(&y) > e(&x));
    }

    #[test]
    fn paws_hooves_and_bodies_render() {
        let s = SurfaceSound::preset("dirt").unwrap();
        for kind in [
            ContactKind::Paw,
            ContactKind::Hoof,
            ContactKind::Wheel,
            ContactKind::Track,
            ContactKind::Body,
        ] {
            let st = Step::from_contact(s, kind, 300.0, 3.0, 4);
            let x = render_mono(&mut FootstepSound::new(FS, &st), 3.0, FS);
            assert!(peak(&x) > 0.02, "{kind:?}");
        }
    }
}
