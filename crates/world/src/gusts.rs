//! Gust waves: wind fronts you can watch roll across a field and into the trees.
//!
//! [`WindField`](crate::WindField) carries two travelling patterns, a broad one and a finer one
//! across the ground ([`GustWaves`]), as a handful of numbers. Everything that moves in the wind
//! evaluates the same function of them, on the CPU ([`GustWaves::value`]) or in a shader
//! ([`GUST_WAVES_WGSL`]), so grass, bushes, trees, cloth, particles and water ripples lean
//! together and a gust can be followed from the meadow into the forest (Ghost of Tsushima's
//! wind, GDC 2021). The weather writes them (direction and speed from the prevailing wind,
//! strength from its gustiness and storm step) and advances their phase each step with
//! [`GustWaves::advance`]; with `strength` 0 (the default) they add nothing.
//!
//! Positions are meters from [`WindField::gust_origin`](crate::WindField::gust_origin) on the
//! ground plane (world X, Z), which world shifts keep near the focus, so `f32` holds them.

use engine_core::math::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// Lateral wavelengths per front wavelength: how far along a front its bends and its strength
/// vary.
pub const GUST_LATERAL: f32 = 2.7;

/// One travelling pattern of wind fronts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GustWave {
    /// Which way the fronts travel: a unit vector on the ground (world X, Z).
    pub dir: Vec2,
    /// Distance from one front to the next, meters.
    pub wavelength_m: f32,
    /// Extra wind on a front, as a share of the wind there (0.4: a front blows 1.4 times as
    /// hard). 0 turns the pattern off.
    pub strength: f32,
    /// How far the fronts have travelled, in wavelengths (wraps at 1).
    pub phase: f32,
    /// How far the pattern has slid along its fronts, in lateral wavelengths (wraps at 1), so
    /// crests bend and wander instead of marching as straight lines.
    pub drift: f32,
}

impl Default for GustWave {
    fn default() -> Self {
        Self {
            dir: Vec2::X,
            wavelength_m: 60.0,
            strength: 0.0,
            phase: 0.0,
            drift: 0.0,
        }
    }
}

impl GustWave {
    /// Extra wind share (0..`strength`) at `x`, meters from the gust origin on the ground.
    /// [`GUST_WAVES_WGSL`]'s `gust_wave` is the same function.
    pub fn value(&self, x: Vec2) -> f32 {
        if self.strength <= 0.0 || self.wavelength_m <= 0.0 {
            return 0.0;
        }
        let tau = std::f32::consts::TAU;
        let inv = 1.0 / self.wavelength_m;
        let perp = Vec2::new(-self.dir.y, self.dir.x);
        let along = x.dot(self.dir) * inv - self.phase;
        let across = x.dot(perp) * inv / GUST_LATERAL - self.drift;
        // Fronts bend along their length, are narrow with long lulls between them, and blow
        // harder in some stretches than others.
        let s = along + 0.35 * (tau * across).sin();
        let crest = 0.5 + 0.5 * (tau * s).cos();
        let front = crest * crest * crest;
        let reach = 0.6 + 0.4 * (0.5 + 0.5 * (tau * (2.0 * across + 0.25)).sin());
        self.strength * front * reach
    }

    /// Move the fronts on by `dt` seconds at `speed` m/s along `dir`, the pattern sliding along
    /// them at a twentieth of that.
    pub fn advance(&mut self, dt: f32, speed: f32) {
        if self.wavelength_m <= 0.0 {
            return;
        }
        let d = speed * dt / self.wavelength_m;
        self.phase = (self.phase + d).rem_euclid(1.0);
        self.drift = (self.drift + 0.05 * d / GUST_LATERAL).rem_euclid(1.0);
    }

    /// For a shader: `[dir.x, dir.y, 1 / wavelength, strength]` and `[phase, drift, 0, 0]`.
    pub fn to_gpu(&self) -> [[f32; 4]; 2] {
        let inv = if self.wavelength_m > 0.0 {
            1.0 / self.wavelength_m
        } else {
            0.0
        };
        [
            [self.dir.x, self.dir.y, inv, self.strength.max(0.0)],
            [self.phase, self.drift, 0.0, 0.0],
        ]
    }
}

/// The broad gust pattern and a finer one across the ground.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GustWaves {
    /// Big fronts, tens of meters apart: a field bowing all at once, a forest edge leaning.
    pub broad: GustWave,
    /// Small cat's-paws a few meters across, riding on the broad fronts.
    pub fine: GustWave,
}

impl Default for GustWaves {
    fn default() -> Self {
        Self {
            broad: GustWave::default(),
            fine: GustWave {
                wavelength_m: 11.0,
                ..Default::default()
            },
        }
    }
}

impl GustWaves {
    /// Extra wind share at `x` (meters from the gust origin on the ground): 0 between fronts,
    /// up to `broad.strength + fine.strength` where they meet. Multiply the wind by `1 + value`.
    pub fn value(&self, x: Vec2) -> f32 {
        self.broad.value(x) + self.fine.value(x)
    }

    /// Point both patterns down `wind` (m/s; kept as it was when nearly calm) and move them on
    /// by `dt` seconds at its speed (the fine pattern a little faster, as gusts near the ground
    /// outrun the mean wind's front).
    pub fn advance(&mut self, dt: f32, wind: Vec3) {
        let flat = Vec2::new(wind.x, wind.z);
        let speed = flat.length();
        if speed > 0.05 {
            let dir = flat / speed;
            self.broad.dir = dir;
            self.fine.dir = dir;
        }
        self.broad.advance(dt, speed);
        self.fine.advance(dt, speed * 1.3);
    }

    /// Both patterns for a shader, in the order [`GUST_WAVES_WGSL`]'s `GustWaves` reads them.
    pub fn to_gpu(&self) -> [[f32; 4]; 4] {
        let [a, b] = self.broad.to_gpu();
        let [c, d] = self.fine.to_gpu();
        [a, b, c, d]
    }
}

/// [`GustWaves::value`] in WGSL, for every shader that moves something in the wind. Bind the
/// four vectors of [`GustWaves::to_gpu`] as a `GustWaves` and pass positions in meters from
/// the gust origin on the ground (world X, Z), e.g. `world.xz - gust_origin.xz` computed with
/// the camera's offset on the CPU so it stays small.
pub const GUST_WAVES_WGSL: &str = r#"
struct GustWaves {
    broad_a: vec4<f32>, // dir.xy, 1 / wavelength, strength
    broad_b: vec4<f32>, // phase, drift
    fine_a: vec4<f32>,
    fine_b: vec4<f32>,
};

const GUST_TAU: f32 = 6.28318530718;
const GUST_LATERAL: f32 = 2.7;

fn gust_wave(a: vec4<f32>, b: vec4<f32>, x: vec2<f32>) -> f32 {
    if (a.w <= 0.0 || a.z <= 0.0) {
        return 0.0;
    }
    let perp = vec2<f32>(-a.y, a.x);
    let along = dot(x, a.xy) * a.z - b.x;
    let across = dot(x, perp) * a.z / GUST_LATERAL - b.y;
    let s = along + 0.35 * sin(GUST_TAU * across);
    let crest = 0.5 + 0.5 * cos(GUST_TAU * s);
    let front = crest * crest * crest;
    let reach = 0.6 + 0.4 * (0.5 + 0.5 * sin(GUST_TAU * (2.0 * across + 0.25)));
    return a.w * front * reach;
}

fn gust_waves(g: GustWaves, x: vec2<f32>) -> f32 {
    return gust_wave(g.broad_a, g.broad_b, x) + gust_wave(g.fine_a, g.fine_b, x);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn breezy() -> GustWaves {
        let mut g = GustWaves::default();
        g.broad.strength = 0.5;
        g.fine.strength = 0.2;
        g
    }

    #[test]
    fn off_by_default() {
        let g = GustWaves::default();
        for x in [Vec2::ZERO, Vec2::new(13.0, -40.0), Vec2::new(900.0, 3.0)] {
            assert_eq!(g.value(x), 0.0);
        }
    }

    #[test]
    fn fronts_are_narrow_and_stay_within_their_strength() {
        let g = breezy();
        let (mut lo, mut hi, mut calm) = (f32::MAX, 0.0f32, 0);
        let n = 2000;
        for i in 0..n {
            let x = Vec2::new(i as f32 * 0.37, (i % 37) as f32 * 3.1);
            let v = g.value(x);
            lo = lo.min(v);
            hi = hi.max(v);
            if v < 0.1 * 0.7 {
                calm += 1;
            }
        }
        assert!(lo >= 0.0 && hi <= 0.7 + 1e-5, "{lo} {hi}");
        assert!(hi > 0.4, "fronts reach most of their strength: {hi}");
        assert!(calm > n / 3, "long lulls between fronts: {calm} of {n}");
    }

    #[test]
    fn a_front_travels_with_the_wind() {
        let mut g = breezy();
        g.fine.strength = 0.0;
        let wind = Vec3::new(8.0, 0.0, 0.0);
        // Find a front on the line z = 0.
        let front = |g: &GustWaves| {
            (0..600)
                .map(|i| i as f32 * 0.1)
                .max_by(|a, b| {
                    g.value(Vec2::new(*a, 0.0))
                        .total_cmp(&g.value(Vec2::new(*b, 0.0)))
                })
                .unwrap()
        };
        g.advance(0.0, wind);
        let before = front(&g);
        g.advance(1.0, wind);
        let after = front(&g);
        // Eight meters on in a second (the crest bends a little as the pattern slides).
        assert!(((after - before) - 8.0).abs() < 1.0, "{before} -> {after}");
        assert_eq!(g.broad.dir, Vec2::X);
    }

    #[test]
    fn phases_wrap_without_a_jump() {
        let mut a = breezy();
        a.broad.phase = 0.999;
        a.broad.drift = 0.25;
        let mut b = a;
        b.broad.phase -= 1.0;
        b.broad.drift += 1.0;
        for x in [
            Vec2::new(3.0, 7.0),
            Vec2::new(-40.0, 12.5),
            Vec2::new(100.0, -80.0),
        ] {
            assert!((a.value(x) - b.value(x)).abs() < 1e-4);
        }
        a.advance(1000.0, Vec3::new(3.0, 0.0, 4.0));
        assert!((0.0..1.0).contains(&a.broad.phase) && (0.0..1.0).contains(&a.fine.drift));
    }

    /// The WGSL is valid and reads its inputs in the order `to_gpu` writes them.
    #[test]
    fn the_shader_function_compiles() {
        let src = format!(
            "{GUST_WAVES_WGSL}
            @group(0) @binding(0) var<uniform> gusts: GustWaves;
            @fragment fn main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
                return vec4<f32>(gust_waves(gusts, p.xy));
            }}"
        );
        let module = naga::front::wgsl::parse_str(&src).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("validates");
        let g = breezy().to_gpu();
        assert_eq!(g[0], [1.0, 0.0, 1.0 / 60.0, 0.5]);
        assert_eq!(g[2][2], 1.0 / 11.0);
    }
}
