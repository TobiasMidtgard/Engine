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
    println!("wrote {}", out.display());
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
