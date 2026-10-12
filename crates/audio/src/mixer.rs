//! The mixer: voices, spatialization, time dilation, buses, reverb and the master limiter.
//!
//! It runs wherever samples are needed: inside the sound card callback (device output) or
//! on the game thread one frame at a time (offline output, used headless and for recordings).
//! The game talks to it only through [`Command`]s and hears back through [`MixerEvent`]s, so
//! the audio thread never waits on the game and never allocates per sample.

use crossbeam_channel::{Receiver, Sender};
use engine_core::math::{DQuat, DVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::dsp::{Biquad, Freeverb, Limiter, OnePole, db, hermite, pan_gains};

/// Frames rendered per inner block. Parameters (gains, pitch, filters) are recomputed once
/// per block and interpolated across it.
pub const BLOCK: usize = 64;

/// Something that makes sound: a procedural patch, a decoded sample, a test tone.
///
/// Sounds render at the mixer's sample rate (given to their constructor). The mixer resamples
/// them for pitch, doppler and slow motion, and pans them, so a sound only produces its raw
/// signal.
pub trait Sound: Send {
    /// 1 (mono, can be positioned in 3D) or 2 (stereo bed, played as is).
    fn channels(&self) -> usize {
        1
    }

    /// Write up to `out.len() / channels` frames (interleaved for stereo) and return how many
    /// were written. Returning fewer than asked ends the sound.
    fn fill(&mut self, out: &mut [f32]) -> usize;

    /// Change a live parameter (`"intensity"`, `"speed"`, …). Unknown names are ignored.
    fn set_param(&mut self, _name: &str, _value: f32) {}
}

/// Identifies a playing voice. `VoiceId::NONE` is returned for sounds that were culled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct VoiceId(pub u32);

impl VoiceId {
    pub const NONE: VoiceId = VoiceId(0);

    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// Mixer buses. World and Ambience are heard "in the world": they slow down with time
/// dilation, go quiet in thin air and in space, and get muffled by snow. Music, Ui and Voice
/// (dialogue) are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Bus {
    /// Footsteps, impacts, weapons, explosions: everything that happens somewhere.
    #[default]
    World,
    /// Rain, wind, thunder, room tone.
    Ambience,
    Music,
    Ui,
    /// Dialogue and voice-over.
    Voice,
}

impl Bus {
    pub const ALL: [Bus; 5] = [Bus::World, Bus::Ambience, Bus::Music, Bus::Ui, Bus::Voice];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Bus::World => "world",
            Bus::Ambience => "ambience",
            Bus::Music => "music",
            Bus::Ui => "ui",
            Bus::Voice => "voice",
        }
    }

    pub(crate) fn in_world(self) -> bool {
        matches!(self, Bus::World | Bus::Ambience)
    }
}

const BUSES: usize = Bus::ALL.len();

/// Which clock a voice plays on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Clock {
    /// World time × factor: slows and lowers in pitch with time dilation (enemies, rain).
    World(f32),
    /// Real time × factor: ignores dilation (UI, music, a player who perceives at full speed).
    Real(f32),
}

impl Default for Clock {
    fn default() -> Self {
        Clock::World(1.0)
    }
}

/// Where a voice is and how it fades with distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spatial {
    /// World position, meters.
    pub position: DVec3,
    /// m/s, for doppler.
    pub velocity: Vec3,
    /// Full volume within this distance, meters.
    pub ref_distance: f32,
    /// Silent beyond this distance, meters.
    pub max_distance: f32,
    /// How fast the level falls past `ref_distance` (1 = inverse distance, like real air).
    pub rolloff: f32,
    /// Delay the start by distance / speed of sound (thunder, distant explosions).
    pub delay: bool,
}

impl Spatial {
    pub fn at(position: DVec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            ref_distance: 1.0,
            max_distance: 100.0,
            rolloff: 1.0,
            delay: true,
        }
    }
}

/// Start a voice.
pub struct Play {
    pub id: VoiceId,
    pub sound: Box<dyn Sound>,
    pub bus: Bus,
    /// Linear gain.
    pub gain: f32,
    /// Playback rate multiplier (2 = an octave up, twice as fast).
    pub pitch: f32,
    pub clock: Clock,
    /// `None` = not positioned (centered mono or stereo as is).
    pub spatial: Option<Spatial>,
    /// How much goes to the room reverb, 0..1.
    pub reverb_send: f32,
    /// Mixer sample clock to start at (offline output); `None` = as soon as possible.
    pub start_at: Option<u64>,
    /// Extra delay before starting, seconds.
    pub delay_secs: f32,
    /// Seconds to fade in.
    pub fade_in: f32,
}

/// Mixer-wide settings sent from [`crate::AudioSettings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixerSettings {
    pub speed_of_sound: f32,
    /// 0 = no doppler, 1 = physical.
    pub doppler: f32,
    /// Air absorption of highs per meter (0.006 ≈ air at 20 °C, 50 % humidity).
    pub air_absorption: f32,
    pub master_gain: f32,
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self {
            speed_of_sound: 343.0,
            doppler: 1.0,
            air_absorption: 0.006,
            master_gain: 1.0,
        }
    }
}

/// Messages from the game to the mixer.
pub enum Command {
    Play(Box<Play>),
    Stop {
        id: VoiceId,
        fade: f32,
    },
    StopAll {
        fade: f32,
    },
    SetGain {
        id: VoiceId,
        gain: f32,
    },
    SetPitch {
        id: VoiceId,
        pitch: f32,
    },
    SetParam {
        id: VoiceId,
        name: &'static str,
        value: f32,
    },
    SetEmitter {
        id: VoiceId,
        position: DVec3,
        velocity: Vec3,
    },
    /// How much is between the voice and the listener: 0 clear line of sight .. 1 behind a
    /// thick wall. Muffles and quietens; the change is smoothed over about 0.15 s.
    SetOcclusion {
        id: VoiceId,
        amount: f32,
    },
    /// The world moved to a new frame (floating origin, entering a planet's frame): move every
    /// voice and the ears with it, so nothing jumps.
    ShiftWorld {
        rotation: DQuat,
        translation: DVec3,
    },
    /// Ears: position, orientation (unit right, up, forward) and velocity.
    SetListener {
        position: DVec3,
        right: Vec3,
        up: Vec3,
        forward: Vec3,
        velocity: Vec3,
    },
    /// Rate of the world clock (time dilation mapped to pitch) and how far to darken the
    /// world for slow motion (0 = off, 1 = deep).
    SetWorldRate {
        rate: f32,
        slow_motion: f32,
    },
    /// Linear gain and low-pass cutoff (Hz) for a bus.
    SetBus {
        bus: Bus,
        gain: f32,
        lowpass_hz: f32,
    },
    /// Room reverb: size and damping 0..1, wet level (linear).
    SetReverb {
        room: f32,
        damping: f32,
        wet: f32,
    },
    /// Air around the listener: density 0 (space) .. 1 (sea level), how fast sound travels in
    /// it relative to sea level (warm air and other planets' air differ), and a
    /// high-frequency cut in dB for falling snow (which swallows highs).
    SetMedium {
        density: f32,
        speed_scale: f32,
        hush_db: f32,
    },
    Settings(MixerSettings),
    /// Send every rendered block (interleaved stereo) here; `None` stops.
    Capture(Option<Sender<Vec<f32>>>),
}

/// Level meters, linear peak and RMS per bus (in [`Bus::ALL`] order) and for the master.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Meters {
    pub bus_peak: [f32; BUSES],
    pub bus_rms: [f32; BUSES],
    pub master_peak: f32,
    pub master_rms: f32,
    /// Lowest limiter gain since the last report (1 = never limited).
    pub limiter_gain: f32,
    pub voices: u32,
}

/// Messages from the mixer back to the game.
#[derive(Debug, Clone)]
pub enum MixerEvent {
    Finished(VoiceId),
    Meters(Meters),
}

// ---------------------------------------------------------------- voices

struct Voice {
    id: VoiceId,
    sound: Box<dyn Sound>,
    channels: usize,
    bus: Bus,
    gain: f32,
    pitch: f32,
    clock: Clock,
    spatial: Option<Spatial>,
    reverb_send: f32,
    delay: u64,
    occlusion: f32,
    occlusion_target: f32,
    // Fades: current level, per-sample step, target.
    fade: f32,
    fade_step: f32,
    fade_target: f32,
    stopping: bool,
    // Smoothed per-block state.
    cur_gain: [f32; 2],
    cur_send: f32,
    cur_rate: f32,
    lp: [OnePole; 2],
    lp_hz: f32,
    // Resampler: four frames of history per channel and the fractional read position.
    hist: [[f32; 4]; 2],
    frac: f32,
    src: Vec<f32>,
    src_len: usize,
    src_pos: usize,
    src_done: bool,
    tail: u8,
    finished: bool,
}

impl Voice {
    fn new(play: Play, fs: f32, delay: u64) -> Self {
        let channels = play.sound.channels().clamp(1, 2);
        let fade_in = play.fade_in.max(0.0);
        let (fade, fade_step) = if fade_in > 0.0 {
            (0.0, 1.0 / (fade_in * fs))
        } else {
            (1.0, 0.0)
        };
        Self {
            id: play.id,
            sound: play.sound,
            channels,
            bus: play.bus,
            gain: play.gain,
            pitch: play.pitch,
            clock: play.clock,
            spatial: play.spatial,
            reverb_send: play.reverb_send,
            delay,
            occlusion: 0.0,
            occlusion_target: 0.0,
            fade,
            fade_step,
            fade_target: 1.0,
            stopping: false,
            cur_gain: [f32::NAN; 2],
            cur_send: 0.0,
            cur_rate: f32::NAN,
            lp: [OnePole::new(fs, 20_000.0); 2],
            lp_hz: 20_000.0,
            hist: [[0.0; 4]; 2],
            frac: 0.0,
            src: vec![0.0; BLOCK * channels],
            src_len: 0,
            src_pos: 0,
            src_done: false,
            tail: 0,
            finished: false,
        }
    }

    /// Next source frame into the history (zeros once the sound has ended).
    #[inline]
    fn advance_source(&mut self) {
        let ch = self.channels;
        let mut frame = [0.0f32; 2];
        if self.src_pos >= self.src_len && !self.src_done {
            let cap = self.src.len() / ch;
            let n = self.sound.fill(&mut self.src[..cap * ch]).min(cap);
            self.src_len = n;
            self.src_pos = 0;
            if n < cap {
                self.src_done = true;
            }
        }
        if self.src_pos < self.src_len {
            let i = self.src_pos * ch;
            frame[0] = self.src[i];
            frame[1] = if ch == 2 { self.src[i + 1] } else { frame[0] };
            self.src_pos += 1;
        } else {
            self.tail = self.tail.saturating_add(1);
        }
        for (c, h) in self.hist.iter_mut().enumerate() {
            h.copy_within(1.., 0);
            h[3] = frame[c];
        }
    }

    fn ended(&self) -> bool {
        self.src_done && self.src_pos >= self.src_len && self.tail >= 4
    }
}

struct BusState {
    gain: f32,
    target_gain: f32,
    lowpass_hz: f32,
    filters: [Biquad; 2],
    filter_hz: f32,
    shelf: [Biquad; 2],
    shelf_db: f32,
    buf: [f32; BLOCK * 2],
    peak: f32,
    sum_sq: f32,
}

impl BusState {
    fn new(fs: f32) -> Self {
        Self {
            gain: 1.0,
            target_gain: 1.0,
            lowpass_hz: 22_000.0,
            filters: [Biquad::lowpass(fs, 22_000.0, 0.707); 2],
            filter_hz: 22_000.0,
            shelf: [Biquad::default(); 2],
            shelf_db: 0.0,
            buf: [0.0; BLOCK * 2],
            peak: 0.0,
            sum_sq: 0.0,
        }
    }
}

struct Listener {
    position: DVec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    velocity: Vec3,
}

/// The mixer. Create one per output; feed it [`Command`]s; call [`Mixer::process`].
pub struct Mixer {
    fs: f32,
    commands: Receiver<Command>,
    events: Sender<MixerEvent>,
    voices: Vec<Voice>,
    buses: Vec<BusState>,
    reverb: Freeverb,
    reverb_wet: f32,
    reverb_target: (f32, f32, f32),
    send_buf: [f32; BLOCK * 2],
    limiter: Limiter,
    listener: Listener,
    world_rate: f32,
    slow_motion: f32,
    density: f32,
    /// Speed of sound relative to sea-level air.
    speed_scale: f32,
    hush_db: f32,
    settings: MixerSettings,
    capture: Option<Sender<Vec<f32>>>,
    clock: u64,
    meters: Meters,
    meter_frames: u32,
    master_sum_sq: f32,
    out_block: [f32; BLOCK * 2],
}

impl Mixer {
    pub fn new(fs: f32, commands: Receiver<Command>, events: Sender<MixerEvent>) -> Self {
        Self {
            fs,
            commands,
            events,
            voices: Vec::with_capacity(128),
            buses: (0..BUSES).map(|_| BusState::new(fs)).collect(),
            reverb: Freeverb::new(fs),
            reverb_wet: 0.15,
            reverb_target: (0.35, 0.5, 0.15),
            send_buf: [0.0; BLOCK * 2],
            limiter: Limiter::new(fs, db(-1.0)),
            listener: Listener {
                position: DVec3::ZERO,
                right: Vec3::X,
                up: Vec3::Y,
                forward: Vec3::NEG_Z,
                velocity: Vec3::ZERO,
            },
            world_rate: 1.0,
            slow_motion: 0.0,
            density: 1.0,
            speed_scale: 1.0,
            hush_db: 0.0,
            settings: MixerSettings::default(),
            capture: None,
            clock: 0,
            meters: Meters {
                limiter_gain: 1.0,
                ..Default::default()
            },
            meter_frames: 0,
            master_sum_sq: 0.0,
            out_block: [0.0; BLOCK * 2],
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    /// Frames rendered so far.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Apply every queued command.
    pub fn apply_commands(&mut self) {
        while let Ok(cmd) = self.commands.try_recv() {
            self.apply(cmd);
        }
    }

    fn voice_mut(&mut self, id: VoiceId) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == id)
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Play(play) => {
                let mut delay = play
                    .start_at
                    .map(|t| t.saturating_sub(self.clock))
                    .unwrap_or(0);
                delay += (play.delay_secs.max(0.0) * self.fs) as u64;
                if let Some(sp) = &play.spatial
                    && sp.delay
                {
                    let d = sp.position.distance(self.listener.position);
                    let c = self.settings.speed_of_sound * self.speed_scale;
                    delay += (d / c.max(1.0) as f64 * self.fs as f64) as u64;
                }
                self.voices.push(Voice::new(*play, self.fs, delay));
            }
            Command::Stop { id, fade } => {
                let fs = self.fs;
                if let Some(v) = self.voice_mut(id) {
                    stop_voice(v, fade, fs);
                }
            }
            Command::StopAll { fade } => {
                let fs = self.fs;
                for v in &mut self.voices {
                    stop_voice(v, fade, fs);
                }
            }
            Command::SetGain { id, gain } => {
                if let Some(v) = self.voice_mut(id) {
                    v.gain = gain;
                }
            }
            Command::SetPitch { id, pitch } => {
                if let Some(v) = self.voice_mut(id) {
                    v.pitch = pitch;
                }
            }
            Command::SetParam { id, name, value } => {
                if let Some(v) = self.voice_mut(id) {
                    v.sound.set_param(name, value);
                }
            }
            Command::SetEmitter {
                id,
                position,
                velocity,
            } => {
                if let Some(v) = self.voice_mut(id)
                    && let Some(sp) = &mut v.spatial
                {
                    sp.position = position;
                    sp.velocity = velocity;
                }
            }
            Command::ShiftWorld {
                rotation,
                translation,
            } => {
                let turn = |v: Vec3| (rotation * v.as_dvec3()).as_vec3();
                for v in &mut self.voices {
                    if let Some(sp) = &mut v.spatial {
                        sp.position = rotation * sp.position + translation;
                        sp.velocity = turn(sp.velocity);
                    }
                }
                let l = &mut self.listener;
                l.position = rotation * l.position + translation;
                l.right = turn(l.right);
                l.up = turn(l.up);
                l.forward = turn(l.forward);
                l.velocity = turn(l.velocity);
            }
            Command::SetOcclusion { id, amount } => {
                if let Some(v) = self.voice_mut(id) {
                    v.occlusion_target = amount.clamp(0.0, 1.0);
                    // Known before the first sample: start there instead of sweeping to it.
                    if v.cur_rate.is_nan() {
                        v.occlusion = v.occlusion_target;
                    }
                }
            }
            Command::SetListener {
                position,
                right,
                up,
                forward,
                velocity,
            } => {
                self.listener = Listener {
                    position,
                    right,
                    up,
                    forward,
                    velocity,
                };
            }
            Command::SetWorldRate { rate, slow_motion } => {
                self.world_rate = rate.max(0.0);
                self.slow_motion = slow_motion.clamp(0.0, 1.0);
            }
            Command::SetBus {
                bus,
                gain,
                lowpass_hz,
            } => {
                let b = &mut self.buses[bus.index()];
                b.target_gain = gain.max(0.0);
                b.lowpass_hz = lowpass_hz.clamp(20.0, 22_000.0);
            }
            Command::SetReverb { room, damping, wet } => {
                self.reverb_target = (room, damping, wet.max(0.0));
            }
            Command::SetMedium {
                density,
                speed_scale,
                hush_db,
            } => {
                self.density = density.clamp(0.0, 1.0);
                self.speed_scale = speed_scale.clamp(0.1, 10.0);
                self.hush_db = hush_db.min(0.0);
            }
            Command::Settings(s) => self.settings = s,
            Command::Capture(c) => self.capture = c,
        }
    }

    /// Render `frames` frames into `out` (interleaved, `channels` per frame: left, right,
    /// then silence; mono gets the average). Applies queued commands first.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        self.apply_commands();
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            self.render_block(n);
            for i in 0..n {
                let (l, r) = (self.out_block[i * 2], self.out_block[i * 2 + 1]);
                let o = &mut out[(done + i) * channels..(done + i + 1) * channels];
                if channels == 1 {
                    o[0] = 0.5 * (l + r);
                } else {
                    o[0] = l;
                    o[1] = r;
                    for x in &mut o[2..] {
                        *x = 0.0;
                    }
                }
            }
            if let Some(cap) = &self.capture
                && cap.send(self.out_block[..n * 2].to_vec()).is_err()
            {
                self.capture = None;
            }
            done += n;
        }
    }

    fn render_block(&mut self, n: usize) {
        let fs = self.fs;
        for b in &mut self.buses {
            b.buf[..n * 2].fill(0.0);
        }
        self.send_buf[..n * 2].fill(0.0);

        let lis = &self.listener;
        let settings = self.settings;
        let speed_scale = self.speed_scale;
        let world_rate = self.world_rate;
        let world_hush = self.density;
        for v in &mut self.voices {
            if v.finished {
                continue;
            }
            if v.delay >= n as u64 {
                v.delay -= n as u64;
                continue;
            }
            let start = v.delay as usize;
            v.delay = 0;

            // --- per-block targets
            let clock_rate = match v.clock {
                Clock::World(f) => world_rate * f,
                Clock::Real(f) => f,
            };
            let mut rate = v.pitch * clock_rate;
            let mut gains = [v.gain, v.gain];
            let mut lp_hz = 20_000.0f32;
            let mut send = v.reverb_send;
            if let Some(sp) = &v.spatial {
                let rel = (sp.position - lis.position).as_vec3();
                let dist = rel.length();
                let dir = if dist > 1e-3 { rel / dist } else { lis.forward };
                let r = sp.ref_distance.max(0.01);
                let mut g = r / (r + sp.rolloff.max(0.0) * (dist.max(r) - r));
                let max = sp.max_distance.max(r * 1.01);
                g *= 1.0 - smooth01((dist - 0.8 * max) / (0.2 * max));
                // Pan narrows when the sound is right on top of the listener.
                let pan = dir.dot(lis.right) * smooth01(dist / r);
                let behind = (-dir.dot(lis.forward)).clamp(0.0, 1.0);
                let (gl, gr) = pan_gains(pan);
                let g = g * (1.0 - 0.15 * behind);
                // Center = unity in each ear, like an unpositioned mono sound.
                let k = v.gain * g * std::f32::consts::SQRT_2;
                gains = [k * gl, k * gr];
                lp_hz = 20_000.0 / (1.0 + dist * settings.air_absorption);
                lp_hz *= 1.0 - 0.45 * behind;
                // The outer ear shades sounds from below a little (footsteps under you).
                lp_hz *= 1.0 - 0.2 * (-dir.dot(lis.up)).clamp(0.0, 1.0);
                // Distant sounds are mostly room.
                send *= 0.35 + 0.65 * smooth01(dist / (r * 12.0));
                if settings.doppler > 0.0 && dist > 1e-3 {
                    let c = (settings.speed_of_sound * speed_scale).max(1.0);
                    let v_src = (-dir).dot(sp.velocity).clamp(-0.9 * c, 0.9 * c);
                    let v_lis = dir.dot(lis.velocity).clamp(-0.9 * c, 0.9 * c);
                    let f = ((c + v_lis) / (c - v_src)).clamp(0.5, 2.0);
                    rate *= 1.0 + (f - 1.0) * settings.doppler;
                }
                // Blocked: the direct sound comes through or around the obstacle, quieter
                // and muffled; the room sound is affected less.
                let k = 1.0 - (-(n as f32) / (0.05 * fs)).exp();
                v.occlusion += (v.occlusion_target - v.occlusion) * k;
                let o = v.occlusion;
                if o > 1e-3 {
                    let g = db(-15.0 * o);
                    gains[0] *= g;
                    gains[1] *= g;
                    lp_hz *= (600.0 / 20_000.0f32).powf(o);
                    send *= 1.0 - 0.35 * o;
                }
            }
            if v.bus.in_world() {
                // Thin air carries less sound; in vacuum nothing.
                let k = world_hush.sqrt();
                gains[0] *= k;
                gains[1] *= k;
            }
            // Fade out a stalled clock (paused world) instead of holding a DC level.
            let stall = smooth01(rate / 0.02);
            gains[0] *= stall;
            gains[1] *= stall;
            if v.cur_rate.is_nan() {
                v.cur_rate = rate;
                v.cur_gain = gains;
                v.cur_send = send;
            }
            if (lp_hz - v.lp_hz).abs() > 1.0 {
                v.lp_hz = lp_hz;
                for f in &mut v.lp {
                    f.set(fs, lp_hz);
                }
            }

            // --- per-sample render with linear ramps across the block
            let span = (n - start) as f32;
            let dg = [
                (gains[0] - v.cur_gain[0]) / span,
                (gains[1] - v.cur_gain[1]) / span,
            ];
            let dr = (rate - v.cur_rate) / span;
            let ds = (send - v.cur_send) / span;
            let bus = &mut self.buses[v.bus.index()].buf;
            let sendb = &mut self.send_buf;
            for i in start..n {
                v.cur_rate += dr;
                v.cur_gain[0] += dg[0];
                v.cur_gain[1] += dg[1];
                v.cur_send += ds;
                v.frac += v.cur_rate.max(0.0);
                // Guard against absurd rates (e.g. a NaN pitch from a script).
                let mut steps = 0;
                while v.frac >= 1.0 && steps < 64 {
                    v.frac -= 1.0;
                    v.advance_source();
                    steps += 1;
                }
                if steps == 64 {
                    v.frac = 0.0;
                }
                if v.fade_step != 0.0 {
                    v.fade += v.fade_step;
                    if (v.fade_step > 0.0 && v.fade >= v.fade_target)
                        || (v.fade_step < 0.0 && v.fade <= v.fade_target)
                    {
                        v.fade = v.fade_target;
                        v.fade_step = 0.0;
                    }
                }
                let t = v.frac;
                let (l, r) = if v.channels == 1 {
                    let h = &v.hist[0];
                    let x = hermite(h[0], h[1], h[2], h[3], t);
                    let x = v.lp[0].process(x);
                    let x = v.lp[1].process(x);
                    (x * v.cur_gain[0], x * v.cur_gain[1])
                } else {
                    let (h0, h1) = (&v.hist[0], &v.hist[1]);
                    let xl = v.lp[0].process(hermite(h0[0], h0[1], h0[2], h0[3], t));
                    let xr = v.lp[1].process(hermite(h1[0], h1[1], h1[2], h1[3], t));
                    (xl * v.cur_gain[0], xr * v.cur_gain[1])
                };
                let (l, r) = (l * v.fade, r * v.fade);
                bus[i * 2] += l;
                bus[i * 2 + 1] += r;
                sendb[i * 2] += l * v.cur_send;
                sendb[i * 2 + 1] += r * v.cur_send;
            }
            v.cur_gain = gains;
            v.cur_rate = rate;
            v.cur_send = send;
            if v.ended() || (v.stopping && v.fade <= 0.0) {
                v.finished = true;
            }
        }

        // Retire finished voices.
        let events = &self.events;
        self.voices.retain(|v| {
            if v.finished {
                let _ = events.try_send(MixerEvent::Finished(v.id));
                false
            } else {
                true
            }
        });

        // --- buses
        let slow = self.slow_motion;
        let hush = self.hush_db;
        for (bi, b) in self.buses.iter_mut().enumerate() {
            let bus = Bus::ALL[bi];
            let mut lp = b.lowpass_hz;
            let mut shelf_db = 0.0;
            if bus.in_world() {
                // Slow motion: the world goes dark and underwater-ish (22 kHz → ~1.2 kHz).
                lp = lp.min(22_000.0 * (1.0 - slow) + 1_200.0 * slow);
                shelf_db = hush;
            }
            if (lp - b.filter_hz).abs() > b.filter_hz * 0.01 {
                b.filter_hz = lp;
                let f = Biquad::lowpass(fs, lp, 0.707);
                for x in &mut b.filters {
                    x.retune(f);
                }
            }
            if (shelf_db - b.shelf_db).abs() > 0.05 {
                b.shelf_db = shelf_db;
                let f = Biquad::high_shelf(fs, 2_500.0, shelf_db);
                for x in &mut b.shelf {
                    x.retune(f);
                }
            }
            let dg = (b.target_gain - b.gain) / n as f32;
            let filtered = b.filter_hz < 21_000.0;
            let shelved = b.shelf_db < -0.05;
            for i in 0..n {
                b.gain += dg;
                let mut l = b.buf[i * 2];
                let mut r = b.buf[i * 2 + 1];
                if filtered {
                    l = b.filters[0].process(l);
                    r = b.filters[1].process(r);
                }
                if shelved {
                    l = b.shelf[0].process(l);
                    r = b.shelf[1].process(r);
                }
                l *= b.gain;
                r *= b.gain;
                b.buf[i * 2] = l;
                b.buf[i * 2 + 1] = r;
                b.peak = b.peak.max(l.abs()).max(r.abs());
                b.sum_sq += 0.5 * (l * l + r * r);
            }
            b.gain = b.target_gain;
        }

        // --- reverb (wet smoothed toward target; slow motion makes it bigger and wetter)
        let (room, damping, wet) = self.reverb_target;
        self.reverb.room += (room.clamp(0.0, 1.0) + 0.15 * slow - self.reverb.room) * 0.05;
        self.reverb.damping += (damping.clamp(0.0, 1.0) - self.reverb.damping) * 0.05;
        let wet_target = wet * (1.0 + slow) * self.density.sqrt();
        let dw = (wet_target - self.reverb_wet) / n as f32;

        // --- master
        let master = self.settings.master_gain;
        let mut min_gain = 1.0f32;
        for i in 0..n {
            self.reverb_wet += dw;
            let (wl, wr) = self
                .reverb
                .process(self.send_buf[i * 2], self.send_buf[i * 2 + 1]);
            let mut l = wl * self.reverb_wet;
            let mut r = wr * self.reverb_wet;
            for b in &self.buses {
                l += b.buf[i * 2];
                r += b.buf[i * 2 + 1];
            }
            l *= master;
            r *= master;
            min_gain = min_gain.min(self.limiter.process(&mut l, &mut r));
            self.out_block[i * 2] = l;
            self.out_block[i * 2 + 1] = r;
            self.meters.master_peak = self.meters.master_peak.max(l.abs()).max(r.abs());
            self.master_sum_sq += 0.5 * (l * l + r * r);
        }
        self.reverb_wet = wet_target;
        self.meters.limiter_gain = self.meters.limiter_gain.min(min_gain);
        self.clock += n as u64;
        self.meter_frames += n as u32;
        if self.meter_frames as f32 >= self.fs * 0.05 {
            self.publish_meters();
        }
    }

    fn publish_meters(&mut self) {
        let frames = self.meter_frames.max(1) as f32;
        for (i, b) in self.buses.iter_mut().enumerate() {
            self.meters.bus_peak[i] = b.peak;
            self.meters.bus_rms[i] = (b.sum_sq / frames).sqrt();
            b.peak = 0.0;
            b.sum_sq = 0.0;
        }
        self.meters.master_rms = (self.master_sum_sq / frames).sqrt();
        self.meters.voices = self.voices.len() as u32;
        let _ = self.events.try_send(MixerEvent::Meters(self.meters));
        self.meters.master_peak = 0.0;
        self.meters.limiter_gain = 1.0;
        self.master_sum_sq = 0.0;
        self.meter_frames = 0;
    }
}

fn stop_voice(v: &mut Voice, fade: f32, fs: f32) {
    v.stopping = true;
    v.fade_target = 0.0;
    if fade <= 0.0 {
        v.fade = 0.0;
        v.fade_step = 0.0;
    } else {
        v.fade_step = -1.0 / (fade * fs);
    }
}

#[inline]
fn smooth01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    const FS: f32 = 48_000.0;

    /// A sine wave for a fixed time.
    struct Sine {
        phase: f32,
        freq: f32,
        left: usize,
    }

    impl Sound for Sine {
        fn fill(&mut self, out: &mut [f32]) -> usize {
            let n = out.len().min(self.left);
            for o in &mut out[..n] {
                *o = (self.phase * std::f32::consts::TAU).sin() * 0.5;
                self.phase = (self.phase + self.freq / FS).fract();
            }
            self.left -= n;
            n
        }
    }

    fn mixer() -> (Mixer, Sender<Command>, Receiver<MixerEvent>) {
        let (tx, rx) = unbounded();
        let (etx, erx) = unbounded();
        (Mixer::new(FS, rx, etx), tx, erx)
    }

    fn play(id: u32, freq: f32, secs: f32, spatial: Option<Spatial>) -> Command {
        Command::Play(Box::new(Play {
            id: VoiceId(id),
            sound: Box::new(Sine {
                phase: 0.0,
                freq,
                left: (secs * FS) as usize,
            }),
            bus: Bus::World,
            gain: 1.0,
            pitch: 1.0,
            clock: Clock::World(1.0),
            spatial,
            reverb_send: 0.0,
            start_at: None,
            delay_secs: 0.0,
            fade_in: 0.0,
        }))
    }

    fn render(m: &mut Mixer, secs: f32) -> Vec<f32> {
        let mut out = vec![0.0; (secs * FS) as usize * 2];
        m.process(&mut out, 2);
        out
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
            .count()
    }

    fn left(x: &[f32]) -> Vec<f32> {
        x.chunks(2).map(|c| c[0]).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn slow_motion_lowers_the_pitch() {
        let (mut m, tx, _) = mixer();
        tx.send(play(1, 1000.0, 1.0, None)).unwrap();
        let normal = left(&render(&mut m, 0.2));
        tx.send(Command::SetWorldRate {
            rate: 0.5,
            slow_motion: 0.0,
        })
        .unwrap();
        render(&mut m, 0.01);
        let slow = left(&render(&mut m, 0.2));
        let (a, b) = (crossings(&normal), crossings(&slow));
        assert!((a as f32 / b as f32 - 2.0).abs() < 0.1, "{a} vs {b}");
    }

    #[test]
    fn real_clock_ignores_dilation() {
        let (mut m, tx, _) = mixer();
        let mut p = play(1, 1000.0, 1.0, None);
        if let Command::Play(p) = &mut p {
            p.clock = Clock::Real(1.0);
        }
        tx.send(p).unwrap();
        tx.send(Command::SetWorldRate {
            rate: 0.25,
            slow_motion: 0.0,
        })
        .unwrap();
        let out = left(&render(&mut m, 0.2));
        assert!((crossings(&out) as i32 - 400).abs() < 6);
    }

    #[test]
    fn sound_to_the_right_is_louder_on_the_right_and_far_is_quieter() {
        let (mut m, tx, _) = mixer();
        let mut sp = Spatial::at(DVec3::new(5.0, 0.0, 0.0));
        sp.delay = false;
        tx.send(play(1, 500.0, 1.0, Some(sp))).unwrap();
        let out = render(&mut m, 0.2);
        let l = rms(&out.chunks(2).map(|c| c[0]).collect::<Vec<_>>());
        let r = rms(&out.chunks(2).map(|c| c[1]).collect::<Vec<_>>());
        assert!(r > l * 4.0, "left {l} right {r}");

        let (mut m2, tx2, _) = mixer();
        let mut far = Spatial::at(DVec3::new(0.0, 0.0, -50.0));
        far.delay = false;
        tx2.send(play(1, 500.0, 1.0, Some(far))).unwrap();
        let out2 = render(&mut m2, 0.2);
        assert!(rms(&out2) < rms(&out) * 0.2);
    }

    #[test]
    fn distant_sounds_arrive_at_the_speed_of_sound() {
        let (mut m, tx, _) = mixer();
        // 343 m away: one second late.
        let mut sp = Spatial::at(DVec3::new(0.0, 0.0, -343.0));
        sp.max_distance = 2000.0;
        tx.send(play(1, 500.0, 0.5, Some(sp))).unwrap();
        let out = left(&render(&mut m, 1.2));
        let first = out.iter().position(|v| v.abs() > 1e-5).unwrap();
        let t = first as f32 / FS;
        assert!((t - 1.0).abs() < 0.01, "{t}");
    }

    #[test]
    fn finished_voices_are_reported_and_removed() {
        let (mut m, tx, events) = mixer();
        tx.send(play(7, 440.0, 0.05, None)).unwrap();
        render(&mut m, 0.1);
        assert_eq!(m.voice_count(), 0);
        let finished: Vec<_> = events
            .try_iter()
            .filter_map(|e| match e {
                MixerEvent::Finished(id) => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(finished, vec![VoiceId(7)]);
    }

    #[test]
    fn stop_fades_out() {
        let (mut m, tx, _) = mixer();
        tx.send(play(1, 440.0, 10.0, None)).unwrap();
        render(&mut m, 0.1);
        tx.send(Command::Stop {
            id: VoiceId(1),
            fade: 0.05,
        })
        .unwrap();
        let out = render(&mut m, 0.1);
        assert_eq!(m.voice_count(), 0);
        assert!(rms(&out[out.len() / 2..]) < 1e-6);
    }

    #[test]
    fn vacuum_silences_the_world_but_not_the_ui() {
        let (mut m, tx, _) = mixer();
        tx.send(Command::SetMedium {
            density: 0.0,
            speed_scale: 1.0,
            hush_db: 0.0,
        })
        .unwrap();
        tx.send(play(1, 440.0, 1.0, None)).unwrap();
        let world = render(&mut m, 0.1);
        assert!(rms(&world) < 1e-6);
        let mut ui = play(2, 440.0, 1.0, None);
        if let Command::Play(p) = &mut ui {
            p.bus = Bus::Ui;
        }
        tx.send(ui).unwrap();
        let out = render(&mut m, 0.1);
        assert!(rms(&out) > 0.1);
    }

    #[test]
    fn start_at_is_sample_accurate() {
        let (mut m, tx, _) = mixer();
        let mut p = play(1, 440.0, 0.1, None);
        if let Command::Play(p) = &mut p {
            p.start_at = Some(1000);
        }
        tx.send(p).unwrap();
        let out = left(&render(&mut m, 0.05));
        let first = out.iter().position(|v| v.abs() > 1e-6).unwrap();
        // Two frames of resampler latency.
        assert!((1000..=1004).contains(&first), "{first}");
    }
}
