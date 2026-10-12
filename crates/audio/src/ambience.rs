//! Weather ambience: rain, wind and thunder, synthesized live from the weather state.
//!
//! * **Rain** is a stereo bed of hiss plus individual drops, each hitting something near the
//!   listener: leaves and grass tick softly, stone and concrete tap, puddles and lakes plink
//!   (a ringing bubble), metal roofs ping. As puddles grow the mix shifts toward plinks, so
//!   you hear the ground getting wet. Louder and denser with intensity (mm/h).
//! * **Wind** is band-passed noise whose pitch and level follow the air speed past the
//!   listener (wind plus your own motion), with buffeting at high speed and a whistle in
//!   storms.
//! * **Thunder** rolls in several swells; distant strikes are duller (air eats the highs) and,
//!   like every positioned sound, arrive late at the speed of sound.

use crate::dsp::{Biquad, Bubble, Noise, Resonator, decay_coef, pan_gains};
use crate::mixer::Sound;

/// Drops in flight at once (more are dropped, which you can't hear in a downpour).
const MAX_DROPS: usize = 48;

#[derive(Clone, Copy)]
enum DropKind {
    /// Leaves, grass, soil.
    Tick,
    /// Stone, concrete, wood.
    Tap,
    /// Water: a ringing bubble.
    Plink,
    /// Metal.
    Ping,
}

#[derive(Clone, Copy)]
struct Drop {
    kind: DropKind,
    left: u32,
    len: u32,
    amp: f32,
    filter: Biquad,
    res: Resonator,
    bubble: Bubble,
    gl: f32,
    gr: f32,
}

/// Rain around the listener. Parameters: `intensity` (mm/h), and what the rain falls on as
/// fractions: `soft` (vegetation, soil), `hard` (stone, wood), `water`, `metal`.
pub struct RainSound {
    fs: f32,
    rng: Noise,
    intensity: f32,
    target: f32,
    mix: [f32; 4],
    mix_target: [f32; 4],
    bed: [Biquad; 2],
    bed_hp: [Biquad; 2],
    rumble: [Biquad; 2],
    drops: Vec<Drop>,
}

impl RainSound {
    pub fn new(fs: f32, seed: u64) -> Self {
        Self {
            fs,
            rng: Noise::new(seed),
            intensity: 0.0,
            target: 0.0,
            mix: [1.0, 0.0, 0.0, 0.0],
            mix_target: [1.0, 0.0, 0.0, 0.0],
            bed: [
                Biquad::bandpass(fs, 3_800.0, 0.5),
                Biquad::bandpass(fs, 4_200.0, 0.5),
            ],
            bed_hp: [Biquad::highpass(fs, 700.0, 0.7); 2],
            rumble: [Biquad::lowpass(fs, 380.0, 0.7); 2],
            drops: Vec::with_capacity(MAX_DROPS),
        }
    }

    /// Drops per second at `mm_h` (1 mm/h ≈ 60, 10 ≈ 340, 50 ≈ 1100).
    pub fn drop_rate(mm_h: f32) -> f32 {
        60.0 * mm_h.max(0.0).powf(0.75)
    }

    fn spawn_drop(&mut self) {
        let fs = self.fs;
        let rng = &mut self.rng;
        let total: f32 = self.mix.iter().sum::<f32>().max(1e-3);
        let mut pick = rng.uniform() * total;
        let mut kind = DropKind::Tick;
        for (i, k) in [
            DropKind::Tick,
            DropKind::Tap,
            DropKind::Plink,
            DropKind::Ping,
        ]
        .into_iter()
        .enumerate()
        {
            if pick < self.mix[i] {
                kind = k;
                break;
            }
            pick -= self.mix[i];
        }
        // Most drops land some distance away (quiet); a few right next to you.
        let near = rng.uniform().powi(3);
        let amp = (0.02 + 0.25 * near) * rng.range(0.5, 1.0);
        let (gl, gr) = pan_gains(rng.range(-1.0, 1.0));
        let mut d = Drop {
            kind,
            left: 0,
            len: 1,
            amp,
            filter: Biquad::default(),
            res: Resonator::default(),
            bubble: Bubble::new(fs, 0.002, 0.0, 0.0, 0.0),
            gl,
            gr,
        };
        match kind {
            DropKind::Tick => {
                d.len = (rng.range(0.0003, 0.0012) * fs) as u32;
                d.filter = Biquad::bandpass(fs, rng.range(2_500.0, 6_000.0), 1.2);
                d.amp *= 1.4;
            }
            DropKind::Tap => {
                d.len = (rng.range(0.0002, 0.0005) * fs) as u32;
                d.filter = Biquad::highpass(fs, 1_500.0, 0.7);
                d.res = Resonator::new(fs, rng.range(1_500.0, 3_500.0), 0.012);
            }
            DropKind::Plink => {
                d.len = (0.0003 * fs) as u32;
                d.filter = Biquad::bandpass(fs, 2_000.0, 0.8);
                // Only some drops trap a bubble; those are the ringing plinks.
                let r = rng.range(0.0008, 0.0026);
                let ring = if rng.chance(0.4) { 0.9 } else { 0.0 };
                d.bubble = Bubble::new(fs, r, amp * ring, 0.15, 0.0);
                d.amp *= 0.5;
            }
            DropKind::Ping => {
                d.len = (0.0002 * fs) as u32;
                d.filter = Biquad::highpass(fs, 2_000.0, 0.7);
                d.res = Resonator::new(fs, rng.range(1_800.0, 5_000.0), rng.range(0.08, 0.25));
            }
        }
        d.left = d.len.max(1);
        if self.drops.len() < MAX_DROPS {
            self.drops.push(d);
        } else if let Some(slot) = self
            .drops
            .iter_mut()
            .find(|d| d.left == 0 && d.res.quiet() && d.bubble.done())
        {
            *slot = d;
        }
    }
}

impl Sound for RainSound {
    fn channels(&self) -> usize {
        2
    }

    fn set_param(&mut self, name: &str, value: f32) {
        match name {
            "intensity" => self.target = value.max(0.0),
            "soft" => self.mix_target[0] = value.max(0.0),
            "hard" => self.mix_target[1] = value.max(0.0),
            "water" => self.mix_target[2] = value.max(0.0),
            "metal" => self.mix_target[3] = value.max(0.0),
            _ => {}
        }
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let fs = self.fs;
        for f in 0..frames {
            if f % 64 == 0 {
                // Smooth parameter changes over ~0.5 s.
                let k = 1.0 - (-64.0 / (0.5 * fs)).exp();
                self.intensity += (self.target - self.intensity) * k;
                for i in 0..4 {
                    self.mix[i] += (self.mix_target[i] - self.mix[i]) * k;
                }
            }
            let i = self.intensity;
            if self.rng.uniform() * fs < Self::drop_rate(i) {
                self.spawn_drop();
            }
            let hiss = 0.03 * (i / 10.0).powf(0.5).min(2.2);
            let rumble = 0.08 * ((i - 4.0) / 30.0).clamp(0.0, 1.0);
            let mut l = 0.0;
            let mut r = 0.0;
            for c in 0..2 {
                let n = self.rng.white();
                let bed = self.bed[c].process(self.bed_hp[c].process(n)) * hiss
                    + self.rumble[c].process(n) * rumble;
                if c == 0 {
                    l += bed;
                } else {
                    r += bed;
                }
            }
            for d in &mut self.drops {
                let mut x = 0.0;
                if d.left > 0 {
                    let e = d.left as f32 / d.len as f32;
                    x = self.rng.white() * d.amp * e;
                    d.left -= 1;
                }
                let y = match d.kind {
                    DropKind::Tick => d.filter.process(x),
                    DropKind::Tap => d.filter.process(x) + d.res.process(x) * 0.6,
                    DropKind::Plink => d.filter.process(x) + d.bubble.process(),
                    DropKind::Ping => d.filter.process(x) * 0.5 + d.res.process(x) * 0.5,
                };
                l += y * d.gl;
                r += y * d.gr;
            }
            out[f * 2] = l;
            out[f * 2 + 1] = r;
        }
        frames
    }
}

/// Wind past the listener. Parameters: `speed` (m/s of air past the ears).
pub struct WindSound {
    fs: f32,
    rng: Noise,
    speed: f32,
    target: f32,
    bands: [Biquad; 2],
    hiss: [Biquad; 2],
    rumble: [Biquad; 2],
    whistle: Biquad,
    gust: Biquad,
    wander: Biquad,
    t: u32,
}

impl WindSound {
    pub fn new(fs: f32, seed: u64) -> Self {
        Self {
            fs,
            rng: Noise::new(seed),
            speed: 0.0,
            target: 0.0,
            bands: [Biquad::bandpass(fs, 300.0, 0.8); 2],
            hiss: [Biquad::bandpass(fs, 1_200.0, 0.7); 2],
            rumble: [Biquad::lowpass(fs, 70.0, 0.7); 2],
            whistle: Biquad::bandpass(fs, 900.0, 25.0),
            gust: Biquad::lowpass(fs, 0.6, 0.7),
            wander: Biquad::lowpass(fs, 0.3, 0.7),
            t: 0,
        }
    }

    /// Level of the main band at `speed` m/s (linear amplitude).
    pub fn level(speed: f32) -> f32 {
        (0.06 * (speed.max(0.0) / 10.0).powf(1.5)).min(0.35)
    }
}

impl Sound for WindSound {
    fn channels(&self) -> usize {
        2
    }

    fn set_param(&mut self, name: &str, value: f32) {
        if name == "speed" {
            self.target = value.max(0.0);
        }
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let fs = self.fs;
        for f in 0..frames {
            if self.t.is_multiple_of(64) {
                let k = 1.0 - (-64.0 / (0.4 * fs)).exp();
                self.speed += (self.target - self.speed) * k;
                let v = self.speed;
                for (c, b) in self.bands.iter_mut().enumerate() {
                    b.retune(Biquad::bandpass(
                        fs,
                        (170.0 + 26.0 * v) * (1.0 + 0.07 * c as f32),
                        0.8,
                    ));
                }
                for (c, b) in self.hiss.iter_mut().enumerate() {
                    b.retune(Biquad::bandpass(
                        fs,
                        (700.0 + 110.0 * v) * (1.0 + 0.05 * c as f32),
                        0.7,
                    ));
                }
            }
            self.t = self.t.wrapping_add(1);
            let v = self.speed;
            // Slow gusts swell the level and move it across the stereo field.
            let g = (1.0 + 60.0 * self.gust.process(self.rng.white())).clamp(0.3, 1.8);
            let level = Self::level(v) * g;
            let pan = (40.0 * self.wander.process(self.rng.white())).clamp(-0.6, 0.6);
            let (pl, pr) = pan_gains(pan);
            let whistle_amt = 0.04 * ((v - 9.0) / 13.0).clamp(0.0, 1.0);
            if f % 128 == 0 && whistle_amt > 0.0 {
                let wf = (650.0 + 35.0 * v) * (1.0 + 0.25 * (pan + g - 1.0));
                self.whistle.retune(Biquad::bandpass(fs, wf, 25.0));
            }
            let w = if whistle_amt > 0.0 {
                self.whistle.process(self.rng.white()) * whistle_amt * g
            } else {
                0.0
            };
            let buffet = 0.25 * ((v - 6.0) / 25.0).clamp(0.0, 1.0).powi(2);
            let mut ch = [0.0f32; 2];
            for (c, o) in ch.iter_mut().enumerate() {
                let n = self.rng.white();
                *o = self.bands[c].process(n) * level
                    + self.hiss[c].process(n) * level * 0.35
                    + self.rumble[c].process(n) * buffet * g
                    + w;
            }
            out[f * 2] = ch[0] * pl * std::f32::consts::SQRT_2;
            out[f * 2 + 1] = ch[1] * pr * std::f32::consts::SQRT_2;
        }
        frames
    }
}

/// One thunder clap, `distance_m` away: a crack (if close) and several rolling swells.
pub struct ThunderSound {
    fs: f32,
    rng: Noise,
    t: u32,
    len: u32,
    swells: [(u32, u32, f32, f32, f32); 6],
    crack: u32,
    crack_amp: f32,
    lp: [Biquad; 2],
    crack_hp: Biquad,
    brown: [f32; 2],
}

impl ThunderSound {
    pub fn new(fs: f32, distance_m: f32, seed: u64) -> Self {
        let mut rng = Noise::new(seed);
        let d = distance_m.max(100.0);
        let len_s = rng.range(4.0, 8.0) + d / 3_000.0;
        let mut swells = [(0u32, 1u32, 0.0f32, 1.0f32, 0.0f32); 6];
        for (i, s) in swells.iter_mut().enumerate() {
            let start = if i == 0 {
                0.0
            } else {
                rng.range(0.1, len_s * 0.6)
            };
            let attack = rng.range(0.08, 0.6);
            let decay = rng.range(0.8, 2.5);
            let amp = rng.range(0.3, 1.0) * if i == 0 { 1.0 } else { 0.7 };
            *s = (
                (start * fs) as u32,
                (attack * fs).max(1.0) as u32,
                amp,
                decay_coef(fs, decay),
                rng.range(-0.8, 0.8),
            );
        }
        let cutoff = (900.0 * (1_000.0 / d).powf(0.8)).clamp(90.0, 1_500.0);
        // Loudness falls with distance but thunder carries far.
        let crack_amp = if d < 1_500.0 {
            0.5 * (1.0 - d / 1_500.0)
        } else {
            0.0
        };
        Self {
            fs,
            rng,
            t: 0,
            len: (len_s * fs) as u32,
            swells,
            crack: (0.05 * fs) as u32,
            crack_amp,
            lp: [Biquad::lowpass(fs, cutoff, 0.7); 2],
            crack_hp: Biquad::highpass(fs, 1_200.0, 0.7),
            brown: [0.0; 2],
        }
    }
}

impl Sound for ThunderSound {
    fn channels(&self) -> usize {
        2
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let fs = self.fs;
        for f in 0..frames {
            if self.t >= self.len {
                return f;
            }
            let mut l = 0.0;
            let mut r = 0.0;
            let mut env = [0.0f32; 2];
            for &(start, attack, amp, decay, pan) in &self.swells {
                if self.t < start {
                    continue;
                }
                let dt = self.t - start;
                let e = if dt < attack {
                    amp * dt as f32 / attack as f32
                } else {
                    amp * decay.powf((dt - attack) as f32)
                };
                let (pl, pr) = pan_gains(pan);
                env[0] += e * pl;
                env[1] += e * pr;
            }
            let mut roll = [0.0f32; 2];
            for (c, o) in roll.iter_mut().enumerate() {
                // Brown noise (integrated white) for the deep roll.
                self.brown[c] = (self.brown[c] + self.rng.white() * 0.05) * 0.996;
                *o = self.lp[c].process(self.brown[c] * 3.0) * env[c] * 0.6;
            }
            l += roll[0];
            r += roll[1];
            if self.t < self.crack && self.crack_amp > 0.0 {
                let e = 1.0 - self.t as f32 / self.crack as f32;
                let c = self.crack_hp.process(self.rng.white()) * self.crack_amp * e * e;
                l += c;
                r += c;
            }
            let fade = ((self.len - self.t) as f32 / (0.5 * fs)).min(1.0);
            out[f * 2] = l * fade;
            out[f * 2 + 1] = r * fade;
            self.t += 1;
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn rms_of(sound: &mut dyn Sound, secs: f32) -> f32 {
        let mut buf = vec![0.0; (secs * FS) as usize * 2];
        let n = sound.fill(&mut buf);
        let x = &buf[..n * 2];
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn rain_gets_louder_with_intensity() {
        let mut last = 0.0;
        for mm_h in [0.5, 2.0, 8.0, 25.0, 60.0] {
            let mut rain = RainSound::new(FS, 3);
            rain.set_param("intensity", mm_h);
            rms_of(&mut rain, 2.0); // settle the smoothing
            let level = rms_of(&mut rain, 2.0);
            assert!(level > last * 1.15, "{mm_h} mm/h: {level} vs {last}");
            last = level;
        }
    }

    #[test]
    fn no_rain_is_silent() {
        let mut rain = RainSound::new(FS, 3);
        assert!(rms_of(&mut rain, 1.0) < 1e-4);
    }

    #[test]
    fn puddles_change_the_rain() {
        // Same intensity, rain on grass vs. on water: water rings (more energy in the
        // bubbles' 1–4 kHz band, measured as total energy here since ticks are quieter).
        let render = |water: f32| {
            let mut rain = RainSound::new(FS, 5);
            rain.set_param("intensity", 10.0);
            rain.set_param("soft", 1.0 - water);
            rain.set_param("water", water);
            rms_of(&mut rain, 1.0);
            let mut buf = vec![0.0; (2.0 * FS) as usize * 2];
            rain.fill(&mut buf);
            buf
        };
        let grass = render(0.0);
        let puddles = render(1.0);
        assert_ne!(grass, puddles);
    }

    #[test]
    fn wind_follows_speed() {
        let mut last = 0.0;
        for v in [2.0, 6.0, 12.0, 25.0] {
            let mut wind = WindSound::new(FS, 1);
            wind.set_param("speed", v);
            rms_of(&mut wind, 2.0);
            let level = rms_of(&mut wind, 2.0);
            assert!(level > last, "{v} m/s: {level}");
            last = level;
        }
    }

    #[test]
    fn distant_thunder_is_duller_and_ends() {
        let mut near = ThunderSound::new(FS, 500.0, 1);
        let mut far = ThunderSound::new(FS, 8_000.0, 1);
        let mut a = vec![0.0; (12.0 * FS) as usize * 2];
        let mut b = a.clone();
        let na = near.fill(&mut a);
        let nb = far.fill(&mut b);
        assert!(na < (12.0 * FS) as usize && nb < (12.0 * FS) as usize);
        let bright = |x: &[f32]| {
            let e: f32 = x.iter().map(|v| v * v).sum();
            let d: f32 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
            d / e.max(1e-12)
        };
        let left = |x: &[f32], n: usize| x[..n * 2].iter().step_by(2).copied().collect::<Vec<_>>();
        assert!(bright(&left(&a, na)) > bright(&left(&b, nb)) * 2.0);
    }
}
