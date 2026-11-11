//! Brain drives, animation animates (N2), on contracts only: no animation module here, so the
//! test plays its part (it arms a requested body with an `AttackWindow` and plays a wind-up in
//! it). Hand-to-hand fighters, archers and riflemen ask for bodies; an archer draws, holds and
//! looses, and a rifleman aims, fires and changes magazines, on the animation module's cues as
//! its brain shoots; the downed keep
//! theirs and a mate kneels where that body lies; and a strike the animation module plays is pressed through the
//! intent, lands through its blows (not this crate's), and has its telegraph timed to the
//! brain's, aimed at the zone its brain picks.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::intent_body::{MAGAZINE_SHOTS, RELOAD_SECS, loadout_of};
use engine_npc::*;
use engine_world::animation::{AnimationParams, AnimationTrigger, BodyRequest, StrikeTargets};
use engine_world::character::draws_appearances;
use engine_world::combat::{
    AttackPhase as WindowPhase, AttackWindow, CombatSkills, ItemClass, Loadout,
};
use engine_world::locomotion::MoveIntent;
use engine_world::{DamageEvent, WorldPlugin};

const STEP: f32 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: STEP as f64,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    draws_appearances(&mut app, "test");
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn fighter(app: &mut App, x: f64, faction: Faction, weapon: NpcWeapon, seed: u64) -> Entity {
    let mut t = Transform::from_translation(DVec3::new(x, 0.0, 0.0));
    t.look_to(if x < 0.0 { Vec3::X } else { Vec3::NEG_X }, Vec3::Y);
    app.world
        .spawn((
            t,
            Npc::default(),
            faction,
            Squad(faction.0 + 3),
            NpcSkills::veteran(),
            weapon,
            Seed(seed),
            StandIn::default(),
        ))
        .id()
}

fn run(app: &mut App, secs: f32) {
    app.update_n((secs / STEP).ceil() as usize);
}

#[test]
fn hand_to_hand_people_ask_the_animation_module_for_bodies() {
    let mut app = app();
    let sword = fighter(&mut app, 0.0, Faction::ENEMY, NpcWeapon::sword(), 1);
    let shield = fighter(
        &mut app,
        3.0,
        Faction::ENEMY,
        NpcWeapon::sword_and_shield(),
        2,
    );
    let rifle = fighter(&mut app, 6.0, Faction::ENEMY, NpcWeapon::rifle(), 3);
    let bow = fighter(&mut app, 9.0, Faction::ENEMY, NpcWeapon::bow(), 4);
    for e in [sword, shield, rifle, bow] {
        app.world.get_mut::<Npc>(e).unwrap().active = false;
    }
    run(&mut app, 0.2);
    let asked = |app: &App, e: Entity| app.world.get::<BodyRequest>(e).cloned();
    let r = asked(&app, sword).expect("a longsword fighter asks");
    assert_eq!((r.kind.as_str(), r.seed), ("human", 1), "its own seed");
    assert_eq!(
        *app.world.get::<Loadout>(sword).unwrap(),
        Loadout {
            main: ItemClass::TwoHandedBlade,
            off: ItemClass::None,
            two_handed: true,
        }
    );
    // Sword and shield too: its own painted shield is drawn on the hand the module poses.
    assert!(asked(&app, shield).is_some(), "shield fighters ask");
    assert_eq!(
        *app.world.get::<Loadout>(shield).unwrap(),
        Loadout {
            main: ItemClass::OneHandedBlade,
            off: ItemClass::Shield,
            two_handed: false,
        }
    );
    assert_eq!(
        loadout_of(&NpcWeapon::sword()).map(|l| l.two_handed),
        Some(true)
    );
    assert!(app.world.get::<StrikeTargets>(sword).is_some());
    // How many lines of cut it knows: a veteran's skill with its two-handed blade.
    let skill = app.world.get::<CombatSkills>(sword).expect("its skill");
    let level = NpcSkills::veteran().level();
    assert!((skill.skill(ItemClass::TwoHandedBlade) - level).abs() < 1e-4);
    assert!(asked(&app, rifle).is_some(), "riflemen ask");
    assert_eq!(
        app.world.get::<Loadout>(rifle).unwrap().main,
        ItemClass::Firearm
    );
    // Archers too: the animation module's bow draws, holds and looses.
    assert!(asked(&app, bow).is_some(), "archers ask");
    assert_eq!(
        *app.world.get::<Loadout>(bow).unwrap(),
        Loadout {
            main: ItemClass::Bow,
            off: ItemClass::None,
            two_handed: true,
        }
    );

    // Off, and with no character module drawing people, nobody asks.
    let mut off = self::app();
    off.world.resource_mut::<NpcSettings>().anim_bodies = false;
    let a = fighter(&mut off, 0.0, Faction::ENEMY, NpcWeapon::sword(), 1);
    off.world.get_mut::<Npc>(a).unwrap().active = false;
    run(&mut off, 0.2);
    assert!(off.world.get::<BodyRequest>(a).is_none());
    let mut bare = App::new();
    bare.add_plugins((WorldPlugin, NpcPlugin));
    let b = bare
        .world
        .spawn((
            Transform::default(),
            Npc::default(),
            NpcWeapon::sword(),
            StandIn::default(),
        ))
        .id();
    bare.update_n(5);
    assert!(bare.world.get::<BodyRequest>(b).is_none());
}

#[test]
fn the_downed_keep_their_bodies_through_the_fall() {
    let mut app = app();
    let e = fighter(&mut app, 0.0, Faction::ENEMY, NpcWeapon::sword(), 1);
    app.world.get_mut::<Npc>(e).unwrap().active = false;
    run(&mut app, 0.2);
    assert!(app.world.get::<BodyRequest>(e).is_some());
    // The animation module takes its fall (the Ragdoll weight) and its get-up.
    app.world.get_mut::<NpcVitals>(e).unwrap().downed = true;
    run(&mut app, 0.5);
    assert!(
        app.world.get::<BodyRequest>(e).is_some(),
        "falls in its body"
    );
    app.world.get_mut::<NpcVitals>(e).unwrap().downed = false;
    run(&mut app, 0.5);
    assert!(
        app.world.get::<BodyRequest>(e).is_some(),
        "and gets up in it"
    );

    // One down before it ever had a body stays on the stand-in's pose until it is up.
    let mut app = self::app();
    let e = fighter(&mut app, 0.0, Faction::ENEMY, NpcWeapon::sword(), 1);
    app.world.get_mut::<Npc>(e).unwrap().active = false;
    app.world.entity_mut(e).insert(NpcVitals {
        downed: true,
        ..Default::default()
    });
    run(&mut app, 0.5);
    assert!(app.world.get::<BodyRequest>(e).is_none());
    app.world.get_mut::<NpcVitals>(e).unwrap().downed = false;
    run(&mut app, 1.0);
    assert!(
        app.world.get::<BodyRequest>(e).is_none(),
        "the stand-in's pose gets it up"
    );
    run(&mut app, NpcBrain::GET_UP_SECS as f32);
    assert!(app.world.get::<BodyRequest>(e).is_some(), "asks once up");
}

/// A pose of `e` lying on its back 2 m to its right (model +X), head toward model +Z, as the
/// animation module's ragdoll might leave it; where its breastbone is, in the world.
fn lie_aside(app: &mut App, e: Entity) -> DVec3 {
    use engine_world::animation::{BonePose, SkeletonPose};
    let bones = [
        ("hips", Vec3::new(2.0, 0.15, 0.0)),
        ("chest", Vec3::new(2.0, 0.18, 0.3)),
        ("neck", Vec3::new(2.0, 0.17, 0.55)),
        ("head", Vec3::new(2.0, 0.15, 0.75)),
    ];
    let pose = SkeletonPose {
        names: bones.iter().map(|(n, _)| n.to_string()).collect(),
        parents: vec![None, Some(0), Some(1), Some(2)].into(),
        bones: bones
            .iter()
            .map(|&(_, p)| BonePose {
                translation: p,
                ..Default::default()
            })
            .collect(),
    };
    app.world.entity_mut(e).insert(pose);
    let t = *app.world.get::<Transform>(e).unwrap();
    t.translation + (t.rotation * Vec3::new(2.0, 0.0, 0.425)).as_dvec3()
}

#[test]
fn a_mate_kneels_where_the_animation_module_let_the_body_fall() {
    let mut app = app();
    let ally = fighter(&mut app, 0.0, Faction::ENEMY, NpcWeapon::sword(), 1);
    let mate = fighter(&mut app, 6.0, Faction::ENEMY, NpcWeapon::sword(), 2);
    run(&mut app, 0.3);
    assert!(app.world.get::<BodyRequest>(ally).is_some());
    // Armed by the animation module (so the stand-in writes no pose of its own), cut down,
    // and lying 2 m from where it stood.
    for e in [ally, mate] {
        app.world.entity_mut(e).insert(AttackWindow::default());
    }
    let at = app.world.get::<Transform>(ally).unwrap().translation;
    app.world.write_message(DamageEvent {
        target: ally,
        amount: 1000.0,
        kind: engine_world::DamageKind::Ballistic,
        point: at + DVec3::Y,
        impulse: Vec3::Z * 10.0,
        source: None,
    });
    run(&mut app, 0.1);
    assert!(app.world.get::<NpcVitals>(ally).unwrap().downed);
    let chest = lie_aside(&mut app, ally);
    let mut knelt = 0;
    for _ in 0..60 * 12 {
        app.update();
        let b = app.world.get::<NpcBrain>(mate).unwrap();
        if b.action == NpcAction::Revive && b.revive_progress > 0.0 {
            knelt += 1;
            let p = app.world.get::<Transform>(mate).unwrap().translation;
            let gap = DVec2::new(p.x - chest.x, p.z - chest.z).length();
            assert!(gap < 0.8, "kneels {gap:.2} m from the body's chest");
            // In its own body, which the animation module kneels.
            assert!(app.world.get::<BodyRequest>(mate).is_some());
            let kneel = app.world.get::<AnimationParams>(mate).unwrap().get("kneel");
            assert_eq!(kneel, 1.0);
        }
        if !app.world.get::<NpcVitals>(ally).unwrap().downed {
            break;
        }
    }
    assert!(knelt > 60, "knelt by it ({knelt} steps)");
    assert!(!app.world.get::<NpcVitals>(ally).unwrap().downed, "revived");
    assert!(
        app.world.get::<BodyRequest>(ally).is_some(),
        "in its own body"
    );
    run(&mut app, 0.5);
    let kneel = app.world.get::<AnimationParams>(mate).unwrap().get("kneel");
    assert_eq!(kneel, 0.0, "and the mate stands again");
}

/// Plays the animation module's part for `e`: arms its body (an `AttackWindow`), and while
/// its brain winds a strike up shows a wind-up that lasts `natural` seconds at tempo 1.
fn play(app: &mut App, e: Entity, natural: f32, pressed: &mut usize, was: &mut bool) {
    let Some(params) = app.world.get::<AnimationParams>(e) else {
        return;
    };
    let speed = |n: &str| params.values.get(n).copied().unwrap_or(1.0);
    let length = natural / (speed("windup_speed") * speed("strike_speed"));
    let attack = app
        .world
        .get::<MoveIntent>(e)
        .is_some_and(|i| i.attack.is_some());
    if attack && !*was {
        *pressed += 1;
    }
    *was = attack;
    let winding = matches!(
        app.world.get::<NpcCombat>(e).map(|c| c.phase),
        Some(engine_npc::agent::AttackPhase::Windup { .. })
    );
    let w = AttackWindow {
        phase: winding.then_some(WindowPhase::Windup),
        name: "attack_light".into(),
        phase_secs: 0.0,
        phase_length_secs: if winding { length } else { 0.0 },
        ..Default::default()
    };
    app.world.entity_mut(e).insert(w);
}

#[test]
fn a_strike_the_animation_module_plays_lands_through_it_on_the_brains_time() {
    // Without the animation module playing them, the brain's strikes land as this crate's.
    let duel = |anim: bool| {
        let mut app = app();
        let a = fighter(&mut app, -0.8, Faction::ENEMY, NpcWeapon::sword(), 1);
        let b = fighter(&mut app, 0.8, Faction::RIVAL, NpcWeapon::sword(), 2);
        // A foe that guards poorly, so blows get through.
        app.world.entity_mut(b).insert(NpcSkills::rookie());
        let mut blows = app.world.resource::<Messages<DamageEvent>>().get_cursor();
        let mut cues = app
            .world
            .resource::<Messages<AnimationTrigger>>()
            .get_cursor();
        let (mut ours, mut pressed, mut was, mut strike_cues) = (0, 0, false, 0);
        for _ in 0..(8.0 / STEP) as usize {
            app.update();
            if anim && app.world.get::<BodyRequest>(a).is_some() {
                play(&mut app, a, 0.9, &mut pressed, &mut was);
            }
            let msgs = app.world.resource::<Messages<DamageEvent>>();
            ours += blows.read(msgs).filter(|d| d.source == Some(a)).count();
            let msgs = app.world.resource::<Messages<AnimationTrigger>>();
            strike_cues += cues
                .read(msgs)
                .filter(|t| t.entity == a && t.name.starts_with("attack_"))
                .count();
            // Keep both standing, so the fight goes on.
            for e in [a, b] {
                let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
                v.health = v.max_health;
            }
        }
        let stats = app.world.get::<NpcCombat>(a).unwrap().stats;
        (app, a, ours, pressed, strike_cues, stats.attacks)
    };
    let (_, _, ours, _, cues, attacks) = duel(false);
    assert!(attacks >= 2, "the brain strikes ({attacks})");
    assert!(ours >= 1, "its own blows land ({ours} of {attacks})");
    assert!(cues >= 2, "and are cued to the animation");

    let (app, a, ours, pressed, cues, attacks) = duel(true);
    assert!(attacks >= 2, "the brain strikes ({attacks})");
    assert!(app.world.get::<NpcCombat>(a).unwrap().anim_strikes);
    assert_eq!(ours, 0, "the animation module's blows land them");
    assert_eq!(cues, 0, "pressed once, through the intent");
    assert!(pressed >= 2, "each strike pressed ({pressed} of {attacks})");
    // The clip's telegraph is timed to the brain's wind-up.
    let params = app.world.get::<AnimationParams>(a).unwrap();
    let speed = params.get("strike_speed");
    let windup = params.get("windup_speed");
    assert!(speed > 1.0 && speed < 1.2, "a veteran's strikes: {speed}");
    let want = 0.9 / (NpcWeapon::sword().windup * speed);
    assert!(
        (windup - want).abs() < 0.02,
        "windup_speed {windup}, want {want}"
    );
    let target = app.world.get::<StrikeTargets>(a).unwrap().target;
    assert_eq!(target, app.world.get::<NpcBrain>(a).unwrap().target);
    assert!(target.is_some(), "strikes close on its foe");
    // Its own sword is drawn in the module's hands: the module's props are hidden.
    assert_eq!(params.get("hide_props"), 1.0);
    // And aim where its brain picked (`aim.rs`).
    let zone = app.world.get::<StrikeTargets>(a).unwrap().zone;
    assert!(zone.is_some(), "an aimed strike");
    assert_eq!(zone, app.world.get::<NpcCombat>(a).unwrap().zone);
}

#[test]
fn a_body_the_animation_module_poses_takes_its_strike_step_and_a_blows_push() {
    let mut app = app();
    let e = fighter(&mut app, 0.0, Faction::ENEMY, NpcWeapon::sword(), 1);
    app.world.get_mut::<Npc>(e).unwrap().active = false;
    run(&mut app, 0.2);
    assert!(app.world.get::<BodyRequest>(e).is_some());
    let at = |app: &App| app.world.get::<Transform>(e).unwrap().translation;
    // The module arms it: a strike that steps 2 cm along +X each frame it publishes.
    let step = Vec3::new(0.02, 0.0, 0.0);
    let frames = 30;
    app.update();
    let from = at(&app);
    for _ in 0..frames {
        app.world.entity_mut(e).insert(AttackWindow {
            warp_delta: step,
            ..Default::default()
        });
        app.update();
    }
    assert!(app.world.get::<NpcCombat>(e).unwrap().anim_strikes);
    let moved = at(&app) - from;
    assert!(
        (moved.x - 0.02 * frames as f64).abs() < 0.03 && moved.z.abs() < 0.01,
        "stepped {moved:?}"
    );
    // Between strikes it stays put (an unchanged window adds nothing); a blow pushes it.
    run(&mut app, 0.5);
    let from = at(&app);
    run(&mut app, 0.5);
    assert!((at(&app) - from).length() < 0.02, "stays between strikes");
    app.world.entity_mut(e).insert(engine_world::Knockback {
        velocity: Vec3::new(0.0, 0.0, -1.5),
    });
    run(&mut app, 0.4);
    let pushed = at(&app) - from;
    assert!(
        pushed.z < -0.45 && pushed.z > -0.75,
        "pushed back {pushed:?}"
    );
}

#[test]
fn an_archer_draws_holds_and_looses_on_the_animation_modules_cues() {
    let mut app = app();
    let archer = fighter(&mut app, -12.0, Faction::ENEMY, NpcWeapon::bow(), 1);
    let foe = fighter(&mut app, 12.0, Faction::RIVAL, NpcWeapon::sword(), 2);
    app.world.get_mut::<Npc>(foe).unwrap().active = false;
    run(&mut app, 0.2);
    assert!(app.world.get::<BodyRequest>(archer).is_some());
    app.world.entity_mut(archer).insert(AttackWindow::default());
    let mut cues = app
        .world
        .resource::<Messages<AnimationTrigger>>()
        .get_cursor();
    let mut shots = app
        .world
        .resource::<Messages<engine_world::ProjectileTrace>>()
        .get_cursor();
    let (mut seen, mut flew, mut pressed) = (Vec::new(), 0, false);
    for _ in 0..(8.0 / STEP) as usize {
        app.update();
        let msgs = app.world.resource::<Messages<AnimationTrigger>>();
        seen.extend(
            cues.read(msgs)
                .filter(|t| t.entity == archer)
                .map(|t| t.name.clone()),
        );
        let msgs = app
            .world
            .resource::<Messages<engine_world::ProjectileTrace>>();
        flew += shots.read(msgs).count();
        pressed |= app
            .world
            .get::<MoveIntent>(archer)
            .is_some_and(|i| i.attack.is_some());
        let mut v = app.world.get_mut::<NpcVitals>(foe).unwrap();
        v.health = v.max_health;
    }
    assert!(app.world.get::<NpcCombat>(archer).unwrap().anim_strikes);
    let draws = seen.iter().filter(|n| *n == "draw").count();
    let looses = seen.iter().filter(|n| *n == "loose").count();
    assert!(draws >= 2 && looses >= 2, "{seen:?}");
    // Each draw is let go before the next (a loose, or the string let down).
    for pair in seen.windows(2) {
        assert!(!(pair[0] == "draw" && pair[1] == "draw"), "{seen:?}");
    }
    assert!(
        !seen.iter().any(|n| n == "aim"),
        "no stand-in cue: {seen:?}"
    );
    assert!(!pressed, "not pressed as a quick shot");
    assert!(flew >= looses.min(2), "its arrows fly ({flew})");
    // It shoots the animation module's bow.
    let params = app.world.get::<AnimationParams>(archer).unwrap();
    assert_eq!(params.get("hide_props"), 0.0);
}

#[test]
fn a_rifleman_aims_fires_and_changes_magazines_on_the_animation_modules_cues() {
    let mut app = app();
    let gun = fighter(&mut app, -12.0, Faction::ENEMY, NpcWeapon::rifle(), 1);
    let foe = fighter(&mut app, 12.0, Faction::RIVAL, NpcWeapon::sword(), 2);
    app.world.get_mut::<Npc>(foe).unwrap().active = false;
    run(&mut app, 0.2);
    app.world.entity_mut(gun).insert(AttackWindow::default());
    let mut cues = app
        .world
        .resource::<Messages<AnimationTrigger>>()
        .get_cursor();
    let mut shots = app
        .world
        .resource::<Messages<engine_world::ProjectileTrace>>()
        .get_cursor();
    let (mut seen, mut pressed, mut aimed, mut reloading) = (Vec::new(), false, false, 0.0);
    let mut shots_in_reload = 0;
    for _ in 0..(14.0 / STEP) as usize {
        app.update();
        let msgs = app.world.resource::<Messages<AnimationTrigger>>();
        seen.extend(
            cues.read(msgs)
                .filter(|t| t.entity == gun)
                .map(|t| t.name.clone()),
        );
        let msgs = app
            .world
            .resource::<Messages<engine_world::ProjectileTrace>>();
        let flew = shots.read(msgs).count();
        pressed |= app
            .world
            .get::<MoveIntent>(gun)
            .is_some_and(|i| i.attack.is_some());
        let params = app.world.get::<AnimationParams>(gun).unwrap();
        aimed |= params.get("aim") > 0.5;
        if params.get("reloading") > 0.5 {
            reloading += STEP;
            shots_in_reload += flew;
        }
        let mut v = app.world.get_mut::<NpcVitals>(foe).unwrap();
        v.health = v.max_health;
    }
    let fires = seen.iter().filter(|n| *n == "fire").count();
    let reloads = seen.iter().filter(|n| *n == "reload").count();
    assert!(fires > MAGAZINE_SHOTS as usize, "{seen:?}");
    assert!(reloads >= 1, "{seen:?}");
    // A magazine's worth of shots, then the change.
    let first = seen.iter().position(|n| n == "reload").unwrap();
    let before = seen[..first].iter().filter(|n| *n == "fire").count();
    assert_eq!(before, MAGAZINE_SHOTS as usize, "{seen:?}");
    assert!(aimed, "shouldered");
    assert!(!pressed, "not pressed as a shot");
    // A real pause: no shot while the magazine changes, and foes see it.
    assert!(reloading > RELOAD_SECS * 0.8, "reloading {reloading:.2} s");
    assert_eq!(shots_in_reload, 0, "shot while reloading");
    let params = app.world.get::<AnimationParams>(gun).unwrap();
    assert_eq!(params.get("hide_props"), 0.0, "its carbine is the module's");
}
