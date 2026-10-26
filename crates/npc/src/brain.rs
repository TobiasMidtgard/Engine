//! Deciding and acting. Every few tenths of a second an NPC scores what it could do (utility
//! AI: engage, take cover, flank, seek high ground, shoot an explosive, retreat, revive a
//! downed squad mate, search, investigate, patrol) from what it knows, its health, how pinned
//! down it is, its squad role and its skills, and keeps doing the winner for a while. Every
//! tick, a controller turns the current action into `MoveIntent` (the same input a player
//! gives): following paths, spacing and circling in melee, striking at openings,
//! dash-attacking when skilled, shooting from cover, crouching behind low walls, kneeling over
//! a downed mate, blocking and dodging.
//!
//! A flanker of a squad maneuver (see `squad`) bounds round to its spot off the target's side
//! (square to the way its cover faces, the same side as its mate) along a path that keeps out
//! of the target's sight, from hidden point to cover, holding at each while its mate moves; a
//! suppressor keeps firing at the cover the target was last seen behind, and lobs a grenade
//! (or smoke, with a mate near) at a target that keeps still there too long (see `grenade`). A shooter
//! too dull to plan (`RUSH_LOGIC`) rushes straight in. Openings the target shows (a recovery,
//! a reload, spent stamina), seen through the reaction delay, are struck or fired into.

use engine_core::prelude::*;
use engine_world::animation::AnimationTrigger;
use engine_world::locomotion::MoveIntent;
use engine_world::physics::{PhysicsBackend, PhysicsQueries};
use engine_world::{AttackEvent, FocusPoints, Ground, GroundWear, HeatField, Obscurance, Water};

use crate::agent::{
    AlertLevel, AttackPhase, Defense, Npc, NpcAction, NpcBrain, NpcClock, NpcCombat, NpcMotor,
    NpcOrder, NpcRole, NpcSettings, NpcVitals, NpcWeapon, WeaponStyle,
};
use crate::combat::{Downed, Lie, begin_attack};
use crate::faction::{Faction, Squad};
use crate::features::ExplosiveProp;
use crate::grenade;
use crate::learning::OpponentModels;
use crate::nav::{NavAgent, NavGrid, PathStatus};
use crate::perception::{Known, NpcMemory, Opening, Perceivables, Snapshot, segment_distance};
use crate::skills::NpcSkills;
use crate::smoke::SmokeClouds;
use crate::squad::{AttackTokens, CLEAR_RADIUS, ReviveClaims, SquadSettings};
use crate::tactics::{PositionQuery, PositionWeights, TacticsCtx, choose_position};
use crate::world_model::{Sight, TacticalWorld, physics_backend};

/// Everything the brain reads from the world.
#[derive(SystemParam)]
pub(crate) struct BrainWorld<'w, 's> {
    seen: Res<'w, Perceivables>,
    world: Res<'w, TacticalWorld>,
    nav: ResMut<'w, NavGrid>,
    ground: Res<'w, Ground>,
    water: Res<'w, Water>,
    /// Worn paths, which routes prefer (when some module provides them).
    wear: Option<Res<'w, GroundWear>>,
    heat: Res<'w, HeatField>,
    obscurance: Res<'w, Obscurance>,
    physics: Res<'w, PhysicsQueries>,
    backend: Option<Res<'w, PhysicsBackend>>,
    settings: Res<'w, NpcSettings>,
    squad: Res<'w, SquadSettings>,
    tokens: ResMut<'w, AttackTokens>,
    models: Res<'w, OpponentModels>,
    focus: Res<'w, FocusPoints>,
    time: Res<'w, Time>,
    smoke: ResMut<'w, SmokeClouds>,
    grenades: ResMut<'w, grenade::Grenades>,
    props: Query<'w, 's, (Entity, &'static Transform, &'static ExplosiveProp)>,
    attacks: MessageWriter<'w, AttackEvent>,
    triggers: MessageWriter<'w, AnimationTrigger>,
}

type BrainItem<'a> = (
    Entity,
    &'a Npc,
    &'a Transform,
    &'a NpcSkills,
    &'a NpcWeapon,
    &'a NpcVitals,
    &'a NpcMotor,
    &'a NpcClock,
    (&'a Faction, Option<&'a Squad>, Option<&'a Downed>),
    &'a mut NpcBrain,
    &'a mut NpcMemory,
    &'a mut NpcCombat,
    &'a mut NavAgent,
    &'a mut MoveIntent,
);

/// Where every NPC is and what it's after (so allies can spread out).
struct Claim {
    e: Entity,
    faction: Faction,
    squad: Option<Squad>,
    /// The squad's medic.
    medic: bool,
    pos: DVec3,
    dest: Option<DVec3>,
    target: Option<Entity>,
    /// Going round with the squad: where to (its flank spot, else its next stop).
    flank: Option<DVec3>,
    /// On its way to or kneeling over a downed mate.
    reviving: bool,
}

/// A downed NPC of a squad, as its squad mates see it.
#[derive(Debug, Clone, Copy)]
struct Fallen {
    e: Entity,
    squad: Squad,
    faction: Faction,
    /// Where it stood as it fell, and the turn of the frame it fell in (see [`Lie`]).
    pos: DVec3,
    frame: Quat,
    lie: Lie,
    /// How long it has been down (its own clock, s).
    down_for: f32,
    /// The squad's medic.
    medic: bool,
}

impl Fallen {
    /// A point of the frame it fell in, in the world.
    fn at(&self, p: Vec3) -> DVec3 {
        self.pos + (self.frame * p).as_dvec3()
    }

    /// About where its breastbone lies (the hands go there).
    fn chest(&self) -> DVec3 {
        self.at(self.lie.sternum())
    }
}

#[derive(Default)]
pub(crate) struct Scratch {
    claims: Vec<Claim>,
    fallen: Vec<Fallen>,
    path: Vec<DVec3>,
    spots: Vec<crate::tactics::Spot>,
    replans: u32,
    /// Spots picked this step (each weighs dozens of sight lines and a route or two).
    picks: u32,
}

/// Spot picks a step: the rest wait a step or two (a few hundredths of a second), so a squad
/// told to move at once doesn't weigh all its spots in a single step.
const PICKS_PER_STEP: u32 = 1;

/// Think (every few tenths of a second) and act (every tick) for every NPC.
pub(crate) fn think_and_act(
    mut q: Query<BrainItem, Without<crate::rest::Resting>>,
    mut w: BrainWorld,
    mut revives: ResMut<ReviveClaims>,
    mut scratch: Local<Scratch>,
    seeds: Query<&crate::seed::Seed>,
) {
    let s = &mut *scratch;
    s.claims.clear();
    s.fallen.clear();
    s.replans = 0;
    s.picks = 0;
    let world_now = w.time.elapsed_secs_f64();
    revives.expire(world_now);
    for (e, npc, t, _, _, vitals, _, clock, (faction, squad, downed), brain, ..) in q.iter() {
        if !vitals.downed {
            s.claims.push(Claim {
                e,
                faction: *faction,
                squad: squad.copied(),
                medic: npc.role == NpcRole::Medic,
                pos: t.translation,
                dest: brain.destination,
                target: brain.target,
                flank: (brain.role == "flank").then(|| {
                    brain
                        .flank_goal
                        .or(brain.destination)
                        .unwrap_or(t.translation)
                }),
                reviving: brain.action == NpcAction::Revive,
            });
        } else if let Some(&squad) = squad {
            let f = t.forward();
            let vary = seeds.get(e).map_or(0.0, |s| s.unit());
            let lie = Lie::of(vary, downed.and_then(|d| d.push));
            s.fallen.push(Fallen {
                e,
                squad,
                faction: *faction,
                pos: t.translation,
                frame: Quat::from_rotation_y((-f.x).atan2(-f.z) + lie.yaw),
                lie,
                down_for: downed.map_or(0.0, |d| (clock.now - d.since) as f32),
                medic: npc.role == NpcRole::Medic,
            });
        }
    }
    for (
        me,
        npc,
        t,
        skills,
        weapon,
        vitals,
        motor,
        clock,
        (faction, squad, _),
        mut brain,
        mut memory,
        mut combat,
        mut nav,
        mut intent,
    ) in &mut q
    {
        let now = clock.now;
        if !npc.active || vitals.downed || clock.dt <= 0.0 {
            // Inactive NPCs leave `MoveIntent` to game code (cutscenes, scripted moves), and
            // the revive they were going for to their mates.
            if vitals.downed {
                *intent = MoveIntent::default();
            } else if !npc.active && brain.revive.is_some() {
                stop_revive(
                    me,
                    now,
                    skills,
                    &mut brain,
                    &mut combat,
                    &mut revives,
                    false,
                );
                brain.switch(NpcAction::Idle, now);
            }
            continue;
        }
        let pos = t.translation;
        if brain.home.is_none() {
            brain.home = Some(npc.home.unwrap_or(pos));
            brain.rng_state = crate::seed::seed_of(&seeds, me) ^ 0x5eed;
            brain.strafe = if brain.random() < 0.5 { -1.0 } else { 1.0 };
        }
        let far =
            w.focus.points.is_empty() || w.focus.distance(pos) > w.settings.far_distance as f64;
        if now >= brain.next_think {
            let jitter = 0.8 + 0.4 * brain.random();
            let pace = w.settings.think_secs * (1.3 - 0.5 * skills.logic) * jitter;
            brain.next_think = now + (pace * if far { 6.0 } else { 1.0 }) as f64;
            let smoke = think(
                me,
                npc,
                pos,
                skills,
                weapon,
                vitals,
                (faction, squad),
                (now, world_now),
                &mut brain,
                &mut memory,
                &mut combat,
                &w,
                (&s.claims, &s.fallen),
                &mut revives,
            );
            match smoke {
                Some(Throw::Smoke(at)) => {
                    w.smoke.throw(at, world_now);
                    w.grenades.thrown_at(brain.target, world_now);
                }
                Some(Throw::Frag { at, target }) => {
                    let hand = pos + DVec3::Y * 1.7;
                    w.grenades.throw(hand, at, world_now, me, Some(target));
                }
                None => {}
            }
        }
        // Pick a spot for actions that need one.
        let positional = matches!(
            brain.action,
            NpcAction::TakeCover
                | NpcAction::Flank
                | NpcAction::SeekHighGround
                | NpcAction::Retreat
        ) || (brain.action == NpcAction::Engage
            && weapon.style == WeaponStyle::Ranged);
        if positional && now >= brain.next_position && s.picks < PICKS_PER_STEP {
            s.picks += 1;
            pick_spot(
                me, pos, skills, weapon, faction, now, &mut brain, &memory, &mut nav, &mut w, s,
            );
        }
        drive(
            me,
            pos,
            t,
            skills,
            weapon,
            motor,
            clock,
            world_now,
            &mut brain,
            &memory,
            &mut combat,
            &mut nav,
            &mut intent,
            &mut w,
            &mut revives,
            s,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn think(
    me: Entity,
    npc: &Npc,
    pos: DVec3,
    skills: &NpcSkills,
    weapon: &NpcWeapon,
    vitals: &NpcVitals,
    (faction, squad): (&Faction, Option<&Squad>),
    (now, world_now): (f64, f64),
    brain: &mut NpcBrain,
    memory: &mut NpcMemory,
    combat: &mut NpcCombat,
    w: &BrainWorld,
    (claims, fallen): (&[Claim], &[Fallen]),
    revives: &mut ReviveClaims,
) -> Option<Throw> {
    // Target.
    let ordered_target = match brain.order {
        Some(NpcOrder::Attack { target }) => Some(Entity::from_bits(target)),
        _ => None,
    };
    let threat: Option<Known> = ordered_target
        .and_then(|e| memory.get(e).cloned())
        .or_else(|| memory.best_threat(pos, 0.5).cloned());
    // Covering or screening the squad's medic at a downed mate: the enemy that threatens it
    // first.
    let threat = match brain.guard {
        Some(at) if ordered_target.is_none() => {
            guard_threat(me, brain.role, at, now, memory, w).or(threat)
        }
        _ => threat,
    };
    brain.target = threat.as_ref().map(|k| k.entity).or(ordered_target);

    let confirmed = threat.as_ref().is_some_and(|k| k.awareness >= 0.8);
    let visible = threat.as_ref().is_some_and(|k| k.visible);
    let lost_secs = threat
        .as_ref()
        .map(|k| (now - k.last_seen) as f32)
        .unwrap_or(f32::INFINITY);
    let dist = threat
        .as_ref()
        .map(|k| k.last_position.distance(pos) as f32)
        .unwrap_or(f32::INFINITY);
    let stimulus = memory.stimulus.filter(|s| now - s.time < 10.0);

    brain.alert = if confirmed && lost_secs < 6.0 {
        AlertLevel::Combat
    } else if threat.is_some() {
        AlertLevel::Searching
    } else if stimulus.is_some() {
        AlertLevel::Suspicious
    } else {
        AlertLevel::Relaxed
    };

    let ranged = weapon.style == WeaponStyle::Ranged;
    let health = vitals.fraction();
    let supp = combat.suppression;
    let allies_near = claims
        .iter()
        .filter(|c| c.e != me && c.faction == *faction && c.pos.distance(pos) < 15.0)
        .count();
    let enemies = memory.known.iter().filter(|k| k.awareness >= 0.8).count();
    let target_pos = threat.as_ref().map(|k| k.last_position).unwrap_or(pos);

    // Explosive next to the enemy, none of ours near it, and we can see it.
    brain.explosive = None;
    if ranged && confirmed && skills.logic >= 0.4 {
        let sight = Sight {
            world: &w.world,
            physics: physics_backend(&w.backend, &w.physics),
            ground: &w.ground,
            obscurance: Some(&w.obscurance),
        };
        for (pe, pt, prop) in w.props.iter() {
            if prop.detonated {
                continue;
            }
            let c = pt.translation;
            let near_enemy = memory
                .known
                .iter()
                .filter(|k| {
                    k.awareness >= 0.8 && k.last_position.distance(c) < (prop.radius * 0.8) as f64
                })
                .count();
            let near_friend = claims.iter().any(|cl| {
                cl.faction == *faction && cl.pos.distance(c) < (prop.radius + 1.0) as f64
            });
            let d = c.distance(pos) as f32;
            if near_enemy == 0 || near_friend || d > weapon.range || d < prop.radius + 1.0 {
                continue;
            }
            if sight.clear(pos + DVec3::Y * 1.5, c + DVec3::Y * 0.5, &[me, pe]) {
                brain.explosive = Some(pe);
                break;
            }
        }
    }
    let mut scores: Vec<(NpcAction, f32)> = Vec::with_capacity(12);
    let mut add = |a: NpcAction, v: f32| scores.push((a, v));
    add(NpcAction::Idle, 0.1);
    if brain.alert == AlertLevel::Relaxed {
        add(NpcAction::Patrol, 0.16);
    }
    if let Some(st) = stimulus
        && !confirmed
    {
        add(NpcAction::Investigate, 0.35 + 0.4 * st.strength);
    }
    if let Some(k) = &threat
        && !visible
        && lost_secs > 1.5
    {
        add(NpcAction::Search, 0.42 + 0.2 * k.awareness);
    }
    // Pinning an enemy in cover or going round it, the squad keeps at it for a while without
    // seeing it.
    let maneuver = matches!(brain.role, "suppress" | "flank") && lost_secs < MANEUVER_SECS;
    if confirmed && (visible || lost_secs < 2.0 || maneuver) {
        let mut engage = if ranged {
            0.55 + 0.1 * skills.aggression - supp * (1.0 - skills.courage) * 0.5
        } else {
            0.6 + 0.3 * skills.aggression
        };
        // A mate is reviving someone: hold here and keep the enemy busy (or stand between the
        // reviver and the enemy); or an enemy stands over a downed mate: drive it off. (Not
        // the one whose turn it is to revive: it goes.)
        let my_revive = fallen
            .iter()
            .any(|f| revives.reviver(f.e, world_now) == Some(me));
        if matches!(brain.role, "cover" | "screen" | "clear") && !my_revive {
            engage += 0.1 + 0.15 * skills.teamwork;
        }
        add(NpcAction::Engage, engage);
        // Cover: when pinned down, and smart shooters take it as soon as they're exposed.
        let exposed = visible && !brain.in_cover;
        let cover = if ranged {
            0.25 + 0.7 * supp * (1.2 - skills.courage)
                + if exposed { 0.35 * skills.logic } else { 0.0 }
        } else if dist > 6.0 && skills.logic > 0.4 {
            0.3 + 0.5 * supp * skills.logic
        } else {
            0.0
        };
        add(NpcAction::TakeCover, cover);
        if brain.role == "flank" {
            add(NpcAction::Flank, 0.55 + 0.45 * skills.logic);
        } else if ranged && skills.logic > 0.75 && allies_near == 0 && dist < 25.0 {
            // A lone smart shooter repositions to the enemy's side now and then.
            add(NpcAction::Flank, 0.35 * skills.logic);
        }
        if let Some(_hg) = brain.high_ground
            && pos.y < target_pos.y + 0.8
        {
            add(NpcAction::SeekHighGround, 0.5 + 0.3 * skills.logic);
        }
        if brain.explosive.is_some() {
            add(NpcAction::UseExplosive, 1.0);
        }
    }
    if confirmed && health < 0.4 {
        let outnumbered = if enemies > allies_near + 1 { 0.25 } else { 0.0 };
        add(
            NpcAction::Retreat,
            (0.4 - health) / 0.4 * (1.15 - skills.courage) + outnumbered * (1.0 - skills.courage),
        );
    }
    // A downed squad mate: bid for the revive; only the best placed of the squad scores it.
    let plan = squad.and_then(|&sq| {
        consider_revive(
            me,
            pos,
            sq,
            faction,
            skills,
            vitals,
            combat,
            (now, world_now),
            brain,
            memory,
            w,
            claims,
            fallen,
            revives,
        )
    });
    let mut bid = None;
    if let Some(p) = plan
        && p.score > 0.0
        && now >= brain.revive_retry
    {
        let mine = brain.action == NpcAction::Revive && brain.revive == Some(p.ally);
        let kneeling = mine && brain.revive_kneel > 0.0;
        let locked = kneeling || (mine && pos.distance(p.spot) < 3.0);
        // Once on its way it keeps to it, and once at it, it sees it through unless things get
        // bad.
        let score = p.score
            + if kneeling {
                0.2
            } else if mine {
                0.1
            } else {
                0.0
            };
        if revives.bid(p.ally, me, score, world_now, locked) {
            add(NpcAction::Revive, score);
            bid = Some(p);
        }
    }
    // The squad's medic throws smoke between the enemy and a downed mate's watched spot (and
    // waits, as any skilled fighter does, for it to hide the spot).
    let smoke = squad
        .filter(|_| npc.role == NpcRole::Medic && world_now >= brain.smoke_ready)
        .and_then(|&sq| smoke_target(me, pos, sq, faction, now, world_now, memory, w, fallen));
    if smoke.is_some() {
        brain.smoke_ready = world_now + SMOKE_EVERY_SECS;
    }
    let throw = smoke.map(Throw::Smoke).or_else(|| {
        flush(
            me,
            pos,
            skills,
            weapon,
            faction,
            (now, world_now),
            brain,
            memory,
            w,
            claims,
        )
    });
    if brain.order.is_some() {
        add(NpcAction::Obey, 2.0);
    }
    // Inertia: keep doing the current thing unless something is clearly better.
    let current = brain.action;
    let committed = now - brain.action_since < 0.6;
    let best = scores
        .iter()
        .map(|&(a, v)| (a, if a == current { v + 0.12 } else { v }))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0)
        .unwrap_or(NpcAction::Idle);
    let current_still_valid = scores.iter().any(|(a, v)| *a == current && *v > 0.0);
    if !(committed && current_still_valid) {
        brain.switch(best, now);
    }
    brain.scores = scores;
    match bid {
        Some(p) if brain.action == NpcAction::Revive => {
            if brain.revive != Some(p.ally) {
                brain.revive = Some(p.ally);
                brain.revive_kneel = 0.0;
                brain.revive_progress = 0.0;
                brain.revive_closest = f32::MAX;
                brain.revive_stalled = 0.0;
            }
            // The spot stays put once it kneels there.
            if brain.revive_kneel <= 0.0 || brain.destination.is_none() {
                brain.destination = Some(p.spot);
            }
            brain.destination_reason = "revive";
        }
        _ => {
            if let Some(ally) = brain.revive {
                // Something else won: the mate is up again, a better placed mate has it, or
                // it got too hot (an abort: it waits before trying again).
                let down = fallen.iter().any(|f| f.e == ally);
                let mine = revives.reviver(ally, world_now).is_none_or(|r| r == me);
                let aborted = down && mine && brain.order.is_none() && current == NpcAction::Revive;
                stop_revive(me, now, skills, brain, combat, revives, aborted);
            } else if bid.is_some() {
                // A bid it doesn't follow up is withdrawn.
                revives.release(me);
            }
        }
    }
    throw
}

/// Something thrown: a smoke canister to land at a spot, or a frag at an enemy.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Throw {
    Smoke(DVec3),
    Frag { at: DVec3, target: Entity },
}

/// Flushing a pinned enemy out: from this much logic a squad's suppressor throws a grenade
/// at an enemy that has kept still behind cover for [`FLUSH_SECS`] plus up to
/// [`FLUSH_SPREAD`] more the less logic it has, from [`THROW_MIN`] to [`THROW_RANGE`] m off;
/// a frag unless a mate is within [`FRAG_CLEAR`] m of the spot (then smoke, which blinds the
/// enemy while the flankers close in); one throw at an enemy every [`THROW_GAP`] s per
/// squad, and each thrower every [`THROW_EVERY`] s. Keeping still is moving less than
/// [`STILL_SLACK`] m.
const FLUSH_LOGIC: f32 = 0.4;
const FLUSH_SECS: f64 = 6.0;
const FLUSH_SPREAD: f64 = 8.0;
const THROW_MIN: f64 = 7.0;
const THROW_RANGE: f64 = 32.0;
const FRAG_CLEAR: f64 = grenade::FRAG_RADIUS as f64 + 2.0;
const THROW_GAP: f64 = 8.0;
const THROW_EVERY: f64 = 20.0;
const STILL_SLACK: f64 = 1.5;

/// A grenade or smoke at the enemy it is pinning, if it has sat still behind cover long
/// enough (see [`FLUSH_LOGIC`]). Keeps track of how long it has.
#[allow(clippy::too_many_arguments)]
fn flush(
    me: Entity,
    pos: DVec3,
    skills: &NpcSkills,
    weapon: &NpcWeapon,
    faction: &Faction,
    (now, world_now): (f64, f64),
    brain: &mut NpcBrain,
    memory: &NpcMemory,
    w: &BrainWorld,
    claims: &[Claim],
) -> Option<Throw> {
    let target = brain.target?;
    let k = memory.get(target).filter(|k| k.awareness >= 0.8)?;
    let still = brain
        .still
        .filter(|&(e, at, _)| e == target && at.distance(k.last_position) < STILL_SLACK);
    let (_, _, since) = *brain
        .still
        .insert(still.unwrap_or((target, k.last_position, now)));
    if weapon.style != WeaponStyle::Ranged
        || skills.logic < FLUSH_LOGIC
        || brain.role != "suppress"
        || world_now < brain.grenade_ready
        || now - since < FLUSH_SECS + FLUSH_SPREAD * (1.0 - skills.logic) as f64
        || now - k.last_seen > MANEUVER_SECS as f64
        || w.grenades.lately(target, world_now, THROW_GAP)
    {
        return None;
    }
    let flat = DVec2::new(k.last_position.x - pos.x, k.last_position.z - pos.z).length();
    if !(THROW_MIN..THROW_RANGE).contains(&flat) {
        return None;
    }
    let sight = Sight {
        world: &w.world,
        physics: physics_backend(&w.backend, &w.physics),
        ground: &w.ground,
        obscurance: Some(&w.obscurance),
    };
    // Only at an enemy behind cover; it lands just behind it, off by as much as the thrower is
    // a poor shot.
    let front = cover_front(&sight, k.last_position, &[me, target])?;
    let a = brain.random() * std::f32::consts::TAU;
    let off = (1.0 - skills.accuracy) * 2.0 * brain.random();
    let at = k.last_position - front.as_dvec3() * 0.6
        + DVec3::new(a.cos() as f64, 0.0, a.sin() as f64) * off as f64;
    brain.grenade_ready = world_now + THROW_EVERY;
    brain.still = Some((target, k.last_position, now));
    let mate_near = claims
        .iter()
        .any(|c| c.e != me && c.faction == *faction && c.pos.distance(at) < FRAG_CLEAR);
    Some(if mate_near {
        Throw::Smoke(at)
    } else {
        Throw::Frag { at, target }
    })
}

/// How far a medic throws a smoke canister (m), and how often it has one ready (s).
const SMOKE_RANGE: f64 = 35.0;
const SMOKE_EVERY_SECS: f64 = 14.0;

/// Where a medic throws smoke, if anywhere: between a downed mate of its squad within throwing
/// range (down a moment, its spot not hidden already) and the enemy with the clearest view of
/// a body kneeling over it, 2 m off the chest on the enemy's side.
#[allow(clippy::too_many_arguments)]
fn smoke_target(
    me: Entity,
    pos: DVec3,
    squad: Squad,
    faction: &Faction,
    now: f64,
    world_now: f64,
    memory: &NpcMemory,
    w: &BrainWorld,
    fallen: &[Fallen],
) -> Option<DVec3> {
    let sight = Sight {
        world: &w.world,
        physics: physics_backend(&w.backend, &w.physics),
        ground: &w.ground,
        obscurance: Some(&w.obscurance),
    };
    let flat = |a: DVec3, b: DVec3| DVec2::new(b.x - a.x, b.z - a.z);
    fallen
        .iter()
        .filter(|f| f.squad == squad && f.faction == *faction && f.e != me && f.down_for > 0.5)
        .filter(|f| f.chest().distance(pos) <= SMOKE_RANGE)
        .filter(|f| !w.smoke.covers(f.chest(), world_now))
        .filter_map(|f| {
            let kneeling = f.chest() + DVec3::Y * 0.9;
            memory
                .known
                .iter()
                .filter(|k| k.awareness >= 0.8 && now - k.last_seen < 3.0)
                .filter(|k| flat(f.chest(), k.last_position).length() < 45.0)
                .map(|k| {
                    let eye = k.last_position + DVec3::Y * 1.5;
                    (
                        sight.visibility(eye, kneeling, &[me, k.entity, f.e]),
                        k.last_position,
                    )
                })
                .filter(|(v, _)| *v > 0.3)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(v, from)| (v, f.chest(), from))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, chest, from)| {
            let d = flat(chest, from).normalize_or_zero() * 2.0;
            DVec3::new(chest.x + d.x, chest.y, chest.z + d.y)
        })
}

/// A revive it could go for: the downed squad mate, how much it wants to (see
/// [`revive_score`]), and where it would kneel.
#[derive(Debug, Clone, Copy)]
struct RevivePlan {
    ally: Entity,
    score: f32,
    spot: DVec3,
}

/// Weigh reviving each downed mate of its squad in range (see [`weigh_revive`]) that no
/// better placed mate is seeing to, and pick the one it most wants to (keeping to the one it
/// is already after unless another is clearly better).
#[allow(clippy::too_many_arguments)]
fn consider_revive(
    me: Entity,
    pos: DVec3,
    squad: Squad,
    faction: &Faction,
    skills: &NpcSkills,
    vitals: &NpcVitals,
    combat: &NpcCombat,
    (now, world_now): (f64, f64),
    brain: &NpcBrain,
    memory: &NpcMemory,
    w: &BrainWorld,
    claims: &[Claim],
    fallen: &[Fallen],
    revives: &ReviveClaims,
) -> Option<RevivePlan> {
    if w.settings.ally_revive_secs <= 0.0 || fallen.is_empty() {
        return None;
    }
    let range = revive_range(&w.settings, claims.iter().any(|c| c.e == me && c.medic));
    let mut best: Option<(f32, RevivePlan)> = None;
    for f in fallen.iter().filter(|f| {
        f.squad == squad && f.faction == *faction && f.chest().distance(pos) as f32 <= range
    }) {
        let score = weigh_revive(
            me, pos, f, faction, skills, vitals, combat, now, brain, memory, w, claims,
        );
        if score <= 0.0 || !revives.open_to(f.e, me, score, world_now) {
            continue;
        }
        let keep = if brain.revive == Some(f.e) { 0.1 } else { 0.0 };
        if best.is_none_or(|(b, _)| score + keep > b) {
            let spot = revive_spot(f, &w.world, &w.ground);
            best = Some((
                score + keep,
                RevivePlan {
                    ally: f.e,
                    score,
                    spot,
                },
            ));
        }
    }
    best.map(|(_, p)| p)
}

/// How much it wants to revive downed mate `f` (see [`revive_score`]): how far it is, how
/// exposed its spot is to the enemies it knows (by line of sight from where they were last
/// seen to a body kneeling there, through smoke; less for an enemy looking elsewhere, and less
/// for one its mates are shooting at, as far as it trusts them), how much fire it is under
/// itself, its health, and whether the squad is winning.
#[allow(clippy::too_many_arguments)]
fn weigh_revive(
    me: Entity,
    pos: DVec3,
    f: &Fallen,
    faction: &Faction,
    skills: &NpcSkills,
    vitals: &NpcVitals,
    combat: &NpcCombat,
    now: f64,
    brain: &NpcBrain,
    memory: &NpcMemory,
    w: &BrainWorld,
    claims: &[Claim],
) -> f32 {
    let chest = f.chest();
    let dist = chest.distance(pos) as f32;
    let sight = Sight {
        world: &w.world,
        physics: physics_backend(&w.backend, &w.physics),
        ground: &w.ground,
        obscurance: Some(&w.obscurance),
    };
    // A body kneeling over the chest: its head about this high.
    let kneeling = chest + DVec3::Y * 0.9;
    let (mut melee_close, mut enemies) = (false, 0usize);
    // The most dangerous view counts fully, the others less (they can't all aim at once).
    let (mut views, mut worst) = (0.0f32, 0.0f32);
    // Nobody starts with an enemy close enough to strike it or the ally; once kneeling, it
    // lets go only for one closer still.
    let going = brain.action == NpcAction::Revive && brain.revive == Some(f.e);
    let (strike_me, strike_ally) = if going { (3.0, 3.0) } else { (4.5, 4.0) };
    // (An enemy at the ally holds the medic off too, however far it is: running in at a
    // fight over the body only gets it hit on the way.)
    let medic = claims.iter().any(|c| c.e == me && c.medic);
    for k in &memory.known {
        let ago = now - k.last_seen;
        if k.awareness < 0.5 || ago > 6.0 {
            continue;
        }
        let (to_me, to_ally) = (
            k.last_position.distance(pos),
            k.last_position.distance(chest),
        );
        // (A shooter that close is a matter of what it sees, below, not of its reach.)
        let shooter = k.history.back().is_some_and(|o| o.shooter);
        if ago < 1.5 && !shooter && (to_me < strike_me || to_ally < strike_ally) {
            melee_close = true;
        }
        if k.awareness < 0.8 {
            continue;
        }
        enemies += 1;
        // In sight now counts fully, seen a moment ago half (it may have ducked).
        let fresh = if k.visible {
            1.0
        } else if ago < 2.0 {
            0.5
        } else {
            0.0
        };
        if fresh > 0.0 && to_ally < 45.0 {
            let eye = k.last_position + DVec3::Y * 1.5;
            let view = sight.visibility(eye, kneeling, &[me, k.entity, f.e]);
            // One aiming somewhere else has to turn and aim first.
            let to = (chest - k.last_position).as_vec3();
            let facing = k.history.back().map_or(1.0, |o| {
                o.forward
                    .dot(Vec3::new(to.x, 0.0, to.z).normalize_or_zero())
            });
            let aim = ((facing - 0.3) / 0.65).clamp(0.0, 1.0);
            let watching = 0.15 + 0.85 * aim * aim * (3.0 - 2.0 * aim);
            // One its mates are shooting at keeps its head down (if it trusts them to).
            let covered = claims
                .iter()
                .any(|c| c.e != me && c.faction == *faction && c.target == Some(k.entity));
            let pinned = if covered { 0.75 * skills.teamwork } else { 0.0 };
            let e = fresh * view * (1.0 - 0.5 * to_ally as f32 / 45.0) * watching * (1.0 - pinned);
            views += e;
            worst = worst.max(e);
        }
    }
    let exposure = worst + 0.4 * (views - worst);
    let around = claims
        .iter()
        .filter(|c| c.e != me && c.faction == *faction && c.pos.distance(pos) < 30.0)
        .count();
    let ours = around as f32 + 1.0;
    let theirs = enemies as f32;
    let hurt_lately = combat.last_hurt > 0.0 && now - combat.last_hurt < 2.0;
    // The squad's medic is up, in range of the ally and not much farther from it than this
    // one is: the others leave it to it.
    let medic_near = !medic
        && claims.iter().any(|c| {
            let d = c.pos.distance(chest) as f32;
            c.e != me
                && c.medic
                && c.faction == *faction
                && c.squad == Some(f.squad)
                && d <= revive_range(&w.settings, true)
                && d <= dist + MEDIC_LEAD
        });
    let case = ReviveCase {
        dist,
        range: revive_range(&w.settings, medic),
        down_for: f.down_for,
        exposure,
        under_fire: combat.suppression + if hurt_lately { 0.6 } else { 0.0 },
        health: vitals.fraction(),
        balance: (ours - theirs) / (ours + theirs).max(1.0),
        melee_close,
        medic,
        medic_near,
        ally_medic: f.medic,
        done: if going {
            brain.revive_progress / revive_secs(&w.settings, skills).max(0.1)
        } else {
            0.0
        },
    };
    revive_score(&case, skills).0
}

/// How much farther from a downed mate than another squad mate the medic may be for the other
/// to leave it to the medic (m).
const MEDIC_LEAD: f32 = 6.0;

/// For this long after a squad mate goes down (s), the others leave it to the squad's medic if
/// it is near enough to go.
const MEDIC_FIRST: f32 = 3.0;

/// How far a squad mate goes to revive a downed ally (m): `ally_revive_range`, half as far
/// again for the squad's medic.
fn revive_range(settings: &NpcSettings, medic: bool) -> f32 {
    settings.ally_revive_range * if medic { 1.5 } else { 1.0 }
}

/// The enemy that most threatens a squad mate kneeling (or about to) at `at`, for a mate
/// covering it (`"cover"`: of those seen a moment ago, the one with the clearest view of it,
/// nearer counting more) or screening it (`"screen"`: the nearest to it); or, for one driving
/// an enemy off a downed mate lying at `at` (`"clear"`), the nearest within reach of it.
fn guard_threat(
    me: Entity,
    role: &str,
    at: DVec3,
    now: f64,
    memory: &NpcMemory,
    w: &BrainWorld,
) -> Option<Known> {
    let fresh = |k: &&Known| k.awareness >= 0.8 && now - k.last_seen < 2.0;
    let flat = |p: DVec3| DVec2::new(p.x - at.x, p.z - at.z).length();
    match role {
        "clear" => memory
            .known
            .iter()
            .filter(fresh)
            .filter(|k| flat(k.last_position) < CLEAR_RADIUS + 1.0)
            .min_by(|a, b| flat(a.last_position).total_cmp(&flat(b.last_position)))
            .cloned(),
        "screen" => memory
            .known
            .iter()
            .filter(fresh)
            .filter(|k| flat(k.last_position) < 20.0)
            .min_by(|a, b| flat(a.last_position).total_cmp(&flat(b.last_position)))
            .cloned(),
        "cover" => {
            let sight = Sight {
                world: &w.world,
                physics: physics_backend(&w.backend, &w.physics),
                ground: &w.ground,
                obscurance: Some(&w.obscurance),
            };
            let kneeling = at + DVec3::Y * 0.9;
            memory
                .known
                .iter()
                .filter(fresh)
                .filter(|k| flat(k.last_position) < 45.0)
                .map(|k| {
                    let eye = k.last_position + DVec3::Y * 1.5;
                    let view = sight.visibility(eye, kneeling, &[me, k.entity]);
                    (view * (1.0 - 0.5 * flat(k.last_position) as f32 / 45.0), k)
                })
                .filter(|(v, _)| *v > 0.1)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, k)| k.clone())
        }
        _ => None,
    }
}

/// What a squad mate weighs before going to revive a downed ally (see [`revive_score`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ReviveCase {
    /// How far it is to the ally (m), and how far squad mates go at most (m).
    pub dist: f32,
    pub range: f32,
    /// How long the ally has been down (s).
    pub down_for: f32,
    /// How exposed the ally's spot is: the known enemies that can see a body kneeling there,
    /// each 0..1 by how clear the view is (less for one not seen this moment, or far off).
    pub exposure: f32,
    /// How much fire it is under itself (suppression 0..1, plus 0.6 for a recent hit).
    pub under_fire: f32,
    /// Its own health (0..1).
    pub health: f32,
    /// Who is winning: −1 (far outnumbered) .. 1 (the enemy is).
    pub balance: f32,
    /// An enemy is close enough to strike it or the ally.
    pub melee_close: bool,
    /// It is the squad's medic; or the squad's medic is up and in range of the ally.
    pub medic: bool,
    pub medic_near: bool,
    /// The ally is the squad's medic (the squad wants it back most).
    pub ally_medic: bool,
    /// How much of the revive it has done already (0..1): nearly done, it sees it through.
    pub done: f32,
}

/// How much a fighter with `skills` wants to revive a downed ally now (a utility to set against
/// its other actions; 0 = not at all), and how risky it judges it (0..1).
///
/// It has to notice the ally went down first (later with a slow reaction and little
/// teamwork). Teamwork, a squad that is winning and a short run make it keener, and a rookie
/// also dashes in on impulse just after it notices. Danger (the ally's spot in the enemy's
/// view, being under fire itself) counts for as much as its logic lets it see: a skilled
/// fighter holds off until its mates have the enemy's heads down or smoke screens the spot; a
/// rookie runs straight in. Badly hurt, a smart fighter leaves it to someone else. Nobody
/// kneels with an enemy close enough to strike. Once well into it, it sees it through unless
/// it gets really hot. The squad's medic watches for downs (it notices in half the time) and
/// wants it most; for the first seconds the others want it less while the medic is near
/// enough to go.
pub(crate) fn revive_score(c: &ReviveCase, s: &NpcSkills) -> (f32, f32) {
    if c.melee_close || c.range <= 0.0 || c.dist > c.range {
        return (0.0, 1.0);
    }
    let notice =
        (2.0 * s.reaction_time + 0.8 * (1.0 - s.teamwork)) * if c.medic { 0.5 } else { 1.0 };
    if c.down_for < notice {
        return (0.0, 0.0);
    }
    let since = c.down_for - notice;
    let impulse = 0.45 * (1.0 - s.logic) * (1.0 - (since / 4.0).clamp(0.0, 1.0));
    let desire = 1.0
        + 0.3 * s.teamwork
        + 0.12 * c.balance.clamp(-1.0, 1.0)
        + impulse
        + 0.6 * c.done.clamp(0.0, 1.0)
        + if c.medic { 0.6 } else { 0.0 }
        + if c.ally_medic { 0.4 } else { 0.0 };
    // The medic gets the first few seconds; after that anyone goes.
    let desire = if c.medic_near && !c.ally_medic && c.down_for < MEDIC_FIRST {
        desire * 0.6
    } else {
        desire
    };
    let near = 1.0 - 0.35 * (c.dist / c.range).powi(2);
    // The more it thinks, the more a watched spot weighs: a skilled fighter waits until the
    // watcher looks away, is pinned by its mates' fire or loses the spot in smoke (all of which
    // lower `exposure`); a rookie runs straight in.
    let watched = (0.55 + 0.6 * s.logic) * c.exposure.clamp(0.0, 2.0);
    let danger = watched + 0.7 * c.under_fire.clamp(0.0, 1.5);
    let caution = 0.2 + 1.1 * s.logic;
    let risk = (danger * caution).clamp(0.0, 1.0);
    let hurt = (1.0 - c.health / 0.4).max(0.0) * (0.2 + 0.6 * s.logic);
    ((desire * near * (1.0 - risk) - hurt).max(0.0), risk)
}

/// Where to kneel to revive `f`: where its knees go beside its chest (see [`Lie::kneel_by`]),
/// on the side of the arm along its body (clear of the arm flung out wide and of what it
/// flung), unless that is inside an obstacle or off an edge and the other side isn't.
fn revive_spot(f: &Fallen, world: &TacticalWorld, ground: &Ground) -> DVec3 {
    let feet = f.pos.y;
    let spot = |side: usize| {
        let p = f.at(f.lie.sternum() * Vec3::new(1.0, 0.0, 1.0) + f.lie.kneel_by(side));
        let h = world.surface_height(p, feet, 0.5, ground);
        let ok = (h - feet).abs() < 0.3 && !world.blocked_at(p, 0.35, 1.0, None);
        (DVec3::new(p.x, h, p.z), ok)
    };
    match (spot(f.lie.along()), spot(f.lie.flung)) {
        ((_, false), (p, true)) | ((p, _), _) => p,
    }
}

/// How far from where its knees go a reviver may kneel down (m, flat): as close as it gets
/// with someone standing on the spot (the body kneels over to it). Once down, it stays down
/// as long as it is within [`REVIVE_STAY`] of it.
pub(crate) const REVIVE_ARRIVE: f64 = 0.9;

/// How far from where its knees go a kneeling reviver may be pushed and stay down (m, flat;
/// the body kneels over to the spot from as far). Pushed farther, it gets up and goes back.
pub(crate) const REVIVE_STAY: f64 = 1.0;

/// How far it has left to go to `dest` along its path (m, flat).
fn way_left(pos: DVec3, dest: DVec3, nav: &NavAgent) -> f32 {
    let flat = |a: DVec3, b: DVec3| DVec2::new(b.x - a.x, b.z - a.z).length();
    let (mut at, mut left) = (pos, 0.0);
    for &p in &nav.path {
        left += flat(at, p);
        at = p;
    }
    (left + flat(at, dest)) as f32
}

/// How long a fighter with `skills` takes to revive a downed mate (s): `ally_revive_secs`, a
/// fifth quicker with teamwork 1, a fifth slower with 0.
pub(crate) fn revive_secs(settings: &NpcSettings, skills: &NpcSkills) -> f32 {
    settings.ally_revive_secs * (1.2 - 0.4 * skills.teamwork.clamp(0.0, 1.0))
}

/// Stop reviving: give up the claim and the progress. After an abort (`aborted`: it broke it
/// off while the mate is still down) it counts it and waits a while before trying again, the
/// skilled longer (they let things calm down first).
pub(crate) fn stop_revive(
    me: Entity,
    now: f64,
    skills: &NpcSkills,
    brain: &mut NpcBrain,
    combat: &mut NpcCombat,
    revives: &mut ReviveClaims,
    aborted: bool,
) {
    revives.release(me);
    brain.revive = None;
    if brain.revive_kneel > 0.0 {
        // Off its knees first, its weapon taken up again.
        brain.revive_rise_until = now + NpcBrain::REVIVE_STAND_SECS;
    }
    brain.revive_kneel = 0.0;
    brain.revive_progress = 0.0;
    if brain.destination_reason == "revive" {
        brain.destination = None;
        brain.destination_reason = "";
    }
    if aborted {
        combat.stats.revives_aborted += 1;
        brain.revive_retry = now + 0.75 + 1.5 * skills.logic as f64;
    }
}

#[allow(clippy::too_many_arguments)]
fn pick_spot(
    me: Entity,
    pos: DVec3,
    skills: &NpcSkills,
    weapon: &NpcWeapon,
    faction: &Faction,
    now: f64,
    brain: &mut NpcBrain,
    memory: &NpcMemory,
    nav: &mut NavAgent,
    w: &mut BrainWorld,
    s: &mut Scratch,
) {
    let Some(target) = brain.target else {
        return;
    };
    let Some(k) = memory.get(target) else {
        return;
    };
    let target_pos = k.last_position;
    let target_forward = k.history.back().map(|h| h.forward).unwrap_or_else(|| {
        Vec3::new(
            (pos.x - target_pos.x) as f32,
            0.0,
            (pos.z - target_pos.z) as f32,
        )
        .normalize_or(Vec3::Z)
    });
    // The medic fights from the back line, a little farther off than its weapon would have it.
    let back = if s.claims.iter().any(|c| c.e == me && c.medic) {
        3.0
    } else {
        0.0
    };
    // A rookie with a gun doesn't weigh positions: it rushes straight at the enemy.
    if brain.action == NpcAction::Engage
        && weapon.style == WeaponStyle::Ranged
        && skills.logic < RUSH_LOGIC
    {
        let mut ctx = w.nav.ctx(&w.world, &w.ground, &w.water);
        let back = DVec3::new(pos.x - target_pos.x, 0.0, pos.z - target_pos.z);
        let back = back.normalize_or(DVec3::Z) * RUSH_GAP.min(back.length());
        let spot = target_pos + back;
        brain.destination = Some(ctx.nearest_walkable(spot, 1.5).unwrap_or(spot));
        brain.destination_reason = "rush";
        brain.in_cover = false;
        brain.low_cover = false;
        brain.next_position = now + 1.0;
        return;
    }
    let going_round = brain.action == NpcAction::Flank && brain.role == "flank";
    let ideal = match brain.action {
        NpcAction::Retreat => weapon.ideal_range().max(8.0) * 2.2,
        NpcAction::Flank => weapon.ideal_range().clamp(6.0, 14.0),
        _ => weapon.ideal_range() + back,
    };
    let mut allies = Vec::new();
    let mut ally_angles = Vec::new();
    let mut flank_angles = Vec::new();
    for c in &s.claims {
        if c.e == me || c.faction != *faction {
            continue;
        }
        allies.push(c.pos);
        if let Some(d) = c.dest {
            allies.push(d);
        }
        if c.target != Some(target) {
            continue;
        }
        let angle = |p: DVec3| {
            Vec3::new(
                (p.x - target_pos.x) as f32,
                0.0,
                (p.z - target_pos.z) as f32,
            )
            .normalize_or_zero()
        };
        ally_angles.push(angle(c.dest.unwrap_or(c.pos)));
        // Going round a target in cover, a flanker weighs only its fellow flankers' ways
        // round (to go the same way unless the other side is clearly better).
        if let Some(p) = c.flank {
            flank_angles.push(angle(p));
        }
    }
    let other_threats: Vec<DVec3> = memory
        .known
        .iter()
        .filter(|o| o.entity != target && o.awareness >= 0.8)
        .map(|o| o.last_position)
        .take(4)
        .collect();
    let explosives: Vec<(DVec3, f32)> = w
        .props
        .iter()
        .filter(|(_, _, p)| !p.detonated)
        .map(|(_, t, p)| (t.translation, p.radius))
        .collect();
    let mut q = PositionQuery {
        me,
        from: pos,
        target,
        target_pos,
        target_forward,
        other_threats,
        allies,
        ally_angles,
        front: None,
        ideal_range: ideal,
        current_destination: brain.destination,
        logic: skills.logic,
        weights: if brain.role == "suppress" {
            // Pinning: a spot to fire on the enemy's cover from and stay at, not a way round
            // to a shot at the enemy itself (that's the flankers' job).
            let w = PositionWeights::for_action(brain.action, skills.logic);
            PositionWeights {
                flank: 0.0,
                pincer: 0.0,
                can_shoot: w.can_shoot * 0.3,
                travel: w.travel + 0.7,
                stay: w.stay + 1.5,
                ..w
            }
        } else {
            PositionWeights::for_action(brain.action, skills.logic)
        },
        explosives,
        search_radius: (ideal + 10.0).clamp(14.0, 32.0),
    };
    let BrainWorld {
        world,
        nav: grid,
        ground,
        water,
        heat,
        obscurance,
        physics,
        backend,
        ..
    } = w;
    let mut navctx = grid.ctx(world, ground, water);
    let mut ctx = TacticsCtx {
        sight: Sight {
            world,
            physics: physics_backend(backend, physics),
            ground,
            obscurance: Some(obscurance),
        },
        nav: &mut navctx,
        heat,
    };
    // A target behind cover is flanked from its side: off the side of its cover, on cover that
    // sees past it, the same side as the other flanker unless the other side is clearly
    // better, not round behind it. One in the open is simply gone round (pincer and flank).
    if going_round && let Some(front) = cover_front(&ctx.sight, target_pos, &[me, target]) {
        let w = q.weights;
        q.front = Some(front);
        q.ally_angles = flank_angles;
        q.weights = PositionWeights {
            flank: 0.0,
            pincer: -1.0,
            side: 3.0,
            can_shoot: w.can_shoot * 1.6,
            cover: w.cover + 0.3,
            ..w
        };
    }
    let chosen = choose_position(&mut ctx, &q, &mut s.path, &mut s.spots);
    // Remember high ground for later (it shows up among the spots when the weights ask for it).
    if brain.action == NpcAction::SeekHighGround && chosen.is_none_or(|c| c.reason != "high_ground")
    {
        brain.high_ground = None;
    }
    brain.spots.clear();
    brain.spots.extend_from_slice(&s.spots);
    let settle = match brain.action {
        NpcAction::Engage => 2.5,
        NpcAction::Flank => 3.0,
        _ => 2.0,
    } * (0.8 + 0.4 * brain.random() as f64);
    brain.next_position = now + settle;
    if let Some(spot) = chosen.filter(|_| going_round) {
        // Round to the flank with the squad: a way out of the enemy's sight, taken from cover
        // to cover in turns (see `bound`).
        let eye = target_pos + DVec3::Y * 1.6;
        let ignore = [me, target];
        let sight = Sight {
            world,
            physics: physics_backend(backend, physics),
            ground,
            obscurance: Some(obscurance),
        };
        let seen = |p: DVec3| sight.clear(eye, p + DVec3::Y * BODY_SEEN, &ignore);
        let mut ctx = grid.ctx(world, ground, water);
        let mut cost = |p: DVec3| {
            let h = heat.sample(p + DVec3::Y * 0.5);
            if h > 150.0 {
                f32::INFINITY
            } else {
                (h / 25.0).max(0.0) + if seen(p) { IN_SIGHT_COST } else { 0.0 }
            }
        };
        ctx.find_path(pos, spot.position, &mut cost, &mut s.path);
        if s.path.is_empty() {
            s.path.push(spot.position);
        }
        let moved = brain
            .flank_goal
            .is_none_or(|g| g.distance(spot.position) > 1.5);
        if moved {
            brain.in_position = false;
        }
        brain.flank_goal = Some(spot.position);
        brain.flank_route.clear();
        let mut from = pos;
        brain.flank_route.push(pos);
        for &p in &s.path {
            let n = (from.distance(p) / ROUTE_STEP).ceil().max(1.0) as usize;
            for k in 1..=n {
                brain.flank_route.push(from.lerp(p, k as f64 / n as f64));
            }
            from = p;
        }
        brain.destination_reason = "flank";
        brain.in_cover = spot.covered;
        brain.low_cover = spot.low_cover;
        if brain.in_position {
            brain.destination = Some(spot.position);
        } else if brain.holding {
            brain.destination = Some(pos);
        } else {
            next_hop(brain, nav, now, &sight, eye, &ignore);
        }
    } else if let Some(spot) = chosen {
        brain.destination = Some(spot.position);
        brain.destination_reason = match brain.action {
            NpcAction::TakeCover => "cover",
            NpcAction::Flank => "flank",
            NpcAction::SeekHighGround => "high ground",
            NpcAction::Retreat => "fall back",
            _ => spot.reason,
        };
        brain.in_cover = spot.covered;
        brain.low_cover = spot.low_cover;
        nav.path.clear();
        nav.path.extend_from_slice(&s.path);
        nav.goal = Some(spot.position);
        nav.status = Some(PathStatus::Complete);
        nav.planned_at = now;
    }
    // Look for high ground to remember (cheap scan, smart shooters only).
    if weapon.style == WeaponStyle::Ranged
        && skills.logic >= 0.5
        && brain.high_ground.is_none()
        && now >= brain.next_high_ground_check
    {
        brain.next_high_ground_check = now + 4.0;
        let mut nctx = grid.ctx(world, ground, water);
        let step = 2.0f64;
        // One route check a look (a hilltop of unreachable cells would otherwise search its
        // way round each one in a single step): each look starts the scan somewhere else.
        let start = (now * 37.0) as usize;
        for k in 0..17 * 17 {
            let c = (start + k) % (17 * 17);
            let (i, j) = ((c / 17) as f64 - 8.0, (c % 17) as f64 - 8.0);
            let p = target_pos + DVec3::new(i * step, 0.0, j * step);
            let (h, ok) = nctx.sample(DVec3::new(p.x, p.y + 50.0, p.z));
            if ok && h > target_pos.y + 0.9 && h < target_pos.y + 6.0 {
                let spot = DVec3::new(p.x, h, p.z);
                // Reachable?
                let status = nctx.find_path(pos, spot, &mut |_| 0.0, &mut s.path);
                if status == PathStatus::Complete {
                    brain.high_ground = Some(spot);
                }
                break;
            }
        }
    }
}

/// Which way the cover the enemy at `at` is behind faces (XZ, from it), if it is behind any:
/// the mean of the ways round it that are blocked within [`COVER_PROBE`] m at chest height.
fn cover_front(sight: &Sight, at: DVec3, ignore: &[Entity]) -> Option<Vec3> {
    let chest = at + DVec3::Y * 1.0;
    let mut sum = Vec3::ZERO;
    let mut blocked = 0;
    for k in 0..16 {
        let a = k as f32 / 16.0 * std::f32::consts::TAU;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        if !sight.clear(chest, chest + d.as_dvec3() * COVER_PROBE, ignore) {
            sum += d;
            blocked += 1;
        }
    }
    // Walled in all round (or nearly): no one way.
    (blocked > 0 && sum.length() > 0.25 * blocked as f32).then(|| sum.normalize())
}

/// How far round an enemy [`cover_front`] looks for its cover (m).
const COVER_PROBE: f64 = 2.5;

/// How long a squad keeps pinning an enemy in cover or going round it without seeing it (s).
const MANEUVER_SECS: f32 = 10.0;
/// Below this much logic a shooter rushes straight at its enemy (to this far from it, m).
const RUSH_LOGIC: f32 = 0.3;
const RUSH_GAP: f64 = 4.0;
/// Going round to a flank: a body is seen at this height (m); a stretch of the way the enemy
/// sees costs this many times its length more; the way is followed in steps this long (m).
const BODY_SEEN: f64 = 1.2;
const IN_SIGHT_COST: f32 = 4.0;
const ROUTE_STEP: f64 = 1.0;
/// A bound from cover to cover is at least this long (m), and a cover more than this far on
/// is taken before it (or, with no cover, a stop this far on); the flanker holds each cover
/// at least this long (s).
const HOP_MIN: f64 = 3.5;
const HOP_MAX: f64 = 9.0;
const HOLD_SECS: f64 = 0.6;

/// How long after it last saw its enemy a shooter keeps firing at where it was: the squad's
/// suppressor as long as the maneuver lasts, a flanker holding a cover (covering its mate) or
/// a shooter covering a revive a few seconds.
fn suppress_secs(brain: &NpcBrain) -> f64 {
    match brain.role {
        "suppress" => MANEUVER_SECS as f64,
        "cover" => 4.0,
        "flank" if brain.holding || !brain.bound => 4.0,
        _ => 0.0,
    }
}

/// The flanker's next bound along its way round: to the last spot out of the enemy's sight
/// before the way shows it (at least [`HOP_MIN`] on), to a cover once it is [`HOP_MAX`] on, or,
/// with no cover, a stop a little farther; the flank spot itself last.
fn next_hop(
    brain: &mut NpcBrain,
    nav: &mut NavAgent,
    now: f64,
    sight: &Sight,
    eye: DVec3,
    ignore: &[Entity],
) {
    let route = &brain.flank_route;
    if route.is_empty() {
        return;
    }
    let hidden = |p: DVec3| !sight.clear(eye, p + DVec3::Y * BODY_SEEN, ignore);
    let mut along = 0.0;
    let mut pick = route.len() - 1;
    let mut cover = None;
    for i in 1..route.len() {
        along += route[i - 1].distance(route[i]);
        if hidden(route[i]) {
            if along >= HOP_MIN {
                cover = Some(i);
            }
        } else if let Some(c) = cover {
            pick = c;
            break;
        }
        if along >= HOP_MAX
            && let Some(c) = cover
        {
            pick = c;
            break;
        }
        if along >= HOP_MAX * 1.6 {
            pick = i;
            break;
        }
    }
    nav.path.clear();
    nav.path.extend_from_slice(&route[1..=pick]);
    brain.flank_route.drain(..pick);
    let hop = brain.flank_route[0];
    brain.destination = Some(hop);
    nav.goal = Some(hop);
    nav.status = Some(PathStatus::Complete);
    nav.planned_at = now;
}

/// Flanking with the squad: it holds the cover it has reached while it isn't its turn
/// (crouched, covering the mate that moves) and a moment after, then runs on to the next cover
/// on its way round; at its flank spot it stays. Returns whether it is where it should be
/// now (holding, or there): it shoots from there.
#[allow(clippy::too_many_arguments)]
fn bound(
    me: Entity,
    pos: DVec3,
    known: Option<&Known>,
    now: f64,
    brain: &mut NpcBrain,
    nav: &mut NavAgent,
    w: &mut BrainWorld,
    s: &mut Scratch,
    out: &mut MoveIntent,
) -> bool {
    let Some(goal) = brain.flank_goal else {
        return true;
    };
    if brain.in_position {
        follow(pos, goal, JOG, nav, now, w, s, out);
        return true;
    }
    if !brain.bound || (brain.holding && now < brain.hold_until) {
        out.crouch = true;
        return true;
    }
    if brain.holding {
        // Its turn again: on to the next cover.
        brain.holding = false;
        if let (Some(k), Some(target)) = (known, brain.target) {
            let sight = Sight {
                world: &w.world,
                physics: physics_backend(&w.backend, &w.physics),
                ground: &w.ground,
                obscurance: Some(&w.obscurance),
            };
            let eye = k.last_position + DVec3::Y * 1.6;
            next_hop(brain, nav, now, &sight, eye, &[me, target]);
        }
    }
    let Some(dest) = brain.destination else {
        return true;
    };
    let far = DVec2::new(dest.x - pos.x, dest.z - pos.z).length() > 2.5;
    let there = follow(pos, dest, 1.0, nav, now, w, s, out);
    out.sprint = far;
    if there {
        if dest.distance(goal) < 1.0 {
            brain.in_position = true;
        } else {
            brain.holding = true;
            brain.hold_until = now + HOLD_SECS;
        }
    }
    there
}

/// A melee mate screening a kneeling medic stands this far from it toward the enemy (m, flat),
/// and goes for an enemy only once it is within [`SCREEN_ENGAGE`] of the medic.
const SCREEN_GAP: f64 = 2.0;
const SCREEN_ENGAGE: f64 = 5.0;

const WALK: f32 = 0.36;
const JOG: f32 = 0.7;
/// `logic × defense` above which a melee fighter waits out an enemy's swing to counter it
/// instead of trading blows (elites and masters; veterans still trade).
const COUNTER_FIGHTER: f32 = 0.6;

#[allow(clippy::too_many_arguments)]
fn drive(
    me: Entity,
    pos: DVec3,
    t: &Transform,
    skills: &NpcSkills,
    weapon: &NpcWeapon,
    motor: &NpcMotor,
    clock: &NpcClock,
    world_now: f64,
    brain: &mut NpcBrain,
    memory: &NpcMemory,
    combat: &mut NpcCombat,
    nav: &mut NavAgent,
    intent: &mut MoveIntent,
    w: &mut BrainWorld,
    revives: &mut ReviveClaims,
    s: &mut Scratch,
) {
    let now = clock.now;
    // Hit or knocked off balance on the way or kneeling: it lets go at once.
    if brain.action == NpcAction::Revive
        && (combat.last_hurt > brain.action_since || combat.is_staggered(now))
    {
        stop_revive(me, now, skills, brain, combat, revives, true);
        brain.switch(NpcAction::Idle, now);
        brain.next_think = now;
    }
    let mut out = MoveIntent::default();
    let fwd = {
        let f = t.forward();
        Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::NEG_Z)
    };
    let known = brain.target.and_then(|e| memory.get(e));
    // What it acts on: the enemy as it was one reaction time ago.
    let perceived: Option<Snapshot> =
        known.and_then(|k| k.perceived(now, skills.reaction_time).copied());
    let target_pos = perceived
        .map(|p| p.position)
        .or(known.map(|k| k.last_position));
    let to_target = target_pos.map(|p| {
        let v = Vec3::new((p.x - pos.x) as f32, 0.0, (p.z - pos.z) as f32);
        (v.normalize_or(fwd), v.length())
    });
    if let Some((d, _)) = to_target
        && brain.alert >= AlertLevel::Searching
    {
        out.look = Some(d);
    }

    // ------------------------------------------------------------ defense overrides
    let attacker_dir = combat
        .incoming
        .iter()
        .rev()
        .find_map(|a| w.seen.get(a.attacker))
        .map(|a| {
            Vec3::new(
                (a.position.x - pos.x) as f32,
                0.0,
                (a.position.z - pos.z) as f32,
            )
            .normalize_or(fwd)
        });
    if combat.is_staggered(now) {
        *intent = out;
        return;
    }
    // Read the swing and holding the guard back for a parry: don't start anything now.
    if combat
        .incoming
        .iter()
        .any(|a| !a.handled && now >= a.react_at && now < a.lands_at)
    {
        out.look = attacker_dir.or(out.look);
        *intent = out;
        return;
    }
    match combat.defense {
        Defense::Block { until } | Defense::Parry { until } if now < until => {
            out.block = true;
            if let Some(d) = attacker_dir {
                out.look = Some(d);
                out.direction = -d * 0.15;
            }
            *intent = out;
            return;
        }
        Defense::Dodge { until } if now < until => {
            if brain.dodge_issued != until {
                brain.dodge_issued = until;
                let d = attacker_dir.unwrap_or(fwd);
                let side =
                    Vec3::new(-d.z, 0.0, d.x) * if brain.random() < 0.5 { 1.0 } else { -1.0 };
                let back = -d * 0.4;
                let mut landing =
                    pos + ((side + back).normalize() * motor.dash_distance).as_dvec3();
                let mut ctx = w.nav.ctx(&w.world, &w.ground, &w.water);
                if !ctx.walkable(landing) {
                    landing = pos + ((-side + back).normalize() * motor.dash_distance).as_dvec3();
                }
                out.dash_to = Some(landing);
            }
            out.look = attacker_dir.or(out.look);
            *intent = out;
            return;
        }
        _ => {}
    }
    // Mid-attack: commit.
    if let AttackPhase::Windup { aim, .. } = combat.phase {
        let d = Vec3::new((aim.x - pos.x) as f32, 0.0, (aim.z - pos.z) as f32).normalize_or(fwd);
        out.look = Some(d);
        out.attack = Some(if weapon.style == WeaponStyle::Ranged {
            engine_world::AttackKind::Ranged
        } else {
            engine_world::AttackKind::Melee
        });
        if weapon.style == WeaponStyle::Melee {
            out.direction = d * 0.25;
        }
        *intent = out;
        return;
    }

    let ready = combat.phase == AttackPhase::Ready && now >= combat.ready_at;
    let explosive_target = brain
        .explosive
        .and_then(|e| w.props.get(e).ok().map(|(_, pt, _)| (e, pt.translation)));
    let mut stuck = false;
    match brain.action {
        NpcAction::Idle | NpcAction::Down => {}
        NpcAction::Patrol => {
            let home = brain.home.unwrap_or(pos);
            if (brain.destination.is_none() || brain.destination_reason != "patrol")
                && now >= brain.wait_until
            {
                let a = brain.random() * TAU;
                let r = 8.0 * brain.random().sqrt();
                let p = home + DVec3::new((a.cos() * r) as f64, 0.0, (a.sin() * r) as f64);
                brain.destination = Some(p);
                brain.destination_reason = "patrol";
            }
            if let Some(dest) = brain.destination
                && follow(pos, dest, WALK, nav, now, w, s, &mut out)
            {
                brain.destination = None;
                brain.wait_until = now + 2.0 + 3.0 * brain.random() as f64;
            }
        }
        NpcAction::Investigate => {
            if let Some(st) = memory.stimulus {
                brain.destination = Some(st.position);
                brain.destination_reason = "investigate";
                let d = Vec3::new(
                    (st.position.x - pos.x) as f32,
                    0.0,
                    (st.position.z - pos.z) as f32,
                );
                out.look = Some(d.normalize_or(fwd));
                if d.length() > 2.0 {
                    follow(pos, st.position, JOG, nav, now, w, s, &mut out);
                }
            }
        }
        NpcAction::Search => {
            if let Some(k) = known {
                let guess = k.predicted(now, skills.logic);
                if brain.destination_reason != "search" || brain.destination.is_none() {
                    brain.destination = Some(guess);
                    brain.destination_reason = "search";
                }
                if let Some(dest) = brain.destination
                    && follow(pos, dest, JOG, nav, now, w, s, &mut out)
                {
                    // Sweep nearby.
                    let a = brain.random() * TAU;
                    let r = 3.0 + 5.0 * brain.random();
                    brain.destination =
                        Some(guess + DVec3::new((a.cos() * r) as f64, 0.0, (a.sin() * r) as f64));
                }
            }
        }
        NpcAction::Engage if weapon.style == WeaponStyle::Melee => {
            // Screening a kneeling mate (the squad's medic most of all): it stands between it
            // and the enemy, and takes on only one that comes close to it.
            let screen = brain
                .guard
                .filter(|_| brain.role == "screen")
                .and_then(|g| {
                    let p = target_pos?;
                    let from = DVec2::new(p.x - g.x, p.z - g.z);
                    (from.length() > SCREEN_ENGAGE).then(|| {
                        let d = from.normalize_or_zero() * SCREEN_GAP;
                        DVec3::new(g.x + d.x, g.y, g.z + d.y)
                    })
                });
            if let Some(spot) = screen {
                if follow(pos, spot, 1.0, nav, now, w, s, &mut out) {
                    out.direction = Vec3::ZERO;
                }
                if let Some((d, _)) = to_target {
                    out.look = Some(d);
                }
            } else if let (Some(p), Some(k), Some((dir, dist))) = (perceived, known, to_target) {
                melee(
                    me, pos, fwd, dir, dist, &p, k, skills, weapon, motor, clock, world_now, ready,
                    brain, combat, &mut out, w,
                );
            } else if let Some(k) = known {
                follow(pos, k.last_position, 1.0, nav, now, w, s, &mut out);
            }
        }
        NpcAction::UseExplosive => {
            if let Some((e, at)) = explosive_target {
                let aim = at + DVec3::Y * 0.5;
                out.look = Some(
                    Vec3::new((at.x - pos.x) as f32, 0.0, (at.z - pos.z) as f32).normalize_or(fwd),
                );
                if ready {
                    begin_attack(
                        me,
                        combat,
                        weapon,
                        skills,
                        clock,
                        Some(e),
                        aim,
                        false,
                        false,
                        false,
                        &mut w.attacks,
                        &mut w.triggers,
                    );
                    brain.explosive = None;
                    brain.next_think = now;
                }
            }
        }
        NpcAction::Revive => {
            let ally = brain
                .revive
                .and_then(|a| s.fallen.iter().find(|f| f.e == a).copied());
            match (ally, brain.destination) {
                (Some(f), Some(_)) if !revives.renew(f.e, me, world_now) => {
                    // A better placed mate took it over.
                    stop_revive(me, now, skills, brain, combat, revives, false);
                    brain.switch(NpcAction::Idle, now);
                    brain.next_think = now;
                }
                (Some(f), Some(spot)) => {
                    let chest = f.chest();
                    let to_chest =
                        Vec3::new((chest.x - pos.x) as f32, 0.0, (chest.z - pos.z) as f32);
                    let gap = DVec2::new(spot.x - pos.x, spot.z - pos.z).length();
                    // (Not from a floor above or below it.)
                    let level = (spot.y - pos.y).abs() < 1.2;
                    // Kneeling already: it stays down (it doesn't shuffle after the spot)
                    // unless pushed off it; then it gets up and goes back (getting down again
                    // before its hands go back on).
                    let arrived = if brain.revive_kneel > 0.0 && gap < REVIVE_STAY && level {
                        nav.clear();
                        brain.revive_closest = 0.0;
                        brain.revive_stalled = 0.0;
                        true
                    } else {
                        brain.revive_kneel = 0.0;
                        if gap > 2.5 {
                            // Running to it, facing the way it goes.
                            out.look = None;
                        }
                        // Getting there along its path (round a wall, or down a ramp and back
                        // up, is getting there too): one whose way stops getting shorter is
                        // stuck. A longer way found counts from where it is.
                        let left = way_left(pos, spot, nav);
                        if left + 0.3 < brain.revive_closest {
                            brain.revive_closest = left;
                            brain.revive_stalled = 0.0;
                        } else {
                            if left > brain.revive_closest + 1.0 {
                                brain.revive_closest = left;
                            }
                            brain.revive_stalled += clock.dt;
                            stuck = brain.revive_stalled > 2.5;
                        }
                        // It kneels where it gets to (the body kneels over to the spot);
                        // close enough counts when it gets no closer (someone stands there).
                        let arrived = follow(pos, spot, 1.0, nav, now, w, s, &mut out)
                            || (gap < REVIVE_ARRIVE && level && brain.revive_stalled > 0.4);
                        // Every second counts: it sprints unless nearly there.
                        out.sprint = gap > 3.5;
                        arrived
                    };
                    if arrived {
                        out.direction = Vec3::ZERO;
                        out.sprint = false;
                        out.crouch = true;
                        out.look = Some(to_chest.normalize_or(fwd));
                        // Down on its knees, the hands on the chest; the revive counts from
                        // then.
                        brain.revive_kneel += clock.dt;
                        brain.revive_progress +=
                            (brain.revive_kneel - NpcBrain::REVIVE_KNEEL_SECS).clamp(0.0, clock.dt);
                    } else if gap < 2.5 {
                        out.look = Some(to_chest.normalize_or(fwd));
                    }
                }
                _ => brain.next_think = now,
            }
        }
        NpcAction::Obey => match brain.order {
            Some(NpcOrder::Hold { at }) | Some(NpcOrder::MoveTo { at }) => {
                let arrived = follow(pos, at, JOG, nav, now, w, s, &mut out);
                if arrived && matches!(brain.order, Some(NpcOrder::MoveTo { .. })) {
                    brain.order = None;
                }
                if let (Some(k), true) = (known, arrived) {
                    ranged_fire(
                        me,
                        pos,
                        k,
                        perceived.as_ref(),
                        skills,
                        weapon,
                        clock,
                        ready,
                        brain,
                        combat,
                        w,
                        &s.claims,
                        &mut out,
                    );
                }
            }
            Some(NpcOrder::Attack { .. }) | None => {
                brain.action = NpcAction::Engage;
            }
        },
        _ => {
            // Ranged engage, cover, flank, high ground, retreat: go to the chosen spot and shoot
            // when it makes sense.
            let urgency = match brain.action {
                NpcAction::TakeCover | NpcAction::Retreat => 1.0,
                NpcAction::Flank => 0.85,
                _ => JOG,
            };
            let mut arrived = true;
            let bounding = brain.action == NpcAction::Flank
                && brain.role == "flank"
                && brain.flank_goal.is_some();
            if bounding {
                arrived = bound(me, pos, known, now, brain, nav, w, s, &mut out);
            } else if let Some(dest) = brain.destination {
                arrived = follow(pos, dest, urgency, nav, now, w, s, &mut out);
                if !arrived && matches!(brain.action, NpcAction::TakeCover | NpcAction::Retreat) {
                    out.sprint = combat.suppression > 0.3 || brain.action == NpcAction::Retreat;
                }
            }
            if arrived && brain.low_cover && brain.in_cover {
                out.crouch = true;
            }
            if weapon.style == WeaponStyle::Melee {
                // Melee fighters taking cover from shooters close in when the shooter reloads.
                if let (Some(p), Some(k), Some((dir, dist))) = (perceived, known, to_target)
                    && (arrived || dist < 4.0)
                    && brain.action != NpcAction::Retreat
                {
                    melee(
                        me, pos, fwd, dir, dist, &p, k, skills, weapon, motor, clock, world_now,
                        ready, brain, combat, &mut out, w,
                    );
                }
            } else if let Some(k) = known {
                // Flankers keep quiet until they're in position (if they're smart).
                let quiet = brain.action == NpcAction::Flank && !arrived && skills.logic > 0.6;
                let on_the_move = !arrived && skills.accuracy < 0.7;
                // An enemy caught reloading or spent is worth a shot on the run.
                let opening = k.visible && k.opening(now, skills.reaction_time).is_some();
                if !quiet && (!on_the_move || opening) {
                    ranged_fire(
                        me,
                        pos,
                        k,
                        perceived.as_ref(),
                        skills,
                        weapon,
                        clock,
                        ready,
                        brain,
                        combat,
                        w,
                        &s.claims,
                        &mut out,
                    );
                }
            }
        }
    }

    if stuck {
        // It can't get to the spot (something in the way): it gives up for now.
        stop_revive(me, now, skills, brain, combat, revives, true);
        brain.switch(NpcAction::Idle, now);
        brain.next_think = now;
    }
    // Keep a little distance from allies (don't stack); one kneeling over a mate stays put.
    let kneeling = brain.action == NpcAction::Revive && brain.revive_kneel > 0.0;
    let mut push = Vec3::ZERO;
    for c in &s.claims {
        if c.e == me {
            continue;
        }
        let d = Vec3::new((pos.x - c.pos.x) as f32, 0.0, (pos.z - c.pos.z) as f32);
        let len = d.length();
        if len < 1.1 && len > 1e-4 {
            push += d / len * (1.1 - len);
        }
    }
    if push != Vec3::ZERO && out.dash_to.is_none() && !kneeling {
        out.direction = (out.direction + push).clamp_length_max(1.0);
    }
    if now < brain.revive_rise_until {
        // Getting up off its knees: it moves once it is up.
        out.direction = Vec3::ZERO;
        out.sprint = false;
        out.crouch = false;
        out.dash_to = None;
    }
    *intent = out;
}

/// Follow a path to `dest` at `speed` (fraction of run speed). Returns true on arrival.
#[allow(clippy::too_many_arguments)]
fn follow(
    pos: DVec3,
    dest: DVec3,
    speed: f32,
    nav: &mut NavAgent,
    now: f64,
    w: &mut BrainWorld,
    s: &mut Scratch,
    out: &mut MoveIntent,
) -> bool {
    let flat = DVec2::new(dest.x - pos.x, dest.z - pos.z).length();
    if flat < 0.6 && (dest.y - pos.y).abs() < 1.2 {
        nav.clear();
        return true;
    }
    let stale = nav.goal.is_none_or(|g| g.distance(dest) > 1.0)
        || (nav.path.is_empty() && flat > 0.8)
        || (nav.status == Some(PathStatus::Partial) && now - nav.planned_at > 1.5);
    if stale && s.replans < 8 {
        s.replans += 1;
        let mut ctx = w
            .nav
            .ctx(&w.world, &w.ground, &w.water)
            .worn(w.wear.as_deref());
        let heat = &w.heat;
        let mut cost = |p: DVec3| {
            let h = heat.sample(p + DVec3::Y * 0.5);
            if h > 150.0 {
                f32::INFINITY
            } else {
                (h / 25.0).max(0.0)
            }
        };
        let status = ctx.find_path(pos, dest, &mut cost, &mut nav.path);
        nav.status = Some(status);
        nav.goal = Some(dest);
        nav.planned_at = now;
        if status == PathStatus::Failed {
            nav.path.clear();
            nav.path.push(dest);
        }
    }
    if let Some(wp) = nav.next_waypoint(pos, 0.45) {
        let d = Vec3::new((wp.x - pos.x) as f32, 0.0, (wp.z - pos.z) as f32);
        let len = d.length();
        if len > 1e-3 {
            // Slow down for the last meter.
            let last = nav.path.len() <= 1;
            let scale = if last { (len / 1.0).min(1.0) } else { 1.0 };
            out.direction = d / len * speed * scale.max(0.35);
            if out.look.is_none() {
                out.look = Some(d / len);
            }
        }
    }
    false
}

/// Melee: keep the right distance, circle, take turns, strike at openings, punish mistakes.
#[allow(clippy::too_many_arguments)]
fn melee(
    me: Entity,
    pos: DVec3,
    fwd: Vec3,
    dir: Vec3,
    dist: f32,
    p: &Snapshot,
    k: &Known,
    skills: &NpcSkills,
    weapon: &NpcWeapon,
    motor: &NpcMotor,
    clock: &NpcClock,
    world_now: f64,
    ready: bool,
    brain: &mut NpcBrain,
    combat: &mut NpcCombat,
    out: &mut MoveIntent,
    w: &mut BrainWorld,
) {
    let now = clock.now;
    out.look = Some(dir);
    let reach = weapon.reach;
    let space = reach.max(2.0) + 0.7;
    let max_attackers = if skills.teamwork >= 0.3 {
        w.squad.max_melee_attackers
    } else {
        16
    };
    let has_turn = ready
        && w.tokens
            .request(k.entity, me, world_now, max_attackers, w.squad.token_secs);
    // An opening it has seen (as late as its reactions let it: a short one is over before a
    // slow fighter knows it was there). A long one (a reload, stamina spent) is worth running
    // in for.
    let seen = k.opening(now, skills.reaction_time);
    let opening = seen.is_some();
    let long = matches!(seen, Some(Opening::Reloading | Opening::Spent));
    if has_turn && now >= brain.next_commit_roll {
        brain.next_commit_roll = now + 0.5;
        if brain.random() < 0.25 + 0.6 * skills.aggression {
            brain.committed_until = now + 1.6;
        }
    }
    // Punish openings at once (as fast as reactions allow: `p` is already reaction-delayed).
    if ready && opening && (dist < reach + motor.dash_distance + 0.5 || (long && dist < 12.0)) {
        brain.committed_until = now + 1.0;
    }
    let committed = has_turn && now < brain.committed_until;
    // Elites and masters fight on the counter: never swing into an attack they can parry.
    // They let it come, parry or slip it, and punish the opening it leaves.
    let counter = p.attacking
        && !opening
        && weapon.can_block
        && skills.logic * skills.defense > COUNTER_FIGHTER;
    if (committed && !counter) || (ready && opening) {
        if dist > reach + 0.15 {
            let gap = dist - reach * 0.7;
            // (Into an opening, from any way out: the window is short.)
            if (opening || skills.agility >= 1.1)
                && skills.agility >= 1.0
                && (dist > reach + 0.8 || opening)
                && gap < motor.dash_distance
            {
                // Blink in and strike.
                out.dash_to = Some(pos + (dir * gap).as_dvec3());
                if opening {
                    combat.stats.punishes += 1;
                }
                begin_attack(
                    me,
                    combat,
                    weapon,
                    skills,
                    clock,
                    Some(k.entity),
                    p.position,
                    false,
                    false,
                    true,
                    &mut w.attacks,
                    &mut w.triggers,
                );
                brain.committed_until = 0.0;
            } else {
                out.direction = dir;
                out.sprint = dist > if long { 3.0 } else { 5.0 };
            }
        } else {
            let model = w.models.get(k.entity);
            let learned = skills.learning * model.confidence();
            let blocks_a_lot = model.block_rate * learned;
            let heavy = p.blocking && brain.random() < 0.25 + 0.6 * blocks_a_lot;
            let feint = !heavy
                && !opening
                && skills.logic > 0.45
                && brain.random() < 0.08 + 0.55 * blocks_a_lot;
            if opening {
                combat.stats.punishes += 1;
            }
            begin_attack(
                me,
                combat,
                weapon,
                skills,
                clock,
                Some(k.entity),
                p.position,
                heavy,
                feint,
                false,
                &mut w.attacks,
                &mut w.triggers,
            );
            brain.committed_until = 0.0;
        }
        return;
    }
    // Spacing: hover just outside the enemy's reach, circling; wait farther out without a turn.
    let others = w.tokens.holders(k.entity, world_now);
    let desired = space + if !has_turn && others > 0 { 1.4 } else { 0.0 };
    let mut radial = ((dist - desired) * 0.9).clamp(-1.0, 1.0);
    if p.attacking && dist < space + 0.3 && brain.random() < 0.4 + 0.5 * skills.logic {
        radial = -1.0; // step out of the swing
    }
    if now >= brain.next_strafe_flip {
        brain.next_strafe_flip = now + 1.2 + 2.5 * brain.random() as f64;
        brain.strafe = -brain.strafe;
        if skills.logic > 0.5 && brain.random() < skills.logic {
            // Circle toward the enemy's back.
            let tangent = Vec3::new(-dir.z, 0.0, dir.x);
            let from_target = -dir;
            let behind = |s: f32| {
                let v = (from_target + tangent * s * 0.3).normalize_or_zero();
                (1.0 - p.forward.dot(v)) * 0.5
            };
            brain.strafe = if behind(1.0) > behind(-1.0) {
                1.0
            } else {
                -1.0
            };
        }
    }
    let tangent = Vec3::new(-dir.z, 0.0, dir.x) * brain.strafe;
    let mut move_dir = dir * radial + tangent * 0.55;
    // Don't circle into walls.
    let ahead = pos + (move_dir.normalize_or_zero() * 1.0).as_dvec3();
    let mut ctx = w.nav.ctx(&w.world, &w.ground, &w.water);
    if !ctx.walkable(ahead) {
        brain.strafe = -brain.strafe;
        move_dir = dir * radial - tangent * 0.55;
    }
    out.direction = move_dir.clamp_length_max(1.0) * 0.6;
    let _ = fwd;
}

/// How close to its line of fire a fighter lets a mate reviving someone be (m).
const REVIVER_CLEARANCE: f64 = 1.2;

/// Shoot at the enemy when there's a shot (or suppress its last position for the squad).
#[allow(clippy::too_many_arguments)]
fn ranged_fire(
    me: Entity,
    pos: DVec3,
    k: &Known,
    perceived: Option<&Snapshot>,
    skills: &NpcSkills,
    weapon: &NpcWeapon,
    clock: &NpcClock,
    ready: bool,
    brain: &mut NpcBrain,
    combat: &mut NpcCombat,
    w: &mut BrainWorld,
    claims: &[Claim],
    out: &mut MoveIntent,
) {
    if !ready || weapon.style != WeaponStyle::Ranged {
        return;
    }
    let now = clock.now;
    let dist = k.last_position.distance(pos) as f32;
    if dist > weapon.range * 1.3 {
        return;
    }
    let aim = if k.visible {
        let p = perceived
            .map(|p| (p.position, p.velocity))
            .unwrap_or((k.last_position, k.last_velocity));
        // Lead moving targets (smart shooters).
        let flight = dist / weapon.projectile_speed.max(1.0) + weapon.windup;
        p.0 + DVec3::Y * 1.15 + (p.1 * flight * skills.logic).as_dvec3()
    } else if now - k.last_seen < suppress_secs(brain) {
        // Pin it behind its cover.
        k.last_position + DVec3::Y * 1.0
    } else {
        return;
    };
    // It doesn't fire past a mate reviving someone (if it has any teamwork): a stray shot
    // would make it let go.
    if skills.teamwork >= 0.3 {
        let muzzle = pos + DVec3::Y * 1.4;
        let mine = claims.iter().find(|c| c.e == me).map(|c| c.faction);
        let in_line = claims.iter().any(|c| {
            c.reviving
                && c.e != me
                && Some(c.faction) == mine
                && segment_distance(c.pos + DVec3::Y * 0.8, muzzle, aim) < REVIVER_CLEARANCE
        });
        if in_line {
            return;
        }
    }
    if k.visible && k.opening(now, skills.reaction_time).is_some() {
        combat.stats.punishes += 1;
    }
    let d =
        Vec3::new((aim.x - pos.x) as f32, 0.0, (aim.z - pos.z) as f32).normalize_or(Vec3::NEG_Z);
    out.look = Some(d);
    out.crouch = false;
    begin_attack(
        me,
        combat,
        weapon,
        skills,
        clock,
        Some(k.entity),
        aim,
        false,
        false,
        false,
        &mut w.attacks,
        &mut w.triggers,
    );
}
