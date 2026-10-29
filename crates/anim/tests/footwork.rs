//! Footwork in guard: a fighter in a sword stance steps in or back to its measure from a foe,
//! facing it, and circles round it when told, the legs walking under the stance's upper
//! body; it rests while its own move plays and stands its ground while the foe's does. Runs
//! without the fetched models.

use std::f32::consts::PI;

use engine_anim::combat::Attacker;
use engine_anim::demo::spawn_humanoid;
use engine_anim::footwork::Footwork;
use engine_anim::{AnimationPlugin, Animator, build::humanoid::Outfit};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::AnimationParams;

const DT: f32 = 1.0 / 60.0;

/// A fighter in sword stance `gap` m from a foe (at the origin, facing it), keeping its
/// measure with `Footwork`, settled.
fn yard(gap: f32) -> (App, Entity, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: DT as f64,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let foe = spawn_humanoid(
        &mut app.world,
        Outfit::Shirt,
        Transform::default().with_rotation(Quat::from_rotation_y(PI)),
    );
    let fighter = spawn_humanoid(
        &mut app.world,
        Outfit::Athlete,
        Transform::from_xyz(0.0, 0.0, gap as f64),
    );
    app.world.entity_mut(fighter).insert(Footwork {
        foe: Some(foe),
        ..Default::default()
    });
    set(&mut app, fighter, "sword_stance", 1.0);
    for _ in 0..10 {
        app.update();
    }
    (app, fighter, foe)
}

fn set(app: &mut App, who: Entity, name: &str, value: f32) {
    if let Some(mut p) = app.world.get_mut::<AnimationParams>(who) {
        p.set(name, value);
    }
}

fn param(app: &App, who: Entity, name: &str) -> f32 {
    app.world.get::<AnimationParams>(who).unwrap().get(name)
}

/// Where the fighter stands (flat) and how well it faces the foe (1 = square on).
fn spot(app: &App, fighter: Entity, foe: Entity) -> (Vec3, f32, f32) {
    let t = app.world.get::<Transform>(fighter).unwrap();
    let at = t.translation.as_vec3().with_y(0.0);
    let foe_at = app
        .world
        .get::<Transform>(foe)
        .unwrap()
        .translation
        .as_vec3()
        .with_y(0.0);
    let to = foe_at - at;
    let facing = (t.rotation * Vec3::NEG_Z).dot(to.normalize());
    (at, to.length(), facing)
}

#[test]
fn a_fighter_steps_in_and_back_to_its_measure_facing_its_foe() {
    for (gap, coming) in [(2.6, true), (0.8, false)] {
        let (mut app, fighter, foe) = yard(gap);
        let measure = Footwork::default().measure;
        let mut headings = Vec::new();
        let mut fastest: f32 = 0.0;
        for _ in 0..(4.0 / DT) as usize {
            app.update();
            let speed = param(&app, fighter, "speed");
            fastest = fastest.max(speed);
            if speed > 0.2 {
                headings.push(param(&app, fighter, "heading"));
            }
        }
        let (_, dist, facing) = spot(&app, fighter, foe);
        assert!(
            (dist - measure).abs() < 0.12,
            "from {gap} m it ended {dist:.2} m off (measure {measure})"
        );
        assert!(facing > 0.99, "from {gap} m it ended facing {facing:.3}");
        let fw = Footwork::default();
        let cap = if coming {
            fw.advance_speed
        } else {
            fw.retreat_speed
        };
        assert!(
            fastest > 0.3 && fastest <= cap + 0.01,
            "from {gap} m it stepped at up to {fastest:.2} m/s"
        );
        assert!(!headings.is_empty());
        for h in &headings {
            let off = if coming { h.abs() } else { PI - h.abs() };
            assert!(
                off < 0.25,
                "from {gap} m it stepped with heading {h:.2} (coming in: {coming})"
            );
        }
        assert_eq!(param(&app, fighter, "speed"), 0.0, "it never stopped");
    }
}

#[test]
fn circling_goes_round_the_foe_at_the_measure_facing_it() {
    let (mut app, fighter, foe) = yard(1.4);
    let (start, _, _) = spot(&app, fighter, foe);
    set(&mut app, fighter, "circle", 1.0);
    let mut worst_gap: f32 = 0.0;
    let mut worst_facing: f32 = 1.0;
    let mut headings = Vec::new();
    for frame in 0..(2.5 / DT) as usize {
        app.update();
        let (_, dist, facing) = spot(&app, fighter, foe);
        if frame as f32 * DT > 0.5 {
            worst_gap = worst_gap.max((dist - 1.4).abs());
            worst_facing = worst_facing.min(facing);
            headings.push(param(&app, fighter, "heading"));
        }
    }
    let (end, _, _) = spot(&app, fighter, foe);
    // Round to its left: anticlockwise seen from above, starting on +Z.
    let turned = (end.x.atan2(end.z) - start.x.atan2(start.z)).to_degrees();
    assert!(
        turned < -40.0,
        "it went {turned:.0}° round the foe (left is negative here)"
    );
    assert!(
        worst_gap < 0.15,
        "it drifted {worst_gap:.2} m off its measure"
    );
    assert!(worst_facing > 0.97, "it faced the foe {worst_facing:.3}");
    let mean = headings.iter().sum::<f32>() / headings.len() as f32;
    assert!(
        (mean - PI / 2.0).abs() < 0.3,
        "circling left had heading {mean:.2}"
    );
}

#[test]
fn in_sword_stance_the_fists_hold_the_guard_while_the_feet_step() {
    let (mut app, fighter, foe) = yard(1.4);
    // The guard standing: each fist in the chest's frame.
    for _ in 0..30 {
        app.update();
    }
    let fists = |app: &App| {
        let a = app.world.get::<Animator>(fighter).unwrap();
        let chest = a.model[a.skeleton.index_of("chest").unwrap()].inverse();
        ["hand_l", "hand_r"]
            .map(|h| chest.transform_point(a.model[a.skeleton.index_of(h).unwrap()].t))
    };
    let feet = |app: &App| {
        let a = app.world.get::<Animator>(fighter).unwrap();
        let gt = app.world.get::<GlobalTransform>(fighter).unwrap();
        let foot = a.skeleton.index_of("foot_l").unwrap();
        a.bone_world(gt, foot).0
    };
    let standing = fists(&app);
    set(&mut app, fighter, "circle", 1.0);
    let mut worst: f32 = 0.0;
    let mut lowest_foot = f64::MAX;
    let mut highest_foot = f64::MIN;
    let mut stepping_frames = 0;
    for _ in 0..(2.5 / DT) as usize {
        app.update();
        if param(&app, fighter, "speed") < 0.5 {
            continue;
        }
        stepping_frames += 1;
        let now = fists(&app);
        for (a, b) in now.iter().zip(standing.iter()) {
            worst = worst.max(a.distance(*b));
        }
        let y = feet(&app).y;
        lowest_foot = lowest_foot.min(y);
        highest_foot = highest_foot.max(y);
    }
    assert!(stepping_frames > 60, "stepped {stepping_frames} frames");
    assert!(
        worst < 0.05,
        "a fist moved {worst:.3} m off the stance's hold relative to the chest"
    );
    assert!(
        highest_foot - lowest_foot > 0.03,
        "the left foot never lifted ({:.3} m)",
        highest_foot - lowest_foot
    );
    let state = app
        .world
        .get::<Animator>(fighter)
        .unwrap()
        .graph()
        .map(|g| g.active_states())
        .unwrap_or_default();
    assert!(
        state.iter().any(|s| s == "sword_stance"),
        "left the stance: {state:?}"
    );
    let _ = foe;
}

#[test]
fn it_rests_during_its_own_move_and_stands_its_ground_during_the_foes() {
    // Its own move: the move's warp closes in, the footwork rests.
    let (mut app, fighter, foe) = yard(2.4);
    app.world
        .entity_mut(fighter)
        .insert(Attacker::new("sword_shield", Some(foe)));
    app.world
        .get_mut::<Animator>(fighter)
        .unwrap()
        .trigger("attack_light");
    let mut during = 0;
    for _ in 0..(1.2 / DT) as usize {
        app.update();
        let a = app.world.get::<Attacker>(fighter).unwrap();
        if a.current.is_some() {
            during += 1;
            let fw = app.world.get::<Footwork>(fighter).unwrap();
            assert!(
                !fw.active && fw.velocity == Vec3::ZERO,
                "stepping during its own move"
            );
        }
    }
    assert!(during > 10, "the move played {during} frames");

    // The foe's move, from too close: it doesn't step (the blow may knock it) until the move
    // ends, then backs off to its measure.
    let (mut app, fighter, foe) = yard(1.0);
    app.world.get_mut::<Footwork>(fighter).unwrap().enabled = false;
    app.world
        .entity_mut(foe)
        .insert(Attacker::new("sword_shield", Some(fighter)));
    set(&mut app, foe, "sword_stance", 1.0);
    for _ in 0..5 {
        app.update();
    }
    app.world.get_mut::<Footwork>(fighter).unwrap().enabled = true;
    app.world
        .get_mut::<Animator>(foe)
        .unwrap()
        .trigger("attack_light");
    let mut over = None;
    for frame in 0..(3.0 / DT) as usize {
        app.update();
        let attacking = app.world.get::<Attacker>(foe).unwrap().current.is_some();
        if attacking && over.is_none() {
            let v = app
                .world
                .get::<Footwork>(fighter)
                .unwrap()
                .velocity
                .length();
            assert!(
                v < 0.01,
                "it stepped at {v:.2} m/s during the foe's move (frame {frame})"
            );
        } else if !attacking {
            over.get_or_insert(frame);
        }
    }
    assert!(over.is_some(), "the foe's move never ended");
    let (_, dist, _) = spot(&app, fighter, foe);
    assert!(
        dist > 1.25,
        "it never backed off to its measure after the move ({dist:.2} m)"
    );
}
