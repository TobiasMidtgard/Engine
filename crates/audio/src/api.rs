//! JSON functions for the console, scripts and AI agents (`audio.*`).

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::{ContactKind, SurfaceMaterials};
use serde_json::{Value, json};

use crate::effects::{ExplosionSound, ToneSound, WhizSound, WhooshSound};
use crate::footstep::{FootstepSound, Step};
use crate::{Audio, FootstepSurfaces, PlayOptions, dsp::to_db, thunder};

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "audio.status",
        "Audio output (device name, offline or off), sample rate, voices playing, level meters, \
         listener position, whether a recording is running, and the recorded footstep takes \
         per surface (empty: every step is synthesized).",
        "{}",
        |world, _| {
            let takes = world.resource::<crate::FootstepSamples>().summary();
            let audio = world.resource::<Audio>();
            let m = audio.meters();
            let p = audio.listener_position();
            Ok(json!({
                "output": audio.status(),
                "sample_rate": audio.sample_rate(),
                "voices": audio.voice_count(),
                "peak_db": to_db(m.master_peak),
                "rms_db": to_db(m.master_rms),
                "listener": [p.x, p.y, p.z],
                "recording": audio.is_recording(),
                "footstep_takes": takes,
            }))
        },
    )
    .register_api(
        "audio.surfaces",
        "Every surface material and how its footsteps sound (layer weights). Change one with \
         the resource API on FootstepSurfaces.",
        "{}",
        |world, _| {
            let materials = world.resource::<SurfaceMaterials>();
            let surfaces = world.resource::<FootstepSurfaces>();
            let list: Vec<Value> = materials
                .materials
                .iter()
                .map(|m| {
                    json!({
                        "name": m.name,
                        "custom": surfaces.surfaces.contains_key(&m.name),
                        "sound": serde_json::to_value(surfaces.get(m)).unwrap_or(Value::Null),
                    })
                })
                .collect();
            Ok(json!({ "surfaces": list }))
        },
    )
    .register_api(
        "audio.play",
        "Play a test sound. sound: footstep (surface, kind foot|paw|hoof|wheel|body, load_kg, \
         speed), explosion (energy_j), thunder (a lightning strike: at [x,y,z] on the ground, \
         or distance_m away in a random direction; peak_current_ka), whiz (speed, miss_m), \
         whoosh (secs, strength) or tone (freq, secs). at: [x,y,z] in the world, or leave it \
         out to play at the listener. gain_db adjusts the level.",
        r#"{"sound": "footstep", "surface": "mud", "at": [0, 0, 2], "load_kg": 75, "speed": 1.4}"#,
        play,
    )
    .register_api(
        "audio.record",
        "Record what the listener hears to a WAV file: {\"path\": \"out.wav\", \"seconds\": 20}. \
         {\"stop\": true} ends it early and writes the file.",
        r#"{"path": "screenshots/trail.wav", "seconds": 20}"#,
        |world, args| {
            let mut audio = world.resource_mut::<Audio>();
            if args["stop"].as_bool() == Some(true) {
                let (path, secs) = audio.stop_recording()?;
                return Ok(json!({ "path": path.display().to_string(), "seconds": secs }));
            }
            let path = args["path"]
                .as_str()
                .ok_or("Give the file to write as \"path\", like \"out.wav\".")?;
            let secs = args["seconds"].as_f64().unwrap_or(30.0) as f32;
            audio.start_recording(path, secs)?;
            Ok(json!({ "recording": path, "seconds": secs }))
        },
    )
    .register_api(
        "audio.stop_all",
        "Fade out every sound that is playing (rain and wind come back while the weather lasts).",
        r#"{"fade": 0.2}"#,
        |world, args| {
            let fade = args["fade"].as_f64().unwrap_or(0.1) as f32;
            world.resource::<Audio>().stop_all(fade);
            Ok(json!({ "ok": true }))
        },
    );
}

fn num(args: &Value, key: &str, default: f32) -> f32 {
    args[key].as_f64().map(|v| v as f32).unwrap_or(default)
}

fn play(world: &mut World, args: Value) -> Result<Value, String> {
    let kind = args["sound"].as_str().unwrap_or("footstep").to_string();
    let at = match &args["at"] {
        Value::Null => None,
        Value::Array(a) if a.len() == 3 => {
            let c: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            if c.len() != 3 {
                return Err("\"at\" needs three numbers, like [0, 0, 2].".into());
            }
            Some(DVec3::new(c[0], c[1], c[2]))
        }
        _ => return Err("\"at\" needs three numbers, like [0, 0, 2].".into()),
    };
    let gain_db = num(&args, "gain_db", 0.0);
    if kind == "thunder" {
        return play_thunder(world, &args, at);
    }
    let mut name = kind.clone();
    let sound: Box<dyn crate::Sound> = {
        let (fs, seed) = {
            let mut audio = world.resource_mut::<Audio>();
            (audio.sample_rate(), audio.next_seed())
        };
        match kind.as_str() {
            "footstep" | "step" | "impact" => {
                let surface = args["surface"].as_str().unwrap_or("default");
                let materials = world.resource::<SurfaceMaterials>();
                let material = materials.get(materials.id_or_default(surface)).clone();
                let sound = world.resource::<FootstepSurfaces>().get(&material);
                name = format!("{kind}.{}", material.name);
                let contact = match args["kind"].as_str().unwrap_or("foot") {
                    "paw" => ContactKind::Paw,
                    "hoof" => ContactKind::Hoof,
                    "wheel" => ContactKind::Wheel,
                    "track" => ContactKind::Track,
                    "body" => ContactKind::Body,
                    _ => ContactKind::Foot,
                };
                let step = if kind == "impact" {
                    Step::impact(sound, num(&args, "strength", 1.0), seed)
                } else {
                    Step::from_contact(
                        sound,
                        contact,
                        num(&args, "load_kg", 75.0),
                        num(&args, "speed", 1.4),
                        seed,
                    )
                };
                Box::new(FootstepSound::new(fs, &step))
            }
            "explosion" => Box::new(ExplosionSound::new(fs, num(&args, "energy_j", 1e6), seed)),
            "whiz" => Box::new(WhizSound::new(
                fs,
                num(&args, "speed", 400.0),
                num(&args, "miss_m", 1.0),
                seed,
            )),
            "whoosh" => Box::new(WhooshSound::new(
                fs,
                num(&args, "secs", 0.35),
                num(&args, "strength", 1.0),
                seed,
            )),
            "tone" => Box::new(ToneSound::new(
                fs,
                num(&args, "freq", 440.0),
                num(&args, "secs", 0.5),
                0.3,
            )),
            other => {
                return Err(format!(
                    "There is no test sound called \"{other}\". Try footstep, impact, explosion, \
                     thunder, whiz, whoosh or tone."
                ));
            }
        }
    };
    let mut audio = world.resource_mut::<Audio>();
    if !audio.is_running() && audio.status() != "not started" {
        return Err(format!("Audio is {}, so nothing can play.", audio.status()));
    }
    let opts = match at {
        Some(p) => PlayOptions::at(p).range(1.5, 10_000.0),
        None => PlayOptions::ui(),
    }
    .gain_db(gain_db);
    let voice = audio.play_boxed(sound, opts);
    if voice.is_none() {
        return Err("The sound was skipped: it is out of earshot or too many are playing.".into());
    }
    Ok(json!({ "playing": name, "voice": voice.0 }))
}

/// `audio.play` for thunder: a lightning bolt down onto `at` (or `distance_m` away).
fn play_thunder(world: &mut World, args: &Value, at: Option<DVec3>) -> Result<Value, String> {
    let mut audio = world.resource_mut::<Audio>();
    if !audio.is_running() {
        return Err(format!("Audio is {}, so nothing can play.", audio.status()));
    }
    let speed_of_sound = audio.speed_of_sound();
    let seed = audio.next_seed();
    let listener = audio.listener_position();
    let current = num(args, "peak_current_ka", 30.0);
    let channel = match at {
        Some(ground) => thunder::random_bolt(ground + DVec3::new(0.0, 2_500.0, 0.0), ground, seed),
        None => {
            let d = num(args, "distance_m", 1_500.0).max(50.0);
            thunder::random_strike(listener, listener.y - 1.7, Vec3::Y, d, d, seed).0
        }
    };
    let first = channel
        .iter()
        .map(|p| p.distance(listener))
        .fold(f64::MAX, f64::min);
    match thunder::play_thunder(&mut audio, &channel, current, speed_of_sound, seed) {
        Some(cue) => Ok(json!({
            "playing": "thunder",
            "voice": cue.voice.0,
            "arrives_in_s": first / speed_of_sound.max(1.0) as f64,
        })),
        None => Err("The thunder was skipped: too many sounds are playing.".into()),
    }
}
