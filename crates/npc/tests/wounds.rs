//! Fighters fight on with what their wounds leave them (`wounds`), read from the combat
//! module's contracts (no combat module here: the test writes the wounds and empties the hand
//! as it would). A fighter that can't stand on a leg neither runs nor dashes nor dodges, and
//! crawls back to its mates or fights on from one knee; one disarmed fights with its fists and
//! backs off; a cut-through neck is not revived; head and neck are guarded harder than shins;
//! and where a foe's blows land is learned from `BodyHit`s.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::intent_body::loadout_of;
use engine_npc::wounds::{CRAWL_SPEED, ONE_LEG_SPEED};
use engine_npc::*;
use engine_world::animation::StrikeTargets;
use engine_world::combat::{Approach, BodyHit, BodyInjuries, BodyZone, ItemClass, Loadout};
use engine_world::locomotion::MoveIntent;
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const STEP: f32 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: STEP as f64,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn run(app: &mut App, secs: f32) {
    app.update_n((secs / STEP).ceil() as usize);
}

fn fighter(
    app: &mut App,
    x: f64,
    faction: Faction,
    skills: NpcSkills,
    weapon: NpcWeapon,
    seed: u64,
) -> Entity {
    let mut t = Transform::from_translation(DVec3::new(x, 0.0, 0.0));
    t.look_to(if x < 0.0 { Vec3::X } else { Vec3::NEG_X }, Vec3::Y);
    app.world
        .spawn((
            t,
            Npc::default(),
            faction,
            Squad(faction.0 + 3),
            skills,
            weapon,
            Seed(seed),
        ))
        .id()
}

fn knee(wound: f32) -> BodyInjuries {
    let mut i = BodyInjuries::default();
    i.get_mut(BodyZone::KneeL).wound = wound;
    i
}

fn heal(app: &mut App, es: &[Entity]) {
    for &e in es {
        let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
        v.health = v.max_health;
    }
}

fn x_of(app: &App, e: Entity) -> f64 {
    app.world.get::<Transform>(e).unwrap().translation.x
}

#[test]
fn a_fighter_on_a_ruined_knee_neither_runs_nor_dodges_and_crawls_back() {
    let mut app = app();
    let lame = fighter(
        &mut app,
        -4.0,
        Faction::ENEMY,
        NpcSkills::rookie(),
        NpcWeapon::sword(),
        1,
    );
    let mate = fighter(
        &mut app,
        -12.0,
        Faction::ENEMY,
        NpcSkills::rookie(),
        NpcWeapon::sword(),
        3,
    );
    let foe = fighter(
        &mut app,
        4.0,
        Faction::RIVAL,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        2,
    );
    app.world.get_mut::<Npc>(foe).unwrap().active = false;
    app.world.get_mut::<Npc>(mate).unwrap().active = false;
    app.world.entity_mut(lame).insert(knee(0.7));
    let (mut fastest, mut back, mut dodges) = (0.0f32, 0, 0);
    let mut was = x_of(&app, lame);
    for _ in 0..(6.0 / STEP) as usize {
        app.update();
        heal(&mut app, &[lame, foe]);
        let i = app.world.get::<MoveIntent>(lame).unwrap();
        assert!(!i.sprint && i.dash_to.is_none(), "runs on a ruined knee");
        let x = x_of(&app, lame);
        fastest = fastest.max(((x - was).abs() / STEP as f64) as f32);
        was = x;
        let b = app.world.get::<NpcBrain>(lame).unwrap();
        assert!(!matches!(
            b.action,
            NpcAction::Flank | NpcAction::SeekHighGround
        ));
        back += usize::from(b.action == NpcAction::Retreat);
        dodges = app.world.get::<NpcCombat>(lame).unwrap().stats.dodges;
    }
    assert!(
        fastest <= ONE_LEG_SPEED + 0.05,
        "drags itself at {fastest:.2} m/s"
    );
    assert!(
        back > 60,
        "a rookie crawls back to its mates ({back} steps)"
    );
    assert!(x_of(&app, lame) < -4.0, "and gets away from the foe");
    assert_eq!(dodges, 0);

    // Both legs gone: a crawl.
    let mut both = knee(0.7);
    both.get_mut(BodyZone::KneeR).severed = true;
    app.world.entity_mut(lame).insert(both);
    let mut fastest = 0.0f32;
    for _ in 0..(2.0 / STEP) as usize {
        let was = x_of(&app, lame);
        app.update();
        fastest = fastest.max(((x_of(&app, lame) - was).abs() / STEP as f64) as f32);
        assert!(app.world.get::<MoveIntent>(lame).unwrap().crouch);
    }
    assert!(fastest <= CRAWL_SPEED + 0.05, "crawls at {fastest:.2} m/s");
}

#[test]
fn a_brave_fighter_on_one_leg_fights_on_from_one_knee_and_does_not_chase() {
    let mut app = app();
    let brave = NpcSkills {
        courage: 1.0,
        ..NpcSkills::master()
    };
    let lame = fighter(&mut app, -1.2, Faction::ENEMY, brave, NpcWeapon::sword(), 1);
    let foe = fighter(
        &mut app,
        1.2,
        Faction::RIVAL,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        2,
    );
    app.world.entity_mut(lame).insert(knee(0.8));
    let (mut knelt, mut struck) = (0, 0);
    for _ in 0..(4.0 / STEP) as usize {
        app.update();
        heal(&mut app, &[lame, foe]);
        let b = app.world.get::<NpcBrain>(lame).unwrap();
        if b.action == NpcAction::Engage {
            knelt += usize::from(app.world.get::<MoveIntent>(lame).unwrap().crouch);
        }
        struck = app.world.get::<NpcCombat>(lame).unwrap().stats.attacks;
    }
    assert!(knelt > 60, "fights from one knee ({knelt} steps)");
    assert!(struck > 0, "and still strikes");
    // The foe steps out of reach: it doesn't go after it.
    app.world.get_mut::<Npc>(foe).unwrap().active = false;
    app.world.get_mut::<Transform>(foe).unwrap().translation.x = 9.0;
    let from = x_of(&app, lame);
    run(&mut app, 3.0);
    assert!(
        x_of(&app, lame) - from < 1.0,
        "chased {:.1} m",
        x_of(&app, lame) - from
    );
}

#[test]
fn a_disarmed_fighter_fights_with_its_fists_and_backs_off() {
    let mut app = app();
    let sword = NpcWeapon::sword();
    let bare = fighter(
        &mut app,
        -3.0,
        Faction::ENEMY,
        NpcSkills::rookie(),
        sword,
        1,
    );
    let foe = fighter(
        &mut app,
        3.0,
        Faction::RIVAL,
        NpcSkills::veteran(),
        sword,
        2,
    );
    app.world.get_mut::<Npc>(foe).unwrap().active = false;
    // The combat module's disarm: the sword hand of the loadout empties.
    let kit = loadout_of(&sword).unwrap();
    app.world.entity_mut(bare).insert(Loadout {
        main: ItemClass::None,
        two_handed: false,
        ..kit
    });
    let (mut back, mut blocks) = (0, 0);
    for _ in 0..(4.0 / STEP) as usize {
        app.update();
        heal(&mut app, &[bare, foe]);
        let b = app.world.get::<NpcBrain>(bare).unwrap();
        back += usize::from(b.action == NpcAction::Retreat);
        blocks = app.world.get::<NpcCombat>(bare).unwrap().stats.blocks;
    }
    assert!(back > 60, "backs off ({back} steps)");
    assert!(x_of(&app, bare) < -3.0, "away from the foe");
    assert_eq!(blocks, 0, "fists don't block a sword");
    // With its sword it would have stood its ground.
    let mut armed = self::app();
    let e = fighter(
        &mut armed,
        -3.0,
        Faction::ENEMY,
        NpcSkills::rookie(),
        sword,
        1,
    );
    let f = fighter(
        &mut armed,
        3.0,
        Faction::RIVAL,
        NpcSkills::veteran(),
        sword,
        2,
    );
    armed.world.get_mut::<Npc>(f).unwrap().active = false;
    armed.world.entity_mut(e).insert(kit);
    let mut back = 0;
    for _ in 0..(4.0 / STEP) as usize {
        armed.update();
        heal(&mut armed, &[e, f]);
        back += usize::from(armed.world.get::<NpcBrain>(e).unwrap().action == NpcAction::Retreat);
    }
    assert_eq!(back, 0);
}

fn cut_down(app: &mut App, e: Entity) {
    let at = app.world.get::<Transform>(e).unwrap().translation;
    app.world.write_message(DamageEvent {
        target: e,
        amount: 1000.0,
        kind: DamageKind::Slash,
        point: at + DVec3::Y * 1.5,
        impulse: Vec3::Z * 10.0,
        source: None,
    });
    run(app, 0.1);
    assert!(app.world.get::<NpcVitals>(e).unwrap().downed);
}

#[test]
fn no_one_revives_a_fighter_whose_neck_was_cut_through() {
    for severed in [false, true] {
        let mut app = app();
        let ally = fighter(
            &mut app,
            0.0,
            Faction::ENEMY,
            NpcSkills::veteran(),
            NpcWeapon::sword(),
            1,
        );
        let mate = fighter(
            &mut app,
            6.0,
            Faction::ENEMY,
            NpcSkills::veteran(),
            NpcWeapon::sword(),
            2,
        );
        run(&mut app, 0.3);
        cut_down(&mut app, ally);
        if severed {
            let mut i = BodyInjuries::default();
            i.get_mut(BodyZone::Neck).severed = true;
            app.world.entity_mut(ally).insert(i);
        }
        let mut went = false;
        for _ in 0..(10.0 / STEP) as usize {
            app.update();
            went |= app.world.get::<NpcBrain>(mate).unwrap().action == NpcAction::Revive;
        }
        let up = !app.world.get::<NpcVitals>(ally).unwrap().downed;
        if severed {
            assert!(!went && !up, "revived a body without its head");
        } else {
            assert!(went && up, "a mate gets it up");
        }
    }
}

/// How many of `n` strikes at `zone` a veteran defender meets with a guard, parry or dodge.
fn guarded(zone: BodyZone, learned: bool) -> (u32, u32) {
    let mut app = app();
    let def = fighter(
        &mut app,
        -1.0,
        Faction::ENEMY,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        1,
    );
    let att = fighter(
        &mut app,
        1.0,
        Faction::RIVAL,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        2,
    );
    app.world.get_mut::<Npc>(def).unwrap().active = false;
    app.world.entity_mut(att).insert(StrikeTargets {
        target: Some(def),
        zone: Some(zone),
        ..Default::default()
    });
    if learned {
        for _ in 0..8 {
            app.world.write_message(BodyHit {
                target: def,
                attacker: Some(att),
                zone,
                approach: Approach::Front,
                bone: None,
                point: Vec3::ZERO,
                kind: DamageKind::Slash,
                energy_j: 50.0,
                cut_quality: 0.5,
                blocked_by: None,
            });
            app.update();
        }
    }
    for _ in 0..(40.0 / STEP) as usize {
        app.update();
        heal(&mut app, &[def, att]);
        // Keep aiming there (the brain doesn't write it without the animation module).
        app.world.get_mut::<StrikeTargets>(att).unwrap().zone = Some(zone);
    }
    let d = app.world.get::<NpcCombat>(def).unwrap().stats;
    let a = app.world.get::<NpcCombat>(att).unwrap().stats;
    (d.blocks + d.parries + d.dodges, a.attacks)
}

#[test]
fn head_and_neck_are_guarded_harder_than_shins_and_a_habit_is_learned() {
    let (head, n_head) = guarded(BodyZone::Head, false);
    let (shin, n_shin) = guarded(BodyZone::ShinL, false);
    assert!(n_head > 15 && n_shin > 15, "{n_head} {n_shin} strikes");
    let (h, s) = (head as f32 / n_head as f32, shin as f32 / n_shin as f32);
    assert!(h > s + 0.1, "guards the head {h:.2}, the shin {s:.2}");
    // A foe that keeps going for the shin gets it guarded better.
    let (learned, n) = guarded(BodyZone::ShinL, true);
    let l = learned as f32 / n as f32;
    assert!(l > s + 0.05, "learned shin guard {l:.2} vs {s:.2}");
}

#[test]
fn where_a_foes_blows_land_is_learned_from_body_hits() {
    let mut app = app();
    let foe = app.world.spawn_empty().id();
    let me = app.world.spawn_empty().id();
    for _ in 0..6 {
        app.world.write_message(BodyHit {
            target: me,
            attacker: Some(foe),
            zone: BodyZone::KneeL,
            approach: Approach::Front,
            bone: None,
            point: Vec3::ZERO,
            kind: DamageKind::Slash,
            energy_j: 40.0,
            cut_quality: 0.5,
            blocked_by: None,
        });
        app.update();
    }
    let m = app.world.resource::<OpponentModels>().get(foe);
    assert!(m.aim_share(BodyZone::KneeL) > 0.5, "{:?}", m.aims);
    // A guard low covers the thigh beside the knee too, a little.
    assert!(m.aim_share(BodyZone::ThighL) > 0.2);
    assert!(m.aim_share(BodyZone::Head) < 0.05);
}

#[test]
fn a_bleeding_wound_lowers_health_only() {
    let mut app = app();
    let e = fighter(
        &mut app,
        0.0,
        Faction::ENEMY,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        7,
    );
    run(&mut app, 0.5);
    let at = app.world.get::<Transform>(e).unwrap().translation;
    let full = app.world.get::<NpcVitals>(e).unwrap().max_health;
    let bleed = |amount| DamageEvent {
        target: e,
        amount,
        kind: DamageKind::Bleed,
        point: at + DVec3::Y,
        impulse: Vec3::ZERO,
        source: None,
    };
    // A wound bleeding for two seconds, a little each step.
    for _ in 0..120 {
        app.world.write_message(bleed(0.1));
        app.update();
    }
    let v = app.world.get::<NpcVitals>(e).unwrap();
    assert!(v.health < full - 10.0, "health {} of {full}", v.health);
    assert_eq!(v.poise, 1.0, "bleeding wears no balance");
    let c = app.world.get::<NpcCombat>(e).unwrap();
    assert_eq!(c.stats.hits_taken, 0);
    assert_eq!(c.last_hurt, 0.0, "felt as no blow");
    assert!(!matches!(c.phase, AttackPhase::Staggered { .. }));
    let m = app.world.get::<NpcMemory>(e).unwrap();
    assert!(
        m.known.is_empty() && m.stimulus.is_none(),
        "nobody to look for"
    );
    assert_eq!(
        app.world.get::<NpcBrain>(e).unwrap().action,
        NpcAction::Idle
    );
    // Bled out: it goes down where it stands.
    app.world.write_message(bleed(full));
    run(&mut app, 0.1);
    assert!(app.world.get::<NpcVitals>(e).unwrap().downed);
}
