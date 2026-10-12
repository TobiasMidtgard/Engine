//! Deciding and acting. Every few tenths of a second an NPC scores what it could do (utility
//! AI: engage, take cover, flank, seek high ground, shoot an explosive, retreat, search,
//! investigate, patrol) from what it knows, its health, how pinned down it is, its squad role
//! and its skills, and keeps doing the winner for a while. Every tick, a controller turns the
//! current action into `MoveIntent` (the same input a player gives): following paths, spacing
//! and circling in melee, striking at openings, dash-attacking when skilled, shooting from
//! cover, crouching behind low walls, blocking and dodging.

use engine_core::prelude::*;
use engine_world::animation::AnimationTrigger;
use engine_world::locomotion::MoveIntent;
use engine_world::physics::{PhysicsBackend, PhysicsQueries};
use engine_world::{AttackEvent, FocusPoints, Ground, HeatField, Water};

use crate::agent::{
    AlertLevel, AttackPhase, Defense, Npc, NpcAction, NpcBrain, NpcClock, NpcCombat, NpcMotor,
    NpcOrder, NpcSettings, NpcVitals, NpcWeapon, WeaponStyle,
};
use crate::combat::begin_attack;
use crate::faction::Faction;
use crate::features::ExplosiveProp;
use crate::learning::OpponentModels;
use crate::nav::{NavAgent, NavGrid, PathStatus};
use crate::perception::{Known, NpcMemory, Perceivables, Snapshot};
use crate::skills::NpcSkills;
use crate::squad::{AttackTokens, SquadSettings};
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
    heat: Res<'w, HeatField>,
    physics: Res<'w, PhysicsQueries>,
    backend: Option<Res<'w, PhysicsBackend>>,
    settings: Res<'w, NpcSettings>,
    squad: Res<'w, SquadSettings>,
    tokens: ResMut<'w, AttackTokens>,
    models: Res<'w, OpponentModels>,
    focus: Res<'w, FocusPoints>,
    time: Res<'w, Time>,
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
    &'a Faction,
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
    pos: DVec3,
    dest: Option<DVec3>,
    target: Option<Entity>,
}

#[derive(Default)]
pub(crate) struct Scratch {
    claims: Vec<Claim>,
    path: Vec<DVec3>,
    spots: Vec<crate::tactics::Spot>,
    replans: u32,
}

/// Think (every few tenths of a second) and act (every tick) for every NPC.
pub(crate) fn think_and_act(
    mut q: Query<BrainItem>,
    mut w: BrainWorld,
    mut scratch: Local<Scratch>,
) {
    let s = &mut *scratch;
    s.claims.clear();
    s.replans = 0;
    for (e, _, t, _, _, vitals, _, _, faction, brain, ..) in q.iter() {
        if !vitals.downed {
            s.claims.push(Claim {
                e,
                faction: *faction,
                pos: t.translation,
                dest: brain.destination,
                target: brain.target,
            });
        }
    }
    let world_now = w.time.elapsed_secs_f64();
    for (
        me,
        npc,
        t,
        skills,
        weapon,
        vitals,
        motor,
        clock,
        faction,
        mut brain,
        mut memory,
        mut combat,
        mut nav,
        mut intent,
    ) in &mut q
    {
        let now = clock.now;
        if !npc.active || vitals.downed || clock.dt <= 0.0 {
            // Inactive NPCs leave `MoveIntent` to game code (cutscenes, scripted moves).
            if vitals.downed {
                *intent = MoveIntent::default();
            }
            continue;
        }
        let pos = t.translation;
        if brain.home.is_none() {
            brain.home = Some(npc.home.unwrap_or(pos));
            brain.rng_state = me.to_bits() ^ 0x5eed;
            brain.strafe = if brain.random() < 0.5 { -1.0 } else { 1.0 };
        }
        let far =
            w.focus.points.is_empty() || w.focus.distance(pos) > w.settings.far_distance as f64;
        if now >= brain.next_think {
            let jitter = 0.8 + 0.4 * brain.random();
            let pace = w.settings.think_secs * (1.3 - 0.5 * skills.logic) * jitter;
            brain.next_think = now + (pace * if far { 6.0 } else { 1.0 }) as f64;
            think(
                me,
                npc,
                pos,
                skills,
                weapon,
                vitals,
                faction,
                now,
                &mut brain,
                &mut memory,
                &combat,
                &w,
                &s.claims,
            );
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
        if positional && now >= brain.next_position && s.replans < 4 {
            s.replans += 1;
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
    faction: &Faction,
    now: f64,
    brain: &mut NpcBrain,
    memory: &mut NpcMemory,
    combat: &NpcCombat,
    w: &BrainWorld,
    claims: &[Claim],
) {
    let _ = npc;
    // Target.
    let ordered_target = match brain.order {
        Some(NpcOrder::Attack { target }) => Some(Entity::from_bits(target)),
        _ => None,
    };
    let threat: Option<Known> = ordered_target
        .and_then(|e| memory.get(e).cloned())
        .or_else(|| memory.best_threat(pos, 0.5).cloned());
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
    if confirmed && (visible || lost_secs < 2.0) {
        let engage = if ranged {
            0.55 + 0.1 * skills.aggression - supp * (1.0 - skills.courage) * 0.5
        } else {
            0.6 + 0.3 * skills.aggression
        };
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
    let ideal = match brain.action {
        NpcAction::Retreat => weapon.ideal_range().max(8.0) * 2.2,
        NpcAction::Flank => weapon.ideal_range().clamp(6.0, 14.0),
        _ => weapon.ideal_range(),
    };
    let mut allies = Vec::new();
    let mut ally_angles = Vec::new();
    for c in &s.claims {
        if c.e == me || c.faction != *faction {
            continue;
        }
        allies.push(c.pos);
        if let Some(d) = c.dest {
            allies.push(d);
        }
        if c.target == Some(target) {
            let p = c.dest.unwrap_or(c.pos);
            ally_angles.push(
                Vec3::new(
                    (p.x - target_pos.x) as f32,
                    0.0,
                    (p.z - target_pos.z) as f32,
                )
                .normalize_or_zero(),
            );
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
    let q = PositionQuery {
        me,
        from: pos,
        target,
        target_pos,
        target_forward,
        other_threats,
        allies,
        ally_angles,
        ideal_range: ideal,
        current_destination: brain.destination,
        logic: skills.logic,
        weights: PositionWeights::for_action(brain.action, skills.logic),
        explosives,
        search_radius: (ideal + 10.0).clamp(14.0, 32.0),
    };
    let BrainWorld {
        world,
        nav: grid,
        ground,
        water,
        heat,
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
        },
        nav: &mut navctx,
        heat,
    };
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
    if let Some(spot) = chosen {
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
        'scan: for i in -8..=8 {
            for j in -8..=8 {
                let p = target_pos + DVec3::new(i as f64 * step, 0.0, j as f64 * step);
                let (h, ok) = nctx.sample(DVec3::new(p.x, p.y + 50.0, p.z));
                if ok && h > target_pos.y + 0.9 && h < target_pos.y + 6.0 {
                    let spot = DVec3::new(p.x, h, p.z);
                    // Reachable?
                    let status = nctx.find_path(pos, spot, &mut |_| 0.0, &mut s.path);
                    if status == PathStatus::Complete {
                        brain.high_ground = Some(spot);
                        break 'scan;
                    }
                }
            }
        }
    }
}

const WALK: f32 = 0.36;
const JOG: f32 = 0.7;

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
    s: &mut Scratch,
) {
    let now = clock.now;
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
            if let (Some(p), Some(k), Some((dir, dist))) = (perceived, known, to_target) {
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
            if let Some(dest) = brain.destination {
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
                if !quiet && !on_the_move {
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
                        &mut out,
                    );
                }
            }
        }
    }

    // Keep a little distance from allies (don't stack).
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
    if push != Vec3::ZERO && out.dash_to.is_none() {
        out.direction = (out.direction + push).clamp_length_max(1.0);
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
        let mut ctx = w.nav.ctx(&w.world, &w.ground, &w.water);
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
    let opening = p.vulnerable;
    if has_turn && now >= brain.next_commit_roll {
        brain.next_commit_roll = now + 0.5;
        if brain.random() < 0.25 + 0.6 * skills.aggression {
            brain.committed_until = now + 1.6;
        }
    }
    // Punish openings at once (as fast as reactions allow: `p` is already reaction-delayed).
    if ready && opening && dist < reach + motor.dash_distance + 0.5 {
        brain.committed_until = now + 1.0;
    }
    let committed = has_turn && now < brain.committed_until;
    if committed || (ready && opening) {
        if dist > reach + 0.15 {
            let gap = dist - reach * 0.7;
            if (opening || skills.agility >= 1.1)
                && skills.agility >= 1.0
                && dist > reach + 0.8
                && gap < motor.dash_distance
            {
                // Blink in and strike.
                out.dash_to = Some(pos + (dir * gap).as_dvec3());
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
                out.sprint = dist > 5.0;
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
    } else if brain.role == "suppress" && now - k.last_seen < 4.0 {
        // Pin it behind its cover.
        k.last_position + DVec3::Y * 1.0
    } else {
        return;
    };
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
