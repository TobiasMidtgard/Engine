//! Render every footstep surface to WAV files so you can listen to them side by side.
//!
//! ```sh
//! cargo run -p engine_audio --release --example footstep_lab -- [out_dir]
//! ```
//!
//! Writes `<out_dir>/<surface>.wav` (four steps each), `<out_dir>/all_surfaces.wav` (every
//! surface in turn) and `<out_dir>/fire_{campfire,house,forest}.wav`, and prints level and
//! brightness for each surface.

use std::path::PathBuf;

use engine_audio::footstep::{FootstepSound, FootstepSurfaces, Step, render_mono};
use engine_audio::wav::write_wav;
use engine_world::ContactKind;

const FS: f32 = 48_000.0;

fn main() {
    if std::env::args().any(|a| a == "--layers") {
        layers();
        return;
    }
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "footstep_lab".into()),
    );
    let surfaces = FootstepSurfaces::default();
    let mut all = Vec::new();
    println!(
        "{:<10} {:>7} {:>7} {:>9} {:>7}",
        "surface", "peak", "rms", "bright Hz", "len s"
    );
    for (name, sound) in &surfaces.surfaces {
        let mut track = vec![0.0f32; (FS * 2.4) as usize];
        let mut stats = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for k in 0..4 {
            let step = Step::from_contact(*sound, ContactKind::Foot, 75.0, 1.4, k as u64 + 1);
            let x = render_mono(&mut FootstepSound::new(FS, &step), 3.0, FS);
            let start = (k as f32 * 0.55 * FS) as usize + 2400;
            for (i, v) in x.iter().enumerate() {
                if let Some(t) = track.get_mut(start + i) {
                    *t += v;
                }
            }
            let e: f32 = x.iter().map(|v| v * v).sum();
            let d: f32 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
            stats.0 = stats.0.max(x.iter().fold(0.0, |m, v| m.max(v.abs())));
            stats.1 += (e / x.len() as f32).sqrt() / 4.0;
            stats.2 += (d / e.max(1e-12)).sqrt() * FS / std::f32::consts::TAU / 4.0;
            stats.3 += x.len() as f32 / FS / 4.0;
        }
        println!(
            "{name:<10} {:>7.3} {:>7.4} {:>9.0} {:>7.2}",
            stats.0, stats.1, stats.2, stats.3
        );
        write_wav(&out.join(format!("{name}.wav")), &track, 1, FS as u32).expect("write wav");
        all.extend_from_slice(&track);
    }
    write_wav(&out.join("all_surfaces.wav"), &all, 1, FS as u32).expect("write wav");

    // Fires of three sizes, four seconds each.
    for (name, size) in [("campfire", 1.0), ("house", 100.0), ("forest", 2_000.0)] {
        use engine_audio::Sound;
        let mut fire = engine_audio::ambience::FireSound::new(FS, 5);
        fire.set_param("size", size);
        let mut track = vec![0.0; (FS * 4.0) as usize];
        fire.fill(&mut track);
        write_wav(&out.join(format!("fire_{name}.wav")), &track, 1, FS as u32).expect("write wav");
    }

    // Water: a calm and a windy lake shore, a brook and rapids, eight seconds each.
    for (name, waves, flow) in [
        ("lake_calm", 0.2, 0.0),
        ("lake_windy", 0.8, 0.0),
        ("brook", 0.0, 1.0),
        ("rapids", 0.0, 3.0),
    ] {
        use engine_audio::Sound;
        let mut water = engine_audio::ambience::WaterSound::new(FS, 6);
        water.set_param("waves", waves);
        water.set_param("flow", flow);
        let mut track = vec![0.0; (FS * 8.0) as usize];
        water.fill(&mut track);
        let tail = &track[track.len() / 4..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!("water {name:<10} rms {rms:.4} peak {peak:.3}");
        write_wav(&out.join(format!("water_{name}.wav")), &track, 1, FS as u32).expect("write wav");
    }

    // Thunder from a bolt 0.5, 2 and 8 km away, mixed roughly as the mixer would (inverse
    // distance, air absorption), with its level over time.
    for (name, km) in [("near", 0.5), ("mid", 2.0), ("far", 8.0)] {
        let track = thunder_heard(km * 1_000.0);
        let env: Vec<String> = track
            .chunks((FS * 1.0) as usize)
            .take(16)
            .map(|c| {
                let rms = (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt();
                format!("{:.0}", engine_audio::dsp::to_db(rms.max(1e-6)))
            })
            .collect();
        let peak = track.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!(
            "thunder {name:<5} peak {peak:.3}  dB per second: {}",
            env.join(" ")
        );
        write_wav(
            &out.join(format!("thunder_{name}.wav")),
            &track,
            1,
            FS as u32,
        )
        .expect("write wav");
    }
    println!("wrote {}", out.display());
}

/// A zigzag cloud-to-ground bolt `distance_m` away as heard from the origin, starting at the
/// first arrival.
fn thunder_heard(distance_m: f64) -> Vec<f32> {
    use engine_audio::Sound;
    use engine_audio::dsp::{Noise, OnePole};
    use engine_audio::thunder::{THUNDER_REF_M, thunder_parts};
    use engine_core::math::DVec3;
    let mut rng = Noise::new(4);
    let ground = DVec3::new(distance_m, 0.0, 300.0);
    let mut p = ground + DVec3::new(0.0, 3_000.0, 0.0);
    let mut bolt = vec![p];
    for i in 0..256 {
        let left = 256 - i;
        let step = (ground - p) / left as f64;
        p += step
            + DVec3::new(
                rng.range(-1.0, 1.0) as f64 * 18.0,
                0.0,
                rng.range(-1.0, 1.0) as f64 * 18.0,
            ) * (left > 1) as i32 as f64;
        bolt.push(if left == 1 { ground } else { p });
    }
    let listener = DVec3::new(0.0, 1.7, 0.0);
    let parts = thunder_parts(FS, &bolt, listener, 343.0, 30.0, 11);
    let first = parts.iter().map(|p| p.delay).fold(f32::MAX, f32::min);
    let mut mix = vec![0.0f32; (FS * 24.0) as usize];
    for mut part in parts {
        let d = part.position.distance(listener) as f32;
        let g = THUNDER_REF_M / d.max(THUNDER_REF_M);
        let mut lp = OnePole::new(FS, 20_000.0 / (1.0 + d * 0.006));
        let mut buf = vec![0.0; (FS * 22.0) as usize];
        let n = part.sound.fill(&mut buf);
        let start = ((part.delay - first) * FS) as usize;
        for (i, v) in buf[..n].iter().enumerate() {
            if let Some(m) = mix.get_mut(start + i) {
                *m += lp.process(*v) * g;
            }
        }
    }
    mix
}

/// Print each layer's level on its own (run with `--layers`).
fn layers() {
    use engine_audio::footstep::SurfaceSound;
    let names = [
        "thump", "click", "scuff", "grit", "crunch", "rustle", "crackle", "squelch", "suction",
        "splash", "hollow", "ring", "hiss", "squeak",
    ];
    for (i, n) in names.iter().enumerate() {
        let mut s = SurfaceSound::SILENT;
        let v = serde_json::to_value(s).unwrap();
        let mut v = v.as_object().unwrap().clone();
        v.insert(n.to_string(), 1.0.into());
        if *n == "squeak" {
            v.insert("crunch".into(), 0.001.into());
        }
        s = serde_json::from_value(serde_json::Value::Object(v)).unwrap();
        let _ = i;
        let mut peak = 0.0f32;
        let mut rms = 0.0f32;
        for seed in 0..8 {
            let st = Step::from_contact(s, ContactKind::Foot, 75.0, 1.4, seed);
            let x = render_mono(&mut FootstepSound::new(FS, &st), 3.0, FS);
            peak += x.iter().fold(0.0f32, |m, v| m.max(v.abs())) / 8.0;
            let e: f32 = x.iter().map(|v| v * v).sum();
            rms += (e / (0.3 * FS)).sqrt() / 8.0;
        }
        println!("{n:<8} peak {peak:.3}  rms(300ms) {rms:.4}");
    }
}
