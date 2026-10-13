//! Wind waves: a physically based sea state turned into a sum of Gerstner (trochoidal) waves.
//!
//! The wind speed and fetch give the significant wave height and peak frequency (Pierson–
//! Moskowitz for the open ocean, JONSWAP fetch-limited growth for lakes); a JONSWAP spectrum
//! spreads energy over [`MAX_WAVES`] components around the wind direction. Each component is a
//! Gerstner wave: water parcels move in circles, so crests are sharp and troughs are flat.
//!
//! The same sum runs on the CPU (here: buoyancy, swimming, the `Water` query) and on the GPU
//! (the water shader), from the same numbers, so floating things sit exactly on the waves you see.
//! Phases are accumulated per wave with the world's (dilated) time step, so slow motion slows
//! the sea and wind changes never make the surface jump.

use engine_core::math::{Vec2, Vec3};

use crate::body::WaveSettings;

/// Most wave components per body (the shader's array size).
pub const MAX_WAVES: usize = 24;
/// Standard gravity, m/s².
pub const G: f32 = 9.81;

/// One Gerstner wave.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Wave {
    /// Direction of travel (unit, world x/z).
    pub dir: Vec2,
    /// Wavenumber 2π/λ, rad/m.
    pub k: f32,
    /// Angular frequency, rad/s.
    pub omega: f32,
    /// Vertical amplitude, meters.
    pub amp: f32,
    /// Horizontal (orbital) amplitude, meters. ≤ amp; makes crests sharp.
    pub horiz: f32,
    /// Current phase, radians (accumulated ω·dt, wrapped).
    pub phase: f32,
}

impl Wave {
    pub fn wavelength(&self) -> f32 {
        std::f32::consts::TAU / self.k.max(1e-6)
    }
}

/// How waves shrink in shallow water: vanish at the shoreline, full height once the water is
/// deeper than about a third of a wavelength.
pub fn shoaling(k: f32, depth: f32) -> f32 {
    (k * depth.max(0.0) * 1.2).tanh()
}

/// Significant wave height (m) and peak angular frequency (rad/s) for wind `u10` (m/s at 10 m)
/// blowing over `fetch_m` meters of water (0 = unlimited, a fully developed sea).
pub fn sea_state(u10: f32, fetch_m: f32) -> (f32, f32) {
    let u = u10.max(0.0);
    if u < 0.05 {
        return (0.0, 10.0);
    }
    // Fully developed (Pierson–Moskowitz).
    let hs_full = 0.24 * u * u / G;
    let wp_full = 0.877 * G / u;
    if fetch_m <= 0.0 {
        return (hs_full, wp_full);
    }
    // Fetch-limited growth (JONSWAP): Hs = 1.6e-3 · U · √(F/g), fp = 3.5 g/U · χ^-0.33.
    let chi = G * fetch_m / (u * u);
    let hs = (1.6e-3 * u * (fetch_m / G).sqrt()).min(hs_full);
    let wp = (std::f32::consts::TAU * 3.5 * G / u * chi.powf(-0.33)).max(wp_full);
    (hs, wp)
}

/// Wavenumber for angular frequency `omega` in water `depth` meters deep (ω² = g·k·tanh(k·h)).
pub fn wavenumber(omega: f32, depth: f32) -> f32 {
    let k0 = omega * omega / G;
    if depth <= 0.0 || k0 * depth > 3.0 {
        return k0;
    }
    let mut k = k0;
    for _ in 0..8 {
        k = k0 / (k * depth).tanh().max(1e-4);
    }
    k
}

fn hash01(seed: u64, i: u64) -> f32 {
    let x = engine_core::math::hash_u64(seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// The target set of waves for a sea state. `wind` is the wind at 10 m (m/s, world x/z),
/// `depth` the body's typical depth. Phases are left at zero (the caller keeps its own).
pub fn build_waves(settings: &WaveSettings, wind: Vec2, depth: f32) -> Vec<Wave> {
    let mut out = Vec::with_capacity(MAX_WAVES);
    let scale = settings.height_scale.max(0.0);
    let seed = settings.seed;

    // Swell: three long waves around the swell direction.
    let swell_count = if settings.swell_height_m > 0.0 && scale > 0.0 {
        3
    } else {
        0
    };
    // Wind sea gets the rest.
    let wind_speed = wind.length();
    let wind_count = if settings.wind_waves && wind_speed > 0.2 && scale > 0.0 {
        MAX_WAVES - swell_count
    } else {
        0
    };

    if wind_count > 0 {
        let (hs, wp) = sea_state(wind_speed, settings.fetch_m);
        let hs = hs * scale;
        let wind_dir = wind / wind_speed;
        let base_angle = wind_dir.y.atan2(wind_dir.x);
        // Frequencies from just below the peak to ~5× it, but no shorter than 12 cm waves
        // (shorter ripples are drawn by the shader's detail normals).
        let w_min = 0.72 * wp;
        let w_max = (5.0 * wp).min((G * std::f32::consts::TAU / 0.12).sqrt());
        let w_max = w_max.max(w_min * 1.5);
        let ratio = (w_max / w_min).powf(1.0 / (wind_count as f32 - 1.0).max(1.0));
        let mut weights = Vec::with_capacity(wind_count);
        let mut omegas = Vec::with_capacity(wind_count);
        for i in 0..wind_count {
            // Jitter frequencies so the pattern never visibly repeats.
            let jitter = 1.0 + (hash01(seed, i as u64 * 3 + 1) - 0.5) * 0.18;
            let w = w_min * ratio.powf(i as f32) * jitter;
            let dw = w * (ratio - 1.0);
            // JONSWAP spectral shape.
            let sigma = if w <= wp { 0.07 } else { 0.09 };
            let r = (-((w - wp) * (w - wp)) / (2.0 * sigma * sigma * wp * wp)).exp();
            let s = w.powf(-5.0) * (-1.25 * (wp / w).powi(4)).exp() * 3.3f32.powf(r);
            weights.push((s * dw).max(0.0));
            omegas.push(w);
        }
        let total: f32 = weights.iter().sum();
        // Hs = 4·√m0 and each wave holds a²/2 of variance.
        let m0 = (hs / 4.0) * (hs / 4.0);
        for i in 0..wind_count {
            let amp = if total > 0.0 {
                (2.0 * m0 * weights[i] / total).sqrt()
            } else {
                0.0
            };
            // Short waves spread wider around the wind (cos² spreading, wider at high f).
            let spread = 0.3 + 0.75 * (i as f32 / wind_count as f32);
            let u = hash01(seed, i as u64 * 3 + 2) * 2.0 - 1.0;
            let angle = base_angle + u * spread;
            let w = omegas[i];
            out.push(Wave {
                dir: Vec2::new(angle.cos(), angle.sin()),
                k: wavenumber(w, depth),
                omega: w,
                amp,
                horiz: 0.0,
                phase: 0.0,
            });
        }
    }

    if swell_count > 0 {
        let hs = settings.swell_height_m * scale;
        let base = settings.swell_direction_deg.to_radians();
        let period = settings.swell_period_s.max(2.0);
        let amps = [0.75f32, 0.5, 0.42];
        let norm: f32 = amps.iter().map(|a| a * a).sum::<f32>().sqrt();
        for (i, a) in amps.iter().enumerate() {
            let t = period * (1.0 + (i as f32 - 1.0) * 0.08);
            let w = std::f32::consts::TAU / t;
            let angle = base + (i as f32 - 1.0) * 0.12;
            // Σa²/2 = (Hs/4)²  →  a_i = (Hs/4)·√2 · a_i/‖a‖.
            let amp = hs / 4.0 * std::f32::consts::SQRT_2 * a / norm;
            out.push(Wave {
                dir: Vec2::new(angle.cos(), angle.sin()),
                k: wavenumber(w, depth),
                omega: w,
                amp,
                horiz: 0.0,
                phase: 0.0,
            });
        }
    }

    // Horizontal amplitudes: sharp crests without the surface looping over itself
    // (Σ k·horiz stays below 0.9).
    let sum_ka: f32 = out.iter().map(|w| w.k * w.amp).sum();
    let s = if sum_ka > 0.0 {
        (0.9 / sum_ka).min(1.0)
    } else {
        1.0
    };
    let chop = settings.choppiness.clamp(0.0, 1.0);
    for w in &mut out {
        w.horiz = chop * w.amp * s;
    }
    out
}

/// What the surface does at one point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WavePoint {
    /// Displacement of the surface parcel from its rest position, meters.
    pub offset: Vec3,
    /// Surface normal (unit).
    pub normal: Vec3,
    /// Velocity of the water at the surface, m/s.
    pub velocity: Vec3,
    /// Area change of the surface (1 = flat, < 0.5 crest compressing into a whitecap).
    pub jacobian: f32,
}

impl Default for WavePoint {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            normal: Vec3::Y,
            velocity: Vec3::ZERO,
            jacobian: 1.0,
        }
    }
}

/// The live waves of one body. Index `i` keeps its identity when the sea state changes, so
/// amplitudes and lengths blend smoothly.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaveSet {
    pub waves: Vec<Wave>,
}

impl WaveSet {
    /// Move every wave a fraction `t` (0..1) toward `target` (phases are kept). Waves missing
    /// from one side fade in or out.
    pub fn blend_toward(&mut self, target: &[Wave], t: f32) {
        let t = t.clamp(0.0, 1.0);
        let n = self.waves.len().max(target.len());
        self.waves.resize(n, Wave::default());
        for (i, w) in self.waves.iter_mut().enumerate() {
            let goal = target.get(i).copied().unwrap_or(Wave {
                amp: 0.0,
                horiz: 0.0,
                ..*w
            });
            if w.k == 0.0 {
                // A new wave: take its shape at once, grow its height.
                w.dir = goal.dir;
                w.k = goal.k;
                w.omega = goal.omega;
            }
            let lerp = |a: f32, b: f32| a + (b - a) * t;
            w.amp = lerp(w.amp, goal.amp);
            w.horiz = lerp(w.horiz, goal.horiz);
            w.k = lerp(w.k, goal.k);
            w.omega = lerp(w.omega, goal.omega);
            w.dir = (w.dir + (goal.dir - w.dir) * t).normalize_or(goal.dir);
        }
        while self.waves.last().is_some_and(|w| w.amp < 1e-5) && self.waves.len() > target.len() {
            self.waves.pop();
        }
    }

    /// Advance every wave by `dt` seconds of world time.
    pub fn advance(&mut self, dt: f32) {
        for w in &mut self.waves {
            w.phase = (w.phase + w.omega * dt).rem_euclid(std::f32::consts::TAU);
        }
    }

    /// Significant wave height of the current waves, meters.
    pub fn significant_height(&self) -> f32 {
        4.0 * (self.waves.iter().map(|w| w.amp * w.amp * 0.5).sum::<f32>()).sqrt()
    }

    /// Displacement, normal and velocity of the parcel whose rest position is `p0` (meters
    /// from the body's wave reference point), in water `depth` meters deep.
    pub fn eval(&self, p0: Vec2, depth: f32) -> WavePoint {
        let mut off = Vec3::ZERO;
        let mut vel = Vec3::ZERO;
        let (mut dhx, mut dhz) = (0.0f32, 0.0f32);
        let (mut jxx, mut jzz, mut jxz) = (0.0f32, 0.0f32, 0.0f32);
        for w in &self.waves {
            let f = shoaling(w.k, depth);
            if f <= 0.0 {
                continue;
            }
            let a = w.amp * f;
            let c = w.horiz * f;
            let theta = w.k * w.dir.dot(p0) - w.phase;
            let (s, co) = theta.sin_cos();
            off.x += c * w.dir.x * co;
            off.z += c * w.dir.y * co;
            off.y += a * s;
            let ka = w.k * a;
            let kc = w.k * c;
            dhx += ka * w.dir.x * co;
            dhz += ka * w.dir.y * co;
            jxx += kc * w.dir.x * w.dir.x * s;
            jzz += kc * w.dir.y * w.dir.y * s;
            jxz += kc * w.dir.x * w.dir.y * s;
            vel.x += c * w.dir.x * w.omega * s;
            vel.z += c * w.dir.y * w.omega * s;
            vel.y -= a * w.omega * co;
        }
        let tx = Vec3::new(1.0 - jxx, dhx, -jxz);
        let tz = Vec3::new(-jxz, dhz, 1.0 - jzz);
        WavePoint {
            offset: off,
            normal: tz.cross(tx).normalize_or(Vec3::Y),
            velocity: vel,
            jacobian: (1.0 - jxx) * (1.0 - jzz) - jxz * jxz,
        }
    }

    /// The surface above the fixed point `p` (meters from the reference point): finds the
    /// parcel that moved there (Gerstner waves shift water sideways) and returns its state.
    pub fn sample(&self, p: Vec2, depth: f32) -> WavePoint {
        if self.waves.is_empty() {
            return WavePoint::default();
        }
        let mut p0 = p;
        for _ in 0..4 {
            let e = self.eval(p0, depth);
            p0 = p - Vec2::new(e.offset.x, e.offset.z);
        }
        self.eval(p0, depth)
    }

    /// Height of the highest crest possible (sum of amplitudes), meters.
    pub fn max_height(&self) -> f32 {
        self.waves.iter().map(|w| w.amp).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sea_state_matches_known_values() {
        // Open ocean, 10 m/s: Hs ≈ 2.4 m, peak period ≈ 7.3 s.
        let (hs, wp) = sea_state(10.0, 0.0);
        assert!((hs - 2.45).abs() < 0.2, "{hs}");
        let tp = std::f32::consts::TAU / wp;
        assert!((tp - 7.3).abs() < 0.6, "{tp}");
        // A 150 m lake with a 5 m/s breeze: a few centimeters of short chop.
        let (hs, wp) = sea_state(5.0, 150.0);
        assert!(hs > 0.01 && hs < 0.06, "{hs}");
        let lambda = std::f32::consts::TAU * G / (wp * wp);
        assert!(lambda > 0.2 && lambda < 1.2, "{lambda}");
        // Stronger wind, bigger waves; longer fetch, bigger waves.
        assert!(sea_state(15.0, 0.0).0 > sea_state(10.0, 0.0).0);
        assert!(sea_state(10.0, 10_000.0).0 > sea_state(10.0, 100.0).0);
        assert_eq!(sea_state(0.0, 0.0).0, 0.0);
    }

    #[test]
    fn built_waves_have_the_requested_height() {
        let settings = WaveSettings {
            fetch_m: 0.0,
            swell_height_m: 0.0,
            ..WaveSettings::default()
        };
        let waves = build_waves(&settings, Vec2::new(10.0, 0.0), 1000.0);
        assert_eq!(waves.len(), MAX_WAVES);
        let set = WaveSet { waves };
        let (hs, _) = sea_state(10.0, 0.0);
        assert!((set.significant_height() - hs).abs() < 0.05 * hs);
        // Mostly traveling downwind.
        let mean: Vec2 = set.waves.iter().map(|w| w.dir * w.amp).sum();
        assert!(mean.x > 0.0 && mean.x > mean.y.abs() * 2.0, "{mean:?}");
        // Deep-water dispersion.
        for w in &set.waves {
            assert!((w.omega * w.omega - G * w.k).abs() < 0.02 * w.omega * w.omega);
        }
    }

    #[test]
    fn swell_only_when_calm() {
        let settings = WaveSettings {
            wind_waves: false,
            swell_height_m: 2.0,
            swell_period_s: 12.0,
            ..WaveSettings::default()
        };
        let set = WaveSet {
            waves: build_waves(&settings, Vec2::new(10.0, 0.0), 1000.0),
        };
        assert_eq!(set.waves.len(), 3);
        assert!((set.significant_height() - 2.0).abs() < 0.05);
        let lambda = set.waves[1].wavelength();
        assert!(
            (lambda - 225.0).abs() < 15.0,
            "12 s swell ≈ 225 m long, got {lambda}"
        );
    }

    #[test]
    fn sampling_finds_the_displaced_surface() {
        let settings = WaveSettings {
            fetch_m: 0.0,
            choppiness: 1.0,
            ..WaveSettings::default()
        };
        let mut set = WaveSet {
            waves: build_waves(&settings, Vec2::new(12.0, 4.0), 1000.0),
        };
        set.advance(3.7);
        for i in 0..40 {
            let p = Vec2::new(i as f32 * 3.3 - 50.0, (i as f32 * 1.7).sin() * 20.0);
            let s = set.sample(p, 1000.0);
            // The parcel we found really sits above p.
            let mut p0 = p;
            for _ in 0..30 {
                let e = set.eval(p0, 1000.0);
                p0 = p - Vec2::new(e.offset.x, e.offset.z);
            }
            let exact = set.eval(p0, 1000.0);
            assert!(
                (s.offset.y - exact.offset.y).abs() < 0.02,
                "{} vs {}",
                s.offset.y,
                exact.offset.y
            );
            assert!(s.normal.y > 0.3 && s.jacobian > 0.0);
        }
    }

    #[test]
    fn normal_matches_finite_differences() {
        let settings = WaveSettings {
            fetch_m: 2000.0,
            choppiness: 0.0,
            ..WaveSettings::default()
        };
        let set = WaveSet {
            waves: build_waves(&settings, Vec2::new(8.0, -3.0), 500.0),
        };
        let p = Vec2::new(3.0, 7.0);
        let e = 0.001;
        let h = |q: Vec2| set.eval(q, 500.0).offset.y;
        let gx = (h(p + Vec2::X * e) - h(p - Vec2::X * e)) / (2.0 * e);
        let gz = (h(p + Vec2::Y * e) - h(p - Vec2::Y * e)) / (2.0 * e);
        let n = Vec3::new(-gx, 1.0, -gz).normalize();
        let got = set.eval(p, 500.0).normal;
        assert!(n.abs_diff_eq(got, 2e-3), "{n:?} vs {got:?}");
    }

    #[test]
    fn waves_shrink_at_the_shore_and_blend_smoothly() {
        let settings = WaveSettings::ocean();
        let set = WaveSet {
            waves: build_waves(&settings, Vec2::new(9.0, 0.0), 50.0),
        };
        let deep = (0..50)
            .map(|i| {
                set.eval(Vec2::new(i as f32 * 7.0, 0.0), 50.0)
                    .offset
                    .y
                    .abs()
            })
            .fold(0.0, f32::max);
        let shore = (0..50)
            .map(|i| {
                set.eval(Vec2::new(i as f32 * 7.0, 0.0), 0.05)
                    .offset
                    .y
                    .abs()
            })
            .fold(0.0, f32::max);
        assert!(shore < deep * 0.1, "shore {shore} deep {deep}");

        // Blending keeps phases and moves amplitudes partway.
        let mut a = set.clone();
        a.advance(1.0);
        let phases: Vec<f32> = a.waves.iter().map(|w| w.phase).collect();
        let calm: Vec<Wave> = Vec::new();
        a.blend_toward(&calm, 0.5);
        assert!(a.significant_height() < set.significant_height() * 0.6);
        for (w, p) in a.waves.iter().zip(phases) {
            assert_eq!(w.phase, p);
        }
    }
}
