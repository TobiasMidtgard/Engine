//! Footsteps synthesized and recorded, side by side, for every trail surface.
//!
//! ```sh
//! cargo run -p fetch-assets -- audio          # the recordings (optional)
//! cargo run -p engine_audio --release --example footstep_ab -- [out_dir] [--takes] [--dir D]
//! ```
//!
//! For each surface writes `<out>/<surface>_before.wav` (six walking steps, synthesized
//! only), `<surface>_after.wav` (the same six steps with the recorded takes),
//! `<surface>_ab.wav` (both, a second apart) and `<surface>_response.wav` (recorded: light,
//! normal and heavy walkers, a run, then the ground soaked by rain and a puddle). Without the
//! fetched recordings "after" is the synthesis too. `--takes` also writes every take of each
//! surface (`takes_<surface>.wav`), lists them and writes each one's noise floor, tail level
//! and length (`takes.csv`); `--dir` reads the recordings from another
//! folder than `assets/fetched/audio/footsteps`.

use std::path::PathBuf;

use engine_audio::footstep::{FootstepSound, FootstepSurfaces, Step, render_mono};
use engine_audio::samples::{FootstepSamples, SAMPLE_DIR, SampleBank};
use engine_audio::wav::write_wav;
use engine_world::{ContactKind, SurfaceCell};

const FS: f32 = 48_000.0;

const SURFACES: [&str; 11] = [
    "grass", "mud", "snow", "ice", "gravel", "wood", "metal", "concrete", "sand", "leaves", "water",
];

/// One step of a test walk: when (s), load (kg), speed (m/s) and the ground's state.
struct At {
    t: f32,
    load: f32,
    speed: f32,
    cell: SurfaceCell,
}

fn walk(n: usize) -> Vec<At> {
    (0..n)
        .map(|k| At {
            t: 0.1 + k as f32 * 0.55,
            load: 75.0,
            speed: 1.4,
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
        let step = Step::from_contact(
            sound,
            ContactKind::Foot,
            at.load,
            at.speed,
            1_000 + k as u64 * 77,
        );
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
        .unwrap_or_else(|| PathBuf::from("assets").join(SAMPLE_DIR));
    let out = PathBuf::from(
        args.iter()
            .enumerate()
            .find(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--dir"))
            .map(|(_, a)| a.clone())
            .unwrap_or_else(|| "footstep_ab".into()),
    );
    let surfaces = FootstepSurfaces::default();
    let t0 = std::time::Instant::now();
    let bank = SampleBank::load(&dir);
    println!(
        "loaded {} takes on {} surfaces from {} in {:.2} s",
        bank.len(),
        bank.surfaces.len(),
        dir.display(),
        t0.elapsed().as_secs_f32()
    );
    if args.iter().any(|a| a == "--takes") {
        // Every take's cleanliness and length in its recording, for measuring the cutter.
        let mut csv = String::from("surface,source,gait,floor_db,tail_db,len_40_s,len_s\n");
        for (name, set) in &bank.surfaces {
            for t in &set.takes {
                let s = t.stats;
                csv += &format!(
                    "{name},{},{:?},{:.1},{:.1},{:.3},{:.3}\n",
                    t.source, t.gait, s.floor_db, s.tail_db, s.len_40_s, s.len_s
                );
            }
        }
        std::fs::create_dir_all(&out).expect("create the output folder");
        std::fs::write(out.join("takes.csv"), csv).expect("write takes.csv");
        for (name, set) in &bank.surfaces {
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
                "{name:<9} {:>3} takes: {}",
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
    println!("wrote {}", out.display());
}
