//! Brain drives, animation animates (N2), on contracts only: no animation module here, so the
//! test plays its part (it arms a requested body with an `AttackWindow` and plays a wind-up in
//! it). Longsword fighters ask for bodies, archers and riflemen don't; the downed give theirs
//! back and ask again once up; and a strike the animation module plays is pressed through the
//! intent, lands through its blows (not this crate's), and has its telegraph timed to the
//! brain's.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::intent_body::loadout_of;
use engine_npc::*;
use engine_world::animation::{AnimationParams, AnimationTrigger, BodyRequest, StrikeTargets};
use engine_world::character::draws_appearances;
use engine_world::combat::{AttackPhase as WindowPhase, AttackWindow, ItemClass, Loadout};
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
    // Not yet: the animation module's round shield carries no team colours.
    assert!(
        asked(&app, shield).is_none(),
        "shield fighters keep the stand-in's pose"
    );
    assert_eq!(
        loadout_of(&NpcWeapon::sword_and_shield()),
        Some(Loadout {
            main: ItemClass::OneHandedBlade,
            off: ItemClass::Shield,
            two_handed: false,
        })
    );
    assert!(app.world.get::<StrikeTargets>(sword).is_some());
    assert!(
        asked(&app, rifle).is_none(),
        "riflemen keep the stand-in's pose"
    );
    assert!(
        asked(&app, bow).is_none(),
        "archers keep the stand-in's pose"
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
fn the_downed_give_their_bodies_back_and_ask_again_once_up() {
    let mut app = app();
    let e = fighter(&mut app, 0.0, Faction::ENEMY, NpcWeapon::sword(), 1);
    app.world.get_mut::<Npc>(e).unwrap().active = false;
    run(&mut app, 0.2);
    assert!(app.world.get::<BodyRequest>(e).is_some());
    app.world.get_mut::<NpcVitals>(e).unwrap().downed = true;
    run(&mut app, 0.5);
    assert!(
        app.world.get::<BodyRequest>(e).is_none(),
        "the stand-in's pose lies it down"
    );
    app.world.get_mut::<NpcVitals>(e).unwrap().downed = false;
    run(&mut app, 1.0);
    assert!(
        app.world.get::<BodyRequest>(e).is_none(),
        "the stand-in's pose gets it up"
    );
    run(&mut app, NpcBrain::GET_UP_SECS as f32);
    assert!(
        app.world.get::<BodyRequest>(e).is_some(),
        "asks again once up"
    );
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
}
