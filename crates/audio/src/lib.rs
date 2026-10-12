#![doc = include_str!("../README.md")]

pub mod ambience;
mod api;
pub mod demo;
pub mod dsp;
pub mod effects;
mod fires;
pub mod footstep;
pub mod mixer;
mod output;
mod systems;
pub mod thunder;
mod water;
pub mod wav;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender, unbounded};
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

pub use footstep::{FootstepSound, FootstepSurfaces, Step, SurfaceSound};
pub use mixer::{Bus, Clock, Meters, Sound, Spatial, VoiceId};

use mixer::{Command, Mixer, MixerEvent, MixerSettings, Play};
use output::Backend;

/// Adds spatial audio: footsteps and impacts from `engine_world` ground contacts and surface
/// materials, rain, wind and thunder from the weather, explosions, bullet whizzes and swings
/// from world events, room reverb, slow-motion pitch, and silence in space.
///
/// Sound goes to the sound card when there is a window, and is rendered offline in step with
/// the frame clock when headless (`--headless`), so recordings (`audio.record`, or the
/// `ENGINE_RECORD_AUDIO=path.wav` environment variable) work everywhere. Adds
/// `engine_world::WorldPlugin` if it is not there yet.
pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        if !app.has_plugin::<engine_world::WorldPlugin>() {
            app.add_plugins(engine_world::WorldPlugin);
        }
        app.init_resource::<AudioSettings>()
            .init_resource::<AudioMix>()
            .init_resource::<FootstepSurfaces>()
            .insert_resource(Audio::new())
            .add_message::<AudioCue>()
            .register_resource::<AudioSettings>(
                "Audio settings: output (Auto/Device/Offline/Off), master_db, speed_of_sound (m/s), \
                 doppler (0..1), air_absorption (/m), slow_motion_pitch (0..1, how much time \
                 dilation lowers pitch), min_pitch, slow_motion_darken (0..1), space_is_silent, \
                 atmosphere_height_m, footstep_distance_m, max_voices, weather_ambience, \
                 occlusion (walls muffle sounds).",
            )
            .register_resource::<AudioMix>(
                "Bus volumes in dB: world (footsteps, impacts, explosions), ambience (rain, wind, \
                 thunder), music, ui, voice; muted silences everything.",
            )
            .register_resource::<FootstepSurfaces>(
                "How each surface material sounds underfoot: layer weights (thump, click, scuff, \
                 grit, crunch, rustle, crackle, squelch, suction, splash, hollow, ring, hiss, \
                 squeak), brightness 0..1, pitch, gain_db. Keyed by SurfaceMaterial name.",
            )
            .register_component::<AudioListener>(
                "The ears. Highest priority wins; without one the active camera listens.",
            )
            .register_component::<ReverbZone>(
                "A room or cave: inside the box (half_extents, m, entity axes) sounds get reverb \
                 (room 0..1 size, damping 0..1, wet 0..1), fading in over fade_m meters.",
            )
            .add_systems(Startup, systems::ensure_output)
            .add_systems(
                PostUpdate,
                (
                    systems::ensure_output,
                    systems::update_listener,
                    systems::play_footsteps,
                    systems::play_world_events,
                    systems::weather_ambience,
                    fires::fire_ambience,
                    water::water_ambience,
                    systems::reverb_zones,
                    systems::follow_emitters,
                    systems::occlusion,
                )
                    .chain()
                    .in_set(UpdateSet::AfterPropagate),
            )
            .add_systems(Last, systems::drive_output);
        api::register(app);
        demo::register(app);
    }
}

/// Where sound goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AudioOutput {
    /// The sound card when there is a window, offline when headless (or when no sound card
    /// is available).
    #[default]
    Auto,
    /// Always try the sound card (falls back to offline if there is none).
    Device,
    /// Render in step with the frame clock without playing (for recordings and tests).
    Offline,
    /// No audio at all.
    Off,
}

/// Global audio settings. Tunable at runtime (JSON) by the console, scripts and AI agents.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Read when the output starts.
    pub output: AudioOutput,
    pub master_db: f32,
    /// m/s. Distant sounds start late by distance / speed of sound.
    pub speed_of_sound: f32,
    /// 0 = no doppler shift, 1 = physical.
    pub doppler: f32,
    /// How fast air swallows high frequencies with distance, per meter.
    pub air_absorption: f32,
    /// How much world time dilation lowers the pitch of world sounds: 1 = like tape (half
    /// speed, an octave down), 0 = not at all.
    pub slow_motion_pitch: f32,
    /// Lowest playback rate slow motion may reach (0.35 ≈ 1.5 octaves down).
    pub min_pitch: f32,
    /// How much slow motion muffles the world and widens the reverb, 0..1.
    pub slow_motion_darken: f32,
    /// No air, no sound: world and ambience fade out with altitude and vanish in space.
    pub space_is_silent: bool,
    /// Scale height of the atmosphere for that fade, meters (Earth ≈ 8500).
    pub atmosphere_height_m: f32,
    /// Footsteps farther than this from the listener are not played, meters.
    pub footstep_distance_m: f32,
    /// Most voices at once; new sounds beyond it are skipped.
    pub max_voices: u32,
    /// Play rain, wind and thunder from the weather.
    pub weather_ambience: bool,
    /// Muffle sounds that walls and terrain stand between (physics raycasts).
    pub occlusion: bool,
    /// Play the roar and crackle of fires near the listener (burning ground in `HeatField`,
    /// burning `Combustible`s).
    pub fires: bool,
    /// Play waves lapping at nearby shores and streams babbling (from the water module's
    /// `WaterQuery`).
    pub water: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            output: AudioOutput::Auto,
            master_db: 0.0,
            speed_of_sound: 343.0,
            doppler: 1.0,
            air_absorption: 0.006,
            slow_motion_pitch: 0.8,
            min_pitch: 0.35,
            slow_motion_darken: 0.7,
            space_is_silent: true,
            atmosphere_height_m: 8_500.0,
            footstep_distance_m: 60.0,
            max_voices: 96,
            weather_ambience: true,
            occlusion: true,
            fires: true,
            water: true,
        }
    }
}

impl AudioSettings {
    /// Playback rate of world sounds at world time scale `scale`.
    pub fn world_rate(&self, scale: f32) -> f32 {
        if scale <= 0.0 {
            0.0
        } else if scale >= 1.0 {
            scale
        } else {
            scale
                .powf(self.slow_motion_pitch.clamp(0.0, 1.0))
                .max(self.min_pitch)
        }
    }

    fn mixer(&self) -> MixerSettings {
        MixerSettings {
            speed_of_sound: self.speed_of_sound,
            doppler: self.doppler,
            air_absorption: self.air_absorption,
            master_gain: dsp::db(self.master_db),
        }
    }
}

/// Bus volumes in dB.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioMix {
    pub world_db: f32,
    pub ambience_db: f32,
    pub music_db: f32,
    pub ui_db: f32,
    pub voice_db: f32,
    pub muted: bool,
}

impl Default for AudioMix {
    fn default() -> Self {
        Self {
            world_db: 0.0,
            ambience_db: -3.0,
            music_db: -6.0,
            ui_db: -6.0,
            voice_db: 0.0,
            muted: false,
        }
    }
}

impl AudioMix {
    pub fn gain(&self, bus: Bus) -> f32 {
        if self.muted {
            return 0.0;
        }
        dsp::db(match bus {
            Bus::World => self.world_db,
            Bus::Ambience => self.ambience_db,
            Bus::Music => self.music_db,
            Bus::Ui => self.ui_db,
            Bus::Voice => self.voice_db,
        })
    }
}

/// The ears. Put it on the player's head or camera; the highest `priority` wins. Without one,
/// the active camera with the highest order listens.
#[derive(Component, Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioListener {
    pub priority: i32,
}

/// An enclosed space that echoes: a cave, a hall, a tunnel, a ship's hold. When the listener
/// is inside the box (entity axes), sounds get this reverb.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct ReverbZone {
    /// Half size of the box, meters.
    pub half_extents: Vec3,
    /// 0 small room .. 1 cathedral.
    pub room: f32,
    /// 0 hard stone .. 1 soft furnishings.
    pub damping: f32,
    /// Reverb level, 0..1.
    pub wet: f32,
    /// Blend in over this many meters inside the walls.
    pub fade_m: f32,
}

impl Default for ReverbZone {
    fn default() -> Self {
        Self {
            half_extents: Vec3::splat(5.0),
            room: 0.7,
            damping: 0.35,
            wet: 0.35,
            fade_m: 2.0,
        }
    }
}

impl ReverbZone {
    pub fn cave(half_extents: Vec3) -> Self {
        Self {
            half_extents,
            room: 0.92,
            damping: 0.25,
            wet: 0.5,
            fade_m: 3.0,
        }
    }
}

/// Sent whenever the audio module starts a sound in the world: debug views, the showcase's
/// sound rings and AI agents can see what is being heard.
#[derive(Message, Debug, Clone)]
pub struct AudioCue {
    pub voice: VoiceId,
    pub position: Option<DVec3>,
    /// What it is: `"footstep.mud"`, `"explosion"`, `"impact.metal"`, …
    pub name: String,
    /// Rough loudness at 1 m, 0..1.
    pub loudness: f32,
}

/// How to play a sound. Start from [`PlayOptions::at`] (3D) or [`PlayOptions::ui`] (2D).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayOptions {
    /// World position; `None` plays it unpositioned (centered, or stereo as recorded).
    pub at: Option<DVec3>,
    pub velocity: Vec3,
    /// Keep the sound attached to this entity as it moves.
    pub follow: Option<Entity>,
    /// The entity making the sound (ignored when checking what blocks it).
    pub source: Option<Entity>,
    pub bus: Bus,
    pub gain_db: f32,
    /// Playback rate (2 = an octave up).
    pub pitch: f32,
    pub clock: Clock,
    /// Full volume within this distance, meters.
    pub ref_distance: f32,
    /// Silent beyond this distance, meters (also used to skip sounds nobody can hear).
    pub max_distance: f32,
    pub rolloff: f32,
    /// Start late by distance / speed of sound.
    pub propagation_delay: bool,
    /// Extra delay before starting, seconds.
    pub delay_secs: f32,
    /// Room reverb send, 0..1.
    pub reverb: f32,
    pub fade_in: f32,
}

impl PlayOptions {
    /// A sound in the world at `position`.
    pub fn at(position: DVec3) -> Self {
        Self {
            at: Some(position),
            velocity: Vec3::ZERO,
            follow: None,
            source: None,
            bus: Bus::World,
            gain_db: 0.0,
            pitch: 1.0,
            clock: Clock::World(1.0),
            ref_distance: 1.0,
            max_distance: 120.0,
            rolloff: 1.0,
            propagation_delay: true,
            delay_secs: 0.0,
            reverb: 0.3,
            fade_in: 0.0,
        }
    }

    /// An unpositioned sound on the UI bus that ignores slow motion.
    pub fn ui() -> Self {
        Self {
            at: None,
            bus: Bus::Ui,
            clock: Clock::Real(1.0),
            reverb: 0.0,
            ..Self::at(DVec3::ZERO)
        }
    }

    /// An unpositioned bed on the ambience bus (rain, wind).
    pub fn ambience() -> Self {
        Self {
            at: None,
            bus: Bus::Ambience,
            reverb: 0.0,
            ..Self::at(DVec3::ZERO)
        }
    }

    pub fn bus(mut self, bus: Bus) -> Self {
        self.bus = bus;
        self
    }

    pub fn gain_db(mut self, db: f32) -> Self {
        self.gain_db = db;
        self
    }

    pub fn pitch(mut self, pitch: f32) -> Self {
        self.pitch = pitch;
        self
    }

    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn range(mut self, ref_distance: f32, max_distance: f32) -> Self {
        self.ref_distance = ref_distance;
        self.max_distance = max_distance;
        self
    }

    pub fn follow(mut self, entity: Entity) -> Self {
        self.follow = Some(entity);
        self.source = Some(entity);
        self
    }

    /// The entity making the sound, so its own collider doesn't muffle it.
    pub fn source(mut self, entity: Option<Entity>) -> Self {
        self.source = entity;
        self
    }

    pub fn reverb(mut self, send: f32) -> Self {
        self.reverb = send;
        self
    }

    pub fn fade_in(mut self, secs: f32) -> Self {
        self.fade_in = secs;
        self
    }

    pub fn delay(mut self, secs: f32) -> Self {
        self.delay_secs = secs;
        self
    }

    pub fn no_propagation_delay(mut self) -> Self {
        self.propagation_delay = false;
        self
    }
}

struct Live {
    follow: Option<Entity>,
    source: Option<Entity>,
    /// Where it is now (positioned sounds).
    last: Option<DVec3>,
    bus: Bus,
    /// Occlusion last sent, and when to check again (real seconds).
    occlusion: f32,
    check_at: f64,
}

struct Recording {
    path: PathBuf,
    samples: Vec<f32>,
    max_frames: usize,
}

/// The running audio output. Systems play sounds through it:
///
/// ```ignore
/// fn bell(mut audio: ResMut<Audio>, q: Query<&GlobalTransform, With<Bell>>) {
///     for t in &q {
///         let fs = audio.sample_rate();
///         audio.play(ToneSound::new(fs, 660.0, 1.0, 0.3), PlayOptions::at(t.translation()));
///     }
/// }
/// ```
#[derive(Resource)]
pub struct Audio {
    tx: Sender<Command>,
    events: Receiver<MixerEvent>,
    pending: Option<(Receiver<Command>, Sender<MixerEvent>)>,
    backend: Option<Backend>,
    status: String,
    sample_rate: f32,
    next_id: u32,
    seed: u64,
    live: HashMap<VoiceId, Live>,
    max_voices: usize,
    listener: DVec3,
    listener_velocity: Vec3,
    /// The entity the ears are on (its own colliders don't block sound).
    listener_entity: Option<Entity>,
    /// Mixer sample to start new sounds at (offline output only).
    start_sample: Option<u64>,
    meters: Meters,
    recording: Option<Recording>,
    capture: Option<Receiver<Vec<f32>>>,
    scratch: Vec<f32>,
}

impl Audio {
    fn new() -> Self {
        let (tx, rx) = unbounded();
        let (etx, erx) = unbounded();
        Self {
            tx,
            events: erx,
            pending: Some((rx, etx)),
            backend: None,
            status: "not started".into(),
            sample_rate: 48_000.0,
            next_id: 1,
            seed: 0x5EED,
            live: HashMap::new(),
            max_voices: 96,
            listener: DVec3::ZERO,
            listener_velocity: Vec3::ZERO,
            listener_entity: None,
            start_sample: None,
            meters: Meters::default(),
            recording: None,
            capture: None,
            scratch: Vec::new(),
        }
    }

    /// Start the output (once). `headless` and `settings.output` pick device or offline.
    fn start(&mut self, headless: bool, settings: &AudioSettings) {
        let Some((rx, etx)) = self.pending.take() else {
            return;
        };
        let want_device = match settings.output {
            AudioOutput::Off => {
                self.status = "off".into();
                return;
            }
            AudioOutput::Auto => !headless,
            AudioOutput::Device => true,
            AudioOutput::Offline => false,
        };
        if want_device {
            match output::open_device(rx, etx) {
                Ok((device, fs)) => {
                    self.sample_rate = fs;
                    self.backend = Some(Backend::Device(device));
                    self.status = self.backend.as_ref().map(|b| b.name()).unwrap_or_default();
                    engine_core::log::info!("audio: {} at {fs} Hz", self.status);
                    return;
                }
                Err(e) => {
                    engine_core::log::warn!("audio: {e}; rendering offline instead");
                    // The failed device took the channel ends with it: make new ones.
                    let (tx, rx) = unbounded();
                    let (etx, erx) = unbounded();
                    self.tx = tx;
                    self.events = erx;
                    self.start_offline(rx, etx);
                    self.status = format!("offline ({e})");
                    return;
                }
            }
        }
        self.start_offline(rx, etx);
        self.status = "offline".into();
    }

    fn start_offline(&mut self, rx: Receiver<Command>, etx: Sender<MixerEvent>) {
        self.sample_rate = 48_000.0;
        let mixer = Mixer::new(self.sample_rate, rx, etx);
        self.backend = Some(Backend::Offline(Box::new(Mutex::new(mixer))));
    }

    /// Mixer sample rate, Hz. Pass it to sound constructors.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// `"device: <name>"`, `"offline"`, `"off"` or why not.
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn is_running(&self) -> bool {
        self.backend.is_some()
    }

    pub fn is_offline(&self) -> bool {
        matches!(self.backend, Some(Backend::Offline(_)))
    }

    /// Voices playing (or waiting to start) right now.
    pub fn voice_count(&self) -> usize {
        self.live.len()
    }

    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.live.contains_key(&id)
    }

    pub fn listener_position(&self) -> DVec3 {
        self.listener
    }

    /// Latest level meters (updated every 50 ms of audio).
    pub fn meters(&self) -> Meters {
        self.meters
    }

    /// A fresh seed for a sound's randomness.
    pub fn next_seed(&mut self) -> u64 {
        self.seed = self
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.seed
    }

    /// Play a sound. Returns [`VoiceId::NONE`] if it was skipped: audio off, too far away to
    /// hear, or too many voices.
    pub fn play(&mut self, sound: impl Sound + 'static, opts: PlayOptions) -> VoiceId {
        self.play_boxed(Box::new(sound), opts)
    }

    /// [`Audio::play`] for an already boxed sound.
    pub fn play_boxed(&mut self, sound: Box<dyn Sound>, opts: PlayOptions) -> VoiceId {
        if self.backend.is_none() && self.pending.is_none() {
            return VoiceId::NONE;
        }
        if let Some(p) = opts.at
            && p.distance(self.listener) > opts.max_distance as f64
        {
            return VoiceId::NONE;
        }
        if self.live.len() >= self.max_voices && opts.bus != Bus::Ambience {
            return VoiceId::NONE;
        }
        let id = VoiceId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let spatial = opts.at.map(|position| Spatial {
            position,
            velocity: opts.velocity,
            ref_distance: opts.ref_distance,
            max_distance: opts.max_distance,
            rolloff: opts.rolloff,
            delay: opts.propagation_delay,
        });
        let play = Play {
            id,
            sound,
            bus: opts.bus,
            gain: dsp::db(opts.gain_db),
            pitch: opts.pitch,
            clock: opts.clock,
            spatial,
            reverb_send: opts.reverb,
            start_at: self.start_sample,
            delay_secs: opts.delay_secs,
            fade_in: opts.fade_in,
        };
        if self.tx.send(Command::Play(Box::new(play))).is_ok() {
            self.live.insert(
                id,
                Live {
                    follow: opts.follow,
                    source: opts.source.or(opts.follow),
                    last: opts.at,
                    bus: opts.bus,
                    occlusion: 0.0,
                    check_at: 0.0,
                },
            );
            id
        } else {
            VoiceId::NONE
        }
    }

    /// Fade a voice out over `fade` seconds and end it.
    pub fn stop(&self, id: VoiceId, fade: f32) {
        let _ = self.tx.send(Command::Stop { id, fade });
    }

    /// Fade every voice out.
    pub fn stop_all(&self, fade: f32) {
        let _ = self.tx.send(Command::StopAll { fade });
    }

    /// Change a live parameter of a playing sound (`"intensity"`, `"speed"`, …).
    pub fn set_param(&self, id: VoiceId, name: &'static str, value: f32) {
        let _ = self.tx.send(Command::SetParam { id, name, value });
    }

    pub fn set_gain_db(&self, id: VoiceId, db: f32) {
        let _ = self.tx.send(Command::SetGain {
            id,
            gain: dsp::db(db),
        });
    }

    pub fn set_pitch(&self, id: VoiceId, pitch: f32) {
        let _ = self.tx.send(Command::SetPitch { id, pitch });
    }

    /// Move a positioned voice.
    pub fn set_position(&self, id: VoiceId, position: DVec3, velocity: Vec3) {
        let _ = self.tx.send(Command::SetEmitter {
            id,
            position,
            velocity,
        });
    }

    fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    /// Record everything heard to a 16-bit stereo WAV at `path`, for up to `max_secs`
    /// seconds (or until [`Audio::stop_recording`], or the app exits).
    pub fn start_recording(
        &mut self,
        path: impl Into<PathBuf>,
        max_secs: f32,
    ) -> Result<(), String> {
        if self.backend.is_none() {
            return Err("audio output is off; set AudioSettings.output to Auto or Offline".into());
        }
        if self.recording.is_some() {
            return Err("already recording; stop the current recording first".into());
        }
        if matches!(self.backend, Some(Backend::Device(_))) {
            let (tx, rx) = unbounded();
            self.send(Command::Capture(Some(tx)));
            self.capture = Some(rx);
        }
        self.recording = Some(Recording {
            path: path.into(),
            samples: Vec::new(),
            max_frames: (max_secs.max(0.1) * self.sample_rate) as usize,
        });
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Finish the recording and write the WAV. Returns its path and length in seconds.
    pub fn stop_recording(&mut self) -> Result<(PathBuf, f32), String> {
        let rec = self.recording.take().ok_or("not recording")?;
        if self.capture.take().is_some() {
            self.send(Command::Capture(None));
        }
        let secs = rec.samples.len() as f32 / 2.0 / self.sample_rate;
        wav::write_wav(&rec.path, &rec.samples, 2, self.sample_rate as u32)
            .map_err(|e| format!("could not write {}: {e}", rec.path.display()))?;
        engine_core::log::info!("audio: wrote {} ({secs:.1} s)", rec.path.display());
        Ok((rec.path, secs))
    }

    fn record(&mut self, samples: &[f32]) {
        let mut full = false;
        if let Some(rec) = &mut self.recording {
            let room = rec.max_frames * 2 - rec.samples.len().min(rec.max_frames * 2);
            let n = samples.len().min(room);
            rec.samples.extend_from_slice(&samples[..n]);
            full = rec.samples.len() >= rec.max_frames * 2;
        }
        if full && let Err(e) = self.stop_recording() {
            engine_core::log::warn!("audio: {e}");
        }
    }

    /// Advance: read mixer events, render offline output up to `until_secs` of real time.
    fn drive(&mut self, until_secs: f64) {
        while let Ok(e) = self.events.try_recv() {
            match e {
                MixerEvent::Finished(id) => {
                    self.live.remove(&id);
                }
                MixerEvent::Meters(m) => self.meters = m,
            }
        }
        if let Some(rx) = &self.capture {
            let chunks: Vec<Vec<f32>> = rx.try_iter().collect();
            for c in chunks {
                self.record(&c);
            }
        }
        if let Some(Backend::Offline(mixer)) = &self.backend {
            let mut mixer = mixer.lock().unwrap_or_else(|e| e.into_inner());
            let target = (until_secs * mixer.sample_rate() as f64).round() as u64;
            let frames = target
                .saturating_sub(mixer.clock())
                .min(mixer.sample_rate() as u64 * 2);
            let mut buf = std::mem::take(&mut self.scratch);
            buf.resize(frames as usize * 2, 0.0);
            mixer.process(&mut buf, 2);
            // Skipped ahead after a long stall: keep the mixer clock in step with real time.
            drop(mixer);
            if self.recording.is_some() {
                self.record(&buf);
            }
            self.scratch = buf;
            // Events produced by this render (finished voices) are read next frame.
        }
    }

    /// Sample a voice's attached entity moved to (internal).
    fn followers(&self) -> impl Iterator<Item = (VoiceId, Entity, Option<DVec3>)> + '_ {
        self.live
            .iter()
            .filter_map(|(id, l)| l.follow.map(|e| (*id, e, l.last)))
    }

    fn set_last(&mut self, id: VoiceId, p: DVec3) {
        if let Some(l) = self.live.get_mut(&id) {
            l.last = Some(p);
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        if self.recording.is_some()
            && let Err(e) = self.stop_recording()
        {
            engine_core::log::warn!("audio: {e}");
        }
    }
}

/// Write `samples` (interleaved stereo at `fs`) to `path` (helper for tools).
pub fn save_wav(path: &Path, samples: &[f32], fs: f32) -> std::io::Result<()> {
    wav::write_wav(path, samples, 2, fs as u32)
}
