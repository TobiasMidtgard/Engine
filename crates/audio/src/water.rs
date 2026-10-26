//! Water near the listener: waves lapping at the nearest shores and streams babbling (the
//! ground around the listener is sampled on a grid; water next to dry ground is shore, water
//! with a current is a stream), and splashes. A splash is one sound, but a source that keeps
//! splashing (a waterfall's foot, a speedboat's bow) becomes one sustained roar that follows
//! how much water comes down.

use std::collections::HashMap;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::{Env, WaterSplash, WorldShift};

use crate::ambience::WaterSound;
use crate::footstep::{FootstepSound, FootstepSurfaces, Step, SurfaceSound};
use crate::systems::clock_for;
use crate::{Audio, AudioCue, AudioSettings, PlayOptions, VoiceId};

/// Splashes from one source closer together than this are one ongoing spray, seconds.
const SPRAY_GAP_S: f64 = 0.6;
/// Splashes in a row before a source counts as ongoing spray.
const SPRAY_AFTER: u32 = 3;

#[derive(Default)]
pub(crate) struct SplashStreams {
    by_source: HashMap<Entity, Spray>,
}

struct Spray {
    voice: VoiceId,
    last: f64,
    count: u32,
    /// kg/s, smoothed.
    rate: f32,
    position: DVec3,
}

/// Play splashes: one sound each, or a sustained roar for a source that keeps splashing.
pub(crate) fn play_splashes(
    mut audio: ResMut<Audio>,
    mut splashes: MessageReader<WaterSplash>,
    scales: Query<&LocalTimeScale>,
    surfaces: Res<FootstepSurfaces>,
    time: Res<Time>,
    mut cues: MessageWriter<AudioCue>,
    mut streams: Local<SplashStreams>,
) {
    if !audio.is_running() {
        splashes.clear();
        return;
    }
    let now = time.elapsed_secs_f64();
    let fs = audio.sample_rate();
    let listener = audio.listener_position();
    let water = surfaces
        .surfaces
        .get("water")
        .copied()
        .or_else(|| SurfaceSound::preset("water"))
        .unwrap_or_default();
    let mut discrete = 0;
    for s in splashes.read() {
        if s.position.distance(listener) > 100.0 {
            continue;
        }
        // A source that keeps splashing: one ongoing roar.
        if let Some(source) = s.source {
            let spray = streams.by_source.entry(source).or_insert(Spray {
                voice: VoiceId::NONE,
                last: f64::NEG_INFINITY,
                count: 0,
                rate: 0.0,
                position: s.position,
            });
            let gap = now - spray.last;
            if gap < SPRAY_GAP_S {
                spray.count += 1;
                let rate = s.mass_kg.max(0.0) / gap.max(0.02) as f32;
                spray.rate += (rate - spray.rate) * 0.3;
                spray.position = spray.position.lerp(s.position, 0.3);
            } else {
                spray.count = 1;
                spray.rate = 0.0;
                spray.position = s.position;
            }
            spray.last = now;
            if spray.count >= SPRAY_AFTER {
                if !audio.is_playing(spray.voice) {
                    let seed = audio.next_seed();
                    let mut sound = WaterSound::new(fs, seed);
                    crate::Sound::set_param(&mut sound, "spray", spray.rate);
                    spray.voice = audio.play(
                        sound,
                        PlayOptions::at(spray.position)
                            .range(2.0 + s.radius, 120.0)
                            .clock(clock_for(Some(source), &scales))
                            .source(Some(source))
                            .fade_in(0.8)
                            .reverb(0.3),
                    );
                    if !spray.voice.is_none() {
                        cues.write(AudioCue {
                            voice: spray.voice,
                            position: Some(spray.position),
                            name: "spray.water".into(),
                            loudness: (0.1 * spray.rate.powf(0.4)).min(1.0),
                        });
                    }
                } else {
                    audio.set_param(spray.voice, "spray", spray.rate);
                    audio.set_position(spray.voice, spray.position, Vec3::ZERO);
                    audio.set_last(spray.voice, spray.position);
                }
                continue;
            }
        }
        // One splash.
        if discrete >= 8 {
            continue;
        }
        let strength = (s.mass_kg.max(0.0) / 4.0).sqrt() * (1.0 + 0.1 * s.velocity.length());
        if strength < 0.05 {
            continue;
        }
        discrete += 1;
        let step = Step::impact(water, strength.min(4.0), audio.next_seed());
        let voice = audio.play(
            FootstepSound::new(fs, &step),
            PlayOptions::at(s.position)
                .range(1.0 + s.radius, 100.0)
                .clock(clock_for(s.source, &scales))
                .source(s.source)
                .reverb(0.35),
        );
        if !voice.is_none() {
            cues.write(AudioCue {
                voice,
                position: Some(s.position),
                name: "splash.water".into(),
                loudness: (strength * 0.4).min(1.0),
            });
        }
    }
    // Sprays that stopped fade out.
    streams.by_source.retain(|_, spray| {
        let alive = now - spray.last < SPRAY_GAP_S * 2.0;
        if !alive && !spray.voice.is_none() {
            audio.stop(spray.voice, 1.5);
        }
        alive
    });
}

/// How far out the ground is searched for water, meters.
pub const WATER_SEARCH_M: f64 = 24.0;
/// Grid spacing of the search, meters. The grid is fixed to the world, so moving keeps the
/// cells already sampled.
const STEP_M: f64 = 2.0;
/// How often the voices follow what the search found, seconds.
const SCAN_INTERVAL: f64 = 0.5;
/// Time the search may take per frame, seconds. Each sample asks the ground and the water,
/// which can cost a tenth of a millisecond on a planet, so the search is spread over frames:
/// the nearest unknown cells first, then the oldest ones again. At least one cell is sampled
/// per frame.
const BUDGET_S: f64 = 0.0005;
/// Cells sampled per frame in an offline render, which must not depend on the wall clock.
const OFFLINE_SAMPLES: usize = 8;
/// A sampled cell is sampled again after this long (rain fills, tides move), seconds.
const RESAMPLE_S: f64 = 8.0;
/// Water slower than this is still (lapping only), m/s.
const FLOWING: f32 = 0.25;

#[derive(Default)]
pub(crate) struct WaterAmbience {
    /// Two shores (the nearest, and the nearest one well away from it) and the nearest stream.
    voices: [Slot; 3],
    next_scan: f64,
    /// Sampled grid cells around the listener.
    cells: HashMap<(i64, i64), Cell>,
}

#[derive(Default, Clone, Copy)]
struct Slot {
    id: VoiceId,
    position: DVec3,
}

/// One sampled grid cell: the water surface there (if any is above the ground) and its
/// current, and when it was sampled.
#[derive(Clone, Copy)]
struct Cell {
    wet: Option<(f64, Vec3)>,
    sampled: f64,
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
    mut shifts: MessageReader<WorldShift>,
    mut state: Local<WaterAmbience>,
) {
    let now = time.real_elapsed_secs_f64();
    let state = &mut *state;
    // The world moved under the grid: forget it, and keep the voices where the water is.
    let mut shift: Option<WorldShift> = None;
    for s in shifts.read() {
        shift = Some(shift.map_or(*s, |t| t.then(s)));
    }
    if let Some(shift) = shift {
        state.cells.clear();
        for slot in &mut state.voices {
            slot.position = shift.point(slot.position);
        }
    }
    if !audio.is_running() || !settings.water {
        for slot in &mut state.voices {
            if !slot.id.is_none() {
                audio.stop(slot.id, 1.5);
            }
            *slot = Slot::default();
        }
        state.cells.clear();
        return;
    }
    let listener = audio.listener_position();
    let budget = if audio.is_offline() {
        Budget::Samples(OFFLINE_SAMPLES)
    } else {
        Budget::Time(BUDGET_S)
    };
    sample_cells(&env, listener, now, budget, &mut state.cells);
    if now < state.next_scan {
        return;
    }
    state.next_scan = now + SCAN_INTERVAL;
    let found = search(&state.cells, listener);
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

/// Cells searched each way from the listener's cell.
fn reach() -> i64 {
    (WATER_SEARCH_M / STEP_M) as i64
}

fn cell_of(p: DVec3) -> (i64, i64) {
    ((p.x / STEP_M).round() as i64, (p.z / STEP_M).round() as i64)
}

fn cell_point(c: (i64, i64), y: f64) -> DVec3 {
    DVec3::new(c.0 as f64 * STEP_M, y, c.1 as f64 * STEP_M)
}

/// How much of the search one frame may do.
#[derive(Clone, Copy)]
enum Budget {
    Time(f64),
    Samples(usize),
}

/// Sample cells around `listener` within `budget`: unknown ones nearest first, then ones
/// sampled longest ago. Cells far outside the search are dropped.
fn sample_cells(
    env: &Env,
    listener: DVec3,
    now: f64,
    budget: Budget,
    cells: &mut HashMap<(i64, i64), Cell>,
) {
    let n = reach();
    let c = cell_of(listener);
    cells.retain(|k, _| (k.0 - c.0).abs() <= n + 4 && (k.1 - c.1).abs() <= n + 4);
    let mut unknown = Vec::new();
    let mut oldest: Vec<((i64, i64), f64)> = Vec::new();
    for z in -n..=n {
        for x in -n..=n {
            let k = (c.0 + x, c.1 + z);
            match cells.get(&k) {
                None => unknown.push((x * x + z * z, k)),
                Some(cell) if now - cell.sampled > RESAMPLE_S => oldest.push((k, cell.sampled)),
                Some(_) => {}
            }
        }
    }
    unknown.sort_unstable();
    oldest.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
    let todo = unknown
        .into_iter()
        .map(|(_, k)| k)
        .chain(oldest.into_iter().map(|(k, _)| k));
    let start = std::time::Instant::now();
    for (i, k) in todo.enumerate() {
        let spent = match budget {
            Budget::Time(s) => i > 0 && start.elapsed().as_secs_f64() > s,
            Budget::Samples(n) => i >= n,
        };
        if spent {
            break;
        }
        let q = cell_point(k, listener.y);
        let ground = env.ground(q).map(|g| g.height);
        let water = env
            .water(q)
            .filter(|w| w.depth > 0.02 && ground.is_none_or(|g| w.surface_height > g + 0.01));
        cells.insert(
            k,
            Cell {
                wet: water.map(|w| (w.surface_height, w.velocity)),
                sampled: now,
            },
        );
    }
}

/// Shores and flowing water among the sampled cells around `listener`. Cells not sampled
/// yet count as neither wet nor dry.
fn search(cells: &HashMap<(i64, i64), Cell>, listener: DVec3) -> Found {
    let n = reach();
    let c = cell_of(listener);
    let inside = |k: (i64, i64)| (k.0 - c.0).abs() <= n && (k.1 - c.1).abs() <= n;
    let mut found = Found::default();
    for z in -n..=n {
        for x in -n..=n {
            let k = (c.0 + x, c.1 + z);
            let Some(Cell {
                wet: Some((height, velocity)),
                ..
            }) = cells.get(&k).copied()
            else {
                continue;
            };
            let p = cell_point(k, height);
            let speed = velocity.length();
            if speed > FLOWING {
                found.streams.push((p, speed));
            }
            // Water next to dry ground (inside the searched area) is a shore.
            let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dz)| {
                let kk = (k.0 + dx, k.1 + dz);
                inside(kk) && cells.get(&kk).is_some_and(|cell| cell.wet.is_none())
            });
            if edge {
                found.shores.push(p);
            }
        }
    }
    found
}
