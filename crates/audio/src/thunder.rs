//! Thunder from a lightning channel. Every stretch of the bolt sends out its own shock wave,
//! which reaches the listener `distance / speed of sound` later, so the part of the channel
//! nearest to you cracks first and the rest rolls in over seconds as sound arrives from
//! farther up and along it. Stretches broadside to you are loudest (a line source radiates
//! sideways), which is what makes thunder rumble in bursts.
//!
//! The channel is split into a few parts, each played as one positioned voice at its middle,
//! so the mixer pans, muffles and dulls each part by its own direction and distance.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::LightningStrike;

use crate::dsp::{Biquad, Noise, OnePole};
use crate::{Audio, AudioCue, AudioSettings, PlayOptions, Sound};

/// Points along the channel thunder is built from (`LightningStrike::path` detail: 257).
const PATH_DETAIL: u32 = 8;

/// Lightning the weather reported recently: once strikes arrive, storms stop making up their
/// own thunder, and the blast the weather sends with a ground strike isn't also played as an
/// explosion.
#[derive(Resource, Debug, Default)]
pub(crate) struct Strikes {
    /// When the last strike arrived (real seconds).
    pub last: Option<f64>,
    /// Recent ground strike points and when they arrived.
    pub ground: Vec<(DVec3, f64)>,
}

impl Strikes {
    /// The weather has been sending strikes lately, so made-up thunder should stay quiet.
    pub fn weather_has_lightning(&self, now: f64) -> bool {
        self.last.is_some_and(|t| now - t < 180.0)
    }

    /// A blast at `p` is a ground strike's (already heard as thunder).
    pub fn is_strike(&self, p: DVec3) -> bool {
        self.ground.iter().any(|(g, _)| g.distance(p) < 30.0)
    }
}

/// Thunder for every lightning strike, rolling along its channel.
pub(crate) fn play_lightning(
    mut audio: ResMut<Audio>,
    settings: Res<AudioSettings>,
    time: Res<Time>,
    mut strikes: MessageReader<LightningStrike>,
    mut seen: ResMut<Strikes>,
    mut cues: MessageWriter<AudioCue>,
) {
    let now = time.real_elapsed_secs_f64();
    seen.ground.retain(|(_, t)| now - t < 1.0);
    for s in strikes.read() {
        seen.last = Some(now);
        if s.cloud_to_ground {
            seen.ground.push((s.to, now));
        }
        if !audio.is_running() || !settings.weather_ambience {
            continue;
        }
        let channel = s.path(PATH_DETAIL);
        if let Some(cue) = play_thunder(
            &mut audio,
            &channel,
            s.peak_current_ka,
            settings.speed_of_sound,
            s.seed,
        ) {
            cues.write(cue);
        }
    }
}

/// Parts the channel is split into (one voice each).
pub const THUNDER_PARTS: usize = 4;
/// Distance at which a part plays at its own level; the mixer scales it by `REF_M / d`.
pub const THUNDER_REF_M: f32 = 100.0;
/// How long the rumble lingers after the last shock wave arrives, seconds (echoes from the
/// ground, hills and cloud base).
const TAIL_S: f32 = 3.5;

/// One part of a thunderclap, ready to play at `position` after `delay` seconds.
pub struct ThunderPart {
    pub position: DVec3,
    /// Seconds from the strike until this part's first shock wave reaches the listener.
    pub delay: f32,
    pub sound: ThunderChannel,
}

/// Split the lightning `channel` (points from cloud to ground) into thunder parts as heard
/// from `listener`. `peak_current_ka` scales how loud it is (30 kA is a typical stroke).
pub fn thunder_parts(
    fs: f32,
    channel: &[DVec3],
    listener: DVec3,
    speed_of_sound: f32,
    peak_current_ka: f32,
    seed: u64,
) -> Vec<ThunderPart> {
    if channel.len() < 2 {
        return Vec::new();
    }
    let c = speed_of_sound.max(1.0) as f64;
    let strength = (peak_current_ka.max(1.0) / 30.0).sqrt();
    let segments = channel.len() - 1;
    let per_part = segments.div_ceil(THUNDER_PARTS);
    let mut parts = Vec::new();
    let mut rng = Noise::new(seed);
    for (k, chunk) in channel
        .windows(2)
        .collect::<Vec<_>>()
        .chunks(per_part)
        .enumerate()
    {
        // The part sits at its length-weighted middle.
        let mut mid = DVec3::ZERO;
        let mut total = 0.0;
        for s in chunk {
            let len = s[0].distance(s[1]);
            mid += (s[0] + s[1]) * 0.5 * len;
            total += len;
        }
        if total <= 0.0 {
            continue;
        }
        mid /= total;
        let d_mid = mid.distance(listener).max(1.0);
        let mut pulses = Vec::with_capacity(chunk.len());
        for s in chunk {
            let a = (s[0] + s[1]) * 0.5;
            let seg = s[1] - s[0];
            let len = seg.length();
            if len <= 0.0 {
                continue;
            }
            let to = a - listener;
            let d = to.length().max(1.0);
            // Broadside stretches are loud, end-on ones quiet.
            let along = (seg / len).dot(to / d).abs();
            let broadside = (1.0 - along * along).max(0.0) as f32;
            let directivity = 0.12 + 0.88 * broadside * broadside;
            // Level relative to the part's middle: the mixer applies REF_M / d_mid.
            let amp = strength * 0.075 * (len as f32).sqrt() * directivity * (d_mid / d) as f32;
            // Shock waves stretch as they travel: close ones are sharp, far ones a thud, and
            // only close ones keep the tearing crackle of the bolt's finer zigzags.
            let width = (0.004 + 0.012 * (d as f32 / 1_000.0).sqrt()).min(0.04);
            let crack = 0.3 + 3.0 * (1.0 - d as f32 / 2_000.0).max(0.0).powi(2);
            pulses.push((d / c, amp, width, crack));
        }
        if pulses.is_empty() {
            continue;
        }
        let first = pulses.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        let sound = ThunderChannel::new(
            fs,
            pulses
                .iter()
                .map(|&(t, amp, width, crack)| ((t - first) as f32, amp, width, crack))
                .collect(),
            rng.next_u32() as u64 ^ (k as u64) << 32,
        );
        parts.push(ThunderPart {
            position: mid,
            delay: first as f32,
            sound,
        });
    }
    parts
}

/// Play the thunder of a lightning `channel` (points from cloud to its end) as heard from the
/// listener. Returns the cue for the part heard first (`None` if nothing could play).
pub(crate) fn play_thunder(
    audio: &mut Audio,
    channel: &[DVec3],
    peak_current_ka: f32,
    speed_of_sound: f32,
    seed: u64,
) -> Option<AudioCue> {
    let fs = audio.sample_rate();
    let listener = audio.listener_position();
    let parts = thunder_parts(fs, channel, listener, speed_of_sound, peak_current_ka, seed);
    let mut first: Option<(f32, AudioCue)> = None;
    for part in parts {
        let (position, delay) = (part.position, part.delay);
        let d = position.distance(listener) as f32;
        let id = audio.play(
            part.sound,
            PlayOptions::at(position)
                .range(THUNDER_REF_M, 60_000.0)
                .no_propagation_delay()
                .delay(delay)
                .reverb(0.3),
        );
        if id.is_none() || first.as_ref().is_some_and(|f| f.0 <= delay) {
            continue;
        }
        let loudness = (peak_current_ka / 30.0).sqrt() * THUNDER_REF_M / d.max(THUNDER_REF_M);
        first = Some((
            delay,
            AudioCue {
                voice: id,
                position: Some(position),
                name: "thunder".into(),
                loudness: loudness.min(1.5),
            },
        ));
    }
    first.map(|f| f.1)
}

/// Where lightning strikes when the weather doesn't say: somewhere around `listener` (up is
/// `up`), between `min_m` and `max_m` away. Two in five stay in the clouds. Returns the
/// channel and its peak current, kA.
pub fn random_strike(
    listener: DVec3,
    ground_height: f64,
    up: Vec3,
    min_m: f32,
    max_m: f32,
    seed: u64,
) -> (Vec<DVec3>, f32) {
    let mut rng = Noise::new(seed);
    let up = up.as_dvec3().normalize_or(DVec3::Y);
    let (a, b) = up.any_orthonormal_pair();
    let angle = rng.range(0.0, std::f32::consts::TAU) as f64;
    let dir = a * angle.cos() + b * angle.sin();
    let distance = (min_m + (max_m - min_m) * rng.uniform().powf(0.7)) as f64;
    let ground = listener + dir * distance;
    let ground = ground + up * (ground_height - listener.dot(up));
    let cloud = rng.range(1_500.0, 3_200.0) as f64;
    let current = 8.0 + 60.0 * rng.uniform().powi(3);
    let seed = rng.next_u32() as u64;
    if rng.chance(0.4) {
        // In the cloud: a long branch running sideways at cloud height.
        let across = a * -angle.sin() + b * angle.cos();
        let half = rng.range(1_000.0, 4_000.0) as f64;
        let mid = ground + up * cloud;
        let channel = random_bolt(mid - across * half, mid + across * half, seed);
        (channel, current * 0.5)
    } else {
        let lean = (a * rng.range(-1.0, 1.0) as f64 + b * rng.range(-1.0, 1.0) as f64) * 600.0;
        (
            random_bolt(ground + up * cloud + lean, ground, seed),
            current,
        )
    }
}

/// A made-up lightning channel for thunder when the weather doesn't say where lightning
/// struck: `to` is where it ends (the ground for cloud-to-ground), `from` where it leaves the
/// cloud. 257 points, zigzagging by midpoint displacement like a real bolt.
pub fn random_bolt(from: DVec3, to: DVec3, seed: u64) -> Vec<DVec3> {
    const N: usize = 256;
    let mut rng = Noise::new(seed);
    let mut points = vec![from; N + 1];
    points[N] = to;
    let axis = (to - from).normalize_or_zero();
    let side = axis.any_orthonormal_pair();
    let mut step = N;
    while step > 1 {
        let half = step / 2;
        for i in (half..N).step_by(step) {
            let (a, b) = (points[i - half], points[i + half]);
            let reach = a.distance(b) * 0.14;
            points[i] = (a + b) * 0.5
                + (side.0 * rng.range(-1.0, 1.0) as f64 + side.1 * rng.range(-1.0, 1.0) as f64)
                    * reach;
        }
        step = half;
    }
    points
}

/// The shock waves from one part of a lightning channel: a sharp N-shaped pressure pulse per
/// stretch with a burst of crackle from its finer zigzags, over a rumble that follows them
/// and lingers. Mono; ends by itself.
pub struct ThunderChannel {
    fs: f32,
    rng: Noise,
    /// (start sample, length in samples, amplitude, crackle), by start.
    pulses: Vec<(u32, u32, f32, f32)>,
    next: usize,
    active: Vec<(u32, u32, f32)>,
    crackle: f32,
    crackle_decay: f32,
    crackle_hp: Biquad,
    env: f32,
    env_release: f32,
    brown: f32,
    rumble: [Biquad; 2],
    shock: OnePole,
    /// Keeps out sub-audible pressure swings that would only eat headroom.
    dc: Biquad,
    t: u32,
    end: u32,
}

impl ThunderChannel {
    /// `pulses`: (seconds from the start, amplitude, width in seconds, how much it crackles)
    /// for each stretch.
    pub fn new(fs: f32, pulses: Vec<(f32, f32, f32, f32)>, seed: u64) -> Self {
        let mut list: Vec<(u32, u32, f32, f32)> = pulses
            .iter()
            .map(|&(t, amp, w, crack)| ((t * fs) as u32, (w * fs).max(2.0) as u32, amp, crack))
            .collect();
        list.sort_by_key(|p| p.0);
        let last = list.last().map(|p| p.0 + p.1).unwrap_or(0);
        Self {
            fs,
            rng: Noise::new(seed),
            pulses: list,
            next: 0,
            active: Vec::new(),
            crackle: 0.0,
            crackle_decay: (-1.0 / (0.03 * fs)).exp(),
            crackle_hp: Biquad::highpass(fs, 900.0, 0.7),
            env: 0.0,
            env_release: (-1.0 / (1.1 * fs)).exp(),
            brown: 0.0,
            rumble: [
                Biquad::highpass(fs, 38.0, 0.7),
                Biquad::lowpass(fs, 140.0, 0.7),
            ],
            shock: OnePole::new(fs, 2_500.0),
            dc: Biquad::highpass(fs, 28.0, 0.7),
            t: 0,
            end: last + (TAIL_S * fs) as u32,
        }
    }

    /// Length in seconds.
    pub fn duration(&self) -> f32 {
        self.end as f32 / self.fs
    }
}

impl Sound for ThunderChannel {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        for (i, o) in out.iter_mut().enumerate() {
            if self.t >= self.end {
                return i;
            }
            while self.next < self.pulses.len() && self.pulses[self.next].0 <= self.t {
                let p = self.pulses[self.next];
                self.active.push((p.0, p.1, p.2));
                // Each stretch's finer zigzags crackle as their tiny shocks arrive.
                self.crackle += p.2 * p.3;
                self.env = self.env.max(p.2) + p.2 * 0.15;
                self.next += 1;
            }
            // N-waves: a jump up, a straight fall through zero, a jump back.
            let mut shock = 0.0;
            let t = self.t;
            self.active.retain(|&(start, len, amp)| {
                let x = (t - start) as f32 / len as f32;
                if x >= 1.0 {
                    return false;
                }
                shock += amp * (1.0 - 2.0 * x);
                true
            });
            let shock = self.shock.process(shock);
            let crackle = self.crackle_hp.process(self.rng.white()) * self.crackle;
            self.crackle *= self.crackle_decay;
            // Rumble: deep noise that swells with the shocks and dies away slowly.
            self.brown = (self.brown + self.rng.white() * 0.06) * 0.99;
            let low = self.rumble[0].process(self.brown);
            let rumble = self.rumble[1].process(low) * 9.0;
            self.env *= self.env_release;
            let tail = ((self.end - self.t) as f32 / (0.4 * self.fs)).min(1.0);
            *o = self.dc.process(shock + crackle + rumble * self.env) * tail;
            self.t += 1;
        }
        out.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    /// A cloud-to-ground bolt from 3 km up straight down onto `ground`, zigzagging a little.
    fn bolt(ground: DVec3) -> Vec<DVec3> {
        let mut rng = Noise::new(9);
        (0..=256)
            .map(|i| {
                let k = i as f64 / 256.0;
                let wiggle = if i == 0 || i == 256 { 0.0 } else { 40.0 };
                ground
                    + DVec3::new(
                        rng.range(-1.0, 1.0) as f64 * wiggle,
                        3_000.0 * (1.0 - k),
                        rng.range(-1.0, 1.0) as f64 * wiggle,
                    )
            })
            .collect()
    }

    /// What the listener hears: every part delayed and scaled by distance like the mixer.
    fn hear(parts: Vec<ThunderPart>, listener: DVec3) -> (Vec<f32>, f32) {
        let first = parts.iter().map(|p| p.delay).fold(f32::MAX, f32::min);
        let mut mix = vec![0.0f32; (FS * 30.0) as usize];
        for mut p in parts {
            let g = THUNDER_REF_M / (p.position.distance(listener) as f32).max(THUNDER_REF_M);
            let mut buf = vec![0.0; (FS * 25.0) as usize];
            let n = p.sound.fill(&mut buf);
            let start = ((p.delay - first) * FS) as usize;
            for (i, v) in buf[..n].iter().enumerate() {
                if let Some(m) = mix.get_mut(start + i) {
                    *m += v * g;
                }
            }
        }
        (mix, first)
    }

    #[test]
    fn thunder_arrives_late_cracks_first_and_rolls() {
        let listener = DVec3::new(0.0, 1.7, 0.0);
        let parts = thunder_parts(
            FS,
            &bolt(DVec3::new(1_000.0, 0.0, 0.0)),
            listener,
            343.0,
            30.0,
            1,
        );
        assert_eq!(parts.len(), THUNDER_PARTS);
        let (x, first) = hear(parts, listener);
        // The foot of the bolt is 1 km away: about 2.9 s.
        assert!(
            (first - 1_000.0 / 343.0).abs() < 0.2,
            "first arrival {first}"
        );
        let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.15..2.0).contains(&peak), "peak {peak}");
        // It rolls: still sounding several seconds after the first crack.
        let rms = |a: f32, b: f32| {
            let s = &x[(a * FS) as usize..(b * FS) as usize];
            (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
        };
        let start = rms(0.0, 0.5);
        let later = rms(4.0, 5.0);
        assert!(later > start * 0.03, "no roll: {start} then {later}");
        assert!(rms(20.0, 21.0) < start * 0.01, "never ends");
    }

    #[test]
    fn far_thunder_is_quieter_and_arrives_later() {
        let listener = DVec3::new(0.0, 1.7, 0.0);
        let level = |km: f64| {
            let parts = thunder_parts(
                FS,
                &bolt(DVec3::new(km * 1_000.0, 0.0, 0.0)),
                listener,
                343.0,
                30.0,
                2,
            );
            let (x, first) = hear(parts, listener);
            let e = x.iter().map(|v| v * v).sum::<f32>();
            (e.sqrt(), first)
        };
        let (near, t_near) = level(0.5);
        let (far, t_far) = level(6.0);
        assert!(far < near * 0.4, "near {near} far {far}");
        assert!(t_far > t_near + 10.0);
    }
}
