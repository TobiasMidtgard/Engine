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
use engine_world::physics::Collider;
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

fn clock(app: &App, e: Entity) -> f64 {
    app.world.get::<NpcClock>(e).unwrap().now
}

fn moving(app: &App, e: Entity) -> Vec3 {
    app.world.get::<MoveIntent>(e).unwrap().direction
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
    let (mut kneeled, mut up_after, mut got_down) = (0.0f32, None, 0.0f32);
    for frame in 0..60 * 12 {
        app.update();
        let b = brain(&app, reviver);
        // Down on its knees first, the weapon set down and the hands on: the revive counts
        // only from then.
        if b.action == NpcAction::Revive && b.revive_progress <= 0.0 && b.revive_kneel > 0.0 {
            got_down = got_down.max(b.revive_kneel);
        }
        if b.revive_kneel < NpcBrain::REVIVE_KNEEL_SECS - 1e-3 {
            assert_eq!(b.revive_progress, 0.0, "counts {:.2} s in", b.revive_kneel);
        }
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
    // It took the walk over, getting down and the revive itself (not the instant it fell):
    // its hands on the chest for as long as its revive takes.
    assert!(
        kneeled >= settings.ally_revive_secs * 0.8 - 0.05,
        "knelt only {kneeled:.2} s"
    );
    let skills = NpcSkills::veteran();
    let takes = settings.ally_revive_secs * (1.2 - 0.4 * skills.teamwork);
    assert!(
        (kneeled - takes).abs() < 0.05,
        "the hands on {kneeled:.2} s, the revive takes {takes:.2} s"
    );
    assert!(
        got_down >= NpcBrain::REVIVE_KNEEL_SECS - 0.02,
        "got down in {got_down:.2} s"
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

/// A downed ally out in the open, the enemy 18 m off with a clear view of it. With `fire`, the
/// enemy also keeps shots whizzing past the squad mate. Returns whether the squad mate went to
/// revive within `secs`, and the app.
fn exposed(skills: NpcSkills, fire: bool, secs: f32) -> (bool, App, Entity, Entity, Entity) {
    let mut app = app();
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::soldier());
    let reviver = mate(&mut app, DVec3::new(4.0, 0.0, 2.0), skills);
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

#[test]
fn skilled_fighters_do_not_revive_in_the_enemys_view_or_under_fire_but_rookies_run_in() {
    // In the enemy's view, an elite holds off; a rookie runs straight in.
    let (went, mut app, ally, reviver, enemy) = exposed(NpcSkills::elite(), false, 6.0);
    assert!(!went, "the elite went in under the enemy's eyes");
    assert!(downed(&app, ally));
    // Once the enemy is gone, it goes.
    app.world.despawn(enemy);
    let mut revived = false;
    for _ in 0..60 * 14 {
        app.update();
        if !downed(&app, ally) {
            revived = true;
            break;
        }
    }
    assert!(revived, "the elite never revived once it was safe");
    assert_eq!(stats(&app, reviver).revives, 1);

    let (went, ..) = exposed(NpcSkills::rookie(), false, 6.0);
    assert!(went, "the rookie should run straight in");

    // Shots whizzing past: no skilled fighter goes.
    for skills in [
        NpcSkills::veteran(),
        NpcSkills::elite(),
        NpcSkills::master(),
    ] {
        let (went, app, ally, reviver, _) = exposed(skills, true, 8.0);
        assert!(!went, "{skills:?} went in under fire");
        assert!(downed(&app, ally));
        assert!(brain(&app, reviver).revive.is_none());
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
    let hit_at = clock(&app, reviver);
    let b = brain(&app, reviver);
    assert_ne!(b.action, NpcAction::Revive, "it kept going after the hit");
    assert_eq!(b.revive_progress, 0.0);
    assert!(b.revive.is_none());
    assert_eq!(stats(&app, reviver).revives_aborted, 1);
    // It waits a while before it tries again (a veteran lets things calm down first)...
    assert!(
        b.revive_retry > hit_at + 1.5,
        "it tries again {:.2} s after",
        b.revive_retry - hit_at
    );
    for _ in 0..90 {
        app.update();
        let now = clock(&app, reviver);
        assert_ne!(
            brain(&app, reviver).action,
            NpcAction::Revive,
            "back at it {:.2} s after the hit",
            now - hit_at
        );
        // ...first getting up off its knees where it is.
        if now < hit_at + NpcBrain::REVIVE_STAND_SECS - 0.1 {
            let m = moving(&app, reviver);
            assert!(
                m.length() < 1e-4,
                "off at {m:?} {:.2} s after",
                now - hit_at
            );
        }
    }
    // The ally stays down meanwhile (the progress was lost).
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
fn turning_revives_off_midway_gets_nobody_up() {
    let mut app = app();
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::veteran());
    let reviver = mate(&mut app, DVec3::new(14.0, 0.0, 6.0), NpcSkills::master());
    run(&mut app, 0.5);
    down(&mut app, ally);
    let mut going = false;
    for _ in 0..60 * 4 {
        app.update();
        if brain(&app, reviver).action == NpcAction::Revive {
            going = true;
            break;
        }
    }
    assert!(going, "it never went");
    app.world.resource_mut::<NpcSettings>().ally_revive_secs = 0.0;
    for _ in 0..60 * 3 {
        app.update();
        assert!(downed(&app, ally), "got up with revives off");
    }
    assert_eq!(stats(&app, reviver).revives, 0);
    assert_ne!(brain(&app, reviver).action, NpcAction::Revive);
}

#[test]
fn a_mate_handed_to_game_code_on_its_way_holds_up_neither_the_revive_nor_the_timer() {
    let mut app = app();
    app.world.resource_mut::<NpcSettings>().revive_secs = 1.5;
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::veteran());
    let reviver = mate(&mut app, DVec3::new(14.0, 0.0, 6.0), NpcSkills::master());
    run(&mut app, 0.5);
    down(&mut app, ally);
    let mut going = false;
    for _ in 0..60 * 3 {
        app.update();
        if brain(&app, reviver).action == NpcAction::Revive {
            going = true;
            break;
        }
    }
    assert!(going, "it never went");
    app.world.get_mut::<Npc>(reviver).unwrap().active = false;
    run(&mut app, 0.1);
    assert!(brain(&app, reviver).revive.is_none(), "it kept its revive");
    // The timer runs out as it would with nobody coming (up where it lies, a mate near).
    run(&mut app, 1.6);
    assert!(!downed(&app, ally), "the timer was held up");
    assert_eq!(stats(&app, reviver).revives, 0);
}

#[test]
fn a_mate_revives_the_ally_it_safely_can_rather_than_the_nearest_in_the_open() {
    let mut app = app();
    // The enemy far up the field; one ally down in the open in its view, another behind a
    // wall, a little farther from the elite that would revive them.
    let near = mate(&mut app, DVec3::ZERO, NpcSkills::soldier());
    let far = mate(&mut app, DVec3::new(8.0, 0.0, 6.0), NpcSkills::soldier());
    let reviver = mate(&mut app, DVec3::new(4.0, 0.0, 2.0), NpcSkills::elite());
    app.world.spawn((
        Transform::from_translation(DVec3::new(8.0, 1.5, 3.0)),
        Collider::cuboid(Vec3::new(3.0, 1.5, 0.3)),
    ));
    let enemy = player(&mut app, DVec3::new(0.0, 0.0, -18.0));
    run(&mut app, 1.5);
    hit(&mut app, near, 1000.0);
    hit(&mut app, far, 1000.0);
    run(&mut app, 0.1);
    assert!(downed(&app, near) && downed(&app, far));
    let mut up = false;
    for _ in 0..60 * 14 {
        app.update();
        if brain(&app, reviver).action == NpcAction::Revive {
            assert_eq!(brain(&app, reviver).revive, Some(far), "went into the open");
        }
        if !downed(&app, far) {
            up = true;
            break;
        }
    }
    assert!(up, "nobody revived the ally behind the wall");
    assert!(downed(&app, near));
    assert_eq!(stats(&app, reviver).revives, 1);
    let _ = enemy;
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

/// A fighter of squad 4 at `at` with `weapon`, facing −Z, as `role`.
fn member(app: &mut App, at: DVec3, skills: NpcSkills, weapon: NpcWeapon, role: NpcRole) -> Entity {
    let e = mate(app, at, skills);
    app.world.entity_mut(e).insert((
        weapon,
        Npc {
            role,
            ..Default::default()
        },
    ));
    e
}

/// The squad's medic: all teamwork, a carbine.
fn medic(app: &mut App, at: DVec3, skills: NpcSkills) -> Entity {
    let skills = NpcSkills {
        teamwork: 1.0,
        ..skills
    };
    member(app, at, skills, NpcWeapon::carbine(), NpcRole::Medic)
}

#[test]
fn the_squads_medic_is_the_one_that_usually_revives() {
    // Two mates nearer the downed ally than the medic is, in a few layouts.
    let layouts = [
        (
            DVec3::new(7.0, 0.0, 3.0),
            DVec3::new(-6.0, 0.0, 5.0),
            DVec3::new(11.0, 0.0, 8.0),
        ),
        (
            DVec3::new(-7.0, 0.0, 2.0),
            DVec3::new(5.0, 0.0, 6.0),
            DVec3::new(-10.0, 0.0, 9.0),
        ),
        (
            DVec3::new(4.0, 0.0, 7.0),
            DVec3::new(-8.0, 0.0, 1.0),
            DVec3::new(9.0, 0.0, -7.0),
        ),
        (
            DVec3::new(6.0, 0.0, -4.0),
            DVec3::new(-5.0, 0.0, -6.0),
            DVec3::new(-12.0, 0.0, 4.0),
        ),
    ];
    let mut by_medic = 0;
    for (n, (a, b, m)) in layouts.into_iter().enumerate() {
        let mut app = app();
        let ally = mate(&mut app, DVec3::ZERO, NpcSkills::soldier());
        let mates = [
            mate(&mut app, a, NpcSkills::veteran()),
            mate(&mut app, b, NpcSkills::soldier()),
        ];
        let medic = medic(&mut app, m, NpcSkills::soldier());
        run(&mut app, 0.5);
        down(&mut app, ally);
        for _ in 0..60 * 12 {
            app.update();
            if !downed(&app, ally) {
                break;
            }
        }
        assert!(!downed(&app, ally), "layout {n}: nobody revived the ally");
        let medic_revived = stats(&app, medic).revives == 1;
        let mates_revived: u32 = mates.iter().map(|&e| stats(&app, e).revives).sum();
        assert_eq!(
            u32::from(medic_revived) + mates_revived,
            1,
            "layout {n}: one revive"
        );
        by_medic += usize::from(medic_revived);
    }
    assert!(
        by_medic >= 3,
        "the medic revived in {by_medic} of 4 layouts"
    );
}

#[test]
fn melee_mates_screen_a_kneeling_medic_from_the_enemy() {
    let mut app = app();
    // The enemy up the field, looking at the spot; a medic that goes in all the same, and a
    // swordsman off to the side.
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::soldier());
    let medic = medic(&mut app, DVec3::new(7.0, 0.0, 5.0), NpcSkills::rookie());
    let sword = member(
        &mut app,
        DVec3::new(-8.0, 0.0, 7.0),
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        NpcRole::Fighter,
    );
    let enemy = player(&mut app, DVec3::new(2.0, 0.0, -16.0));
    run(&mut app, 1.5);
    down(&mut app, ally);
    let (mut knelt_at, mut closest, mut on_the_enemy_side) = (None, f64::MAX, false);
    for _ in 0..60 * 10 {
        app.update();
        let b = brain(&app, medic);
        if !b.kneeling() || !downed(&app, ally) {
            if knelt_at.is_some() {
                break;
            }
            continue;
        }
        let now = clock(&app, medic);
        let since = now - *knelt_at.get_or_insert(now);
        let s = brain(&app, sword);
        // Once the medic kneels, the swordsman screens it.
        if since > 1.0 {
            assert_eq!(s.role, "screen", "{since:.1} s into the kneel");
            assert!(s.guard.is_some());
        }
        let (m, me, foe) = (at(&app, medic), at(&app, sword), at(&app, enemy));
        let flat = |v: DVec3| DVec3::new(v.x, 0.0, v.z);
        let gap = flat(me - m).length();
        if gap < closest {
            closest = gap;
            on_the_enemy_side = flat(me - m).normalize().dot(flat(foe - m).normalize()) > 0.5;
        }
    }
    assert!(knelt_at.is_some(), "the medic never knelt");
    assert!(
        closest < 3.0,
        "the swordsman came no nearer than {closest:.1} m"
    );
    assert!(
        on_the_enemy_side,
        "the swordsman stood off the enemy's side"
    );
    // It holds there rather than running at the enemy 16 m off.
    assert!(at(&app, sword).distance(at(&app, enemy)) > 8.0);
}

#[test]
fn a_squad_without_a_medic_revives_as_before() {
    let mut app = app();
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::soldier());
    let reviver = mate(&mut app, DVec3::new(9.0, 0.0, 4.0), NpcSkills::rookie());
    let sword = member(
        &mut app,
        DVec3::new(-8.0, 0.0, 7.0),
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        NpcRole::Fighter,
    );
    let _enemy = player(&mut app, DVec3::new(2.0, 0.0, -16.0));
    run(&mut app, 1.5);
    down(&mut app, ally);
    for _ in 0..60 * 10 {
        app.update();
        // Nobody screens and nobody guards a spot: there is no medic to cover.
        let s = brain(&app, sword);
        assert_ne!(s.role, "screen");
        assert!(s.guard.is_none());
        if !downed(&app, ally) {
            break;
        }
    }
    assert!(!downed(&app, ally), "nobody revived the ally");
    // One of them got it up (as before: whoever is best placed).
    assert_eq!(stats(&app, reviver).revives + stats(&app, sword).revives, 1);
}

#[test]
fn a_mate_going_the_long_way_round_a_wall_is_not_taken_for_stuck() {
    let mut app = app();
    // The downed ally in a round pen open on the far side: the reviver's way there runs half
    // way round the pen's wall, all that while no nearer to it as the crow flies.
    let ally = mate(&mut app, DVec3::ZERO, NpcSkills::soldier());
    let reviver = mate(&mut app, DVec3::new(0.0, 0.0, 7.0), NpcSkills::veteran());
    for i in (0..20).filter(|i| !(9..=11).contains(i)) {
        let a = i as f32 * std::f32::consts::TAU / 20.0;
        app.world.spawn((
            Transform::from_translation(DVec3::new(
                6.0 * a.sin() as f64,
                1.5,
                6.0 * a.cos() as f64,
            ))
            .with_rotation(Quat::from_rotation_y(a)),
            Collider::cuboid(Vec3::new(1.0, 1.5, 0.25)),
        ));
    }
    run(&mut app, 0.5);
    down(&mut app, ally);
    // How long at a stretch the straight-line gap to the chest failed to shrink by 0.3 m.
    let (mut closest, mut since, mut longest) = (f64::MAX, None::<f64>, 0.0f64);
    let mut up = false;
    for _ in 0..60 * 20 {
        app.update();
        let now = clock(&app, reviver);
        let b = brain(&app, reviver);
        if b.action == NpcAction::Revive && b.revive_kneel <= 0.0 {
            let (p, c) = (at(&app, reviver), chest(&app, ally));
            let gap = DVec2::new(p.x - c.x, p.z - c.z).length();
            if gap + 0.3 < closest {
                (closest, since) = (gap, None);
            } else {
                longest = longest.max(now - *since.get_or_insert(now));
            }
        }
        if !downed(&app, ally) {
            up = true;
            break;
        }
    }
    eprintln!("no nearer as the crow flies for {longest:.1} s at a stretch");
    assert!(
        longest > 2.5,
        "the way round took no detour ({longest:.1} s): the wall does not test it"
    );
    assert!(up, "the reviver never got round the wall to its ally");
    assert_eq!(stats(&app, reviver).revives_aborted, 0);
    assert_eq!(stats(&app, reviver).revives, 1);
}

#[test]
fn nobody_revives_a_mate_up_on_a_ledge_from_below() {
    let mut app = app();
    // A block 3 m high with no way up: the ally downed at its edge on top, a mate on the
    // ground at its foot. However close it gets as the crow flies, it doesn't kneel there.
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, 1.5, 0.0)),
        Collider::cuboid(Vec3::new(2.0, 1.5, 2.0)),
    ));
    let ally = mate(&mut app, DVec3::new(1.5, 3.0, 0.5), NpcSkills::soldier());
    let reviver = mate(&mut app, DVec3::new(4.0, 0.0, 1.5), NpcSkills::rookie());
    run(&mut app, 0.5);
    let up_there = at(&app, ally).y;
    assert!(
        up_there > 2.5,
        "the ally is not up on the block ({up_there:.2} m)"
    );
    down(&mut app, ally);
    let (mut tried, mut nearest) = (false, f64::MAX);
    for _ in 0..60 * 12 {
        app.update();
        let b = brain(&app, reviver);
        let (p, c) = (at(&app, reviver), chest(&app, ally));
        if b.action == NpcAction::Revive {
            tried = true;
            nearest = nearest.min(DVec2::new(p.x - c.x, p.z - c.z).length());
        }
        assert!(
            b.revive_kneel <= 0.0,
            "kneels {:.1} m below its ally",
            c.y - p.y
        );
        assert!(downed(&app, ally), "revived from below");
    }
    // It went, and got as near as would count as there (0.9 m) on level ground.
    assert!(tried, "it never went to revive");
    assert!(
        nearest < 0.9,
        "came no nearer than {nearest:.2} m as the crow flies"
    );
    assert_eq!(stats(&app, reviver).revives, 0);
}
