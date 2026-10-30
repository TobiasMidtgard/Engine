//! The paddock's round-27 exhibits, run headless: the course runner runs both walls with its
//! feet on them and lands, the horse drinks at the trough with its muzzle at the water and
//! then grazes, and the dog shakes off, sniffs, sits, lies down from the sit and bows.

use engine_anim::actions::AnimalActions;
use engine_anim::course::{WALL_LEFT, WALL_RIGHT};
use engine_anim::demo::{DemoEntities, spawn_demo_world};
use engine_anim::{AnimationPlugin, Animator};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;

const STEP: f64 = 1.0 / 60.0;

fn app() -> (App, DemoEntities) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let demo = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    (app, demo)
}

fn bone_world(world: &World, e: Entity, bone: &str) -> Option<DVec3> {
    let a = world.get::<Animator>(e)?;
    let gt = world.get::<GlobalTransform>(e)?;
    Some(a.bone_world(gt, a.skeleton.index_of(bone)?).0)
}

fn in_state(world: &World, e: Entity, state: &str) -> bool {
    world
        .get::<Animator>(e)
        .and_then(|a| a.graph())
        .is_some_and(|g| g.in_state(state))
}

#[test]
fn the_runner_runs_both_walls_and_the_animals_act() {
    let (mut app, d) = app();
    let (mut left, mut right, mut feet_on_wall, mut landed) = (0, 0, 0, false);
    let mut worst_foot = 0.0f64;
    let mut actions_seen: Vec<String> = Vec::new();
    let mut lay_from_sit = false;
    for frame in 1..=(36.0 / STEP) as usize {
        app.update();
        let t = frame as f64 * STEP;
        let w = &app.world;
        if t < 6.0 {
            let r = d.wall_runner;
            if in_state(w, r, "wall_run_l") {
                left += 1;
            }
            if in_state(w, r, "wall_run_r") {
                right += 1;
            }
            if in_state(w, r, "land") || in_state(w, r, "land_hard") {
                landed = true;
            }
            // A foot the clip has firmly on the wall is at its face.
            let a = w.get::<Animator>(r).unwrap();
            for (curve, bone) in [("wall.foot_l", "foot_l"), ("wall.foot_r", "foot_r")] {
                if a.curve(curve) > 0.99 {
                    let p = bone_world(w, r, bone).unwrap();
                    let face = if p.z < 24.0 {
                        WALL_LEFT.0 as f64
                    } else {
                        WALL_RIGHT.0 as f64
                    };
                    // The sole is 8.5 cm out from the ankle.
                    let off = (p.z - face).abs() - 0.085;
                    worst_foot = worst_foot.max(off.abs());
                    feet_on_wall += 1;
                }
            }
        }
        for e in [d.horse, d.dog] {
            if let Some(p) = w.get::<AnimalActions>(e).and_then(|a| a.playing.clone()) {
                if !actions_seen.contains(&p.clip) {
                    actions_seen.push(p.clip.clone());
                }
                lay_from_sit |= p.clip == "dog_lie_from_sit";
            }
        }
    }
    assert!(
        left > 15 && right > 15,
        "wall runs: left {left}, right {right} frames"
    );
    assert!(feet_on_wall > 10, "{feet_on_wall}");
    assert!(
        worst_foot < 0.06,
        "a foot on the wall {worst_foot} m off its face"
    );
    assert!(landed, "no landing");
    for clip in [
        "horse_drink",
        "horse_graze",
        "dog_shake_off",
        "dog_sniff",
        "dog_sit",
        "dog_play_bow",
    ] {
        assert!(
            actions_seen.iter().any(|c| c == clip),
            "{clip} not played: {actions_seen:?}"
        );
    }
    assert!(
        lay_from_sit,
        "the dog lay down from the sit: {actions_seen:?}"
    );
}
