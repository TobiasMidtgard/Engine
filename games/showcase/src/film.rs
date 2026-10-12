//! `--film key[,key…]`: a short film for people, where the tour is a contact sheet for checks.
//! With one place the camera holds there and slowly pushes in while the zone's demo plays; with
//! several it glides from each place to the next in the order given. Every frame is saved as
//! `screenshots/film/NNNNN.png` and what the camera hears as `screenshots/film/film.wav`. Both
//! run on the same simulated clock, so picture and sound line up at any frame rate.
//!
//! `--secs N` is the length of each shot or glide (default 8) and `--fps F` the frame rate
//! (default 30). Turn the frames into a video with:
//!
//! ```sh
//! ffmpeg -framerate 30 -i screenshots/film/%05d.png -i screenshots/film/film.wav \
//!     -c:v libx264 -pix_fmt yuv420p -crf 18 -c:a aac -shortest film.mp4
//! ```

use crate::camera::{self, ShowcaseCamera};
use crate::landscape;
use crate::places::{self, Place};
use engine::prelude::*;

/// Frames rendered before recording starts, so auto-exposure and the demos have settled.
const SETTLE_FRAMES: u32 = 8;
const OUT_DIR: &str = "screenshots/film";

#[derive(Resource, Debug)]
pub struct Film {
    pub places: Vec<Place>,
    /// Seconds per shot (one place) or per glide (between two places).
    pub secs: f32,
    pub fps: f32,
    pub frame: u32,
}

impl Film {
    /// Frames to record in all.
    fn total_frames(&self) -> u32 {
        let shots = self.places.len().saturating_sub(1).max(1);
        (shots as f32 * self.secs * self.fps).round() as u32
    }
}

/// Install the film: fixed time steps of one frame and fast-adapting exposure. Unknown keys are
/// reported and skipped.
pub fn install(app: &mut App, keys: &str, secs: f32, fps: f32) {
    let all = places::places();
    let mut list = Vec::new();
    for key in keys.split(',').filter(|k| !k.is_empty()) {
        match all.iter().find(|p| p.key == key) {
            Some(p) => list.push(p.clone()),
            None => engine::core::log::warn!("film: no place called \"{key}\""),
        }
    }
    let fps = fps.clamp(1.0, 120.0);
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: 1.0 / fps as f64,
        });
    app.insert_resource(Film {
        places: list,
        secs: secs.max(0.5),
        fps,
        frame: 0,
    });
    app.add_systems(Update, run.after(camera::fly));
}

/// Where the camera is `t` seconds into the film: eye, target and hour.
fn pose(places: &[Place], secs: f32, t: f32) -> (DVec3, DVec3, f32) {
    let first = &places[0];
    if places.len() == 1 {
        // Hold, pushing in gently so the shot doesn't feel frozen.
        let push = 0.06 * smootherstep((t / secs).clamp(0.0, 1.0)) as f64;
        return (
            first.position.lerp(first.target, push),
            first.target,
            first.hour,
        );
    }
    let legs = places.len() - 1;
    let leg = ((t / secs) as usize).min(legs - 1);
    let (a, b) = (&places[leg], &places[leg + 1]);
    let s = smootherstep((t / secs - leg as f32).clamp(0.0, 1.0)) as f64;
    let mut eye = a.position.lerp(b.position, s);
    // Arc up between far-apart places, and never dip into a hill on the way.
    let span = (b.position - a.position).length();
    eye.y += (std::f64::consts::PI * s).sin() * (span * 0.12).min(250.0);
    let ground = landscape::shared().height(eye.x, eye.z);
    eye.y = eye.y.max(ground + 2.0);
    let target = a.target.lerp(b.target, s);
    let hour = a.hour + (b.hour - a.hour) * s as f32;
    (eye, target, hour)
}

fn smootherstep(x: f32) -> f32 {
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

fn call(world: &mut World, name: &str, args: serde_json::Value) {
    if let Err(e) = engine::core::registry::call_api(world, name, args) {
        engine::core::log::warn!("film: {name}: {e}");
    }
}

fn run(world: &mut World) {
    let (places, secs, fps, frame, total) = {
        let film = world.resource::<Film>();
        (
            film.places.clone(),
            film.secs,
            film.fps,
            film.frame,
            film.total_frames(),
        )
    };
    if places.is_empty() {
        world.write_message(AppExit::Success);
        return;
    }
    if frame == 0 {
        if let Err(e) = std::fs::create_dir_all(OUT_DIR) {
            engine::core::log::warn!("film: can't create {OUT_DIR}: {e}");
        }
        places::visit(world, &places[0]);
        let mut cams = world.query::<&mut Camera>();
        for mut cam in cams.iter_mut(world) {
            if let Exposure::Auto { speed, .. } = &mut cam.exposure {
                *speed = 40.0;
            }
        }
    }
    let recorded = frame.saturating_sub(SETTLE_FRAMES);
    if frame >= SETTLE_FRAMES && recorded >= total {
        call(world, "audio.record", serde_json::json!({ "stop": true }));
        places::leave(world);
        engine::core::log::info!("film: {total} frames in {OUT_DIR}");
        world.write_message(AppExit::Success);
        return;
    }

    let t = recorded as f32 / fps;
    // Each glide runs its destination's arrival calls (weather, demo restarts) as it leaves.
    if places.len() > 1 && frame >= SETTLE_FRAMES && recorded > 0 {
        let leg = |f: u32| ((f as f32 / fps / secs) as usize).min(places.len() - 2);
        if leg(recorded) != leg(recorded - 1) {
            places::visit(world, &places[leg(recorded)]);
        }
    }
    let (eye, target, hour) = pose(&places, secs, t);
    let mut q = world.query::<(&mut Transform, &mut ShowcaseCamera)>();
    for (mut tf, mut cam) in q.iter_mut(world) {
        camera::look(&mut tf, &mut cam, eye, target);
    }
    world.resource_mut::<engine_weather::Sky>().hour = hour;

    if frame == SETTLE_FRAMES {
        let path = format!("{OUT_DIR}/film.wav");
        call(
            world,
            "audio.record",
            serde_json::json!({ "path": path, "seconds": total as f32 / fps }),
        );
    }
    if frame >= SETTLE_FRAMES {
        world
            .resource_mut::<Screenshots>()
            .request(format!("{OUT_DIR}/{recorded:05}.png"));
        if recorded % (fps as u32 * 2).max(1) == 0 {
            engine::core::log::info!("film: frame {recorded}/{total}");
        }
    }
    world.resource_mut::<Film>().frame += 1;
}
