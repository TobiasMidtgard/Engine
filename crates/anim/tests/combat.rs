//! Combat moves: presses buffered and linked into a combo in cancel windows, the attacker
//! stepping in to range, blows landing on the target with hitstop, a raised guard blocking
//! them on the gloves (but not a hook to the body), a skilled target slipping and rolling
//! under them and countering, and mashing that doesn't queue a whole combo.

use engine_anim::combat::{AttackPhase, Attacker, Evader};
use engine_anim::demo::spawn_humanoid;
use engine_anim::{
    AnimationPlugin, Animator, HitReaction, Hurt, Knockdown, build::humanoid::Outfit,
};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::AnimationParams;

const DT: f32 = 1.0 / 60.0;

/// A boxer 1.1 m in front of a partner who faces it, the boxer in a guard (and the partner
/// with `guard`), settled.
fn ring(target: bool, guard: bool) -> (App, Entity, Entity) {
    ring_at(target, guard, DT)
}

/// [`ring`], stepping `step` seconds a frame.
fn ring_at(target: bool, guard: bool, step: f32) -> (App, Entity, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: step as f64,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let partner = spawn_humanoid(
        &mut app.world,
        Outfit::Shirt,
        Transform::default().with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
    );
    let boxer = spawn_humanoid(
        &mut app.world,
        Outfit::Athlete,
        Transform::from_xyz(0.0, 0.0, 1.1),
    );
    // The people's models (when fetched) are on before the first frame, so a run doesn't
    // depend on how fast they load.
    engine_anim::character::finish_loading(&mut app.world);
    app.world
        .entity_mut(boxer)
        .insert(Attacker::new("boxing", target.then_some(partner)));
    for (e, up) in [(boxer, true), (partner, guard)] {
        if up && let Some(mut p) = app.world.get_mut::<AnimationParams>(e) {
            p.set("guarding", 1.0);
        }
    }
    for _ in 0..30 {
        app.update();
    }
    (app, boxer, partner)
}

fn press(app: &mut App, e: Entity, input: &str) {
    app.world.get_mut::<Animator>(e).unwrap().trigger(input);
}

/// Run `secs`, pressing `presses` (seconds from now, input) on the way; the moves played in
/// order, hits landed, frames frozen.
fn fight(
    app: &mut App,
    boxer: Entity,
    secs: f32,
    presses: &[(f32, &str)],
) -> (Vec<String>, u32, u32) {
    let mut moves: Vec<String> = Vec::new();
    let (mut hits, mut frozen) = (0, 0);
    let mut last_hits = 0;
    let mut t = 0.0;
    while t < secs {
        for &(at, input) in presses {
            if at >= t && at < t + DT {
                press(app, boxer, input);
            }
        }
        app.update();
        let a = app.world.get::<Attacker>(boxer).unwrap().clone();
        if let Some(c) = &a.current
            && moves.last() != Some(c)
        {
            moves.push(c.clone());
            last_hits = 0;
        }
        if a.hits > last_hits {
            hits += a.hits - last_hits;
            last_hits = a.hits;
        }
        if app.world.get::<Animator>(boxer).unwrap().frozen() > 0.0 {
            frozen += 1;
        }
        t += DT;
    }
    (moves, hits, frozen)
}

#[test]
fn presses_link_into_a_combo_that_steps_in_and_lands() {
    let (mut app, boxer, partner) = ring(true, false);
    let (moves, hits, frozen) = fight(
        &mut app,
        boxer,
        3.5,
        &[
            (0.1, "attack_light"),
            (0.35, "attack_light"),
            (0.8, "attack_light"),
            (1.3, "attack_light"),
        ],
    );
    assert_eq!(moves, ["jab", "cross", "hook", "uppercut"]);
    assert_eq!(hits, 4, "only {hits} of 4 punches landed");
    // Hitstop: some frames frozen, but not many (2 to 6 frames a blow).
    assert!((8..=30).contains(&frozen), "{frozen} frozen frames");
    // It stepped in to striking range, and back out to sparring distance after the
    // uppercut, facing its partner.
    let gap = app
        .world
        .get::<GlobalTransform>(boxer)
        .unwrap()
        .translation()
        - app
            .world
            .get::<GlobalTransform>(partner)
            .unwrap()
            .translation();
    let gap = DVec3::new(gap.x, 0.0, gap.z).length();
    assert!((0.85..1.1).contains(&gap), "ended {gap} m from the partner");
    let a = app.world.get::<Attacker>(boxer).unwrap();
    assert_eq!(a.current, None);
    assert_eq!(a.phase, AttackPhase::Idle);
}

#[test]
fn an_early_press_waits_in_the_buffer_and_a_stale_one_is_dropped() {
    let (mut app, boxer, _) = ring(true, false);
    // The jab's cancel window opens about 0.2 s in: a press at 0.1 s is held until then.
    let (moves, ..) = fight(
        &mut app,
        boxer,
        1.5,
        &[(0.0, "attack_light"), (0.1, "attack_light")],
    );
    assert_eq!(moves, ["jab", "cross"]);
    let (mut app, boxer, _) = ring(true, false);
    // The cross's window opens about 0.26 s in: a press at 0.02 s has waited too long by
    // then and is dropped.
    let (moves, ..) = fight(
        &mut app,
        boxer,
        1.5,
        &[(0.0, "attack_heavy"), (0.02, "attack_light")],
    );
    assert_eq!(moves, ["cross"], "a stale press linked");
}

#[test]
fn mashing_plays_one_move_at_a_time_and_misses_without_a_target() {
    let (mut app, boxer, _) = ring(false, false);
    let presses: Vec<(f32, &str)> = (0..12).map(|k| (k as f32 * DT, "attack_light")).collect();
    let (moves, hits, frozen) = fight(&mut app, boxer, 1.0, &presses);
    // Twelve presses in 0.2 s: the first starts the jab, the newest waits for its cancel
    // window and links into the cross, and the rest are gone.
    assert_eq!(moves, ["jab", "cross"]);
    assert_eq!((hits, frozen), (0, 0));
}

#[test]
fn out_of_reach_without_warping_the_blows_miss() {
    let (mut app, boxer, partner) = ring(true, false);
    // Too far to reach and not allowed to step in: the arm reaches out and falls short.
    app.world.get_mut::<Attacker>(boxer).unwrap().warp = false;
    app.world.get_mut::<Transform>(boxer).unwrap().translation.z = 1.6;
    let (moves, hits, frozen) = fight(&mut app, boxer, 0.15, &[(0.0, "attack_light")]);
    assert_eq!(moves, ["jab"]);
    // In the warp window it publishes the step it would have taken, toward the partner.
    let a = app.world.get::<Attacker>(boxer).unwrap();
    assert!(a.warp_delta.z < 0.0, "warp asked for {}", a.warp_delta);
    let (_, more, frozen_more) = fight(&mut app, boxer, 1.0, &[]);
    assert_eq!((hits + more, frozen + frozen_more), (0, 0));
    let z = app.world.get::<Transform>(boxer).unwrap().translation.z;
    assert_eq!(z, 1.6, "it moved itself");
    let _ = partner;
}

#[test]
fn a_raised_guard_blocks_the_combo_on_the_gloves() {
    let (mut app, boxer, partner) = ring(true, true);
    app.world.entity_mut(boxer).insert(HitReaction::default());
    let presses = [
        (0.1, "attack_light"),
        (0.35, "attack_light"),
        (0.8, "attack_light"),
        (1.3, "attack_light"),
    ];
    let (mut blocked, mut last) = (0, 0);
    let (mut on_glove, mut aimed, mut bounced) = (0, 0, false);
    let mut t = 0.0;
    while t < 3.5 {
        for &(at, input) in &presses {
            if at >= t && at < t + DT {
                press(&mut app, boxer, input);
            }
        }
        app.update();
        let a = app.world.get::<Attacker>(boxer).unwrap().clone();
        if a.blocked != last {
            blocked += a.blocked.saturating_sub(last);
            last = a.blocked;
        }
        assert_eq!(a.hits, 0, "a punch got through the guard");
        // Strikes aim at the partner's gloves, not the face.
        if let Some((_, aim)) = a.aim {
            let p = app.world.get::<Animator>(partner).unwrap();
            let gt = app.world.get::<GlobalTransform>(partner).unwrap();
            let glove = ["hand_l", "hand_r"]
                .iter()
                .map(|h| p.bone_world(gt, p.skeleton.bone(h)).0.distance(aim))
                .fold(f64::INFINITY, f64::min);
            aimed += 1;
            if glove < 0.15 {
                on_glove += 1;
            }
        }
        // The boxer bounces back off the guard (it faces -Z, so back is +Z).
        bounced |= app.world.get::<HitReaction>(boxer).unwrap().stagger.0.z > 0.0;
        t += DT;
    }
    assert_eq!(blocked, 4, "{blocked} of 4 punches blocked");
    assert!(
        on_glove * 10 >= aimed * 9,
        "{on_glove} of {aimed} aims on a glove"
    );
    assert!(bounced, "the boxer wasn't pushed back");
    // The partner's struck arm flinched.
    assert!(app.world.get::<HitReaction>(partner).is_some());
}

#[test]
fn a_heavy_press_after_the_hook_finishes_hard_and_a_downed_target_is_left_alone() {
    let (mut app, boxer, partner) = ring(true, false);
    let (moves, hits, _) = fight(
        &mut app,
        boxer,
        3.5,
        &[
            (0.1, "attack_light"),
            (0.35, "attack_light"),
            (0.8, "attack_light"),
            (1.3, "attack_heavy"),
        ],
    );
    assert_eq!(moves, ["jab", "cross", "hook", "uppercut_heavy"]);
    assert_eq!(hits, 4, "only {hits} of 4 punches landed");
    // On the floor (no physics here, so knocked down by hand): presses are dropped.
    app.world.entity_mut(partner).insert(Knockdown::default());
    let (moves, ..) = fight(&mut app, boxer, 1.0, &[(0.1, "attack_light")]);
    assert!(moves.is_empty(), "attacked a downed target: {moves:?}");
    // Up again: back at it.
    app.world.entity_mut(partner).remove::<Knockdown>();
    let (moves, ..) = fight(&mut app, boxer, 1.0, &[(0.1, "attack_light")]);
    assert_eq!(moves, ["jab"]);
}

#[test]
fn a_body_hook_gets_under_the_guard_and_a_kneeling_target_is_left_alone() {
    let (mut app, boxer, partner) = ring(true, true);
    let mut blocked = 0;
    let mut last_blocked = 0;
    let mut moves: Vec<String> = Vec::new();
    let mut hits = 0;
    let mut t = 0.0;
    while t < 1.4 {
        if (0.1..0.1 + DT).contains(&t) {
            press(&mut app, boxer, "attack_light");
        }
        if (0.35..0.35 + DT).contains(&t) {
            press(&mut app, boxer, "attack_heavy");
        }
        app.update();
        let a = app.world.get::<Attacker>(boxer).unwrap().clone();
        if a.current.as_ref().is_some_and(|c| moves.last() != Some(c)) {
            moves.push(a.current.clone().unwrap());
        }
        if a.blocked > last_blocked {
            blocked += a.blocked - last_blocked;
        }
        last_blocked = a.blocked;
        if a.current.as_deref() == Some("body_hook") {
            hits = a.hits;
        }
        t += DT;
    }
    assert_eq!(moves, ["jab", "body_hook"]);
    assert_eq!((blocked, hits), (1, 1), "jab blocked, body hook landed");
    // Landed on the liver: down on a knee a moment later, and left alone there.
    let mut knelt = false;
    let mut attacked = false;
    for frame in 0..(1.0 / DT) as usize {
        if frame == 30 {
            press(&mut app, boxer, "attack_light");
        }
        app.update();
        let hurt = app.world.get::<Hurt>(partner).expect("hurt");
        knelt |= hurt.is_down();
        attacked |= app.world.get::<Attacker>(boxer).unwrap().current.is_some() && knelt;
    }
    assert!(knelt, "the liver shot didn't take a knee");
    assert!(!attacked, "attacked a kneeling target");
    // Up again: back at it.
    for _ in 0..(4.0 / DT) as usize {
        app.update();
    }
    assert!(!app.world.get::<Hurt>(partner).unwrap().is_down());
    let (moves, ..) = fight(&mut app, boxer, 1.0, &[(0.1, "attack_light")]);
    assert_eq!(moves, ["jab"]);
}

#[test]
fn at_eight_frames_a_second_short_hit_windows_still_land() {
    // A frame of 0.125 s is longer than a hit window (0.11 s): the body hook's falls between
    // two frames.
    let step = 0.125;
    let (mut app, boxer, partner) = ring_at(true, true, step);
    let mut moves: Vec<String> = Vec::new();
    let mut hook_hits = 0;
    for frame in 0..14 {
        if frame == 0 {
            press(&mut app, boxer, "attack_light");
        }
        if frame == 2 {
            press(&mut app, boxer, "attack_heavy");
        }
        app.update();
        let a = app.world.get::<Attacker>(boxer).unwrap().clone();
        if a.current.as_ref().is_some_and(|c| moves.last() != Some(c)) {
            moves.push(a.current.clone().unwrap());
        }
        if a.current.as_deref() == Some("body_hook") {
            hook_hits = a.hits;
        }
    }
    assert_eq!(moves, ["jab", "body_hook"]);
    assert_eq!(hook_hits, 1, "the body hook missed");
    let hurt = app.world.get::<Hurt>(partner).expect("hurt");
    assert!(hurt.liver.is_some() || hurt.is_down(), "not on the liver");
}

/// The combo at a partner who evades every strike it reads (when `evasive`), and counters:
/// the boxer's moves, its hits and misses, the evasions played, and the partner's moves and
/// hits.
fn at_an_evader(evasive: bool) -> (Vec<String>, u32, u32, Vec<String>, Vec<String>, u32) {
    let (mut app, boxer, partner) = ring(true, false);
    app.world
        .entity_mut(partner)
        .insert((Evader::new(1.0), Attacker::new("boxing", Some(boxer))));
    if let Some(mut p) = app.world.get_mut::<AnimationParams>(partner) {
        p.set("evasive", if evasive { 1.0 } else { 0.0 });
    }
    let (mut moves, mut evasions, mut counters) = (Vec::new(), Vec::new(), Vec::new());
    let (mut hits, mut missed, mut countered) = (0, 0, 0);
    let (mut last_hits, mut last_missed, mut last_countered) = (0, 0, 0);
    let mut t = 0.0;
    while t < 2.6 {
        for at in [0.1, 0.35, 0.8] {
            if (at..at + DT).contains(&t) {
                press(&mut app, boxer, "attack_light");
            }
        }
        app.update();
        let a = app.world.get::<Attacker>(boxer).unwrap().clone();
        if a.current.as_ref().is_some_and(|c| moves.last() != Some(c)) {
            moves.push(a.current.clone().unwrap());
            (last_hits, last_missed) = (0, 0);
        }
        hits += a.hits.saturating_sub(last_hits);
        missed += a.missed.saturating_sub(last_missed);
        (last_hits, last_missed) = (a.hits, a.missed);
        let e = app.world.get::<Evader>(partner).unwrap();
        if let Some((clip, ..)) = &e.evading
            && evasions.last() != Some(clip)
        {
            evasions.push(clip.clone());
        }
        let p = app.world.get::<Attacker>(partner).unwrap().clone();
        if p.current
            .as_ref()
            .is_some_and(|c| counters.last() != Some(c))
        {
            counters.push(p.current.clone().unwrap());
            last_countered = 0;
        }
        countered += p.hits.saturating_sub(last_countered);
        last_countered = p.hits;
        t += DT;
    }
    (moves, hits, missed, evasions, counters, countered)
}

#[test]
fn a_skilled_target_slips_rolls_under_the_combo_and_counters() {
    let (moves, hits, missed, evasions, counters, countered) = at_an_evader(true);
    assert_eq!(moves, ["jab", "cross", "hook"]);
    assert_eq!(
        (hits, missed),
        (0, 3),
        "{hits} landed, {missed} missed; {evasions:?} {counters:?}"
    );
    assert_eq!(evasions, ["slip_right", "slip_left", "roll"]);
    // Out of the roll, a cross back at the boxer, mid-hook and so not guarding.
    assert_eq!(counters, ["cross"]);
    assert_eq!(countered, 1, "the counter missed");
}

#[test]
fn with_evasive_off_the_combo_lands() {
    let (moves, hits, missed, evasions, counters, _) = at_an_evader(false);
    assert_eq!(moves, ["jab", "cross", "hook"]);
    assert_eq!((hits, missed), (3, 0), "{hits} landed, {missed} missed");
    assert!(evasions.is_empty() && counters.is_empty());
}
