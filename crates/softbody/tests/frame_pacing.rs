//! In play (a real clock) heavy cloth is solved beside the frame. A frame that runs two fixed
//! steps must not leave a body standing still for one and jumping the next: as with physics,
//! only a frame's last step hands its bodies to the job beside the frame; a step with another
//! after it in the same frame steps them there, so the next step never begins with a job that
//! had no time.
//!
//! `CLOTH_PACING_CSV=<path>` writes the drawn path of a free corner, frame by frame.

use engine_core::diagnostics::Diagnostics;
use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, ClothPins, SoftBodyConfig, SoftBodyPlugin, SoftBodySim};
use engine_world::WeatherState;

fn curtain() -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.resource_mut::<SoftBodyConfig>().beside_frame = Some(true);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::new(4.0, 0.0, 1.5);
    weather.gustiness = 0.0;
    let rod = app.world.spawn(Transform::from_xyz(0.0, 3.0, 0.0)).id();
    let e = app
        .world
        .spawn((
            Transform::IDENTITY,
            Cloth {
                size: [1.5, 2.0],
                resolution: [24, 32],
                pins: ClothPins::TopEdge { every: 2 },
                ..Cloth::default()
            },
            ChildOf(rod),
        ))
        .id();
    // Built and swinging before the clock turns real.
    app.update_n(30);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Realtime);
    (app, e)
}

#[test]
fn frames_of_two_steps_neither_wait_for_the_job_nor_hold_the_cloth() {
    let (mut app, e) = curtain();
    // A free corner: the last particle not pinned.
    let k = {
        let b = app.world.get::<SoftBodySim>(e).unwrap();
        (0..b.sim.len())
            .rfind(|&i| b.sim.inv_mass[i] > 0.0)
            .unwrap()
    };
    let mut rows = vec!["frame,ms,steps,held,x,y,z".to_string()];
    let (mut held_frames, mut two_step_frames, mut waited_ms) = (0, 0, 0.0f64);
    let mut last = std::time::Instant::now();
    for frame in 0..180 {
        // About 45 frames a second: one fixed step a frame, every third frame two.
        std::thread::sleep(std::time::Duration::from_millis(22));
        app.update();
        let ms = last.elapsed().as_secs_f64() * 1000.0;
        last = std::time::Instant::now();
        let fixed = app.world.resource::<FixedTime>();
        let (steps, alpha) = (fixed.steps_this_frame(), fixed.alpha());
        let diagnostics = app.world.resource::<Diagnostics>();
        let held = diagnostics.get("softbody.beside_frame_held").unwrap_or(0.0);
        waited_ms += diagnostics
            .get("softbody.beside_frame_wait_ms")
            .unwrap_or(0.0);
        let b = app.world.get::<SoftBodySim>(e).unwrap();
        let p = b.sim.origin + b.sim.interpolated(k, b.draw_alpha(alpha)).as_dvec3();
        if frame >= 20 {
            two_step_frames += usize::from(steps >= 2);
            held_frames += usize::from(held > 0.0);
        }
        rows.push(format!(
            "{frame},{ms:.1},{steps},{held},{:.4},{:.4},{:.4}",
            p.x, p.y, p.z
        ));
    }
    if let Ok(path) = std::env::var("CLOTH_PACING_CSV") {
        std::fs::write(path, rows.join("\n")).unwrap();
    }
    eprintln!(
        "two-step frames {two_step_frames}, frames holding the cloth {held_frames}, waited {waited_ms:.1} ms in all"
    );
    assert!(two_step_frames > 5, "the pacing gave frames of two steps");
    assert_eq!(
        held_frames, 0,
        "a job started on a frame's last step has the rest of the frame: none holds the cloth"
    );
}
