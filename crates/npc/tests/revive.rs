//! Squad mates revive the downed: whole NPCs in a headless app (no GPU). A squad mate comes,
//! kneels beside a downed ally and gets it up where it lies; skilled fighters hold off while
//! the spot is in the enemy's view or they are under fire (a rookie runs straight in); a
//! reviver that is hit lets go; nobody revives when it is turned off; and the showcase timer
//! waits for a revive under way, gets a fighter up where it lies when a mate is near, and sends
//! it home only when it is alone.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::agent::CombatStats;
use engine_npc::*;
use engine_world::locomotion::{LocomotionState, MoveIntent};
use engine_world::{DamageEvent, DamageKind, ProjectileTrace, WorldPlugin};

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

fn run(app: &mut App, secs: f32) {
    let frames = (secs * 60.0).ceil() as usize;
    app.update_n(frames);
}

/// A fighter of squad 4 at `at`, facing −Z.
fn mate(app: &mut App, at: DVec3, skills: NpcSkills) -> Entity {
    let mut t = Transform::from_translation(at);
    t.look_to(Vec3::NEG_Z, Vec3::Y);
    app.world
        .spawn((
            t,
            Npc::default(),
            Faction::ENEMY,
            Squad(4),
            skills,
            NpcWeapon::rifle(),
        ))
        .id()
}

/// A stand-in player at `at` (NPCs see it), facing +Z.
fn player(app: &mut App, at: DVec3) -> Entity {
    let mut t = Transform::from_translation(at);
    t.look_to(Vec3::Z, Vec3::Y);
    app.world
        .spawn((t, Faction::PLAYER, LocomotionState::default()))
        .id()
}

fn hit(app: &mut App, e: Entity, amount: f32) {
    let at = app.world.get::<Transform>(e).unwrap().translation;
    app.world.write_message(DamageEvent {
        target: e,
        amount,
        kind: DamageKind::Ballistic,
        point: at + DVec3::Y,
        impulse: Vec3::Z * 10.0,
        source: None,
    });
}

/// Cut `e` down (and let it land).
fn down(app: &mut App, e: Entity) {
    hit(app, e, 1000.0);
    run(app, 0.1);
    assert!(app.world.get::<NpcVitals>(e).unwrap().downed);
}

fn brain(app: &App, e: Entity) -> &NpcBrain {
    app.world.get::<NpcBrain>(e).unwrap()
}

fn downed(app: &App, e: Entity) -> bool {
    app.world.get::<NpcVitals>(e).unwrap().downed
}

fn at(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn stats(app: &App, e: Entity) -> CombatStats {
    app.world.get::<NpcCombat>(e).unwrap().stats
}

/// Where the downed `e`'s chest lies: about 0.7 m behind its feet (it falls on its back).
fn chest(app: &App, e: Entity) -> DVec3 {
    let t = app.world.get::<Transform>(e).unwrap();
    let f = t.forward();
    t.translation - (Vec3::new(f.x, 0.0, f.z).normalize() * 0.7).as_dvec3()
}

#[test]
fn a_squad_mate_revives_a_downed_ally_where_it_lies() {
    let mut app = app();
    let settings = *app.world.resource::<NpcSettings>();
    assert!(settings.ally_revive_secs > 0.0 && settings.revive_secs == 0.0);
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::veteran());
    let reviver = mate(&mut app, DVec3::new(9.0, 0.0, 4.0), NpcSkills::veteran());
    run(&mut app, 0.5);
    down(&mut app, ally);
    let lies = at(&app, ally);
    let (mut kneeled, mut up_after) = (0.0f32, None);
    for frame in 0..60 * 12 {
        app.update();
        let b = brain(&app, reviver);
        if b.action == NpcAction::Revive && b.revive_progress > 0.0 {
            kneeled += 1.0 / 60.0;
            assert_eq!(b.revive, Some(ally));
            // Kneeling beside its chest, facing it.
            let p = at(&app, reviver);
            let gap = DVec2::new(p.x - chest(&app, ally).x, p.z - chest(&app, ally).z).length();
            assert!(gap < 1.3, "kneels {gap:.2} m from the chest");
            assert!(app.world.get::<MoveIntent>(reviver).unwrap().crouch);
        }
        if !downed(&app, ally) {
            up_after = Some(frame as f32 / 60.0);
            break;
        }
    }
    let up_after = up_after.expect("the ally was never revived");
    // It took the walk over and the revive itself (not the instant it fell).
    assert!(
        kneeled >= settings.ally_revive_secs * 0.8 - 0.05,
        "knelt only {kneeled:.2} s"
    );
    assert!(up_after > 2.5, "up after {up_after:.2} s");
    // Up where it lay, hurt, and nobody left kneeling.
    let moved = at(&app, ally).distance(lies);
    assert!(moved < 0.05, "it moved {moved:.2} m getting up");
    let v = app.world.get::<NpcVitals>(ally).unwrap();
    assert!(
        (v.fraction() - settings.ally_revive_health).abs() < 0.02,
        "{v:?}"
    );
    assert!(app.world.get::<Downed>(ally).is_none());
    assert_eq!(stats(&app, reviver).revives, 1);
    run(&mut app, 0.5);
    assert_ne!(brain(&app, reviver).action, NpcAction::Revive);
    assert!(brain(&app, reviver).revive.is_none());
}

/// A downed ally out in the open, the enemy 18 m off watching it, every body's randomness from
/// `seed`. With `fire`, the enemy also keeps shots whizzing past the squad mate. Returns whether
/// the squad mate went to revive within `secs`, and the app.
fn exposed(
    skills: NpcSkills,
    fire: bool,
    secs: f32,
    seed: u64,
) -> (bool, App, Entity, Entity, Entity) {
    let mut app = app();
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::soldier());
    let reviver = mate(&mut app, DVec3::new(4.0, 0.0, 2.0), skills);
    app.world
        .entity_mut(ally)
        .insert(Seed(hash_u64(2 * seed + 1)));
    app.world
        .entity_mut(reviver)
        .insert(Seed(hash_u64(2 * seed + 2)));
    let enemy = player(&mut app, DVec3::new(0.0, 0.0, -18.0));
    // Both see the enemy first.
    run(&mut app, 1.5);
    down(&mut app, ally);
    let mut went = false;
    let steps = (secs / 0.2).ceil() as usize;
    for _ in 0..steps {
        if fire {
            let p = at(&app, reviver);
            app.world.write_message(ProjectileTrace {
                from: DVec3::new(0.0, 1.5, -18.0),
                to: p + DVec3::new(0.5, 1.2, 1.5),
                radius: 0.05,
                speed: 600.0,
                shooter: Some(enemy),
            });
        }
        for _ in 0..12 {
            app.update();
            went |= brain(&app, reviver).action == NpcAction::Revive;
        }
    }
    (went, app, ally, reviver, enemy)
}

/// The runs of each case: every one with its own seeds, the same ones on every test run.
const SEEDS: u64 = 16;

/// `run(seed)` for each of the [`SEEDS`], side by side.
fn each_seed<T: Send>(run: impl Fn(u64) -> T + Sync) -> Vec<T> {
    let run = &run;
    std::thread::scope(|s| {
        let runs: Vec<_> = (0..SEEDS).map(|seed| s.spawn(move || run(seed))).collect();
        runs.into_iter()
            .map(|r| r.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

/// In how many of the [`SEEDS`] runs a squad mate with `skills` went in, printed run by run.
fn went_in(name: &str, skills: NpcSkills, fire: bool, secs: f32) -> usize {
    let runs = each_seed(|seed| exposed(skills, fire, secs, seed).0);
    let n = runs.iter().filter(|&&w| w).count();
    let marks: String = runs.iter().map(|&w| if w { 'x' } else { '.' }).collect();
    eprintln!("{name}: went in {n}/{SEEDS} [{marks}]");
    n
}

/// Once the downed ally's spot is safe, a squad mate gets it up within 14 s, in every run.
fn revived_once_safe(name: &str, make_safe: impl Fn(&mut App, Entity) + Sync) {
    let runs = each_seed(|seed| {
        let (_, mut app, ally, reviver, enemy) = exposed(NpcSkills::elite(), false, 6.0, seed);
        if !downed(&app, ally) {
            return None; // went in under the enemy's eyes (counted above)
        }
        make_safe(&mut app, enemy);
        for frame in 0..60 * 14 {
            app.update();
            if !downed(&app, ally) {
                assert_eq!(stats(&app, reviver).revives, 1, "seed {seed}");
                return Some(frame as f32 / 60.0);
            }
        }
        panic!("seed {seed}: the elite never revived once {name}");
    });
    let up: Vec<String> = runs.iter().flatten().map(|s| format!("{s:.1}")).collect();
    eprintln!("elite, {name}: up after {} s", up.join(" "));
}

#[test]
fn skilled_fighters_do_not_revive_in_the_enemys_view_or_under_fire_but_rookies_run_in() {
    // In the enemy's view, an elite holds off and a veteran mostly does; a rookie runs
    // straight in.
    let elite = went_in("elite, watched", NpcSkills::elite(), false, 6.0);
    let veteran = went_in("veteran, watched", NpcSkills::veteran(), false, 6.0);
    let rookie = went_in("rookie, watched", NpcSkills::rookie(), false, 6.0);
    let n = SEEDS as usize;
    assert!(
        elite * 10 <= n,
        "elites went in under the enemy's eyes {elite}/{n}"
    );
    assert!(
        rookie * 10 >= n * 9,
        "rookies ran straight in only {rookie}/{n}"
    );
    assert!(
        veteran * 2 <= rookie,
        "veterans went in {veteran}/{n}, not clearly less than rookies ({rookie}/{n})"
    );

    // Once the spot is safe, it goes: the watcher looks away, or is gone.
    revived_once_safe("the enemy looked away", |app, enemy| {
        app.world
            .get_mut::<Transform>(enemy)
            .unwrap()
            .look_to(Vec3::NEG_Z, Vec3::Y);
    });
    revived_once_safe("the enemy was gone", |app, enemy| {
        app.world.despawn(enemy);
    });

    // Shots whizzing past: no skilled fighter goes.
    for (name, skills) in [
        ("veteran, under fire", NpcSkills::veteran()),
        ("elite, under fire", NpcSkills::elite()),
        ("master, under fire", NpcSkills::master()),
    ] {
        let went = went_in(name, skills, true, 8.0);
        assert_eq!(went, 0, "{name}: went in {went}/{n}");
    }
}

#[test]
fn a_reviver_that_is_hit_lets_go_and_tries_again_later() {
    let mut app = app();
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::veteran());
    let reviver = mate(&mut app, DVec3::new(6.0, 0.0, 3.0), NpcSkills::veteran());
    run(&mut app, 0.5);
    down(&mut app, ally);
    // Wait until it is kneeling over the ally and a second into it.
    let mut kneeling = false;
    for _ in 0..60 * 8 {
        app.update();
        if brain(&app, reviver).revive_progress > 1.0 {
            kneeling = true;
            break;
        }
    }
    assert!(kneeling, "it never knelt");
    hit(&mut app, reviver, 10.0);
    run(&mut app, 0.05);
    let b = brain(&app, reviver);
    assert_ne!(b.action, NpcAction::Revive, "it kept going after the hit");
    assert_eq!(b.revive_progress, 0.0);
    assert!(b.revive.is_none());
    assert_eq!(stats(&app, reviver).revives_aborted, 1);
    // The ally stays down for a while (the progress was lost, and the reviver waits)...
    run(&mut app, 1.5);
    assert!(downed(&app, ally));
    // ...then it comes back and finishes.
    let mut revived = false;
    for _ in 0..60 * 12 {
        app.update();
        if !downed(&app, ally) {
            revived = true;
            break;
        }
    }
    assert!(revived, "it never came back to finish");
    assert_eq!(stats(&app, reviver).revives, 1);
}

#[test]
fn squad_mates_do_not_revive_when_it_is_off() {
    let mut app = app();
    app.world.resource_mut::<NpcSettings>().ally_revive_secs = 0.0;
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::veteran());
    let reviver = mate(&mut app, DVec3::new(5.0, 0.0, 2.0), NpcSkills::master());
    run(&mut app, 0.5);
    down(&mut app, ally);
    for _ in 0..60 * 10 {
        app.update();
        assert_ne!(brain(&app, reviver).action, NpcAction::Revive);
    }
    assert!(downed(&app, ally));
    assert_eq!(stats(&app, reviver).revives, 0);
}

#[test]
fn the_showcase_timer_waits_for_a_mate_and_sends_home_only_the_alone() {
    let home = DVec3::new(20.0, 0.0, 20.0);
    // A mate on its way: the timer waits for it.
    let mut w = app();
    w.world.resource_mut::<NpcSettings>().revive_secs = 1.5;
    let ally = mate(&mut w, DVec3::ZERO, NpcSkills::master());
    let reviver = mate(&mut w, DVec3::new(10.0, 0.0, 6.0), NpcSkills::master());
    run(&mut w, 0.5);
    down(&mut w, ally);
    run(&mut w, 10.0);
    assert!(!downed(&w, ally));
    assert_eq!(
        stats(&w, reviver).revives,
        1,
        "the timer cut the revive short"
    );

    // A mate near that doesn't come (here: told to hold still): up where it lies, hurt.
    let mut w = app();
    w.world.resource_mut::<NpcSettings>().revive_secs = 1.5;
    let ally = mate(&mut w, DVec3::ZERO, NpcSkills::soldier());
    w.world.get_mut::<Npc>(ally).unwrap().home = Some(home);
    let idle = mate(&mut w, DVec3::new(6.0, 0.0, 0.0), NpcSkills::soldier());
    w.world.get_mut::<Npc>(idle).unwrap().active = false;
    run(&mut w, 0.5);
    down(&mut w, ally);
    let lies = at(&w, ally);
    run(&mut w, 2.0);
    assert!(!downed(&w, ally));
    assert!(at(&w, ally).distance(lies) < 0.05, "it was moved");
    let v = w.world.get::<NpcVitals>(ally).unwrap();
    assert!(v.fraction() < 0.5, "{v:?}");

    // Nobody near: sent home, whole.
    let mut w = app();
    w.world.resource_mut::<NpcSettings>().revive_secs = 1.5;
    let ally = mate(&mut w, DVec3::ZERO, NpcSkills::soldier());
    w.world.get_mut::<Npc>(ally).unwrap().home = Some(home);
    run(&mut w, 0.5);
    down(&mut w, ally);
    run(&mut w, 2.0);
    assert!(!downed(&w, ally));
    assert!(at(&w, ally).distance(home) < 0.5, "not sent home");
    assert_eq!(w.world.get::<NpcVitals>(ally).unwrap().fraction(), 1.0);
}
