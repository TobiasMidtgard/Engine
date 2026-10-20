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
use crate::brain::revive_secs;
use crate::faction::{Faction, Squad};
use crate::features::ExplosiveProp;
use crate::learning::OpponentModels;
use crate::perception::{NpcMemory, Perceivable, Perceivables};
use crate::seed::{Seed, seed_of};
use crate::skills::NpcSkills;
use crate::squad::{AttackTokens, ReviveClaims};
use crate::world_model::{TacticalWorld, physics_backend};

/// Deterministic 0..1 noise from a body's [`Seed`], a time and a salt (combat rolls).
pub(crate) fn roll(seed: u64, now: f64, salt: u64) -> f32 {
    let h = hash_u64(seed ^ (now * 1000.0) as u64 ^ salt.wrapping_mul(0x9e37_79b9));
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
    seeds: Query<&Seed>,
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
            let mut reaction =
                skills.reaction_time * (0.85 + 0.4 * roll(seed_of(&seeds, me), clock.now, 7));
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
    seeds: Query<&Seed>,
) {
    for (me, clock, skills, weapon, mut combat, vitals, brain) in &mut q {
        let now = clock.now;
        combat.incoming.retain(|a| now < a.lands_at + 0.6);
        if vitals.downed {
            continue;
        }
        // Down on its knees by a mate (the weapon set down, the hands on the chest), or getting
        // up from there: it can neither block nor dodge.
        if brain.kneeling() || now < brain.revive_rise_until {
            if combat.defense != Defense::None {
                combat.defense = Defense::None;
            }
            continue;
        }
        let staggered = combat.is_staggered(now);
        let mut response: Option<Defense> = None;
        let mut late = 0;
        for a in combat.incoming.iter_mut() {
            if a.handled || now < a.react_at {
                continue;
            }
            // Still reeling: look again once the stagger is over (if it ends in time).
            if staggered && now < a.lands_at {
                continue;
            }
            let left = (a.lands_at - now) as f32;
            // Skilled sword fighters often hold their guard for the last moment to parry.
            if !a.ranged
                && !staggered
                && left > 0.2
                && weapon.can_block
                && skills.defense > 0.6
                && roll(seed_of(&seeds, me), a.lands_at, 13) < skills.defense * 0.7
            {
                continue;
            }
            a.handled = true;
            if now >= a.lands_at {
                late += 1;
                continue;
            }
            let r = roll(seed_of(&seeds, me), now, 11);
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
    /// The attacker's clock when it landed (seeds the shot's scatter).
    at: f64,
    /// The attacker's [`Seed`].
    seed: u64,
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
    seeds: Query<&Seed>,
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
            at: now,
            seed: seed_of(&seeds, me),
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

/// Where a landed cut strikes `target` (struck along `dir`): a heavy blow comes down on the head
/// and shoulders, a cut on the run drives into the trunk, and the rest land on the trunk or, about
/// one in four (picked by the swing's time, so a run repeats), low on the legs. The point is on
/// the side of the body the blade came from.
fn strike_point(s: &Strike, target: &Perceivable, dir: Vec3) -> DVec3 {
    let low = (s.at * 7.31).fract() < 0.27;
    let up = if s.heavy {
        0.9
    } else if s.moving || !low {
        0.6
    } else {
        0.33
    };
    let near_side = -dir * 0.18;
    target.position + DVec3::Y * (target.height * up) as f64 + near_side.as_dvec3()
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
                point: strike_point(s, &target, dir),
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
    // Random direction in a cone (deterministic per shot; seeded by when it is fired, so a
    // shooter standing still doesn't send every shot to the same spot).
    let r1 = roll(s.seed, s.at, 3);
    let r2 = roll(s.seed, s.at, 5);
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

/// Marks a downed NPC (health reached 0): when it went down (NPC clock), and which way the
/// blow that felled it knocked it, if it did (see [`Lie`]).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct Downed {
    pub since: f64,
    /// The turn (rad about Y off straight back, left positive) of the way the felling blow
    /// knocked it over, if it pushed it back or to a side.
    pub push: Option<f32>,
}

/// How a downed NPC lies, as its body falls and as its squad mates see it: on its back,
/// fallen along `yaw` (rad about Y off straight back from the way it faced, left positive),
/// the leg on side `bent` folded out to the side and the arm on side `flung` flung out wide (0
/// its left, 1 its right); the other leg lies straight, the other arm along the body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Lie {
    pub yaw: f32,
    pub bent: usize,
    pub flung: usize,
}

impl Lie {
    /// About where its breastbone lies, in the frame it fell in (on the ground between where
    /// its feet stood, its head toward +Z and its right toward +X): a little toward the side
    /// of the folded leg (the hips roll that way), and not quite as far round as the frame
    /// turns (the feet stay where they stood as the body goes over). How it stood when it fell
    /// moves it a few cm more.
    pub fn sternum(self) -> Vec3 {
        let bent = if self.bent == 0 { -1.0 } else { 1.0 };
        Vec3::new(
            -0.015 + 0.029 * bent - 0.13 * self.yaw,
            0.28,
            0.76 - 0.03 * self.yaw * self.yaw,
        )
    }

    /// How a body lies, knocked over along `push` (see [`Downed::push`]); with none, back and a
    /// little to one side. How it lands differs from one NPC to the next: `vary` is its
    /// [`Seed::unit`](crate::Seed::unit).
    pub fn of(vary: f32, push: Option<f32>) -> Lie {
        Lie {
            yaw: push.unwrap_or((vary - 0.5) * 0.8),
            bent: usize::from(vary > 0.5),
            flung: usize::from((vary * 7.0).fract() > 0.45),
        }
    }

    /// Which way a blow pushing along `push` (owner frame x, z) knocks a fighter over (see
    /// [`Downed::push`]): along the push if it pushes back or to a side, at most a little past
    /// square to the side.
    pub fn pushed(push: Vec2) -> Option<f32> {
        let p = push.normalize_or_zero();
        (p.y > -0.2 && p.length() > 0.1).then(|| p.x.atan2(p.y).clamp(-0.9, 0.9))
    }

    /// The side of the arm along its body (0 its left, 1 its right), where a squad mate
    /// kneels to revive it when it can (see [`Lie::kneel_by`]).
    pub fn along(self) -> usize {
        1 - self.flung
    }

    /// Where a squad mate's knees go to revive it from side `side` (0 its left, 1 its right),
    /// from its breastbone in the frame it fell in (flat; see [`Lie::sternum`]): beside its chest, outside the arm
    /// along its body (what that hand let go of lies by the thigh, clear of the knees); on the
    /// side of the arm flung out wide (and of what it flung), beside its belly, short of that
    /// arm. A mate kneels on the side of the arm along the body when it can.
    pub fn kneel_by(self, side: usize) -> Vec3 {
        let s = if side == 0 { -1.0 } else { 1.0 };
        let k = if side == self.flung {
            KNEEL_BY_BELLY
        } else {
            KNEEL_BY_ARM
        };
        Vec3::new(s * k.x, 0.0, k.y)
    }
}

/// Where the knees go from a downed NPC's breastbone (out to the side, toward its head; see
/// [`Lie::kneel_by`]): outside the arm along its body, or beside its belly below the arm flung
/// out wide.
const KNEEL_BY_ARM: Vec2 = Vec2::new(0.5, 0.08);
const KNEEL_BY_BELLY: Vec2 = Vec2::new(0.32, -0.2);

type VictimItem<'a> = (
    Entity,
    &'a NpcClock,
    &'a Transform,
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
    mut revives: ResMut<ReviveClaims>,
    mut triggers: MessageWriter<AnimationTrigger>,
) {
    for h in hurts.read() {
        let Ok((e, clock, t, mut vitals, mut combat, mut brain, mut intent)) = q.get_mut(h.target)
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
            brain.revive = None;
            brain.revive_kneel = 0.0;
            brain.revive_progress = 0.0;
            *intent = MoveIntent::default();
            tokens.release(e);
            revives.release(e);
            // Knocked over along the blow (the body falls that way; its mates kneel by it).
            let p = t.rotation.inverse() * h.impulse;
            let push = Lie::pushed(Vec2::new(p.x, p.z));
            commands
                .entity(e)
                .insert((Downed { since: now, push }, Ragdoll { weight: 1.0 }));
        }
    }
}

type ReviveItem<'a> = (
    Entity,
    &'a Npc,
    &'a NpcClock,
    &'a NpcSkills,
    (&'a Faction, Option<&'a Squad>, Option<&'a Downed>),
    &'a mut NpcVitals,
    &'a mut NpcBrain,
    &'a mut NpcCombat,
    &'a mut NpcMemory,
    &'a mut Transform,
);

/// What the revive pass saw of the NPCs this step (kept between steps to reuse the space).
#[derive(Default)]
pub(crate) struct ReviveScratch {
    /// Revives done: (the downed, its reviver).
    done: Vec<(Entity, Entity)>,
    /// Downed NPCs a squad mate is reviving (on its way or kneeling).
    tended: Vec<Entity>,
    /// Standing squad members: squad, faction, where.
    standing: Vec<(Squad, Faction, DVec3)>,
}

/// Get a downed NPC up where it lies with `health` (a share of its maximum): it keeps what it
/// knows and thinks again at once.
fn get_up(
    now: f64,
    health: f32,
    vitals: &mut NpcVitals,
    brain: &mut NpcBrain,
    combat: &mut NpcCombat,
) {
    vitals.health = (vitals.max_health * health.clamp(0.0, 1.0)).max(1.0);
    vitals.downed = false;
    vitals.poise = 1.0;
    *combat = NpcCombat {
        stats: combat.stats,
        ..Default::default()
    };
    brain.switch(NpcAction::Idle, now);
    brain.next_think = now;
    brain.destination = None;
    // Up off the ground before it kneels by another.
    brain.revive_retry = brain.revive_retry.max(now + NpcBrain::GET_UP_SECS);
}

/// Revives. A squad mate kneeling over a downed NPC for its revive time (see
/// `NpcSettings::ally_revive_secs`) gets it up where it lies with `ally_revive_health`. The
/// showcase timer `NpcSettings::revive_secs` is the fallback: it waits while a squad mate is
/// on the way or kneeling; when it runs out, an NPC with a standing squad mate within
/// `ally_revive_range` comes to and gets up where it lies (with `ally_revive_health`), and one
/// with nobody to revive it is sent back to its home at full health, as fresh (what it knew is
/// forgotten).
pub(crate) fn revive(
    mut commands: Commands,
    settings: Res<NpcSettings>,
    mut revives: ResMut<ReviveClaims>,
    mut q: Query<ReviveItem>,
    mut scratch: Local<ReviveScratch>,
) {
    let s = &mut *scratch;
    s.done.clear();
    s.tended.clear();
    s.standing.clear();
    // Squad revives off: nobody is being revived (those on their way give up at their next
    // think).
    let on = settings.ally_revive_secs > 0.0;
    for (e, npc, clock, skills, (faction, squad, _), vitals, brain, _, _, t) in q.iter() {
        if vitals.downed {
            continue;
        }
        if let Some(&squad) = squad {
            s.standing.push((squad, *faction, t.translation));
        }
        // (One that is not acting, handed to game code or stopped in time, revives nobody.)
        let Some(ally) = brain
            .revive
            .filter(|_| brain.action == NpcAction::Revive && on && npc.active && clock.dt > 0.0)
        else {
            continue;
        };
        if brain.revive_progress >= revive_secs(&settings, skills) {
            s.done.push((ally, e));
        } else {
            s.tended.push(ally);
        }
    }
    let health = settings.ally_revive_health;
    for &(ally, reviver) in &s.done {
        let mut up = false;
        if let Ok((_, _, clock, _, _, mut vitals, mut brain, mut combat, ..)) = q.get_mut(ally)
            && vitals.downed
        {
            get_up(clock.now, health, &mut vitals, &mut brain, &mut combat);
            commands
                .entity(ally)
                .remove::<Downed>()
                .insert(Ragdoll { weight: 0.0 });
            up = true;
        }
        if let Ok((_, _, clock, _, _, _, mut brain, mut combat, ..)) = q.get_mut(reviver) {
            let now = clock.now;
            brain.revive = None;
            brain.revive_kneel = 0.0;
            brain.revive_progress = 0.0;
            brain.destination = None;
            brain.switch(NpcAction::Idle, now);
            // Off its knees and its weapon taken up again before it thinks on.
            brain.revive_rise_until = now + NpcBrain::REVIVE_STAND_SECS;
            brain.next_think = now + NpcBrain::REVIVE_STAND_SECS;
            if up {
                combat.stats.revives += 1;
            }
        }
        revives.release(reviver);
    }
    if settings.revive_secs <= 0.0 {
        return;
    }
    for (
        e,
        npc,
        clock,
        _,
        (faction, squad, downed),
        mut vitals,
        mut brain,
        mut combat,
        mut memory,
        mut t,
    ) in &mut q
    {
        let Some(downed) = downed else {
            continue;
        };
        // A revive under way holds the timer off (a mate that stops getting closer gives up,
        // so this doesn't last).
        if !vitals.downed
            || clock.now - downed.since < settings.revive_secs as f64
            || s.tended.contains(&e)
        {
            continue;
        }
        let reach = settings.ally_revive_range as f64;
        let mate_near = settings.ally_revive_secs > 0.0
            && squad.is_some_and(|sq| {
                s.standing
                    .iter()
                    .any(|(q, f, p)| q == sq && f == faction && p.distance(t.translation) < reach)
            });
        if mate_near {
            get_up(clock.now, health, &mut vitals, &mut brain, &mut combat);
        } else {
            get_up(clock.now, 1.0, &mut vitals, &mut brain, &mut combat);
            memory.known.clear();
            memory.stimulus = None;
            brain.alert = Default::default();
            brain.target = None;
            if let Some(home) = npc.home {
                t.translation = home;
            }
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
    use crate::faction::Faction;

    #[test]
    fn a_reviver_on_its_knees_neither_blocks_nor_dodges() {
        use bevy_ecs::message::Messages;
        use bevy_ecs::system::RunSystemOnce;
        use bevy_ecs::world::World;
        // Shots coming at a skilled shield bearer, and at a skilled rifleman (who dodges some).
        for weapon in [NpcWeapon::sword_and_shield(), NpcWeapon::rifle()] {
            for kneeling in [false, true] {
                let mut defended = 0;
                for n in 0..12 {
                    let now = 1.0 + 0.37 * f64::from(n);
                    let mut world = World::new();
                    world.init_resource::<Messages<AnimationTrigger>>();
                    let attacker = world.spawn_empty().id();
                    let brain = NpcBrain {
                        action: if kneeling {
                            NpcAction::Revive
                        } else {
                            NpcAction::Engage
                        },
                        revive_kneel: if kneeling { 2.0 } else { 0.0 },
                        ..Default::default()
                    };
                    let e = world
                        .spawn((
                            NpcClock {
                                now,
                                dt: 1.0 / 60.0,
                                rate: 1.0,
                            },
                            NpcSkills {
                                defense: 1.0,
                                ..NpcSkills::master()
                            },
                            weapon,
                            NpcCombat {
                                incoming: vec![IncomingAttack {
                                    attacker,
                                    lands_at: now + 0.5,
                                    react_at: now - 0.1,
                                    ranged: true,
                                    reach: 40.0,
                                    handled: false,
                                }],
                                ..Default::default()
                            },
                            NpcVitals::default(),
                            brain,
                        ))
                        .id();
                    world.run_system_once(defend).unwrap();
                    let c = world.get::<NpcCombat>(e).unwrap();
                    if c.defense != Defense::None {
                        defended += 1;
                    }
                }
                let case = format!("{:?} kneeling {kneeling}", weapon.style);
                if kneeling {
                    assert_eq!(defended, 0, "{case}");
                } else {
                    assert!(defended > 0, "{case}");
                }
            }
        }
    }

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

    /// Landed cuts strike high, mid and low (not one fixed point), from the attacker's side.
    #[test]
    fn cuts_land_high_mid_and_low() {
        let target = Perceivable {
            entity: Entity::PLACEHOLDER,
            faction: Faction(1),
            position: DVec3::new(0.0, 2.0, 0.0),
            forward: Vec3::Z,
            velocity: Vec3::ZERO,
            height: 1.8,
            mode: LocomotionMode::Walk,
            blocking: false,
            attacking: false,
            vulnerable: false,
            stamina: 1.0,
            reloading: false,
        };
        let swing = |at: f64, heavy: bool, moving: bool| Strike {
            attacker: Entity::PLACEHOLDER,
            target: Some(target.entity),
            heavy,
            weapon: NpcWeapon::sword(),
            accuracy: 1.0,
            suppression: 0.0,
            moving,
            origin: DVec3::new(0.0, 2.0, 1.5),
            forward: -Vec3::Z,
            aim: target.position,
            at,
            seed: 0,
        };
        let dir = -Vec3::Z;
        let (mut head, mut body, mut legs) = (0, 0, 0);
        for i in 0..200 {
            let at = 3.0 + i as f64 * 0.37;
            let p = strike_point(&swing(at, false, false), &target, dir) - target.position;
            assert!(p.z > 0.1, "a cut lands on the near side: {p}");
            match p.y {
                y if y < 0.85 => legs += 1,
                y if y > 1.45 => head += 1,
                _ => body += 1,
            }
            let heavy = strike_point(&swing(at, true, false), &target, dir) - target.position;
            assert!(heavy.y > 1.45, "a heavy blow comes down high: {heavy}");
            let run = strike_point(&swing(at, false, true), &target, dir) - target.position;
            assert!(
                (0.85..1.45).contains(&run.y),
                "a cut on the run takes the trunk: {run}"
            );
        }
        assert_eq!(head, 0, "light cuts land below the head");
        assert!(
            body > 120 && legs > 25,
            "light cuts mix trunk and legs: {body} {legs}"
        );
    }
}
