//! What happens where a spell lands, applied in one place and in a fixed order (Overwatch's
//! Contact pattern, research-architecture.md): for each hit, element and reaction first, then
//! damage, statuses, force, heal, ward, lifesteal, and last the stages it fires.
//!
//! Stages fired here run at the next depth; nothing past [`MAX_STAGE_DEPTH`] fires stages of
//! its own, and what a reaction or a meeting of spells makes never reacts again, so no chain
//! of detonations can run away.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{
    ApplyImpulse, BodyKind, CharacterController, Collider, ColliderShape, PhysicsQueries,
    QueryFilter, RigidBody,
};
use engine_world::{
    Combustible, DamageEvent, Died, Faction, Factions, FreezeRequest, Gravity, Ground, GrowthPulse,
    IgniteRequest, MindEffect, MindKind, NoiseEvent, SmokeEmission, WorldImpulse,
};

use crate::caster::{MagicCaster, Vitality};
use crate::combine::Recipe;
use crate::compile::{CastContext, Spell, compile};
use crate::design::{CastType, SpellDesign, TargetFilter, Trigger};
use crate::effects::ForceDir;
use crate::looks::{LookCmd, Role};
use crate::runtime::{
    CastInfo, ChainArc, CreatureFilter, HitQueue, MAX_STAGE_DEPTH, MagicQueue, PendingHit, Spawn,
    release,
};
use crate::schools::{Element, Mastery, Power, Status};
use crate::status::{Afflictions, StatusChange};

/// A spell reached something (for readouts, tests, AI and sound).
#[derive(Message, Debug, Clone)]
pub struct SpellHit {
    pub spell: Arc<Spell>,
    pub caster: Option<Entity>,
    pub target: Option<Entity>,
    pub point: DVec3,
    /// Damage after reactions and wards.
    pub damage: f32,
    /// The recipe a reaction made (`steam`).
    pub reaction: Option<&'static str>,
    pub status: Option<StatusChange>,
    pub shattered: bool,
    pub killed: bool,
}

/// Every message a hit can send.
#[derive(SystemParam)]
pub struct HitWriters<'w> {
    damage: MessageWriter<'w, DamageEvent>,
    impulse: MessageWriter<'w, ApplyImpulse>,
    blast: MessageWriter<'w, WorldImpulse>,
    ignite: MessageWriter<'w, IgniteRequest>,
    smoke: MessageWriter<'w, SmokeEmission>,
    noise: MessageWriter<'w, NoiseEvent>,
    hit: MessageWriter<'w, SpellHit>,
    freeze: MessageWriter<'w, FreezeRequest>,
    growth: MessageWriter<'w, GrowthPulse>,
    mind: MessageWriter<'w, MindEffect>,
    ward: MessageWriter<'w, crate::wards::WardStruck>,
}

/// What the hits may read about a body.
type BodyInfo<'a> = (
    Option<&'a GlobalTransform>,
    Option<&'a MagicCaster>,
    Option<&'a RigidBody>,
    Has<Combustible>,
    Has<CharacterController>,
    Option<&'a Faction>,
);

/// A spell made by a reaction or by two spells meeting: an area of the recipe's ingredients
/// at a share of the hit's power, which never reacts again.
pub fn reaction_spell(
    recipe: &Recipe,
    power: Power,
    radius: f32,
    secs: f32,
    strength: f32,
) -> Option<Arc<Spell>> {
    let fx: Vec<&str> = recipe.ingredients.to_vec();
    let design = SpellDesign::new(&fx, CastType::Area).named(recipe.name);
    let s = compile(&design, CastContext::new(Mastery::Ascended3, power)).ok()?;
    let mut s = (*s).clone();
    s.carrier.radius = radius;
    s.carrier.duration = secs;
    s.carrier.tick = 0.5;
    s.payload.damage *= strength;
    s.payload.force = None;
    s.stages.clear();
    Some(Arc::new(s))
}

/// How much a spell speeds plants up (> 0, life) or withers them (< 0, blight) where it lands.
pub fn growth_of(spell: &Spell, scale: f32) -> f32 {
    let p = &spell.payload;
    let k = scale.clamp(0.3, 2.0);
    if spell.element() == Some(Element::Blight) {
        -k
    } else if p.heal > 0.0 && !p.harmful() {
        k
    } else {
        0.0
    }
}

/// Apply this step's hits.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn apply_hits(
    mut queue_in: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    queries: Res<PhysicsQueries>,
    gravity: Res<Gravity>,
    mut affs: Query<&mut Afflictions>,
    mut vitals: Query<&mut Vitality>,
    info: Query<BodyInfo>,
    creatures: Query<(), CreatureFilter>,
    mut out: HitWriters,
    ground: Option<Res<Ground>>,
    colliders: Query<&Collider>,
    mut fresh: Local<HashMap<Entity, Afflictions>>,
    mut dealt: Local<HashMap<Entity, f32>>,
    (factions, rewinding): (
        Option<Res<Factions>>,
        Query<(), With<crate::space::Rewinding>>,
    ),
) {
    let list = std::mem::take(&mut queue_in.0);
    if list.is_empty() {
        return;
    }
    fresh.clear();
    dealt.clear();
    let mut staged: Vec<PendingHit> = Vec::new();
    for h in list {
        let spell = h.cast.spell.clone();
        let p = &spell.payload;
        let scale = h.cast.scale;
        let deeper = h.cast.depth < MAX_STAGE_DEPTH;
        if h.expire {
            if deeper {
                for st in spell.stages_on(Trigger::Expire) {
                    fire_stage(st, &h, &queries, &gravity, &mut queue, &mut staged);
                }
            }
            continue;
        }
        let target = h.target;
        // Nothing touches a body rewinding along its path (a recall), but its own spells.
        if target.is_some_and(|t| rewinding.contains(t) && Some(t) != h.cast.caster) {
            continue;
        }
        let creature = target.is_some_and(|t| creatures.contains(t));
        let (t_pos, t_caster, t_body, combustible, character, t_faction) = target
            .and_then(|t| info.get(t).ok())
            .map(|(g, c, b, comb, ch, f)| (g.map(|g| g.translation()), c, b, comb, ch, f))
            .unwrap_or((None, None, None, false, false, None));
        // Who a spell may touch: filters and safeguard.
        if let Some(t) = target
            && creature
        {
            let friend = h.cast.friend(t, t_caster, t_faction, factions.as_deref());
            let skip = match spell.filter {
                TargetFilter::Enemies => friend,
                TargetFilter::Allies => !friend,
                TargetFilter::Everyone => spell.safeguard && friend && p.harmful(),
            };
            if skip {
                continue;
            }
        }
        let has_life = target.is_some_and(|t| vitals.contains(t));
        let touches_life = creature || has_life;
        let mut mult = 1.0;
        let mut reaction: Option<&'static Recipe> = None;
        let mut change: Option<StatusChange> = None;
        let mut shattered = false;
        let base_damage = p.damage * scale;

        // ---- element, reaction, statuses
        if let Some(t) = target
            && touches_life
        {
            let aff: &mut Afflictions = match affs.get_mut(t) {
                Ok(a) => a.into_inner(),
                Err(_) => fresh.entry(t).or_default(),
            };
            if base_damage > 0.0 {
                aff.source = h.cast.caster;
            }
            let before = (aff.dread, aff.calmed, aff.veiled);
            // A warded body's ward takes the blow, and with it the element (wards.rs).
            let warded = aff.ward > 0.0 && base_damage > 0.0;
            if let Some(el) = spell.element()
                && !warded
                && !p.statuses.is_empty()
                && aff.gate(el, h.multi)
            {
                if h.reacts
                    && !h.cast.reaction
                    && let Some((r, m)) = aff.react(el, 1.0)
                {
                    mult = m;
                    reaction = Some(r);
                }
                for &(s, n) in &p.statuses {
                    if let Some(c) = aff.add(
                        s,
                        n * scale.max(0.5),
                        base_damage,
                        spell.carrier.stack_threshold,
                    ) {
                        change = change.or(Some(c));
                    }
                }
                if spell.carrier.rend {
                    aff.add(
                        Status::Bleeding,
                        1.0,
                        base_damage,
                        spell.carrier.stack_threshold,
                    );
                }
                if let Some(r) = reaction {
                    for &(s, n) in r.reaction.statuses {
                        if let Some(c) = aff.add(s, n, base_damage, spell.carrier.stack_threshold) {
                            change = change.or(Some(c));
                        }
                    }
                    if r.reaction.quench {
                        aff.quench();
                    }
                }
            }
            // A heavy blow shatters ice.
            let heavy = !h.multi
                && (p.force.is_some() || h.cast.depth > 0 || base_damage >= 12.0)
                && change != Some(StatusChange::Froze);
            if heavy && base_damage > 0.0 && aff.shatter() {
                shattered = true;
                mult = (mult * 1.5).min(2.5);
            }
            if p.time.is_some() {
                aff.tempo = p.time.map(|(f, s)| (f, s * scale.max(0.5)));
            }
            // A shell comes from a single hit (a ward on yourself, a touch, a bolt); a ward cast
            // as an area or a wall stands on its own (wards.rs), and a field's ticks don't
            // shell what stands in it.
            if p.ward > 0.0 && !h.multi {
                aff.raise_ward(p.ward * scale, p.ward_element);
            }
            if p.veil_secs > 0.0 {
                aff.veiled = aff.veiled.max(p.veil_secs);
            }
            if p.calm_secs > 0.0 {
                aff.calmed = aff.calmed.max(p.calm_secs);
            }
            // Fire cracks a stone grip (and stone closes again only after it is gone).
            if aff.rooted > 0.0 && spell.element() == Some(Element::Fire) && base_damage > 0.0 {
                aff.rooted = aff.rooted.min(0.2);
                if let Some(tp) = t_pos {
                    queue.looks.push(LookCmd::Burst {
                        at: tp + DVec3::Y * 0.3,
                        up: Vec3::Y,
                        look: "stone".into(),
                        role: Role::Impact,
                        scale: 0.9,
                        color: crate::combine::STONE_COLOR,
                    });
                }
            } else if p.stone_m > 0.0 && creature && h.impact && !h.multi {
                // Stone closes around the feet: a stone grip roots the body for a few seconds.
                let secs = (p.stone_secs * 0.35).clamp(2.0, 8.0) * scale.clamp(0.5, 1.5);
                if aff.rooted <= 0.0
                    && let Some(tp) = t_pos
                {
                    let feet = crate::runtime::ground_below(
                        &queries,
                        tp + DVec3::Y * 0.5,
                        &QueryFilter::excluding(t),
                    );
                    let seed = h.cast.seed as u64 ^ feet.x.to_bits() ^ (feet.z.to_bits() << 7);
                    queue
                        .stones
                        .extend(crate::stone::grip_stones(feet, t, secs + 1.0, seed));
                }
                aff.rooted = aff.rooted.max(secs);
            }
            if p.fear_secs > 0.0 && base_damage <= 0.0 {
                aff.dread = aff.dread.max(p.fear_secs);
            }
            // The terrified run from where the terror came from: its caster, else back along
            // the blow.
            if aff.dread > before.0 + 0.5 {
                aff.dread_from = Some(
                    h.cast
                        .caster
                        .filter(|c| Some(*c) != target)
                        .and_then(|c| info.get(c).ok())
                        .and_then(|i| i.0)
                        .map(|g| g.translation())
                        .unwrap_or(h.point - (h.dir * 3.0).as_dvec3()),
                );
            }
            // Command and Wrath take hold of creatures (minds.rs).
            if creature && (p.command_secs > 0.0 || p.wrath_secs > 0.0) {
                queue.minds.push(crate::minds::MindCmd {
                    target: t,
                    by: h.cast.clone(),
                    command_secs: p.command_secs * scale.clamp(0.5, 1.5),
                    wrath_secs: p.wrath_secs * scale.clamp(0.5, 1.5),
                });
            }
            // Minds are NPC AI's: it makes the terrified flee, the calmed stand down and the
            // veiled unseen. Sent when one starts or grows, not on every tick of an aura.
            for (kind, was, now) in [
                (MindKind::Fear, before.0, aff.dread),
                (MindKind::Calm, before.1, aff.calmed),
                (MindKind::Veil, before.2, aff.veiled),
            ] {
                if now > was + 0.5 {
                    let secs = now;
                    out.mind.write(MindEffect {
                        target: t,
                        kind,
                        secs,
                        source: h.cast.caster,
                    });
                }
            }
        }

        // ---- damage
        let mut damage = base_damage * mult;
        if let Some(t) = target
            && touches_life
            && damage > 0.0
        {
            let aff: &mut Afflictions = match affs.get_mut(t) {
                Ok(a) => a.into_inner(),
                Err(_) => fresh.entry(t).or_default(),
            };
            let had = aff.ward;
            let ward_element = aff.ward_element;
            let (left, taken) = aff.absorb_hit(damage, spell.element());
            damage = left;
            // A hard blow shakes off terror (WoW's fear breaks past a share of health): a
            // fifth of the body's health in one hit, from anything but the terror itself.
            if aff.dread > 0.0
                && p.fear_secs <= 0.0
                && vitals.get(t).is_ok_and(|v| damage >= 0.2 * v.max)
            {
                aff.dread = aff.dread.min(0.3);
            }
            if had > 0.0 {
                let full = aff.ward_full.max(1e-3);
                out.ward.write(crate::wards::WardStruck {
                    ward: t,
                    barrier: false,
                    point: h.point,
                    dir: h.dir,
                    taken,
                    share: taken / full,
                    broke: aff.ward <= 0.0,
                    perfect: false,
                });
                // An elemental ward answers a blow from close by: fire burns, frost chills
                // toward a freeze, storm shocks.
                if let Some(el) = ward_element
                    && let Some(att) = h.cast.caster
                    && att != t
                    && h.cast.depth == 0
                    && let (Some(tp), Ok(ai)) = (t_pos, info.get(att))
                    && let Some(ap) = ai.0.map(|g| g.translation())
                    && ap.distance(tp) <= crate::wards::PAYBACK_M as f64
                {
                    staged.push(PendingHit {
                        cast: CastInfo {
                            spell: crate::wards::payback_spell(el),
                            caster: Some(t),
                            side: t_caster.map_or(h.cast.side.wrapping_add(1), |c| c.side),
                            faction: t_faction.copied(),
                            depth: 1,
                            scale: 0.5,
                            seed: h.cast.seed.wrapping_mul(31).wrapping_add(7),
                            reaction: true,
                        },
                        target: Some(att),
                        point: ap + DVec3::Y * 1.1,
                        normal: -h.dir,
                        dir: -h.dir,
                        multi: false,
                        impact: false,
                        reacts: false,
                        expire: false,
                    });
                }
            }
        }
        let force = p.force.map(|(dir, imp)| {
            let caster_pos = h
                .cast
                .caster
                .and_then(|c| info.get(c).ok())
                .and_then(|i| i.0)
                .map(|g| g.translation());
            let v = match dir {
                ForceDir::Away => (h.dir + Vec3::Y * 0.35).normalize_or(Vec3::Y),
                ForceDir::Toward => match (caster_pos, t_pos) {
                    (Some(c), Some(tp)) => ((c - tp).as_vec3().normalize_or(-h.dir)
                        + Vec3::Y * 0.3)
                        .normalize_or(Vec3::Y),
                    _ => (-h.dir + Vec3::Y * 0.3).normalize_or(Vec3::Y),
                },
                ForceDir::Up => Vec3::Y,
                ForceDir::Down => Vec3::NEG_Y,
            };
            v * imp * scale
        });
        let mut killed = false;
        if let Some(t) = target {
            if touches_life && (damage > 0.0 || force.is_some()) {
                out.damage.write(DamageEvent {
                    target: t,
                    amount: damage,
                    kind: p.damage_kind,
                    point: h.point,
                    impulse: force.unwrap_or(Vec3::ZERO),
                    source: h.cast.caster,
                });
                if let Ok(v) = vitals.get(t) {
                    let before = v.health - dealt.get(&t).copied().unwrap_or(0.0);
                    *dealt.entry(t).or_default() += damage;
                    killed = before > 0.0 && before - damage <= 0.0 && !p.subdue;
                }
            }
            // ---- force on bodies (crates, barrels, dummies on their bases)
            if let Some(f) = force
                && let Some(b) = t_body
                && b.kind == BodyKind::Dynamic
                && !character
            {
                let mass = b.mass_kg.unwrap_or(60.0).max(1.0);
                // A lasting lift holds things up (a little more than gravity each tick, so
                // they rise and bob); everything else is one shove, never past 16 m/s.
                let lifting = h.multi && p.force.is_some_and(|(d, _)| d == ForceDir::Up);
                let f = if lifting {
                    let tick = spell.carrier.tick.max(0.1);
                    let g = gravity.at(h.point).length().max(1.0);
                    let wobble = crate::runtime::roll(h.cast.seed as u64, t.to_bits()) - 0.5;
                    Vec3::new(wobble, 0.0, -wobble) * mass * 0.6 + Vec3::Y * mass * g * tick * 1.12
                } else {
                    f.clamp_length_max(16.0 * mass)
                };
                let spin = Vec3::new(f.z, 0.0, -f.x) * if lifting { 0.004 } else { 0.02 };
                out.impulse.write(ApplyImpulse {
                    entity: t,
                    impulse: f,
                    at: None,
                    angular: spin,
                });
            }
        }

        // ---- the world around an impact
        let ground_hit = !creature;
        // Fire spreads through the ground's fuel under a point whatever its height, so heat
        // and ignition reach the ground only from a hit on the ground itself: not on a body,
        // nor on a stone floor, wall or roof standing over grass (a placed solid under the
        // point, where terrain is a heightfield).
        let on_ground = p.ignite_c > 0.0
            && ground
                .as_ref()
                .and_then(|g| g.0.sample(h.point))
                .is_none_or(|s| (h.point.y - s.height).abs() < 0.35)
            && !queries
                .raycast(
                    h.point + DVec3::Y * 0.3,
                    Vec3::NEG_Y,
                    0.8,
                    &QueryFilter::default(),
                )
                .is_some_and(|under| {
                    colliders
                        .get(under.entity)
                        .is_ok_and(|c| !matches!(c.shape, ColliderShape::Heightfield { .. }))
                });
        if h.impact {
            let blast_r = if matches!(spell.cast, CastType::Area) {
                spell.carrier.radius
            } else {
                spell.carrier.radius * 4.0
            };
            if p.force.is_some() || spell.cast == CastType::Area {
                let b = WorldImpulse {
                    center: h.point,
                    radius: blast_r.max(1.5) * 1.6,
                    strength: (6.0 + p.force.map(|f| f.1).unwrap_or(0.0) * 0.04).min(40.0),
                    energy_j: 0.0,
                    heat_c: if p.ignite_c > 0.0 && spell.cast == CastType::Area && on_ground {
                        p.ignite_c
                    } else {
                        0.0
                    },
                    source: h.cast.caster,
                };
                if let Some(n) = b.noise() {
                    out.noise.write(n);
                }
                out.blast.write(b);
            }
            if p.ignite_c > 0.0 && on_ground {
                out.ignite.write(IgniteRequest {
                    position: h.point,
                    radius: blast_r.clamp(0.4, 4.0),
                    temperature_c: p.ignite_c,
                    source: h.cast.caster,
                });
            }
            // Missiles show their own impact; areas show theirs where they spawn.
            if matches!(
                spell.cast,
                CastType::Bolt
                    | CastType::Lob
                    | CastType::Rain
                    | CastType::Orbit
                    | CastType::Touch
                    | CastType::Beam
            ) {
                queue.looks.push(LookCmd::Burst {
                    at: h.point,
                    up: h.normal,
                    look: spell.weave.key.clone(),
                    role: Role::Impact,
                    // It grows slower than the bolt, so a master's blast reads as bigger, not
                    // as a sky full of fire; a rain or split volley lands many at once, each
                    // burst smaller still.
                    scale: ((spell.carrier.radius / 0.16).powf(0.6) * 0.8 * scale.sqrt()
                        / if h.multi { 1.6 } else { 1.0 })
                    .clamp(0.3, 2.2),
                    color: spell.weave.color,
                });
            }
            // A stone bolt raises a step where it strikes the ground (never under a loose
            // body: rising inside a crate would throw it).
            if p.stone_m > 0.0
                && ground_hit
                && !h.multi
                && matches!(spell.cast, CastType::Bolt | CastType::Lob)
                && h.normal.y > 0.6
                && t_body.is_none_or(|b| b.kind != BodyKind::Dynamic)
            {
                use crate::stone::{NewStone, StoneKind, stone_size};
                queue.stones.push(NewStone {
                    kind: StoneKind::Block,
                    at: h.point,
                    along: Vec3::new(h.dir.x, 0.0, h.dir.z).normalize_or(Vec3::X),
                    size: stone_size(StoneKind::Block, p.stone_m, 0.0),
                    owner: None,
                    life: p.stone_secs.max(4.0),
                    seed: h.cast.seed as u64 ^ h.point.x.to_bits() ^ (h.point.z.to_bits() << 9),
                });
            }
            if ground_hit {
                // Frost freezes water where it lands (an ice floe under a frost bolt), life
                // greens the plants around and blight withers them.
                if spell.element() == Some(Element::Frost) {
                    out.freeze.write(FreezeRequest {
                        center: h.point,
                        radius_m: blast_r.clamp(0.8, 8.0),
                        secs: 20.0 * scale.max(0.5),
                        source: h.cast.caster,
                    });
                }
                let growth = growth_of(&spell, scale);
                if growth != 0.0 {
                    out.growth.write(GrowthPulse {
                        center: h.point,
                        radius_m: blast_r.clamp(1.0, 10.0),
                        amount: growth,
                        secs: 6.0,
                        source: h.cast.caster,
                    });
                }
                // The dark marks and the lightning fern came with the Particles round of
                // 2026-11-05; looks::apply falls back to the older kind where they're missing.
                let kind = match spell.element() {
                    Some(Element::Fire) => Some("scorch"),
                    Some(Element::Frost) => Some("frost"),
                    Some(Element::Storm) => Some("burn"),
                    Some(Element::Blood) => Some("blood"),
                    Some(Element::Blight) => Some("rot"),
                    Some(Element::Shadow) => Some("void"),
                    _ => None,
                };
                if let Some(kind) = kind {
                    queue.looks.push(LookCmd::Mark {
                        at: h.point,
                        normal: h.normal,
                        kind,
                        radius: blast_r.clamp(0.4, 6.0) * 0.7,
                    });
                }
            }
        } else if creature && h.multi && change.is_some() {
            queue.looks.push(LookCmd::Burst {
                at: h.point,
                up: Vec3::Y,
                look: spell.weave.key.clone(),
                role: Role::Impact,
                scale: 0.5,
                color: spell.weave.color,
            });
        }
        if let (Some(t), Some(StatusChange::Ignited)) = (target, change)
            && combustible
        {
            out.ignite.write(IgniteRequest {
                position: h.point,
                radius: 0.8,
                temperature_c: p.ignite_c.max(600.0),
                source: h.cast.caster.or(Some(t)),
            });
        }
        if change == Some(StatusChange::Froze) || shattered {
            queue.looks.push(LookCmd::Burst {
                at: t_pos.map(|p| p + DVec3::Y).unwrap_or(h.point),
                up: Vec3::Y,
                look: "frost".into(),
                role: Role::Impact,
                scale: if shattered { 1.8 } else { 1.0 },
                color: Element::Frost.color(),
            });
        }

        // ---- reactions: a burst of the recipe, clouds, chains, spreading. A woven spell
        // (steam, plague) brings its own recipe's where it lands.
        let woven = (h.impact && !h.cast.reaction && h.cast.depth == 0 && reaction.is_none())
            .then(|| crate::combine::recipe(&spell.weave.key))
            .flatten()
            .filter(|r| {
                spell.weave.elements.len() > 1
                    && (r.reaction.cloud_m > 0.0 || r.reaction.chains > 0 || r.reaction.spreads)
            });
        if let Some(r) = reaction.or(woven) {
            let at = t_pos.map(|p| p + DVec3::Y).unwrap_or(h.point);
            queue.looks.push(LookCmd::Burst {
                at,
                up: Vec3::Y,
                look: r.key.into(),
                role: Role::Impact,
                scale: 1.5,
                color: r.color,
            });
            let child = |s: Arc<Spell>| CastInfo {
                spell: s,
                caster: h.cast.caster,
                side: h.cast.side,
                faction: h.cast.faction,
                depth: MAX_STAGE_DEPTH,
                scale: 1.0,
                seed: h.cast.seed.wrapping_add(977),
                reaction: true,
            };
            if r.reaction.cloud_m > 0.0
                && let Some(s) = reaction_spell(r, spell.power, r.reaction.cloud_m, 6.0, 0.25)
            {
                let ground = crate::runtime::ground_below(&queries, at, &QueryFilter::default());
                queue.spawns.push(Spawn::Field(
                    crate::runtime::field(
                        &child(s),
                        crate::runtime::FieldKind::Lasting,
                        r.reaction.cloud_m,
                        6.0,
                    ),
                    ground,
                ));
                out.smoke.write(SmokeEmission {
                    position: at,
                    velocity: Vec3::Y * 1.5,
                    radius: r.reaction.cloud_m * 0.6,
                    mass_kg: 0.4 * r.reaction.cloud_m,
                    temperature_c: if r.reaction.cloud_dark < 0.3 {
                        100.0
                    } else {
                        40.0
                    },
                    darkness: r.reaction.cloud_dark,
                });
            }
            if r.reaction.spreads
                && let Some(s) = reaction_spell(r, spell.power, 4.0, 0.0, 0.5)
            {
                queue.spawns.push(Spawn::Field(
                    crate::runtime::field(&child(s), crate::runtime::FieldKind::Burst, 4.0, 0.0),
                    at - DVec3::Y,
                ));
            }
            if r.reaction.chains > 0
                && let Some(s) = reaction_spell(r, spell.power, 0.0, 0.0, 0.6)
            {
                let mut s2 = (*s).clone();
                s2.carrier.chain_radius = 8.0;
                s2.carrier.range = 8.0;
                queue.spawns.push(Spawn::Chain(ChainArc {
                    cast: child(Arc::new(s2)),
                    at,
                    hit: target.into_iter().collect(),
                    jumps_left: r.reaction.chains,
                    next_in: 0.1,
                    from_reaction: true,
                }));
            }
        }

        // ---- heal, lifesteal
        if let Some(t) = target
            && p.heal > 0.0
        {
            let blocked = affs.get(t).is_ok_and(|a| a.heal_blocked());
            if !blocked
                && let Ok(mut v) = vitals.get_mut(t)
                && !v.down()
            {
                v.health = (v.health + p.heal * scale).min(v.max);
            }
        }
        let steal = p.lifesteal + reaction.map(|r| r.reaction.drain).unwrap_or(0.0);
        if steal > 0.0
            && damage > 0.0
            && let Some(c) = h.cast.caster
            && let Ok(mut v) = vitals.get_mut(c)
        {
            v.health = (v.health + damage * steal).min(v.max);
            if let Some(tp) = t_pos {
                queue.looks.push(LookCmd::Beam {
                    from: tp + DVec3::Y,
                    to: info
                        .get(c)
                        .ok()
                        .and_then(|i| i.0)
                        .map(|g| g.translation() + DVec3::Y * 1.2)
                        .unwrap_or(h.point),
                    look: "blood".into(),
                    color: Element::Blood.color(),
                    width: 0.06,
                    secs: 0.3,
                });
            }
        }

        // ---- stages
        if deeper {
            if h.impact {
                for st in spell.stages_on(Trigger::Impact) {
                    fire_stage(st, &h, &queries, &gravity, &mut queue, &mut staged);
                }
            }
            if creature {
                for st in spell.stages_on(Trigger::Hit) {
                    fire_stage(st, &h, &queries, &gravity, &mut queue, &mut staged);
                }
            }
            if killed {
                for st in spell.stages_on(Trigger::Kill) {
                    fire_stage(st, &h, &queries, &gravity, &mut queue, &mut staged);
                }
            }
        }
        if target.is_some() || h.impact {
            out.hit.write(SpellHit {
                spell: spell.clone(),
                caster: h.cast.caster,
                target,
                point: h.point,
                damage,
                reaction: reaction.map(|r| r.key),
                status: change,
                shattered,
                killed,
            });
        }
    }
    for (e, a) in fresh.drain() {
        queue.afflict.push((e, a));
    }
    queue_in.0.extend(staged);
}

/// Fire a stage spell from where its parent hit.
fn fire_stage(
    stage: &Arc<Spell>,
    h: &PendingHit,
    queries: &PhysicsQueries,
    gravity: &Gravity,
    queue: &mut MagicQueue,
    staged: &mut Vec<PendingHit>,
) {
    let info = CastInfo {
        spell: stage.clone(),
        caster: h.cast.caster,
        side: h.cast.side,
        faction: h.cast.faction,
        depth: h.cast.depth + 1,
        scale: h.cast.scale,
        seed: h.cast.seed.wrapping_mul(31).wrapping_add(7),
        reaction: h.cast.reaction,
    };
    let at = h.point + (h.normal * 0.15).as_dvec3();
    let mut hits = HitQueue::default();
    release(
        &info,
        at,
        at,
        h.dir,
        at + (h.dir * 4.0).as_dvec3(),
        None,
        gravity.at(at).length(),
        queries,
        queue,
        &mut hits,
    );
    staged.extend(hits.0);
}

/// Health from any module's damage: practice dummies and casters take sword blows as well as
/// spells.
pub fn take_damage(
    mut events: MessageReader<DamageEvent>,
    mut vitals: Query<&mut Vitality>,
    mut died: MessageWriter<Died>,
) {
    for d in events.read() {
        if let Ok(mut v) = vitals.get_mut(d.target)
            && !v.down()
        {
            v.health = (v.health - d.amount.max(0.0)).max(0.0);
            if v.down() {
                died.write(Died {
                    entity: d.target,
                    killer: d.source,
                    cause: d.kind,
                    point: d.point,
                });
            }
        }
    }
}

/// Health comes back; practice dummies stand up again.
pub fn recover(time: Res<Time>, mut vitals: Query<&mut Vitality>) {
    let dt = time.delta_secs();
    for mut v in &mut vitals {
        if v.down() {
            if v.revive_secs > 0.0 {
                v.down_secs += dt;
                if v.down_secs >= v.revive_secs {
                    v.health = v.max;
                    v.down_secs = 0.0;
                }
            }
        } else if v.regen > 0.0 && v.health < v.max {
            v.health = (v.health + v.regen * dt).min(v.max);
        }
    }
}
