//! Build a motion-matching database for one of the paddock's people and report on it: each
//! move's length, pace and turn, then which moves a scripted walk (start, walk, turn, run,
//! stop) plays.
//!
//! `cargo run -p engine_anim --release --example motion_db -- [person] [moves dir]`
//! (person: athlete, lifter, reacher or walker; moves default to the fetched ones).

use engine_anim::character::{add_locomotion, add_motion_matching, load_human_file};
use engine_anim::demo::{character_model, fetched_path};
use engine_world::animation::AnimationParams;

fn main() {
    let mut args = std::env::args().skip(1);
    let person = args.next().unwrap_or_else(|| "lifter".into());
    let mut model = character_model(&person).expect("fetch the animation assets first");
    if let Some(dir) = args.next() {
        let set = if person == "athlete" { "f_" } else { "m_" };
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .expect("moves dir")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "glb")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(set))
            })
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        files.sort();
        model.moves = files;
    }
    let start = std::time::Instant::now();
    let mut c = load_human_file(&model.path).unwrap();
    add_locomotion(&mut c, &model.idle, &model.walk, &model.run).unwrap();
    let loaded = start.elapsed();
    add_motion_matching(&mut c, &model.moves).unwrap();
    let db = c.motion.as_ref().expect("no moves found");
    println!(
        "{}: {} moves, {} frames ({:.1} s), top speed {:.2} m/s; model {:.0} ms, database {:.0} ms",
        db.name,
        db.clip_names.len(),
        db.frame_count(),
        db.frame_count() as f32 / engine_anim::motion::FPS,
        db.top_speed,
        loaded.as_secs_f64() * 1e3,
        (start.elapsed() - loaded).as_secs_f64() * 1e3
    );
    for line in db.summary() {
        println!("  {line}");
    }
    let _ = fetched_path;
    if let Ok(name) = std::env::var("TRACE") {
        let first = db.first_frame(&name).expect("no such move");
        let mut f = first;
        while f < db.frame_count() && db.clip_of(f) == name {
            let t = db.trajectory(f);
            println!(
                "  @{:3} velocity {:5.2} {:5.2}  in 1/3 s {:5.2} {:5.2}  in 1 s {:5.2} {:5.2} facing {:5.2} {:5.2}",
                f - first,
                db.root_velocity(f).x,
                db.root_velocity(f).z,
                t[0].0.x,
                t[0].0.y,
                t[2].0.x,
                t[2].0.y,
                t[2].1.x,
                t[2].1.y
            );
            f += 4;
        }
        return;
    }

    // A scripted walk: stand, start walking, turn left, run, stop, turn round on the spot.
    let script: [(f32, f32, f32); 7] = [
        // (seconds, speed m/s, turn rate rad/s)
        (1.0, 0.0, 0.0),
        (2.5, 1.4, 0.0),
        (1.5, 1.4, 1.0),
        (2.5, 3.5, 0.0),
        (1.5, 0.0, 0.0),
        (1.5, 0.0, 2.0),
        (1.0, 0.0, 0.0),
    ];
    let mut params = AnimationParams::default();
    let mut player =
        engine_anim::motion::MotionPlayer::new(std::sync::Arc::new(db.clone()), &c.logic).unwrap();
    let mut pose = Vec::new();
    let mut events = Vec::new();
    let dt = 1.0 / 60.0;
    let mut speed = 0.0f32;
    let mut last = String::new();
    let mut t = 0.0;
    for (secs, want, turn) in script {
        let mut left = secs;
        while left > 0.0 {
            let accel = ((want - speed) / 0.5).clamp(-4.0, 4.0);
            speed = (speed + accel * dt).max(0.0);
            params.set("speed", speed);
            params.set("turn", (turn / 1.2).clamp(-1.0, 1.0));
            params.set("accel", engine_anim::body::accel_param(accel));
            player.update(&params, dt, &mut pose, &mut events);
            let clip = player
                .frame()
                .map_or("-".into(), |f| db.clip_of(f).to_string());
            if clip != last {
                println!("{t:5.2} s  speed {speed:4.2}  turn {turn:3.1}  → {clip}");
                if std::env::var("EXPLAIN").is_ok()
                    && let Some(f) = player.frame()
                {
                    for (name, at, pose, path) in db.explain(&params, f).into_iter().take(5) {
                        println!("           {name:50} @{at:3}  pose {pose:5.2}  path {path:5.2}");
                    }
                }
                last = clip;
            }
            left -= dt;
            t += dt;
        }
    }
}
