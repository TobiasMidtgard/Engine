//! Checks how the paddock's fighters hold their weapons: runs the paddock headless and, every
//! frame a hand holds something, measures the visible hand against the handle
//! ([`engine_anim::grip`]). Prints the worst of each over the run, and writes them as JSON.
//!
//! ```text
//! cargo run -p engine_anim --example grip_check --release -- [seconds] [report.json] [t,t,...]
//! ```
//!
//! At each of the times given last it prints where each holder's hands are (world), for
//! aiming close-ups.
//!
//! Needs the paddock's people (`cargo run -p fetch-assets -- animation`); without them the
//! generated bodies have no visible hands to check.

use engine_anim::AnimationPlugin;
use engine_anim::Animator;
use engine_anim::demo::spawn_demo_world;
use engine_anim::grip::{GripReport, HeldItems};
use engine_anim::retarget::Retarget;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;

/// The worst of a hand's reports over the run.
#[derive(Default, serde::Serialize)]
struct Worst {
    who: String,
    hand: &'static str,
    frames: usize,
    palm_min: f32,
    palm_max: f32,
    deepest: f32,
    widest: f32,
    thumb_unopposed_frames: usize,
    size: f32,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds: f64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(12.0);
    let out = args.get(2).cloned().filter(|p| !p.is_empty());
    let times: Vec<f64> = args
        .get(3)
        .map(|t| t.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default();
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let d = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    let people = [
        ("swordsman", d.swordsman),
        ("shield partner", d.shield_partner),
        ("great sword", d.greatsword),
        ("archer", d.archer),
    ];
    let mut worst: Vec<Worst> = Vec::new();
    for frame in 1..=(seconds * 60.0) as usize {
        app.update();
        let t = frame as f64 / 60.0;
        if times.iter().any(|&w| (w - t).abs() < 0.5 / 60.0) {
            for (who, e) in people {
                let w = &app.world;
                let (Some(a), Some(gt)) = (w.get::<Animator>(e), w.get::<GlobalTransform>(e))
                else {
                    continue;
                };
                let at = |b: &str| {
                    a.skeleton
                        .index_of(b)
                        .map(|i| engine_anim::ik::to_world(gt, a.model[i].t))
                };
                println!(
                    "t {t:.2} {who}: body {:.2} facing {:.2} hand_l {:.2?} hand_r {:.2?}",
                    gt.translation(),
                    gt.rotation() * Vec3::NEG_Z,
                    at("hand_l"),
                    at("hand_r"),
                );
            }
        }
        for (who, e) in people {
            let w = &app.world;
            let (Some(animator), Some(retarget), Some(held)) = (
                w.get::<Animator>(e),
                w.get::<Retarget>(e),
                w.get::<HeldItems>(e),
            ) else {
                continue;
            };
            for side in 0..2 {
                let Some(handle) = held.hands[side] else {
                    continue;
                };
                let Some(r) = retarget.retargeter.grip_report(
                    &animator.model,
                    &animator.skeleton,
                    &retarget.model,
                    side,
                    &handle,
                ) else {
                    continue;
                };
                let hand = ["left", "right"][side];
                let k = match worst.iter().position(|x| x.who == who && x.hand == hand) {
                    Some(k) => k,
                    None => {
                        worst.push(Worst {
                            who: who.into(),
                            hand,
                            palm_min: f32::MAX,
                            palm_max: f32::MIN,
                            ..Default::default()
                        });
                        worst.len() - 1
                    }
                };
                note(&mut worst[k], &r);
            }
        }
    }
    if worst.is_empty() {
        println!("no visible hands held anything (are the paddock's people fetched?)");
    }
    println!(
        "{:<15} {:<5} {:>6} {:>17} {:>8} {:>8} {:>10} {:>6}",
        "who", "hand", "frames", "palm gap mm", "deepest", "widest", "thumb off", "size"
    );
    for w in &worst {
        println!(
            "{:<15} {:<5} {:>6} {:>8.1}..{:<7.1} {:>8.1} {:>8.1} {:>10} {:>6.2}",
            w.who,
            w.hand,
            w.frames,
            w.palm_min * 1000.0,
            w.palm_max * 1000.0,
            w.deepest * 1000.0,
            w.widest * 1000.0,
            w.thumb_unopposed_frames,
            w.size,
        );
    }
    if let Some(path) = out {
        std::fs::write(&path, serde_json::to_string_pretty(&worst).unwrap()).unwrap();
        println!("wrote {path}");
    }
}

fn note(w: &mut Worst, r: &GripReport) {
    w.frames += 1;
    w.palm_min = w.palm_min.min(r.palm);
    w.palm_max = w.palm_max.max(r.palm);
    w.deepest = w.deepest.max(r.deepest);
    w.widest = w.widest.max(r.widest);
    w.thumb_unopposed_frames += usize::from(!r.thumb_opposed);
    w.size = r.size;
}
