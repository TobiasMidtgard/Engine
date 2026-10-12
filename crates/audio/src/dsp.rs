//! Small DSP building blocks used by the mixer and the procedural sounds.
//!
//! Everything runs one sample at a time at the mixer's sample rate, allocates nothing after
//! construction (except [`Freeverb`]'s delay lines, sized once) and is deterministic for a
//! given seed, so tests can render a sound twice and compare.

use std::f32::consts::{PI, TAU};

/// Fast deterministic white noise and random numbers (xorshift32).
#[derive(Debug, Clone)]
pub struct Noise(u32);

impl Noise {
    pub fn new(seed: u64) -> Self {
        let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        Self((x as u32) | 1)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn uniform(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// Uniform white noise in `[-1, 1)`.
    #[inline]
    pub fn white(&mut self) -> f32 {
        self.uniform() * 2.0 - 1.0
    }

    #[inline]
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.uniform()
    }

    #[inline]
    pub fn chance(&mut self, p: f32) -> bool {
        self.uniform() < p
    }

    /// Exponentially distributed waiting time for events at `rate` per second, in seconds.
    #[inline]
    pub fn exp_interval(&mut self, rate: f32) -> f32 {
        if rate <= 0.0 {
            return f32::INFINITY;
        }
        -(1.0 - self.uniform()).max(1e-7).ln() / rate
    }
}

/// Second-order IIR filter (RBJ audio EQ cookbook), transposed direct form II.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    /// A filter that passes the signal through unchanged.
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

fn omega(fs: f32, fc: f32) -> (f32, f32) {
    let fc = fc.clamp(5.0, fs * 0.49);
    let w = TAU * fc / fs;
    (w.cos(), w.sin())
}

impl Biquad {
    fn from(b: [f32; 3], a: [f32; 3]) -> Self {
        let inv = 1.0 / a[0];
        Self {
            b0: b[0] * inv,
            b1: b[1] * inv,
            b2: b[2] * inv,
            a1: a[1] * inv,
            a2: a[2] * inv,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Low-pass at `fc` Hz. `q` 0.707 is maximally flat; higher values resonate.
    pub fn lowpass(fs: f32, fc: f32, q: f32) -> Self {
        let (c, s) = omega(fs, fc);
        let alpha = s / (2.0 * q.max(0.05));
        Self::from(
            [(1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        )
    }

    /// High-pass at `fc` Hz.
    pub fn highpass(fs: f32, fc: f32, q: f32) -> Self {
        let (c, s) = omega(fs, fc);
        let alpha = s / (2.0 * q.max(0.05));
        Self::from(
            [(1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        )
    }

    /// Band-pass around `fc` Hz with 0 dB gain at the center; higher `q` is narrower.
    pub fn bandpass(fs: f32, fc: f32, q: f32) -> Self {
        let (c, s) = omega(fs, fc);
        let alpha = s / (2.0 * q.max(0.05));
        Self::from([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    /// High shelf at `fc` Hz: boosts (or cuts, negative dB) everything above it.
    pub fn high_shelf(fs: f32, fc: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (c, s) = omega(fs, fc);
        let alpha = s / 2.0 * 2f32.sqrt();
        let sa = 2.0 * a.sqrt() * alpha;
        Self::from(
            [
                a * ((a + 1.0) + (a - 1.0) * c + sa),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - sa),
            ],
            [
                (a + 1.0) - (a - 1.0) * c + sa,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - sa,
            ],
        )
    }

    /// Replace the coefficients with `other`'s, keeping this filter's state (no click).
    pub fn retune(&mut self, other: Biquad) {
        self.b0 = other.b0;
        self.b1 = other.b1;
        self.b2 = other.b2;
        self.a1 = other.a1;
        self.a2 = other.a2;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Magnitude response at `f` Hz (for tests and tuning).
    pub fn magnitude(&self, fs: f32, f: f32) -> f32 {
        let w = TAU * f / fs;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }
}

/// One-pole low-pass (6 dB/octave): cheap smoothing and gentle tone control.
#[derive(Debug, Clone, Copy, Default)]
pub struct OnePole {
    a: f32,
    y: f32,
}

impl OnePole {
    pub fn new(fs: f32, fc: f32) -> Self {
        let mut f = Self::default();
        f.set(fs, fc);
        f
    }

    pub fn set(&mut self, fs: f32, fc: f32) {
        self.a = 1.0 - (-TAU * fc.clamp(1.0, fs * 0.49) / fs).exp();
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

/// Per-sample multiplier that makes a level fall by 60 dB in `t60` seconds.
#[inline]
pub fn decay_coef(fs: f32, t60: f32) -> f32 {
    if t60 <= 0.0 {
        0.0
    } else {
        (-6.907_755 / (t60 * fs)).exp()
    }
}

/// Two-pole resonator: rings at `freq` Hz for `t60` seconds when struck. Modal synthesis
/// (wood, metal, stone) is a sum of these.
#[derive(Debug, Clone, Copy, Default)]
pub struct Resonator {
    c1: f32,
    c2: f32,
    gain: f32,
    y1: f32,
    y2: f32,
}

impl Resonator {
    pub fn new(fs: f32, freq: f32, t60: f32) -> Self {
        let r = decay_coef(fs, t60);
        let w = TAU * freq.clamp(10.0, fs * 0.45) / fs;
        Self {
            c1: 2.0 * r * w.cos(),
            c2: -r * r,
            // A unit impulse in gives a sine of amplitude ≈ 1 out.
            gain: w.sin(),
            y1: 0.0,
            y2: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.c1 * self.y1 + self.c2 * self.y2 + x * self.gain;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// True once it has rung out.
    pub fn quiet(&self) -> bool {
        self.y1.abs() < 1e-5 && self.y2.abs() < 1e-5
    }
}

/// An air bubble ringing in water (van den Doel's model): the "plink" of a raindrop on a
/// puddle and the gloop of mud. Frequency follows Minnaert's law (≈ 3.26 / radius Hz) and
/// rises linearly as the bubble nears the surface.
#[derive(Debug, Clone, Copy)]
pub struct Bubble {
    phase: f32,
    f0: f32,
    freq: f32,
    max_freq: f32,
    slope: f32,
    amp: f32,
    damp: f32,
    delay: u32,
    inv_fs: f32,
}

impl Bubble {
    /// `radius_m` sets the pitch (1 mm ≈ 3.3 kHz, 5 mm ≈ 650 Hz), `rise` how much it chirps
    /// upward (0.1 is natural, 1 a cartoon "bloop"), `delay_s` when it starts.
    pub fn new(fs: f32, radius_m: f32, amp: f32, rise: f32, delay_s: f32) -> Self {
        let f0 = 3.26 / radius_m.max(1e-4);
        let d = 0.043 * f0 + 0.0014 * f0.powf(1.5);
        Self {
            phase: 0.0,
            f0,
            freq: f0,
            max_freq: f0 * 3.0,
            slope: rise * d * f0,
            amp,
            damp: (-d / fs).exp(),
            delay: (delay_s.max(0.0) * fs) as u32,
            inv_fs: 1.0 / fs,
        }
    }

    /// Ring `k` times shorter (bubbles in thick mud die quickly).
    pub fn damped(mut self, k: f32, fs: f32) -> Self {
        let d = -self.damp.ln() * fs * k.max(0.01);
        self.damp = (-d / fs).exp();
        self
    }

    #[inline]
    pub fn process(&mut self) -> f32 {
        if self.delay > 0 {
            self.delay -= 1;
            return 0.0;
        }
        let y = self.amp * self.phase.sin();
        self.phase += std::f32::consts::TAU * self.freq * self.inv_fs;
        if self.phase > std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }
        self.freq = (self.freq + self.slope * self.inv_fs).min(self.max_freq);
        self.amp *= self.damp;
        y
    }

    /// Starting pitch, Hz.
    pub fn frequency(&self) -> f32 {
        self.f0
    }

    pub fn done(&self) -> bool {
        self.delay == 0 && self.amp < 1e-4
    }
}

/// Removes DC offset (a slow drift that wastes headroom and clicks on start/stop).
#[derive(Debug, Clone, Copy, Default)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
}

impl DcBlock {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + 0.995 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Peak limiter for the master bus: instant attack, smooth release, then a soft clip, so a
/// pile of explosions gets louder without ever distorting harshly.
#[derive(Debug, Clone, Copy)]
pub struct Limiter {
    env: f32,
    release: f32,
    threshold: f32,
}

impl Limiter {
    pub fn new(fs: f32, threshold: f32) -> Self {
        Self {
            env: 0.0,
            release: decay_coef(fs, 0.6),
            threshold,
        }
    }

    /// Process one stereo frame in place. Returns the gain reduction applied (1 = none).
    #[inline]
    pub fn process(&mut self, l: &mut f32, r: &mut f32) -> f32 {
        let peak = l.abs().max(r.abs());
        self.env = if peak > self.env {
            peak
        } else {
            self.env * self.release + peak * (1.0 - self.release)
        };
        let g = if self.env > self.threshold {
            self.threshold / self.env
        } else {
            1.0
        };
        *l = soft_clip(*l * g);
        *r = soft_clip(*r * g);
        g
    }
}

/// Gentle saturation above ±0.9, never exceeding ±1.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.9 {
        x
    } else {
        let over = (a - 0.9) / 0.1;
        x.signum() * (0.9 + 0.1 * (over / (1.0 + over)))
    }
}

/// Decibels to linear amplitude.
#[inline]
pub fn db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Linear amplitude to decibels (−120 dB floor).
#[inline]
pub fn to_db(amp: f32) -> f32 {
    20.0 * amp.max(1e-6).log10()
}

/// Equal-power stereo pan: `pan` −1 (left) .. 1 (right) → (left gain, right gain).
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
    (a.cos(), a.sin())
}

/// Catmull-Rom interpolation between `p1` and `p2` at `t` in 0..1.
#[inline]
pub fn hermite(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (p2 - p0);
    let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);
    ((c3 * t + c2) * t + c1) * t + p1
}

// ---------------------------------------------------------------- reverb

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
            store: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.buf[self.idx];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.idx] = x + self.store * feedback;
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        let y = b - x;
        self.buf[self.idx] = x + b * 0.5;
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
        y
    }
}

/// Stereo room reverb (Jezar's Freeverb). `room` 0..1 sets the decay, `damping` 0..1 how fast
/// highs die (soft rooms, snow), `width` the stereo spread.
pub struct Freeverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    pre: [Vec<f32>; 2],
    pre_idx: usize,
    pub room: f32,
    pub damping: f32,
    pub width: f32,
}

impl Freeverb {
    pub fn new(fs: f32) -> Self {
        let scale = fs / 44_100.0;
        let len = |n: usize| ((n as f32) * scale) as usize;
        const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
        const SPREAD: usize = 23;
        let combs = [0, SPREAD].map(|s| COMBS.iter().map(|&n| Comb::new(len(n + s))).collect());
        let allpasses = [0, SPREAD].map(|s| {
            ALLPASSES
                .iter()
                .map(|&n| Allpass::new(len(n + s)))
                .collect()
        });
        // 12 ms pre-delay separates the dry hit from the room.
        let pre_len = (0.012 * fs) as usize;
        Self {
            combs,
            allpasses,
            pre: [vec![0.0; pre_len.max(1)], vec![0.0; pre_len.max(1)]],
            pre_idx: 0,
            room: 0.5,
            damping: 0.5,
            width: 1.0,
        }
    }

    /// Process one stereo frame of reverb input; returns the wet stereo output.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let feedback = 0.7 + 0.28 * self.room.clamp(0.0, 1.0);
        let damp = 0.4 * self.damping.clamp(0.0, 1.0);
        let i = self.pre_idx;
        let x_l = std::mem::replace(&mut self.pre[0][i], in_l);
        let x_r = std::mem::replace(&mut self.pre[1][i], in_r);
        self.pre_idx = (i + 1) % self.pre[0].len();
        let input = (x_l + x_r) * 0.015;
        let mut out = [0.0f32; 2];
        for (ch, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for c in &mut self.combs[ch] {
                acc += c.process(input, feedback, damp);
            }
            for a in &mut self.allpasses[ch] {
                acc = a.process(acc);
            }
            *o = acc;
        }
        let w1 = self.width * 0.5 + 0.5;
        let w2 = (1.0 - self.width) * 0.5;
        (out[0] * w1 + out[1] * w2, out[1] * w1 + out[0] * w2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    #[test]
    fn filters_pass_and_stop_the_right_bands() {
        let lp = Biquad::lowpass(FS, 1000.0, 0.707);
        assert!((lp.magnitude(FS, 100.0) - 1.0).abs() < 0.02);
        assert!(lp.magnitude(FS, 8000.0) < 0.03);
        let hp = Biquad::highpass(FS, 1000.0, 0.707);
        assert!(hp.magnitude(FS, 100.0) < 0.02);
        assert!((hp.magnitude(FS, 8000.0) - 1.0).abs() < 0.03);
        let bp = Biquad::bandpass(FS, 2000.0, 4.0);
        assert!((bp.magnitude(FS, 2000.0) - 1.0).abs() < 0.01);
        assert!(bp.magnitude(FS, 500.0) < 0.1);
        let shelf = Biquad::high_shelf(FS, 2000.0, -12.0);
        assert!((shelf.magnitude(FS, 100.0) - 1.0).abs() < 0.05);
        assert!((to_db(shelf.magnitude(FS, 15_000.0)) + 12.0).abs() < 1.0);
    }

    #[test]
    fn resonator_rings_at_its_frequency_for_its_t60() {
        let mut r = Resonator::new(FS, 440.0, 0.5);
        let mut out = vec![0.0; (FS * 0.5) as usize];
        for (i, o) in out.iter_mut().enumerate() {
            *o = r.process(if i == 0 { 1.0 } else { 0.0 });
        }
        // Zero crossings in the first 100 ms ≈ 2 × 440 × 0.1.
        let n = (FS * 0.1) as usize;
        let crossings = out[..n]
            .windows(2)
            .filter(|w| w[0].signum() != w[1].signum())
            .count();
        assert!((crossings as i32 - 88).abs() <= 2, "{crossings}");
        let start = out[..480].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let end = out[out.len() - 480..]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.8..1.2).contains(&start), "{start}");
        // −60 dB after t60.
        assert!(end < start * 0.0015, "{end}");
    }

    #[test]
    fn bubble_pitch_follows_minnaert() {
        let mut b = Bubble::new(FS, 0.002, 1.0, 0.0, 0.0);
        let n = (FS * 0.005) as usize;
        let samples: Vec<f32> = (0..n).map(|_| b.process()).collect();
        let crossings = samples
            .windows(2)
            .filter(|w| w[0].signum() != w[1].signum())
            .count();
        // 1630 Hz for 5 ms ≈ 16 crossings.
        assert!((14..=18).contains(&crossings), "{crossings}");
    }

    #[test]
    fn limiter_never_exceeds_full_scale() {
        let mut lim = Limiter::new(FS, 0.8);
        let mut noise = Noise::new(1);
        for _ in 0..48_000 {
            let mut l = noise.white() * 6.0;
            let mut r = noise.white() * 6.0;
            lim.process(&mut l, &mut r);
            assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
        }
    }

    #[test]
    fn noise_is_deterministic_and_centered() {
        let mut a = Noise::new(7);
        let mut b = Noise::new(7);
        let mut sum = 0.0;
        for _ in 0..10_000 {
            let x = a.white();
            assert_eq!(x, b.white());
            sum += x;
        }
        assert!((sum / 10_000.0).abs() < 0.03);
    }

    #[test]
    fn reverb_has_a_tail() {
        let mut rev = Freeverb::new(FS);
        rev.room = 0.9;
        let mut energy_late = 0.0;
        for i in 0..(FS as usize) {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = rev.process(x, x);
            if i > (FS * 0.5) as usize {
                energy_late += l * l + r * r;
            }
        }
        assert!(energy_late > 1e-6, "{energy_late}");
    }
}
