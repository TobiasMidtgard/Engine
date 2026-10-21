//! Footsteps on worn paths: a walk on grass and on snow at several wear levels, recorded and
//! synthesized, for listening and for spectrograms.
//!
//! ```sh
//! cargo run -p fetch-assets -- audio          # the recordings (optional)
//! cargo run -p engine_audio --release --example footstep_worn -- [out_dir] [--dir D]
//! ```
//!
//! For `grass` at wear 0, 0.5 and 1 (12 °C), `snow` at 0 and 1 in the cold (-8 °C, `snow`)
//! and in mild weather (+2 °C, `mild_snow`) and wet grass (wetness 1) at 0 and 1, writes
//! `<out>/<ground>_w<wear>.wav` (eight walking steps with the recorded takes) and
//! `<ground>_w<wear>_synth.wav` (the same steps synthesized only); `grass_ab.wav` (wear 0,
//! 0.5, 1 a second apart), `snow_ab.wav`, `mild_snow_ab.wav`, `wet_ab.wav` (0, 1),
//! `grass_walk24_w<wear>.wav` (24 steps on grass worn 0, 0.5 and 1, for how alike its steps
//! are),
//! `crossing.wav` (twenty steps along a meadow that wears from untouched to a bare path and
//! back) and `wheels.wav` (a car's tyre at 8 m/s across a meadow, onto a worn path and off,
//! then on powder snow onto a packed track, at -8 °C). Without the fetched recordings the
//! takes are synthesized too. Prints each file's level.

use std::path::PathBuf;

use engine_audio::footstep::{FootstepSound, FootstepSurfaces, Step, render_mono};
use engine_audio::mixer::Sound;
use engine_audio::samples::{AUDIO_DIR, FootstepSamples, SampleBank};
use engine_audio::vehicle::{TyreGround, TyreSound};
use engine_audio::wav::write_wav;
use engine_world::{ContactKind, SurfaceCell, SurfaceMaterials};

const FS: f32 = 48_000.0;
const GAP_S: f32 = 0.55;

/// `wears.len()` walking steps on `ground` at `temperature_c`, the k-th on ground worn
/// `wears[k]` deep.
fn walk(
    ground: &str,
    wears: &[f32],
    cell: &SurfaceCell,
    temperature_c: f32,
    surfaces: &FootstepSurfaces,
    samples: Option<&mut FootstepSamples>,
) -> Vec<f32> {
    let mats = SurfaceMaterials::default();
    let material = mats.get(mats.id(ground).expect("a default material"));
    let len = wears.len() as f32 * GAP_S + 1.2;
    let mut track = vec![0.0f32; (len * FS) as usize];
    let dry = surfaces.get(material);
    let mut samples = samples;
    for (k, &wear) in wears.iter().enumerate() {
        let worn = surfaces.worn_path(material, wear, cell);
        let sound = worn.apply(&dry).with_state(
            cell,
            worn.absorbency(material.absorbency),
            temperature_c,
            0.0,
            &worn.snow,
        );
        let step = Step::from_contact(sound, ContactKind::Foot, 75.0, 1.4, 4_000 + k as u64 * 131);
        let recorded = samples.as_deref_mut().and_then(|s| {
            s.recorded(
                &material.name,
                &dry,
                &worn,
                cell,
                0.0,
                surfaces,
                &step,
                FS,
                true,
            )
        });
        let mut voice = match &recorded {
            Some((synth, take)) => FootstepSound::with_sample(FS, synth, take),
            None => FootstepSound::new(FS, &step),
        };
        let x = render_mono(&mut voice, 3.0, FS);
        let start = ((0.1 + k as f32 * GAP_S) * FS) as usize;
        for (i, v) in x.iter().enumerate() {
            if let Some(o) = track.get_mut(start + i) {
                *o += v;
            }
        }
    }
    track
}

fn join(parts: &[&[f32]]) -> Vec<f32> {
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.extend(std::iter::repeat_n(0.0, FS as usize));
        }
        out.extend_from_slice(p);
    }
    out
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
            .unwrap_or_else(|| "footstep_worn".into()),
    );
    std::fs::create_dir_all(&out).expect("create the output folder");
    let surfaces = FootstepSurfaces::default();
    let bank = SampleBank::load_all(&dir);
    println!("loaded {} takes from {}", bank.len(), dir.display());
    let dry = SurfaceCell::default();
    let wet = SurfaceCell {
        wetness: 1.0,
        ..Default::default()
    };
    let cases: [(&str, &str, &SurfaceCell, f32, &[f32]); 4] = [
        ("grass", "grass", &dry, 12.0, &[0.0, 0.5, 1.0]),
        ("snow", "snow", &dry, -8.0, &[0.0, 1.0]),
        ("mild_snow", "snow", &dry, 2.0, &[0.0, 1.0]),
        ("wet", "grass", &wet, 12.0, &[0.0, 1.0]),
    ];
    let rms_db = |x: &[f32]| {
        let e = x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32;
        10.0 * e.max(1e-12).log10()
    };
    for (label, ground, cell, temperature, wears) in cases {
        let mut ab = Vec::new();
        for &w in wears {
            // A fresh bank per file, so every file picks the same takes in the same order.
            let mut samples = FootstepSamples::from_bank(bank.clone());
            let rec = walk(
                ground,
                &[w; 8],
                cell,
                temperature,
                &surfaces,
                Some(&mut samples),
            );
            let synth = walk(ground, &[w; 8], cell, temperature, &surfaces, None);
            let name = format!("{label}_w{w:.1}");
            write_wav(&out.join(format!("{name}.wav")), &rec, 1, FS as u32).expect("write");
            write_wav(&out.join(format!("{name}_synth.wav")), &synth, 1, FS as u32).expect("write");
            println!(
                "{name:<10} recorded {:6.1} dB rms   synthesized {:6.1} dB rms",
                rms_db(&rec),
                rms_db(&synth)
            );
            ab.push(rec);
        }
        let parts: Vec<&[f32]> = ab.iter().map(|v| v.as_slice()).collect();
        write_wav(
            &out.join(format!("{label}_ab.wav")),
            &join(&parts),
            1,
            FS as u32,
        )
        .expect("write");
    }
    // Across a meadow and along a path worn into it, and off it again.
    let wears: Vec<f32> = (0..20)
        .map(|k| {
            let x = k as f32 / 19.0;
            (1.0 - (2.0 * x - 1.0).abs()) * 1.25
        })
        .map(|w: f32| w.min(1.0))
        .collect();
    for w in [0.0, 0.5, 1.0] {
        let mut samples = FootstepSamples::from_bank(bank.clone());
        let long = walk("grass", &[w; 24], &dry, 12.0, &surfaces, Some(&mut samples));
        let name = format!("grass_walk24_w{w:.1}.wav");
        write_wav(&out.join(name), &long, 1, FS as u32).expect("write");
    }
    let mut samples = FootstepSamples::from_bank(bank);
    let crossing = walk("grass", &wears, &dry, 12.0, &surfaces, Some(&mut samples));
    write_wav(&out.join("crossing.wav"), &crossing, 1, FS as u32).expect("write");
    println!(
        "crossing   wears {:?}",
        wears
            .iter()
            .map(|w| (w * 100.0).round() / 100.0)
            .collect::<Vec<_>>()
    );
    let wheels = wheels(&surfaces);
    write_wav(&out.join("wheels.wav"), &wheels, 1, FS as u32).expect("write");
    println!("wheels     {:6.1} dB rms", rms_db(&wheels));
}

/// The tyres' ground on `ground` worn `wear` deep at `temperature_c`, as the vehicle sounds
/// read it.
fn tyre_ground(
    surfaces: &FootstepSurfaces,
    ground: &str,
    wear: f32,
    temperature_c: f32,
) -> TyreGround {
    let mats = SurfaceMaterials::default();
    let m = mats.get(mats.id(ground).expect("a default material"));
    let cell = SurfaceCell::default();
    let w = surfaces.worn_path(m, wear, &cell);
    let s = w.apply(&surfaces.get(m)).with_state(
        &cell,
        w.absorbency(m.absorbency),
        temperature_c,
        0.0,
        &w.snow,
    );
    TyreGround::from_surface(&s, 0.0)
}

/// A tyre at 8 m/s: 4 s across a meadow (wear 0, then onto a path worn to 1, then off), a
/// second's gap, then 4 s on powder snow onto a packed track and off, at -8 °C.
fn wheels(surfaces: &FootstepSurfaces) -> Vec<f32> {
    let mut out = Vec::new();
    for (k, (ground, temperature)) in [("grass", 12.0), ("snow", -8.0)].into_iter().enumerate() {
        if k > 0 {
            out.extend(std::iter::repeat_n(0.0, FS as usize));
        }
        let mut tyre = TyreSound::new(FS, 11 + k as u64);
        tyre.set_param("speed", 8.0);
        tyre.set_param("load", 400.0);
        let blocks = (6.0 * FS / 480.0) as usize;
        for b in 0..blocks {
            let t = b as f32 * 480.0 / FS;
            // 0-1.5 s untouched, 2-4 s on the path, off again by 4.5 s.
            let wear = if t < 1.5 {
                0.0
            } else if t < 2.0 {
                (t - 1.5) / 0.5
            } else if t < 4.0 {
                1.0
            } else {
                (1.0 - (t - 4.0) / 0.5).max(0.0)
            };
            let g = tyre_ground(surfaces, ground, wear, temperature);
            for (name, v) in [
                ("hard", g.hard),
                ("gravel", g.gravel),
                ("soft", g.soft),
                ("grass", g.grass),
                ("mud", g.mud),
                ("snow", g.snow),
                ("packed", g.packed),
                ("squeak", g.squeak),
                ("wet", g.wet),
            ] {
                tyre.set_param(name, v);
            }
            let mut buf = [0.0f32; 480];
            tyre.fill(&mut buf);
            out.extend_from_slice(&buf);
        }
    }
    out
}
