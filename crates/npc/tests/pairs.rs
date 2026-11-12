//! Two hand-to-hand fighters on one foe on the higher tiers (`pair`, `difficulty`): two
//! swordsmen of one squad against a player that keeps its guard up and turns to block the one
//! swinging at it (so it blocks whatever comes at its front), over several seeds, in a
//! headless app (no GPU).
//!
//! * Very hard: one holds the player from the front while the other goes round behind it, so
//!   the two end up on opposite sides and most of their hits land from behind; the first one
//!   shouts it ("Get behind them!").
//! * Extreme: the one holding the player steps in with a feint once its mate is placed
//!   behind it.
//! * Hard: each fights on its own, and nobody shouts.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::bark::{BarkLine, Barks};
use engine_npc::perception::BodyHeight;
use engine_npc::*;
use engine_world::locomotion::LocomotionState;
use engine_world::{CombatDifficulty, DamageEvent, DifficultyTier, SimulationFocus, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;
const SECS: f32 = 20.0;
const SEEDS: u64 = 6;
/// The player's reaction time, in frames (0.25 s).
const REACTION: usize = 15;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, NpcPlugin));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn fighter(
    app: &mut App,
    at: DVec3,
    faction: Faction,
    skills: NpcSkills,
    weapon: NpcWeapon,
    seed: u64,
) -> Entity {
    let mut t = Transform::from_translation(at);
    let face = Vec3::new(-at.x as f32, 0.0, -at.z as f32).normalize_or(Vec3::Z);
    t.look_to(face, Vec3::Y);
    let e = app
        .world
        .spawn((
            t,
            Npc::default(),
            faction,
            Squad(faction.0 + 3),
            skills,
            weapon,
            Seed(hash_u64(seed * 131 + at.x.to_bits())),
        ))
        .id();
    let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
    v.max_health = 100_000.0;
    v.health = 100_000.0;
    e
}

#[derive(Debug, Default)]
struct Fight {
    /// Hits on the foe by the two swordsmen, and how many of those came from behind it.
    hits: u32,
    behind: u32,
    /// Seconds the two stood on opposite sides of the foe (more than 120 degrees apart).
    apart_secs: f32,
    /// Baits: the holder's feints as its mate is placed to cut in.
    baits: u32,
    flank_barks: u32,
    /// The pair's roles were ever handed out.
    paired: bool,
}

/// The swordsmen: `LEVEL` (default soldiers: on their own they don't think to go round a foe).
fn skills() -> NpcSkills {
    std::env::var("LEVEL")
        .ok()
        .and_then(|l| NpcSkills::preset(&l))
        .unwrap_or_else(NpcSkills::soldier)
}

fn fight(tier: DifficultyTier, level: NpcSkills, seed: u64) -> Fight {
    let mut app = app();
    app.world.resource_mut::<CombatDifficulty>().tier = tier;
    // The player: it blocks all the time and turns to face the one swinging at it (else the
    // nearer), as quickly as a player's mouse but a reaction time late.
    let foe = app
        .world
        .spawn((
            Transform::from_translation(DVec3::ZERO),
            Faction::PLAYER,
            LocomotionState {
                blocking: true,
                ..Default::default()
            },
            BodyHeight(1.75),
            SimulationFocus::default(),
        ))
        .id();
    let a = fighter(
        &mut app,
        DVec3::new(-1.5, 0.0, 9.0),
        Faction::ENEMY,
        level,
        NpcWeapon::sword(),
        seed,
    );
    let b = fighter(
        &mut app,
        DVec3::new(1.5, 0.0, 9.5),
        Faction::ENEMY,
        level,
        NpcWeapon::sword(),
        seed,
    );
    let mut cursor = app.world.resource::<Messages<DamageEvent>>().get_cursor();
    let mut f = Fight::default();
    let mut baits: Vec<f64> = Vec::new();
    let mut seen: Vec<(DVec3, DVec3, bool, bool)> = Vec::new();
    let trace = std::env::var("PAIR_TRACE").is_ok();
    let flat = |v: DVec3| Vec3::new(v.x as f32, 0.0, v.z as f32).normalize_or_zero();
    for frame in 0..(SECS as f64 / STEP) as usize {
        let t = frame as f64 * STEP;
        {
            // (It sees who swings, or who is nearer, a player's reaction time late.)
            let at = |e: Entity| app.world.get::<Transform>(e).unwrap().translation;
            let swings = |e: Entity| {
                matches!(
                    app.world.get::<NpcCombat>(e).unwrap().phase,
                    AttackPhase::Windup { .. }
                )
            };
            seen.push((at(a), at(b), swings(a), swings(b)));
            let (pa, pb, sa, sb) = seen[seen.len().saturating_sub(1 + REACTION)];
            let near = match (sa, sb) {
                (true, false) => at(a),
                (false, true) => at(b),
                _ if pa.length() < pb.length() => at(a),
                _ => at(b),
            };
            let to = Vec3::new(near.x as f32, 0.0, near.z as f32);
            if to.length() > 0.1 {
                // (It turns at most 6 rad/s, as a player's camera would.)
                let mut t = app.world.get_mut::<Transform>(foe).unwrap();
                let now = t.forward();
                let want = to.normalize();
                let ang = now.angle_between(want).min(6.0 * STEP as f32);
                let side = now.cross(want).y.signum();
                let turned = Quat::from_rotation_y(ang * side) * now;
                t.look_to(turned, Vec3::Y);
            }
        }
        app.update();
        let foe_t = *app.world.get::<Transform>(foe).unwrap();
        let fwd = flat(foe_t.forward().as_dvec3());
        let at = |e: Entity| app.world.get::<Transform>(e).unwrap().translation;
        let (pa, pb) = (at(a), at(b));
        let (da, db) = (flat(pa - foe_t.translation), flat(pb - foe_t.translation));
        if da.dot(db) < -0.5 {
            f.apart_secs += STEP as f32;
        }

        for me in [a, b] {
            let brain = app.world.get::<NpcBrain>(me).unwrap();
            if brain.pair.is_some() {
                f.paired = true;
            }
            let feinting = matches!(
                app.world.get::<NpcCombat>(me).unwrap().phase,
                AttackPhase::Windup { feint: true, .. }
            );
            if brain.pair.is_some()
                && feinting
                && brain.destination_reason == "bait"
                && baits.last().is_none_or(|&s| t - s > 1.0)
            {
                baits.push(t);
            }
        }
        for d in cursor.read(app.world.resource::<Messages<DamageEvent>>()) {
            if d.target != foe || d.amount <= 0.0 {
                continue;
            }
            let Some(src) = d.source.filter(|&s| s == a || s == b) else {
                continue;
            };
            f.hits += 1;
            let from = flat(at(src) - foe_t.translation);
            if fwd.dot(from) < -0.2 {
                f.behind += 1;
            }
        }
        if trace && (frame % 30 == 0 || std::env::var("PAIR_TRACE").as_deref() == Ok("2")) {
            let role = |e: Entity| {
                let b = app.world.get::<NpcBrain>(e).unwrap();
                let c = app.world.get::<NpcCombat>(e).unwrap();
                format!(
                    "{:?} {:?} {} {:?}",
                    c.phase,
                    b.action,
                    b.pair.map_or("-", |r| r.name()),
                    b.destination_reason
                )
            };
            eprintln!(
                "t{t:.1} foe [{:.1}] fwd ({:.2},{:.2}) a {} [{pa:.1}] b {} [{pb:.1}] hits {} behind {}",
                foe_t.translation,
                fwd.x,
                fwd.z,
                role(a),
                role(b),
                f.hits,
                f.behind
            );
        }
        // Nobody goes down: the test is about where the cuts come from.
        for e in [a, b] {
            let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
            v.health = v.max_health;
        }
    }
    if trace {
        for e in [a, b] {
            eprintln!("{:?}", app.world.get::<NpcCombat>(e).unwrap().stats);
        }
    }
    f.baits = baits.len() as u32;
    f.flank_barks = app
        .world
        .resource::<Barks>()
        .said
        .iter()
        .filter(|b| b.line == BarkLine::Flank)
        .count() as u32;
    f
}

fn tier_env() -> DifficultyTier {
    match std::env::var("TIER").as_deref() {
        Ok("hard") => DifficultyTier::Hard,
        Ok("extreme") => DifficultyTier::Extreme,
        _ => DifficultyTier::VeryHard,
    }
}

/// `PAIR_TRACE=1 TIER=hard|veryhard|extreme SEED=n cargo test -p engine_npc --test pairs
/// trace -- --ignored --nocapture`
#[test]
#[ignore]
fn trace() {
    let seed = std::env::var("SEED").map_or(1, |s| s.parse().unwrap());
    eprintln!("{:?}", fight(tier_env(), skills(), seed));
}

fn totals(tier: DifficultyTier, level: NpcSkills) -> Vec<Fight> {
    (1..=SEEDS).map(|s| fight(tier, level, s)).collect()
}

fn sum(fs: &[Fight], g: fn(&Fight) -> f32) -> f32 {
    fs.iter().map(g).sum()
}

#[test]
fn on_very_hard_soldiers_pair_up_one_holding_the_other_cutting_from_behind() {
    // (Soldiers: on their own they don't think to go round a foe. Veterans already circle to
    // its back on Hard.)
    let hard = totals(DifficultyTier::Hard, NpcSkills::soldier());
    let vh = totals(DifficultyTier::VeryHard, NpcSkills::soldier());
    eprintln!("hard {hard:#?}\nvery hard {vh:#?}");
    assert!(
        hard.iter().all(|f| !f.paired && f.flank_barks == 0),
        "{hard:?}"
    );
    assert!(vh.iter().all(|f| f.paired && f.flank_barks >= 1), "{vh:?}");
    // On opposite sides of the player most of the fight, and more cuts land, most from
    // behind it.
    let apart = |fs: &[Fight]| sum(fs, |f| f.apart_secs);
    assert!(apart(&vh) > SEEDS as f32 * SECS * 0.55, "{}", apart(&vh));
    assert!(
        apart(&vh) > 1.5 * apart(&hard),
        "{} vs {}",
        apart(&vh),
        apart(&hard)
    );
    let hits = |fs: &[Fight]| sum(fs, |f| f.hits as f32);
    assert!(hits(&vh) > hits(&hard), "{} vs {}", hits(&vh), hits(&hard));
    assert!(sum(&vh, |f| f.behind as f32) >= 0.4 * hits(&vh));
}

#[test]
fn on_extreme_the_holder_steps_in_with_a_feint_as_its_mate_is_placed_behind() {
    let vh = totals(DifficultyTier::VeryHard, NpcSkills::veteran());
    let x = totals(DifficultyTier::Extreme, NpcSkills::veteran());
    eprintln!("very hard {vh:#?}\nextreme {x:#?}");
    assert!(vh.iter().all(|f| f.baits == 0), "{vh:?}");
    assert!(x.iter().all(|f| f.baits >= 1), "{x:?}");
}
