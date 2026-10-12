//! Vehicle sounds: engines and tyres, synthesized from what the vehicle is doing.
//!
//! * [`EngineSound`] for a piston engine fires one exhaust pulse per cylinder per two
//!   crankshaft turns, in firing order with each engine's own small unevenness (what makes a
//!   V8 burble), through the exhaust pipe's and silencer's resonances, with intake roar under
//!   load, valve-train tick, turbo whine and pops on the overrun. Turbines whine and roar,
//!   rockets crackle and thunder, electric motors sing, thrusters hiss.
//! * [`TyreSound`] rolls, crunches, squelches and hisses by ground, and squeals when it
//!   slides on hard ground.

use crate::Sound;
use crate::dsp::{Biquad, Bubble, Noise, OnePole, Resonator};

/// What kind of engine to synthesize.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EngineVoice {
    /// A four-stroke piston engine with this many cylinders and this much displacement,
    /// litres (bigger engines sound deeper).
    Piston {
        cylinders: u8,
        litres: f32,
    },
    /// Jet, turboprop or helicopter turbine.
    Turbine,
    Rocket,
    Electric,
    /// Reaction-control or ion thrusters.
    Thrusters,
}

/// A running engine. Mono; params:
/// `"rpm"`, `"throttle"` 0..1, `"load"` 0..1 (how hard it is working), `"boost"` 0..1 (turbo
/// spool or afterburner), `"thrust"` N (turbines, rockets, thrusters), `"running"` 0 or 1.
pub struct EngineSound {
    fs: f32,
    voice: EngineVoice,
    rng: Noise,
    // Targets and smoothed values.
    target: [f32; 6],
    value: [f32; 6],
    /// Position in the engine cycle (two crankshaft turns for a four-stroke), 0..1.
    cycle: f32,
    /// Each cylinder's firing point in the cycle and its strength.
    fire_at: Vec<(f32, f32)>,
    pulse: f32,
    pipe: Resonator,
    pipe2: Resonator,
    silencer: Resonator,
    body: Biquad,
    exhaust_noise: Biquad,
    intake: Biquad,
    tick: Biquad,
    whine_phase: [f32; 3],
    roar: [Biquad; 2],
    hiss: Biquad,
    pops: Vec<(Biquad, f32, f32)>,
    rumble: [Biquad; 2],
    brown: f32,
    dc: Biquad,
    last_throttle: f32,
    blow_off: f32,
}

const RPM: usize = 0;
const THROTTLE: usize = 1;
const LOAD: usize = 2;
const BOOST: usize = 3;
const THRUST: usize = 4;
const RUNNING: usize = 5;

impl EngineSound {
    pub fn new(fs: f32, voice: EngineVoice, seed: u64) -> Self {
        let mut rng = Noise::new(seed);
        let (fire_at, pipe_hz, silencer_hz) = match voice {
            EngineVoice::Piston { cylinders, litres } => {
                let n = cylinders.clamp(1, 16) as usize;
                // Firing order spacing with each engine's own small unevenness; V8s with a
                // cross-plane crank fire in uneven pairs, which is their burble.
                let uneven = if n == 8 { 0.18 } else { 0.05 };
                let fire = (0..n)
                    .map(|k| {
                        let jitter = rng.range(-uneven, uneven) / n as f32;
                        (
                            (k as f32 / n as f32 + jitter).rem_euclid(1.0),
                            rng.range(0.8, 1.0),
                        )
                    })
                    .collect();
                let size = litres.clamp(0.1, 10.0);
                (fire, 160.0 / size.powf(0.25), 520.0 / size.powf(0.2))
            }
            _ => (Vec::new(), 120.0, 500.0),
        };
        Self {
            fs,
            voice,
            rng,
            target: [800.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            value: [800.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            cycle: 0.0,
            fire_at,
            pulse: 0.0,
            pipe: Resonator::new(fs, pipe_hz, 0.05),
            pipe2: Resonator::new(fs, pipe_hz * 2.7, 0.03),
            silencer: Resonator::new(fs, silencer_hz, 0.02),
            body: Biquad::lowpass(fs, 900.0, 0.7),
            exhaust_noise: Biquad::bandpass(fs, 300.0, 0.8),
            intake: Biquad::bandpass(fs, 1_400.0, 1.2),
            tick: Biquad::bandpass(fs, 6_000.0, 2.0),
            whine_phase: [0.0; 3],
            roar: [
                // Rockets roar much brighter than jets.
                Biquad::lowpass(
                    fs,
                    if voice == EngineVoice::Rocket {
                        3_000.0
                    } else {
                        1_200.0
                    },
                    0.7,
                ),
                Biquad::highpass(fs, 40.0, 0.7),
            ],
            hiss: Biquad::highpass(fs, 2_500.0, 0.7),
            pops: Vec::new(),
            rumble: [
                Biquad::highpass(fs, 35.0, 0.7),
                Biquad::lowpass(fs, 160.0, 0.7),
            ],
            brown: 0.0,
            dc: Biquad::highpass(fs, 25.0, 0.7),
            last_throttle: 0.0,
            blow_off: 0.0,
        }
    }

    fn pop(&mut self, amp: f32, lo: f32, hi: f32) {
        if self.pops.len() < 16 {
            let f = self.rng.range(lo, hi);
            let decay = (-1.0 / (self.rng.range(0.004, 0.02) * self.fs)).exp();
            self.pops
                .push((Biquad::bandpass(self.fs, f, 1.5), amp, decay));
        }
    }

    /// Deep noise, 35 to 160 Hz.
    fn rumble_out(&mut self) -> f32 {
        self.brown = (self.brown + self.rng.white() * 0.05) * 0.99;
        let low = self.rumble[0].process(self.brown);
        self.rumble[1].process(low) * 2.0
    }

    fn pops_out(&mut self) -> f32 {
        let mut y = 0.0;
        for (f, amp, decay) in &mut self.pops {
            y += f.process(self.rng.white()) * *amp;
            *amp *= *decay;
        }
        self.pops.retain(|p| p.1 > 1e-4);
        y
    }

    fn piston(&mut self) -> f32 {
        let fs = self.fs;
        let rpm = self.value[RPM].max(0.0);
        let throttle = self.value[THROTTLE];
        let load = self.value[LOAD];
        let running = self.value[RUNNING];
        // One cycle per two crankshaft turns.
        let step = rpm / 60.0 / 2.0 / fs;
        let before = self.cycle;
        self.cycle = (self.cycle + step).fract();
        let wrapped = self.cycle < before;
        let mut kick = 0.0;
        if running > 0.01 {
            for &(at, strength) in &self.fire_at {
                let crossed = if wrapped {
                    at > before || at <= self.cycle
                } else {
                    at > before && at <= self.cycle
                };
                if crossed {
                    // Harder combustion under load; a light misfire now and then at idle.
                    let misfire = if load < 0.1 && self.rng.chance(0.03) {
                        0.4
                    } else {
                        1.0
                    };
                    kick += strength * (0.35 + 0.65 * load) * misfire * running;
                }
            }
        }
        // Each firing is a sharp pressure step that decays before the next.
        let firing_hz = rpm / 60.0 / 2.0 * self.fire_at.len() as f32;
        let decay = (-firing_hz.max(10.0) * 3.0 / fs).exp();
        self.pulse = self.pulse * decay + kick;
        let p = self.pulse;
        let roar = self.exhaust_noise.process(self.rng.white()) * p * 0.6;
        // Each firing knocks the exhaust pipe and silencer into ringing; the pulse train
        // itself is the engine's drone.
        let ring = self.pipe.process(kick * 0.5)
            + self.pipe2.process(kick * 0.2)
            + self.silencer.process(kick * 0.25);
        let exhaust = self.body.process(p * 0.3 + ring + roar);
        // Intake: air rushing in under throttle, rising with rpm.
        let rev = (rpm / 7_000.0).clamp(0.0, 1.5);
        let intake = self.intake.process(self.rng.white()) * throttle * rev * 0.08 * running;
        // Valve train: a fine tick that thickens with rpm.
        let tick = self.tick.process(self.rng.white()) * p * 0.05;
        // Turbo whine, and a hiss when the throttle snaps shut on boost.
        let boost = self.value[BOOST];
        let mut turbo = 0.0;
        if boost > 0.01 {
            let f = 2_000.0 + 9_000.0 * boost;
            self.whine_phase[0] = (self.whine_phase[0] + f / fs).fract();
            turbo = (self.whine_phase[0] * std::f32::consts::TAU).sin() * boost * 0.015;
        }
        if self.last_throttle - throttle > 0.4 && boost > 0.3 {
            self.blow_off = boost * 0.12;
        }
        self.last_throttle = throttle;
        turbo += self.hiss.process(self.rng.white()) * self.blow_off;
        self.blow_off *= (-1.0 / (0.35 * fs)).exp();
        // Overrun: off the throttle at high revs, unburnt fuel pops in the exhaust.
        if throttle < 0.05 && rev > 0.45 && running > 0.5 && self.rng.chance(6.0 * rev / fs) {
            let amp = self.rng.range(0.2, 0.6);
            self.pop(amp, 300.0, 1_200.0);
        }
        let gain = 0.5 + 0.5 * rev.min(1.0);
        (exhaust * 0.5 + intake + tick + turbo) * gain + self.pops_out()
    }

    fn turbine(&mut self) -> f32 {
        let fs = self.fs;
        let rpm = self.value[RPM].max(0.0);
        let thrust = self.value[THRUST].max(0.0);
        let running = self.value[RUNNING];
        // Fan blades passing: a whine and its overtones.
        let blade = rpm / 60.0 * 24.0;
        let mut whine = 0.0;
        for (k, ph) in self.whine_phase.iter_mut().enumerate() {
            let f = (blade * (k + 1) as f32 * (1.0 + 0.002 * k as f32)).min(fs * 0.45);
            *ph = (*ph + f / fs).fract();
            whine += (*ph * std::f32::consts::TAU).sin() / (k + 1) as f32;
        }
        let spool = (rpm / 12_000.0).clamp(0.0, 1.2);
        let level = (thrust / 50_000.0).max(0.0).powf(0.35).min(2.0);
        let low = self.roar[0].process(self.rng.white());
        let roar = self.roar[1].process(low) * level * 0.3;
        let boost = self.value[BOOST];
        let mut burner = 0.0;
        if boost > 0.01 {
            burner = self.rumble_out() * boost * 1.2;
            if self.rng.chance(40.0 * boost / fs) {
                self.pop(0.3 * boost, 400.0, 2_500.0);
            }
        }
        (whine * spool * 0.05 + roar + burner) * running + self.pops_out()
    }

    fn rocket(&mut self) -> f32 {
        let thrust = self.value[THRUST].max(0.0);
        let running = self.value[RUNNING];
        let level = (thrust / 1.0e5).powf(0.3).min(3.0) * running;
        if level <= 1e-4 {
            return self.pops_out();
        }
        let low = self.roar[0].process(self.rng.white());
        let roar = self.roar[1].process(low) * 0.35;
        let rumble = self.rumble_out();
        // The crackle of shock-laden exhaust: dense random pops.
        if self.rng.chance(220.0 * level.min(1.5) / self.fs) {
            let amp = 0.6 * self.rng.uniform().powi(3) + 0.05;
            self.pop(amp, 600.0, 4_000.0);
        }
        (roar + rumble) * level + self.pops_out() * level.min(1.0)
    }

    fn electric(&mut self) -> f32 {
        let fs = self.fs;
        let rpm = self.value[RPM].max(0.0);
        let load = self.value[LOAD];
        let running = self.value[RUNNING];
        let motor = rpm / 60.0 * 4.0;
        let mut y = 0.0;
        for (k, (ph, amp)) in self
            .whine_phase
            .iter_mut()
            .zip([1.0, 0.4, 0.25])
            .enumerate()
        {
            let f = (motor * [1.0, 3.0, 7.3][k]).min(fs * 0.45);
            *ph = (*ph + f / fs).fract();
            y += (*ph * std::f32::consts::TAU).sin() * amp;
        }
        let road = self.roar[0].process(self.rng.white()) * 0.02 * (rpm / 10_000.0).min(1.0);
        (y * 0.025 * (0.3 + 0.7 * load) + road) * running
    }

    fn thrusters(&mut self) -> f32 {
        let thrust = self.value[THRUST].max(0.0);
        let level = (thrust / 2_000.0).powf(0.4).min(2.0) * self.value[RUNNING];
        self.hiss.process(self.rng.white()) * level * 0.15
    }
}

impl Sound for EngineSound {
    fn set_param(&mut self, name: &str, value: f32) {
        let i = match name {
            "rpm" => RPM,
            "throttle" => THROTTLE,
            "load" => LOAD,
            "boost" => BOOST,
            "thrust" => THRUST,
            "running" => RUNNING,
            _ => return,
        };
        self.target[i] = if i == RPM || i == THRUST {
            value.max(0.0)
        } else {
            value.clamp(0.0, 1.0)
        };
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        // Revs and load glide (a flywheel can't jump), the rest follow quickly.
        let rates = [0.06, 0.03, 0.05, 0.25, 0.08, 0.3];
        for (i, o) in out.iter_mut().enumerate() {
            if i % 32 == 0 {
                for (k, rate) in rates.iter().enumerate() {
                    let a = 1.0 - (-32.0 / (rate * self.fs)).exp();
                    self.value[k] += (self.target[k] - self.value[k]) * a;
                }
            }
            let y = match self.voice {
                EngineVoice::Piston { .. } => self.piston(),
                EngineVoice::Turbine => self.turbine(),
                EngineVoice::Rocket => self.rocket(),
                EngineVoice::Electric => self.electric(),
                EngineVoice::Thrusters => self.thrusters(),
            };
            *o = self.dc.process(y);
        }
        out.len()
    }
}

/// The ground under a vehicle's tyres, as shares 0..1 (they needn't add up to 1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TyreGround {
    /// Asphalt, concrete, rock, metal.
    pub hard: f32,
    pub gravel: f32,
    /// Dirt, grass, sand.
    pub soft: f32,
    pub mud: f32,
    pub snow: f32,
    /// Standing water or a wet road (hiss and spray).
    pub wet: f32,
}

/// Tyres rolling and sliding. Mono; params `"speed"` m/s, `"slip"` 0..1, `"load"` kg per
/// tyre, and the ground shares `"hard"`, `"gravel"`, `"soft"`, `"mud"`, `"snow"`, `"wet"`.
pub struct TyreSound {
    fs: f32,
    rng: Noise,
    speed: f32,
    slip: f32,
    load: f32,
    ground: TyreGround,
    target: (f32, f32, f32, TyreGround),
    road: Biquad,
    road2: Biquad,
    rumble: Biquad,
    grains: Vec<(Biquad, f32, f32)>,
    bubbles: Vec<Bubble>,
    spray: Biquad,
    squeal_phase: f32,
    squeal_wobble: OnePole,
    squeal_level: f32,
}

impl TyreSound {
    pub fn new(fs: f32, seed: u64) -> Self {
        let ground = TyreGround {
            hard: 1.0,
            ..Default::default()
        };
        Self {
            fs,
            rng: Noise::new(seed),
            speed: 0.0,
            slip: 0.0,
            load: 400.0,
            ground,
            target: (0.0, 0.0, 400.0, ground),
            road: Biquad::bandpass(fs, 950.0, 0.9),
            road2: Biquad::lowpass(fs, 300.0, 0.7),
            rumble: Biquad::lowpass(fs, 160.0, 0.7),
            grains: Vec::new(),
            bubbles: Vec::new(),
            spray: Biquad::highpass(fs, 1_800.0, 0.7),
            squeal_phase: 0.0,
            squeal_wobble: OnePole::new(fs, 12.0),
            squeal_level: 0.0,
        }
    }

    fn grain(&mut self, amp: f32, lo: f32, hi: f32, ms: f32) {
        if self.grains.len() < 32 {
            let f = self.rng.range(lo, hi);
            let decay = (-1.0 / (ms * 0.001 * self.fs)).exp();
            self.grains
                .push((Biquad::bandpass(self.fs, f, 1.2), amp, decay));
        }
    }
}

impl Sound for TyreSound {
    fn set_param(&mut self, name: &str, value: f32) {
        let g = &mut self.target.3;
        match name {
            "speed" => self.target.0 = value.abs(),
            "slip" => self.target.1 = value.clamp(0.0, 1.0),
            "load" => self.target.2 = value.max(0.0),
            "hard" => g.hard = value.clamp(0.0, 1.0),
            "gravel" => g.gravel = value.clamp(0.0, 1.0),
            "soft" => g.soft = value.clamp(0.0, 1.0),
            "mud" => g.mud = value.clamp(0.0, 1.0),
            "snow" => g.snow = value.clamp(0.0, 1.0),
            "wet" => g.wet = value.clamp(0.0, 1.0),
            _ => {}
        }
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        for (i, o) in out.iter_mut().enumerate() {
            if i % 64 == 0 {
                let a = 1.0 - (-64.0 / (0.05 * fs)).exp();
                self.speed += (self.target.0 - self.speed) * a;
                self.slip += (self.target.1 - self.slip) * a;
                self.load += (self.target.2 - self.load) * a;
                let (g, t) = (&mut self.ground, self.target.3);
                g.hard += (t.hard - g.hard) * a;
                g.gravel += (t.gravel - g.gravel) * a;
                g.soft += (t.soft - g.soft) * a;
                g.mud += (t.mud - g.mud) * a;
                g.snow += (t.snow - g.snow) * a;
                g.wet += (t.wet - g.wet) * a;
            }
            let v = self.speed;
            let g = self.ground;
            if v < 0.05 && self.grains.is_empty() && self.bubbles.is_empty() {
                *o = 0.0;
                continue;
            }
            let weight = (self.load / 400.0).sqrt().clamp(0.3, 3.0);
            let roll = (v / 25.0).min(2.0);
            // Rolling: tread noise on hard ground, a dull rumble on soft.
            let white = self.rng.white();
            let road = self.road.process(white) * g.hard * roll.powf(1.3) * 0.25;
            let rumble = self.rumble.process(self.rng.white()) * (g.soft + g.mud) * roll * 0.12;
            let low = self.road2.process(white) * g.hard * roll * 0.04;
            // Gravel crunches and pings, snow packs and creaks: grains at a rate set by speed.
            let rate = (v * 40.0).min(900.0) / fs;
            if self.rng.chance(rate * g.gravel) {
                let amp = self.rng.range(0.02, 0.12) * weight;
                self.grain(amp, 1_200.0, 5_000.0, 4.0);
            }
            if self.rng.chance(rate * 0.5 * g.snow) {
                let amp = self.rng.range(0.02, 0.08) * weight;
                self.grain(amp, 500.0, 1_800.0, 9.0);
            }
            // Mud: squelch and sucking bubbles.
            if self.rng.chance((v * 6.0).min(60.0) / fs * g.mud) && self.bubbles.len() < 24 {
                let r = self.rng.range(0.004, 0.015);
                self.bubbles
                    .push(Bubble::new(fs, r, 0.08 * weight, 0.6, 0.0).damped(3.0, fs));
            }
            let mut grains = 0.0;
            for (f, amp, decay) in &mut self.grains {
                grains += f.process(self.rng.white()) * *amp;
                *amp *= *decay;
            }
            self.grains.retain(|g| g.1 > 1e-4);
            let mut bubbles = 0.0;
            for b in &mut self.bubbles {
                bubbles += b.process();
            }
            if i % 256 == 0 {
                self.bubbles.retain(|b| !b.done());
            }
            // Wet: the hiss of water squeezed out of the tread, and spray.
            let spray = self.spray.process(self.rng.white()) * g.wet * roll.powf(1.5) * 0.08;
            // Squeal: sliding on dry hard ground makes the rubber sing.
            let squeal_on = smooth(0.12, 0.45, self.slip) * (g.hard - g.wet * 0.7).max(0.0);
            let target = squeal_on * (v / 8.0).min(1.0) * weight;
            self.squeal_level += (target - self.squeal_level) * 0.002;
            let mut squeal = 0.0;
            if self.squeal_level > 1e-4 {
                let wobble = self.squeal_wobble.process(self.rng.white()) * 400.0;
                let f = 1_100.0 + 300.0 * self.slip + wobble;
                self.squeal_phase = (self.squeal_phase + f / fs).fract();
                let s = (self.squeal_phase * std::f32::consts::TAU).sin();
                squeal = (s + 0.3 * s * s * s) * self.squeal_level * 0.09;
            }
            // Sliding on loose ground throws it about instead.
            let scrabble = self.slip * (g.gravel + g.soft + g.snow) * roll;
            if self.rng.chance(scrabble * 300.0 / fs) {
                let amp = self.rng.range(0.03, 0.1);
                self.grain(amp, 800.0, 3_500.0, 6.0);
            }
            *o = road + low + rumble + grains + bubbles + spray + squeal;
        }
        out.len()
    }
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn render(sound: &mut dyn Sound, secs: f32) -> Vec<f32> {
        let mut buf = vec![0.0; (secs * FS) as usize];
        let n = sound.fill(&mut buf);
        buf.truncate(n);
        buf
    }

    fn rms(x: &[f32]) -> f32 {
        let x = &x[x.len() / 3..];
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// The strongest frequency between `lo` and `hi` Hz (a plain DFT scan, 1 Hz steps).
    fn dominant(x: &[f32], lo: f32, hi: f32) -> f32 {
        let x = &x[x.len() / 2..];
        let mut best = (0.0, 0.0);
        let mut f = lo;
        while f <= hi {
            let w = std::f32::consts::TAU * f / FS;
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (n, v) in x.iter().enumerate().step_by(2) {
                re += v * (w * n as f32).cos();
                im += v * (w * n as f32).sin();
            }
            let m = re * re + im * im;
            if m > best.1 {
                best = (f, m);
            }
            f += 1.0;
        }
        best.0
    }

    fn v8(rpm: f32, throttle: f32, load: f32) -> EngineSound {
        let mut e = EngineSound::new(
            FS,
            EngineVoice::Piston {
                cylinders: 8,
                litres: 5.0,
            },
            3,
        );
        e.set_param("rpm", rpm);
        e.value[RPM] = rpm;
        e.set_param("throttle", throttle);
        e.set_param("load", load);
        e
    }

    #[test]
    fn a_v8_fires_four_times_a_turn_and_gets_louder_with_load() {
        // 1 800 rpm = 30 turns/s; a V8 fires 4 times per turn = 120 Hz.
        let x = render(&mut v8(1_800.0, 0.3, 0.3), 1.0);
        let f = dominant(&x, 60.0, 200.0);
        assert!((f - 120.0).abs() < 6.0, "firing at {f} Hz");
        let idle = rms(&render(&mut v8(1_800.0, 0.0, 0.05), 1.0));
        let pulling = rms(&render(&mut v8(1_800.0, 1.0, 1.0), 1.0));
        assert!(pulling > idle * 1.5, "idle {idle} pulling {pulling}");
        let screaming = rms(&render(&mut v8(6_500.0, 1.0, 1.0), 1.0));
        assert!(screaming > pulling, "{pulling} vs {screaming}");
        assert!(screaming < 1.0);
    }

    #[test]
    fn every_engine_makes_sound_and_stops_when_off() {
        for voice in [
            EngineVoice::Turbine,
            EngineVoice::Rocket,
            EngineVoice::Electric,
            EngineVoice::Thrusters,
            EngineVoice::Piston {
                cylinders: 4,
                litres: 1.6,
            },
        ] {
            let mut e = EngineSound::new(FS, voice, 1);
            e.set_param("rpm", 9_000.0);
            e.set_param("thrust", 80_000.0);
            e.set_param("throttle", 1.0);
            e.set_param("load", 1.0);
            let on = rms(&render(&mut e, 1.0));
            assert!(on > 0.003 && on < 1.0, "{voice:?}: {on}");
            e.set_param("running", 0.0);
            render(&mut e, 2.0);
            let off = rms(&render(&mut e, 1.0));
            assert!(off < on * 0.05, "{voice:?} still going: {off}");
        }
    }

    #[test]
    fn tyres_squeal_sliding_on_tarmac_but_not_on_gravel() {
        let tyre = |hard: f32, gravel: f32, slip: f32| {
            let mut t = TyreSound::new(FS, 2);
            t.set_param("speed", 15.0);
            t.set_param("slip", slip);
            t.set_param("hard", hard);
            t.set_param("gravel", gravel);
            render(&mut t, 2.0)
        };
        let rolling = tyre(1.0, 0.0, 0.0);
        let sliding = tyre(1.0, 0.0, 0.8);
        let squeal = dominant(&sliding, 900.0, 1_800.0);
        assert!(rms(&sliding) > rms(&rolling) * 2.0, "no squeal");
        assert!(
            (1_000.0..1_700.0).contains(&squeal),
            "squeal at {squeal} Hz"
        );
        let gravel = tyre(0.0, 1.0, 0.0);
        let gravel_slide = tyre(0.0, 1.0, 0.8);
        assert!(rms(&gravel) > 0.003, "gravel is silent");
        // Sliding on gravel is louder (stones thrown about) but has no tonal squeal.
        assert!(rms(&gravel_slide) > rms(&gravel));
        let stopped = {
            let mut t = TyreSound::new(FS, 2);
            rms(&render(&mut t, 1.0))
        };
        assert!(stopped < 1e-4);
    }
}
