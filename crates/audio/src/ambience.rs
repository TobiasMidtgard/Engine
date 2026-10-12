//! Ambience: rain and wind from the weather, and fire, synthesized live.
//!
//! * **Rain** is a stereo bed of hiss plus individual drops, each hitting something near the
//!   listener: leaves and grass tick softly, stone and concrete tap, puddles and lakes plink
//!   (a ringing bubble), metal roofs ping. As puddles grow the mix shifts toward plinks, so
//!   you hear the ground getting wet. Louder and denser with intensity (mm/h).
//! * **Wind** is band-passed noise whose pitch and level follow the air speed past the
//!   listener (wind plus your own motion), with buffeting at high speed and a whistle in
//!   storms.
//! * **Fire** roars, crackles and pops more densely and deeply the more area is in flames.
//!
//! Thunder lives in [`crate::thunder`]: it rolls along the lightning channel.

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

/// A fire: the low roar of the flames, crackle and pops from the fuel, now and then a loud
/// snap. Mono, so the mixer places it. Param `"size"`: area in flames, m² (0 = out; a
/// campfire ≈ 1, a burning house ≈ 100, a forest front ≈ 2000).
pub struct FireSound {
    fs: f32,
    rng: Noise,
    size: f32,
    target: f32,
    roar: Biquad,
    roar2: Biquad,
    flutter: Biquad,
    hiss: Biquad,
    pops: Vec<Pop>,
    t: u32,
    // Per-block levels.
    roar_amp: f32,
    hiss_amp: f32,
    pop_rate: f32,
    snap_rate: f32,
}

struct Pop {
    filter: Biquad,
    amp: f32,
    decay: f32,
}

/// Most pops sounding at once.
const MAX_POPS: usize = 24;

impl FireSound {
    pub fn new(fs: f32, seed: u64) -> Self {
        Self {
            fs,
            rng: Noise::new(seed),
            size: 0.0,
            target: 0.0,
            roar: Biquad::lowpass(fs, 400.0, 0.7),
            roar2: Biquad::lowpass(fs, 400.0, 0.7),
            flutter: Biquad::lowpass(fs, 6.0, 0.7),
            hiss: Biquad::highpass(fs, 3_000.0, 0.7),
            pops: Vec::with_capacity(MAX_POPS),
            t: 0,
            roar_amp: 0.0,
            hiss_amp: 0.0,
            pop_rate: 0.0,
            snap_rate: 0.0,
        }
    }

    /// Overall loudness for `size` m² of flames (linear, at the fire's reference distance).
    pub fn level(size: f32) -> f32 {
        if size <= 0.0 {
            0.0
        } else {
            (0.12 * (1.0 + size).ln().powf(0.8)).min(0.8)
        }
    }

    fn pop(&mut self, snap: bool) {
        if self.pops.len() >= MAX_POPS {
            return;
        }
        let fs = self.fs;
        let r = &mut self.rng;
        // Many small pops, a few loud ones (Pareto-like).
        let amp = (0.05 * r.uniform().max(1e-3).powf(-0.55)).min(1.0);
        let (f, q, t60) = if snap {
            (r.range(700.0, 1_600.0), 1.2, r.range(0.012, 0.03))
        } else {
            (r.range(1_800.0, 6_500.0), 1.8, r.range(0.002, 0.008))
        };
        self.pops.push(Pop {
            filter: Biquad::bandpass(fs, f, q),
            amp: if snap { amp.max(0.35) * 1.4 } else { amp },
            decay: decay_coef(fs, t60),
        });
    }
}

impl Sound for FireSound {
    fn set_param(&mut self, name: &str, value: f32) {
        if name == "size" {
            self.target = value.max(0.0);
        }
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        for o in out.iter_mut() {
            if self.t.is_multiple_of(64) {
                let k = 1.0 - (-64.0 / (0.5 * fs)).exp();
                self.size += (self.target - self.size) * k;
                let level = Self::level(self.size);
                // Bigger fires roar deeper and crackle more densely.
                let cutoff = (520.0 / (1.0 + self.size).powf(0.12)).max(140.0);
                self.roar.retune(Biquad::lowpass(fs, cutoff, 0.7));
                self.roar2.retune(Biquad::lowpass(fs, cutoff * 1.6, 0.7));
                self.roar_amp = level * (0.5 + 0.5 * (self.size / 30.0).min(1.0));
                self.hiss_amp = level * 0.05;
                self.pop_rate = if self.size > 0.0 {
                    (12.0 * self.size.sqrt()).clamp(4.0, 400.0) / fs
                } else {
                    0.0
                };
                self.snap_rate = if self.size > 0.0 {
                    (0.4 + 0.15 * self.size.sqrt()).min(6.0) / fs
                } else {
                    0.0
                };
            }
            self.t = self.t.wrapping_add(1);
            if self.rng.chance(self.pop_rate) {
                self.pop(false);
            }
            if self.rng.chance(self.snap_rate) {
                self.pop(true);
            }
            let level = Self::level(self.size);
            // Turbulent flutter of the flames: slow random swells.
            let flutter = 1.0 + 1.8 * self.flutter.process(self.rng.white());
            let n = self.rng.white();
            let roar =
                self.roar2.process(self.roar.process(n)) * 2.2 * self.roar_amp * flutter.max(0.0);
            let hiss = self.hiss.process(self.rng.white()) * self.hiss_amp;
            let mut crackle = 0.0;
            for p in &mut self.pops {
                crackle += p.filter.process(self.rng.white()) * p.amp;
                p.amp *= p.decay;
            }
            self.pops.retain(|p| p.amp > 1e-4);
            *o = roar + hiss + crackle * level * 1.6;
        }
        out.len()
    }
}

/// Water at the shore and in streams. Mono, so the mixer places it.
///
/// * `"waves"` 0..1: small waves lapping at the shore, one every couple of seconds: a slap as
///   it hits, the wash running up and draining back, and the gurgle of bubbles in the
///   backwash. Bigger waves come slower and wash longer.
/// * `"flow"` m/s: moving water babbling over stones, a continuous run of bubbles and a
///   hiss that grows into a roar in rapids.
/// * `"spray"` kg/s: water falling or thrown into water (a waterfall's foot, a boat's bow
///   wave): a broad roar with a deep rumble for big falls and dense bubble chatter.
pub struct WaterSound {
    fs: f32,
    rng: Noise,
    waves: f32,
    waves_target: f32,
    flow: f32,
    flow_target: f32,
    spray: f32,
    spray_target: f32,
    spray_band: Biquad,
    spray_low: Biquad,
    /// Samples until the next wave reaches the shore.
    next_wave: u32,
    /// The current wave: samples since it hit, its size.
    wave_t: u32,
    wave_size: f32,
    slap: Biquad,
    wash: [Biquad; 2],
    hiss: Biquad,
    roar: Biquad,
    bubbles: Vec<Bubble>,
    t: u32,
}

/// Most bubbles ringing at once.
const MAX_BUBBLES: usize = 40;

impl WaterSound {
    pub fn new(fs: f32, seed: u64) -> Self {
        let mut rng = Noise::new(seed);
        let first = (rng.range(0.2, 1.5) * fs) as u32;
        Self {
            fs,
            rng,
            waves: 0.0,
            waves_target: 0.0,
            flow: 0.0,
            flow_target: 0.0,
            spray: 0.0,
            spray_target: 0.0,
            spray_band: Biquad::bandpass(fs, 900.0, 0.45),
            spray_low: Biquad::lowpass(fs, 220.0, 0.7),
            next_wave: first,
            wave_t: u32::MAX,
            wave_size: 0.0,
            slap: Biquad::bandpass(fs, 700.0, 0.9),
            wash: [
                Biquad::lowpass(fs, 2_200.0, 0.7),
                Biquad::highpass(fs, 180.0, 0.7),
            ],
            hiss: Biquad::bandpass(fs, 2_800.0, 0.5),
            roar: Biquad::lowpass(fs, 500.0, 0.7),
            bubbles: Vec::with_capacity(MAX_BUBBLES),
            t: 0,
        }
    }

    fn bubble(&mut self, small: f32, big: f32, amp: f32, delay_s: f32) {
        if self.bubbles.len() >= MAX_BUBBLES {
            return;
        }
        // Mostly small bubbles, a few big ones (bigger = lower and louder).
        let r = small + (big - small) * self.rng.uniform().powi(3);
        let loud = amp * (r / big).sqrt();
        let rise = self.rng.range(0.05, 0.3);
        self.bubbles
            .push(Bubble::new(self.fs, r, loud, rise, delay_s).damped(1.5, self.fs));
    }
}

impl Sound for WaterSound {
    fn set_param(&mut self, name: &str, value: f32) {
        match name {
            "waves" => self.waves_target = value.clamp(0.0, 1.0),
            "flow" => self.flow_target = value.max(0.0),
            "spray" => self.spray_target = value.max(0.0),
            _ => {}
        }
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        let k = 1.0 - (-1.0 / (0.8 * fs)).exp();
        for o in out.iter_mut() {
            self.waves += (self.waves_target - self.waves) * k;
            self.flow += (self.flow_target - self.flow) * k;
            self.spray += (self.spray_target - self.spray) * k;

            // A wave reaches the shore.
            if self.waves > 0.01 {
                if self.next_wave == 0 {
                    self.wave_t = 0;
                    self.wave_size = self.waves * self.rng.range(0.5, 1.0);
                    let period = self.rng.range(1.4, 2.6) * (0.8 + 1.4 * self.waves);
                    self.next_wave = (period * fs) as u32;
                    // Backwash bubbles over the next second.
                    let n = (4.0 + 14.0 * self.wave_size) as usize;
                    let amp = 0.15 * self.wave_size.powf(0.7);
                    for _ in 0..n {
                        let at = self.rng.range(0.25, 1.4);
                        self.bubble(0.0015, 0.007, amp, at);
                    }
                } else {
                    self.next_wave -= 1;
                }
            }
            let mut y = 0.0;
            if self.wave_t != u32::MAX {
                let t = self.wave_t as f32 / fs;
                let size = self.wave_size;
                let loud = size.powf(0.7);
                let up = 0.35 + 0.4 * size;
                let back = 0.9 + 1.2 * size;
                if t > up + back {
                    self.wave_t = u32::MAX;
                } else {
                    // Slap: a short knock as the wave lands.
                    let slap = (-t / 0.025).exp() * loud * 2.0;
                    // Wash: up the shore, then draining back, swishing as it goes.
                    let wash = if t < up {
                        (t / up).powi(2)
                    } else {
                        (1.0 - (t - up) / back).max(0.0).powi(2)
                    } * loud
                        * 0.5;
                    let n = self.rng.white();
                    let slap = self.slap.process(n) * slap;
                    let low = self.wash[0].process(self.rng.white());
                    let wash = self.wash[1].process(low) * wash;
                    y += slap + wash;
                    self.wave_t += 1;
                }
            }

            // Moving water: bubbles popping up all the time, a hiss, a roar in rapids.
            if self.flow > 0.05 {
                let rate = (25.0 * self.flow).min(220.0) / fs;
                if self.rng.chance(rate) {
                    let amp = 0.08 * self.flow.min(2.0).sqrt();
                    self.bubble(0.0012, 0.006, amp, 0.0);
                }
                let hiss = self.hiss.process(self.rng.white()) * 0.05 * self.flow.min(3.0);
                let rapids = ((self.flow - 1.0) / 2.0).clamp(0.0, 1.0);
                let roar = self.roar.process(self.rng.white()) * 0.4 * rapids;
                y += hiss + roar;
            }

            // Falling water: roar and chatter that grow with the mass coming down.
            if self.spray > 0.01 {
                let level = (0.05 * self.spray.powf(0.4)).min(0.4);
                let roar = self.spray_band.process(self.rng.white()) * level;
                let big = ((self.spray - 5.0) / 50.0).clamp(0.0, 1.0);
                let rumble = self.spray_low.process(self.rng.white()) * level * 1.5 * big;
                if self
                    .rng
                    .chance((60.0 + 30.0 * self.spray.sqrt()).min(400.0) / fs)
                {
                    self.bubble(0.001, 0.005, level * 0.5, 0.0);
                }
                y += roar + rumble;
            }

            let mut b = 0.0;
            for bubble in &mut self.bubbles {
                b += bubble.process();
            }
            if self.t.is_multiple_of(256) {
                self.bubbles.retain(|b| !b.done());
            }
            self.t = self.t.wrapping_add(1);
            *o = y + b;
        }
        out.len()
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

    fn render(sound: &mut dyn Sound, secs: f32) -> Vec<f32> {
        let mut buf = vec![0.0; (secs * FS) as usize];
        let n = sound.fill(&mut buf);
        buf.truncate(n);
        buf
    }

    fn rms_of_mono(sound: &mut dyn Sound, secs: f32) -> f32 {
        let x = render(sound, secs);
        let x = &x[x.len() / 4..];
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
    fn fire_grows_and_goes_out() {
        let render = |size: f32| {
            let mut f = FireSound::new(FS, 9);
            f.set_param("size", size);
            let mut out = vec![0.0; (FS * 3.0) as usize];
            f.fill(&mut out);
            let x = &out[(FS * 1.5) as usize..];
            (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
        };
        let (out, camp, house, forest) = (render(0.0), render(1.0), render(100.0), render(2_000.0));
        assert!(out < 1e-4, "a fire that's out is silent: {out}");
        assert!(camp > 0.005, "a campfire is audible: {camp}");
        assert!(
            house > camp * 1.5 && forest > house,
            "{camp} {house} {forest}"
        );
        assert!(forest < 0.6, "{forest}");
    }

    #[test]
    fn water_laps_in_waves_and_babbles_when_it_flows() {
        let mut still = WaterSound::new(FS, 4);
        assert!(rms_of_mono(&mut still, 3.0) < 1e-4, "still water is silent");

        let mut lake = WaterSound::new(FS, 4);
        lake.set_param("waves", 0.5);
        let x = render(&mut lake, 12.0);
        // Waves come and go: the level per half second swings.
        let levels: Vec<f32> = x
            .chunks((FS * 0.5) as usize)
            .skip(4)
            .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
            .collect();
        let (lo, hi) = levels
            .iter()
            .fold((f32::MAX, 0.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(hi > 0.005 && hi > lo * 3.0, "waves {lo}..{hi}");

        let mut brook = WaterSound::new(FS, 5);
        brook.set_param("flow", 0.8);
        let calm = rms_of_mono(&mut brook, 4.0);
        let mut rapids = WaterSound::new(FS, 5);
        rapids.set_param("flow", 3.0);
        let wild = rms_of_mono(&mut rapids, 4.0);
        assert!(
            calm > 0.003 && wild > calm * 1.5,
            "brook {calm} rapids {wild}"
        );
    }
}
