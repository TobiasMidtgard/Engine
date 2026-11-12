//! CPU cost of a crowd of animated people without a renderer: `count` people asked for with
//! `BodyRequest`, walking circles at spread-out distances from the simulation focus (5 m to
//! `far` m), some crouched, some trotting. Prints the average frame time with the crowd and
//! without it (the same app, bodies taken away), so the difference is the animation's cost.
//!
//! ```sh
//! cargo run -p engine_anim --release --example crowd_bench -- 40 60 20   # count, far (m), secs
//! ```

use engine_anim::AnimationPlugin;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{AnimationParams, BodyRequest};
use engine_world::locomotion::MoveIntent;
use engine_world::{SimulationFocus, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

fn main() {
    let mut args = std::env::args()
        .skip(1)
        .map(|a| a.parse::<f64>().expect("numbers: count far secs"));
    let count = args.next().unwrap_or(40.0) as usize;
    let far = args.next().unwrap_or(60.0);
    let secs = args.next().unwrap_or(20.0);

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let focus = Transform::from_xyz(0.0, 1.7, 0.0);
    app.world.spawn((
        focus,
        GlobalTransform::from(focus),
        SimulationFocus::default(),
    ));
    // (centre, radius, speed m/s, crouched)
    let people: Vec<(Entity, DVec3, f64, f64, bool)> = (0..count)
        .map(|i| {
            let k = i as f64 / count.max(1) as f64;
            let dist = 5.0 + (far - 5.0) * k;
            let angle = i as f64 * 2.399;
            let centre = DVec3::new(dist * angle.cos(), 0.0, dist * angle.sin());
            let speed = [0.0, 1.3, 1.3, 3.5, 1.0][i % 5];
            let crouched = i % 5 == 4;
            let t = Transform::from_translation(centre);
            let e = app
                .world
                .spawn((
                    t,
                    GlobalTransform::from(t),
                    BodyRequest::new("human").with_seed(i as u64),
                    AnimationParams::default(),
                    MoveIntent {
                        crouch: crouched,
                        ..Default::default()
                    },
                ))
                .id();
            (e, centre, 2.0 + (i % 3) as f64, speed, crouched)
        })
        .collect();
    let mut t = 0.0;
    let mut step = |app: &mut App, t: &mut f64| {
        *t += STEP;
        for &(e, centre, radius, speed, _) in &people {
            if app.world.get_entity(e).is_err() {
                continue;
            }
            let a = *t * speed / radius;
            let at = centre + DVec3::new(radius * a.cos(), 0.0, radius * a.sin());
            let ahead = DVec3::new(-a.sin(), 0.0, a.cos());
            let tr = Transform::from_translation(at).looking_at(at + ahead, Vec3::Y);
            if let Some(mut x) = app.world.get_mut::<Transform>(e) {
                *x = tr;
            }
        }
        app.update();
    };
    // Bodies build and settle first.
    for _ in 0..120 {
        step(&mut app, &mut t);
    }
    let frames = (secs / STEP) as usize;
    let time = |app: &mut App, t: &mut f64, step: &mut dyn FnMut(&mut App, &mut f64)| {
        let start = std::time::Instant::now();
        for _ in 0..frames {
            step(app, t);
        }
        start.elapsed().as_secs_f64() * 1000.0 / frames as f64
    };
    let with = time(&mut app, &mut t, &mut step);
    for &(e, ..) in &people {
        app.world.entity_mut(e).despawn();
    }
    for _ in 0..10 {
        step(&mut app, &mut t);
    }
    let without = time(&mut app, &mut t, &mut step);
    println!(
        "{count} people out to {far} m: {with:.3} ms a frame, {without:.3} ms without them, \
         {:.3} ms for the crowd ({:.1} µs a person)",
        with - without,
        (with - without) * 1000.0 / count.max(1) as f64
    );
}
