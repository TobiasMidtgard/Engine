//! Behavior tests: whole NPCs in a headless app (no GPU), checking that the things that make
//! them "smart" actually happen: they see and remember, react within their reaction time (and
//! slower in slow motion), block and parry, take cover when shot at, flank, take turns,
//! share sightings, keep spacing in melee, and shoot explosives next to enemies.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::locomotion::LocomotionState;
use engine_world::physics::Collider;
use engine_world::{AttackEvent, AttackKind, DamageEvent, ProjectileTrace, WorldPlugin};

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

fn fighter(
    app: &mut App,
    at: DVec3,
    face: Vec3,
    faction: Faction,
    skills: NpcSkills,
    weapon: NpcWeapon,
) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at).looking_to(face, Vec3::Y),
            Npc::default(),
            faction,
            skills,
            weapon,
        ))
        .id()
}

/// A stand-in player: just a body with a faction that NPCs can see.
fn dummy(app: &mut App, at: DVec3, face: Vec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at).looking_to(face, Vec3::Y),
            Faction::PLAYER,
            LocomotionState::default(),
        ))
        .id()
}

fn wall(app: &mut App, center: DVec3, half: Vec3) -> Entity {
    app.world
        .spawn((Transform::from_translation(center), Collider::cuboid(half)))
        .id()
}

fn awareness(app: &App, npc: Entity, of: Entity) -> f32 {
    app.world
        .get::<NpcMemory>(npc)
        .unwrap()
        .get(of)
        .map(|k| k.awareness)
        .unwrap_or(0.0)
}

trait LookingTo {
    fn looking_to(self, dir: Vec3, up: Vec3) -> Self;
}
impl LookingTo for Transform {
    fn looking_to(mut self, dir: Vec3, up: Vec3) -> Self {
        self.look_to(dir, up);
        self
    }
}

#[test]
fn sees_what_is_in_front_not_behind_or_through_walls() {
    let mut app = app();
    let npc = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::veteran(),
        NpcWeapon::rifle(),
    );
    app.world.get_mut::<Npc>(npc).unwrap().active = false;
    let front = dummy(&mut app, DVec3::new(0.0, 0.0, -15.0), Vec3::Z);
    let behind = dummy(&mut app, DVec3::new(0.0, 0.0, 15.0), Vec3::Z);
    let hidden = dummy(&mut app, DVec3::new(10.0, 0.0, -10.0), Vec3::Z);
    wall(
        &mut app,
        DVec3::new(6.0, 1.5, -5.0),
        Vec3::new(1.0, 1.5, 3.0),
    );
    run(&mut app, 2.0);
    assert!(
        awareness(&app, npc, front) > 0.95,
        "front {}",
        awareness(&app, npc, front)
    );
    assert!(
        awareness(&app, npc, behind) < 0.01,
        "behind {}",
        awareness(&app, npc, behind)
    );
    assert!(
        awareness(&app, npc, hidden) < 0.01,
        "hidden {}",
        awareness(&app, npc, hidden)
    );
}

/// Throw `n` telegraphed melee attacks at the NPC and count how many it defended in time.
fn defended(local: Option<LocalTimeScale>, skills: NpcSkills, n: usize) -> (u32, u32) {
    let mut app = app();
    let npc = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        skills,
        NpcWeapon::sword(),
    );
    app.world.get_mut::<Npc>(npc).unwrap().active = false;
    if let Some(l) = local {
        app.world.entity_mut(npc).insert(l);
    }
    let player = dummy(&mut app, DVec3::new(0.0, 0.0, -1.6), Vec3::Z);
    run(&mut app, 1.0);
    let mut in_time = 0;
    for _ in 0..n {
        app.world.write_message(AttackEvent {
            attacker: player,
            target: Some(npc),
            windup_secs: 0.3,
            reach: 2.0,
            kind: AttackKind::Melee,
        });
        // Check the guard right when the hit lands.
        run(&mut app, 0.29);
        let now = app.world.get::<NpcClock>(npc).unwrap().now;
        let c = app.world.get::<NpcCombat>(npc).unwrap();
        if c.is_blocking(now) || c.is_dodging(now) {
            in_time += 1;
        }
        run(&mut app, 1.0);
    }
    let stats = app.world.get::<NpcCombat>(npc).unwrap().stats;
    (in_time, stats.reacted_too_late)
}

#[test]
fn reaction_time_and_defense_decide_who_blocks() {
    let (master, _) = defended(None, NpcSkills::master(), 10);
    let (rookie, rookie_late) = defended(None, NpcSkills::rookie(), 10);
    assert!(master >= 8, "master defended {master}/10");
    assert!(rookie <= 2, "rookie defended {rookie}/10");
    assert!(
        rookie_late >= 5,
        "rookie reacted too late {rookie_late} times"
    );
}

#[test]
fn slow_motion_slows_npc_reactions() {
    // A master slowed to a fifth of normal speed reacts 5× slower in world time: too late.
    let (slowed, late) = defended(
        Some(LocalTimeScale {
            factor: 0.2,
            ignore_world_dilation: false,
        }),
        NpcSkills::master(),
        6,
    );
    assert!(slowed <= 1, "slowed master defended {slowed}/6");
    assert!(late >= 4, "{late}");
}

#[test]
fn shooters_take_cover_when_shot_at() {
    let mut app = app();
    let npc = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, 0.0),
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::veteran(),
        NpcWeapon::rifle(),
    );
    let player = dummy(&mut app, DVec3::new(0.0, 0.0, -18.0), Vec3::Z);
    // A low wall a few meters to the side.
    wall(
        &mut app,
        DVec3::new(4.0, 0.6, -3.0),
        Vec3::new(1.5, 0.6, 0.3),
    );
    run(&mut app, 1.0);
    for _ in 0..30 {
        app.world.write_message(ProjectileTrace {
            from: DVec3::new(0.0, 1.5, -18.0),
            to: DVec3::new(0.6, 1.2, 2.0),
            radius: 0.05,
            speed: 600.0,
            shooter: Some(player),
        });
        run(&mut app, 0.2);
    }
    let t = app.world.get::<Transform>(npc).unwrap().translation;
    let brain = app.world.get::<NpcBrain>(npc).unwrap();
    // Behind the wall as seen from the player (the wall's back face is at z = -2.7).
    assert!(
        t.z > -2.7 && t.z < -1.0 && (t.x - 4.0).abs() < 2.2,
        "ended at {t} doing {:?}",
        brain.action
    );
    let world = app.world.resource::<TacticalWorld>();
    let blocked = world
        .raycast(DVec3::new(0.0, 1.6, -18.0), t + DVec3::Y * 0.9)
        .is_some();
    assert!(blocked, "the player can still see it at {t}");
    let stats = app.world.get::<NpcCombat>(npc).unwrap().stats;
    assert!(stats.shots > 0, "it should shoot back");
}

#[test]
fn melee_fighters_close_in_strike_and_hurt() {
    let mut app = app();
    let npc = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, 6.0),
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
    );
    let player = dummy(&mut app, DVec3::ZERO, Vec3::Z);
    run(&mut app, 6.0);
    let t = app.world.get::<Transform>(npc).unwrap().translation;
    let stats = app.world.get::<NpcCombat>(npc).unwrap().stats;
    assert!(t.distance(DVec3::ZERO) < 4.5, "kept too far: {t}");
    assert!(stats.attacks >= 2, "{stats:?}");
    assert!(stats.hits >= 1, "{stats:?}");
    let _ = player;
}

#[test]
fn melee_fighters_parry_and_punish_each_other() {
    let mut app = app();
    let a = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, 3.0),
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::master(),
        NpcWeapon::sword(),
    );
    let b = fighter(
        &mut app,
        DVec3::new(0.0, 0.0, -3.0),
        Vec3::Z,
        Faction::RIVAL,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
    );
    run(&mut app, 20.0);
    let sa = app.world.get::<NpcCombat>(a).unwrap().stats;
    let sb = app.world.get::<NpcCombat>(b).unwrap().stats;
    assert!(sa.attacks > 3 && sb.attacks > 3, "{sa:?} {sb:?}");
    assert!(
        sa.blocks + sa.parries + sa.dodges > 2,
        "master defends: {sa:?}"
    );
    assert!(
        sb.blocks + sb.parries + sb.dodges > 0,
        "veteran defends too: {sb:?}"
    );
    let ha = app.world.get::<NpcVitals>(a).unwrap().health;
    let hb = app.world.get::<NpcVitals>(b).unwrap().health;
    assert!(ha > hb, "the master should win the exchange: {ha} vs {hb}");
}

#[test]
fn squad_mates_share_sightings() {
    let mut app = app();
    let spotter = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::elite(),
        NpcWeapon::rifle(),
    );
    let mate = fighter(
        &mut app,
        DVec3::new(6.0, 0.0, 0.0),
        Vec3::Z,
        Faction::ENEMY,
        NpcSkills::elite(),
        NpcWeapon::rifle(),
    );
    for e in [spotter, mate] {
        app.world.entity_mut(e).insert(Squad(7));
    }
    let player = dummy(&mut app, DVec3::new(0.0, 0.0, -20.0), Vec3::Z);
    run(&mut app, 1.5);
    assert!(awareness(&app, spotter, player) > 0.9);
    let mem = app.world.get::<NpcMemory>(mate).unwrap();
    let k = mem.get(player).expect("the mate heard the callout");
    assert!(k.awareness >= 0.6);
}

#[test]
fn melee_squads_take_turns() {
    let mut app = app();
    app.world
        .resource_mut::<SquadSettings>()
        .max_melee_attackers = 2;
    let player = dummy(&mut app, DVec3::ZERO, Vec3::Z);
    let mut npcs = Vec::new();
    for k in 0..5 {
        let a = k as f64 / 5.0 * std::f64::consts::TAU;
        let at = DVec3::new(a.cos() * 6.0, 0.0, a.sin() * 6.0);
        let e = fighter(
            &mut app,
            at,
            (-at).as_vec3().normalize(),
            Faction::ENEMY,
            NpcSkills::elite(),
            NpcWeapon::sword(),
        );
        app.world.entity_mut(e).insert(Squad(1));
        npcs.push(e);
    }
    let mut max_winding = 0;
    for _ in 0..400 {
        app.update();
        let winding = npcs
            .iter()
            .filter(|&&e| {
                matches!(
                    app.world.get::<NpcCombat>(e).unwrap().phase,
                    AttackPhase::Windup { .. }
                )
            })
            .count();
        max_winding = max_winding.max(winding);
    }
    let attacks: u32 = npcs
        .iter()
        .map(|&e| app.world.get::<NpcCombat>(e).unwrap().stats.attacks)
        .sum();
    assert!(attacks >= 4, "{attacks}");
    assert!(max_winding <= 2, "{max_winding} attacked at once");
    let _ = player;
}

#[test]
fn shooters_detonate_explosives_next_to_enemies() {
    let mut app = app();
    let npc = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::elite(),
        NpcWeapon::rifle(),
    );
    let player = dummy(&mut app, DVec3::new(0.0, 0.0, -16.0), Vec3::Z);
    let barrel = app
        .world
        .spawn((
            Transform::from_xyz(1.2, 0.0, -16.5),
            ExplosiveProp::default(),
        ))
        .id();
    let mut hurt = false;
    for _ in 0..240 {
        app.update();
        let msgs = app.world.resource::<Messages<DamageEvent>>();
        let mut cursor = msgs.get_cursor();
        hurt |= cursor
            .read(msgs)
            .any(|d| d.target == player && d.kind == engine_world::DamageKind::Explosive);
    }
    let prop = app.world.get::<ExplosiveProp>(barrel).unwrap();
    assert!(prop.detonated, "the barrel should have been shot");
    assert!(hurt, "the blast should hurt the player");
    assert!(
        app.world
            .get::<NpcCombat>(npc)
            .unwrap()
            .stats
            .explosives_used
            >= 1
    );
}

#[test]
fn a_ranged_squad_sends_a_flanker() {
    let mut app = app();
    let _player = dummy(&mut app, DVec3::ZERO, Vec3::Z);
    // A low wall in front of the player so the squad has something to work around.
    wall(
        &mut app,
        DVec3::new(0.0, 0.6, 2.0),
        Vec3::new(3.0, 0.6, 0.3),
    );
    let mut squad = Vec::new();
    for (k, x) in [-3.0, 0.0, 3.0].into_iter().enumerate() {
        let skills = if k == 1 {
            NpcSkills::master()
        } else {
            NpcSkills::veteran()
        };
        let e = fighter(
            &mut app,
            DVec3::new(x, 0.0, 16.0),
            Vec3::NEG_Z,
            Faction::ENEMY,
            skills,
            NpcWeapon::rifle(),
        );
        app.world.entity_mut(e).insert(Squad(3));
        // Keep them alive for the test.
        app.world.get_mut::<NpcVitals>(e).unwrap().max_health = 1e6;
        app.world.get_mut::<NpcVitals>(e).unwrap().health = 1e6;
        squad.push(e);
    }
    run(&mut app, 8.0);
    let roles: Vec<&str> = squad
        .iter()
        .map(|&e| app.world.get::<NpcBrain>(e).unwrap().role)
        .collect();
    assert!(roles.contains(&"flank"), "{roles:?}");
    let flanker = squad[roles.iter().position(|r| *r == "flank").unwrap()];
    let p = app.world.get::<Transform>(flanker).unwrap().translation;
    // The player faces +Z (toward the squad); the flanker ends up well to its side.
    let dir = Vec3::new(p.x as f32, 0.0, p.z as f32).normalize();
    let angle = Vec3::Z.dot(dir).acos().to_degrees();
    assert!(angle > 55.0, "flanker at {p} ({angle}°)");
}

#[test]
fn a_movement_backend_takes_only_the_bodies_it_controls() {
    let mut app = app();
    engine_world::locomotion::claim_backend(&mut app, "test movement");
    let walker = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
    );
    let controlled = fighter(
        &mut app,
        DVec3::new(10.0, 0.0, 0.0),
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
    );
    app.world
        .entity_mut(controlled)
        .insert(engine_world::physics::CharacterController::default());
    for e in [walker, controlled] {
        app.world.get_mut::<Npc>(e).unwrap().active = false;
    }
    for _ in 0..60 {
        for e in [walker, controlled] {
            app.world
                .get_mut::<engine_world::locomotion::MoveIntent>(e)
                .unwrap()
                .direction = Vec3::Z;
        }
        app.update();
    }
    let moved = |e| app.world.get::<Transform>(e).unwrap().translation.z;
    assert!(
        moved(walker) > 2.0,
        "the fallback mover still moves NPCs without a controller"
    );
    assert!(
        moved(controlled).abs() < 1e-6,
        "the backend's bodies are left alone"
    );
}
