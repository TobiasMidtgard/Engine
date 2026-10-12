//! Fighting: attacks are telegraphed with an `AttackEvent` before they land, so anyone can
//! react. NPCs notice incoming attacks, wait out their reaction time (on their own clock, so
//! slow motion slows them), then block, parry or dodge depending on their defense skill. A
//! parried attacker staggers and is open to a punish. Shots spread by accuracy, suppression and
//! movement, can be stopped by shields and cover, and send `ProjectileTrace`s so smoke parts
//! and everyone nearby hears them whiz by.

use engine_core::prelude::*;
use engine_world::animation::{AnimationTrigger, Ragdoll};
use engine_world::locomotion::{LocomotionMode, MoveIntent};
use engine_world::physics::{PhysicsBackend, PhysicsQueries, QueryFilter};
use engine_world::{
    AttackEvent, AttackKind, DamageEvent, DamageKind, Ground, NoiseEvent, Obscurance,
    ProjectileTrace,
};

use crate::agent::{
    AttackPhase, Defense, IncomingAttack, Npc, NpcAction, NpcBrain, NpcClock, NpcCombat,
    NpcSettings, NpcVitals, NpcWeapon, WeaponStyle,
};
use crate::features::ExplosiveProp;
use crate::learning::OpponentModels;
use crate::perception::{NpcMemory, Perceivables};
use crate::skills::NpcSkills;
use crate::squad::AttackTokens;
use crate::world_model::{TacticalWorld, physics_backend};

/// Deterministic 0..1 noise from an entity, a time and a salt (combat rolls).
pub(crate) fn roll(e: Entity, now: f64, salt: u64) -> f32 {
    let h = hash_u64(e.to_bits() ^ (now * 1000.0) as u64 ^ salt.wrapping_mul(0x9e37_79b9));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Start an attack: enter the windup, telegraph it to the world and trigger the animation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_attack(
    me: Entity,
    combat: &mut NpcCombat,
    weapon: &NpcWeapon,
    skills: &NpcSkills,
    clock: &NpcClock,
    target: Option<Entity>,
    aim: DVec3,
    heavy: bool,
    feint: bool,
    dash: bool,
    attacks: &mut MessageWriter<AttackEvent>,
    triggers: &mut MessageWriter<AnimationTrigger>,
) {
    let ranged = weapon.style == WeaponStyle::Ranged;
    let windup = if ranged {
        weapon.windup * (1.4 - 0.7 * skills.accuracy)
    } else {
        weapon.windup * if heavy { 1.5 } else { 1.0 }
    };
    combat.phase = AttackPhase::Windup {
        target,
        hit_at: clock.now + windup as f64,
        heavy,
        feint,
        aim,
    };
    combat.stats.attacks += 1;
    let rate = clock.rate.max(1e-3);
    attacks.write(AttackEvent {
        attacker: me,
        target,
        windup_secs: windup / rate,
        reach: if ranged { weapon.range } else { weapon.reach },
        kind: if ranged {
            AttackKind::Ranged
        } else if dash {
            AttackKind::Dash
        } else {
            AttackKind::Melee
        },
    });
    triggers.write(AnimationTrigger {
        entity: me,
        name: if ranged {
            "aim".into()
        } else if heavy {
            "attack_heavy".into()
        } else {
            "attack_light".into()
        },
    });
}

type DefenderItem<'a> = (
    Entity,
    &'a Transform,
    &'a NpcClock,
    &'a NpcSkills,
    &'a NpcWeapon,
    &'a NpcMemory,
    &'a mut NpcCombat,
    &'a NpcVitals,
);

/// Turn `AttackEvent`s into incoming attacks the targeted NPCs will react to.
pub(crate) fn notice_attacks(
    mut events: MessageReader<AttackEvent>,
    mut q: Query<DefenderItem>,
    seen: Res<Perceivables>,
    mut models: ResMut<OpponentModels>,
    time: Res<Time>,
) {
    for ev in events.read() {
        models
            .entry(ev.attacker)
            .record_attack(time.elapsed_secs_f64());
        let Some(attacker) = seen.get(ev.attacker).copied() else {
            continue;
        };
        for (me, t, clock, skills, weapon, memory, mut combat, vitals) in &mut q {
            if me == ev.attacker || vitals.downed {
                continue;
            }
            let to_me = t.translation - attacker.position;
            let dist = to_me.length() as f32;
            let aimed_at_me = ev.target == Some(me)
                || (ev.target.is_none()
                    && dist <= ev.reach + 1.0
                    && attacker
                        .forward
                        .dot(Vec3::new(to_me.x as f32, 0.0, to_me.z as f32).normalize_or_zero())
                        > 0.5);
            if !aimed_at_me {
                continue;
            }
            // You can only react to what you've noticed (or what's right in your face).
            let fwd = t.forward();
            let facing = Vec3::new(fwd.x, 0.0, fwd.z)
                .normalize_or_zero()
                .dot(-Vec3::new(to_me.x as f32, 0.0, to_me.z as f32).normalize_or_zero());
            let aware = memory.get(ev.attacker).map(|k| k.awareness).unwrap_or(0.0);
            if aware < 0.5 && !(dist < 3.0 && facing > 0.3) {
                continue;
            }
            let model = models.get(ev.attacker);
            let mut reaction = skills.reaction_time * (0.85 + 0.4 * roll(me, clock.now, 7));
            // Rhythm learned: the guard is already half up when the swing comes.
            if let Some(expected) = model.next_attack_expected()
                && (expected - time.elapsed_secs_f64()).abs() < 0.35
            {
                reaction *= 1.0 - 0.5 * skills.learning * model.confidence();
            }
            let lands = clock.now + (ev.windup_secs * clock.rate) as f64;
            combat.incoming.push(IncomingAttack {
                attacker: ev.attacker,
                lands_at: lands,
                react_at: clock.now + reaction as f64,
                ranged: matches!(ev.kind, AttackKind::Ranged | AttackKind::Spell),
                reach: ev.reach,
                handled: false,
            });
            let _ = weapon;
        }
    }
}

/// React to incoming attacks once the reaction time has passed.
pub(crate) fn defend(
    mut q: Query<(
        Entity,
        &NpcClock,
        &NpcSkills,
        &NpcWeapon,
        &mut NpcCombat,
        &NpcVitals,
        &mut NpcBrain,
    )>,
    mut triggers: MessageWriter<AnimationTrigger>,
) {
    for (me, clock, skills, weapon, mut combat, vitals, brain) in &mut q {
        let now = clock.now;
        combat.incoming.retain(|a| now < a.lands_at + 0.6);
        if vitals.downed {
            continue;
        }
        let staggered = combat.is_staggered(now);
        let mut response: Option<Defense> = None;
        let mut late = 0;
        for a in combat.incoming.iter_mut() {
            if a.handled || now < a.react_at {
                continue;
            }
            let left = (a.lands_at - now) as f32;
            // Skilled sword fighters often hold their guard for the last moment to parry.
            if !a.ranged
                && !staggered
                && left > 0.2
                && weapon.can_block
                && skills.defense > 0.6
                && roll(me, a.lands_at, 13) < skills.defense * 0.7
            {
                continue;
            }
            a.handled = true;
            if now >= a.lands_at {
                late += 1;
                continue;
            }
            if staggered {
                continue;
            }
            let r = roll(me, now, 11);
            if r > skills.defense {
                continue; // read it, but not well enough to do anything
            }
            let d = if a.ranged {
                if weapon.shield {
                    Defense::Block {
                        until: a.lands_at + 0.3,
                    }
                } else if r < skills.defense * 0.7 {
                    Defense::Dodge {
                        until: a.lands_at + 0.15,
                    }
                } else {
                    continue;
                }
            } else if weapon.can_block {
                // Masters wait for the last moment and deflect; others raise the guard.
                if left < 0.2 && skills.defense > 0.6 && r < skills.defense * 0.6 {
                    Defense::Parry {
                        until: a.lands_at + 0.08,
                    }
                } else if skills.agility > 1.1 && r < skills.defense * 0.25 {
                    Defense::Dodge {
                        until: a.lands_at + 0.2,
                    }
                } else {
                    Defense::Block {
                        until: a.lands_at + 0.35,
                    }
                }
            } else {
                Defense::Dodge {
                    until: a.lands_at + 0.2,
                }
            };
            response = Some(match (response, d) {
                (Some(Defense::Block { until: u0 }), Defense::Block { until: u1 }) => {
                    Defense::Block { until: u0.max(u1) }
                }
                (Some(prev), _) if !matches!(prev, Defense::None) => prev,
                _ => d,
            });
        }
        combat.stats.reacted_too_late += late;
        if let Some(d) = response {
            // Smart fighters abandon their own swing to defend; rookies trade blows.
            if let AttackPhase::Windup { hit_at, .. } = combat.phase
                && skills.logic > 0.4
                && hit_at - now > 0.12
            {
                combat.phase = AttackPhase::Ready;
            }
            if matches!(combat.phase, AttackPhase::Windup { .. }) {
                continue;
            }
            match d {
                Defense::Block { .. } => combat.stats.blocks += 1,
                Defense::Parry { .. } => combat.stats.parries += 1,
                Defense::Dodge { .. } => combat.stats.dodges += 1,
                Defense::None => {}
            }
            combat.defense = d;
            triggers.write(AnimationTrigger {
                entity: me,
                name: match d {
                    Defense::Dodge { .. } => "dash".into(),
                    _ => "block".into(),
                },
            });
            let _ = &brain;
        }
    }
}

/// One attack reaching its hit moment.
pub(crate) struct Strike {
    attacker: Entity,
    target: Option<Entity>,
    heavy: bool,
    weapon: NpcWeapon,
    accuracy: f32,
    suppression: f32,
    moving: bool,
    origin: DVec3,
    forward: Vec3,
    aim: DVec3,
}

type FighterItem<'a> = (
    Entity,
    &'a Transform,
    &'a NpcClock,
    &'a NpcSkills,
    &'a NpcWeapon,
    &'a mut NpcCombat,
    &'a mut NpcVitals,
    &'a MoveIntent,
);

/// Land (or cancel) attacks whose windup is over and send the results as world events.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_attacks(
    mut q: Query<FighterItem>,
    seen: Res<Perceivables>,
    props: Query<(Entity, &Transform, &ExplosiveProp)>,
    world: Res<TacticalWorld>,
    physics: Res<PhysicsQueries>,
    backend: Option<Res<PhysicsBackend>>,
    ground: Res<Ground>,
    obscurance: Res<Obscurance>,
    settings: Res<NpcSettings>,
    mut models: ResMut<OpponentModels>,
    mut tokens: ResMut<AttackTokens>,
    mut damage: MessageWriter<DamageEvent>,
    mut traces: MessageWriter<ProjectileTrace>,
    mut noise: MessageWriter<NoiseEvent>,
    mut strikes: Local<Vec<Strike>>,
) {
    strikes.clear();
    for (me, t, clock, skills, weapon, mut combat, _, intent) in &mut q {
        let now = clock.now;
        // Expire finished states.
        match combat.phase {
            AttackPhase::Recovery { until } | AttackPhase::Staggered { until } if now >= until => {
                combat.phase = AttackPhase::Ready;
            }
            _ => {}
        }
        if let Defense::Block { until } | Defense::Parry { until } | Defense::Dodge { until } =
            combat.defense
            && now >= until
        {
            combat.defense = Defense::None;
        }
        combat.suppression = (combat.suppression - 0.35 * clock.dt).max(0.0);
        let AttackPhase::Windup {
            target,
            hit_at,
            heavy,
            feint,
            aim,
        } = combat.phase
        else {
            continue;
        };
        if feint && now >= hit_at - 0.22 {
            combat.phase = AttackPhase::Ready;
            combat.ready_at = now + 0.15;
            combat.stats.feints += 1;
            continue;
        }
        if now < hit_at {
            continue;
        }
        let recovery = weapon.recovery * if heavy { 1.4 } else { 1.0 };
        combat.phase = AttackPhase::Recovery {
            until: now + recovery as f64,
        };
        combat.ready_at = now + weapon.cooldown as f64;
        tokens.release(me);
        let fwd = t.forward();
        strikes.push(Strike {
            attacker: me,
            target,
            heavy,
            weapon: *weapon,
            accuracy: skills.accuracy,
            suppression: combat.suppression,
            moving: intent.direction.length() > 0.3,
            origin: t.translation,
            forward: Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::NEG_Z),
            aim,
        });
    }
    if !settings.resolve_hits {
        return;
    }
    let phys = physics_backend(&backend, &physics);
    for s in strikes.drain(..) {
        match s.weapon.style {
            WeaponStyle::Melee => {
                resolve_melee(&s, &mut q, &seen, &mut models, &mut damage, &mut noise)
            }
            WeaponStyle::Ranged => resolve_shot(
                &s,
                &mut q,
                &seen,
                &props,
                &world,
                phys,
                &ground,
                &obscurance,
                &mut models,
                &mut damage,
                &mut traces,
                &mut noise,
            ),
        }
    }
}

fn resolve_melee(
    s: &Strike,
    q: &mut Query<FighterItem>,
    seen: &Perceivables,
    models: &mut OpponentModels,
    damage: &mut MessageWriter<DamageEvent>,
    noise: &mut MessageWriter<NoiseEvent>,
) {
    let Some(target) = s.target.and_then(|t| seen.get(t)).copied() else {
        return;
    };
    let to = target.position - s.origin;
    let flat = Vec3::new(to.x as f32, 0.0, to.z as f32);
    let dist = flat.length();
    let dir = flat.normalize_or(s.forward);
    let in_reach = dist <= s.weapon.reach + 0.45 && (to.y.abs() < 1.6);
    let in_arc = s.forward.dot(dir) > 0.25 || dist < 0.8;
    // The target faces the attacker?
    let faces = target.forward.dot(-dir) > 0.2;
    let side = target.forward.cross(Vec3::Y).dot(dir).signum();
    let mut outcome = "hit";
    if !(in_reach && in_arc) {
        outcome = "whiff";
    } else if let Ok((_, _, tclock, _, tweapon, mut tcombat, mut tvitals, _)) =
        q.get_mut(target.entity)
    {
        let now = tclock.now;
        match tcombat.defense {
            Defense::Dodge { until } if now < until => outcome = "dodged",
            Defense::Parry { until } if now < until && faces && tweapon.can_block => {
                outcome = "parried"
            }
            Defense::Block { until } | Defense::Parry { until }
                if now < until && faces && tweapon.can_block =>
            {
                outcome = "blocked";
                tvitals.poise -= if s.heavy { 0.55 } else { 0.12 };
                if tvitals.poise <= 0.0 {
                    // Guard broken by a heavy blow.
                    tvitals.poise = 1.0;
                    tcombat.defense = Defense::None;
                    tcombat.phase = AttackPhase::Staggered {
                        until: now + tvitals.stagger_secs as f64,
                    };
                }
            }
            _ => {}
        }
    } else if target.mode == LocomotionMode::Dash {
        outcome = "dodged";
    } else if target.blocking && faces {
        outcome = "blocked";
    }
    let model = models.entry(target.entity);
    if outcome != "whiff" {
        model.record_defense(
            outcome == "blocked" || outcome == "parried",
            outcome == "dodged",
            side,
        );
    }
    match outcome {
        "hit" => {
            if let Ok((_, _, _, _, _, mut c, _, _)) = q.get_mut(s.attacker) {
                c.stats.hits += 1;
            }
            let amount = s.weapon.damage * if s.heavy { 1.6 } else { 1.0 };
            damage.write(DamageEvent {
                target: target.entity,
                amount,
                kind: DamageKind::Slash,
                point: target.position + DVec3::Y * (target.height as f64 * 0.6),
                impulse: dir * amount * 3.0,
                source: Some(s.attacker),
            });
        }
        "parried" => {
            if let Ok((_, _, clock, _, _, mut c, _, _)) = q.get_mut(s.attacker) {
                c.stats.blocked_by_target += 1;
                c.phase = AttackPhase::Staggered {
                    until: clock.now + 0.8,
                };
            }
            noise.write(NoiseEvent {
                position: target.position + DVec3::Y * 1.2,
                radius: 25.0,
                source: Some(target.entity),
            });
        }
        "blocked" => {
            if let Ok((_, _, _, _, _, mut c, _, _)) = q.get_mut(s.attacker) {
                c.stats.blocked_by_target += 1;
            }
            noise.write(NoiseEvent {
                position: target.position + DVec3::Y * 1.2,
                radius: 20.0,
                source: Some(target.entity),
            });
        }
        "whiff" => {
            // Overextended: a longer recovery, open to a counter.
            if let Ok((_, _, clock, _, _, mut c, _, _)) = q.get_mut(s.attacker)
                && let AttackPhase::Recovery { until } = c.phase
            {
                c.phase = AttackPhase::Recovery {
                    until: until.max(clock.now) + 0.25,
                };
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_shot(
    s: &Strike,
    q: &mut Query<FighterItem>,
    seen: &Perceivables,
    props: &Query<(Entity, &Transform, &ExplosiveProp)>,
    world: &TacticalWorld,
    physics: Option<&PhysicsQueries>,
    ground: &Ground,
    obscurance: &Obscurance,
    models: &mut OpponentModels,
    damage: &mut MessageWriter<DamageEvent>,
    traces: &mut MessageWriter<ProjectileTrace>,
    noise: &mut MessageWriter<NoiseEvent>,
) {
    let eye = s.origin + DVec3::Y * 1.45 + (s.forward * 0.4).as_dvec3();
    let aim_dir = (s.aim - eye).as_vec3().normalize_or(s.forward);
    // Shooting into smoke is mostly guessing.
    let haze = obscurance.transmittance(eye, s.aim);
    let spread = (s.weapon.spread_deg * (1.0 - 0.9 * s.accuracy)).to_radians()
        * (1.0 + 1.5 * s.suppression)
        * if s.moving { 1.6 } else { 1.0 }
        * (1.0 + 3.0 * (1.0 - haze));
    // Random direction in a cone (deterministic per shot).
    let r1 = roll(s.attacker, s.origin.x + s.aim.z, 3);
    let r2 = roll(s.attacker, s.origin.z + s.aim.x, 5);
    let ang = spread * r1.sqrt();
    let rot = r2 * TAU;
    let side = aim_dir.any_orthonormal_vector();
    let up = aim_dir.cross(side);
    let dir = (aim_dir * ang.cos() + (side * rot.cos() + up * rot.sin()) * ang.sin()).normalize();
    let max_len = (s.weapon.range * 2.0).max(10.0);
    // What does the shot hit first: level geometry, the ground, the target, or a barrel?
    let ignore = [s.attacker];
    let mut block_t = match physics {
        Some(p) => p
            .raycast(
                eye,
                dir,
                max_len,
                &QueryFilter {
                    exclude: ignore.to_vec(),
                    ..Default::default()
                },
            )
            .map(|h| h.distance)
            .unwrap_or(max_len),
        None => world
            .raycast(eye, eye + (dir * max_len).as_dvec3())
            .map(|(t, _)| t)
            .unwrap_or(max_len),
    };
    // Ground.
    if dir.y < 0.0
        && let Some(g) = ground.height(eye)
    {
        let t = ((eye.y - g) as f32 / -dir.y).max(0.0);
        block_t = block_t.min(t);
    }
    let mut hit_entity: Option<(Entity, f32, bool)> = None;
    // Bodies (capsule test) — targets and anyone else in the line of fire.
    let mut near = Vec::new();
    let mid = eye + (dir * block_t * 0.5).as_dvec3();
    seen.near(mid, block_t as f64 * 0.5 + 1.0, &mut near);
    for &i in &near {
        let p = seen.list[i as usize];
        if p.entity == s.attacker {
            continue;
        }
        let a = p.position + DVec3::Y * 0.3;
        let b = p.position + DVec3::Y * (p.height as f64 - 0.25);
        let (t, d) = ray_segment(eye, dir, a, b);
        if d < 0.33 && t > 0.0 && t < block_t && hit_entity.is_none_or(|h| t < h.1) {
            hit_entity = Some((p.entity, t, false));
        }
    }
    for (pe, pt, prop) in props.iter() {
        if prop.detonated {
            continue;
        }
        let c = pt.translation + DVec3::Y * 0.5;
        let oc = (c - eye).as_vec3();
        let t = oc.dot(dir);
        if t > 0.0
            && t < block_t + 0.5
            && (oc - dir * t).length() < 0.5
            && hit_entity.is_none_or(|h| t < h.1)
        {
            hit_entity = Some((pe, t, true));
        }
    }
    let end_t = hit_entity.map(|h| h.1).unwrap_or(block_t);
    let end = eye + (dir * end_t).as_dvec3();
    traces.write(ProjectileTrace {
        from: eye,
        to: end,
        radius: 0.08,
        speed: s.weapon.projectile_speed,
        shooter: Some(s.attacker),
    });
    noise.write(NoiseEvent {
        position: eye,
        radius: 90.0,
        source: Some(s.attacker),
    });
    if let Ok((_, _, _, _, _, mut c, _, _)) = q.get_mut(s.attacker) {
        c.stats.shots += 1;
    }
    let Some((victim, _, is_prop)) = hit_entity else {
        if let Some(t) = s.target {
            models.entry(t).record_defense(false, false, 0.0);
        }
        return;
    };
    if is_prop {
        damage.write(DamageEvent {
            target: victim,
            amount: s.weapon.damage,
            kind: DamageKind::Ballistic,
            point: end,
            impulse: dir * 30.0,
            source: Some(s.attacker),
        });
        if s.target == Some(victim)
            && let Ok((_, _, _, _, _, mut c, _, _)) = q.get_mut(s.attacker)
        {
            c.stats.explosives_used += 1;
        }
        return;
    }
    // Shields stop shots from the front.
    if let Ok((_, vt, vclock, _, vweapon, vcombat, _, _)) = q.get(victim) {
        let now = vclock.now;
        let fwd = vt.forward();
        let faces = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero().dot(-dir) > 0.3;
        let blocked = vweapon.shield && faces && vcombat.is_blocking(now);
        let dodged = vcombat.is_dodging(now);
        if blocked || dodged {
            models.entry(victim).record_defense(blocked, dodged, 0.0);
            return;
        }
    }
    if Some(victim) == s.target {
        models.entry(victim).record_defense(false, false, 0.0);
        if let Ok((_, _, _, _, _, mut c, _, _)) = q.get_mut(s.attacker) {
            c.stats.hits += 1;
        }
    }
    damage.write(DamageEvent {
        target: victim,
        amount: s.weapon.damage,
        kind: DamageKind::Ballistic,
        point: end,
        impulse: dir * s.weapon.damage * 2.0,
        source: Some(s.attacker),
    });
}

/// Closest approach between a ray and a segment: (distance along the ray, gap).
fn ray_segment(o: DVec3, d: Vec3, a: DVec3, b: DVec3) -> (f32, f32) {
    let d = d.as_dvec3();
    let u = b - a;
    let w = o - a;
    let (aa, bb, cc, dd, ee) = (d.dot(d), d.dot(u), u.dot(u), d.dot(w), u.dot(w));
    let denom = aa * cc - bb * bb;
    let (mut sc, mut tc);
    if denom.abs() < 1e-9 {
        sc = 0.0;
        tc = if bb > cc { dd / bb } else { ee / cc };
    } else {
        sc = (bb * ee - cc * dd) / denom;
        tc = (aa * ee - bb * dd) / denom;
    }
    tc = tc.clamp(0.0, 1.0);
    // Re-project for the clamped segment parameter.
    let p_seg = a + u * tc;
    let _ = sc;
    sc = (p_seg - o).dot(d).max(0.0) / aa;
    let p_ray = o + d * sc;
    (sc as f32, p_ray.distance(p_seg) as f32)
}

/// Marks a downed NPC (health reached 0) and when it went down (NPC clock).
#[derive(Component, Debug, Clone, Copy)]
pub struct Downed {
    pub since: f64,
}

type VictimItem<'a> = (
    Entity,
    &'a NpcClock,
    &'a mut NpcVitals,
    &'a mut NpcCombat,
    &'a mut NpcBrain,
    &'a mut MoveIntent,
);

/// Apply `DamageEvent`s to NPC health and balance; down NPCs at 0 health.
pub(crate) fn apply_damage(
    mut commands: Commands,
    mut hurts: MessageReader<DamageEvent>,
    mut q: Query<VictimItem>,
    mut tokens: ResMut<AttackTokens>,
    mut triggers: MessageWriter<AnimationTrigger>,
) {
    for h in hurts.read() {
        let Ok((e, clock, mut vitals, mut combat, mut brain, mut intent)) = q.get_mut(h.target)
        else {
            continue;
        };
        if vitals.downed || h.amount <= 0.0 {
            continue;
        }
        let now = clock.now;
        vitals.health -= h.amount;
        combat.stats.hits_taken += 1;
        combat.last_hurt = now;
        vitals.poise -= h.amount / 60.0;
        if vitals.poise <= 0.0 {
            vitals.poise = 1.0;
            combat.phase = AttackPhase::Staggered {
                until: now + vitals.stagger_secs as f64,
            };
            combat.defense = Defense::None;
        } else if let AttackPhase::Windup { heavy: false, .. } = combat.phase
            && h.kind != DamageKind::Fire
        {
            // A light attack is interrupted by a hit.
            combat.phase = AttackPhase::Recovery { until: now + 0.3 };
        }
        triggers.write(AnimationTrigger {
            entity: e,
            name: "hit_react".into(),
        });
        if vitals.health <= 0.0 {
            vitals.health = 0.0;
            vitals.downed = true;
            brain.switch(NpcAction::Down, now);
            brain.destination = None;
            *intent = MoveIntent::default();
            tokens.release(e);
            commands
                .entity(e)
                .insert((Downed { since: now }, Ragdoll { weight: 1.0 }));
        }
    }
}

/// Bring downed NPCs back after `NpcSettings::revive_secs` (showcases keep fighting forever).
#[allow(clippy::type_complexity)]
pub(crate) fn revive(
    mut commands: Commands,
    settings: Res<NpcSettings>,
    mut q: Query<(
        Entity,
        &Npc,
        &NpcClock,
        &Downed,
        &mut NpcVitals,
        &mut NpcBrain,
        &mut NpcCombat,
        &mut NpcMemory,
        &mut Transform,
    )>,
) {
    if settings.revive_secs <= 0.0 {
        return;
    }
    for (e, npc, clock, downed, mut vitals, mut brain, mut combat, mut memory, mut t) in &mut q {
        if clock.now - downed.since < settings.revive_secs as f64 {
            continue;
        }
        vitals.health = vitals.max_health;
        vitals.downed = false;
        vitals.poise = 1.0;
        *combat = NpcCombat {
            stats: combat.stats,
            ..Default::default()
        };
        memory.known.clear();
        memory.stimulus = None;
        brain.switch(NpcAction::Idle, clock.now);
        brain.alert = Default::default();
        brain.target = None;
        if let Some(home) = npc.home {
            t.translation = home;
        }
        commands
            .entity(e)
            .remove::<Downed>()
            .insert(Ragdoll { weight: 0.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_segment_distance() {
        let (t, d) = ray_segment(
            DVec3::ZERO,
            Vec3::X,
            DVec3::new(5.0, -1.0, 0.5),
            DVec3::new(5.0, 1.0, 0.5),
        );
        assert!((t - 5.0).abs() < 1e-4 && (d - 0.5).abs() < 1e-4, "{t} {d}");
        let (_, d) = ray_segment(
            DVec3::ZERO,
            Vec3::X,
            DVec3::new(5.0, 2.0, 0.0),
            DVec3::new(5.0, 3.0, 0.0),
        );
        assert!((d - 2.0).abs() < 1e-4);
    }
}
