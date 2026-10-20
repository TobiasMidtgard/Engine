//! People lean into a climb and, going down stairs, soften their knees, drop their chin to
//! watch the steps and hold their arms a little out: the paddock walker on its ramp and
//! stairs.

use engine_anim::demo::spawn_demo_world;
use engine_anim::{AnimationPlugin, Animator};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::AnimationParams;

/// How far the head is tipped back (+) or forward (−), degrees.
fn head_pitch(world: &World, e: Entity) -> f32 {
    let a = world.get::<Animator>(e).unwrap();
    let up = a.model[a.skeleton.bone("head")].r * Vec3::Y;
    up.z.atan2(up.y).to_degrees()
}

#[test]
fn walker_leans_up_the_ramp_and_watches_the_stairs() {
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
    let e = d.walker;
    // Flat sand, then up the ramp (3-6.5 s), across the deck and down the stairs (9-12 s).
    let (mut flat, mut down) = (Vec::new(), Vec::new());
    let (mut most_up, mut most_down) = (0.0f32, 0.0f32);
    for frame in 1..=(12.0 * 60.0) as usize {
        app.update();
        let t = frame as f32 / 60.0;
        let w = &app.world;
        let grade = w
            .get::<AnimationParams>(e)
            .and_then(|p| p.values.get("grade").copied())
            .unwrap_or(0.0);
        most_up = most_up.max(grade);
        most_down = most_down.min(grade);
        if (1.0..2.5).contains(&t) {
            flat.push(head_pitch(w, e));
        }
        if (10.0..11.5).contains(&t) {
            down.push(head_pitch(w, e));
        }
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    assert!(most_up > 0.4, "the ramp read as grade {most_up:.2}");
    assert!(most_down < -0.6, "the stairs read as grade {most_down:.2}");
    let (flat, down) = (mean(&flat), mean(&down));
    assert!(
        down < flat - 8.0,
        "head at {down:.1}° down the stairs, {flat:.1}° on the flat"
    );
}

#[test]
fn a_person_reads_the_slope_from_its_motion() {
    use engine_world::animation::BodyRequest;
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let t = Transform::default();
    let e = app
        .world
        .spawn((t, GlobalTransform::from(t), BodyRequest::human(0)))
        .id();
    app.update();
    let (mut most, mut least) = (0.0f32, 0.0f32);
    // Walk at 1.2 m/s up a 30% slope for 2 s, then back down it.
    for frame in 0..240 {
        let rise = if frame < 120 { 0.3 } else { -0.3 };
        let mut tr = app.world.get_mut::<Transform>(e).unwrap();
        tr.translation.z -= 1.2 / 60.0;
        tr.translation.y += rise * 1.2 / 60.0;
        app.update();
        let grade = app.world.get::<AnimationParams>(e).unwrap().values["grade"];
        most = most.max(grade);
        least = least.min(grade);
    }
    assert!(most > 0.7, "the climb read as grade {most:.2}");
    assert!(least < -0.7, "the descent read as grade {least:.2}");
}
