//! Recorded footsteps: real steps cut from field recordings, one bank of takes per surface.
//!
//! The recordings are CC0 and CC-BY Freesound previews listed in
//! `assets/manifests/audio.toml` and fetched to `assets/fetched/audio/footsteps/<surface>/`
//! (`cargo run -p fetch-assets -- audio`). Each file holds a run of steps; loading cuts it
//! into one take per step (where the envelope jumps), drops the ones that are not a clean step
//! (a jump, a scrape, a step much quieter than its neighbours), labels each one walk or run,
//! and normalizes every take to the same loudness.
//!
//! A footstep then plays one take, picked at random but never one of the last few on that
//! surface, slightly re-pitched and re-levelled, brighter for heavier and faster steps, and
//! duller and quieter when the ground's state (rain, a puddle, frost) has changed what the
//! recording sounds like. The synthesized layers of [`crate::footstep`] still play on top,
//! but only what the recording lacks: the squelch of wet ground, a puddle's splash, a heavy
//! walker's thump, a snow crust's squeak. A surface with no takes, or a missing folder, falls
//! back to the synthesis alone, so nothing needs the fetched files (CI never fetches them).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, bounded};
use engine_core::prelude::*;
use engine_world::{ContactKind, SurfaceCell};

use crate::dsp::{Biquad, Noise, db, hermite};
use crate::footstep::{FootstepSurfaces, Step, SurfaceSound, smooth};

/// Folder under the asset root that holds one subfolder of recordings per surface material
/// (`<root>/fetched/audio/footsteps/gravel/*.ogg`).
pub const SAMPLE_DIR: &str = "fetched/audio/footsteps";

/// How a take was walked, so running steps pick running takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gait {
    Walk,
    Run,
}

/// One recorded step, ready to play.
#[derive(Debug, Clone)]
pub struct Take {
    /// Mono samples at [`Take::sample_rate`], scaled to unit [`loudness`]; the step's onset
    /// is [`ONSET_PAD_S`] after the start.
    pub pcm: Arc<[f32]>,
    pub sample_rate: f32,
    pub gait: Gait,
    /// Which recording it came from (the file name without extension).
    pub source: Arc<str>,
}

/// Silence kept before each take's onset, s.
pub const ONSET_PAD_S: f32 = 0.004;

/// The takes of one surface, and the last ones played (so a take never repeats right away).
#[derive(Debug, Clone, Default)]
pub struct TakeSet {
    pub takes: Vec<Take>,
    recent: Vec<usize>,
}

impl TakeSet {
    pub fn new(takes: Vec<Take>) -> Self {
        Self {
            takes,
            recent: Vec::new(),
        }
    }

    /// How many of the last picks to avoid: none with one take, the last with two, up to the
    /// last four with plenty.
    fn avoid(n: usize) -> usize {
        (n / 3).clamp(1, 4).min(n.saturating_sub(1))
    }

    /// Pick a take: `run` (0..1) is the chance it is a running take (when there are any),
    /// and none of the last few picks comes back. Deterministic for a given `rng` state and
    /// pick history.
    pub fn pick(&mut self, run: f32, rng: &mut Noise) -> Option<&Take> {
        if self.takes.is_empty() {
            return None;
        }
        let want = if rng.chance(run.clamp(0.0, 1.0)) {
            Gait::Run
        } else {
            Gait::Walk
        };
        let mut pool: Vec<usize> = (0..self.takes.len())
            .filter(|&i| self.takes[i].gait == want)
            .collect();
        if pool.is_empty() {
            pool = (0..self.takes.len()).collect();
        }
        let avoid = Self::avoid(pool.len());
        let recent: Vec<usize> = self
            .recent
            .iter()
            .rev()
            .filter(|i| pool.contains(i))
            .take(avoid)
            .copied()
            .collect();
        pool.retain(|i| !recent.contains(i));
        let i = pool[((rng.uniform() * pool.len() as f32) as usize).min(pool.len() - 1)];
        self.recent.push(i);
        if self.recent.len() > 8 {
            self.recent.remove(0);
        }
        Some(&self.takes[i])
    }
}

/// Every surface's takes, keyed by surface material name.
#[derive(Debug, Clone, Default)]
pub struct SampleBank {
    pub surfaces: BTreeMap<String, TakeSet>,
}

impl SampleBank {
    /// Load every recording under `dir` (`<dir>/<surface>/*.ogg|*.wav`). A missing or empty
    /// folder gives an empty bank; a file that doesn't decode is skipped with a warning.
    pub fn load(dir: &Path) -> Self {
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Self::default();
        };
        for surface in entries.flatten() {
            let Ok(inner) = std::fs::read_dir(surface.path()) else {
                continue;
            };
            let name = surface.file_name().to_string_lossy().into_owned();
            for f in inner.flatten() {
                let p = f.path();
                if matches!(p.extension().and_then(|e| e.to_str()), Some("ogg" | "wav")) {
                    files.push((name.clone(), p));
                }
            }
        }
        files.sort();
        // Decode on a few threads (a minute of Vorbis takes a while in a debug build).
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4);
        let chunks: Vec<&[(String, PathBuf)]> =
            files.chunks(files.len().div_ceil(workers).max(1)).collect();
        let cut: Vec<Vec<(String, Vec<Take>)>> = std::thread::scope(|s| {
            let handles: Vec<_> = chunks
                .iter()
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter()
                            .filter_map(|(surface, path)| match decode(path) {
                                Ok((x, sr)) => {
                                    let stem = path
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    Some((surface.clone(), takes_from_recording(&stem, &x, sr)))
                                }
                                Err(e) => {
                                    engine_core::log::warn!(
                                        "audio: skipping footstep recording {}: {e}",
                                        path.display()
                                    );
                                    None
                                }
                            })
                            .collect()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect()
        });
        let mut bank = Self::default();
        for (surface, takes) in cut.into_iter().flatten() {
            bank.surfaces
                .entry(surface)
                .or_default()
                .takes
                .extend(takes);
        }
        bank.surfaces.retain(|_, t| !t.takes.is_empty());
        for set in bank.surfaces.values_mut() {
            match_sources(&mut set.takes);
        }
        bank
    }

    /// Takes for a surface material name (or the surface it borrows them from, like `tile`
    /// from `concrete`).
    pub fn takes(&self, surface: &str) -> Option<&TakeSet> {
        self.surfaces
            .get(surface)
            .or_else(|| self.surfaces.get(alias(surface)?))
    }

    fn takes_mut(&mut self, surface: &str) -> Option<&mut TakeSet> {
        let key = if self.surfaces.contains_key(surface) {
            surface
        } else {
            alias(surface)?
        };
        self.surfaces.get_mut(key)
    }

    /// Total takes over all surfaces.
    pub fn len(&self) -> usize {
        self.surfaces.values().map(|t| t.takes.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Bring the recordings of one surface halfway to a common tone, so that consecutive steps
/// from different recordists (and microphones) sound like the same ground: each recording's
/// takes get a tilt (a high shelf at 1.2 kHz, at most ±6 dB) from its median spectral
/// centroid toward the surface's, then are levelled again.
pub fn match_sources(takes: &mut [Take]) {
    let mut by_source: BTreeMap<Arc<str>, Vec<f32>> = BTreeMap::new();
    for t in takes.iter() {
        by_source
            .entry(t.source.clone())
            .or_default()
            .push(centroid(&t.pcm, t.sample_rate).max(20.0).log2());
    }
    if by_source.len() < 2 {
        return;
    }
    let median = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let mut centroids: BTreeMap<Arc<str>, (f32, usize)> = BTreeMap::new();
    for (k, v) in &mut by_source {
        let n = v.len();
        centroids.insert(k.clone(), (median(v), n));
    }
    let total: usize = centroids.values().map(|c| c.1).sum();
    let target = centroids.values().map(|(c, n)| c * *n as f32).sum::<f32>() / total as f32;
    for t in takes.iter_mut() {
        let octaves = target - centroids[&t.source].0;
        let gain_db = (3.0 * octaves).clamp(-6.0, 6.0);
        if gain_db.abs() < 0.5 {
            continue;
        }
        let mut shelf = Biquad::high_shelf(t.sample_rate, 1_200.0, gain_db);
        let mut pcm: Vec<f32> = t.pcm.iter().map(|&v| shelf.process(v)).collect();
        if normalize(&mut pcm, t.sample_rate) {
            t.pcm = pcm.into();
        }
    }
}

/// Energy-weighted mean frequency of a take, Hz (from the spectrum of its first 0.5 s).
fn centroid(x: &[f32], sr: f32) -> f32 {
    // Band energies from a bank of band-passes, an octave apart: cheap and good enough.
    let n = x.len().min((0.5 * sr) as usize);
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    let mut f = 62.5f32;
    while f < sr * 0.45 {
        let mut bp = Biquad::bandpass(sr, f, 1.41);
        let e: f64 = x[..n]
            .iter()
            .map(|&v| {
                let y = bp.process(v) as f64;
                y * y
            })
            .sum();
        num += e * f as f64;
        den += e;
        f *= 2.0;
    }
    if den <= 0.0 { 0.0 } else { (num / den) as f32 }
}

/// Surfaces that sound close enough to borrow another's recordings.
fn alias(surface: &str) -> Option<&'static str> {
    Some(match surface {
        "asphalt" | "tile" | "rock" | "stone" => "concrete",
        "planks" => "wood",
        _ => return None,
    })
}

// ---------------------------------------------------------------- decoding

/// Decode an Ogg Vorbis or WAV file to mono samples and their rate.
pub fn decode(path: &Path) -> Result<(Vec<f32>, f32), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("wav") => {
            let w = crate::wav::parse_wav(&bytes)?;
            Ok((
                downmix(&w.samples, w.channels as usize),
                w.sample_rate as f32,
            ))
        }
        _ => decode_vorbis(&bytes),
    }
}

fn decode_vorbis(bytes: &[u8]) -> Result<(Vec<f32>, f32), String> {
    let mut r = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("not Ogg Vorbis ({e})"))?;
    let channels = r.ident_hdr.audio_channels.max(1) as usize;
    let sr = r.ident_hdr.audio_sample_rate as f32;
    let mut out = Vec::new();
    loop {
        match r.read_dec_packet_itl() {
            Ok(Some(p)) => {
                out.extend(p.chunks(channels).map(|f| {
                    f.iter().map(|&v| v as f32).sum::<f32>() / (32_768.0 * channels as f32)
                }))
            }
            Ok(None) => break,
            Err(e) => return Err(format!("broken Ogg Vorbis ({e})")),
        }
    }
    Ok((out, sr))
}

fn downmix(x: &[f32], channels: usize) -> Vec<f32> {
    let c = channels.max(1);
    x.chunks(c)
        .map(|f| f.iter().sum::<f32>() / c as f32)
        .collect()
}

// ---------------------------------------------------------------- cutting takes

/// What a stretch of a recording holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Walk,
    Run,
    /// Steps of either kind: told apart by how close together they are.
    Steps,
}

/// The parts of each known recording worth cutting (seconds), by file name. Other stretches
/// (jumps and landings, scrapes, barefoot) are left out. Recordings not listed are cut whole
/// and their steps labelled by spacing.
/// Stretches of one recording: start and end (s) and what they hold.
type Parts = &'static [(f32, f32, Part)];

const PARTS: &[(&str, Parts)] = &[
    // Nox_Sound's mountain-boot sequences: walk, run, then jumps, scrapes and scuffs.
    (
        "nox_sound_556042",
        &[(0.0, 20.3, Part::Walk), (20.5, 30.6, Part::Run)],
    ),
    (
        "nox_sound_556002",
        &[(0.0, 12.8, Part::Walk), (13.3, 18.3, Part::Run)],
    ),
    (
        "nox_sound_564893",
        &[(0.0, 26.4, Part::Walk), (26.7, 35.2, Part::Run)],
    ),
    (
        "nox_sound_548384",
        &[(0.0, 5.4, Part::Walk), (5.6, 10.3, Part::Run)],
    ),
    (
        "nox_sound_gritty_530589",
        &[(0.0, 5.5, Part::Walk), (6.0, 11.5, Part::Run)],
    ),
    // Wood in seven kinds of shoes, three steps each: keep the shod ones.
    (
        "nox_sound_543685",
        &[
            (0.0, 2.7, Part::Walk),
            (6.5, 16.6, Part::Walk),
            (20.9, 24.6, Part::Walk),
        ],
    ),
    // Vrymaa's walk / jog / run takes.
    (
        "vrymaa_moss_770095",
        &[(0.0, 15.2, Part::Walk), (15.3, 36.0, Part::Run)],
    ),
    (
        "vrymaa_770097",
        &[(0.0, 17.2, Part::Walk), (17.3, 29.8, Part::Run)],
    ),
    (
        "vrymaa_rock_770084",
        &[(0.0, 22.6, Part::Walk), (22.8, 36.4, Part::Run)],
    ),
    // Shuffling through a deep drift of leaves in the middle.
    (
        "splicesound_218305",
        &[(0.0, 61.5, Part::Walk), (69.0, 100.0, Part::Walk)],
    ),
    // A jump into the puddle first.
    ("launemax_249911", &[(2.9, 9.7, Part::Walk)]),
    // Squishy wet gravel first, then the puddle.
    ("patchytherat_531571", &[(6.0, 16.1, Part::Walk)]),
    // Walks whose steps have quiet halves (heel, then a louder toe or crunch), which would
    // otherwise look like a quick run.
    ("inspectorj_338265", &[(0.0, 61.0, Part::Walk)]),
    ("vlatkoblazek_260979", &[(0.0, 13.3, Part::Walk)]),
    ("nox_sound_613849", &[(0.0, 11.3, Part::Walk)]),
    ("vrymaa_773736", &[(0.0, 68.7, Part::Walk)]),
    ("vrymaa_825854", &[(0.0, 38.2, Part::Walk)]),
    ("softdistortionfx_398937", &[(0.0, 91.1, Part::Walk)]),
    ("sturmankin_272253", &[(0.0, 30.2, Part::Walk)]),
];

/// Most takes kept per recording and gait: the ones closest to the recording's typical step.
const MAX_TAKES: usize = 24;

/// Highest peak a take may have over its loudness (dB) before its transient is limited:
/// about the synthesized steps' crest, so recorded steps stay as peak-safe as they were.
pub const MAX_CREST_DB: f32 = 22.0;

/// Cut a recording into takes (see the [module docs](self)).
pub fn takes_from_recording(name: &str, x: &[f32], sr: f32) -> Vec<Take> {
    let parts: Vec<(f32, f32, Part)> = PARTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| p.to_vec())
        .unwrap_or_else(|| vec![(0.0, f32::MAX, Part::Steps)]);
    let source: Arc<str> = name.into();
    // Rumble and handling noise below the lowest footfall thud.
    let mut hp = Biquad::highpass(sr, 50.0, 0.7);
    let x: Vec<f32> = x.iter().map(|&v| hp.process(v)).collect();
    let x = &x[..];
    let mut cuts = Vec::new();
    for (a, b, part) in parts {
        let a = ((a * sr) as usize).min(x.len());
        let b = ((b * sr) as usize).min(x.len());
        if b > a {
            cuts.extend(cut_steps(&x[a..b], sr, part).into_iter().map(|mut c| {
                c.range = a + c.range.start..a + c.range.end;
                c
            }));
        }
    }
    // Keep the most typical steps of each gait when a recording has plenty.
    let mut out = Vec::new();
    for gait in [Gait::Walk, Gait::Run] {
        let mut of: Vec<&Cut> = cuts.iter().filter(|c| c.gait == gait).collect();
        if of.len() > MAX_TAKES {
            let mut peaks: Vec<f32> = of.iter().map(|c| c.peak_db).collect();
            peaks.sort_by(f32::total_cmp);
            let median = peaks[peaks.len() / 2];
            of.sort_by(|p, q| {
                (p.peak_db - median)
                    .abs()
                    .total_cmp(&(q.peak_db - median).abs())
            });
            of.truncate(MAX_TAKES);
            of.sort_by_key(|c| c.range.start);
        }
        for c in of {
            let mut pcm = x[c.range.clone()].to_vec();
            expand(&mut pcm, c.floor_db, sr);
            let fade_in = ((0.002 * sr) as usize).min(pcm.len());
            for (j, v) in pcm.iter_mut().take(fade_in).enumerate() {
                *v *= j as f32 / fade_in as f32;
            }
            let fade_out = ((0.03 * sr) as usize).min(pcm.len() / 4).max(1);
            let n = pcm.len();
            for j in 0..fade_out {
                let g = 0.5 - 0.5 * (std::f32::consts::PI * j as f32 / fade_out as f32).cos();
                pcm[n - 1 - j] *= g;
            }
            if !normalize(&mut pcm, sr) {
                continue;
            }
            out.push(Take {
                pcm: pcm.into(),
                sample_rate: sr,
                gait,
                source: source.clone(),
            });
        }
    }
    out
}

/// Scale a take to unit [`loudness`], limiting its transient to [`MAX_CREST_DB`] over it
/// first. False for silence.
fn normalize(pcm: &mut [f32], sr: f32) -> bool {
    for _ in 0..2 {
        let l = loudness(pcm, sr);
        if l <= 1e-7 {
            return false;
        }
        pcm.iter_mut().for_each(|v| *v /= l);
        let ceiling = db(MAX_CREST_DB);
        if pcm.iter().all(|v| v.abs() <= ceiling * 1.02) {
            return true;
        }
        limit(pcm, ceiling, sr);
    }
    let l = loudness(pcm, sr);
    pcm.iter_mut().for_each(|v| *v /= l);
    true
}

/// Push a recording's background (its room, wind, hiss) down between and after the sounds of
/// a step: a downward expander, 1:3 below 12 dB over the noise floor `floor_db` (of the
/// 10 ms envelope), at most 24 dB, so takes played over each other don't pile up a bed of
/// noise.
fn expand(x: &mut [f32], floor_db: f32, sr: f32) {
    let threshold = floor_db + 12.0;
    let env_k = 1.0 - (-1.0 / (0.005 * sr)).exp();
    let attack = 1.0 - (-1.0 / (0.001 * sr)).exp();
    let release = 1.0 - (-1.0 / (0.04 * sr)).exp();
    // Run the detector ahead by 2 ms so the gain is open when a sound starts.
    let ahead = (0.002 * sr) as usize;
    let mut ms = 0.0f32;
    let mut levels = vec![0.0f32; x.len()];
    for (i, l) in levels.iter_mut().enumerate() {
        let v = x[(i + ahead).min(x.len() - 1)];
        ms += (v * v - ms) * env_k;
        *l = 10.0 * (ms + 1e-14).log10();
    }
    let mut g = 1.0f32;
    for (v, l) in x.iter_mut().zip(&levels) {
        let want = db(((l - threshold) * 2.0).clamp(-24.0, 0.0));
        g += (want - g) * if want > g { attack } else { release };
        *v *= g;
    }
}

/// A look-ahead peak limiter: 1.5 ms look-ahead, 40 ms release.
fn limit(x: &mut [f32], ceiling: f32, sr: f32) {
    let ahead = ((0.0015 * sr) as usize).max(1);
    let need: Vec<f32> = x
        .iter()
        .map(|v| (ceiling / v.abs().max(1e-9)).min(1.0))
        .collect();
    // The gain each sample needs, spread back over the look-ahead so it ramps down in time.
    let mut target = need.clone();
    let mut q: std::collections::VecDeque<usize> = Default::default();
    for i in (0..need.len()).rev() {
        while q.front().is_some_and(|&j| j > i + ahead) {
            q.pop_front();
        }
        while q.back().is_some_and(|&j| need[j] >= need[i]) {
            q.pop_back();
        }
        q.push_back(i);
        target[i] = need[*q.front().unwrap_or(&i)];
    }
    let attack = 1.0 - (-1.0 / (0.0005 * sr)).exp();
    let release = 1.0 - (-1.0 / (0.04 * sr)).exp();
    let mut g = 1.0f32;
    for (v, t) in x.iter_mut().zip(&target) {
        g += (t - g) * if *t < g { attack } else { release };
        *v *= g.min(*t);
    }
}

/// A step found in a recording.
#[derive(Debug, Clone)]
struct Cut {
    range: std::ops::Range<usize>,
    gait: Gait,
    /// Peak of its 10 ms envelope, dB.
    peak_db: f32,
    /// The recording's noise floor (10 ms envelope), dB.
    floor_db: f32,
}

/// The step period of a recording, s: the strongest repeat of its envelope between 0.25 and
/// 1.6 s (or half of it, when every other step is louder). `None` without a clear rhythm.
fn step_period(d10: &[f32], top: f32) -> Option<f32> {
    // 10 ms frames of the envelope, floored 45 dB under the loudest steps.
    let frames: Vec<f32> = d10
        .chunks(10)
        .map(|c| c.iter().fold(f32::MIN, |m, v| m.max(*v)).max(top - 45.0))
        .collect();
    if frames.len() < 200 {
        return None;
    }
    let mean = frames.iter().sum::<f32>() / frames.len() as f32;
    let f: Vec<f32> = frames.iter().map(|v| v - mean).collect();
    let ac = |lag: usize| -> f32 {
        f.iter().zip(&f[lag..]).map(|(a, b)| a * b).sum::<f32>() / (f.len() - lag) as f32
    };
    let zero = ac(0).max(1e-9);
    let lags: Vec<(usize, f32)> = (25..160.min(f.len() / 2))
        .map(|l| (l, ac(l) / zero))
        .collect();
    let &(best, peak) = lags.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    if peak < 0.12 {
        return None;
    }
    // A local maximum at about half the lag that is nearly as strong is the real step.
    let half = lags
        .iter()
        .filter(|(l, v)| {
            (*l as f32 - best as f32 / 2.0).abs() <= 4.0 && *v >= 0.6 * peak && *l >= 25
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    Some(half.map(|h| h.0).unwrap_or(best) as f32 * 0.01)
}

/// Find the steps in a recording: each starts where the 10 ms envelope jumps by more than
/// 9 dB over its level since the last step, at most once per 60 % of the step period.
/// Steps much quieter or louder than the median, too short, or swelling slowly (scrapes)
/// are dropped.
fn cut_steps(x: &[f32], sr: f32, part: Part) -> Vec<Cut> {
    let hop = (sr * 0.001).max(1.0) as usize;
    // High-passed envelope in dB at 1 ms hops.
    let mut hp = Biquad::highpass(sr, 60.0, 0.7);
    let mut sq = vec![0.0f64; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        let y = hp.process(*v) as f64;
        sq[i + 1] = sq[i] + y * y;
    }
    let env = |win_s: f32| -> Vec<f32> {
        let w = ((win_s * sr) as usize).max(1);
        (0..x.len() / hop)
            .map(|k| {
                let c = k * hop;
                let lo = c.saturating_sub(w / 2);
                let hi = (c + w / 2).min(x.len());
                let ms = (sq[hi] - sq[lo]) / (hi - lo).max(1) as f64;
                (10.0 * (ms + 1e-14).log10()) as f32
            })
            .collect()
    };
    let d10 = env(0.010);
    let d2 = env(0.002);
    if d10.len() < 100 {
        return Vec::new();
    }
    let mut sorted = d10.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let top = sorted[sorted.len() * 99 / 100];
    let thr = (floor + 12.0).max(top - 40.0);
    let found = step_period(&d10, top);
    let period = match part {
        Part::Walk => found.unwrap_or(0.55).clamp(0.45, 1.3),
        Part::Run => found.unwrap_or(0.33).clamp(0.25, 0.5),
        Part::Steps => found.unwrap_or(0.55),
    };
    let min_gap = ((period * 600.0) as usize).clamp(180, 700);
    let look = (min_gap / 2).max(80);
    let mut onsets: Vec<usize> = Vec::new();
    let mut k = 5;
    while k < d10.len() {
        let from = k
            .saturating_sub(look)
            .max(onsets.last().map(|o| o + 40).unwrap_or(0));
        let back = d10[from.min(k - 4)..k - 4]
            .iter()
            .fold(f32::MAX, |m, v| m.min(*v));
        if d10[k] > thr && d10[k] - back > 9.0 && onsets.last().is_none_or(|&o| k - o >= min_gap) {
            // The onset proper: going back from the peak of the next 40 ms, where the 2 ms
            // envelope was 20 dB below it.
            let end = (k + 40).min(d2.len());
            let (pk, pk_db) =
                (k..end).fold(
                    (k, f32::MIN),
                    |m, j| {
                        if d2[j] > m.1 { (j, d2[j]) } else { m }
                    },
                );
            let mut o = pk;
            while o > k.saturating_sub(30) && d2[o] > pk_db - 20.0 {
                o -= 1;
            }
            onsets.push(o);
            k = o + min_gap;
            continue;
        }
        k += 1;
    }
    let max_len = ((period * 1100.0) as usize).clamp(250, 700);
    let pad = (ONSET_PAD_S * 1000.0) as usize;
    let gait = match part {
        Part::Walk => Gait::Walk,
        Part::Run => Gait::Run,
        Part::Steps if period < 0.42 => Gait::Run,
        Part::Steps => Gait::Walk,
    };
    let mut steps = Vec::new();
    for (i, &o) in onsets.iter().enumerate() {
        let next = onsets.get(i + 1).copied().unwrap_or(d10.len());
        let limit = (next.saturating_sub(8)).min(o + max_len).min(d10.len());
        if limit <= o + 20 {
            continue;
        }
        let peak = d10[o..(o + 150).min(limit)]
            .iter()
            .fold(f32::MIN, |m, v| m.max(*v));
        let mut end = limit;
        while end > o + 60 && d10[end - 1] < peak - 36.0 {
            end -= 1;
        }
        let end = (end + 15).min(limit);
        // Slow swells (a scrape, a shuffle) are not steps: nothing near the peak comes early.
        // (A step's toe or compaction may well be louder than its heel.)
        let rise = (o..(o + 150).min(limit))
            .position(|j| d10[j] >= peak - 6.0)
            .unwrap_or(0);
        steps.push((o, end, peak, rise));
    }
    if steps.is_empty() {
        return Vec::new();
    }
    let mut peaks: Vec<f32> = steps.iter().map(|s| s.2).collect();
    peaks.sort_by(f32::total_cmp);
    let median = peaks[peaks.len() / 2];
    steps
        .into_iter()
        .filter(|&(o, end, peak, rise)| {
            end - o >= 90 && rise <= 100 && peak > median - 10.0 && peak < median + 8.0
        })
        .map(|(o, end, peak, _)| Cut {
            range: o.saturating_sub(pad) * hop..(end * hop).min(x.len()),
            gait,
            peak_db: peak,
            floor_db: floor,
        })
        .filter(|c| c.range.end > c.range.start)
        .collect()
}

/// Loudness of a step: its whole energy, K-weighted as in ITU-R BS.1770 (+4 dB above
/// 1.7 kHz, high-pass at 38 Hz), as an RMS over a fixed 0.4 s, so a short click and a long
/// rustle of the same energy measure the same. Synthesized steps are measured the same way to
/// match levels.
pub fn loudness(x: &[f32], sr: f32) -> f32 {
    let mut hp = Biquad::highpass(sr, 38.0, 0.5);
    let mut shelf = Biquad::high_shelf(sr, 1_681.0, 4.0);
    let n = ((0.4 * sr) as usize).max(1);
    let e: f64 = x
        .iter()
        .map(|&v| {
            let y = shelf.process(hp.process(v)) as f64;
            y * y
        })
        .sum();
    (e / n as f64).sqrt() as f32
}

// ---------------------------------------------------------------- playing a take

/// A take as one footstep plays it: which take, how fast (pitch), how loud, and its tone.
#[derive(Debug, Clone)]
pub struct StepSample {
    pub take: Take,
    /// Playback speed: 1 plays the recording as recorded (pitch and length).
    pub speed: f32,
    /// Linear gain applied to the (unit-loudness) take.
    pub gain: f32,
    /// High shelf above 2.5 kHz, dB: heavier and faster steps are brighter.
    pub shelf_db: f32,
    /// Low-pass, Hz (20 kHz = open): wet ground is duller.
    pub lowpass_hz: f32,
}

/// Plays a [`StepSample`] at the mixer's rate (Catmull-Rom resampling).
#[derive(Debug, Clone)]
pub(crate) struct SampleVoice {
    pcm: Arc<[f32]>,
    pos: f64,
    inc: f64,
    gain: f32,
    shelf: Biquad,
    lp: Biquad,
    fade: f32,
}

impl SampleVoice {
    pub(crate) fn new(fs: f32, s: &StepSample) -> Self {
        let nyq = fs * 0.45;
        Self {
            pcm: s.take.pcm.clone(),
            pos: 0.0,
            inc: (s.take.sample_rate / fs * s.speed.clamp(0.25, 4.0)) as f64,
            gain: s.gain,
            shelf: Biquad::high_shelf(fs, 2_500.0, s.shelf_db.clamp(-12.0, 12.0)),
            lp: Biquad::lowpass(fs, s.lowpass_hz.clamp(200.0, nyq), 0.707),
            fade: 1.0,
        }
    }

    /// Output samples left (at the mixer's rate).
    pub(crate) fn remaining(&self) -> u32 {
        ((self.pcm.len() as f64 - self.pos).max(0.0) / self.inc) as u32
    }

    pub(crate) fn active(&self) -> bool {
        (self.pos as usize) + 2 < self.pcm.len()
    }

    #[inline]
    pub(crate) fn next(&mut self) -> f32 {
        let i = self.pos as usize;
        let n = self.pcm.len();
        if i + 2 >= n {
            return 0.0;
        }
        let t = (self.pos - i as f64) as f32;
        let p = &self.pcm;
        let p0 = if i > 0 { p[i - 1] } else { p[i] };
        let v = hermite(p0, p[i], p[i + 1], p[i + 2], t);
        self.pos += self.inc;
        // A short fade over the last few milliseconds, so a cut tail never clicks.
        let left = n as f64 - self.pos;
        if left < 96.0 {
            self.fade = (left / 96.0).max(0.0) as f32;
        }
        self.lp.process(self.shelf.process(v)) * self.gain * self.fade
    }
}

// ---------------------------------------------------------------- the resource

enum Load {
    /// Not asked for yet.
    Idle,
    /// Loading on a background thread.
    Loading(Receiver<SampleBank>),
    Ready(SampleBank),
}

/// The recorded footsteps (see the [module docs](self)): loads them in the background at
/// startup and decides, step by step, which take plays and how.
#[derive(Resource)]
pub struct FootstepSamples {
    load: Load,
    dir: Option<PathBuf>,
    /// Synthesized loudness per surface (by its [`SurfaceSound`]), the level takes match.
    reference: HashMap<String, (SurfaceSound, f32)>,
}

impl Default for FootstepSamples {
    fn default() -> Self {
        Self {
            load: Load::Idle,
            dir: None,
            reference: HashMap::new(),
        }
    }
}

impl FootstepSamples {
    /// A bank that is ready right away (tests, tools).
    pub fn from_bank(bank: SampleBank) -> Self {
        Self {
            load: Load::Ready(bank),
            ..Default::default()
        }
    }

    /// Start loading `dir` (usually `<asset root>/`[`SAMPLE_DIR`]) on a background thread.
    pub fn start_loading(&mut self, dir: PathBuf) {
        let (tx, rx) = bounded(1);
        let d = dir.clone();
        let spawned = std::thread::Builder::new()
            .name("footstep-samples".into())
            .spawn(move || {
                let _ = tx.send(SampleBank::load(&d));
            });
        self.dir = Some(dir);
        self.load = match spawned {
            Ok(_) => Load::Loading(rx),
            Err(_) => Load::Ready(SampleBank::default()),
        };
    }

    /// Whether loading has been started (or a bank was given).
    pub fn started(&self) -> bool {
        !matches!(self.load, Load::Idle)
    }

    /// The bank once loaded. With `wait`, blocks until loading is done (offline rendering, so
    /// a recording comes out the same every time); otherwise `None` while it loads.
    pub fn bank(&mut self, wait: bool) -> Option<&mut SampleBank> {
        if let Load::Loading(rx) = &self.load {
            let got = if wait {
                rx.recv().ok()
            } else {
                match rx.try_recv() {
                    Ok(b) => Some(b),
                    Err(crossbeam_channel::TryRecvError::Empty) => return None,
                    Err(_) => None,
                }
            };
            let bank = got.unwrap_or_default();
            let dir = self
                .dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            if bank.is_empty() {
                engine_core::log::info!(
                    "audio: no recorded footsteps in {dir} (`cargo run -p fetch-assets -- audio` \
                     fetches them); footsteps are synthesized"
                );
            } else {
                engine_core::log::info!(
                    "audio: {} recorded footstep takes on {} surfaces from {dir}",
                    bank.len(),
                    bank.surfaces.len()
                );
            }
            self.load = Load::Ready(bank);
        }
        match &mut self.load {
            Load::Ready(b) => Some(b),
            _ => None,
        }
    }

    /// Takes per surface, for status reports (empty while loading).
    pub fn summary(&self) -> BTreeMap<String, usize> {
        match &self.load {
            Load::Ready(b) => b
                .surfaces
                .iter()
                .map(|(k, v)| (k.clone(), v.takes.len()))
                .collect(),
            _ => BTreeMap::new(),
        }
    }

    /// How a footstep on `material` sounds with a recording: the take to play, and the step
    /// for the synthesized layers on top (only what the recording lacks). `None` when there
    /// is no fitting take, so the step is synthesized whole.
    ///
    /// `step` is the step as it would be synthesized (its surface already changed by the
    /// ground's state), `dry` the material's own sound, `cell` and `wading_m` the ground's
    /// state, and `wait` blocks for a bank that is still loading.
    #[allow(clippy::too_many_arguments)]
    pub fn recorded(
        &mut self,
        material: &str,
        dry: &SurfaceSound,
        cell: &SurfaceCell,
        wading_m: f32,
        surfaces: &FootstepSurfaces,
        step: &Step,
        fs: f32,
        wait: bool,
    ) -> Option<(Step, StepSample)> {
        let (kind_speed, kind_gain) = match step.kind {
            ContactKind::Foot => (1.0, 1.0),
            // A dog's paw: lighter and quicker than a boot.
            ContactKind::Paw => (1.3, db(-7.0)),
            // A hoof: heavier and lower, with the synthesized clop on top.
            ContactKind::Hoof => (0.82, db(1.5)),
            _ => return None,
        };
        // What the ground sounds like now decides which recordings fit: deep fresh snow
        // covers anything, and a puddle (or wading) is water whatever is under it.
        let snow = surfaces.snow();
        let cover = smooth(0.005, 0.07, cell.snow_m);
        let puddle = smooth(0.002, 0.03, cell.puddle_m);
        let (name, base) = if cover >= 0.5 && dry.crunch < snow.crunch {
            ("snow".to_string(), snow)
        } else if (puddle >= 0.5 || wading_m > 0.05) && material != "water" {
            let water = surfaces
                .surfaces
                .get("water")
                .copied()
                .or_else(|| SurfaceSound::preset("water"))
                .unwrap_or_default();
            ("water".to_string(), water)
        } else {
            (material.to_string(), *dry)
        };
        let state = step.surface;
        let kept = state.kept(&base);
        if kept < 0.4 {
            // The ground has changed too much (frozen mud is not mud): synthesize it.
            return None;
        }
        self.bank(wait)?.takes(&name)?;
        let reference = self.reference_loudness(&name, &base, fs);
        let bank = self.bank(false)?;
        let mut rng = Noise::new(step.seed ^ 0x5EED_FA11_0F57_E950);
        let run = ((0.11 - step.roll) / 0.06).clamp(0.0, 1.0);
        let take = bank.takes_mut(&name)?.pick(run, &mut rng)?.clone();

        let i = step.intensity.max(0.05);
        // The synthesized level law (gain ∝ i^0.8, strikes ∝ √i up to 2) and its loudness at
        // the reference step (75 kg walking, i = 1), plus ±1.5 dB per step.
        let level = i.powf(0.8) * i.min(2.0).sqrt();
        let gain = reference * level * kept.sqrt() * kind_gain * db(rng.range(-1.5, 1.5));
        let pitch = (state.pitch / base.pitch.max(0.05)).clamp(0.7, 1.4);
        let speed = kind_speed * pitch * 2f32.powf(rng.range(-0.6, 0.6) / 12.0);
        let tone = state.brightness / base.brightness.max(0.05);
        let shelf_db = (4.5 * i.log2()).clamp(-6.0, 4.0) + (8.0 * tone.log2()).clamp(-9.0, 3.0);
        let lowpass_hz = if tone < 1.0 {
            (20_000.0 * tone.powf(2.5)).max(1_200.0)
        } else {
            20_000.0
        };
        // Synthesize what the recording lacks, plus a little of what it has: weight (a
        // heavier walker thumps more) and per-step grain.
        let mut over = step.surface.over_recording(&base, 0.12);
        over.thump += base.thump.min(state.thump) * (0.25 * (i - 1.0)).clamp(0.0, 0.5);
        let synth = Step {
            surface: over,
            ..*step
        };
        Some((
            synth,
            StepSample {
                take,
                speed,
                gain,
                shelf_db,
                lowpass_hz,
            },
        ))
    }

    /// Loudness of the synthesized reference step on `surface` (cached until its sound is
    /// changed in [`FootstepSurfaces`]).
    fn reference_loudness(&mut self, name: &str, sound: &SurfaceSound, fs: f32) -> f32 {
        if let Some((s, l)) = self.reference.get(name)
            && s == sound
        {
            return *l;
        }
        let l = crate::footstep::reference_loudness(sound, fs);
        self.reference.insert(name.to_string(), (*sound, l));
        l
    }
}

/// Start loading the recorded footsteps at startup (from the asset root).
pub(crate) fn start_loading(
    mut samples: ResMut<FootstepSamples>,
    settings: Res<crate::AudioSettings>,
    assets: Option<Res<engine_assets::AssetServer>>,
) {
    if samples.started() || !settings.recorded_footsteps {
        return;
    }
    let root = assets
        .map(|a| a.root())
        .unwrap_or_else(|| PathBuf::from("assets"));
    samples.start_loading(root.join(SAMPLE_DIR));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footstep::SurfaceSound;

    const SR: f32 = 48_000.0;

    /// A fake recording: a click-and-decay "step" every `gap` seconds over a noise floor.
    fn steps(n: usize, gap: f32, seed: u64) -> Vec<f32> {
        let mut rng = Noise::new(seed);
        let len = ((n as f32 * gap + 0.5) * SR) as usize;
        let mut x: Vec<f32> = (0..len).map(|_| rng.white() * 0.0005).collect();
        for k in 0..n {
            let at = ((0.2 + k as f32 * gap) * SR) as usize;
            for j in 0..(0.3 * SR) as usize {
                if let Some(v) = x.get_mut(at + j) {
                    let t = j as f32 / SR;
                    *v += rng.white() * 0.3 * (-t * 25.0).exp();
                }
            }
        }
        x
    }

    fn take(gait: Gait, v: f32) -> Take {
        Take {
            pcm: vec![v; 100].into(),
            sample_rate: SR,
            gait,
            source: "test".into(),
        }
    }

    #[test]
    fn a_recording_is_cut_into_one_take_per_step() {
        let x = steps(8, 0.6, 1);
        let takes = takes_from_recording("unlisted", &x, SR);
        assert_eq!(takes.len(), 8);
        for t in &takes {
            assert_eq!(t.gait, Gait::Walk);
            // Normalized, and starting just before the step.
            assert!((loudness(&t.pcm, SR) - 1.0).abs() < 0.01);
            let onset = t.pcm.iter().position(|v| v.abs() > 0.5).unwrap();
            assert!(onset < (0.01 * SR) as usize, "onset at {onset}");
            assert!(t.pcm.len() < (0.6 * SR) as usize);
        }
        // Close-together steps are running.
        let run = takes_from_recording("unlisted", &steps(8, 0.33, 2), SR);
        assert!(run.len() >= 7 && run.iter().all(|t| t.gait == Gait::Run));
    }

    #[test]
    fn odd_steps_are_left_out() {
        let mut x = steps(7, 0.6, 3);
        // A much quieter fourth step (a distant or partial one).
        let at = ((0.2 + 3.0 * 0.6) * SR) as usize;
        for v in &mut x[at..at + (0.3 * SR) as usize] {
            *v *= 0.05;
        }
        let takes = takes_from_recording("unlisted", &x, SR);
        assert_eq!(takes.len(), 6);
    }

    #[test]
    fn takes_never_repeat_right_away_and_follow_the_gait() {
        let mut set = TakeSet::new(
            (0..6)
                .map(|i| take(if i < 4 { Gait::Walk } else { Gait::Run }, i as f32))
                .collect(),
        );
        let mut rng = Noise::new(7);
        let mut last = f32::NAN;
        let mut seen = [0; 6];
        for _ in 0..200 {
            let t = set.pick(0.0, &mut rng).unwrap();
            let id = t.pcm[0];
            assert_eq!(t.gait, Gait::Walk);
            assert_ne!(id, last, "repeated a take");
            last = id;
            seen[id as usize] += 1;
        }
        // Every walking take gets used.
        assert!(seen[..4].iter().all(|&n| n > 20), "{seen:?}");
        let t = set.pick(1.0, &mut rng).unwrap();
        assert_eq!(t.gait, Gait::Run);
        // Two takes alternate; one take is all there is.
        let mut two = TakeSet::new(vec![take(Gait::Walk, 0.0), take(Gait::Walk, 1.0)]);
        let a = two.pick(0.0, &mut rng).unwrap().pcm[0];
        let b = two.pick(0.0, &mut rng).unwrap().pcm[0];
        assert_ne!(a, b);
        let mut one = TakeSet::new(vec![take(Gait::Run, 5.0)]);
        assert_eq!(one.pick(0.0, &mut rng).unwrap().pcm[0], 5.0);
        assert!(TakeSet::default().pick(0.0, &mut rng).is_none());
    }

    #[test]
    fn picks_are_reproducible() {
        let make = || {
            TakeSet::new(
                (0..9)
                    .map(|i| take(Gait::Walk, i as f32))
                    .collect::<Vec<_>>(),
            )
        };
        let run = |mut set: TakeSet| {
            let mut rng = Noise::new(42);
            (0..30)
                .map(|_| set.pick(0.3, &mut rng).unwrap().pcm[0])
                .collect::<Vec<_>>()
        };
        assert_eq!(run(make()), run(make()));
    }

    /// A bank with a few takes on grass, snow and water (each a recognizable tone).
    fn bank() -> SampleBank {
        let tone = |hz: f32, gait: Gait| {
            let pcm: Vec<f32> = (0..(0.3 * SR) as usize)
                .map(|i| {
                    let t = i as f32 / SR;
                    (std::f32::consts::TAU * hz * t).sin() * (-t * 12.0).exp()
                })
                .collect();
            let l = loudness(&pcm, SR);
            Take {
                pcm: pcm.iter().map(|v| v / l).collect::<Vec<_>>().into(),
                sample_rate: SR,
                gait,
                source: "tone".into(),
            }
        };
        let mut b = SampleBank::default();
        for (name, hz) in [("grass", 500.0), ("snow", 900.0), ("water", 1_300.0)] {
            b.surfaces.insert(
                name.into(),
                TakeSet::new(vec![
                    tone(hz, Gait::Walk),
                    tone(hz * 1.01, Gait::Walk),
                    tone(hz * 1.02, Gait::Run),
                ]),
            );
        }
        b
    }

    fn step_on(
        samples: &mut FootstepSamples,
        surface: &str,
        cell: &SurfaceCell,
        kind: ContactKind,
        load: f32,
        speed: f32,
        temperature: f32,
    ) -> Option<(Step, StepSample)> {
        let surfaces = FootstepSurfaces::default();
        let dry = surfaces.surfaces[surface];
        let state = dry.with_state(cell, 0.6, temperature, 0.0, &surfaces.snow());
        let step = Step::from_contact(state, kind, load, speed, 11);
        samples.recorded(surface, &dry, cell, 0.0, &surfaces, &step, SR, true)
    }

    #[test]
    fn surfaces_without_takes_and_other_contacts_are_synthesized() {
        let mut s = FootstepSamples::from_bank(bank());
        let dry = SurfaceCell::default();
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Foot, 75.0, 1.4, 15.0).is_some());
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Paw, 20.0, 1.4, 15.0).is_some());
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Hoof, 500.0, 1.4, 15.0).is_some());
        // No takes for gravel; wheels and falling bodies are always synthesized.
        assert!(step_on(&mut s, "gravel", &dry, ContactKind::Foot, 75.0, 1.4, 15.0).is_none());
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Wheel, 900.0, 5.0, 15.0).is_none());
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Body, 75.0, 0.0, 15.0).is_none());
        // Frozen mud is not mud any more.
        let mut b = bank();
        let mud = b.surfaces["grass"].clone();
        b.surfaces.insert("mud".into(), mud);
        let mut s = FootstepSamples::from_bank(b);
        assert!(step_on(&mut s, "mud", &dry, ContactKind::Foot, 75.0, 1.4, 10.0).is_some());
        assert!(step_on(&mut s, "mud", &dry, ContactKind::Foot, 75.0, 1.4, -12.0).is_none());
        // No bank at all.
        let mut none = FootstepSamples::from_bank(SampleBank::default());
        assert!(step_on(&mut none, "grass", &dry, ContactKind::Foot, 75.0, 1.4, 15.0).is_none());
    }

    #[test]
    fn deep_snow_and_puddles_play_their_own_takes() {
        let mut s = FootstepSamples::from_bank(bank());
        // The take's tone, from its upward zero crossings.
        let freq = |r: &(Step, StepSample)| {
            let p = &r.1.take.pcm;
            p.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32 / 0.3
        };
        let snowed = SurfaceCell {
            snow_m: 0.2,
            ..Default::default()
        };
        let r = step_on(&mut s, "grass", &snowed, ContactKind::Foot, 75.0, 1.4, -2.0).unwrap();
        assert!((850.0..960.0).contains(&freq(&r)), "{}", freq(&r));
        let puddle = SurfaceCell {
            puddle_m: 0.05,
            ..Default::default()
        };
        let r = step_on(&mut s, "grass", &puddle, ContactKind::Foot, 75.0, 1.4, 12.0).unwrap();
        assert!((1_250.0..1_360.0).contains(&freq(&r)), "{}", freq(&r));
        // The grass under the puddle still rustles: synthesized over the water's take.
        assert!(r.0.surface.rustle > 0.5);
    }

    #[test]
    fn the_synthesis_adds_only_what_the_recording_lacks() {
        let mut s = FootstepSamples::from_bank(bank());
        let dry = SurfaceCell::default();
        let (synth, _) =
            step_on(&mut s, "grass", &dry, ContactKind::Foot, 75.0, 1.4, 15.0).unwrap();
        let grass = SurfaceSound::preset("grass").unwrap();
        assert!(synth.surface.rustle < grass.rustle * 0.2);
        assert!(synth.surface.squelch < 0.01);
        let wet = SurfaceCell {
            wetness: 1.0,
            ..Default::default()
        };
        let (synth, take) =
            step_on(&mut s, "grass", &wet, ContactKind::Foot, 75.0, 1.4, 15.0).unwrap();
        // Rain's squelch is synthesized; the recording is duller.
        assert!(synth.surface.squelch > 0.2);
        assert!(take.lowpass_hz < 15_000.0);
    }

    #[test]
    fn heavier_and_faster_steps_are_louder_and_brighter_and_runs_pick_running_takes() {
        let mut s = FootstepSamples::from_bank(bank());
        let dry = SurfaceCell::default();
        let mut at = |load: f32, speed: f32| {
            step_on(&mut s, "grass", &dry, ContactKind::Foot, load, speed, 15.0)
                .unwrap()
                .1
        };
        let (light, normal, heavy) = (at(40.0, 1.4), at(75.0, 1.4), at(150.0, 1.4));
        assert!(light.gain < normal.gain && normal.gain < heavy.gain);
        assert!(light.shelf_db < normal.shelf_db && normal.shelf_db < heavy.shelf_db);
        let run = at(75.0, 5.5);
        assert!(run.gain > normal.gain * 1.5);
        assert_eq!(run.take.gait, Gait::Run);
        // Walking never picks the running take.
        for _ in 0..20 {
            assert_eq!(at(75.0, 1.4).take.gait, Gait::Walk);
        }
    }

    #[test]
    fn a_missing_folder_gives_an_empty_bank() {
        let bank = SampleBank::load(Path::new("no/such/folder"));
        assert!(bank.is_empty());
        let mut samples = FootstepSamples::default();
        samples.start_loading("no/such/folder".into());
        assert!(samples.bank(true).unwrap().is_empty());
    }

    #[test]
    fn wav_recordings_load_from_a_folder() {
        let dir = std::env::temp_dir().join(format!("engine_audio_takes_{}", std::process::id()));
        let x = steps(6, 0.6, 5);
        crate::wav::write_wav(&dir.join("concrete/test_steps.wav"), &x, 1, SR as u32).unwrap();
        let bank = SampleBank::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(bank.takes("concrete").map(|t| t.takes.len()), Some(6));
        // Tile borrows concrete's; nothing for snow.
        assert!(bank.takes("tile").is_some());
        assert!(bank.takes("snow").is_none());
    }
}
