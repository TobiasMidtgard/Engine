//! One-shot effects for world events: explosions, bullets passing by, swings and dashes, and
//! a plain tone for UI and tests. All mono, so the mixer can place them in 3D.

use crate::dsp::{Biquad, Noise, decay_coef};
use crate::mixer::Sound;

/// An explosion (or any big blast): a crack, a falling low boom and a rumbling tail. Bigger
/// blasts (more `energy_j`) are deeper and longer.
pub struct ExplosionSound {
    fs: f32,
    rng: Noise,
    t: u32,
    len: u32,
    phase: f32,
    f_start: f32,
    f_end: f32,
    boom: f32,
    boom_k: f32,
    rumble: f32,
    rumble_k: f32,
    crack: u32,
    crack_hp: Biquad,
    rumble_lp: Biquad,
    brown: f32,
    gain: f32,
}

impl ExplosionSound {
    pub fn new(fs: f32, energy_j: f32, seed: u64) -> Self {
        let mut rng = Noise::new(seed);
        // A grenade is ~1e6 J (size 1), a car bomb ~1e8 J (size ~2.2).
        let size = (energy_j.max(1e3) / 1e6).powf(1.0 / 6.0).clamp(0.3, 4.0);
        let len_s = 1.5 + 1.5 * size;
        Self {
            fs,
            t: 0,
            len: (len_s * fs) as u32,
            phase: 0.0,
            f_start: 90.0 / size.sqrt() * rng.range(0.9, 1.1),
            f_end: 28.0 / size.sqrt(),
            boom: 0.8,
            boom_k: decay_coef(fs, 0.9 * size),
            rumble: 0.7,
            rumble_k: decay_coef(fs, len_s),
            crack: (0.03 * fs) as u32,
            crack_hp: Biquad::highpass(fs, 1_400.0, 0.7),
            rumble_lp: Biquad::lowpass(fs, 900.0 / size.sqrt(), 0.7),
            brown: 0.0,
            gain: (0.7 * size.sqrt()).min(1.0),
            rng,
        }
    }
}

impl Sound for ExplosionSound {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        for (i, o) in out.iter_mut().enumerate() {
            if self.t >= self.len {
                return i;
            }
            let tt = self.t as f32 / fs;
            let f = self.f_end + (self.f_start - self.f_end) * (-tt / 0.25).exp();
            self.phase = (self.phase + f / fs).fract();
            let boom = (self.phase * std::f32::consts::TAU).sin() * self.boom;
            self.boom *= self.boom_k;
            self.brown = (self.brown + self.rng.white() * 0.06) * 0.997;
            let rumble = self.rumble_lp.process(self.brown * 0.9) * self.rumble;
            self.rumble *= self.rumble_k;
            let mut crack = 0.0;
            if self.t < self.crack {
                let e = 1.0 - self.t as f32 / self.crack as f32;
                crack = self.crack_hp.process(self.rng.white()) * e * 0.8;
            }
            let fade = ((self.len - self.t) as f32 / (0.3 * fs)).min(1.0);
            *o = (boom + rumble + crack) * self.gain * fade;
            self.t += 1;
        }
        out.len()
    }
}

/// A projectile passing close by: an airy whiz that drops in pitch as it passes, plus the
/// sharp crack of the shock wave when it is supersonic.
pub struct WhizSound {
    fs: f32,
    rng: Noise,
    t: u32,
    len: u32,
    bp: Biquad,
    crack_at: Option<u32>,
    crack_hp: Biquad,
    gain: f32,
}

impl WhizSound {
    /// `speed` m/s, `miss_m` how close it passed (closer is louder).
    pub fn new(fs: f32, speed: f32, miss_m: f32, seed: u64) -> Self {
        let len = (0.22 * fs) as u32;
        Self {
            fs,
            rng: Noise::new(seed),
            t: 0,
            len,
            bp: Biquad::bandpass(fs, 3_500.0, 2.0),
            crack_at: (speed > 343.0).then_some(len / 2 - (0.01 * fs) as u32),
            crack_hp: Biquad::highpass(fs, 600.0, 0.7),
            gain: (1.0 / (1.0 + miss_m.max(0.0) * 0.6)).min(1.0),
        }
    }
}

impl Sound for WhizSound {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        for (i, o) in out.iter_mut().enumerate() {
            if self.t >= self.len {
                return i;
            }
            let x = self.t as f32 / self.len as f32;
            if self.t.is_multiple_of(32) {
                // Doppler: high on approach, low as it leaves.
                let f = 4_500.0 - 2_700.0 * x;
                self.bp.retune(Biquad::bandpass(fs, f, 2.0));
            }
            let env = (x * std::f32::consts::PI).sin().powi(4);
            let mut y = self.bp.process(self.rng.white()) * env * 1.6;
            if let Some(c) = self.crack_at {
                // N-wave: a sharp up-then-down pressure jump about a millisecond long.
                let n = (0.0008 * fs) as u32;
                if self.t >= c && self.t < c + n {
                    let k = (self.t - c) as f32 / n as f32;
                    y += self.crack_hp.process(if k < 0.5 { 0.9 } else { -0.9 });
                } else {
                    y += self.crack_hp.process(0.0);
                }
            }
            *o = y * self.gain;
            self.t += 1;
        }
        out.len()
    }
}

/// A swing or dash: wind rushing past an arm, a blade or a whole body. `secs` is how long the
/// motion takes, `strength` 0..2 how violent.
pub struct WhooshSound {
    fs: f32,
    rng: Noise,
    t: u32,
    len: u32,
    bp: Biquad,
    low: Biquad,
    strength: f32,
    peak_hz: f32,
}

impl WhooshSound {
    pub fn new(fs: f32, secs: f32, strength: f32, seed: u64) -> Self {
        let mut rng = Noise::new(seed);
        Self {
            fs,
            t: 0,
            len: (secs.clamp(0.05, 2.0) * fs) as u32,
            bp: Biquad::bandpass(fs, 500.0, 1.4),
            low: Biquad::lowpass(fs, 300.0, 0.7),
            strength: strength.clamp(0.0, 2.5),
            peak_hz: rng.range(1_100.0, 1_700.0) * (0.8 + 0.2 * strength.min(2.0)),
            rng,
        }
    }
}

impl Sound for WhooshSound {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        for (i, o) in out.iter_mut().enumerate() {
            if self.t >= self.len {
                return i;
            }
            let x = self.t as f32 / self.len as f32;
            // Peaks a little after the middle, like a swing accelerating then braking.
            let shape = (x.powf(0.8) * std::f32::consts::PI).sin().max(0.0);
            if self.t.is_multiple_of(32) {
                let f = 350.0 + (self.peak_hz - 350.0) * shape;
                self.bp.retune(Biquad::bandpass(fs, f, 1.4));
            }
            let n = self.rng.white();
            let y = self.bp.process(n) * 0.9 + self.low.process(n) * 0.3 * self.strength;
            *o = y * shape * shape * 0.6 * self.strength;
            self.t += 1;
        }
        out.len()
    }
}

/// A sine tone (UI beeps, tests, tuning). Fades in and out over 5 ms to avoid clicks.
pub struct ToneSound {
    fs: f32,
    freq: f32,
    phase: f32,
    t: u32,
    len: u32,
    amp: f32,
}

impl ToneSound {
    pub fn new(fs: f32, freq: f32, secs: f32, amp: f32) -> Self {
        Self {
            fs,
            freq,
            phase: 0.0,
            t: 0,
            len: (secs.max(0.0) * fs) as u32,
            amp,
        }
    }
}

impl Sound for ToneSound {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let ramp = (0.005 * self.fs).max(1.0);
        for (i, o) in out.iter_mut().enumerate() {
            if self.t >= self.len {
                return i;
            }
            let edge = (self.t as f32 / ramp)
                .min((self.len - self.t) as f32 / ramp)
                .min(1.0);
            *o = (self.phase * std::f32::consts::TAU).sin() * self.amp * edge;
            self.phase = (self.phase + self.freq / self.fs).fract();
            self.t += 1;
        }
        out.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footstep::render_mono;

    const FS: f32 = 48_000.0;

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn effects_render_and_end() {
        let sounds: Vec<(&str, Box<dyn Sound>)> = vec![
            ("explosion", Box::new(ExplosionSound::new(FS, 1e6, 1))),
            ("big explosion", Box::new(ExplosionSound::new(FS, 1e9, 1))),
            ("whiz", Box::new(WhizSound::new(FS, 300.0, 1.0, 1))),
            ("supersonic", Box::new(WhizSound::new(FS, 900.0, 1.0, 1))),
            ("whoosh", Box::new(WhooshSound::new(FS, 0.3, 1.0, 1))),
            ("tone", Box::new(ToneSound::new(FS, 440.0, 0.5, 0.5))),
        ];
        for (name, mut s) in sounds {
            let x = render_mono(s.as_mut(), 20.0, FS);
            assert!(x.len() < (19.0 * FS) as usize, "{name} never ends");
            let p = peak(&x);
            assert!((0.05..1.5).contains(&p), "{name}: peak {p}");
        }
    }

    #[test]
    fn bigger_explosions_last_longer() {
        let small = render_mono(&mut ExplosionSound::new(FS, 1e5, 2), 30.0, FS);
        let big = render_mono(&mut ExplosionSound::new(FS, 1e10, 2), 30.0, FS);
        assert!(big.len() > small.len());
    }
}
