//! The ECS side: listener, footsteps, world events, weather ambience, reverb zones, and
//! driving the output each frame.

use std::collections::HashMap;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::animation::AnimationParams;
use engine_world::physics::{Collider, ContactEvent, ContactPhase, PhysicsQueries, QueryFilter};
use engine_world::{
    AttackEvent, AttackKind, Env, GroundContact, PrecipitationKind, ProjectileTrace, SurfaceCell,
    WorldImpulse,
};

use crate::ambience::{RainSound, WindSound};
use crate::dsp::{Noise, to_db};
use crate::effects::{ExplosionSound, WhizSound, WhooshSound};
use crate::footstep::{FootstepSound, Step, SurfaceSound};
use crate::mixer::{Bus, Clock, Command, VoiceId};
use crate::samples::RecordedSteps;
use crate::thunder;
use crate::{
    Audio, AudioCue, AudioListener, AudioMix, AudioSettings, FootstepSamples, FootstepSurfaces,
    PlayOptions, ReverbZone,
};

pub(crate) fn ensure_output(
    mut audio: ResMut<Audio>,
    config: Res<EngineConfig>,
    settings: Res<AudioSettings>,
) {
    if audio.pending.is_none() {
        return;
    }
    audio.max_voices = settings.max_voices as usize;
    audio.start(config.headless, &settings);
    audio.send(Command::Settings(settings.mixer()));
    if let Ok(path) = std::env::var("ENGINE_RECORD_AUDIO")
        && !path.is_empty()
    {
        let secs = std::env::var("ENGINE_RECORD_AUDIO_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(600.0);
        match audio.start_recording(&path, secs) {
            Ok(()) => engine_core::log::info!("audio: recording to {path}"),
            Err(e) => engine_core::log::warn!("audio: can't record to {path}: {e}"),
        }
    }
}

/// Clock for sounds made by `entity`: its own time scale if it has one.
pub(crate) fn clock_for(entity: Option<Entity>, scales: &Query<&LocalTimeScale>) -> Clock {
    match entity.and_then(|e| scales.get(e).ok()) {
        Some(s) if s.ignore_world_dilation => Clock::Real(s.factor),
        Some(s) => Clock::World(s.factor),
        None => Clock::World(1.0),
    }
}

/// Standard sea-level air: kg/m³ and m/s (15 °C).
const SEA_LEVEL_DENSITY: f32 = 1.225;
const SEA_LEVEL_SPEED_OF_SOUND: f32 = 340.3;

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_listener(
    mut audio: ResMut<Audio>,
    settings: Res<AudioSettings>,
    mix: Res<AudioMix>,
    time: Res<Time>,
    env: Env,
    listeners: Query<(Entity, &GlobalTransform, &AudioListener)>,
    cameras: Query<(Entity, &GlobalTransform, &Camera)>,
    mut last: Local<Option<DVec3>>,
) {
    if !audio.is_running() {
        return;
    }
    let pose = listeners
        .iter()
        .max_by_key(|(_, _, l)| l.priority)
        .map(|(e, t, _)| (e, t))
        .or_else(|| {
            cameras
                .iter()
                .filter(|(_, _, c)| c.active)
                .max_by_key(|(_, _, c)| c.order)
                .map(|(e, t, _)| (e, t))
        });
    audio.listener_entity = pose.map(|(e, _)| e);
    let (pos, rot) = pose
        .map(|(_, t)| (t.translation(), t.rotation()))
        .unwrap_or((DVec3::ZERO, Quat::IDENTITY));
    let dt = time.real_delta_secs().max(1e-4) as f64;
    // A jump of more than 50 m in one frame is a teleport, not a velocity.
    let velocity = match *last {
        Some(p) if p.distance(pos) < 50.0 => ((pos - p) / dt).as_vec3(),
        _ => Vec3::ZERO,
    };
    *last = Some(pos);
    audio.listener = pos;
    audio.listener_velocity = velocity;
    audio.send(Command::SetListener {
        position: pos,
        right: rot * Vec3::X,
        up: rot * Vec3::Y,
        forward: rot * Vec3::NEG_Z,
        velocity,
    });

    let scale = if time.is_paused() { 0.0 } else { time.scale() };
    let slow = if scale > 0.0 && scale < 1.0 {
        settings.slow_motion_darken.clamp(0.0, 1.0) * (1.0 - scale)
    } else {
        0.0
    };
    audio.send(Command::SetWorldRate {
        rate: settings.world_rate(scale),
        slow_motion: slow,
    });

    // Air: thins out with altitude, differs per planet, none in space.
    let air = env.air(pos);
    let density = if settings.space_is_silent {
        (air.density / SEA_LEVEL_DENSITY).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let speed_scale = if air.density > 0.0 {
        air.speed_of_sound / SEA_LEVEL_SPEED_OF_SOUND
    } else {
        1.0
    };
    audio.speed_of_sound = settings.speed_of_sound * speed_scale;
    // Falling and lying snow swallow the highs.
    let w = &env.weather;
    let mut hush = 0.0;
    if w.is_snowing() {
        hush -= 6.0 * smooth(0.0, 4.0, w.precipitation_mm_h);
    }
    hush -= 3.0 * smooth(0.02, 0.3, env.surface_state(pos).snow_m);
    audio.send(Command::SetMedium {
        density,
        speed_scale,
        hush_db: hush,
    });

    if mix.is_changed() {
        for bus in Bus::ALL {
            audio.send(Command::SetBus {
                bus,
                gain: mix.gain(bus),
                lowpass_hz: 22_000.0,
            });
        }
    }
    if settings.is_changed() {
        audio.max_voices = settings.max_voices as usize;
        audio.send(Command::Settings(settings.mixer()));
    }
    audio.start_sample = if audio.is_offline() {
        let start = (time.real_elapsed_secs_f64() - dt).max(0.0);
        Some((start * audio.sample_rate() as f64).round() as u64)
    } else {
        None
    };
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Most footsteps started per frame (a stampede still sounds like one).
const MAX_STEPS_PER_FRAME: usize = 24;

#[allow(clippy::too_many_arguments)]
pub(crate) fn play_footsteps(
    mut contacts: MessageReader<GroundContact>,
    mut audio: ResMut<Audio>,
    env: Env,
    surfaces: Res<FootstepSurfaces>,
    mut samples: ResMut<FootstepSamples>,
    settings: Res<AudioSettings>,
    scales: Query<&LocalTimeScale>,
    animated: Query<&AnimationParams>,
    switches: Query<&RecordedSteps>,
    mut cues: MessageWriter<AudioCue>,
) {
    if !audio.is_running() {
        contacts.clear();
        return;
    }
    let max = settings.footstep_distance_m;
    let snow = surfaces.snow();
    let mut played = 0;
    for c in contacts.read() {
        if played >= MAX_STEPS_PER_FRAME || c.position.distance(audio.listener) > max as f64 {
            continue;
        }
        let id = c
            .surface
            .or_else(|| env.ground(c.position).map(|g| g.surface))
            .unwrap_or_default();
        let material = env.material(id);
        let cell = env.surface_state(c.position);
        let temperature = env.temperature(c.position);
        let wading = env
            .water(c.position)
            .map(|w| (w.surface_height - c.position.y) as f32)
            .filter(|d| *d > 0.0)
            .unwrap_or(0.0)
            .min(1.5);
        let dry = surfaces.get(material);
        let sound = dry.with_state(&cell, material.absorbency, temperature, wading, &snow);
        // A contact that leaves its speed out (an animation clip's footstep) goes at the pace
        // its walker's animation says, so a stride sounds like a stride.
        let speed = match c.entity.and_then(|e| animated.get(e).ok()) {
            Some(params) if c.speed <= 0.0 => params.get("speed").max(0.0),
            _ => c.speed,
        };
        let step = Step::from_contact(sound, c.kind, c.load_kg, speed, audio.next_seed());
        let fs = audio.sample_rate();
        let opts = PlayOptions::at(c.position)
            .range(1.5, max)
            .clock(clock_for(c.entity, &scales))
            .source(c.entity)
            .reverb(0.35);
        // Takes unless they are off, or off for this walker.
        let takes = settings.recorded_footsteps
            && c.entity
                .and_then(|e| switches.get(e).ok())
                .is_none_or(|s| s.enabled);
        let recorded = if takes {
            let wait = audio.is_offline();
            samples.recorded(
                &material.name,
                &dry,
                &cell,
                wading,
                &surfaces,
                &step,
                fs,
                wait,
            )
        } else {
            None
        };
        let sound = match &recorded {
            Some((synth, take)) => FootstepSound::with_sample(fs, synth, take),
            None => FootstepSound::new(fs, &step),
        };
        let voice = audio.play(sound, opts);
        if !voice.is_none() {
            played += 1;
            cues.write(AudioCue {
                voice,
                position: Some(c.position),
                name: format!("footstep.{}", material.name),
                loudness: (step.intensity * 0.4).min(1.0),
            });
        }
    }
}

/// Material sound of a collider's surface (default when it has no collider).
fn collider_sound(
    e: Entity,
    colliders: &Query<&Collider>,
    env: &Env,
    surfaces: &FootstepSurfaces,
) -> (SurfaceSound, String) {
    let id = colliders.get(e).map(|c| c.surface).unwrap_or_default();
    let m = env.material(id);
    (surfaces.get(m), m.name.clone())
}

#[derive(Default)]
pub(crate) struct ImpactMemory {
    last: HashMap<(Entity, Entity), f64>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn play_world_events(
    mut audio: ResMut<Audio>,
    mut impulses: MessageReader<WorldImpulse>,
    mut traces: MessageReader<ProjectileTrace>,
    mut attacks: MessageReader<AttackEvent>,
    mut contacts: MessageReader<ContactEvent>,
    transforms: Query<&GlobalTransform>,
    colliders: Query<&Collider>,
    scales: Query<&LocalTimeScale>,
    env: Env,
    surfaces: Res<FootstepSurfaces>,
    time: Res<Time>,
    strikes: Res<thunder::Strikes>,
    mut cues: MessageWriter<AudioCue>,
    mut memory: Local<ImpactMemory>,
) {
    if !audio.is_running() {
        impulses.clear();
        traces.clear();
        attacks.clear();
        contacts.clear();
        return;
    }
    let fs = audio.sample_rate();
    let listener = audio.listener;

    // Explosions and other blasts.
    for i in impulses.read() {
        let energy = i.energy_j.max(i.strength * i.strength * 2_000.0);
        // A lightning strike's blast is already in its thunder.
        if energy < 5e4 || strikes.is_strike(i.center) {
            continue;
        }
        let seed = audio.next_seed();
        let voice = audio.play(
            ExplosionSound::new(fs, energy, seed),
            PlayOptions::at(i.center)
                .range(6.0 + i.radius, 6_000.0)
                .reverb(0.6),
        );
        if !voice.is_none() {
            cues.write(AudioCue {
                voice,
                position: Some(i.center),
                name: "explosion".into(),
                loudness: 1.0,
            });
        }
    }

    // Bullets, arrows and bolts passing close by.
    for t in traces.read() {
        let seg = t.to - t.from;
        let len2 = seg.length_squared();
        if len2 < 1e-6 || t.from.distance(listener) < 3.0 {
            continue;
        }
        let k = ((listener - t.from).dot(seg) / len2).clamp(0.0, 1.0);
        let closest = t.from + seg * k;
        let miss = closest.distance(listener) as f32;
        if miss > 10.0 || k <= 0.0 || k >= 1.0 {
            continue;
        }
        let seed = audio.next_seed();
        let voice = audio.play(
            WhizSound::new(fs, t.speed, miss, seed),
            PlayOptions::at(closest)
                .range(1.0, 30.0)
                .no_propagation_delay()
                .reverb(0.1),
        );
        if !voice.is_none() {
            cues.write(AudioCue {
                voice,
                position: Some(closest),
                name: "whiz".into(),
                loudness: 0.5,
            });
        }
    }

    // Swings, spells and dashes.
    for a in attacks.read() {
        let (secs, strength) = match a.kind {
            AttackKind::Melee => (a.windup_secs.clamp(0.12, 0.45) + 0.08, 1.0),
            AttackKind::Dash => (0.4, 1.8),
            AttackKind::Spell => (0.5, 0.7),
            AttackKind::Ranged => continue,
        };
        let Ok(t) = transforms.get(a.attacker) else {
            continue;
        };
        let seed = audio.next_seed();
        let voice = audio.play(
            WhooshSound::new(fs, secs, strength, seed),
            PlayOptions::at(t.translation())
                .range(1.5, 40.0)
                .clock(clock_for(Some(a.attacker), &scales))
                .follow(a.attacker),
        );
        if !voice.is_none() {
            cues.write(AudioCue {
                voice,
                position: Some(t.translation()),
                name: format!("whoosh.{:?}", a.kind).to_lowercase(),
                loudness: 0.4 * strength,
            });
        }
    }

    // Physics impacts: what hit what, how hard.
    let now = time.elapsed_secs_f64();
    let mut played = 0;
    for c in contacts.read() {
        if c.phase != ContactPhase::Started || c.sensor || c.impulse < 2.0 || played >= 8 {
            continue;
        }
        if c.point.distance(listener) > 80.0 {
            continue;
        }
        let key = if c.a < c.b { (c.a, c.b) } else { (c.b, c.a) };
        if memory.last.get(&key).is_some_and(|t| now - t < 0.08) {
            continue;
        }
        memory.last.insert(key, now);
        let (sa, na) = collider_sound(c.a, &colliders, &env, &surfaces);
        let (sb, nb) = collider_sound(c.b, &colliders, &env, &surfaces);
        let sound = sa.blend(&sb, 0.5);
        let strength = (c.impulse / 40.0).sqrt().clamp(0.1, 3.0);
        let step = Step::impact(sound, strength, audio.next_seed());
        let voice = audio.play(
            FootstepSound::new(fs, &step),
            PlayOptions::at(c.point).range(1.5, 80.0).reverb(0.4),
        );
        if !voice.is_none() {
            played += 1;
            let name = if na == nb { na } else { format!("{na}+{nb}") };
            cues.write(AudioCue {
                voice,
                position: Some(c.point),
                name: format!("impact.{name}"),
                loudness: (strength * 0.4).min(1.0),
            });
        }
    }
    if memory.last.len() > 512 {
        memory.last.retain(|_, t| now - *t < 1.0);
    }
}

#[derive(Default)]
pub(crate) struct AmbienceState {
    rain: VoiceId,
    wind: VoiceId,
    next_thunder: f32,
    rng: Option<Noise>,
}

/// What rain falling on this spot sounds like: 0 soft, 1 hard, 2 water, 3 metal.
fn rain_class(s: &SurfaceSound, cell: &SurfaceCell) -> usize {
    if cell.puddle_m > 0.004 || s.splash > 0.5 {
        2
    } else if s.ring > 0.5 {
        3
    } else if s.click > 0.5 || s.hollow > 0.5 {
        1
    } else {
        0
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn weather_ambience(
    mut audio: ResMut<Audio>,
    settings: Res<AudioSettings>,
    env: Env,
    physics: Res<PhysicsQueries>,
    surfaces: Res<FootstepSurfaces>,
    time: Res<Time>,
    strikes: Res<thunder::Strikes>,
    mut cues: MessageWriter<AudioCue>,
    mut state: Local<AmbienceState>,
) {
    if !audio.is_running() {
        return;
    }
    let state = &mut *state;
    if !settings.weather_ambience {
        for id in [state.rain, state.wind] {
            if !id.is_none() {
                audio.stop(id, 1.0);
            }
        }
        state.rain = VoiceId::NONE;
        state.wind = VoiceId::NONE;
        return;
    }
    let fs = audio.sample_rate();
    let p = audio.listener;
    let w = env.weather.clone();
    let up = env.up(p);
    let roof = physics.raycast(p, up, 40.0, &QueryFilter::default());

    // Rain.
    let rain = if w.is_raining() || w.precipitation_kind == PrecipitationKind::Hail {
        w.precipitation_mm_h
    } else {
        0.0
    };
    if rain > 0.02 {
        if !audio.is_playing(state.rain) {
            let seed = audio.next_seed();
            state.rain = audio.play(
                RainSound::new(fs, seed),
                PlayOptions::ambience().fade_in(2.0),
            );
        }
        let mut mix = [0.0f32; 4];
        if let Some(hit) = roof {
            // Under cover: the rain drums on the roof.
            let s = surfaces.get(env.material(hit.surface));
            mix[rain_class(&s, &SurfaceCell::default())] = 1.0;
        } else {
            for dz in [-6.0, 0.0, 6.0] {
                for dx in [-6.0, 0.0, 6.0] {
                    let q = p + DVec3::new(dx, 0.0, dz);
                    let id = env.ground(q).map(|g| g.surface).unwrap_or_default();
                    let s = surfaces.get(env.material(id));
                    let water = env
                        .water(q)
                        .is_some_and(|w| w.surface_height > q.y - 50.0 && w.depth > 0.05);
                    let class = if water {
                        2
                    } else {
                        rain_class(&s, &env.surface_state(q))
                    };
                    mix[class] += 1.0 / 9.0;
                }
            }
        }
        let id = state.rain;
        audio.set_param(
            id,
            "intensity",
            rain * if roof.is_some() { 0.6 } else { 1.0 },
        );
        for (i, name) in ["soft", "hard", "water", "metal"].into_iter().enumerate() {
            audio.set_param(id, name, mix[i]);
        }
    } else if !state.rain.is_none() {
        audio.stop(state.rain, 3.0);
        state.rain = VoiceId::NONE;
    }

    // Wind: the air moving past the ears (wind minus your own motion).
    let air = env.wind(p) - audio.listener_velocity;
    let speed = air.length() * if roof.is_some() { 0.5 } else { 1.0 };
    if speed > 0.3 {
        if !audio.is_playing(state.wind) {
            let seed = audio.next_seed();
            state.wind = audio.play(
                WindSound::new(fs, seed),
                PlayOptions::ambience().fade_in(1.5),
            );
        }
        audio.set_param(state.wind, "speed", speed);
    } else if !state.wind.is_none() {
        audio.stop(state.wind, 2.0);
        state.wind = VoiceId::NONE;
    }

    // Thunder, made up while the weather doesn't send its own lightning strikes.
    if w.storm > 0.02 && !strikes.weather_has_lightning(time.real_elapsed_secs_f64()) {
        let seed = audio.next_seed();
        let rng = state.rng.get_or_insert_with(|| Noise::new(seed));
        state.next_thunder -= time.delta_secs();
        if state.next_thunder <= 0.0 {
            let next = rng.exp_interval(w.storm / 18.0).min(120.0);
            let seed = rng.next_u32() as u64;
            state.next_thunder = next;
            let ground = env.ground(p).map(|g| g.height).unwrap_or(p.y - 1.7);
            let (channel, current) = thunder::random_strike(p, ground, up, 300.0, 8_000.0, seed);
            let speed = audio.speed_of_sound();
            if let Some(cue) = thunder::play_thunder(&mut audio, &channel, current, speed, seed) {
                cues.write(cue);
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct ReverbState {
    sent: Option<(f32, f32, f32)>,
}

pub(crate) fn reverb_zones(
    audio: Res<Audio>,
    zones: Query<(&GlobalTransform, &ReverbZone)>,
    mut state: Local<ReverbState>,
) {
    if !audio.is_running() {
        return;
    }
    // Outdoors: a hint of early reflections from the ground and surroundings.
    let outdoor = (0.3f32, 0.6f32, 0.06f32);
    let p = audio.listener;
    let mut best: Option<(f32, &ReverbZone)> = None;
    for (t, z) in &zones {
        let local = t.rotation().inverse() * (p - t.translation()).as_vec3();
        let margin = (z.half_extents - local.abs()).min_element();
        if margin <= 0.0 {
            continue;
        }
        let w = smooth(0.0, z.fade_m.max(0.01), margin);
        if best.is_none_or(|(bw, _)| w > bw) {
            best = Some((w, z));
        }
    }
    let target = match best {
        Some((w, z)) => (
            outdoor.0 + (z.room - outdoor.0) * w,
            outdoor.1 + (z.damping - outdoor.1) * w,
            outdoor.2 + (z.wet - outdoor.2) * w,
        ),
        None => outdoor,
    };
    let changed = state.sent.is_none_or(|s| {
        (s.0 - target.0).abs() > 0.005
            || (s.1 - target.1).abs() > 0.005
            || (s.2 - target.2).abs() > 0.002
    });
    if changed {
        audio.send(Command::SetReverb {
            room: target.0,
            damping: target.1,
            wet: target.2,
        });
        state.sent = Some(target);
    }
}

pub(crate) fn follow_emitters(
    mut audio: ResMut<Audio>,
    transforms: Query<&GlobalTransform>,
    time: Res<Time>,
    mut scratch: Local<Vec<(VoiceId, DVec3, Vec3)>>,
) {
    let dt = time.real_delta_secs().max(1e-4) as f64;
    scratch.clear();
    for (id, e, last) in audio.followers() {
        if let Ok(t) = transforms.get(e) {
            let p = t.translation();
            let v = last
                .filter(|l| l.distance(p) < 50.0)
                .map(|l| ((p - l) / dt).as_vec3())
                .unwrap_or(Vec3::ZERO);
            scratch.push((id, p, v));
        }
    }
    for &(id, p, v) in scratch.iter() {
        audio.set_position(id, p, v);
        audio.set_last(id, p);
    }
}

/// The world moved to a new frame: take the ears and every sound along.
pub(crate) fn shift_world(
    mut audio: ResMut<Audio>,
    mut shifts: MessageReader<engine_world::WorldShift>,
) {
    let mut total: Option<engine_world::WorldShift> = None;
    for s in shifts.read() {
        total = Some(total.map_or(*s, |t| t.then(s)));
    }
    if let Some(shift) = total {
        audio.shift_world(&shift);
    }
}

pub(crate) fn drive_output(
    mut audio: ResMut<Audio>,
    time: Res<Time>,
    mut exits: MessageReader<AppExit>,
    diagnostics: Option<ResMut<Diagnostics>>,
) {
    audio.drive(time.real_elapsed_secs_f64());
    if exits.read().next().is_some()
        && audio.is_recording()
        && let Err(e) = audio.stop_recording()
    {
        engine_core::log::warn!("audio: {e}");
    }
    if let Some(mut d) = diagnostics {
        let m = audio.meters();
        d.set("audio.voices", audio.voice_count() as f64);
        d.set("audio.peak_db", to_db(m.master_peak) as f64);
    }
}

/// A sound due an occlusion check: voice, position, source entity, occlusion last sent.
type OcclusionCheck = (VoiceId, DVec3, Option<Entity>, f32);

/// Rays cast per frame for occlusion (3 per voice checked).
const OCCLUSION_RAYS_PER_FRAME: usize = 48;
/// How often each sound's line of sight is rechecked, real seconds.
const OCCLUSION_INTERVAL: f64 = 0.1;

/// Muffle sounds that something solid stands between: one ray straight to the sound and two
/// past it (over the top and round the side), so a sound just round a corner or over a low
/// wall is only partly muffled. Dense materials (stone, metal) block more than wood or cloth.
pub(crate) fn occlusion(
    mut audio: ResMut<Audio>,
    settings: Res<AudioSettings>,
    physics: Res<PhysicsQueries>,
    env: Env,
    time: Res<Time>,
    parents: Query<&ChildOf>,
    mut work: Local<Vec<OcclusionCheck>>,
) {
    if !audio.is_running() {
        return;
    }
    let now = time.real_elapsed_secs_f64();
    let listener = audio.listener;
    let enabled = settings.occlusion;
    work.clear();
    for (id, l) in audio.live.iter() {
        let Some(p) = l.last else { continue };
        if !l.bus.in_world() || (now < l.check_at && enabled) {
            continue;
        }
        if !enabled && l.occlusion == 0.0 {
            continue;
        }
        work.push((*id, p, l.source, l.occlusion));
        if work.len() * 3 >= OCCLUSION_RAYS_PER_FRAME {
            break;
        }
    }
    if work.is_empty() {
        return;
    }
    // The listener's own body (and whatever carries it) never blocks.
    let mut exclude: Vec<Entity> = Vec::new();
    let mut e = audio.listener_entity;
    while let Some(x) = e {
        exclude.push(x);
        e = parents.get(x).ok().map(|c| c.0);
        if exclude.len() > 8 {
            break;
        }
    }
    let up = env.up(listener);
    for &(id, p, source, last) in work.iter() {
        let amount = if enabled {
            let mut filter = QueryFilter {
                exclude: exclude.clone(),
                ..Default::default()
            };
            filter.exclude.extend(source);
            let target = p + (up * 0.3).as_dvec3();
            let to = (target - listener).as_vec3();
            let dist = to.length();
            if dist < 0.5 {
                0.0
            } else {
                let side = to.cross(up).normalize_or_zero() * (0.25 * dist).clamp(0.5, 3.0);
                let over = up * (0.15 * dist).clamp(0.5, 3.0);
                let mut amount = 0.0;
                for (weight, aim) in [(0.6, Vec3::ZERO), (0.2, over), (0.2, side)] {
                    let goal = target + aim.as_dvec3();
                    let d = (goal - listener).as_vec3();
                    let len = d.length();
                    if let Some(hit) = physics.raycast(listener, d / len, len, &filter)
                        && hit.distance > 0.2
                        && hit.distance < len - 0.25
                    {
                        let density = env.material(hit.surface).density;
                        amount += weight * (0.55 + 0.45 * (density / 2_400.0).min(1.0));
                    }
                }
                amount
            }
        } else {
            0.0
        };
        if let Some(l) = audio.live.get_mut(&id) {
            l.check_at = now + OCCLUSION_INTERVAL;
            l.occlusion = amount;
        }
        if (amount - last).abs() > 0.02 {
            audio.send(Command::SetOcclusion { id, amount });
        }
    }
}
