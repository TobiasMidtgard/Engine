//! Planted feet stay planted: a person walking at any speed, or turning on the spot, keeps
//! each foot still on the ground while it bears weight (foot locking), and a person turned
//! where it stands steps round instead of swivelling on its feet. Runs with or without the
//! fetched models.

use engine_anim::AnimationPlugin;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{BodyRequest, SkeletonPose};

const STEP: f64 = 1.0 / 60.0;

/// How the feet moved: completed stances (a foot down, then up again), the average and the
/// largest distance a foot slid during one, meters.
struct Stances {
    count: usize,
    mean: f64,
    worst: f64,
}

/// Move a person ahead at `speed` (m/s) while turning at `turn` (rad/s) for a few seconds,
/// and measure its planted feet.
fn stances(speed: f64, turn: f32) -> Stances {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let t = Transform::default();
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::human(0)))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    let mut low = [f64::MAX; 2];
    let mut track: [Vec<DVec3>; 2] = [Vec::new(), Vec::new()];
    for frame in 0..(6.0 / STEP) as usize {
        {
            let mut t = app.world.get_mut::<Transform>(e).unwrap();
            t.rotate_y(turn * STEP as f32);
            let ahead = (t.rotation * Vec3::NEG_Z).as_dvec3();
            t.translation += ahead * speed * STEP;
        }
        app.update();
        // After the first two seconds (getting going).
        if frame < 120 {
            continue;
        }
        let pose = app.world.get::<SkeletonPose>(e).unwrap();
        let gt = app.world.get::<GlobalTransform>(e).unwrap();
        for (k, name) in ["foot_l", "foot_r"].iter().enumerate() {
            let p = pose.bone_world(gt, pose.index_of(name).unwrap()).unwrap().0;
            low[k] = low[k].min(p.y);
            track[k].push(p);
        }
    }
    let (mut count, mut total, mut worst) = (0, 0.0, 0.0f64);
    for k in 0..2 {
        // A stance runs from the foot coming to rest on the ground to its lifting; a foot
        // still skimming in to land isn't standing yet, but a foot that never comes to rest
        // while down slides the whole way.
        let (mut slid, mut skid, mut last, mut planted) = (0.0, 0.0, None::<DVec3>, false);
        for p in &track[k] {
            if p.y < low[k] + 0.015 {
                if let Some(q) = last {
                    let d = (p - q).with_y(0.0).length();
                    planted |= d / STEP < 0.3;
                    if planted {
                        slid += d;
                    } else {
                        skid += d;
                    }
                }
                last = Some(*p);
            } else if last.take().is_some() {
                let slid = if planted { slid } else { skid };
                count += 1;
                total += slid;
                worst = worst.max(slid);
                (planted, skid) = (false, 0.0);
            }
            if last.is_none() {
                slid = 0.0;
            }
        }
    }
    Stances {
        count,
        mean: total / count.max(1) as f64,
        worst,
    }
}

#[test]
fn walking_and_running_feet_do_not_slide() {
    for (speed, turn) in [(0.8, 0.0), (1.4, 0.0), (2.5, 0.0), (4.0, 0.0), (1.4, 0.8)] {
        let s = stances(speed, turn);
        assert!(s.count >= 4, "{speed} m/s: only {} steps", s.count);
        assert!(
            s.worst < 0.01,
            "{speed} m/s turning {turn}: feet slid {:.3} m on average, {:.3} at worst",
            s.mean,
            s.worst
        );
    }
}

#[test]
fn turned_on_the_spot_a_person_steps_round() {
    let s = stances(0.0, 1.0);
    assert!(s.count >= 4, "{} steps in 4 s of turning", s.count);
    assert!(
        s.worst < 0.03,
        "planted feet slid {:.3} m on average, {:.3} at worst",
        s.mean,
        s.worst
    );
    // Standing still, nothing moves.
    assert_eq!(stances(0.0, 0.0).count, 0);
}
