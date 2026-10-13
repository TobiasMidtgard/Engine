//! Nature around the listener: songbirds singing from the trees by day (loudest in the dawn
//! chorus), owls hooting at night, crickets chirping on warm evenings and cicadas buzzing on
//! hot afternoons.
//!
//! Each bird sits on a perch in a tree somewhere around the listener and sings its own song
//! now and then as its own positioned voice, so birds pan, fade with distance and go quiet
//! behind a hill like any other sound. Rain, strong wind, cold and bare ground (concrete,
//! metal, rock, ice) quiet them; thin air and space silence them like everything else.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::{Env, PrecipitationKind, WorldShift};

use crate::dsp::{Biquad, Noise, Resonator};
use crate::{Audio, AudioCue, AudioSettings, PlayOptions, Sound, VoiceId};

/// A kind of bird and how it sings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Species {
    /// A soft, sweet trill of thin notes sliding down the scale (willow warbler).
    Warbler,
    /// Fluting phrases, each sung two or three times (song thrush).
    Thrush,
    /// A clear two-note whistle, "fee-bee" (chickadee).
    Chickadee,
    /// A quickening run of notes that ends in a flourish (chaffinch).
    Finch,
    /// Soft hollow coos (wood pigeon).
    Dove,
    /// Harsh caws.
    Crow,
    /// A woodpecker drumming on a branch.
    Woodpecker,
    /// Hooting at night (tawny owl).
    Owl,
    /// "Cuck-oo", over and over.
    Cuckoo,
}

impl Species {
    pub const ALL: [Species; 9] = [
        Species::Warbler,
        Species::Thrush,
        Species::Chickadee,
        Species::Finch,
        Species::Dove,
        Species::Crow,
        Species::Woodpecker,
        Species::Owl,
        Species::Cuckoo,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Species::Warbler => "warbler",
            Species::Thrush => "thrush",
            Species::Chickadee => "chickadee",
            Species::Finch => "finch",
            Species::Dove => "dove",
            Species::Crow => "crow",
            Species::Woodpecker => "woodpecker",
            Species::Owl => "owl",
            Species::Cuckoo => "cuckoo",
        }
    }

    /// Seconds of quiet between songs (min, max).
    fn pause(self) -> (f32, f32) {
        match self {
            Species::Warbler => (3.0, 9.0),
            Species::Thrush => (1.5, 5.0),
            Species::Chickadee => (5.0, 14.0),
            Species::Finch => (5.0, 12.0),
            Species::Dove => (10.0, 25.0),
            Species::Crow => (8.0, 30.0),
            Species::Woodpecker => (10.0, 40.0),
            Species::Owl => (15.0, 45.0),
            Species::Cuckoo => (6.0, 20.0),
        }
    }

    /// Peak level at 1 m.
    fn loudness(self) -> f32 {
        match self {
            Species::Warbler => 0.45,
            Species::Thrush => 0.75,
            Species::Chickadee => 0.5,
            Species::Finch => 0.6,
            Species::Dove => 0.7,
            Species::Crow => 0.9,
            Species::Woodpecker => 1.0,
            Species::Owl => 0.7,
            Species::Cuckoo => 0.8,
        }
    }
}

/// One note of a song: a pitch sweep with an envelope.
#[derive(Debug, Clone, Copy)]
struct Note {
    /// Seconds from the start of the song.
    start: f32,
    dur: f32,
    /// Hz at the start and the end of the note.
    f0: f32,
    f1: f32,
    /// How the sweep runs: 1 straight, below 1 fast at first, above 1 slow at first.
    bend: f32,
    /// Vibrato rate (Hz) and depth (fraction of the pitch).
    vib_hz: f32,
    vib: f32,
    amp: f32,
    /// Share of the second harmonic.
    harm: f32,
    /// Share of breath noise.
    breath: f32,
    /// Share of the note spent swelling in and fading out (0.1 crisp .. 0.5 soft).
    soft: f32,
}

impl Note {
    fn new(start: f32, dur: f32, f0: f32, f1: f32) -> Self {
        Self {
            start,
            dur,
            f0,
            f1,
            bend: 1.0,
            vib_hz: 0.0,
            vib: 0.0,
            amp: 1.0,
            harm: 0.08,
            breath: 0.0,
            soft: 0.15,
        }
    }

    fn end(&self) -> f32 {
        self.start + self.dur
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Voice {
    /// Whistled notes (most songbirds, doves, owls, cuckoos).
    Tonal,
    /// A harsh buzzy call through the throat's resonances (crows).
    Caw,
    /// Knocks on wood.
    Drum,
}

/// One song of one bird. Mono; ends when the song does.
pub struct BirdSong {
    fs: f32,
    voice: Voice,
    notes: Vec<Note>,
    level: f32,
    /// Index of the first note that hasn't ended.
    cur: usize,
    t: f32,
    phase: f32,
    rng: Noise,
    breath: [Biquad; 2],
    formants: [Biquad; 3],
    knocks: [Resonator; 3],
    struck: usize,
    len: f32,
}

impl BirdSong {
    /// A song of `species`. `bird` picks the individual (its pitch, tempo and favourite
    /// phrases stay the same from song to song); `song` varies this one song.
    pub fn new(fs: f32, species: Species, bird: u64, song: u64) -> Self {
        let mut me = Noise::new(bird);
        // Each bird has its own voice: a little higher or lower and faster or slower.
        let pitch = me.range(0.9, 1.1);
        let tempo = me.range(0.9, 1.12);
        let mut rng = Noise::new(song ^ bird.rotate_left(17));
        let notes = compose(species, pitch, tempo, &mut me, &mut rng);
        let voice = match species {
            Species::Crow => Voice::Caw,
            Species::Woodpecker => Voice::Drum,
            _ => Voice::Tonal,
        };
        let len = notes.iter().map(Note::end).fold(0.0, f32::max);
        let wood = me.range(0.8, 1.25);
        Self {
            fs,
            voice,
            notes,
            level: species.loudness(),
            cur: 0,
            t: 0.0,
            phase: 0.0,
            rng,
            // Breath noise around the song's pitch; big low birds breathe low.
            breath: {
                let hz = match species {
                    Species::Dove | Species::Owl | Species::Cuckoo => 700.0,
                    _ => 3_000.0,
                } * pitch;
                [
                    Biquad::bandpass(fs, hz, 1.2),
                    Biquad::lowpass(fs, hz * 1.8, 0.7),
                ]
            },
            formants: [
                Biquad::bandpass(fs, 1_150.0 * pitch, 4.0),
                Biquad::bandpass(fs, 1_900.0 * pitch, 5.0),
                Biquad::lowpass(fs, 4_500.0, 0.7),
            ],
            knocks: [
                Resonator::new(fs, 1_150.0 * wood, 0.025),
                Resonator::new(fs, 2_700.0 * wood, 0.012),
                Resonator::new(fs, 420.0 * wood, 0.05),
            ],
            struck: 0,
            len,
        }
    }

    /// Seconds the song lasts.
    pub fn duration(&self) -> f32 {
        self.len
    }

    fn tonal(&mut self, n: Note, tau: f32) -> f32 {
        let x = (tau / n.dur).clamp(0.0, 1.0);
        let mut f = n.f0 + (n.f1 - n.f0) * x.powf(n.bend);
        if n.vib > 0.0 {
            f *= 1.0 + n.vib * (std::f32::consts::TAU * n.vib_hz * tau).sin();
        }
        self.phase = (self.phase + f / self.fs).fract();
        let w = std::f32::consts::TAU * self.phase;
        let env = envelope(x, n.soft);
        let breath = if n.breath > 0.0 {
            let b = self.breath[0].process(self.rng.white());
            self.breath[1].process(b) * n.breath * 2.0
        } else {
            0.0
        };
        (w.sin() + n.harm * (2.0 * w).sin() + breath) * env * n.amp
    }

    /// The branch ringing: a crack, a higher tick and the wood's low knock.
    fn knock(&mut self, hit: f32) -> f32 {
        self.knocks[0].process(hit) * 0.55
            + self.knocks[1].process(hit) * 0.25
            + self.knocks[2].process(hit) * 0.3
    }

    fn caw(&mut self, n: Note, tau: f32) -> f32 {
        let x = (tau / n.dur).clamp(0.0, 1.0);
        let f = (n.f0 + (n.f1 - n.f0) * x) * (1.0 + 0.03 * self.rng.white());
        self.phase = (self.phase + f / self.fs).fract();
        // A rough glottal buzz: sawtooth plus noise.
        let src = (2.0 * self.phase - 1.0) + 0.5 * self.rng.white();
        let y = self.formants[0].process(src) * 1.6 + self.formants[1].process(src) * 1.1;
        self.formants[2].process(y) * envelope(x, n.soft) * n.amp
    }
}

/// Swell in and fade out over `soft` of the note at each end (smoothstep).
fn envelope(x: f32, soft: f32) -> f32 {
    let s = soft.clamp(0.02, 0.5);
    let a = (x / s).min(1.0);
    let r = ((1.0 - x) / s).min(1.0);
    let a = a * a * (3.0 - 2.0 * a);
    let r = r * r * (3.0 - 2.0 * r);
    a * r
}

/// Write the notes of one song.
fn compose(species: Species, pitch: f32, tempo: f32, me: &mut Noise, rng: &mut Noise) -> Vec<Note> {
    let mut notes = Vec::new();
    let k = 1.0 / tempo;
    match species {
        Species::Warbler => {
            // Thin notes stepping down the scale, slowing a little at the end.
            let n = 11 + (rng.uniform() * 6.0) as usize;
            let mut t = 0.0;
            for i in 0..n {
                let x = i as f32 / (n - 1) as f32;
                let base = (6_200.0 - 2_600.0 * x) * pitch * rng.range(0.97, 1.03);
                let dur = (0.065 + 0.03 * x) * k;
                let mut note = Note::new(t, dur, base * 1.12, base * 0.86);
                note.bend = 0.7;
                note.amp = 0.6 + 0.4 * (1.0 - (x - 0.4).abs());
                note.harm = 0.05;
                notes.push(note);
                t += dur + rng.range(0.025, 0.04) * k;
            }
        }
        Species::Thrush => {
            // Two or three different phrases, each repeated two or three times.
            let mut t = 0.0;
            for _ in 0..(2 + (rng.uniform() * 2.0) as usize) {
                let notes_in = 2 + (rng.uniform() * 3.0) as usize;
                let phrase: Vec<(f32, f32, f32)> = (0..notes_in)
                    .map(|_| {
                        let f = rng.range(2_000.0, 4_800.0) * pitch;
                        (f, f * rng.range(0.75, 1.3), rng.range(0.07, 0.15))
                    })
                    .collect();
                for _ in 0..(2 + (rng.uniform() * 1.6) as usize) {
                    for &(f0, f1, dur) in &phrase {
                        let mut note = Note::new(t, dur * k, f0, f1);
                        note.harm = 0.22;
                        note.breath = 0.04;
                        note.bend = 0.8;
                        notes.push(note);
                        t += (dur + 0.03) * k;
                    }
                    t += 0.12 * k;
                }
                t += 0.35 * k;
            }
        }
        Species::Chickadee => {
            let hi = 3_950.0 * pitch;
            let mut fee = Note::new(0.0, 0.34 * k, hi, hi * 0.985);
            fee.harm = 0.02;
            fee.soft = 0.2;
            let mut bee = Note::new(0.42 * k, 0.3 * k, hi * 0.86, hi * 0.84);
            bee.harm = 0.02;
            bee.soft = 0.2;
            // Some sing "fee-bee-ee", the second note split by a tiny dip.
            if me.chance(0.4) {
                bee.dur = 0.14 * k;
                let mut ee = Note::new(0.58 * k, 0.16 * k, hi * 0.85, hi * 0.83);
                ee.harm = 0.02;
                ee.soft = 0.25;
                notes.extend([fee, bee, ee]);
            } else {
                notes.extend([fee, bee]);
            }
        }
        Species::Finch => {
            // A run of notes falling in pitch and getting quicker, then the flourish.
            let n = 8 + (rng.uniform() * 5.0) as usize;
            let mut t = 0.0;
            for i in 0..n {
                let x = i as f32 / n as f32;
                let f = (5_200.0 - 2_000.0 * x) * pitch;
                let dur = 0.05 * k;
                let mut note = Note::new(t, dur, f * 1.05, f * 0.92);
                note.harm = 0.15;
                notes.push(note);
                t += dur + (0.1 - 0.06 * x) * k;
            }
            let f = 2_900.0 * pitch;
            let mut up = Note::new(t, 0.07 * k, f, f * 2.0);
            up.bend = 1.6;
            up.harm = 0.12;
            let mut down = Note::new(t + 0.08 * k, 0.11 * k, f * 2.1, f * 0.85);
            down.bend = 0.6;
            down.harm = 0.12;
            notes.extend([up, down]);
        }
        Species::Dove => {
            // "coo-COO-coo, coo-coo", low and hollow.
            let f = 480.0 * pitch;
            let mut t = 0.0;
            for (i, (dur, amp, rise)) in [
                (0.26, 0.7, 1.04),
                (0.45, 1.0, 1.08),
                (0.24, 0.7, 1.02),
                (0.22, 0.8, 1.03),
                (0.3, 0.75, 1.02),
            ]
            .into_iter()
            .enumerate()
            {
                let mut note = Note::new(t, dur * k, f * 0.97, f * rise);
                note.amp = amp;
                note.harm = 0.35;
                note.breath = 0.18;
                note.soft = 0.35;
                notes.push(note);
                t += (dur + if i == 2 { 0.45 } else { 0.12 }) * k;
            }
        }
        Species::Crow => {
            let mut t = 0.0;
            for _ in 0..(2 + (rng.uniform() * 3.0) as usize) {
                let f = rng.range(480.0, 600.0) * pitch;
                let dur = rng.range(0.28, 0.4) * k;
                let mut note = Note::new(t, dur, f, f * 0.88);
                note.soft = 0.12;
                notes.push(note);
                t += dur + rng.range(0.35, 0.6) * k;
            }
        }
        Species::Woodpecker => {
            // A drum roll: about 16 knocks in 0.8 s, easing off at the end.
            let n = 14 + (rng.uniform() * 6.0) as usize;
            let mut t = 0.0;
            for i in 0..n {
                let x = i as f32 / n as f32;
                let mut note = Note::new(t, 0.01, 0.0, 0.0);
                note.amp = 1.0 - 0.45 * x;
                notes.push(note);
                t += (0.045 + 0.02 * x * x) * k;
            }
        }
        Species::Owl => {
            // "Hooo ... hu, hu-hooooo".
            let f = 390.0 * pitch;
            let mut first = Note::new(0.0, 0.45 * k, f * 1.02, f * 0.97);
            first.harm = 0.12;
            first.breath = 0.25;
            first.soft = 0.3;
            let mut short = Note::new(2.6 * k, 0.16 * k, f * 0.95, f * 0.93);
            short.harm = 0.12;
            short.breath = 0.25;
            short.soft = 0.3;
            let mut long = Note::new(2.95 * k, 1.1 * k, f, f * 0.96);
            long.harm = 0.12;
            long.breath = 0.25;
            long.soft = 0.25;
            long.vib_hz = 7.5;
            long.vib = 0.035;
            notes.extend([first, short, long]);
        }
        Species::Cuckoo => {
            let f = 720.0 * pitch;
            let mut t = 0.0;
            for _ in 0..(3 + (rng.uniform() * 4.0) as usize) {
                let mut cuck = Note::new(t, 0.2 * k, f, f * 0.98);
                cuck.harm = 0.15;
                cuck.breath = 0.05;
                cuck.soft = 0.25;
                let mut oo = Note::new(t + 0.32 * k, 0.32 * k, f * 0.81, f * 0.79);
                oo.harm = 0.15;
                oo.breath = 0.05;
                oo.soft = 0.3;
                notes.extend([cuck, oo]);
                t += 0.85 * k;
            }
        }
    }
    notes
}

impl Sound for BirdSong {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let dt = 1.0 / self.fs;
        for (i, o) in out.iter_mut().enumerate() {
            if self.t > self.len + 0.1 {
                return i;
            }
            while self.cur < self.notes.len() && self.notes[self.cur].end() < self.t {
                self.cur += 1;
            }
            let mut y = 0.0;
            match self.voice {
                Voice::Drum => {
                    // Strike each knock once as its time comes.
                    if self.struck < self.notes.len() && self.notes[self.struck].start <= self.t {
                        let a = self.notes[self.struck].amp;
                        self.struck += 1;
                        let hit = a * (1.0 + 0.1 * self.rng.white());
                        y += self.knock(hit);
                    } else {
                        y += self.knock(0.0);
                    }
                }
                _ => {
                    if let Some(&n) = self.notes.get(self.cur)
                        && n.start <= self.t
                    {
                        let tau = self.t - n.start;
                        y = if self.voice == Voice::Caw {
                            self.caw(n, tau)
                        } else {
                            self.tonal(n, tau)
                        };
                    } else {
                        // Between notes: start the next one from a zero crossing.
                        self.phase = 0.0;
                    }
                }
            }
            *o = y * self.level;
            self.t += dt;
        }
        out.len()
    }
}

/// Crickets and cicadas: many insects in one voice. Mono; params `"crickets"` and
/// `"cicadas"` 0..1 (how many are calling) and `"temperature"` °C (crickets chirp faster
/// when it is warm: Dolbear's law).
pub struct InsectChorus {
    fs: f32,
    rng: Noise,
    crickets: [Cricket; 10],
    cicada: Biquad,
    cicada2: Biquad,
    cicada_hp: Biquad,
    cicada_phase: f32,
    swell_phase: f32,
    target: [f32; 3],
    value: [f32; 3],
}

#[derive(Clone, Copy, Default)]
struct Cricket {
    carrier: f32,
    phase: f32,
    /// Seconds until the next chirp starts.
    wait: f32,
    /// Seconds into the current chirp (negative: not chirping).
    t: f32,
    amp: f32,
    /// This cricket's own tempo.
    rate: f32,
}

const CRICKETS: usize = 0;
const CICADAS: usize = 1;
const TEMPERATURE: usize = 2;

/// Pulses per chirp, pulse length and spacing (seconds).
const PULSES: f32 = 3.0;
const PULSE_S: f32 = 0.014;
const PULSE_GAP_S: f32 = 0.032;

impl InsectChorus {
    pub fn new(fs: f32, seed: u64) -> Self {
        let mut rng = Noise::new(seed);
        let mut crickets = [Cricket::default(); 10];
        for c in &mut crickets {
            c.carrier = rng.range(4_300.0, 5_200.0);
            c.wait = rng.range(0.0, 1.0);
            c.t = -1.0;
            // A few close and loud, most farther off.
            c.amp = 0.05 + 0.25 * rng.uniform().powi(3);
            c.rate = rng.range(0.9, 1.1);
        }
        Self {
            fs,
            crickets,
            rng,
            cicada: Biquad::bandpass(fs, 5_600.0, 2.0),
            cicada2: Biquad::bandpass(fs, 7_400.0, 3.0),
            cicada_hp: Biquad::highpass(fs, 3_500.0, 0.7),
            cicada_phase: 0.0,
            swell_phase: 0.0,
            target: [0.0, 0.0, 20.0],
            value: [0.0, 0.0, 20.0],
        }
    }

    /// Chirps per second for a cricket at `celsius` (Dolbear: about 40 a minute at 10 °C,
    /// 7 more a minute per degree), none below 8 °C.
    pub fn chirps_per_second(celsius: f32) -> f32 {
        if celsius < 8.0 {
            0.0
        } else {
            (40.0 + 7.2 * (celsius - 10.0)).max(15.0) / 60.0
        }
    }
}

impl Sound for InsectChorus {
    fn set_param(&mut self, name: &str, value: f32) {
        match name {
            "crickets" => self.target[CRICKETS] = value.clamp(0.0, 1.0),
            "cicadas" => self.target[CICADAS] = value.clamp(0.0, 1.0),
            "temperature" => self.target[TEMPERATURE] = value,
            _ => {}
        }
    }

    fn fill(&mut self, out: &mut [f32]) -> usize {
        let fs = self.fs;
        let dt = 1.0 / fs;
        let chirp_len = PULSES * PULSE_GAP_S;
        for (i, o) in out.iter_mut().enumerate() {
            if i % 64 == 0 {
                let a = 1.0 - (-64.0 / (2.0 * fs)).exp();
                for k in 0..3 {
                    self.value[k] += (self.target[k] - self.value[k]) * a;
                }
            }
            let crickets = self.value[CRICKETS];
            let rate = Self::chirps_per_second(self.value[TEMPERATURE]);
            let mut y = 0.0;
            if crickets > 0.001 && rate > 0.0 {
                // The share of crickets calling grows with `crickets`.
                let calling = (crickets * self.crickets.len() as f32).ceil() as usize;
                for c in self.crickets.iter_mut().take(calling) {
                    if c.t < 0.0 {
                        c.wait -= dt;
                        if c.wait <= 0.0 {
                            c.t = 0.0;
                            let period = 1.0 / (rate * c.rate);
                            c.wait = period * (1.0 + 0.04 * self.rng.white());
                        }
                        continue;
                    }
                    c.t += dt;
                    c.wait -= dt;
                    if c.t > chirp_len {
                        c.t = -1.0;
                        continue;
                    }
                    let in_pulse = c.t % PULSE_GAP_S;
                    if in_pulse < PULSE_S {
                        let x = in_pulse / PULSE_S;
                        let env = (std::f32::consts::PI * x).sin();
                        c.phase = (c.phase + c.carrier / fs).fract();
                        y += (std::f32::consts::TAU * c.phase).sin() * env * c.amp;
                    }
                }
            }
            let cicadas = self.value[CICADAS];
            if cicadas > 0.001 {
                // A rasping buzz pulsing at about 120 Hz that swells and fades over seconds.
                self.cicada_phase = (self.cicada_phase + 118.0 / fs).fract();
                self.swell_phase = (self.swell_phase + 0.08 / fs).fract();
                let swell = 0.55 + 0.45 * (std::f32::consts::TAU * self.swell_phase).sin();
                let pulse = if self.cicada_phase < 0.35 { 1.0 } else { 0.15 };
                let n = self.rng.white();
                let buzz = self
                    .cicada_hp
                    .process(self.cicada.process(n) + 0.6 * self.cicada2.process(n));
                y += buzz * pulse * swell * cicadas * 0.35;
            }
            *o = y;
        }
        out.len()
    }
}

/// Most birds singing around the listener at once.
const MAX_BIRDS: usize = 8;
/// Birds farther than this find a new perch nearer the listener, meters.
const PERCH_RANGE_M: f64 = 90.0;
/// How often the flock is checked, seconds.
const SCAN_INTERVAL: f64 = 0.5;

struct Perch {
    position: DVec3,
    species: Species,
    bird: u64,
    songs: u64,
    next_song: f64,
    voice: VoiceId,
}

#[derive(Default)]
pub(crate) struct NatureState {
    perches: Vec<Perch>,
    next_scan: f64,
    rng: Option<Noise>,
    insects: [VoiceId; 2],
}

/// What the place and the hour make of birds and insects.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Chorus {
    /// 0..1: how many songbirds sing (1 = the dawn chorus in a wood).
    pub birds: f32,
    /// 0..1: owls calling.
    pub owls: f32,
    pub crickets: f32,
    pub cicadas: f32,
    /// Songs come this many times more often than usual (dawn chorus).
    pub eagerness: f32,
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Chorus {
    /// Birds and insects for the sun's height (`sun`: sine of its elevation), the hour
    /// (morning or evening), the weather and how wild the place is (`habitat` 0 city .. 1
    /// woodland).
    pub fn new(
        sun: f32,
        hour: f32,
        celsius: f32,
        rain_mm_h: f32,
        snowing: bool,
        wind: f32,
        habitat: f32,
    ) -> Self {
        let morning = hour < 12.0;
        let day = smooth(-0.12, 0.05, sun);
        let night = 1.0 - smooth(-0.1, 0.02, sun);
        // The dawn chorus: twilight to a couple of hours after sunrise.
        let dawn = if morning {
            smooth(-0.17, -0.07, sun) * (1.0 - smooth(0.25, 0.45, sun))
        } else {
            0.0
        };
        // A smaller evening chorus.
        let dusk = if morning {
            0.0
        } else {
            smooth(-0.12, -0.02, sun) * (1.0 - smooth(0.15, 0.3, sun)) * 0.5
        };
        let quiet = (1.0 - smooth(0.3, 4.0, rain_mm_h))
            * (1.0 - smooth(6.0, 14.0, wind))
            * if snowing { 0.4 } else { 1.0 };
        let cold = 0.35 + 0.65 * smooth(-10.0, 6.0, celsius);
        let midday = 0.45 + 0.55 * dawn.max(dusk);
        let birds = (day * midday).max(dawn) * quiet * cold * habitat;
        // Owls call in the dark, and stop once the dawn chorus starts (nautical twilight).
        let dark = 1.0 - smooth(-0.2, -0.1, sun);
        let owls = dark * (1.0 - smooth(1.0, 6.0, rain_mm_h)) * habitat;
        let rain = 1.0 - smooth(0.2, 2.0, rain_mm_h);
        let crickets = (night + dusk * 0.6).min(1.0) * smooth(9.0, 18.0, celsius) * rain * habitat;
        let cicadas = day * (1.0 - dawn) * smooth(22.0, 30.0, celsius) * rain * habitat;
        Self {
            birds: birds.clamp(0.0, 1.0),
            owls: owls.clamp(0.0, 1.0),
            crickets: crickets.clamp(0.0, 1.0),
            cicadas: cicadas.clamp(0.0, 1.0),
            eagerness: 1.0 + 1.5 * dawn,
        }
    }
}

/// How wild a ground material is for birds and insects: woods and meadows 1, towns and bare
/// rock low.
fn wildness(material: &str) -> f32 {
    match material {
        "grass" | "leaves" | "dirt" | "soil" | "mud" => 1.0,
        "default" | "sand" | "gravel" | "water" => 0.6,
        "snow" | "rock" | "stone" | "wood" | "planks" => 0.4,
        "ash" | "ice" => 0.2,
        "concrete" | "asphalt" | "tile" | "metal" | "glass" => 0.1,
        _ => 0.5,
    }
}

/// Pick a songbird for the time of year (cuckoos only in spring and early summer).
fn pick_songbird(rng: &mut Noise, day_of_year: f32) -> Species {
    let cuckoo = if (100.0..200.0).contains(&day_of_year) {
        0.6
    } else {
        0.0
    };
    let weights = [
        (Species::Warbler, 3.0),
        (Species::Thrush, 2.0),
        (Species::Chickadee, 2.0),
        (Species::Finch, 3.0),
        (Species::Dove, 1.5),
        (Species::Crow, 1.0),
        (Species::Woodpecker, 0.7),
        (Species::Cuckoo, cuckoo),
    ];
    let total: f32 = weights.iter().map(|w| w.1).sum();
    let mut x = rng.uniform() * total;
    for (s, w) in weights {
        if x < w {
            return s;
        }
        x -= w;
    }
    Species::Finch
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn nature_ambience(
    mut audio: ResMut<Audio>,
    settings: Res<AudioSettings>,
    env: Env,
    time: Res<Time>,
    mut shifts: MessageReader<WorldShift>,
    mut cues: MessageWriter<AudioCue>,
    mut state: Local<NatureState>,
) {
    let state = &mut *state;
    // Perches are places in the world: they move with it.
    for s in shifts.read() {
        for p in &mut state.perches {
            p.position = s.point(p.position);
        }
    }
    let now = time.elapsed_secs_f64();
    if now < state.next_scan {
        return;
    }
    state.next_scan = now + SCAN_INTERVAL;
    if !audio.is_running() || !settings.nature {
        for id in state.insects {
            if !id.is_none() {
                audio.stop(id, 1.5);
            }
        }
        state.insects = [VoiceId::NONE; 2];
        state.perches.clear();
        return;
    }
    let seed = audio.next_seed();
    let rng = state.rng.get_or_insert_with(|| Noise::new(seed));
    let listener = audio.listener_position();
    let up = env.up(listener);
    let w = &env.weather;
    let sun = w.sun_direction.dot(up);
    let celsius = env.temperature(listener);

    // How wild the place is: the ground under the listener and in a ring around. No ground
    // (an empty level, a ship in orbit) is no habitat; high above it (flying, a cliff top)
    // the birds are too far below to hear, and under water they are gone.
    let mut habitat = 0.0;
    for k in 0..9 {
        let q = if k == 0 {
            listener
        } else {
            let a = k as f64 / 8.0 * std::f64::consts::TAU;
            listener + DVec3::new(a.cos() * 25.0, 0.0, a.sin() * 25.0)
        };
        if let Some(g) = env.ground(q) {
            habitat += wildness(&env.material(g.surface).name) / 9.0;
        }
    }
    let aloft = env
        .ground(listener)
        .map_or(0.0, |g| (listener.y - g.height) as f32);
    let submerged = env
        .water(listener)
        .is_some_and(|s| listener.y < s.surface_height);
    let habitat = if submerged {
        0.0
    } else {
        habitat * (1.0 - smooth(30.0, 80.0, aloft))
    };
    let chorus = Chorus::new(
        sun,
        w.time_of_day_h,
        celsius,
        w.precipitation_mm_h,
        w.precipitation_kind == PrecipitationKind::Snow && w.precipitation_mm_h > 0.2,
        w.wind.length(),
        habitat,
    );

    // The flock: songbirds by day, an owl or two by night.
    let want_songbirds = (MAX_BIRDS as f32 * chorus.birds).round() as usize;
    let want_owls = if chorus.owls > 0.3 {
        1 + (chorus.owls > 0.7) as usize
    } else {
        0
    };
    let fs = audio.sample_rate();
    // Birds that are too far, out of season or out of hours fly off (after their song).
    let mut songbirds = 0;
    let mut owls = 0;
    state.perches.retain(|p| {
        let singing = audio.is_playing(p.voice);
        let far = p.position.distance(listener) > PERCH_RANGE_M;
        let keep = if p.species == Species::Owl {
            owls < want_owls
        } else {
            songbirds < want_songbirds
        };
        let stay = singing || (!far && keep);
        if stay {
            if p.species == Species::Owl {
                owls += 1;
            } else {
                songbirds += 1;
            }
        }
        stay
    });
    while songbirds < want_songbirds || owls < want_owls {
        let owl = owls < want_owls;
        let species = if owl {
            Species::Owl
        } else {
            pick_songbird(rng, w.day_of_year)
        };
        let a = rng.uniform() as f64 * std::f64::consts::TAU;
        let d = rng.range(14.0, 65.0) as f64;
        let ground = listener + DVec3::new(a.cos() * d, 0.0, a.sin() * d);
        let height = env
            .ground(ground)
            .map(|g| g.height)
            .unwrap_or(listener.y - 1.7);
        let perch = DVec3::new(ground.x, height + rng.range(3.0, 12.0) as f64, ground.z);
        state.perches.push(Perch {
            position: perch,
            species,
            bird: rng.next_u32() as u64,
            songs: 0,
            next_song: now + rng.range(0.2, 4.0) as f64,
            voice: VoiceId::NONE,
        });
        if owl {
            owls += 1;
        } else {
            songbirds += 1;
        }
    }

    for p in &mut state.perches {
        if now < p.next_song || audio.is_playing(p.voice) {
            continue;
        }
        let song = BirdSong::new(fs, p.species, p.bird, p.songs);
        let len = song.duration();
        p.songs += 1;
        p.voice = audio.play(
            song,
            PlayOptions::at(p.position).range(1.0, 180.0).reverb(0.3),
        );
        let (lo, hi) = p.species.pause();
        let eager = if p.species == Species::Owl {
            1.0
        } else {
            chorus.eagerness
        };
        p.next_song = now + len as f64 + rng.range(lo, hi) as f64 / eager as f64;
        if !p.voice.is_none() {
            cues.write(AudioCue {
                voice: p.voice,
                position: Some(p.position),
                name: format!("bird.{}", p.species.name()),
                loudness: p.species.loudness(),
            });
        }
    }

    // Insects: two choruses either side, so the field around you is wide.
    let insects = chorus.crickets.max(chorus.cicadas);
    let sides = [DVec3::new(11.0, 0.0, 6.0), DVec3::new(-9.0, 0.0, -10.0)];
    for (id, side) in state.insects.iter_mut().zip(sides) {
        let ground = listener + side;
        let height = env
            .ground(ground)
            .map(|g| g.height)
            .unwrap_or(listener.y - 1.7);
        let at = DVec3::new(ground.x, height + 0.3, ground.z);
        if insects < 0.02 {
            if !id.is_none() {
                audio.stop(*id, 3.0);
            }
            *id = VoiceId::NONE;
            continue;
        }
        if !audio.is_playing(*id) {
            let seed = audio.next_seed();
            *id = audio.play(
                InsectChorus::new(fs, seed),
                PlayOptions::at(at).range(4.0, 60.0).fade_in(3.0),
            );
        } else {
            audio.set_position(*id, at, Vec3::ZERO);
            audio.set_last(*id, at);
        }
        audio.set_param(*id, "crickets", chorus.crickets);
        audio.set_param(*id, "cicadas", chorus.cicadas);
        audio.set_param(*id, "temperature", celsius);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn render(s: &mut impl Sound, secs: f32) -> Vec<f32> {
        let mut out = vec![0.0; (FS * secs) as usize];
        let n = s.fill(&mut out);
        out.truncate(n);
        out
    }

    /// Dominant pitch of the loud parts: zero crossings counted in the blocks whose level is
    /// within 10 dB of the loudest block.
    fn pitch(x: &[f32]) -> f32 {
        let blocks: Vec<&[f32]> = x.chunks(256).collect();
        let rms = |b: &[f32]| (b.iter().map(|v| v * v).sum::<f32>() / b.len() as f32).sqrt();
        let loudest = blocks.iter().map(|b| rms(b)).fold(0.0f32, f32::max);
        let mut crossings = 0;
        let mut samples = 0;
        for b in blocks.iter().filter(|b| rms(b) > loudest * 0.3) {
            samples += b.len();
            crossings += b
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
        }
        crossings as f32 / 2.0 / (samples.max(1) as f32 / FS)
    }

    #[test]
    fn every_bird_sings_in_its_own_range_and_stops() {
        for species in Species::ALL {
            let mut song = BirdSong::new(FS, species, 7, 1);
            let len = song.duration();
            assert!(len > 0.1 && len < 8.0, "{species:?} sings {len} s");
            let x = render(&mut song, 10.0);
            assert!((x.len() as f32 / FS) < len + 0.2, "{species:?} didn't stop");
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(x.iter().all(|v| v.is_finite()));
            assert!(peak > 0.05 && peak < 1.5, "{species:?} peak {peak}");
            let f = pitch(&x);
            let (lo, hi) = match species {
                Species::Warbler | Species::Chickadee | Species::Finch => (2_500.0, 8_000.0),
                Species::Thrush => (1_500.0, 7_000.0),
                Species::Dove | Species::Owl => (250.0, 900.0),
                Species::Cuckoo => (450.0, 1_000.0),
                Species::Crow => (400.0, 3_000.0),
                Species::Woodpecker => (300.0, 3_000.0),
            };
            assert!(f > lo && f < hi, "{species:?} at {f} Hz");
        }
    }

    #[test]
    fn a_bird_keeps_its_voice_from_song_to_song() {
        let a = render(&mut BirdSong::new(FS, Species::Chickadee, 3, 1), 2.0);
        let b = render(&mut BirdSong::new(FS, Species::Chickadee, 3, 2), 2.0);
        let other = render(&mut BirdSong::new(FS, Species::Chickadee, 99, 1), 2.0);
        let (pa, pb, po) = (pitch(&a), pitch(&b), pitch(&other));
        assert!((pa - pb).abs() < 30.0, "{pa} vs {pb}");
        assert!(
            (pa - po).abs() > 30.0,
            "two birds sound alike: {pa} vs {po}"
        );
    }

    fn chirps(celsius: f32) -> usize {
        let mut c = InsectChorus::new(FS, 5);
        c.set_param("crickets", 0.1); // one cricket
        c.set_param("temperature", celsius);
        c.value = c.target;
        let x = render(&mut c, 10.0);
        // Count chirps: bursts separated by more than 0.1 s of quiet.
        let mut count = 0;
        let mut quiet = usize::MAX;
        for v in x {
            if v.abs() > 0.01 {
                if quiet > (0.1 * FS) as usize {
                    count += 1;
                }
                quiet = 0;
            } else {
                quiet = quiet.saturating_add(1);
            }
        }
        count
    }

    #[test]
    fn crickets_chirp_faster_when_it_is_warm() {
        let cool = chirps(12.0);
        let warm = chirps(26.0);
        assert!(cool >= 5, "{cool} chirps at 12 °C");
        assert!(
            warm as f32 > cool as f32 * 1.6,
            "{cool} at 12 °C, {warm} at 26 °C"
        );
        assert_eq!(chirps(4.0), 0, "crickets are silent in the cold");
    }

    #[test]
    fn the_dawn_chorus_is_loudest_and_rain_and_towns_quiet_it() {
        // Sun 3° below the horizon at 5 am, in a wood, mild and still.
        let dawn = Chorus::new(-0.05, 5.0, 12.0, 0.0, false, 1.0, 1.0);
        let noon = Chorus::new(0.9, 12.5, 18.0, 0.0, false, 1.0, 1.0);
        let night = Chorus::new(-0.6, 1.0, 16.0, 0.0, false, 1.0, 1.0);
        let rain = Chorus::new(0.9, 12.5, 18.0, 6.0, false, 1.0, 1.0);
        let town = Chorus::new(0.9, 12.5, 18.0, 0.0, false, 1.0, 0.1);
        assert!(
            dawn.birds > 0.9 && dawn.eagerness > 2.0 && dawn.owls < 0.01,
            "{dawn:?}"
        );
        assert!(noon.birds > 0.3 && noon.birds < dawn.birds, "{noon:?}");
        assert!(
            night.birds < 0.01 && night.owls > 0.9 && night.crickets > 0.5,
            "{night:?}"
        );
        assert!(rain.birds < 0.05 && rain.crickets < 0.01, "{rain:?}");
        assert!(town.birds < noon.birds * 0.2, "{town:?}");
        let hot = Chorus::new(0.9, 14.0, 31.0, 0.0, false, 1.0, 1.0);
        assert!(hot.cicadas > 0.9 && noon.cicadas < 0.01, "{hot:?}");
    }
}
