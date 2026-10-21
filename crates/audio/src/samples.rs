//! Recorded footsteps: real steps cut from field recordings, one bank of takes per surface.
//!
//! The recordings are CC0 and CC-BY Freesound previews listed in
//! `assets/manifests/audio.toml` and fetched to `assets/fetched/audio/footsteps/<surface>/`
//! (`cargo run -p fetch-assets -- audio`). Each file holds a run of steps; loading cuts it
//! into one take per step (where the envelope jumps, until it falls back to the noise before
//! it, at most 0.35 s on hard ground and 0.6 s on soft), drops the ones that are not a clean
//! step (a jump, a scrape, a step much quieter than its neighbours, one less than
//! [`MIN_SNR_DB`] over the recording's hiss), labels each one walk or run, and normalizes
//! every take to the same loudness. A step that misses the noise bar by up to 15 dB has the
//! recording around it cleaned ([`denoise`]: rain, wind, room tone and hiss taken out, the
//! tail gated) and must pass the bar again on the cleaned recording; cleaned takes only fill
//! in where a ground has fewer than [`CLEAN_ENOUGH`] clean ones. A surface left with fewer
//! than [`MIN_TAKES`] takes is synthesized.
//!
//! A footstep then plays one take, picked at random but never one of the last few on that
//! surface, slightly re-pitched and re-levelled, brighter for heavier and faster steps, and
//! duller and quieter when the ground's state (rain, a puddle, frost) has changed what the
//! recording sounds like. The synthesized layers of [`crate::footstep`] still play on top,
//! but only what the recording lacks: the squelch of wet ground, a puddle's splash, a heavy
//! walker's thump, a snow crust's squeak. A surface with no takes, or a missing folder, falls
//! back to the synthesis alone, so nothing needs the fetched files (CI never fetches them).
//!
//! Shoes and boots, a dog's paws and a horse's hooves each have their own recordings
//! ([`Feet`]): `fetched/audio/footsteps/<surface>/`, `fetched/audio/paws/<ground>/` and
//! `fetched/audio/hooves/<ground>/`. Animals are recorded on far fewer grounds, so their
//! folders are kinds of ground (`hard`, `soft`, `wood`, `gravel`, ...) that several surfaces
//! share, each recorded on one surface ([`Feet::recorded_on`]); the synthesis on top adds what
//! the step's own surface has beyond that one (a dog's paw take from a wooden floor, played
//! on rock, gets rock's grit synthesized over it). Their takes are cut shorter (a paw is a
//! tap, a hoof a clop), so four feet don't fill the gaps between steps with noise. A walker
//! with [`RecordedSteps`] `{ enabled: false }` is always synthesized.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, bounded};
use engine_core::prelude::*;
use engine_world::{ContactKind, SurfaceCell};

use crate::dsp::{Biquad, Noise, db, hermite};
use crate::footstep::{FootstepSurfaces, Step, SurfaceSound, WornPath, smooth};

/// Folder under the asset root that holds one subfolder of recordings per surface material
/// (`<root>/fetched/audio/footsteps/gravel/*.ogg`).
pub const SAMPLE_DIR: &str = "fetched/audio/footsteps";

/// Folder under the asset root that holds every kind of [`Feet`]'s folder
/// (`<root>/fetched/audio/{footsteps,paws,hooves}/`).
pub const AUDIO_DIR: &str = "fetched/audio";

/// Which recordings a contact plays: people's shoes, a dog's paws or a horse's hooves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Feet {
    Shoes,
    Paws,
    Hooves,
}

impl Feet {
    /// Every kind, in folder order.
    pub const ALL: [Feet; 3] = [Feet::Shoes, Feet::Paws, Feet::Hooves];

    /// The recordings a contact plays (`None`: wheels, tracks and falling bodies, which are
    /// always synthesized).
    pub fn of(kind: ContactKind) -> Option<Self> {
        match kind {
            ContactKind::Foot => Some(Feet::Shoes),
            ContactKind::Paw => Some(Feet::Paws),
            ContactKind::Hoof => Some(Feet::Hooves),
            _ => None,
        }
    }

    /// Its folder under [`AUDIO_DIR`].
    pub fn folder(self) -> &'static str {
        match self {
            Feet::Shoes => "footsteps",
            Feet::Paws => "paws",
            Feet::Hooves => "hooves",
        }
    }

    /// The folder (of this kind's) whose takes a step on `surface` plays: the surface itself
    /// for shoes (or the one it borrows from, like `tile` from `concrete`), the kind of ground
    /// for paws and hooves. `None` for a surface no folder covers.
    pub fn ground(self, surface: &str) -> Option<&str> {
        match self {
            Feet::Shoes => Some(surface),
            Feet::Paws => Some(match surface {
                "water" => "water",
                "wood" | "planks" | "tile" | "concrete" | "asphalt" | "rock" | "stone"
                | "metal" | "ice" | "glass" => "hard",
                "grass" | "dirt" | "soil" | "moss" | "leaves" | "sand" | "mud" | "snow"
                | "gravel" | "ash" | "fabric" | "carpet" | "path" | "packed_snow" => "soft",
                _ => return None,
            }),
            Feet::Hooves => Some(match surface {
                "wood" | "planks" => "wood",
                "gravel" => "gravel",
                "concrete" | "asphalt" | "tile" | "rock" | "stone" | "metal" | "ice" => "hard",
                "grass" | "dirt" | "soil" | "moss" | "leaves" | "sand" | "mud" | "snow" | "ash"
                | "path" | "packed_snow" => "dirt",
                "water" => "water",
                _ => return None,
            }),
        }
    }

    /// The surface the recordings in `folder` were made on, whose [`SurfaceSound`] the
    /// synthesis on top is measured against: the folder itself for shoes; a wooden floor for
    /// paws on hard ground, a lawn for soft, a road for hooves on hard ground, a dirt track
    /// for soft.
    pub fn recorded_on(self, folder: &str) -> &str {
        match (self, folder) {
            (Feet::Paws, "hard") => "wood",
            (Feet::Paws, "soft") => "grass",
            (Feet::Hooves, "hard") => "asphalt",
            (Feet::Hooves, "dirt") => "dirt",
            _ => folder,
        }
    }

    /// How hard ([`Step::intensity`]) the steps in this kind's recordings are: a 75 kg
    /// person walking, one paw of a 22 kg dog and one hoof of a 500 kg horse at a walk (half
    /// the weight on each foot that lands).
    pub fn recorded_intensity(self) -> f32 {
        match self {
            Feet::Shoes => 1.0,
            Feet::Paws => 0.43,
            Feet::Hooves => 2.0,
        }
    }

    /// How this kind's recordings are cut into takes.
    fn cutter(self, folder: &str) -> Cutter {
        match self {
            Feet::Shoes => Cutter::SHOES.with_max(max_take_s(folder)),
            // A paw lands every 0.1-0.2 s at a trot: taps, cut short.
            Feet::Paws => Cutter {
                min_gap_ms: Some(70),
                look_ms: 60,
                peak_ms: 40,
                min_len_ms: 30,
                max_rise_ms: 25,
                settle_ms: 15,
                max_s: 0.15,
                tail: Some((0.05, 0.04)),
            },
            Feet::Hooves => Cutter {
                min_gap_ms: Some(110),
                look_ms: 90,
                peak_ms: 70,
                min_len_ms: 50,
                max_rise_ms: 40,
                settle_ms: 25,
                max_s: if matches!(folder, "dirt" | "gravel") {
                    0.3
                } else {
                    0.25
                },
                tail: Some((0.09, 0.07)),
            },
        }
    }
}

/// Per walker: whether its steps may play recorded takes (`enabled`, the default) or are
/// always synthesized. Takes also need [`crate::AudioSettings::recorded_footsteps`] on and the
/// recordings fetched.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordedSteps {
    pub enabled: bool,
}

impl Default for RecordedSteps {
    fn default() -> Self {
        Self { enabled: true }
    }
}

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
    /// How clean and how long it was in its recording.
    pub stats: TakeStats,
}

/// What the cutter measured of a take in its recording, on the 10 ms envelope: dB are
/// relative to the step's peak, so `-floor_db` is its signal-to-noise ratio.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TakeStats {
    /// The recording's noise just before the onset (the quietest tenth of the 0.3 s before
    /// it), dB.
    pub floor_db: f32,
    /// Level of the take's last 100 ms, dB.
    pub tail_db: f32,
    /// How long the step stays within 40 dB of its peak, s.
    pub len_40_s: f32,
    /// The take's length, s.
    pub len_s: f32,
    /// Where its onset is in the recording, s.
    pub at_s: f32,
    /// Whether the take was cleaned of its recording's background noise ([`denoise`]) to
    /// pass [`MIN_SNR_DB`]; then `floor_db` is the noise left after cleaning.
    pub cleaned: bool,
    /// `floor_db` as recorded, before any cleaning, dB.
    pub raw_floor_db: f32,
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

/// Every surface's takes: shoes keyed by surface material name, paws and hooves by kind of
/// ground ([`Feet::ground`]).
#[derive(Debug, Clone, Default)]
pub struct SampleBank {
    /// Shoes and boots.
    pub surfaces: BTreeMap<String, TakeSet>,
    pub paws: BTreeMap<String, TakeSet>,
    pub hooves: BTreeMap<String, TakeSet>,
}

impl SampleBank {
    /// Load every shoe recording under `dir` (`<dir>/<surface>/*.ogg|*.wav`). A missing or
    /// empty folder gives an empty bank; a file that doesn't decode is skipped with a warning.
    pub fn load(dir: &Path) -> Self {
        Self {
            surfaces: load_sets(dir, Feet::Shoes),
            ..Default::default()
        }
    }

    /// Load every kind's recordings under `dir` (usually `<asset root>/`[`AUDIO_DIR`]): its
    /// `footsteps`, `paws` and `hooves` folders.
    pub fn load_all(dir: &Path) -> Self {
        Self {
            surfaces: load_sets(&dir.join(Feet::Shoes.folder()), Feet::Shoes),
            paws: load_sets(&dir.join(Feet::Paws.folder()), Feet::Paws),
            hooves: load_sets(&dir.join(Feet::Hooves.folder()), Feet::Hooves),
        }
    }

    /// One kind's takes, by folder name.
    pub fn sets(&self, feet: Feet) -> &BTreeMap<String, TakeSet> {
        match feet {
            Feet::Shoes => &self.surfaces,
            Feet::Paws => &self.paws,
            Feet::Hooves => &self.hooves,
        }
    }

    fn sets_mut(&mut self, feet: Feet) -> &mut BTreeMap<String, TakeSet> {
        match feet {
            Feet::Shoes => &mut self.surfaces,
            Feet::Paws => &mut self.paws,
            Feet::Hooves => &mut self.hooves,
        }
    }

    /// The folder whose takes `feet` play on `surface`, if it has takes.
    pub fn folder_for(&self, feet: Feet, surface: &str) -> Option<&str> {
        let sets = self.sets(feet);
        let g = feet.ground(surface)?;
        if let Some((k, _)) = sets.get_key_value(g) {
            return Some(k);
        }
        let a = if feet == Feet::Shoes {
            alias(g)?
        } else {
            None?
        };
        sets.get_key_value(a).map(|(k, _)| k.as_str())
    }

    /// Takes `feet` play on `surface` (see [`Feet::ground`]).
    pub fn takes_for(&self, feet: Feet, surface: &str) -> Option<&TakeSet> {
        let f = self.folder_for(feet, surface)?;
        self.sets(feet).get(f)
    }

    /// Takes for a surface material name (or the surface it borrows them from, like `tile`
    /// from `concrete` and `stone` from `rock`).
    pub fn takes(&self, surface: &str) -> Option<&TakeSet> {
        self.takes_for(Feet::Shoes, surface)
    }

    /// Total takes over all surfaces and kinds.
    pub fn len(&self) -> usize {
        Feet::ALL
            .iter()
            .flat_map(|f| self.sets(*f).values())
            .map(|t| t.takes.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Load one kind's recordings: `<dir>/<folder>/*.ogg|*.wav`, cut, the folders left with too
/// few takes dropped and the rest's recordings brought to a common tone.
fn load_sets(dir: &Path, feet: Feet) -> BTreeMap<String, TakeSet> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    for folder in entries.flatten() {
        let Ok(inner) = std::fs::read_dir(folder.path()) else {
            continue;
        };
        let name = folder.file_name().to_string_lossy().into_owned();
        for f in inner.flatten() {
            let p = f.path();
            if matches!(p.extension().and_then(|e| e.to_str()), Some("ogg" | "wav")) {
                files.push((name.clone(), p));
            }
        }
    }
    files.sort();
    // Decode and cut on a few threads (a minute of Vorbis takes a while in a debug build),
    // each taking the next file, the largest first, so they finish together.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 4);
    let mut order: Vec<usize> = (0..files.len()).collect();
    let size = |i: usize| std::fs::metadata(&files[i].1).map(|m| m.len()).unwrap_or(0);
    order.sort_by_key(|&i| std::cmp::Reverse(size(i)));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut done: Vec<(usize, String, Vec<Take>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&i) = order.get(k) else {
                            break;
                        };
                        let (folder, path) = &files[i];
                        match decode(path) {
                            Ok((x, sr)) => {
                                let stem = path
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                let takes = cut_recording(feet, &stem, folder, &x, sr);
                                mine.push((i, folder.clone(), takes));
                            }
                            Err(e) => engine_core::log::warn!(
                                "audio: skipping footstep recording {}: {e}",
                                path.display()
                            ),
                        }
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    // In file order, whichever thread cut them.
    done.sort_by_key(|d| d.0);
    let cut = done.into_iter().map(|(_, folder, takes)| (folder, takes));
    let mut sets: BTreeMap<String, TakeSet> = BTreeMap::new();
    for (folder, takes) in cut {
        sets.entry(folder).or_default().takes.extend(takes);
    }
    // A ground with plenty of takes clean as recorded plays only those; cleaned ones fill in
    // where they are scarce.
    for set in sets.values_mut() {
        if set.takes.iter().filter(|t| !t.stats.cleaned).count() >= CLEAN_ENOUGH {
            set.takes.retain(|t| !t.stats.cleaned);
        }
    }
    // A surface with too few clean takes is synthesized.
    sets.retain(|_, t| t.takes.len() >= MIN_TAKES);
    for set in sets.values_mut() {
        match_sources(&mut set.takes);
    }
    sets
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

/// A surface with no recordings of its own that plays another's, reshaped: the worn-path
/// surfaces ([`crate::footstep::WornPath`]). A folder of its own (`footsteps/path/`,
/// `footsteps/packed_snow/`) takes over from these once it is fetched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Borrowed {
    /// The folder whose takes it plays.
    pub folder: &'static str,
    /// What those takes hold once reshaped, as layers: the synthesis on top adds what the
    /// surface has beyond that, and its tone is measured against it.
    pub holds: SurfaceSound,
    /// Low-pass on the takes, Hz.
    pub lowpass_hz: f32,
    /// Longest a take plays, s (it fades out over [`CUT_FADE_S`] there).
    pub max_s: f32,
}

/// The takes a worn-path surface borrows (see [`Borrowed`]).
///
/// - `path` (bare trodden earth): gravel takes under a 1.8 kHz low-pass, which leaves the
///   boot's thud and the crunch of the fines and takes off the stones' rattle; held to 0.3 s,
///   since a packed path doesn't keep settling the way loose stones do. They then hold a
///   muffled gravel step (half its grit, its thud and scuff), and the synthesis adds the
///   earth's fuller thud, firmer heel and toe scrape.
/// - `packed_snow`: snow takes cut to 0.2 s, so the long soft crunch of powder becomes a short
///   tight one; the tone difference (brighter, a little higher) comes from `packed_snow`'s
///   own brightness and pitch over snow's, and the synthesis adds the squeak and heel.
pub fn borrowed(surface: &str) -> Option<Borrowed> {
    Some(match surface {
        "path" => Borrowed {
            folder: "gravel",
            holds: SurfaceSound {
                thump: 0.55,
                click: 0.05,
                grit: 0.5,
                crunch: 0.1,
                scuff: 0.3,
                brightness: 0.4,
                pitch: 1.0,
                ..SurfaceSound::SILENT
            },
            lowpass_hz: 1_800.0,
            max_s: 0.3,
        },
        "packed_snow" => Borrowed {
            folder: "snow",
            holds: SurfaceSound::preset("snow")?,
            lowpass_hz: 20_000.0,
            max_s: 0.2,
        },
        _ => return None,
    })
}

/// How long a take cut short ([`StepSample::length_s`]) fades out, s.
pub const CUT_FADE_S: f32 = 0.035;

/// Surfaces that sound close enough to borrow another's recordings.
fn alias(surface: &str) -> Option<&'static str> {
    Some(match surface {
        "asphalt" | "tile" => "concrete",
        "stone" => "rock",
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

/// Stretches of one recording: start and end (s) and what they hold.
type Parts = &'static [(f32, f32, Part)];

/// The parts of each known recording worth cutting (seconds), by file name. Other stretches
/// (jumps and landings, scrapes, barefoot) are left out; a recording with none is not used.
/// Recordings not listed are cut whole and their steps labelled by spacing.
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
    // Mud, clean but bright and noise-like (spectral flatness 0.33 and a 4.8 kHz centroid
    // in its steps, against 0.11-0.15 for gravel and sand): a hiss, not a squelch. With the
    // other mud recording too noisy, mud is synthesized.
    ("nox_sound_548384", &[]),
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
    // Stamping through frozen puddles: steps a stride apart, however close the cracks.
    ("jackmenhorn_213578", &[(0.0, 100.6, Part::Walk)]),
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
pub const MAX_CREST_DB: f32 = 20.0;

/// Least signal-to-noise ratio a take needs (its peak over the recording's noise just before
/// it, 10 ms envelopes, dB): below it a take is as much hiss, wind or water as step. A step
/// that misses it is cleaned ([`denoise`]) and has to pass it again on the cleaned recording.
pub const MIN_SNR_DB: f32 = 35.0;

/// How far under the recording's background a cleaned take may follow its step's decay, dB.
const BELOW_BACKGROUND_DB: f32 = 6.0;

/// Least signal-to-noise ratio, as recorded, of a step that gets cleaned to try for
/// [`MIN_SNR_DB`] (dB): under it the background is too much of the step to take out.
pub const RESCUE_MIN_SNR_DB: f32 = 20.0;

/// Takes clean as recorded a ground needs to do without cleaned ones ([`denoise`]): with as
/// many, the cleaned takes of its recordings are left out.
pub const CLEAN_ENOUGH: usize = 24;

/// Fewest clean takes a surface needs to play recordings at all; with fewer it is synthesized,
/// rather than repeating a handful.
pub const MIN_TAKES: usize = 8;

/// Hard ground, where a step is a click (heel, then toe) and a short scuff.
pub fn hard_ground(surface: &str) -> bool {
    matches!(
        surface,
        "concrete"
            | "metal"
            | "wood"
            | "ice"
            | "rock"
            | "glass"
            | "tile"
            | "asphalt"
            | "stone"
            | "planks"
    )
}

/// Longest take on a surface, s: a click and a short scuff on hard ground, a longer squelch,
/// crunch or rustle on soft ground.
pub fn max_take_s(surface: &str) -> f32 {
    if hard_ground(surface) { 0.35 } else { 0.6 }
}

/// On hard ground, how long after the onset a take plays untouched (heel and toe), s, and
/// the time constant of its fade after that, s: what a recording holds on past them (a
/// sustained scuff, the room) dies away instead of standing 20 dB under the click.
const HARD_TAIL_S: (f32, f32) = (0.15, 0.15);

/// How one kind of recording is cut into steps (all times in ms of the 1 ms envelope).
#[derive(Debug, Clone, Copy)]
struct Cutter {
    /// Fixed least time between onsets; `None` takes 60 % of the step period found in the
    /// recording (180-700 ms).
    min_gap_ms: Option<usize>,
    /// How far back the 9 dB jump of an onset is measured from (`None` gap: half of it).
    look_ms: usize,
    /// Where a step's peak is looked for after its onset.
    peak_ms: usize,
    /// Shortest step.
    min_len_ms: usize,
    /// Slowest rise to within 6 dB of the peak: slower is a scrape or shuffle.
    max_rise_ms: usize,
    /// How long the envelope stays down before the step counts as over.
    settle_ms: usize,
    /// Longest take, s.
    max_s: f32,
    /// After how long (s) every take fades, and its time constant (s); `None` only on hard
    /// ground ([`HARD_TAIL_S`]).
    tail: Option<(f32, f32)>,
}

impl Cutter {
    const SHOES: Cutter = Cutter {
        min_gap_ms: None,
        look_ms: 80,
        peak_ms: 150,
        min_len_ms: 90,
        max_rise_ms: 100,
        settle_ms: 50,
        max_s: 0.6,
        tail: None,
    };

    fn with_max(self, max_s: f32) -> Self {
        Self { max_s, ..self }
    }
}

/// Cut a recording of shoes or boots on `surface` into takes (see the [module docs](self)).
pub fn takes_from_recording(name: &str, surface: &str, x: &[f32], sr: f32) -> Vec<Take> {
    cut_recording(Feet::Shoes, name, surface, x, sr)
}

/// Cut a recording of `feet` in `folder` (a surface, or a kind of ground for animals) into
/// takes (see the [module docs](self)).
pub fn cut_recording(feet: Feet, name: &str, folder: &str, x: &[f32], sr: f32) -> Vec<Take> {
    let cutter = feet.cutter(folder);
    let surface = folder;
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
            // With the 0.3 s before the part, so its first step's noise can be measured.
            let from = a.saturating_sub((NOISE_WINDOW_S * sr) as usize);
            let lead_ms = ((a - from) as f32 / sr * 1000.0) as usize;
            cuts.extend(
                cut_steps(&x[from..b], sr, part, &cutter, lead_ms)
                    .into_iter()
                    .map(|mut c| {
                        c.range = from + c.range.start..from + c.range.end;
                        c.onset += from;
                        c.limit += from;
                        c.stats.at_s = c.onset as f32 / sr;
                        c
                    }),
            );
        }
    }
    // Steps that miss the noise bar, and only it, get a second chance: the recording around
    // them is cleaned of its background and they are measured again.
    let (mut cuts, noisy): (Vec<Cut>, Vec<Cut>) = cuts
        .into_iter()
        .partition(|c| c.peak_db - c.noise_db >= MIN_SNR_DB);
    let cleaned = if noisy.is_empty() {
        Vec::new()
    } else {
        let spans: Vec<std::ops::Range<usize>> = noisy.iter().map(|c| c.context(sr)).collect();
        let y = denoise(x, sr, &spans);
        let (d10, _) = envelopes(&y, sr);
        cuts.extend(
            noisy
                .iter()
                .filter_map(|n| rescue(&d10, n, &cutter, sr, y.len())),
        );
        cuts.sort_by_key(|c| c.range.start);
        y
    };
    // Keep the most typical steps of each gait when a recording has plenty, the ones clean
    // as recorded first.
    let mut out = Vec::new();
    for gait in [Gait::Walk, Gait::Run] {
        let mut of: Vec<&Cut> = cuts.iter().filter(|c| c.gait == gait).collect();
        if of.len() > MAX_TAKES {
            // The typical step of those clean as recorded, when there are some.
            let raw = of.iter().any(|c| !c.stats.cleaned);
            let mut peaks: Vec<f32> = of
                .iter()
                .filter(|c| !(raw && c.stats.cleaned))
                .map(|c| c.peak_db)
                .collect();
            peaks.sort_by(f32::total_cmp);
            let median = peaks[peaks.len() / 2];
            of.sort_by(|p, q| {
                (p.stats.cleaned, (p.peak_db - median).abs())
                    .partial_cmp(&(q.stats.cleaned, (q.peak_db - median).abs()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            of.truncate(MAX_TAKES);
            of.sort_by_key(|c| c.range.start);
        }
        for c in of {
            let mut pcm = if c.stats.cleaned {
                cleaned[c.range.clone()].to_vec()
            } else {
                x[c.range.clone()].to_vec()
            };
            if let Some(at) = c.gate_at {
                // A noise gate on a cleaned take's tail: once the step has sunk to the
                // recording's background, it fades out over 15 ms and ends.
                let at = at.min(pcm.len());
                let fade = ((0.015 * sr) as usize).min(pcm.len() - at);
                for j in 0..fade {
                    pcm[at + j] *=
                        0.5 + 0.5 * (std::f32::consts::PI * j as f32 / fade as f32).cos();
                }
                pcm.truncate(at + fade);
                if pcm.len() < (0.02 * sr) as usize {
                    continue;
                }
            }
            expand(&mut pcm, c.floor_db, sr);
            let tail = cutter
                .tail
                .or_else(|| hard_ground(surface).then_some(HARD_TAIL_S));
            if let Some((start, tau)) = tail {
                let from = ((ONSET_PAD_S + start) * sr) as usize;
                for (j, v) in pcm.iter_mut().enumerate().skip(from) {
                    *v *= (-((j - from) as f32) / (tau * sr)).exp();
                }
            }
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
            let mut stats = c.stats;
            stats.len_s = pcm.len() as f32 / sr;
            out.push(Take {
                pcm: pcm.into(),
                sample_rate: sr,
                gait,
                source: source.clone(),
                stats,
            });
        }
    }
    out
}

/// Scale a take to unit [`loudness`], limiting its transient to [`MAX_CREST_DB`] over it
/// first. A take so sharp that limiting keeps eating its loudness ends up a little quieter
/// rather than over the crest. False for silence.
fn normalize(pcm: &mut [f32], sr: f32) -> bool {
    let ceiling = db(MAX_CREST_DB);
    for _ in 0..6 {
        let l = loudness(pcm, sr);
        if l <= 1e-7 {
            return false;
        }
        pcm.iter_mut().for_each(|v| *v /= l);
        if pcm.iter().all(|v| v.abs() <= ceiling * 1.05) {
            return true;
        }
        limit(pcm, ceiling, sr);
    }
    let peak = pcm.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > ceiling {
        pcm.iter_mut().for_each(|v| *v *= ceiling / peak);
    }
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
    /// The noise floor the take's background is expanded down from (10 ms envelope), dB:
    /// the recording's, or for a cleaned take the noise left before it.
    floor_db: f32,
    /// The recording's noise just before the step ([`noise_before`]), dB.
    noise_db: f32,
    /// Onset, and the latest the step may end (the next step, or the longest take), samples.
    onset: usize,
    limit: usize,
    /// Where a cleaned take's tail gate closes, samples from the take's start.
    gate_at: Option<usize>,
    stats: TakeStats,
}

impl Cut {
    /// The stretch of recording a cleanup of this step needs: its noise window (and a little
    /// more, so the cleaned window starts settled) up to its latest end, samples.
    fn context(&self, sr: f32) -> std::ops::Range<usize> {
        self.onset
            .saturating_sub(((NOISE_WINDOW_S + 0.15) * sr) as usize)
            ..self.limit + (0.02 * sr) as usize
    }
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
/// 9 dB over its level since the last step, at most once per 60 % of the step period, and
/// ends once the envelope has fallen to within 6 dB of the noise before the step, or 40 dB
/// under its peak, and stayed there (at most `c.max_s` long). Steps much quieter or louder
/// than the median, too short, far too noisy (under [`RESCUE_MIN_SNR_DB`]; between it and
/// [`MIN_SNR_DB`] they are only candidates for a cleanup) or swelling slowly (scrapes) are
/// dropped, and so are steps without a full [`NOISE_WINDOW_S`] of recording before them to
/// measure the noise in (the first step of a file, or the first after digital silence that
/// only opens the file): their noise can't be told. Onsets in the first `lead_ms` (the
/// recording before the part) are only context. Animals' paws and hooves use their own,
/// shorter times ([`Cutter`]).
fn cut_steps(x: &[f32], sr: f32, part: Part, c: &Cutter, lead_ms: usize) -> Vec<Cut> {
    let hop = (sr * 0.001).max(1.0) as usize;
    let (d10, d2) = envelopes(x, sr);
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
    let (min_gap, look) = match c.min_gap_ms {
        Some(gap) => (gap, c.look_ms),
        None => {
            let gap = ((period * 600.0) as usize).clamp(180, 700);
            (gap, (gap / 2).max(c.look_ms))
        }
    };
    // Where the recording starts: after any digital silence it opens with.
    let opens = d10.iter().position(|v| *v > -120.0).unwrap_or(d10.len());
    let measurable = opens + (NOISE_WINDOW_S * 1000.0) as usize;
    let mut onsets: Vec<usize> = Vec::new();
    let mut k = lead_ms.max(5);
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
    let max_ms = (c.max_s * 1000.0) as usize;
    let max_len = match c.min_gap_ms {
        Some(_) => max_ms,
        None => ((period * 1100.0) as usize).clamp(250, 700).min(max_ms),
    };
    let pad = (ONSET_PAD_S * 1000.0) as usize;
    let gait = match part {
        Part::Walk => Gait::Walk,
        Part::Run => Gait::Run,
        Part::Steps if period < 0.42 && c.min_gap_ms.is_none() => Gait::Run,
        Part::Steps => Gait::Walk,
    };
    let mut steps = Vec::new();
    for (i, &o) in onsets.iter().enumerate() {
        let next = onsets.get(i + 1).copied().unwrap_or(d10.len());
        let limit = (next.saturating_sub(8)).min(o + max_len).min(d10.len());
        if limit <= o + 20 {
            continue;
        }
        let (end, peak, rise, noise) = measure_step(&d10, o, limit, c, f32::MIN);
        steps.push((o, end, peak, rise, noise, limit));
    }
    if steps.is_empty() {
        return Vec::new();
    }
    let mut peaks: Vec<f32> = steps.iter().map(|s| s.2).collect();
    peaks.sort_by(f32::total_cmp);
    let median = peaks[peaks.len() / 2];
    steps
        .into_iter()
        .filter(|&(o, end, peak, rise, noise, _)| {
            o >= measurable
                && end - o >= c.min_len_ms
                && rise <= c.max_rise_ms
                && peak > median - 10.0
                && peak < median + 8.0
                && peak - noise >= RESCUE_MIN_SNR_DB
        })
        .map(|(o, end, peak, _, noise, limit)| {
            let range = o.saturating_sub(pad) * hop..(end * hop).min(x.len());
            let mut stats = take_stats(&d10, o, end, peak, range.len() as f32 / sr);
            stats.raw_floor_db = stats.floor_db;
            Cut {
                stats,
                range,
                gait,
                peak_db: peak,
                floor_db: floor,
                noise_db: noise,
                onset: o * hop,
                limit: (limit * hop).min(x.len()),
                gate_at: None,
            }
        })
        .filter(|c| c.range.end > c.range.start)
        .collect()
}

/// How much recording before a step its noise is measured over, s.
pub const NOISE_WINDOW_S: f32 = 0.3;

/// The recording's noise just before onset `o`: the quietest tenth of the 10 ms envelope
/// `d10` (1 ms hops) over the [`NOISE_WINDOW_S`] before it, dB (-140 in digital silence).
fn noise_before(d10: &[f32], o: usize) -> f32 {
    let w = (NOISE_WINDOW_S * 1000.0) as usize;
    let mut before = d10[o.saturating_sub(w)..o.saturating_sub(5)].to_vec();
    before.sort_by(f32::total_cmp);
    before.get(before.len() / 10).copied().unwrap_or(-140.0)
}

/// The high-passed (60 Hz) level of `x` in dB at 1 ms hops: over 10 ms windows, and over 2 ms.
fn envelopes(x: &[f32], sr: f32) -> (Vec<f32>, Vec<f32>) {
    let hop = (sr * 0.001).max(1.0) as usize;
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
    (env(0.010), env(0.002))
}

/// Measure the step with onset `o` on the 10 ms envelope `d10` (1 ms hops), at most until
/// `limit`: where it ends, its peak (dB), how long it takes to come within 6 dB of the peak
/// (ms) and the noise before it (dB). It ends where it falls to just over the noise before
/// it, or over `background` (dB) when that is louder (a cleaned recording's step ends where
/// it sank into the background as recorded).
fn measure_step(
    d10: &[f32],
    o: usize,
    limit: usize,
    c: &Cutter,
    background: f32,
) -> (usize, f32, usize, f32) {
    let limit = limit.min(d10.len());
    let peak = d10[o.min(limit)..(o + c.peak_ms).min(limit)]
        .iter()
        .fold(f32::MIN, |m, v| m.max(*v));
    // The step is over once the envelope has fallen to just over the noise before it (or
    // 40 dB under the peak) and stayed down for 50 ms (less for a paw or a hoof): what
    // follows is the recording's hiss, wind or room, not the step.
    let noise = noise_before(d10, o);
    let quiet = (noise.max(background) + 6.0).max(peak - 40.0);
    let mut end = limit;
    let mut below = 0;
    for (j, &level) in d10.iter().enumerate().take(limit).skip(o + 10) {
        if level < quiet {
            below += 1;
            if below >= c.settle_ms {
                end = j + 1 - below;
                break;
            }
        } else {
            below = 0;
        }
    }
    let end = (end.max(o + c.min_len_ms.saturating_sub(10)) + 15).min(limit);
    // Slow swells (a scrape, a shuffle) are not steps: nothing near the peak comes early.
    // (A step's toe or compaction may well be louder than its heel.)
    let rise = (o..(o + c.peak_ms).min(limit))
        .position(|j| d10[j] >= peak - 6.0)
        .unwrap_or(0);
    (end, peak, rise, noise)
}

/// Measure a step that missed [`MIN_SNR_DB`] again on its cleaned recording (`d10`, the
/// cleaned 10 ms envelope; `len` samples): the take it gives if it now passes and is still a
/// step (as long and as sharp as one). It ends about where it falls back into the background
/// as recorded ([`BELOW_BACKGROUND_DB`] under it), and a gate closes its tail there.
fn rescue(d10: &[f32], n: &Cut, c: &Cutter, sr: f32, len: usize) -> Option<Cut> {
    let hop = (sr * 0.001).max(1.0) as usize;
    let (o, limit) = (n.onset / hop, n.limit / hop);
    if o + 20 >= limit.min(d10.len()) {
        return None;
    }
    // What of the step stood over the background as recorded, and a little under it that the
    // cleaning brings out: further down it is as much the background's remains as step.
    let background = n.noise_db - BELOW_BACKGROUND_DB;
    let (end, peak, rise, noise) = measure_step(d10, o, limit, c, background);
    if peak - noise < MIN_SNR_DB || end - o < c.min_len_ms || rise > c.max_rise_ms {
        return None;
    }
    let pad = (ONSET_PAD_S * 1000.0) as usize;
    let start = o.saturating_sub(pad);
    let last = (o..end)
        .rev()
        .find(|&j| d10[j] >= background.max(noise + 10.0))
        .unwrap_or(o);
    let gate = ((last + 5).max(o + c.min_len_ms.saturating_sub(15)) - start) * hop;
    let range = start * hop..(end * hop).min(len);
    let mut stats = take_stats(d10, o, end, peak, range.len() as f32 / sr);
    stats.cleaned = true;
    stats.raw_floor_db = n.stats.floor_db;

    stats.at_s = n.stats.at_s;
    Some(Cut {
        stats,
        range,
        gait: n.gait,
        peak_db: peak,
        floor_db: noise,
        noise_db: noise,
        onset: n.onset,
        limit: n.limit,
        gate_at: Some(gate),
    })
}

/// [`TakeStats`] of the step from `o` to `end` (1 ms hops of the 10 ms envelope `d10`).
fn take_stats(d10: &[f32], o: usize, end: usize, peak: f32, len_s: f32) -> TakeStats {
    let floor = noise_before(d10, o);
    let tail = &d10[end.saturating_sub(100).max(o)..end];
    let tail_ms = tail.iter().map(|v| 10f32.powf(v / 10.0)).sum::<f32>() / tail.len().max(1) as f32;
    let last = (o..end).rev().find(|&j| d10[j] >= peak - 40.0).unwrap_or(o);
    TakeStats {
        floor_db: floor - peak,
        tail_db: 10.0 * (tail_ms + 1e-14).log10() - peak,
        len_40_s: (last - o) as f32 * 0.001,
        len_s,
        ..Default::default()
    }
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

// ---------------------------------------------------------------- cleaning noisy recordings

/// How far the cleanup turns a recording's background down at most, dB: a floor, so what is
/// left is the same background quieter rather than the warbling ("musical noise") that
/// subtracting all of it leaves.
pub const DENOISE_FLOOR_DB: f32 = -20.0;

/// How many times its estimated noise a bin's power must be before the cleanup lets any of it
/// through: over-subtraction, so the estimate's own scatter doesn't poke through as tones.
const OVERSUBTRACT: f32 = 2.0;

/// How fast a bin's gain may close after a sound, dB/s: 20 dB in 50 ms, so a step's decay
/// is kept whole.
const DENOISE_RELEASE_DB_S: f32 = 400.0;

/// Time constant of the power each bin's gain is decided on, s: a few frames, so one
/// random peak of the noise doesn't open it, while a step's onset still does at once.
const DENOISE_SMOOTH_S: f32 = 0.01;

/// How much recording on each side of a moment its noise is estimated from, s.
const NOISE_ESTIMATE_S: f32 = 1.5;

/// The share of that recording, its quietest frames, the noise is estimated from.
const NOISE_QUANTILE: f32 = 0.2;

/// Clean a recording's background (rain, wind, room tone, hiss, distant traffic) out of the
/// sample ranges `spans` of `x`; the rest is returned as it was. A short-time Fourier
/// transform (frames of about 11 ms, half overlapped, square-root Hann windows, so what
/// passes untouched comes back exactly; two frames per complex transform) and a Wiener-style
/// gain per frequency bin: the noise in each bin is estimated from the quietest fifth of the
/// frames within 1.5 s (the gaps between steps), a bin well over it passes, one near it is
/// turned down to [`DENOISE_FLOOR_DB`] at most. The power a gain is decided on is smoothed
/// over neighbouring bins and a few frames, and a gain closes at most 20 dB in 50 ms after a
/// sound, so a step and its decay come through whole. Noise that is not steady (a bird's
/// call, a voice, a car passing close) is not taken out.
pub fn denoise(x: &[f32], sr: f32, spans: &[std::ops::Range<usize>]) -> Vec<f32> {
    let mut y = x.to_vec();
    let n = if sr > 64_000.0 { 1024 } else { 512 };
    let hop = n / 2;
    let bins = n / 2 + 1;
    let mut fft = Fft::new(n);
    let window: Vec<f32> = (0..n)
        .map(|i| (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).sqrt())
        .collect();
    let floor = db(DENOISE_FLOOR_DB);
    let smooth = (-(hop as f32) / (DENOISE_SMOOTH_S * sr)).exp();
    let release = db(-DENOISE_RELEASE_DB_S * hop as f32 / sr);
    // Bins under 60 Hz (handling rumble, already high-passed) don't decide what is quiet.
    let low = ((60.0 / sr * n as f32) as usize).max(1);
    let mut p = vec![0.0f32; bins];
    for span in merge_spans(spans, x.len()) {
        // The noise: frames (not overlapped) over the span and 1.5 s on each side, the
        // quietest of those around each half second.
        let e0 = span.start.saturating_sub((NOISE_ESTIMATE_S * sr) as usize);
        let e1 = (span.end + (NOISE_ESTIMATE_S * sr) as usize).min(x.len());
        let starts: Vec<usize> = (e0..(e1 + 1).saturating_sub(n)).step_by(n).collect();
        if starts.len() < 8 {
            continue;
        }
        let mut frames: Vec<(f32, f32, Vec<f32>)> = Vec::with_capacity(starts.len());
        for pair in starts.chunks(2) {
            let second = pair.get(1).map(|&b| &x[b..b + n]);
            fft.forward_pair(&x[pair[0]..pair[0] + n], second, &window);
            for (j, &at) in pair.iter().enumerate() {
                fft.power(j, &mut p);
                let energy = p[low..].iter().sum::<f32>();
                frames.push(((at + n / 2) as f32, energy, p.clone()));
            }
        }
        let block = (0.5 * sr) as usize;
        let reach = NOISE_ESTIMATE_S * sr;
        let centres: Vec<f32> = (span.start..span.end + block)
            .step_by(block)
            .map(|c| c as f32)
            .collect();
        let estimates: Vec<Vec<f32>> = centres
            .iter()
            .map(|&c| {
                let mut near: Vec<&(f32, f32, Vec<f32>)> =
                    frames.iter().filter(|f| (f.0 - c).abs() <= reach).collect();
                near.sort_by(|a, b| a.1.total_cmp(&b.1));
                let k = ((near.len() as f32 * NOISE_QUANTILE) as usize).clamp(1, near.len());
                let mut mean = vec![0.0f32; bins];
                for f in &near[..k] {
                    for (m, v) in mean.iter_mut().zip(&f.2) {
                        *m += v / k as f32;
                    }
                }
                // Smoothed across five bins: the estimate of a few frames scatters.
                (0..bins)
                    .map(|b| {
                        let (lo, hi) = (b.saturating_sub(2), (b + 3).min(bins));
                        (mean[lo..hi].iter().sum::<f32>() / (hi - lo) as f32).max(1e-20)
                    })
                    .collect()
            })
            .collect();
        // The span with a frame of recording on each side, so its overlap-add is complete.
        let a = span.start.saturating_sub(n);
        let b = (span.end + n).min(x.len());
        let mut padded = vec![0.0f32; n];
        padded.extend_from_slice(&x[a..b]);
        padded.extend(std::iter::repeat_n(0.0, 2 * n));
        let mut out = vec![0.0f32; padded.len()];
        let mut held = vec![0.0f32; bins];
        let mut last = vec![floor; bins];
        let mut gains = [vec![floor; bins], vec![floor; bins]];
        let starts: Vec<usize> = (0..padded.len() + 1 - n).step_by(hop).collect();
        for pair in starts.chunks(2) {
            let second = pair.get(1).map(|&s| &padded[s..s + n]);
            fft.forward_pair(&padded[pair[0]..pair[0] + n], second, &window);
            for (j, &f0) in pair.iter().enumerate() {
                // Which noise estimate: between the two nearest half-second blocks.
                let t = (a + f0 + n / 2) as f32 - n as f32;
                let pos = ((t - centres[0]) / block as f32).clamp(0.0, (centres.len() - 1) as f32);
                let i0 = pos.floor() as usize;
                let i1 = (i0 + 1).min(centres.len() - 1);
                let w = pos - i0 as f32;
                fft.power(j, &mut p);
                for (h, v) in held.iter_mut().zip(&p) {
                    *h = *h * smooth + v * (1.0 - smooth);
                }
                let g = &mut gains[j];
                for k in 0..bins {
                    let (lo, hi) = (k.saturating_sub(1), (k + 2).min(bins));
                    let power = held[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
                    let noise = estimates[i0][k] * (1.0 - w) + estimates[i1][k] * w;
                    let xi = (power / noise - OVERSUBTRACT).max(0.0);
                    g[k] = (xi / (1.0 + xi)).max(last[k] * release).max(floor);
                }
                last.copy_from_slice(g);
            }
            fft.inverse_pair(&gains[0], &gains[1]);
            for (j, &f0) in pair.iter().enumerate() {
                // Square-root Hann twice, half overlapped, sums to 1.
                let frame = if j == 0 { &fft.re } else { &fft.im };
                for (o, (v, w)) in out[f0..f0 + n].iter_mut().zip(frame.iter().zip(&window)) {
                    *o += v * w;
                }
            }
        }
        for (i, v) in y[span.clone()].iter_mut().enumerate() {
            *v = out[n + span.start - a + i];
        }
    }
    y
}

/// `spans` clipped to `0..len`, sorted and with overlapping ones joined.
fn merge_spans(spans: &[std::ops::Range<usize>], len: usize) -> Vec<std::ops::Range<usize>> {
    let mut s: Vec<std::ops::Range<usize>> = spans
        .iter()
        .map(|r| r.start.min(len)..r.end.min(len))
        .filter(|r| r.end > r.start)
        .collect();
    s.sort_by_key(|r| r.start);
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for r in s {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// A radix-2 complex FFT of one size (a power of two), with its own buffers, used two real
/// frames at a time (one as the real part, one as the imaginary).
struct Fft {
    n: usize,
    /// cos and sin of -2πk/n for k < n/2.
    twiddle: Vec<(f32, f32)>,
    reverse: Vec<usize>,
    re: Vec<f32>,
    im: Vec<f32>,
    scratch: (Vec<f32>, Vec<f32>),
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        Self {
            n,
            twiddle: (0..n / 2)
                .map(|k| {
                    let a = -std::f64::consts::TAU * k as f64 / n as f64;
                    (a.cos() as f32, a.sin() as f32)
                })
                .collect(),
            reverse: (0..n)
                .map(|i| i.reverse_bits() >> (usize::BITS - bits))
                .collect(),
            re: vec![0.0; n],
            im: vec![0.0; n],
            scratch: (vec![0.0; n], vec![0.0; n]),
        }
    }

    /// Transform the buffers in place; `inverse` also divides by `n`.
    fn run(&mut self, inverse: bool) {
        let n = self.n;
        let (re, im) = (&mut self.re, &mut self.im);
        for i in 0..n {
            let j = self.reverse[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            let half = len / 2;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (c, s) = self.twiddle[k * step];
                    let s = if inverse { -s } else { s };
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
        if inverse {
            let k = 1.0 / n as f32;
            re.iter_mut().for_each(|v| *v *= k);
            im.iter_mut().for_each(|v| *v *= k);
        }
    }

    /// The spectra of `a` and `b` (zeros without), each windowed by `window`, together.
    fn forward_pair(&mut self, a: &[f32], b: Option<&[f32]>, window: &[f32]) {
        for i in 0..self.n {
            self.re[i] = a[i] * window[i];
            self.im[i] = b.map_or(0.0, |b| b[i] * window[i]);
        }
        self.run(false);
    }

    /// Bin `k`'s value in the spectrum of frame `j` (0 or 1) of the last [`Fft::forward_pair`].
    #[inline]
    fn bin(&self, j: usize, k: usize) -> (f32, f32) {
        let m = (self.n - k) % self.n;
        let (zr, zi, mr, mi) = (self.re[k], self.im[k], self.re[m], self.im[m]);
        if j == 0 {
            (0.5 * (zr + mr), 0.5 * (zi - mi))
        } else {
            (0.5 * (zi + mi), -0.5 * (zr - mr))
        }
    }

    /// The power spectrum (bins 0..=n/2) of frame `j` of the last [`Fft::forward_pair`].
    fn power(&self, j: usize, out: &mut [f32]) {
        for (k, o) in out.iter_mut().enumerate() {
            let (r, i) = self.bin(j, k);
            *o = r * r + i * i;
        }
    }

    /// Scale the two spectra of the last [`Fft::forward_pair`] by real gains per bin (0..=n/2)
    /// and transform back: the first frame ends up in `re`, the second in `im`.
    fn inverse_pair(&mut self, g0: &[f32], g1: &[f32]) {
        let n = self.n;
        let (mut yr, mut yi) = std::mem::take(&mut self.scratch);
        for k in 0..n {
            let kk = k.min(n - k);
            let (ar, ai) = self.bin(0, k);
            let (br, bi) = self.bin(1, k);
            yr[k] = g0[kk] * ar - g1[kk] * bi;
            yi[k] = g0[kk] * ai + g1[kk] * br;
        }
        std::mem::swap(&mut self.re, &mut yr);
        std::mem::swap(&mut self.im, &mut yi);
        self.scratch = (yr, yi);
        self.run(true);
    }
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
    /// Longest it plays, s (of the take as recorded): a take cut short fades out over
    /// [`CUT_FADE_S`] there. `f32::INFINITY` plays it whole.
    pub length_s: f32,
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
    /// Where it stops (of `pcm`), and over how many samples before that it fades out.
    end: usize,
    fade_len: f64,
}

impl SampleVoice {
    pub(crate) fn new(fs: f32, s: &StepSample) -> Self {
        let nyq = fs * 0.45;
        let n = s.take.pcm.len();
        let cut = (s.length_s.max(0.0) * s.take.sample_rate) as f64;
        let (end, fade_len) = if cut < n as f64 {
            let fade = (CUT_FADE_S * s.take.sample_rate) as f64;
            (cut as usize, fade.min(cut * 0.5).max(1.0))
        } else {
            (n, 96.0)
        };
        Self {
            end,
            fade_len,
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
        ((self.end as f64 - self.pos).max(0.0) / self.inc) as u32
    }

    pub(crate) fn active(&self) -> bool {
        (self.pos as usize) + 2 < self.end
    }

    #[inline]
    pub(crate) fn next(&mut self) -> f32 {
        let i = self.pos as usize;
        let n = self.end;
        if i + 2 >= n {
            return 0.0;
        }
        let t = (self.pos - i as f64) as f32;
        let p = &self.pcm;
        let p0 = if i > 0 { p[i - 1] } else { p[i] };
        let v = hermite(p0, p[i], p[i + 1], p[i + 2], t);
        self.pos += self.inc;
        // A short fade over the last few milliseconds, so a cut tail never clicks (longer
        // where the take is cut short).
        let left = n as f64 - self.pos;
        if left < self.fade_len {
            self.fade = (left / self.fade_len).max(0.0) as f32;
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

    /// Start loading `dir` (usually `<asset root>/`[`AUDIO_DIR`], see
    /// [`SampleBank::load_all`]) on a background thread.
    pub fn start_loading(&mut self, dir: PathBuf) {
        let (tx, rx) = bounded(1);
        let d = dir.clone();
        let spawned = std::thread::Builder::new()
            .name("footstep-samples".into())
            .spawn(move || {
                let _ = tx.send(SampleBank::load_all(&d));
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
                    "audio: {} recorded footstep takes from {dir}: shoes on {} surfaces, \
                     paws on {} kinds of ground, hooves on {}",
                    bank.len(),
                    bank.surfaces.len(),
                    bank.paws.len(),
                    bank.hooves.len()
                );
            }
            self.load = Load::Ready(bank);
        }
        match &mut self.load {
            Load::Ready(b) => Some(b),
            _ => None,
        }
    }

    /// Takes per surface, for status reports (empty while loading): shoes by surface name,
    /// paws and hooves as `paws/<ground>` and `hooves/<ground>`.
    pub fn summary(&self) -> BTreeMap<String, usize> {
        match &self.load {
            Load::Ready(b) => Feet::ALL
                .iter()
                .flat_map(|f| {
                    b.sets(*f).iter().map(move |(k, v)| {
                        let name = match f {
                            Feet::Shoes => k.clone(),
                            _ => format!("{}/{k}", f.folder()),
                        };
                        (name, v.takes.len())
                    })
                })
                .collect(),
            _ => BTreeMap::new(),
        }
    }

    /// How a footstep on `material` sounds with a recording: the take to play, and the step
    /// for the synthesized layers on top (only what the recording lacks). `None` when there
    /// is no fitting take, so the step is synthesized whole. Shoes, paws and hooves each play
    /// their own recordings ([`Feet`]); wheels, tracks and falling bodies are always
    /// synthesized.
    ///
    /// `step` is the step as it would be synthesized (its surface already changed by the
    /// worn path and the ground's state), `dry` the material's own sound, `worn` the path worn
    /// into it ([`FootstepSurfaces::worn_path`]; [`WornPath::NONE`] for none), `cell` and
    /// `wading_m` the ground's state, and `wait` blocks for a bank that is still loading.
    ///
    /// On a worn path a share of the steps (`worn.t`, rising smoothly with the wear) play the
    /// path's recordings instead of the ground's: bare earth's (borrowed, see [`borrowed`]),
    /// mud's on a share `worn.mud` of those as the ground gets wet, packed snow's on trodden
    /// snow. Whichever plays, the layers on top add what the blended step has beyond it, so
    /// both kinds of step sound like the same half-worn ground.
    #[allow(clippy::too_many_arguments)]
    pub fn recorded(
        &mut self,
        material: &str,
        dry: &SurfaceSound,
        worn: &WornPath,
        cell: &SurfaceCell,
        wading_m: f32,
        surfaces: &FootstepSurfaces,
        step: &Step,
        fs: f32,
        wait: bool,
    ) -> Option<(Step, StepSample)> {
        let feet = Feet::of(step.kind)?;
        // What the ground sounds like now decides which recordings fit: deep fresh snow
        // covers anything, and a puddle (or wading) is water whatever is under it.
        let snow = surfaces.snow();
        let cover = smooth(0.005, 0.07, cell.snow_m);
        let puddle = smooth(0.002, 0.03, cell.puddle_m);
        let mut path_rng = Noise::new(step.seed ^ 0x9A7B_0E4A_2D11_C3F5);
        let (ground, path, path_t) = if cover >= 0.5 && dry.crunch < snow.crunch {
            (("snow", snow), "packed_snow", worn.packed)
        } else if (puddle >= 0.5 || wading_m > 0.05) && material != "water" {
            (("water", surfaces.named("water")), "", 0.0)
        } else {
            let path = if worn.path == "path" && path_rng.chance(worn.mud) {
                "mud"
            } else {
                worn.path
            };
            ((material, *dry), path, worn.t)
        };
        // Ground first, or the path's on a share `path_t` of the steps; the other if the first
        // has no fitting takes. Untouched ground never tries the path's.
        let path = (path_t > 0.0).then(|| (path, surfaces.named(path)));
        let order = match path {
            Some(p) if path_rng.chance(path_t) => [Some(p), Some(ground)],
            p => [Some(ground), p],
        };
        for (name, base) in order.into_iter().flatten() {
            if let Some(r) = self.recorded_as(feet, name, &base, step, surfaces, fs, wait) {
                return Some(r);
            }
        }
        None
    }

    /// [`Self::recorded`] with takes of the surface `name` (whose own sound is `base`), if it
    /// (or the surface it borrows from) has fitting takes.
    #[allow(clippy::too_many_arguments)]
    fn recorded_as(
        &mut self,
        feet: Feet,
        name: &str,
        base: &SurfaceSound,
        step: &Step,
        surfaces: &FootstepSurfaces,
        fs: f32,
        wait: bool,
    ) -> Option<(Step, StepSample)> {
        let bank = self.bank(wait)?;
        let (folder, borrow) = match bank.folder_for(feet, name) {
            Some(f) => (f.to_string(), None),
            None if feet == Feet::Shoes => {
                let b = borrowed(name)?;
                (bank.folder_for(feet, b.folder)?.to_string(), Some(b))
            }
            None => return None,
        };
        // The surface the recording was made on: for shoes the one it plays on (or its
        // look-alike, or what a borrowed take holds); for animals the one their kind of
        // ground was recorded on.
        let (recorded, tone_of) = match (&borrow, feet) {
            (Some(b), _) => (b.holds, b.holds),
            (None, Feet::Shoes) => (*base, *base),
            (None, _) => (surfaces.named(feet.recorded_on(&folder)), *base),
        };
        let state = step.surface;
        let kept = state.kept(&recorded);
        if kept < 0.4 {
            // The ground has changed too much (frozen mud is not mud, gravel is not a lawn):
            // synthesize it.
            return None;
        }
        let reference = self.reference_loudness(feet, step.kind, name, base, fs);
        let bank = self.bank(false)?;
        let mut rng = Noise::new(step.seed ^ 0x5EED_FA11_0F57_E950);
        let run = ((0.11 - step.roll) / 0.06).clamp(0.0, 1.0);
        let take = bank
            .sets_mut(feet)
            .get_mut(&folder)?
            .pick(run, &mut rng)?
            .clone();

        let i = step.intensity.max(0.05);
        // The synthesized level law (gain ∝ i^0.8, strikes ∝ √i up to 2) from its loudness at
        // the reference step (the walker the recordings are of: a 75 kg person, one paw of a
        // 22 kg dog, one hoof of a 500 kg horse), plus ±1.5 dB per step.
        let law = |i: f32| i.powf(0.8) * i.min(2.0).sqrt();
        let i0 = feet.recorded_intensity();
        let gain = reference * law(i) / law(i0) * kept.sqrt() * db(rng.range(-1.5, 1.5));
        // Tone follows how hard the step is against that walker's.
        let i_rel = i / i0;
        let pitch = (state.pitch / tone_of.pitch.max(0.05)).clamp(0.7, 1.4);
        let speed = pitch * 2f32.powf(rng.range(-0.6, 0.6) / 12.0);
        let tone = state.brightness / tone_of.brightness.max(0.05);
        let shelf_db = (4.5 * i_rel.log2()).clamp(-6.0, 4.0) + (8.0 * tone.log2()).clamp(-9.0, 3.0);
        let mut lowpass_hz = if tone < 1.0 {
            (20_000.0 * tone.powf(2.5)).max(1_200.0)
        } else {
            20_000.0
        };
        let mut length_s = f32::INFINITY;
        if let Some(b) = &borrow {
            lowpass_hz = lowpass_hz.min(b.lowpass_hz);
            length_s = b.max_s;
        }
        // Synthesize what the recording lacks, plus a little of what it has: weight (a
        // heavier walker thumps more) and per-step grain.
        let mut over = step.surface.over_recording(&recorded, 0.12);
        over.thump += recorded.thump.min(state.thump) * (0.25 * (i_rel - 1.0)).clamp(0.0, 0.5);
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
                length_s,
            },
        ))
    }
    /// Loudness of the synthesized reference step of `kind` on `surface`, as hard as the
    /// steps in `feet`'s recordings (cached until its sound is changed in
    /// [`FootstepSurfaces`]).
    fn reference_loudness(
        &mut self,
        feet: Feet,
        kind: ContactKind,
        name: &str,
        sound: &SurfaceSound,
        fs: f32,
    ) -> f32 {
        let key = format!("{}/{name}", feet.folder());
        if let Some((s, l)) = self.reference.get(&key)
            && s == sound
        {
            return *l;
        }
        let l = crate::footstep::reference_loudness_of(sound, kind, feet.recorded_intensity(), fs);
        self.reference.insert(key, (*sound, l));
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
    samples.start_loading(root.join(AUDIO_DIR));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footstep::SurfaceSound;

    const SR: f32 = 48_000.0;

    /// A fake recording: a click-and-decay "step" every `gap` seconds over a noise floor,
    /// the first after 0.4 s of it.
    fn steps(n: usize, gap: f32, seed: u64) -> Vec<f32> {
        taps(n, gap, 0.3, 25.0, seed)
    }

    /// `n` noise bursts `gap` s apart, each `len` s long decaying at `rate` /s, over a quiet
    /// floor, the first after 0.4 s.
    fn taps(n: usize, gap: f32, len: f32, rate: f32, seed: u64) -> Vec<f32> {
        let mut rng = Noise::new(seed);
        let len_n = ((n as f32 * gap + 0.7) * SR) as usize;
        let mut x: Vec<f32> = (0..len_n).map(|_| rng.white() * 0.0005).collect();
        for k in 0..n {
            let at = ((0.4 + k as f32 * gap) * SR) as usize;
            for j in 0..(len * SR) as usize {
                if let Some(v) = x.get_mut(at + j) {
                    let t = j as f32 / SR;
                    *v += rng.white() * 0.3 * (-t * rate).exp();
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
            stats: TakeStats::default(),
        }
    }

    #[test]
    fn a_recording_is_cut_into_one_take_per_step() {
        let x = steps(8, 0.6, 1);
        let takes = takes_from_recording("unlisted", "grass", &x, SR);
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
        let run = takes_from_recording("unlisted", "grass", &steps(8, 0.33, 2), SR);
        assert!(run.len() >= 7 && run.iter().all(|t| t.gait == Gait::Run));
    }

    /// `x` with white hiss of amplitude `level` added.
    fn hiss(x: &[f32], level: f32, seed: u64) -> Vec<f32> {
        let mut rng = Noise::new(seed);
        x.iter().map(|v| v + rng.white() * level).collect()
    }

    #[test]
    fn noisy_steps_are_cleaned_or_left_out_and_takes_end_with_the_step() {
        // The same steps over hiss about 23 dB under them: cleaned, they pass.
        let noisy = hiss(&steps(8, 0.6, 7), 0.02, 9);
        let takes = takes_from_recording("unlisted", "grass", &noisy, SR);
        assert_eq!(takes.len(), 8);
        for t in &takes {
            assert!(t.stats.cleaned, "{:?}", t.stats);
            assert!(t.stats.raw_floor_db > -MIN_SNR_DB + 5.0, "{:?}", t.stats);
            assert!(t.stats.floor_db <= -MIN_SNR_DB, "{:?}", t.stats);
        }
        // Hiss 14 dB under the steps is too much of them to take out: no takes.
        let noisier = hiss(&steps(8, 0.6, 7), 0.06, 9);
        assert!(takes_from_recording("unlisted", "grass", &noisier, SR).is_empty());
        // Clean steps end where the step does, not at the next one.
        for t in &takes_from_recording("unlisted", "grass", &steps(8, 0.6, 7), SR) {
            assert!(t.stats.floor_db < -MIN_SNR_DB);
            assert!(t.stats.len_s < 0.3, "{:?}", t.stats);
        }
        // Long ringing steps are capped, shorter on hard ground.
        let mut rng = Noise::new(10);
        let mut x: Vec<f32> = (0..(7.0 * SR) as usize)
            .map(|_| rng.white() * 0.0005)
            .collect();
        for k in 0..8 {
            let at = ((0.4 + k as f32 * 0.8) * SR) as usize;
            for j in 0..(0.75 * SR) as usize {
                if let Some(v) = x.get_mut(at + j) {
                    *v += rng.white() * 0.3 * (-(j as f32 / SR) * 4.0).exp();
                }
            }
        }
        let soft = takes_from_recording("unlisted", "grass", &x, SR);
        let hard = takes_from_recording("unlisted", "concrete", &x, SR);
        assert!(soft.len() >= 6 && hard.len() >= 6);
        for t in &soft {
            assert!((0.45..=0.62).contains(&t.stats.len_s), "{:?}", t.stats);
        }
        for t in &hard {
            assert!(t.stats.len_s <= 0.37, "{:?}", t.stats);
        }
    }

    #[test]
    fn the_fft_transforms_two_frames_and_back() {
        let n = 64;
        let mut rng = Noise::new(3);
        let a: Vec<f32> = (0..n).map(|_| rng.white()).collect();
        let b: Vec<f32> = (0..n).map(|_| rng.white()).collect();
        let ones = vec![1.0; n];
        let mut fft = Fft::new(n);
        fft.forward_pair(&a, Some(&b), &ones);
        // Each frame's power spectrum is that of a direct DFT.
        for (j, x) in [&a, &b].into_iter().enumerate() {
            let mut p = vec![0.0; n / 2 + 1];
            fft.power(j, &mut p);
            for (k, got) in p.iter().enumerate() {
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (i, v) in x.iter().enumerate() {
                    let w = -std::f64::consts::TAU * (k * i) as f64 / n as f64;
                    re += *v as f64 * w.cos();
                    im += *v as f64 * w.sin();
                }
                let want = (re * re + im * im) as f32;
                assert!(
                    (got - want).abs() <= 1e-3 * want.max(1.0),
                    "{j} {k} {got} {want}"
                );
            }
        }
        // Unit gains give both frames back; a gain of a half halves only its frame.
        let g1 = vec![1.0; n / 2 + 1];
        let half = vec![0.5; n / 2 + 1];
        fft.inverse_pair(&g1, &half);
        for i in 0..n {
            assert!((fft.re[i] - a[i]).abs() < 1e-4 && (fft.im[i] - 0.5 * b[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn denoising_takes_out_steady_noise_and_keeps_the_steps() {
        let clean = steps(8, 0.6, 7);
        let noisy = hiss(&clean, 0.02, 9);
        let y = denoise(&noisy, SR, std::slice::from_ref(&(0..noisy.len())));
        let rms = |x: &[f32], at: f32, len: f32| {
            let r = &x[(at * SR) as usize..((at + len) * SR) as usize];
            (r.iter().map(|v| v * v).sum::<f32>() / r.len() as f32).sqrt()
        };
        for k in 1..7 {
            let at = 0.4 + k as f32 * 0.6;
            // The step's first 40 ms within 1 dB of the noisy recording's.
            let step = 20.0 * (rms(&y, at, 0.04) / rms(&noisy, at, 0.04)).log10();
            assert!(step.abs() < 1.0, "step {k}: {step} dB");
            // Between the steps, the hiss at least 15 dB down (the floor is 20).
            let gap = 20.0 * (rms(&y, at - 0.15, 0.1) / rms(&noisy, at - 0.15, 0.1)).log10();
            assert!(
                (-21.0..-15.0).contains(&gap),
                "gap before step {k}: {gap} dB"
            );
        }
        // Outside the spans nothing changes.
        let part = denoise(
            &noisy,
            SR,
            std::slice::from_ref(&(SR as usize..2 * SR as usize)),
        );
        assert_eq!(part[..SR as usize], noisy[..SR as usize]);
        assert_eq!(part[2 * SR as usize..], noisy[2 * SR as usize..]);
        assert!(rms(&part, 1.45, 0.1) < 0.3 * rms(&noisy, 1.45, 0.1));
    }

    #[test]
    fn odd_steps_are_left_out() {
        let mut x = steps(7, 0.6, 3);
        // A much quieter fourth step (a distant or partial one).
        let at = ((0.4 + 3.0 * 0.6) * SR) as usize;
        for v in &mut x[at..at + (0.3 * SR) as usize] {
            *v *= 0.05;
        }
        let takes = takes_from_recording("unlisted", "grass", &x, SR);
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
        bank_of(&[("grass", 500.0), ("snow", 900.0), ("water", 1_300.0)])
    }

    /// A bank with three takes per surface, each a decaying tone of about `hz`.
    fn bank_of(surfaces: &[(&str, f32)]) -> SampleBank {
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
                stats: TakeStats::default(),
            }
        };
        let mut b = SampleBank::default();
        for &(name, hz) in surfaces {
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
        samples.recorded(
            surface,
            &dry,
            &WornPath::NONE,
            cell,
            0.0,
            &surfaces,
            &step,
            SR,
            true,
        )
    }

    #[test]
    fn surfaces_without_takes_and_other_contacts_are_synthesized() {
        let mut s = FootstepSamples::from_bank(bank());
        let dry = SurfaceCell::default();
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Foot, 75.0, 1.4, 15.0).is_some());
        // No takes for gravel, none for paws or hooves in this bank, and wheels and falling
        // bodies are always synthesized.
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Paw, 20.0, 1.4, 15.0).is_none());
        assert!(step_on(&mut s, "grass", &dry, ContactKind::Hoof, 500.0, 1.4, 15.0).is_none());
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

    /// A walking step on `surface` worn `wear` deep, as `play_footsteps` makes it.
    fn worn_step(
        samples: &mut FootstepSamples,
        surface: &str,
        wear: f32,
        cell: &SurfaceCell,
        seed: u64,
    ) -> (Step, Option<(Step, StepSample)>) {
        let mats = engine_world::SurfaceMaterials::default();
        let m = mats.get(mats.id(surface).unwrap());
        let surfaces = FootstepSurfaces::default();
        let dry = surfaces.get(m);
        let worn = surfaces.worn_path(m, wear, cell);
        let state =
            worn.apply(&dry)
                .with_state(cell, worn.absorbency(m.absorbency), 12.0, 0.0, &worn.snow);
        let step = Step::from_contact(state, ContactKind::Foot, 75.0, 1.4, seed);
        let r = samples.recorded(&m.name, &dry, &worn, cell, 0.0, &surfaces, &step, SR, true);
        (step, r)
    }

    #[test]
    fn worn_paths_play_the_paths_takes_on_a_share_of_steps_rising_with_wear() {
        let b = bank_of(&[
            ("grass", 500.0),
            ("snow", 900.0),
            ("gravel", 1_700.0),
            ("mud", 2_100.0),
        ]);
        let freq = |r: &(Step, StepSample)| {
            let p = &r.1.take.pcm;
            p.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32 / 0.3
        };
        let dry = SurfaceCell::default();
        // The share of 200 steps that play the takes around `hz`.
        let share = |surface: &str, wear: f32, cell: &SurfaceCell, hz: f32| {
            let mut s = FootstepSamples::from_bank(b.clone());
            let hits = (0..200u64)
                .filter(|k| {
                    let (_, r) = worn_step(&mut s, surface, wear, cell, 100 + k * 17);
                    let r = r.expect("a take");
                    (freq(&r) - hz).abs() < hz * 0.06
                })
                .count();
            hits as f32 / 200.0
        };
        // Untouched: exactly as without a path, take for take.
        let mut a = FootstepSamples::from_bank(b.clone());
        let mut c = FootstepSamples::from_bank(b.clone());
        for k in 0..20u64 {
            let (step, r) = worn_step(&mut a, "grass", 0.0, &dry, k);
            let surfaces = FootstepSurfaces::default();
            let dry_grass = surfaces.surfaces["grass"];
            let old = c.recorded(
                "grass",
                &dry_grass,
                &WornPath::NONE,
                &dry,
                0.0,
                &surfaces,
                &step,
                SR,
                true,
            );
            let (r, old) = (r.unwrap(), old.unwrap());
            assert_eq!(r.0, old.0);
            assert_eq!(r.1.take.pcm, old.1.take.pcm);
            assert_eq!(r.1.length_s, f32::INFINITY);
        }
        assert_eq!(share("grass", 0.0, &dry, 500.0), 1.0);
        // Half worn: about half the steps play bare earth's (borrowed gravel) takes.
        let half = share("grass", 0.5, &dry, 1_700.0);
        assert!((0.35..0.65).contains(&half), "{half}");
        assert!(share("grass", 0.3, &dry, 1_700.0) < half);
        assert_eq!(share("grass", 1.0, &dry, 1_700.0), 1.0);
        // Bare earth's takes are muffled and short, and the layers add its thud.
        let mut s = FootstepSamples::from_bank(b.clone());
        let (step, r) = worn_step(&mut s, "grass", 1.0, &dry, 7);
        let (synth, take) = r.unwrap();
        assert!(take.lowpass_hz <= 1_800.0 && take.length_s <= 0.3);
        assert!(synth.surface.thump > 0.3 && synth.surface.rustle < 0.01);
        assert!(step.surface.rustle < 0.01);
        // A soaked path is mud.
        let wet = SurfaceCell {
            wetness: 1.0,
            ..Default::default()
        };
        assert_eq!(share("grass", 1.0, &wet, 2_100.0), 1.0);
        // Trodden snow plays its own takes cut short, brighter and higher.
        assert_eq!(share("snow", 1.0, &dry, 900.0), 1.0);
        let (_, r) = worn_step(&mut s, "snow", 1.0, &dry, 7);
        let take = r.unwrap().1;
        assert!(take.length_s <= 0.2 && take.speed > 1.05);
        let (_, r) = worn_step(&mut s, "snow", 0.0, &dry, 7);
        assert_eq!(r.unwrap().1.length_s, f32::INFINITY);
        // Without any takes it is all synthesized.
        let mut none = FootstepSamples::from_bank(SampleBank::default());
        assert!(worn_step(&mut none, "grass", 1.0, &dry, 7).1.is_none());
    }

    #[test]
    fn a_take_cut_short_fades_out_where_it_is_cut() {
        let b = bank();
        let take = b.surfaces["grass"].takes[0].clone();
        let play = |length_s: f32| {
            let s = StepSample {
                take: take.clone(),
                speed: 1.0,
                gain: 1.0,
                shelf_db: 0.0,
                lowpass_hz: 20_000.0,
                length_s,
            };
            let mut v = SampleVoice::new(SR, &s);
            let mut out = Vec::new();
            while v.active() {
                out.push(v.next());
            }
            out
        };
        let whole = play(f32::INFINITY);
        let cut = play(0.1);
        assert!((whole.len() as f32 / SR - 0.3).abs() < 0.01);
        assert!((cut.len() as f32 / SR - 0.1).abs() < 0.01);
        // It fades: the last few milliseconds are far quieter than the same place uncut.
        let n = cut.len();
        let tail = |x: &[f32]| x[n - 48..n].iter().map(|v| v.abs()).fold(0.0, f32::max);
        assert!(tail(&cut) < tail(&whole) * 0.2);
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
    fn a_step_without_its_noise_window_is_not_a_take() {
        // The first step 0.1 s in: its noise can't be measured, so it is left out.
        let full = steps(8, 0.6, 21);
        let early = &full[(0.3 * SR) as usize..];
        assert_eq!(
            takes_from_recording("unlisted", "grass", &full, SR).len(),
            8
        );
        assert_eq!(
            takes_from_recording("unlisted", "grass", early, SR).len(),
            7
        );
        // Nor right after digital silence that opens a file (the recording starts there).
        let mut opened = vec![0.0; (2.0 * SR) as usize];
        opened.extend_from_slice(early);
        assert_eq!(
            takes_from_recording("unlisted", "grass", &opened, SR).len(),
            7
        );
        // A part's first step is measured on the recording before the part.
        let parted = cut_steps(&full, SR, Part::Walk, &Cutter::SHOES, 350);
        assert_eq!(parted.len(), 8);
    }

    #[test]
    fn paws_and_hooves_are_cut_short() {
        // A paw every 0.15 s, each a 0.1 s tap.
        let x = taps(16, 0.15, 0.1, 60.0, 31);
        let paws = cut_recording(Feet::Paws, "unlisted", "hard", &x, SR);
        assert!(paws.len() >= 12, "{}", paws.len());
        for t in &paws {
            assert!(t.stats.len_s <= 0.16, "{:?}", t.stats);
            assert_eq!(t.gait, Gait::Walk);
        }
        // A hoof every 0.27 s.
        let x = taps(10, 0.27, 0.25, 40.0, 32);
        let hooves = cut_recording(Feet::Hooves, "unlisted", "hard", &x, SR);
        assert!(hooves.len() >= 8, "{}", hooves.len());
        assert!(hooves.iter().all(|t| t.stats.len_s <= 0.26));
        let soft = cut_recording(Feet::Hooves, "unlisted", "dirt", &x, SR);
        assert!(soft.iter().all(|t| t.stats.len_s <= 0.31));
    }

    #[test]
    fn paws_and_hooves_play_their_own_ground() {
        let mut b = bank();
        let grass = b.surfaces["grass"].clone();
        let snow = b.surfaces["snow"].clone();
        b.paws.insert("hard".into(), grass.clone());
        b.paws.insert("soft".into(), snow.clone());
        b.hooves.insert("gravel".into(), grass);
        let mut s = FootstepSamples::from_bank(b);
        let dry = SurfaceCell::default();
        let tone = |r: &(Step, StepSample)| {
            let p = &r.1.take.pcm;
            p.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32 / 0.3
        };
        // A paw on rock plays the hard-ground takes, with rock's grit (which a wooden floor,
        // where they were recorded, lacks) synthesized over them.
        let r = step_on(&mut s, "rock", &dry, ContactKind::Paw, 11.0, 1.8, 15.0).unwrap();
        assert!((450.0..560.0).contains(&tone(&r)), "{}", tone(&r));
        assert!(r.0.surface.grit > 0.15, "{:?}", r.0.surface);
        let r = step_on(&mut s, "leaves", &dry, ContactKind::Paw, 11.0, 1.8, 15.0).unwrap();
        assert!((850.0..960.0).contains(&tone(&r)), "{}", tone(&r));
        // Gravel is too unlike a lawn for the soft-ground takes; a hoof has its own.
        assert!(step_on(&mut s, "gravel", &dry, ContactKind::Paw, 11.0, 1.8, 15.0).is_none());
        let r = step_on(&mut s, "gravel", &dry, ContactKind::Hoof, 250.0, 1.7, 15.0).unwrap();
        assert!((450.0..560.0).contains(&tone(&r)), "{}", tone(&r));
        // No hooves on wood here, and shoes never play the animals' takes.
        assert!(step_on(&mut s, "wood", &dry, ContactKind::Hoof, 250.0, 1.7, 15.0).is_none());
        assert!(step_on(&mut s, "rock", &dry, ContactKind::Foot, 75.0, 1.4, 15.0).is_none());
        // A dog's paw is as bright as the dog recorded, not dulled for being lighter than a
        // person.
        let r = step_on(&mut s, "rock", &dry, ContactKind::Paw, 11.0, 1.8, 15.0).unwrap();
        assert!(r.1.shelf_db.abs() < 1.5, "{}", r.1.shelf_db);
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
        let x = steps(8, 0.6, 5);
        crate::wav::write_wav(&dir.join("concrete/test_steps.wav"), &x, 1, SR as u32).unwrap();
        // Too few steps to play: synthesized.
        let few = steps(5, 0.6, 6);
        crate::wav::write_wav(&dir.join("mud/test_steps.wav"), &few, 1, SR as u32).unwrap();
        let bank = SampleBank::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(bank.takes("concrete").map(|t| t.takes.len()), Some(8));
        assert!(bank.takes("mud").is_none());
        // Tile borrows concrete's; nothing for snow.
        assert!(bank.takes("tile").is_some());
        assert!(bank.takes("snow").is_none());
    }

    #[test]
    fn cleaned_takes_only_fill_in_where_clean_ones_are_scarce() {
        let dir = std::env::temp_dir().join(format!("engine_audio_clean_{}", std::process::id()));
        let noisy = hiss(&steps(8, 0.6, 51), 0.02, 52);
        let wav = |path: &str, x: &[f32]| {
            crate::wav::write_wav(&dir.join(path), x, 1, SR as u32).unwrap();
        };
        // Grass has 30 clean takes and a noisy recording; mud only the noisy recording.
        wav("grass/a.wav", &steps(15, 0.6, 53));
        wav("grass/b.wav", &steps(15, 0.6, 54));
        wav("grass/noisy.wav", &noisy);
        wav("mud/noisy.wav", &noisy);
        let bank = SampleBank::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let grass = &bank.takes("grass").unwrap().takes;
        assert_eq!(grass.len(), 30);
        assert!(grass.iter().all(|t| !t.stats.cleaned));
        let mud = &bank.takes("mud").unwrap().takes;
        assert_eq!(mud.len(), 8);
        assert!(
            mud.iter()
                .all(|t| t.stats.cleaned && t.stats.floor_db <= -MIN_SNR_DB)
        );
    }

    #[test]
    fn every_kind_loads_from_its_folder() {
        let dir = std::env::temp_dir().join(format!("engine_audio_kinds_{}", std::process::id()));
        let shoes = steps(8, 0.6, 41);
        let paws = taps(16, 0.15, 0.1, 60.0, 42);
        crate::wav::write_wav(&dir.join("footsteps/wood/a.wav"), &shoes, 1, SR as u32).unwrap();
        crate::wav::write_wav(&dir.join("paws/hard/b.wav"), &paws, 1, SR as u32).unwrap();
        let bank = SampleBank::load_all(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(bank.takes("wood").map(|t| t.takes.len()), Some(8));
        assert!(bank.takes_for(Feet::Paws, "concrete").is_some());
        assert!(bank.takes_for(Feet::Paws, "wood").is_some());
        assert!(bank.takes_for(Feet::Paws, "grass").is_none());
        assert!(bank.takes_for(Feet::Hooves, "wood").is_none());
        let mut samples = FootstepSamples::from_bank(bank);
        let _ = samples.bank(false);
        let summary = samples.summary();
        assert_eq!(summary.get("wood"), Some(&8));
        assert!(summary.get("paws/hard").is_some_and(|n| *n >= 12));
    }
}
