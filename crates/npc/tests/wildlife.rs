//! Wildlife behavior tests: herds bolt from a runner but let a sneaker come closer, panic
//! spreads through a herd, wolf packs run prey down and feed, birds take off together and
//! land again, boars root about and run from a runner but turn and charge when cornered or
//! wounded, a sow charges to defend her piglets, wolves pick a piglet over its elders, a pack
//! singles out a wounded deer, flanks it and lies in wait for it and gives up a long chase,
//! a herd's alarm spreads within a reaction time, and a deer zigzags from a wolf at its heels.

use std::f32::consts::{PI, TAU};

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::wildlife::{
    ESCAPE_GAP, PACK_CHASE_SECS, PACK_REST_SECS, SCENT_REACH, TEST_SECS, ZIGZAG_RANGE,
};
use engine_npc::*;
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::physics::Collider;
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    app
}

fn animal(app: &mut App, species: Species, at: DVec3, herd: u32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Animal::new(species),
            Herd(herd),
        ))
        .id()
}

fn herd(app: &mut App, species: Species, center: DVec3, n: usize, id: u32) -> Vec<Entity> {
    (0..n)
        .map(|k| {
            let a = k as f64 / n as f64 * std::f64::consts::TAU;
            animal(
                app,
                species,
                center + DVec3::new(a.cos() * 2.5, 0.0, a.sin() * 2.5),
                id,
            )
        })
        .collect()
}

fn state(app: &App, e: Entity) -> AnimalState {
    app.world.get::<AnimalMind>(e).unwrap().state
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// Walk a stand-in player straight at `target` in `mode` until something in `watch` flees;
/// returns the distance at which the first one bolted.
fn approach(
    app: &mut App,
    from: DVec3,
    target: DVec3,
    mode: LocomotionMode,
    speed: f64,
    watch: &[Entity],
) -> Option<f64> {
    let player = app
        .world
        .spawn((
            Transform::from_translation(from),
            Faction::PLAYER,
            LocomotionState {
                mode,
                ..Default::default()
            },
        ))
        .id();
    let dir = (target - from).normalize();
    for _ in 0..60 * 60 {
        {
            let mut t = app.world.get_mut::<Transform>(player).unwrap();
            if t.translation.distance(target) > 1.5 {
                t.translation += dir * speed / 60.0;
            }
        }
        app.world
            .get_mut::<LocomotionState>(player)
            .unwrap()
            .velocity = (dir * speed).as_vec3();
        app.update();
        if watch.iter().any(|&e| state(app, e) == AnimalState::Flee) {
            let p = app.world.get::<Transform>(player).unwrap().translation;
            return Some(p.distance(target));
        }
    }
    None
}

#[test]
fn deer_bolt_from_a_runner_but_let_a_sneaker_closer() {
    let center = DVec3::new(0.0, 0.0, 0.0);
    let mut a = app();
    let deer = herd(&mut a, Species::Deer, center, 4, 1);
    a.update_n(60);
    let run = approach(
        &mut a,
        DVec3::new(0.0, 0.0, -60.0),
        center,
        LocomotionMode::Sprint,
        7.0,
        &deer,
    )
    .expect("a sprinter spooks them");

    let mut b = app();
    let deer = herd(&mut b, Species::Deer, center, 4, 1);
    b.update_n(60);
    let sneak = approach(
        &mut b,
        DVec3::new(0.0, 0.0, -60.0),
        center,
        LocomotionMode::Crouch,
        1.5,
        &deer,
    )
    .expect("even a sneaker spooks them up close");

    assert!(run > 25.0, "a sprinter should spook them early: {run:.1} m");
    assert!(
        sneak < run * 0.55,
        "a sneaker gets much closer: {sneak:.1} m vs {run:.1} m"
    );
}

#[test]
fn panic_spreads_through_the_herd() {
    let mut app = app();
    let deer = herd(&mut app, Species::Deer, DVec3::ZERO, 5, 2);
    app.update_n(60);
    // Shoot one from the side.
    let shooter = app.world.spawn(Transform::from_xyz(-40.0, 1.5, 0.0)).id();
    app.world.write_message(DamageEvent {
        target: deer[0],
        amount: 5.0,
        kind: DamageKind::Ballistic,
        point: pos(&app, deer[0]),
        impulse: Vec3::ZERO,
        source: Some(shooter),
    });
    app.update_n(60);
    let fleeing = deer
        .iter()
        .filter(|&&e| state(&app, e) == AnimalState::Flee)
        .count();
    assert_eq!(fleeing, deer.len(), "the whole herd runs");
    app.update_n(120);
    // Away from the shot (+X), together.
    for &e in &deer {
        assert!(pos(&app, e).x > 8.0, "{e:?} at {}", pos(&app, e));
    }
}

#[test]
fn a_wolf_pack_runs_prey_down_and_feeds() {
    let mut app = app();
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let deer = herd(&mut app, Species::Deer, DVec3::new(0.0, 0.0, 0.0), 3, 1);
    let wolves = herd(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -45.0), 3, 2);
    for &w in &wolves {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    let mut stalked = false;
    let mut chased = false;
    let mut killed = None;
    for _ in 0..60 * 90 {
        app.update();
        stalked |= wolves.iter().any(|&w| state(&app, w) == AnimalState::Stalk);
        chased |= wolves.iter().any(|&w| state(&app, w) == AnimalState::Chase);
        if let Some(&d) = deer.iter().find(|&&d| state(&app, d) == AnimalState::Dead) {
            killed = Some(d);
            break;
        }
    }
    assert!(stalked, "the pack stalks first");
    assert!(chased, "then charges");
    let kill = killed.expect("the pack should catch a deer within 90 s");
    app.update_n(60 * 6);
    let feeding = wolves
        .iter()
        .filter(|&&w| state(&app, w) == AnimalState::Feed)
        .count();
    assert!(feeding >= 2, "the pack feeds: {feeding}");
    let k = pos(&app, kill);
    for &w in &wolves {
        if state(&app, w) == AnimalState::Feed {
            assert!(pos(&app, w).distance(k) < 3.5);
        }
    }
}

#[test]
fn birds_take_off_together() {
    let mut app = app();
    let flock = herd(&mut app, Species::Bird, DVec3::ZERO, 8, 5);
    app.update_n(60);
    assert!(
        flock
            .iter()
            .all(|&b| state(&app, b) == AnimalState::Perched)
    );
    // Someone runs at the flock from the side: the nearest birds flush and lift the rest.
    let runner = app
        .world
        .spawn((
            Transform::from_xyz(-30.0, 0.0, 0.0),
            Faction::PLAYER,
            LocomotionState {
                mode: LocomotionMode::Run,
                velocity: Vec3::new(5.0, 0.0, 0.0),
                ..Default::default()
            },
        ))
        .id();
    let mut first = None;
    for frame in 0..60 * 8 {
        app.world
            .get_mut::<Transform>(runner)
            .unwrap()
            .translation
            .x += 5.0 / 60.0;
        app.update();
        let up = flock
            .iter()
            .filter(|&&b| state(&app, b) == AnimalState::Flying)
            .count();
        if up > 0 && first.is_none() {
            first = Some(frame);
        }
        if up == flock.len() {
            let spread = frame - first.unwrap();
            assert!(
                spread < 60,
                "the flock lifts within a second: {spread} frames"
            );
            app.update_n(90);
            let high = flock.iter().filter(|&&b| pos(&app, b).y > 3.0).count();
            assert!(high >= 6, "{high} are high in the air");
            return;
        }
    }
    panic!("the flock never took off");
}

#[test]
fn birds_settle_when_the_coast_is_clear() {
    let mut app = app();
    let flock = herd(&mut app, Species::Bird, DVec3::ZERO, 6, 5);
    app.update_n(30);
    // A gunshot scares them up; nothing follows.
    app.world.write_message(engine_world::NoiseEvent {
        position: DVec3::new(10.0, 1.0, 0.0),
        radius: 90.0,
        source: None,
    });
    app.update_n(60);
    assert!(flock.iter().all(|&b| state(&app, b) == AnimalState::Flying));
    app.update_n(60 * 40);
    let perched = flock
        .iter()
        .filter(|&&b| state(&app, b) == AnimalState::Perched)
        .count();
    assert!(perched >= 5, "{perched} landed again");
    for &b in &flock {
        assert!(pos(&app, b).distance(DVec3::ZERO) < 20.0);
    }
}

/// A boar (`male` a tusker, `young` a piglet) of sounder `herd`.
fn boar(app: &mut App, at: DVec3, herd: u32, male: bool, young: bool) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Animal {
                male,
                young,
                ..Animal::new(Species::Boar)
            },
            Herd(herd),
        ))
        .id()
}

/// A stand-in player at `at`, moving in `mode`.
fn player(app: &mut App, at: DVec3, mode: LocomotionMode) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Faction::PLAYER,
            LocomotionState {
                mode,
                ..Default::default()
            },
        ))
        .id()
}

/// Walk the stand-in player one step (1/60 s) toward `to` at `speed`, stopping 1 m short.
fn step_toward(app: &mut App, who: Entity, to: DVec3, speed: f64) {
    let at = pos(app, who);
    let d = DVec3::new(to.x - at.x, 0.0, to.z - at.z);
    let v = if d.length() > 1.0 {
        d.normalize() * speed
    } else {
        DVec3::ZERO
    };
    app.world.get_mut::<Transform>(who).unwrap().translation += v / 60.0;
    app.world.get_mut::<LocomotionState>(who).unwrap().velocity = v.as_vec3();
}

/// Whether a tusk (or a bite) from `from` landed on `on` lately.
fn struck(app: &App, from: Entity, on: Entity) -> bool {
    let msgs = app.world.resource::<Messages<DamageEvent>>();
    let mut cursor = msgs.get_cursor();
    cursor
        .read(msgs)
        .any(|d| d.target == on && d.source == Some(from))
}

#[test]
fn boars_root_about_and_wander_with_the_sounder() {
    let mut app = app();
    let home = DVec3::ZERO;
    let sounder: Vec<Entity> = (0..3)
        .map(|k| {
            let a = k as f64 * 2.1;
            boar(
                &mut app,
                DVec3::new(a.cos() * 2.0, 0.0, a.sin() * 2.0),
                40,
                false,
                false,
            )
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    // How far each went while rooting (shuffling on with its snout down).
    let mut rooted = vec![0.0; sounder.len()];
    let mut last: Vec<DVec3> = sounder.iter().map(|&e| pos(&app, e)).collect();
    // How far the boars spread from the middle of the sounder: the most, and on average.
    let (mut widest, mut spread, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for _ in 0..60 * 90 {
        app.update();
        let mid = sounder.iter().map(|&e| pos(&app, e)).sum::<DVec3>() / sounder.len() as f64;
        for &e in &sounder {
            let d = pos(&app, e).distance(mid);
            widest = widest.max(d);
            spread += d;
            n += 1.0;
        }
        for (k, &e) in sounder.iter().enumerate() {
            let s = state(&app, e);
            seen.insert(s);
            let p = pos(&app, e);
            if s == AnimalState::Graze {
                rooted[k] += p.distance(last[k]);
            }
            last[k] = p;
            assert!(
                p.distance(home) < 45.0,
                "{e:?} strayed from its range to {p}"
            );
        }
    }
    for s in [AnimalState::Graze, AnimalState::Wander] {
        assert!(seen.contains(&s), "{s:?} never seen: {seen:?}");
    }
    assert!(
        !seen.contains(&AnimalState::Flee) && !seen.contains(&AnimalState::Charge),
        "nothing to run from or fight: {seen:?}"
    );
    for (k, r) in rooted.iter().enumerate() {
        assert!(*r > 1.0, "boar {k} rooted in one spot ({r:.2} m)");
    }
    // And they keep together as a sounder.
    let spread = spread / n;
    eprintln!("the sounder spread {spread:.1} m on average, {widest:.1} m at most");
    assert!(
        widest < 15.0,
        "a boar strayed {widest:.1} m from the sounder"
    );
    assert!(spread < 8.0, "the sounder spread {spread:.1} m on average");
}

#[test]
fn boars_run_from_a_sprinter_in_the_open() {
    let mut app = app();
    let sounder: Vec<Entity> = (0..3)
        .map(|k| {
            boar(
                &mut app,
                DVec3::new(k as f64 * 2.0, 0.0, 0.0),
                40,
                k == 0,
                false,
            )
        })
        .collect();
    app.update_n(60);
    let runner = player(
        &mut app,
        DVec3::new(0.0, 0.0, -60.0),
        LocomotionMode::Sprint,
    );
    let mut fled_at = None;
    for _ in 0..60 * 14 {
        step_toward(&mut app, runner, DVec3::ZERO, 7.0);
        app.update();
        for &e in &sounder {
            let s = state(&app, e);
            assert_ne!(s, AnimalState::Charge, "{e:?} charged in the open");
            if s == AnimalState::Flee && fled_at.is_none() {
                fled_at = Some(pos(&app, runner).distance(pos(&app, e)));
            }
        }
    }
    let d = fled_at.expect("the sounder runs");
    assert!(d > 18.0, "a sprinter should spook them early: {d:.1} m");
    // And they got away.
    for &e in &sounder {
        let d = pos(&app, e).distance(pos(&app, runner));
        assert!(d > 15.0, "{e:?} only {d:.1} m from the runner");
    }
}

#[test]
fn a_cornered_boar_charges_and_slashes() {
    let mut app = app();
    // A closed pen 6 m across, the boar in one corner of it.
    for (c, half) in [
        (DVec3::new(0.0, 1.0, 3.5), Vec3::new(4.0, 1.0, 0.5)),
        (DVec3::new(0.0, 1.0, -3.5), Vec3::new(4.0, 1.0, 0.5)),
        (DVec3::new(3.5, 1.0, 0.0), Vec3::new(0.5, 1.0, 4.0)),
        (DVec3::new(-3.5, 1.0, 0.0), Vec3::new(0.5, 1.0, 4.0)),
    ] {
        app.world
            .spawn((Transform::from_translation(c), Collider::cuboid(half)));
    }
    let tusker = boar(&mut app, DVec3::new(-1.6, 0.0, -1.6), 40, true, false);
    app.update_n(30);
    // Someone walks in at it from the far corner.
    let man = player(&mut app, DVec3::new(2.0, 0.0, 2.0), LocomotionMode::Walk);
    let mut charged_from = None;
    let mut slashed = false;
    for _ in 0..60 * 6 {
        let at = pos(&app, tusker);
        step_toward(&mut app, man, at, 1.2);
        app.update();
        if state(&app, tusker) == AnimalState::Charge && charged_from.is_none() {
            charged_from = Some(pos(&app, man).distance(pos(&app, tusker)));
        }
        slashed |= struck(&app, tusker, man);
        if slashed {
            break;
        }
    }
    let d = charged_from.expect("a cornered boar charges");
    assert!(
        d > 3.0,
        "it turns when it finds no way out, not only when trodden on: {d:.1} m"
    );
    assert!(slashed, "and its tusks strike");
}

#[test]
fn a_wounded_boar_turns_on_whoever_hurt_it() {
    let mut app = app();
    let tusker = boar(&mut app, DVec3::ZERO, 40, true, false);
    app.update_n(60);
    let hunter = player(&mut app, DVec3::new(14.0, 0.0, 0.0), LocomotionMode::Idle);
    app.update_n(30);
    app.world.write_message(DamageEvent {
        target: tusker,
        amount: 20.0,
        kind: DamageKind::Ballistic,
        point: pos(&app, tusker),
        impulse: Vec3::ZERO,
        source: Some(hunter),
    });
    let mut charged = false;
    let mut slashed = false;
    for _ in 0..60 * 5 {
        app.update();
        charged |= state(&app, tusker) == AnimalState::Charge;
        slashed |= struck(&app, tusker, hunter);
    }
    assert!(charged, "the wounded boar charges");
    assert!(slashed, "and reaches the hunter");
    // Then it breaks off and runs before it may turn again.
    app.update_n(30);
    assert_ne!(state(&app, tusker), AnimalState::Charge);
}

/// Hurt `on` from `by` (a gunshot).
fn shoot(app: &mut App, by: Entity, on: Entity) {
    let point = pos(app, on);
    app.world.write_message(DamageEvent {
        target: on,
        amount: 20.0,
        kind: DamageKind::Ballistic,
        point,
        impulse: Vec3::ZERO,
        source: Some(by),
    });
}

#[test]
fn a_wounded_boar_turns_on_the_shooter_not_on_what_it_was_watching() {
    let mut app = app();
    let tusker = boar(&mut app, DVec3::ZERO, 40, true, false);
    app.update_n(60);
    // Someone crouched 10 m off on one side (it watches them), and a hunter crouched further
    // off on the other, who shoots it.
    let watched = player(
        &mut app,
        DVec3::new(-10.0, 0.0, 0.0),
        LocomotionMode::Crouch,
    );
    let hunter = player(&mut app, DVec3::new(14.0, 0.0, 0.0), LocomotionMode::Crouch);
    app.update_n(60);
    let mind = app.world.get::<AnimalMind>(tusker).unwrap();
    assert_eq!(mind.threat, Some(watched), "it watches the nearer one");
    assert_ne!(mind.state, AnimalState::Charge);
    shoot(&mut app, hunter, tusker);
    let (mut charged, mut slashed, mut wrong) = (false, false, false);
    for _ in 0..60 * 5 {
        app.update();
        charged |= state(&app, tusker) == AnimalState::Charge;
        slashed |= struck(&app, tusker, hunter);
        wrong |= struck(&app, tusker, watched);
    }
    assert!(charged, "the wounded boar charges");
    assert!(slashed, "the shooter");
    assert!(!wrong, "not the one it was watching");
}

#[test]
fn a_boar_hurt_by_something_it_cannot_find_stands_and_scents_for_it() {
    let mut app = app();
    let tusker = boar(&mut app, DVec3::ZERO, 40, true, false);
    app.update_n(60);
    // A blow from 3 m off, and no one there: it stands its ground facing that way, looking
    // and scenting, and slashes at nothing.
    let at = pos(&app, tusker);
    app.world.write_message(DamageEvent {
        target: tusker,
        amount: 20.0,
        kind: DamageKind::Ballistic,
        point: at + DVec3::new(3.0, 0.0, 0.0),
        impulse: Vec3::ZERO,
        source: None,
    });
    let mut looked = false;
    for f in 0..60 * 4 {
        app.update();
        let s = state(&app, tusker);
        assert_ne!(s, AnimalState::Charge, "it charged at nothing");
        looked |= s == AnimalState::Alert;
        if f == 60 * 2 {
            let moved = pos(&app, tusker).distance(at);
            assert!(moved < 1.0, "it ran {moved:.1} m instead of looking about");
        }
    }
    assert!(looked, "it stands and looks about");
    let mind = app.world.get::<AnimalMind>(tusker).unwrap();
    assert!(mind.bite.is_none(), "it struck at nothing");
}

#[test]
fn a_wounded_boar_runs_from_fire_rather_than_charge_into_it() {
    let mut app = app();
    let tusker = boar(&mut app, DVec3::ZERO, 40, true, false);
    app.update_n(60);
    // Shot from far off, so it runs; and it runs into burning grass.
    let hunter = player(&mut app, DVec3::new(0.0, 0.0, 60.0), LocomotionMode::Idle);
    app.update_n(30);
    shoot(&mut app, hunter, tusker);
    app.update_n(20);
    let at = pos(&app, tusker);
    {
        let mut heat = app.world.resource_mut::<engine_world::HeatField>();
        for x in -6..=6 {
            for z in -6..=6 {
                let c = heat
                    .grid
                    .cell_at(at + DVec3::new(f64::from(x), 0.0, f64::from(z)));
                heat.grid.set(c, 300.0);
            }
        }
    }
    let mut fled = false;
    for _ in 0..60 * 6 {
        app.update();
        let s = state(&app, tusker);
        assert_ne!(s, AnimalState::Charge, "it charged into the fire");
        fled |= s == AnimalState::Flee;
    }
    assert!(fled, "it runs from the fire");
}

#[test]
fn wolves_give_room_to_a_boar_charging_at_them_only() {
    // Across several spots (each its own draw of the animals' seeds).
    for k in 0..6 {
        let o = DVec3::new(k as f64 * 97.0, 0.0, k as f64 * 53.0);
        let mut app = app();
        let tusker = boar(&mut app, o, 40, true, false);
        app.update_n(60);
        // Shot from one side, it charges the hunter; a wolf turns up 5 m behind it as it
        // sets off, and stays put: the boar is not charging at it.
        let hunter = player(
            &mut app,
            o + DVec3::new(14.0, 0.0, 0.0),
            LocomotionMode::Crouch,
        );
        app.update_n(10);
        shoot(&mut app, hunter, tusker);
        let mut wolf = None;
        for _ in 0..90 {
            app.update();
            if wolf.is_none() && state(&app, tusker) == AnimalState::Charge {
                let at = pos(&app, tusker) - DVec3::new(5.0, 0.0, 0.0);
                wolf = Some(animal(&mut app, Species::Wolf, at, 2));
            }
            if let Some(w) = wolf {
                assert_ne!(
                    state(&app, w),
                    AnimalState::Flee,
                    "spot {k}: the wolf ran from a boar charging away from it"
                );
            }
        }
        wolf.expect("the boar charges the hunter");

        // Bitten by a wolf 15 m off, it charges the wolf, which gives it room. (Both wander
        // for a second first; the wolf is 15 m off when it bites.)
        let mut app = self::app();
        let tusker = boar(&mut app, o, 40, true, false);
        let wolf = animal(&mut app, Species::Wolf, o + DVec3::new(-15.0, 0.0, 0.0), 2);
        app.update_n(60);
        let at = pos(&app, tusker) - DVec3::new(15.0, 0.0, 0.0);
        app.world.get_mut::<Transform>(wolf).unwrap().translation = at;
        shoot(&mut app, wolf, tusker);
        let (mut charged, mut gave_way) = (false, false);
        for _ in 0..60 * 6 {
            app.update();
            charged |= state(&app, tusker) == AnimalState::Charge;
            gave_way |= charged && state(&app, wolf) == AnimalState::Flee;
        }
        assert!(charged, "spot {k}: the boar charges the wolf");
        assert!(gave_way, "spot {k}: and the wolf gives it room");
    }
}

/// A sow with `piglets` about her, and someone stepping out of cover 10 m away and walking
/// at her: whether she charged, whether she struck, whether all her piglets ran, and whether
/// she ran.
fn surprise(piglets: usize) -> (bool, bool, bool, bool) {
    let mut app = app();
    let sow = boar(&mut app, DVec3::ZERO, 40, false, false);
    let young: Vec<Entity> = (0..piglets)
        .map(|k| {
            let a = k as f64 * 2.0;
            boar(
                &mut app,
                DVec3::new(a.cos() * 2.0, 0.0, a.sin() * 2.0),
                40,
                false,
                true,
            )
        })
        .collect();
    app.update_n(60);
    let man = player(&mut app, DVec3::new(10.0, 0.0, 0.0), LocomotionMode::Walk);
    let (mut charged, mut slashed, mut fled) = (false, false, false);
    let mut scattered = vec![false; young.len()];
    for _ in 0..60 * 5 {
        step_toward(&mut app, man, DVec3::ZERO, 1.0);
        app.update();
        charged |= state(&app, sow) == AnimalState::Charge;
        fled |= state(&app, sow) == AnimalState::Flee;
        slashed |= struck(&app, sow, man);
        for (k, &p) in young.iter().enumerate() {
            let s = state(&app, p);
            assert_ne!(s, AnimalState::Charge, "piglets don't fight");
            scattered[k] |= s == AnimalState::Flee;
        }
    }
    (charged, slashed, scattered.iter().all(|&s| s), fled)
}

#[test]
fn a_sow_charges_to_defend_her_piglets() {
    let (charged, slashed, scattered, _) = surprise(3);
    assert!(charged, "the sow charges");
    assert!(slashed, "and strikes");
    assert!(scattered, "her piglets run");
    // Without young about her, she runs too.
    let (charged, _, _, fled) = surprise(0);
    assert!(!charged, "a sow on her own doesn't charge");
    assert!(fled, "a sow on her own runs");
}

#[test]
fn wolves_pick_a_piglet_over_its_elders() {
    let mut app = app();
    let sounder = [
        boar(&mut app, DVec3::new(0.0, 0.0, 0.0), 40, true, false),
        boar(&mut app, DVec3::new(2.0, 0.0, 1.0), 40, false, false),
        boar(&mut app, DVec3::new(1.0, 0.0, 3.0), 40, false, true),
    ];
    let wolves = herd(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -45.0), 3, 2);
    app.update();
    for &w in &wolves {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    for _ in 0..60 * 10 {
        app.update();
        if let Some(prey) = wolves
            .iter()
            .find_map(|&w| app.world.get::<AnimalMind>(w).unwrap().prey)
        {
            assert!(sounder.contains(&prey));
            assert!(
                app.world.get::<Animal>(prey).unwrap().young,
                "the pack goes for the piglet"
            );
            return;
        }
    }
    panic!("the pack never picked a quarry");
}

fn mind(app: &App, e: Entity) -> &AnimalMind {
    app.world.get::<AnimalMind>(e).unwrap()
}

fn flat(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, 0.0, v.z as f32)
}

/// A pack of `wolves` 45 m from a herd of `deer`, hungry.
fn hunt(app: &mut App, deer: usize, wolves: usize) -> (Vec<Entity>, Vec<Entity>) {
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let herd_ = herd(app, Species::Deer, DVec3::ZERO, deer, 1);
    let pack = herd(app, Species::Wolf, DVec3::new(0.0, 0.0, -45.0), wolves, 2);
    app.update();
    for &w in &pack {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    (herd_, pack)
}

/// Run until a deer is killed (or `secs` pass): which one, the pack's first pick, and every
/// quarry a running wolf was after.
fn run_down(
    app: &mut App,
    deer: &[Entity],
    pack: &[Entity],
    secs: f64,
) -> (Option<Entity>, Option<Entity>, Vec<Entity>) {
    let (mut first, mut chased) = (None, Vec::new());
    for _ in 0..(secs * 60.0) as usize {
        app.update();
        for &w in pack {
            let m = mind(app, w);
            first = first.or(m.prey);
            if m.state == AnimalState::Chase
                && let Some(p) = m.prey
                && !chased.contains(&p)
            {
                chased.push(p);
            }
        }
        if let Some(&d) = deer.iter().find(|&&d| state(app, d) == AnimalState::Dead) {
            return (Some(d), first, chased);
        }
    }
    (None, first, chased)
}

#[test]
fn a_pack_singles_out_the_wounded_deer() {
    for wolves in [3, 4, 5] {
        // The deer furthest from the pack is wounded: the pack picks it from the start, runs
        // it and only it, and catches it.
        let mut app = app();
        let (deer, pack) = hunt(&mut app, 4, wolves);
        let hurt = deer[1];
        for &d in &deer {
            assert!(
                pos(&app, d).z <= pos(&app, hurt).z,
                "the wounded deer is the furthest"
            );
        }
        {
            let mut v = app.world.get_mut::<NpcVitals>(hurt).unwrap();
            v.health = v.max_health * 0.4;
        }
        let (killed, first, chased) = run_down(&mut app, &deer, &pack, 90.0);
        assert_eq!(first, Some(hurt), "{wolves} wolves picked the wounded deer");
        assert_eq!(
            chased,
            vec![hurt],
            "{wolves} wolves ran only the wounded deer"
        );
        assert_eq!(
            killed,
            Some(hurt),
            "{wolves} wolves caught the wounded deer"
        );

        // Unhurt, it is the last they would pick: they take the nearest, and catch one.
        let mut app = self::app();
        let (deer, pack) = hunt(&mut app, 4, wolves);
        let (killed, first, _) = run_down(&mut app, &deer, &pack, 90.0);
        assert_eq!(first, Some(deer[3]), "{wolves} wolves, a healthy herd");
        assert!(killed.is_some(), "{wolves} wolves catch a healthy deer too");
    }
}

/// A pack of `wolves` 45 m from a herd of `deer`, hungry, every body seeded from draw `k`
/// (so a test can run the same hunt over several draws instead of one).
fn seeded_hunt(app: &mut App, deer: usize, wolves: usize, k: u64) -> (Vec<Entity>, Vec<Entity>) {
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let herd_ = herd(app, Species::Deer, DVec3::ZERO, deer, 1);
    let pack = herd(app, Species::Wolf, DVec3::new(0.0, 0.0, -45.0), wolves, 2);
    for (i, &e) in herd_.iter().chain(&pack).enumerate() {
        let s = (k + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (i as u64 + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        app.world.entity_mut(e).insert(Seed(s));
    }
    app.update();
    for &w in &pack {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    (herd_, pack)
}

/// How a pack's flankers ran. `chase` counts the steps the quarry ran from the pack; the
/// rest is over the flanking phase, from the rush until a wolf closes to 3 m or the quarry,
/// once at full sprint, tires below three quarters of it: its steps, the steps both flankers
/// were out on their own sides, the longest both ran abreast of it, the steps a flanker
/// running level with it (no more than 6 m behind) was on its far side, and the steps the
/// tester was behind it. Also, as the tester rushed, how far beyond the quarry the ambusher
/// lay and whether it was still lying in wait.
#[derive(Debug, Default)]
struct Flanking {
    chase: usize,
    phase: usize,
    sides: usize,
    abreast: usize,
    wrong_level: usize,
    tester_behind: usize,
    ambusher: Option<(f32, bool)>,
}

/// Runs `pack`'s hunt of `deer` until a kill (or 90 s), watching where each wolf is from the
/// quarry, across (toward a flanker's own side) and along its line of flight.
fn flanking(app: &mut App, deer: &[Entity], pack: &[Entity]) -> Flanking {
    let mut f = Flanking::default();
    let (mut rushed, mut open, mut fast, mut run) = (false, true, false, 0);
    for _ in 0..60 * 90 {
        app.update();
        if deer.iter().any(|&d| state(app, d) == AnimalState::Dead) {
            break;
        }
        let Some(tester) = pack
            .iter()
            .copied()
            .find(|&w| mind(app, w).role == PackRole::Tester)
        else {
            continue;
        };
        let m = mind(app, tester);
        let (true, Some(q)) = (m.state == AnimalState::Chase, m.prey) else {
            continue;
        };
        if !rushed
            && let Some(&a) = pack
                .iter()
                .find(|&&w| mind(app, w).role == PackRole::Ambusher)
        {
            let line = flat(pos(app, q) - pos(app, tester)).normalize();
            let beyond = flat(pos(app, a) - pos(app, q)).dot(line);
            f.ambusher = Some((beyond, state(app, a) == AnimalState::Stalk));
        }
        rushed = true;
        let v = app.world.get::<LocomotionState>(q).unwrap().velocity;
        let v = Vec3::new(v.x, 0.0, v.z);
        if v.length() < 3.0 {
            continue;
        }
        f.chase += 1;
        if open {
            let sprint = app.world.get::<NpcMotor>(q).unwrap().sprint_speed;
            fast |= v.length() > 0.95 * sprint;
            let close = pack
                .iter()
                .any(|&w| flat(pos(app, w) - pos(app, q)).length() < 3.0);
            open = !(close || fast && v.length() < 0.75 * sprint);
        }
        if !open {
            continue;
        }
        let ahead = v.normalize();
        let left = Vec3::new(ahead.z, 0.0, -ahead.x);
        f.phase += 1;
        let (mut abeam, mut sides) = (0, 0);
        for &w in pack {
            let r = mind(app, w).role;
            let off = flat(pos(app, w) - pos(app, q));
            let (d, across, along) = (off.length(), off.dot(left) * r.side(), off.dot(ahead));
            if r == PackRole::Tester && along < 0.0 {
                f.tester_behind += 1;
            }
            if r.side() == 0.0 {
                continue;
            }
            if d < 12.0 && across > 1.5 && along.abs() < across + 1.0 {
                abeam += 1;
            }
            if d < 20.0 && (across > 1.5 || d < 1.6) {
                sides += 1;
            }
            if d < 12.0 && d > 4.0 && across < -1.5 && along > -6.0 {
                f.wrong_level += 1;
            }
        }
        f.sides += (sides == 2) as usize;
        run = if abeam == 2 { run + 1 } else { 0 };
        f.abreast = f.abreast.max(run);
    }
    f
}

/// Over twenty-four draws of each pack size: every hunt is a real chase, with the ambusher
/// (in a pack of four or more) lying in wait well beyond the quarry as the tester rushes, the
/// tester driving it from behind, and never a flanker level with it on its far side. Each
/// pack size (four as well as three and five) keeps both flankers out on their own sides for
/// 85 % of its flanking, and in at least five of every eight hunts (15 of the 24) both run
/// abreast of the quarry for a second (unless the flanking was over within two) and hold
/// their sides for three quarters of it. (A deer at full sprint outruns a wolf, so on a long
/// straight bolt the flankers fall back: hence not every hunt.)
/// (Each pack size is its own test, so they run side by side.)
fn flankers_run_out_on_the_quarrys_sides(wolves: usize) {
    const DRAWS: usize = 24;
    let (mut phase, mut sides, mut good) = (0, 0, 0);
    for k in 0..DRAWS as u64 {
        let mut app = app();
        let (deer, pack) = seeded_hunt(&mut app, 4, wolves, k);
        let f = flanking(&mut app, &deer, &pack);
        eprintln!("{wolves} wolves, draw {k}: {f:?}");
        let at = format!("{wolves} wolves, draw {k}");
        assert!(f.chase >= 60, "{at}: a real chase ({} steps)", f.chase);
        assert_eq!(f.ambusher.is_some(), wolves >= 4, "{at}: an ambusher");
        if let Some((beyond, lying_in_wait)) = f.ambusher {
            assert!(beyond > 12.0, "{at}: the ambusher {beyond:.1} m beyond");
            assert!(lying_in_wait, "{at}: the ambusher lying in wait");
        }
        assert!(
            f.tester_behind * 10 >= f.phase * 9,
            "{at}: the tester drives from behind ({} of {})",
            f.tester_behind,
            f.phase
        );
        assert_eq!(
            f.wrong_level, 0,
            "{at}: a flanker level with the quarry on its far side"
        );
        phase += f.phase;
        sides += f.sides;
        let abreast = f.abreast >= 60 || f.phase < 120;
        good += (abreast && f.sides * 4 >= f.phase * 3) as usize;
    }
    assert!(
        sides * 20 >= phase * 17,
        "{wolves} wolves: both flankers on their sides for {sides} of {phase} steps"
    );
    assert!(
        good * 8 >= DRAWS * 5,
        "{wolves} wolves: both flankers abreast and on their sides in {good} of {DRAWS} hunts"
    );
}

#[test]
fn a_pack_of_three_runs_out_on_the_quarrys_sides() {
    flankers_run_out_on_the_quarrys_sides(3);
}

#[test]
fn a_pack_of_four_runs_out_on_the_quarrys_sides() {
    flankers_run_out_on_the_quarrys_sides(4);
}

#[test]
fn a_pack_of_five_runs_out_on_the_quarrys_sides() {
    flankers_run_out_on_the_quarrys_sides(5);
}

/// Bearing (rad, 0 = straight ahead) from `e`'s facing to `at`.
fn bearing(app: &App, e: Entity, at: DVec3) -> f32 {
    let f = app.world.get::<Transform>(e).unwrap().forward();
    let f = Vec3::new(f.x, 0.0, f.z).normalize();
    f.angle_between(flat(at - pos(app, e)).normalize())
}

/// A deer that bolts from a wolf it sees stots its first bounds (a signal to the wolf); one
/// that bolts at a bang just runs, flat out from the start.
#[test]
fn a_deer_stots_at_a_wolf_it_sees_but_not_at_a_bang() {
    // How hard it stotted over its first second of flight, and how far it got in it.
    let flight = |app: &mut App, deer: Entity| {
        let (mut stot, mut from) = (0.0f32, None);
        for _ in 0..60 * 4 {
            app.update();
            if state(app, deer) != AnimalState::Flee {
                continue;
            }
            let now = app.world.get::<NpcClock>(deer).unwrap().now;
            let start = *from.get_or_insert((now, pos(app, deer)));
            if now - start.0 > 1.0 {
                return (stot, flat(pos(app, deer) - start.1).length());
            }
            stot = stot.max(mind(app, deer).stot(now));
        }
        panic!("it never bolted");
    };
    let mut app = app();
    let deer = animal(&mut app, Species::Deer, DVec3::ZERO, 1);
    app.update_n(30);
    animal(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -16.0), 2);
    let (stot, ran) = flight(&mut app, deer);
    eprintln!("at a wolf: stot {stot:.2}, {ran:.1} m in its first second");
    assert!(stot > 0.9, "it stots at the wolf ({stot:.2})");
    assert!(
        mind(&app, deer).alarm.is_some_and(|a| a.1),
        "it raised the alarm"
    );

    let mut app = self::app();
    let deer = animal(&mut app, Species::Deer, DVec3::ZERO, 1);
    app.update_n(30);
    app.world.write_message(engine_world::NoiseEvent {
        position: DVec3::new(0.0, 1.0, -12.0),
        radius: 60.0,
        source: None,
    });
    let (stot, ran) = flight(&mut app, deer);
    eprintln!("at a bang: stot {stot:.2}, {ran:.1} m in its first second");
    assert!(
        mind(&app, deer).alarm.is_some_and(|a| a.1),
        "it raised the alarm"
    );
    assert_eq!(stot, 0.0, "it doesn't stot at a bang");
    assert!(
        ran > 3.0,
        "it runs flat out from the start ({ran:.1} m in a second)"
    );
}

#[test]
fn the_herds_alarm_spreads_within_a_reaction_time() {
    let mut app = app();
    let deer = herd(&mut app, Species::Deer, DVec3::ZERO, 6, 3);
    app.update_n(60);
    // Shot from the side by someone the herd can't see: the one hit bolts, snorting; each
    // of the others turns to look where the alarm came from, then bolts with it.
    let at = DVec3::new(-40.0, 1.5, 0.0);
    let shooter = app.world.spawn(Transform::from_translation(at)).id();
    app.world.write_message(DamageEvent {
        target: deer[0],
        amount: 5.0,
        kind: DamageKind::Ballistic,
        point: pos(&app, deer[0]),
        impulse: Vec3::ZERO,
        source: Some(shooter),
    });
    let n = deer.len();
    let (mut alert, mut fled) = (vec![None; n], vec![None; n]);
    let mut facing = vec![(0.0f32, 0.0f32); n];
    let mut spread = 0.0f32;
    for f in 0..60 * 4 {
        app.update();
        for (k, &d) in deer.iter().enumerate() {
            let s = state(&app, d);
            if s == AnimalState::Alert && alert[k].is_none() && fled[k].is_none() {
                alert[k] = Some(f);
                facing[k].0 = bearing(&app, d, at);
            }
            if s == AnimalState::Flee && fled[k].is_none() {
                fled[k] = Some(f);
                facing[k].1 = bearing(&app, d, at);
            }
        }
        // From two and a half seconds into the flight: how far the herd strings out.
        if f >= 150 {
            let c = deer.iter().map(|&d| pos(&app, d)).sum::<DVec3>() / n as f64;
            let s = deer
                .iter()
                .map(|&d| flat(pos(&app, d) - c).length())
                .fold(0.0, f32::max);
            spread = spread.max(s);
        }
    }
    let t0 = fled[0].expect("the deer that was hit bolts");
    assert!(t0 <= 1, "at once: {t0}");
    assert_eq!(alert[0], None, "the one hit doesn't stop to look");
    assert!(
        mind(&app, deer[0]).alarm.is_some_and(|a| a.1),
        "it raised the alarm"
    );
    for k in 1..n {
        let (Some(a), Some(b)) = (alert[k], fled[k]) else {
            panic!("deer {k}: looked {:?}, bolted {:?}", alert[k], fled[k]);
        };
        let secs = |f: usize| (f as f32 - t0 as f32) / 60.0;
        eprintln!(
            "deer {k}: looks up {:.2} s, bolts {:.2} s after the alarm; facing the shot {:.2} \
             -> {:.2} rad",
            secs(a),
            secs(b),
            facing[k].0,
            facing[k].1
        );
        assert!(
            secs(a) <= 0.3,
            "deer {k} looked up {:.2} s after the alarm",
            secs(a)
        );
        assert!(
            secs(b) - secs(a) >= 0.29,
            "deer {k} turned to look before it bolted: {:.2} s",
            secs(b) - secs(a)
        );
        assert!(
            (0.3..=0.85).contains(&secs(b)),
            "deer {k} bolted {:.2} s after the alarm",
            secs(b)
        );
        assert!(
            facing[k].1 < facing[k].0 - 0.25 || facing[k].1 < 0.5,
            "deer {k} turned toward the shot: {:.2} -> {:.2} rad",
            facing[k].0,
            facing[k].1
        );
        assert!(
            mind(&app, deer[k]).alarm.is_some_and(|a| !a.1),
            "deer {k} caught the alarm"
        );
    }
    eprintln!("strung out over {spread:.1} m");
    assert!(spread < 4.0, "the herd bunches as it runs: {spread:.1} m");
}

#[test]
fn a_deer_zigzags_once_a_wolf_is_close() {
    let mut app = app();
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    // A wary deer that bolts while the wolf is still well back but can't outrun it (an old
    // one, say), and a hungry wolf.
    let deer = app
        .world
        .spawn((
            Transform::from_translation(DVec3::ZERO),
            Animal {
                skittishness: 6.0,
                range: 200.0,
                ..Animal::new(Species::Deer)
            },
            Herd(1),
            NpcMotor {
                run_speed: 6.0,
                sprint_speed: 7.0,
                ..Species::Deer.motor()
            },
        ))
        .id();
    let wolf = animal(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -30.0), 2);
    app.update();
    app.world.get_mut::<AnimalMind>(wolf).unwrap().hunger = 1.0;
    // How fast the deer swings its line of flight (rad/s, left positive) while the wolf is
    // well back, and once it is close.
    let (mut far, mut close) = (Vec::new(), Vec::new());
    let mut last: Option<f32> = None;
    for _ in 0..60 * 40 {
        app.update();
        if state(&app, deer) == AnimalState::Dead {
            break;
        }
        let v = app.world.get::<LocomotionState>(deer).unwrap().velocity;
        let running = state(&app, deer) == AnimalState::Flee
            && state(&app, wolf) == AnimalState::Chase
            && Vec3::new(v.x, 0.0, v.z).length() > 2.0;
        if !running {
            last = None;
            continue;
        }
        let yaw = v.x.atan2(v.z);
        let Some(was) = last.replace(yaw) else {
            continue;
        };
        let swing = (yaw - was + PI).rem_euclid(TAU) - PI;
        let rate = swing * 60.0;
        let d = flat(pos(&app, deer) - pos(&app, wolf)).length();
        let pursuer = mind(&app, deer).pursuer.map(|p| p.1);
        assert!(
            pursuer.is_some_and(|p| (p - d).abs() < 3.0),
            "the deer knows the wolf is after it: {pursuer:?} vs {d:.1} m"
        );
        if d > ZIGZAG_RANGE + 3.0 {
            far.push(rate);
        } else if d < ZIGZAG_RANGE {
            close.push(rate);
        }
    }
    let hard = |r: &&f32| r.abs() > 1.5;
    assert!(far.len() > 60, "the wolf runs it from well back");
    let swerves = far.iter().filter(hard).count();
    assert!(
        swerves * 20 < far.len(),
        "it runs straight away while the wolf is well back: {swerves} of {} steps swerving",
        far.len()
    );
    // Within a second and a half of the wolf closing in, it has cut hard one way, the other
    // way and back again.
    let mut cuts: Vec<f32> = close
        .iter()
        .take(90)
        .filter(hard)
        .map(|r| r.signum())
        .collect();
    cuts.dedup();
    assert!(
        cuts.len() >= 3,
        "it zigzags once the wolf is close: cut {cuts:?} in {} steps",
        close.len().min(90)
    );
}

/// A pack that scents a herd from further off than it would run one down stalks it in and
/// rushes it, rather than giving it up as lost the moment it picks it.
#[test]
fn a_pack_stalks_a_herd_it_scents_from_afar() {
    let mut app = app();
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let deer = herd(&mut app, Species::Deer, DVec3::ZERO, 4, 1);
    let pack = herd(&mut app, Species::Wolf, DVec3::new(0.0, 0.0, -90.0), 3, 2);
    app.update();
    let far = flat(pos(&app, pack[0]) - pos(&app, deer[0])).length();
    assert!(
        far > 75.0 && (far as f64) < SCENT_REACH,
        "the pack starts {far:.0} m off"
    );
    for &w in &pack {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    let (mut stalked, mut rushed, mut rested) = (false, None, false);
    for f in 0..60 * 60 {
        app.update();
        for &w in &pack {
            match state(&app, w) {
                AnimalState::Stalk => stalked = true,
                AnimalState::Chase => {
                    rushed.get_or_insert(f);
                }
                AnimalState::Rest => rested = true,
                _ => {}
            }
        }
        if rushed.is_some() {
            break;
        }
    }
    let nearest = pack
        .iter()
        .flat_map(|&w| deer.iter().map(move |&d| (w, d)))
        .map(|(w, d)| flat(pos(&app, w) - pos(&app, d)).length())
        .fold(f32::MAX, f32::min);
    eprintln!(
        "stalked {stalked}, rushed at {:?} s, rested {rested}, nearest {nearest:.1} m",
        rushed.map(|f| f as f32 / 60.0)
    );
    assert!(stalked, "the pack stalks the herd it scents");
    assert!(!rested, "the pack never gives the herd up as lost");
    assert!(rushed.is_some(), "the pack rushes the herd within a minute");
    assert!(
        nearest < 40.0,
        "the pack rushes from close ({nearest:.1} m)"
    );
}

/// A wary deer in its prime that bolts while a pack of three is still well off, running
/// `speed` m/s however long it runs (and with room to run), and the pack.
fn long_chase(app: &mut App, speed: f32) -> (Entity, Vec<Entity>) {
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let deer = app
        .world
        .spawn((
            Transform::from_translation(DVec3::ZERO),
            Animal {
                skittishness: 6.0,
                range: 200.0,
                ..Animal::new(Species::Deer)
            },
            Herd(1),
            NpcMotor {
                run_speed: speed,
                sprint_speed: speed,
                ..Species::Deer.motor()
            },
        ))
        .id();
    let pack = herd(app, Species::Wolf, DVec3::new(0.0, 0.0, -40.0), 3, 2);
    app.update();
    (deer, pack)
}

/// A deer just faster than the wolves (10.8 m/s against their 10.5) keeps its lead without
/// getting away: the pack runs it to its time, gives up all together, rests and hunts again.
#[test]
fn the_pack_gives_up_a_long_chase_and_rests() {
    let mut app = app();
    let (deer, pack) = long_chase(&mut app, 10.8);
    let starve = |app: &mut App| {
        for &w in &pack {
            app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
        }
    };
    starve(&mut app);
    let step = |app: &mut App| {
        app.update();
        assert_ne!(state(app, deer), AnimalState::Dead, "it can't be caught");
    };
    let hunting =
        |app: &App, w: Entity| matches!(state(app, w), AnimalState::Stalk | AnimalState::Chase);
    // The pack sets off ...
    let mut f = 0;
    while !pack.iter().any(|&w| state(&app, w) == AnimalState::Chase) {
        step(&mut app);
        f += 1;
        assert!(f < 60 * 60, "the pack never ran");
    }
    let start = f;
    // ... keeps on at it until its time is up, then all of it gives up together ...
    let limit = (PACK_CHASE_SECS * 60.0) as usize;
    loop {
        step(&mut app);
        f += 1;
        let resting = pack
            .iter()
            .filter(|&&w| state(&app, w) == AnimalState::Rest)
            .count();
        if f - start < limit - 20 {
            assert_eq!(
                resting,
                0,
                "gave up early, {:.1} s in",
                (f - start) as f32 / 60.0
            );
            for &w in &pack {
                assert!(
                    hunting(&app, w),
                    "{w:?} stopped hunting {:.1} s in",
                    (f - start) as f32 / 60.0
                );
            }
        }
        if resting == pack.len() {
            break;
        }
        assert!(f - start < limit + 30, "the pack kept on past its time");
    }
    // ... and rests its time out, hungry as it is ...
    let rest = f;
    while f - rest < ((PACK_REST_SECS - 1.0) * 60.0) as usize {
        starve(&mut app);
        step(&mut app);
        f += 1;
        for &w in &pack {
            assert_eq!(
                state(&app, w),
                AnimalState::Rest,
                "{w:?} back up {:.1} s into its rest",
                (f - rest) as f32 / 60.0
            );
        }
    }
    // ... before it hunts again (the deer back in reach).
    app.world.get_mut::<Transform>(deer).unwrap().translation =
        pos(&app, pack[0]) + DVec3::new(0.0, 0.0, 40.0);
    for _ in 0..60 * 10 {
        starve(&mut app);
        step(&mut app);
    }
    assert!(
        pack.iter().all(|&w| hunting(&app, w)),
        "the pack hunts again after its rest"
    );
}

/// A fit deer that outruns the pack (11.5 m/s) and keeps pulling away is let go once the
/// pack has tested it, long before its time is up: all of it together, and it rests.
#[test]
fn the_pack_breaks_off_from_a_fit_deer_that_outruns_it() {
    let mut app = app();
    let (deer, pack) = long_chase(&mut app, 11.5);
    for &w in &pack {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    let mut f = 0;
    while !pack.iter().any(|&w| state(&app, w) == AnimalState::Chase) {
        app.update();
        f += 1;
        assert!(f < 60 * 60, "the pack never ran");
    }
    let start = f;
    let gap = |app: &App| {
        pack.iter()
            .map(|&w| flat(pos(app, w) - pos(app, deer)).length())
            .fold(f32::MAX, f32::min)
    };
    let mut first = None;
    loop {
        app.update();
        f += 1;
        assert_ne!(state(&app, deer), AnimalState::Dead, "it can't be caught");
        let secs = (f - start) as f64 / 60.0;
        let resting = pack
            .iter()
            .filter(|&&w| state(&app, w) == AnimalState::Rest)
            .count();
        if resting > 0 {
            first.get_or_insert((secs, gap(&app)));
        }
        if resting == pack.len() {
            break;
        }
        assert!(secs < PACK_CHASE_SECS, "the pack ran on to its time");
    }
    let (at, lead) = first.unwrap();
    let all = (f - start) as f64 / 60.0;
    eprintln!("broke off {at:.1} s in, {lead:.1} m behind, all resting {all:.1} s in");
    assert!(at > TEST_SECS, "it tested the deer first ({at:.1} s)");
    assert!(
        at < PACK_CHASE_SECS * 0.6,
        "it let the deer go well before its time ({at:.1} s)"
    );
    assert!(lead > ESCAPE_GAP, "the deer had got away ({lead:.1} m)");
    assert!(all - at < 0.5, "all together ({:.1} s apart)", all - at);
    for _ in 0..60 * 10 {
        app.update();
        for &w in &pack {
            assert_eq!(state(&app, w), AnimalState::Rest, "{w:?} rests");
        }
    }
}
