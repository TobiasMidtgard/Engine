//! The yard's shot curtain (the one the pedestal gun fires through) as a player sees it: the
//! demo spawned as in the showcase, the focus in front of the curtains, and a real clock paced
//! at `fps` (or the manual clock the tours use). Prints how far its cloth stretches and how
//! fast it moves, frame by frame summed up.
//!
//! ```sh
//! cargo run -p engine_softbody --example shot_curtain --release -- [frames] [fps] [manual]
//! CSV=path cargo run ...   # per frame: ms, steps, max stretch, edges over 1.3x, mean speed
//! SNAP=300:path cargo run ...  # the curtain's particles and triangles at frame 300
//! ```

use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::SoftBodySim;
use engine_world::FocusPoints;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(600);
    let fps: f64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(45.0);
    let manual = args.get(3).is_some_and(|a| a == "manual");
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_world::WorldPlugin, engine_softbody::SoftBodyPlugin));
    app.world
        .insert_resource(engine_assets::Assets::<engine_render::Mesh>::default());
    // In play bodies are solved beside the frame.
    app.world
        .resource_mut::<engine_softbody::SoftBodyConfig>()
        .beside_frame = Some(!manual);
    let yard = DVec3::new(125.0, 0.0, -35.0);
    app.add_systems(Startup, move |mut commands: Commands| {
        engine_softbody::demo::spawn_demo(&mut commands, yard);
    });
    // Standing where the tour's curtains view stands.
    app.add_systems(PreUpdate, move |mut focus: ResMut<FocusPoints>| {
        focus.points.clear();
        focus.points.push(yard + DVec3::new(-12.0, 1.7, 15.0));
    });
    app.update_n(30);
    if !manual {
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Realtime);
    } else {
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / fps,
            });
    }
    let frame_time = Duration::from_secs_f64(1.0 / fps);
    let mut rows = vec!["frame,ms,steps,max_stretch,over_1_3,mean_speed".to_string()];
    let (mut worst, mut over_frames, mut speed_sum) = (0.0f32, 0usize, 0.0f32);
    let mut last = Instant::now();
    for f in 0..frames {
        let t = Instant::now();
        app.update();
        if !manual && let Some(rest) = frame_time.checked_sub(t.elapsed()) {
            std::thread::sleep(rest);
        }
        let ms = last.elapsed().as_secs_f64() * 1000.0;
        last = Instant::now();
        let steps = app.world.resource::<FixedTime>().steps_this_frame();
        let mut q = app.world.query::<(&Name, &SoftBodySim)>();
        let Some((_, b)) = q
            .iter(&app.world)
            .find(|(n, _)| n.as_str() == "Shot curtain")
        else {
            continue;
        };
        let sim = &b.sim;
        // SNAP=<frame>:<path>: the curtain's particles (x y z) and triangles at that frame.
        if let Ok(snap) = std::env::var("SNAP")
            && let Some((at, path)) = snap.split_once(':')
            && at.parse() == Ok(f)
        {
            let mut out = String::new();
            for p in &sim.pos {
                out += &format!("v {} {} {}\n", p.x, p.y, p.z);
            }
            for t in &sim.aero_triangles {
                out += &format!("f {} {} {}\n", t[0], t[1], t[2]);
            }
            std::fs::write(path, out).unwrap();
        }
        let (mut max, mut over) = (0.0f32, 0usize);
        for d in &sim.distances {
            if d.rest <= 0.0 {
                continue;
            }
            let r = (sim.pos[d.a as usize] - sim.pos[d.b as usize]).length() / d.rest;
            max = max.max(r);
            over += usize::from(r > 1.3);
        }
        let free: Vec<f32> = sim
            .vel
            .iter()
            .zip(&sim.inv_mass)
            .filter(|(_, w)| **w > 0.0)
            .map(|(v, _)| v.length())
            .collect();
        let speed = free.iter().sum::<f32>() / free.len().max(1) as f32;
        if f >= 60 {
            worst = worst.max(max);
            over_frames += usize::from(over > 0);
            speed_sum += speed;
        }
        rows.push(format!("{f},{ms:.1},{steps},{max:.3},{over},{speed:.3}"));
    }
    if let Ok(path) = std::env::var("CSV") {
        std::fs::write(path, rows.join("\n")).unwrap();
    }
    let counted = frames.saturating_sub(60).max(1);
    println!(
        "{}: worst stretch {worst:.2}x, frames with an edge over 1.3x {over_frames}/{counted}, mean speed {:.3} m/s",
        if manual { "manual clock" } else { "real clock" },
        speed_sum / counted as f32
    );
}
