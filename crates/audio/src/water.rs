//! Water near the listener: waves lapping at the nearest shores and streams babbling. The
//! ground around the listener is sampled on a grid; water next to dry ground is shore, water
//! with a current is a stream. Each plays as a [`WaterSound`] at the nearest such place.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::Env;

use crate::ambience::WaterSound;
use crate::{Audio, AudioSettings, PlayOptions, VoiceId};

/// How far out the ground is searched for water, meters.
pub const WATER_SEARCH_M: f64 = 24.0;
/// Grid spacing of the search, meters.
const STEP_M: f64 = 2.0;
/// How often the search runs, seconds.
const SCAN_INTERVAL: f64 = 0.5;
/// Water slower than this is still (lapping only), m/s.
const FLOWING: f32 = 0.25;

#[derive(Default)]
pub(crate) struct WaterAmbience {
    /// Two shores (the nearest, and the nearest one well away from it) and the nearest stream.
    voices: [Slot; 3],
    next_scan: f64,
}

#[derive(Default, Clone, Copy)]
struct Slot {
    id: VoiceId,
    position: DVec3,
}

/// What the search found: shore points and flowing water points with their speed.
#[derive(Default)]
struct Found {
    shores: Vec<DVec3>,
    streams: Vec<(DVec3, f32)>,
}

pub(crate) fn water_ambience(
    mut audio: ResMut<Audio>,
    settings: Res<AudioSettings>,
    env: Env,
    time: Res<Time>,
    mut state: Local<WaterAmbience>,
) {
    let now = time.real_elapsed_secs_f64();
    if now < state.next_scan {
        return;
    }
    state.next_scan = now + SCAN_INTERVAL;
    if !audio.is_running() || !settings.water {
        for slot in &mut state.voices {
            if !slot.id.is_none() {
                audio.stop(slot.id, 1.5);
            }
            *slot = Slot::default();
        }
        return;
    }
    let listener = audio.listener_position();
    let found = search(&env, listener);
    let wind = env.wind(listener).length();
    // Small lake waves even on a calm day; more with wind.
    let waves = (0.18 + wind / 12.0).clamp(0.0, 1.0);

    let near = |points: &[DVec3], away_from: Option<DVec3>| {
        points
            .iter()
            .filter(|p| away_from.is_none_or(|a| a.distance(**p) > 10.0))
            .min_by(|a, b| a.distance(listener).total_cmp(&b.distance(listener)))
            .copied()
    };
    let first = near(&found.shores, None);
    let second = first.and_then(|f| near(&found.shores, Some(f)));
    let stream = found
        .streams
        .iter()
        .min_by(|a, b| a.0.distance(listener).total_cmp(&b.0.distance(listener)))
        .copied();

    let wanted = [
        first.map(|p| (p, waves, 0.0)),
        second.map(|p| (p, waves * 0.8, 0.0)),
        stream.map(|(p, v)| (p, 0.0, v)),
    ];
    let fs = audio.sample_rate();
    for (slot, want) in state.voices.iter_mut().zip(wanted) {
        match want {
            Some((p, waves, flow)) => {
                if audio.is_playing(slot.id) {
                    // Glide, so walking along a shore doesn't make the water jump.
                    let p = if slot.position.distance(p) < 15.0 {
                        slot.position.lerp(p, 0.35)
                    } else {
                        p
                    };
                    slot.position = p;
                    audio.set_position(slot.id, p, Vec3::ZERO);
                    audio.set_last(slot.id, p);
                } else {
                    let seed = audio.next_seed();
                    slot.id = audio.play(
                        WaterSound::new(fs, seed),
                        PlayOptions::at(p).range(2.0, 60.0).fade_in(2.0),
                    );
                    slot.position = p;
                }
                audio.set_param(slot.id, "waves", waves);
                audio.set_param(slot.id, "flow", flow);
            }
            None => {
                if !slot.id.is_none() {
                    audio.stop(slot.id, 2.0);
                }
                *slot = Slot::default();
            }
        }
    }
}

/// Sample the ground around `listener` for shores and flowing water.
fn search(env: &Env, listener: DVec3) -> Found {
    let n = (WATER_SEARCH_M / STEP_M) as i32;
    let size = (2 * n + 1) as usize;
    // Per grid point: the water surface there (if any is above the ground) and its current.
    let mut wet: Vec<Option<(f64, Vec3)>> = Vec::with_capacity(size * size);
    for z in -n..=n {
        for x in -n..=n {
            let q = listener + DVec3::new(x as f64 * STEP_M, 0.0, z as f64 * STEP_M);
            let ground = env.ground(q).map(|g| g.height);
            let water = env
                .water(q)
                .filter(|w| w.depth > 0.02 && ground.is_none_or(|g| w.surface_height > g + 0.01));
            wet.push(water.map(|w| (w.surface_height, w.velocity)));
        }
    }
    let at = |x: i32, z: i32| -> Option<(f64, Vec3)> {
        if x < -n || x > n || z < -n || z > n {
            return None;
        }
        wet[((z + n) as usize) * size + (x + n) as usize]
    };
    let mut found = Found::default();
    let any_dry = (0..size * size).any(|i| wet[i].is_none());
    for z in -n..=n {
        for x in -n..=n {
            let Some((height, velocity)) = at(x, z) else {
                continue;
            };
            let p = listener + DVec3::new(x as f64 * STEP_M, 0.0, z as f64 * STEP_M);
            let p = DVec3::new(p.x, height, p.z);
            let speed = velocity.length();
            if speed > FLOWING {
                found.streams.push((p, speed));
            }
            // Water next to dry ground (inside the searched area) is a shore.
            let edge = any_dry
                && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dz)| {
                    let (xx, zz) = (x + dx, z + dz);
                    xx >= -n && xx <= n && zz >= -n && zz <= n && at(xx, zz).is_none()
                });
            if edge {
                found.shores.push(p);
            }
        }
    }
    found
}
