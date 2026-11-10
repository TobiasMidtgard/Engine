//! Spells on minds (`engine_world::MindEffect`, `Statuses::veil`): the terrified run, the
//! calmed stand down until struck, the veiled go unseen, the commanded fight for the side that
//! took them and the frenzied strike whoever is nearest; animals bolt and settle.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::locomotion::LocomotionState;
use engine_world::{DamageEvent, DamageKind, MindEffect, MindKind, Statuses, WorldPlugin};

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn run(app: &mut App, secs: f32) {
    app.update_n((secs * 60.0).ceil() as usize);
}

fn facing(at: DVec3, dir: Vec3) -> Transform {
    let mut t = Transform::from_translation(at);
    t.look_to(dir, Vec3::Y);
    t
}

fn fighter(app: &mut App, at: DVec3, face: Vec3, faction: Faction, weapon: NpcWeapon) -> Entity {
    app.world
        .spawn((
            facing(at, face),
            Npc::default(),
            faction,
            NpcSkills::veteran(),
            weapon,
        ))
        .id()
}

/// A stand-in player: a body with a faction that NPCs can see.
fn dummy(app: &mut App, at: DVec3, face: Vec3) -> Entity {
    app.world
        .spawn((
            facing(at, face),
            Faction::PLAYER,
            LocomotionState::default(),
        ))
        .id()
}

fn cast(app: &mut App, target: Entity, kind: MindKind, secs: f32, source: Option<Entity>) {
    app.world
        .resource_mut::<Messages<MindEffect>>()
        .write(MindEffect {
            target,
            kind,
            secs,
            source,
        });
}

fn awareness(app: &App, npc: Entity, of: Entity) -> f32 {
    app.world
        .get::<NpcMemory>(npc)
        .unwrap()
        .get(of)
        .map_or(0.0, |k| k.awareness)
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn action(app: &App, e: Entity) -> NpcAction {
    app.world.get::<NpcBrain>(e).unwrap().action
}

fn stats(app: &App, e: Entity) -> engine_npc::agent::CombatStats {
    app.world.get::<NpcCombat>(e).unwrap().stats
}

#[test]
fn a_veiled_body_is_not_seen_until_the_veil_lifts() {
    let mut app = app();
    let npc = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::rifle(),
    );
    app.world.get_mut::<Npc>(npc).unwrap().active = false;
    let player = dummy(&mut app, DVec3::new(0.0, 0.0, -15.0), Vec3::Z);
    app.world.entity_mut(player).insert(Statuses {
        veil: 1.0,
        ..Default::default()
    });
    run(&mut app, 2.0);
    assert!(awareness(&app, npc, player) < 0.01, "seen through the veil");
    // The veil lifts: seen as plainly as anyone.
    app.world.get_mut::<Statuses>(player).unwrap().veil = 0.0;
    run(&mut app, 2.0);
    assert!(
        awareness(&app, npc, player) > 0.95,
        "{}",
        awareness(&app, npc, player)
    );
}

#[test]
fn a_veil_spell_hides_a_fighter_from_its_enemies() {
    let mut app = app();
    let a = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::rifle(),
    );
    let b = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, -15.0),
        Vec3::Z,
        Faction::RIVAL,
        NpcWeapon::rifle(),
    );
    for e in [a, b] {
        app.world.get_mut::<Npc>(e).unwrap().active = false;
    }
    cast(&mut app, b, MindKind::Veil, 3.0, None);
    run(&mut app, 2.0);
    assert!(awareness(&app, a, b) < 0.01, "the veiled one was seen");
    assert!(awareness(&app, b, a) > 0.95, "the veiled one still sees");
    // The veil runs out.
    run(&mut app, 3.0);
    assert!(awareness(&app, a, b) > 0.95);
    let now = app.world.resource::<Time>().elapsed_secs_f64();
    assert!(!app.world.get::<Spellbound>(b).unwrap().veiled(now));
}

#[test]
fn a_frightened_fighter_runs_from_the_caster_and_then_comes_back_to_itself() {
    let mut app = app();
    let npc = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::rifle(),
    );
    let player = dummy(&mut app, DVec3::new(0.0, 0.0, -14.0), Vec3::Z);
    run(&mut app, 2.0);
    assert!(awareness(&app, npc, player) > 0.9);
    let shots = stats(&app, npc).shots;
    let start = pos(&app, npc).distance(pos(&app, player));
    cast(&mut app, npc, MindKind::Fear, 4.0, Some(player));
    run(&mut app, 3.0);
    assert_eq!(action(&app, npc), NpcAction::Flee);
    let away = pos(&app, npc).distance(pos(&app, player));
    assert!(away > start + 8.0, "ran {start:.1} -> {away:.1} m");
    // It fires nothing while it runs (a shot already under way may land as it turns).
    assert!(stats(&app, npc).shots <= shots + 1);
    run(&mut app, 3.0);
    assert_ne!(action(&app, npc), NpcAction::Flee, "fear wore off");
}

#[test]
fn the_calmed_stand_down_until_struck() {
    let mut app = app();
    let a = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::sword(),
    );
    let b = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, -5.0),
        Vec3::Z,
        Faction::RIVAL,
        NpcWeapon::sword(),
    );
    for e in [a, b] {
        cast(&mut app, e, MindKind::Calm, 30.0, None);
    }
    run(&mut app, 5.0);
    for e in [a, b] {
        assert_eq!(stats(&app, e).attacks, 0, "a calmed fighter attacked");
        assert_eq!(app.world.get::<NpcBrain>(e).unwrap().target, None);
    }
    // A blow ends it: the struck one fights back.
    let point = pos(&app, a) + DVec3::Y;
    app.world
        .resource_mut::<Messages<DamageEvent>>()
        .write(DamageEvent {
            target: a,
            amount: 5.0,
            kind: DamageKind::Slash,
            point,
            impulse: Vec3::ZERO,
            source: Some(b),
        });
    run(&mut app, 4.0);
    assert!(stats(&app, a).attacks >= 1, "{:?}", stats(&app, a));
}

#[test]
fn a_commanded_fighter_turns_on_its_own_side_and_goes_back_after() {
    let mut app = app();
    let turned = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::sword(),
    );
    let mate = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, -4.0),
        Vec3::Z,
        Faction::ENEMY,
        NpcWeapon::sword(),
    );
    for e in [turned, mate] {
        app.world.entity_mut(e).insert(Squad(3));
    }
    let caster = dummy(&mut app, DVec3::new(8.0, 0.0, 0.0), Vec3::NEG_X);
    cast(
        &mut app,
        turned,
        MindKind::Command {
            side: Faction::PLAYER.0,
        },
        6.0,
        Some(caster),
    );
    run(&mut app, 4.0);
    assert_eq!(*app.world.get::<Faction>(turned).unwrap(), Faction::PLAYER);
    assert_ne!(
        app.world.get::<Squad>(turned),
        Some(&Squad(3)),
        "left its squad"
    );
    assert!(
        stats(&app, turned).attacks >= 1,
        "{:?}",
        stats(&app, turned)
    );
    assert_eq!(
        app.world.get::<NpcBrain>(turned).unwrap().target,
        Some(mate),
        "fights its old mate"
    );
    run(&mut app, 4.0);
    assert_eq!(*app.world.get::<Faction>(turned).unwrap(), Faction::ENEMY);
    assert_eq!(app.world.get::<Squad>(turned), Some(&Squad(3)));
}

#[test]
fn a_frenzied_fighter_strikes_the_nearest_friend_or_foe() {
    let mut app = app();
    let mad = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::sword(),
    );
    let mate = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, -3.0),
        Vec3::Z,
        Faction::ENEMY,
        NpcWeapon::sword(),
    );
    for e in [mad, mate] {
        app.world.entity_mut(e).insert(Squad(5));
    }
    cast(&mut app, mad, MindKind::Frenzy, 6.0, None);
    run(&mut app, 5.0);
    let f = *app.world.get::<Faction>(mad).unwrap();
    assert!(f.0 >= engine_npc::minds::FRENZY_FACTIONS, "{f:?}");
    assert_eq!(app.world.get::<NpcBrain>(mad).unwrap().target, Some(mate));
    assert!(stats(&app, mad).attacks >= 1, "{:?}", stats(&app, mad));
    run(&mut app, 3.0);
    assert_eq!(*app.world.get::<Faction>(mad).unwrap(), Faction::ENEMY);
}

#[test]
fn animals_bolt_from_fear_and_settle_when_calmed() {
    let mut app = app();
    let deer = app
        .world
        .spawn((
            Transform::from_translation(DVec3::ZERO),
            Animal::new(Species::Deer),
        ))
        .id();
    let caster = dummy(&mut app, DVec3::new(30.0, 0.0, 0.0), Vec3::NEG_X);
    run(&mut app, 1.0);
    cast(&mut app, deer, MindKind::Fear, 3.0, Some(caster));
    run(&mut app, 1.5);
    assert_eq!(
        app.world.get::<AnimalMind>(deer).unwrap().state,
        AnimalState::Flee
    );
    let fled = pos(&app, deer);
    assert!(fled.x < -3.0, "ran away from the caster: {fled}");
    cast(&mut app, deer, MindKind::Calm, 10.0, None);
    run(&mut app, 4.0);
    let s = app.world.get::<AnimalMind>(deer).unwrap().state;
    assert!(
        !matches!(s, AnimalState::Flee | AnimalState::Alert),
        "still {s:?}"
    );
}
