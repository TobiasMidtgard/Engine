//! Wildlife behavior tests: herds bolt from a runner but let a sneaker come closer, panic
//! spreads through a herd, wolf packs run prey down and feed, birds take off together and
//! land again, boars root about and run from a runner but turn and charge when cornered or
//! wounded, a sow charges to defend her piglets, and wolves pick a piglet over its elders.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
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
    let mut app = app();
    let tusker = boar(&mut app, DVec3::ZERO, 40, true, false);
    app.update_n(60);
    // Shot from one side, it charges the hunter; a wolf turns up 5 m behind it as it sets
    // off, and stays put: the boar is not charging at it.
    let hunter = player(&mut app, DVec3::new(14.0, 0.0, 0.0), LocomotionMode::Crouch);
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
                "the wolf ran from a boar charging away from it"
            );
        }
    }
    wolf.expect("the boar charges the hunter");

    // Bitten by a wolf 15 m off, it charges the wolf, which gives it room.
    let mut app = self::app();
    let tusker = boar(&mut app, DVec3::ZERO, 40, true, false);
    let wolf = animal(&mut app, Species::Wolf, DVec3::new(-15.0, 0.0, 0.0), 2);
    app.update_n(60);
    shoot(&mut app, wolf, tusker);
    let (mut charged, mut gave_way) = (false, false);
    for _ in 0..60 * 6 {
        app.update();
        charged |= state(&app, tusker) == AnimalState::Charge;
        gave_way |= charged && state(&app, wolf) == AnimalState::Flee;
    }
    assert!(charged, "the boar charges the wolf");
    assert!(gave_way, "and the wolf gives it room");
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
