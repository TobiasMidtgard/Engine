//! Footsteps synthesized and recorded, side by side, for every trail surface, and the dog's
//! paws and the horse's hooves on each.
//!
//! ```sh
//! cargo run -p fetch-assets -- audio          # the recordings (optional)
//! cargo run -p engine_audio --release --example footstep_ab -- [out_dir] [--takes] [--dir D]
//! ```
//!
//! For each surface writes `<out>/<surface>_before.wav` (six walking steps, synthesized
//! only), `<surface>_after.wav` (the same six steps with the recorded takes),
//! `<surface>_ab.wav` (both, a second apart) and `<surface>_response.wav` (recorded: light,
//! normal and heavy walkers, a run, then the ground soaked by rain and a puddle); and
//! `dog_<surface>_{before,after,ab}.wav` (16 paws of the trail's 22 kg dog at 1.8 m/s) and
//! `horse_<surface>_{before,after,ab}.wav` (10 hooves of its 500 kg horse at 1.7 m/s).
//! Without the fetched recordings "after" is the synthesis too. `--takes` also writes every
//! take of each surface (`takes_<surface>.wav`, `takes_paws_<ground>.wav`, ...), lists them
//! and writes each one's noise floor, tail level, length, where it was in its recording and
//! whether it was cleaned (`takes.csv`); `--dir` reads the
//! recordings from another folder than `assets/fetched/audio` (with `footsteps`, `paws` and
//! `hooves` in it).

use std::path::PathBuf;

use engine_audio::footstep::{FootstepSound, FootstepSurfaces, Step, render_mono};
use engine_audio::samples::{AUDIO_DIR, Feet, FootstepSamples, SampleBank};
use engine_audio::wav::write_wav;
use engine_world::{ContactKind, SurfaceCell};

const FS: f32 = 48_000.0;

const SURFACES: [&str; 11] = [
    "grass", "mud", "snow", "ice", "gravel", "wood", "metal", "concrete", "sand", "leaves", "water",
];

/// One step of a test walk: when (s), what touches down, load (kg), speed (m/s) and the
/// ground's state.
struct At {
    t: f32,
    kind: ContactKind,
    load: f32,
    speed: f32,
    cell: SurfaceCell,
}

fn walk(n: usize) -> Vec<At> {
    (0..n)
        .map(|k| At {
            t: 0.1 + k as f32 * 0.55,
            kind: ContactKind::Foot,
            load: 75.0,
            speed: 1.4,
            cell: SurfaceCell::default(),
        })
        .collect()
}

/// The trail's dog (22 kg, 1.8 m/s, a paw every 0.26 m) or horse (500 kg, 1.7 m/s, a hoof
/// every 0.45 m), half its weight on each foot that lands.
fn animal(kind: ContactKind, n: usize) -> Vec<At> {
    let (mass, speed, step_len) = match kind {
        ContactKind::Paw => (22.0, 1.8, 0.26),
        _ => (500.0, 1.7, 0.45),
    };
    (0..n)
        .map(|k| At {
            t: 0.1 + k as f32 * step_len / speed,
            kind,
            load: mass * 0.5,
            speed,
            cell: SurfaceCell::default(),
        })
        .collect()
}

fn response() -> Vec<At> {
    let mut v = Vec::new();
    let mut t = 0.1;
    let mut add = |n: usize, load: f32, speed: f32, gap: f32, cell: SurfaceCell| {
        for _ in 0..n {
            v.push(At {
                t,
                kind: ContactKind::Foot,
                load,
                speed,
                cell,
            });
            t += gap;
        }
        t += 0.4;
    };
    add(3, 40.0, 1.2, 0.55, SurfaceCell::default());
    add(3, 75.0, 1.4, 0.55, SurfaceCell::default());
    add(3, 130.0, 1.4, 0.6, SurfaceCell::default());
    add(5, 75.0, 4.5, 0.33, SurfaceCell::default());
    let wet = SurfaceCell {
        wetness: 1.0,
        ..Default::default()
    };
    add(3, 75.0, 1.4, 0.55, wet);
    let puddle = SurfaceCell {
        wetness: 1.0,
        puddle_m: 0.04,
        ..Default::default()
    };
    add(3, 75.0, 1.4, 0.55, puddle);
    v
}

fn render(
    surface: &str,
    steps: &[At],
    surfaces: &FootstepSurfaces,
    samples: Option<&mut FootstepSamples>,
) -> Vec<f32> {
    let len = steps.last().map(|s| s.t + 1.2).unwrap_or(1.0);
    let mut track = vec![0.0f32; (len * FS) as usize];
    let dry = surfaces.surfaces[surface];
    let snow = surfaces.snow();
    let mut samples = samples;
    for (k, at) in steps.iter().enumerate() {
        let sound = dry.with_state(&at.cell, 0.6, 12.0, 0.0, &snow);
        let step = Step::from_contact(sound, at.kind, at.load, at.speed, 1_000 + k as u64 * 77);
        let recorded = samples
            .as_deref_mut()
            .and_then(|s| s.recorded(surface, &dry, &at.cell, 0.0, surfaces, &step, FS, true));
        let mut voice = match &recorded {
            Some((synth, take)) => FootstepSound::with_sample(FS, synth, take),
            None => FootstepSound::new(FS, &step),
        };
        let x = render_mono(&mut voice, 3.0, FS);
        let start = (at.t * FS) as usize;
        for (i, v) in x.iter().enumerate() {
            if let Some(o) = track.get_mut(start + i) {
                *o += v;
            }
        }
    }
    track
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let dir = value("--dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets").join(AUDIO_DIR));
    let out = PathBuf::from(
        args.iter()
            .enumerate()
            .find(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--dir"))
            .map(|(_, a)| a.clone())
            .unwrap_or_else(|| "footstep_ab".into()),
    );
    let surfaces = FootstepSurfaces::default();
    let t0 = std::time::Instant::now();
    let bank = SampleBank::load_all(&dir);
    println!(
        "loaded {} takes from {} in {:.2} s: shoes on {} surfaces, paws on {} grounds, hooves \
         on {}",
        bank.len(),
        dir.display(),
        t0.elapsed().as_secs_f32(),
        bank.surfaces.len(),
        bank.paws.len(),
        bank.hooves.len(),
    );
    if args.iter().any(|a| a == "--takes") {
        // Every take's cleanliness and length in its recording, for measuring the cutter.
        // Shoes by surface; paws and hooves as `paws_<ground>`, `hooves_<ground>`.
        let named = |feet: Feet, name: &str| match feet {
            Feet::Shoes => name.to_string(),
            _ => format!("{}_{name}", feet.folder()),
        };
        let all_sets: Vec<(String, &engine_audio::samples::TakeSet)> = Feet::ALL
            .iter()
            .flat_map(|f| bank.sets(*f).iter().map(move |(k, v)| (named(*f, k), v)))
            .collect();
        let mut csv = String::from(
            "surface,source,gait,floor_db,tail_db,len_40_s,len_s,at_s,cleaned,raw_floor_db\n",
        );
        for (name, set) in &all_sets {
            for t in &set.takes {
                let s = t.stats;
                csv += &format!(
                    "{name},{},{:?},{:.1},{:.1},{:.3},{:.3},{:.3},{},{:.1}\n",
                    t.source,
                    t.gait,
                    s.floor_db,
                    s.tail_db,
                    s.len_40_s,
                    s.len_s,
                    s.at_s,
                    s.cleaned,
                    s.raw_floor_db
                );
            }
        }
        std::fs::create_dir_all(&out).expect("create the output folder");
        std::fs::write(out.join("takes.csv"), csv).expect("write takes.csv");
        for (name, set) in &all_sets {
            let mut all = Vec::new();
            let mut sources: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
            for t in &set.takes {
                // Played back at the mixer's rate as recorded, at unit loudness.
                let mut ratio_pos = 0.0f64;
                let inc = (t.sample_rate / FS) as f64;
                while (ratio_pos as usize) < t.pcm.len() {
                    all.push(t.pcm[ratio_pos as usize] * 0.1);
                    ratio_pos += inc;
                }
                all.extend(std::iter::repeat_n(0.0, (0.15 * FS) as usize));
                let e = sources.entry(&t.source).or_default();
                match t.gait {
                    engine_audio::samples::Gait::Walk => e.0 += 1,
                    engine_audio::samples::Gait::Run => e.1 += 1,
                }
            }
            let list: Vec<String> = sources
                .iter()
                .map(|(s, (w, r))| format!("{s} {w} walk {r} run"))
                .collect();
            println!(
                "{name:<12} {:>3} takes: {}",
                set.takes.len(),
                list.join(", ")
            );
            write_wav(&out.join(format!("takes_{name}.wav")), &all, 1, FS as u32)
                .expect("write wav");
        }
    }
    let mut samples = FootstepSamples::from_bank(bank);
    for name in SURFACES {
        let before = render(name, &walk(6), &surfaces, None);
        let after = render(name, &walk(6), &surfaces, Some(&mut samples));
        let resp = render(name, &response(), &surfaces, Some(&mut samples));
        let mut ab = before.clone();
        ab.extend(std::iter::repeat_n(0.0, (0.6 * FS) as usize));
        ab.extend_from_slice(&after);
        for (suffix, x) in [
            ("before", &before),
            ("after", &after),
            ("ab", &ab),
            ("response", &resp),
        ] {
            write_wav(&out.join(format!("{name}_{suffix}.wav")), x, 1, FS as u32)
                .expect("write wav");
        }
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!(
            "{name:<9} peak before {:.3} after {:.3}",
            peak(&before),
            peak(&after)
        );
    }
    for (animal_name, kind, n) in [
        ("dog", ContactKind::Paw, 16),
        ("horse", ContactKind::Hoof, 10),
    ] {
        for name in SURFACES.iter().chain(["rock"].iter()) {
            let steps = animal(kind, n);
            let before = render(name, &steps, &surfaces, None);
            let after = render(name, &steps, &surfaces, Some(&mut samples));
            let mut ab = before.clone();
            ab.extend(std::iter::repeat_n(0.0, (0.6 * FS) as usize));
            ab.extend_from_slice(&after);
            for (suffix, x) in [("before", &before), ("after", &after), ("ab", &ab)] {
                write_wav(
                    &out.join(format!("{animal_name}_{name}_{suffix}.wav")),
                    x,
                    1,
                    FS as u32,
                )
                .expect("write wav");
            }
        }
    }
    println!("wrote {}", out.display());
}
