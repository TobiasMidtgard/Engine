//! Spells in the world: the cast in progress, and the carriers a released spell becomes.
//!
//! Seventeen cast types run on a few shared kernels (research-architecture.md, recommendation
//! 2), so no spell and no modifier has its own system:
//!
//! | Kernel | Cast types | How it finds what it hits |
//! |---|---|---|
//! | [`Missile`] | bolt, lob, orbit, rain drops, forks | a sphere swept along each step's path |
//! | [`Field`] | area, aura, wall, wave, rune, cone, linger, blast | an overlap on its tick |
//! | [`BeamCast`] | beam, and tether (which holds one body: tether.rs) | a ray each tick |
//! | [`ChainArc`] | chain, and chain modifiers and reactions | the nearest creature not yet hit |
//! | instant | touch, self | at release |
//! | [`crate::summon::Minion`] | summon | its own eyes: the nearest foe, or the most hurt ally |
//!
//! Every system here only reads the world and writes its own components; what it finds goes
//! into the [`HitQueue`] (applied in `hits.rs`), and what must be spawned or shown goes into the
//! [`MagicQueue`], flushed by one system with `Commands` at the end of the step.

use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::MeshRenderer;
use engine_world::animation::{AnimationTrigger, SkeletonPose};
use engine_world::combat::{ItemClass, Loadout};
use engine_world::physics::{
    BodyKind, CharacterController, ColliderShape, PhysicsQueries, QueryFilter, RigidBody,
};
use engine_world::{
    AttackEvent, AttackKind, Faction, Factions, FreezeRequest, Gravity, GrowthPulse, NoiseEvent,
    ProjectileTrace, WorldShift,
};

use crate::caster::{
    Aim, CastOutcome, CastRequest, Casting, MagicCaster, SpellCast, SpellChoice, Vitality,
};
use crate::compile::Spell;
use crate::design::CastType;
use crate::looks::{LookCmd, Role};
use crate::schools::{Element, Status};
use crate::status::Afflictions;
use crate::stone::NewStone;

/// Stages nest at most this deep (Noita's limit): a blast's linger fires, the linger's own
/// stages don't.
pub const MAX_STAGE_DEPTH: u8 = 2;

/// Where a carrier is, kept by this module and copied into `Transform` each frame (so the heavy
/// systems never write `Transform`; AGENTS.md, performance rules).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct SpellPos {
    pub pos: DVec3,
    /// The look's up axis (the direction of travel for bolts).
    pub up: Vec3,
}

/// What every carrier knows about the cast that made it.
#[derive(Debug, Clone)]
pub struct CastInfo {
    pub spell: Arc<Spell>,
    pub caster: Option<Entity>,
    /// The caster's side (safeguard and filters spare it).
    pub side: u32,
    /// The caster's faction, when it has one: friend and foe then follow the engine's
    /// `Factions` (NPCs, animals and the player), else magic's own sides.
    pub faction: Option<Faction>,
    /// Stage depth: 0 for the cast itself.
    pub depth: u8,
    /// Strength multiplier (forks and echoes are weaker).
    pub scale: f32,
    pub seed: u32,
    /// Made by an element reaction or spells meeting: never sets off another reaction.
    pub reaction: bool,
}

impl CastInfo {
    /// Is `other` on this cast's side? Its caster always is; two bodies with factions are
    /// friends unless `Factions` says the cast's faction would attack the other's; otherwise
    /// magic's own sides decide.
    pub fn friend(
        &self,
        other: Entity,
        caster: Option<&MagicCaster>,
        faction: Option<&Faction>,
        factions: Option<&Factions>,
    ) -> bool {
        if Some(other) == self.caster {
            return true;
        }
        if let (Some(a), Some(&b)) = (self.faction, faction) {
            return factions.map_or(a == b, |f| !f.hostile(a, b));
        }
        caster.is_some_and(|c| c.side == self.side)
    }
}

/// Whose side a body is on: magic's own side and the engine's faction.
#[derive(bevy_ecs::query::QueryData)]
pub struct Allegiance {
    pub caster: Option<&'static MagicCaster>,
    pub faction: Option<&'static Faction>,
}

/// A circling orb.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub angle: f32,
    pub radius: f32,
    pub height: f32,
    /// rad/s.
    pub rate: f32,
}

/// A projectile in flight: bolt, lob, rain drop, fork, orbiting orb.
#[derive(Component, Debug, Clone)]
pub struct Missile {
    pub cast: CastInfo,
    pub vel: Vec3,
    pub traveled: f32,
    pub age: f32,
    /// Who it has already hit (pierce, boomerang passes, orbits' cooldowns).
    pub hits: Vec<(Entity, f32)>,
    pub pierce_left: u32,
    pub bounce_left: u32,
    pub target: Option<Entity>,
    pub returning: bool,
    pub orbit: Option<Orbit>,
    /// A fork's child (doesn't fork again).
    pub forked: bool,
    /// Seconds since it was spent (hidden, waiting for its trail to fade), or None while alive.
    pub spent: Option<f32>,
}

/// How a field behaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldKind {
    /// Hits everyone inside once (an area burst, a blast, a cone).
    Burst,
    /// Ticks for its duration (an area left burning, a linger, an aura, a wall).
    Lasting,
    /// Waits, armed, until someone steps in; then bursts.
    Rune,
}

/// An area of effect: area, aura, wall, wave, cone, rune, blast, linger.
#[derive(Component, Debug, Clone)]
pub struct Field {
    pub cast: CastInfo,
    pub kind: FieldKind,
    pub radius: f32,
    pub age: f32,
    pub duration: f32,
    pub next_tick: f32,
    /// Moves with this entity (an aura on its caster).
    pub follow: Option<Entity>,
    /// A cone: hits only within this half-angle (degrees) of `dir`.
    pub cone: Option<(Vec3, f32)>,
    /// A wall: a line of this half-length along `dir` (horizontal), radius thick.
    pub wall: Option<(Vec3, f32)>,
    /// A wave: moves along `dir` at `speed` up to `range`, hugging the ground.
    pub wave: Option<(Vec3, f32, f32)>,
    /// Per-victim cooldown (seconds left), so a lasting field ticks each victim once per tick.
    pub cooldowns: Vec<(Entity, f32)>,
    /// When another element last met it (spells that meet), field age.
    pub met_at: f32,
    /// How many spells have met it (capped).
    pub meets: u32,
    pub spent: Option<f32>,
}

/// A continuous beam from the caster's hand.
#[derive(Component, Debug, Clone)]
pub struct BeamCast {
    pub cast: CastInfo,
    pub aim: Aim,
    pub age: f32,
    pub duration: f32,
    pub next_tick: f32,
    pub end: DVec3,
    pub drawn: bool,
    /// The age its look lasts until; a channel held past it draws again.
    pub drawn_until: f32,
    /// A tether's hold on its body (tether.rs): set on its first tick.
    pub tether: Option<crate::tether::TetherLink>,
}

/// Lightning (or blood, or plague) jumping from body to body.
#[derive(Component, Debug, Clone)]
pub struct ChainArc {
    pub cast: CastInfo,
    pub at: DVec3,
    pub hit: Vec<Entity>,
    pub jumps_left: u32,
    pub next_in: f32,
    /// Hops made by a reaction never set off further reactions.
    pub from_reaction: bool,
    /// The body it was aimed at: its first hop goes there when it is in reach.
    pub first: Option<Entity>,
}

/// A rain of drops over an area.
#[derive(Component, Debug, Clone)]
pub struct Rainfall {
    pub cast: CastInfo,
    pub center: DVec3,
    pub age: f32,
    pub duration: f32,
    pub dropped: u32,
    /// The caster's line, flat: meteors (Rain + stone) come in along it.
    pub toward: Vec3,
}

/// One thing a carrier reached this step, applied in order by `hits::apply_hits`.
#[derive(Debug, Clone)]
pub struct PendingHit {
    pub cast: CastInfo,
    /// The body hit, if any (a creature, a crate, a wall); `None` for the ground or thin air.
    pub target: Option<Entity>,
    pub point: DVec3,
    pub normal: Vec3,
    /// Direction the spell was travelling (or from the blast's center).
    pub dir: Vec3,
    /// A multi-hit carrier (beam, field tick, orbit, rain): elements apply on the internal
    /// cooldown.
    pub multi: bool,
    /// Fire the spell's Impact stages here.
    pub impact: bool,
    /// May set off an element reaction.
    pub reacts: bool,
    /// The carrier ended without hitting anything (fires Expire stages).
    pub expire: bool,
}

/// Hits found this step, applied in one place.
#[derive(Resource, Default, Debug)]
pub struct HitQueue(pub Vec<PendingHit>);

/// Something to spawn this step.
#[derive(Debug, Clone)]
pub enum Spawn {
    Missile(Missile, DVec3),
    Field(Field, DVec3),
    Beam(BeamCast, DVec3),
    Chain(ChainArc),
    Rain(Rainfall),
    /// A status look riding on a body.
    Follow(FollowLook, DVec3),
}

/// Work for the step's last system: spawns, despawns, component inserts and looks.
#[derive(Resource, Default, Debug)]
pub struct MagicQueue {
    pub spawns: Vec<Spawn>,
    pub looks: Vec<LookCmd>,
    pub despawn: Vec<Entity>,
    /// Teleports (blink): entity and where.
    pub moves: Vec<(Entity, DVec3)>,
    /// Bodies touched for the first time, with what touched them.
    pub afflict: Vec<(Entity, Afflictions)>,
    /// Set (Some) or remove (None) a `LocalTimeScale`.
    pub time_scales: Vec<(Entity, Option<LocalTimeScale>)>,
    /// Wall pieces' new bodies (a wall that changed with a spell meeting it).
    pub wall_bodies: Vec<crate::walls::NewBody>,
    /// Bodies' first `Statuses` (the engine's view of their afflictions).
    pub statuses: Vec<(Entity, engine_world::Statuses)>,
    /// Stone to raise (Shaping: pillars, steps, grips; walls raise theirs in the flush).
    pub stones: Vec<crate::stone::NewStone>,
    /// Stones that found their ground and become solid now (pillars), with their size.
    pub solids: Vec<(Entity, Vec3)>,
    /// Summoned creatures to put into the world (summon.rs).
    pub minions: Vec<crate::summon::NewMinion>,
    /// Raises, for a fallen body near their aim first (undead.rs), else bones from the ground.
    pub raises: Vec<crate::summon::NewMinion>,
    /// Domes and ward wall panels to put up (wards.rs).
    pub barriers: Vec<crate::wards::NewBarrier>,
    /// Portal ends to open (space.rs).
    pub gates: Vec<crate::space::NewGate>,
    /// Portal ends something just went through (they flare).
    pub gate_pulses: Vec<Entity>,
    /// Blinks and recalls to start (space.rs).
    pub travels: Vec<crate::space::Travel>,
    /// Grips to start (kinesis.rs).
    pub grips: Vec<crate::kinesis::Grip>,
    /// Meteors to drop and fault lines to open (cataclysm.rs).
    pub cataclysm: Vec<crate::cataclysm::NewCataclysm>,
    /// Copies of casters to make (minds.rs).
    pub phantasms: Vec<crate::minds::NewPhantasm>,
    /// Creatures taken by Command or Wrath (minds.rs).
    pub minds: Vec<crate::minds::MindCmd>,
    /// Conjured blades to make: a phalanx, a bladefall, spirit blades, a blade in the hand
    /// (blades.rs).
    pub blades: Vec<crate::blades::NewBlades>,
    /// Imbues to put on their casters' weapons and creatures (blades.rs).
    pub imbues: Vec<CastInfo>,
    /// Bodies for chains to bind, hooks that bit, bonds' hops (chains.rs).
    pub binds: Vec<crate::chains::NewBind>,
    pub hooks: Vec<crate::chains::NewHook>,
    pub bond_hops: Vec<crate::chains::BondHop>,
    /// Hellgates to open and demons' meteors that landed (demons.rs).
    pub hellgates: Vec<crate::demons::NewHellgate>,
}

/// Bodies spells look for: anything with health, a skeleton or a character controller.
pub type CreatureFilter = Or<(
    With<Vitality>,
    With<SkeletonPose>,
    With<CharacterController>,
)>;

/// A cheap deterministic random number in 0..1 from a seed and a salt.
pub fn roll(seed: u64, salt: u64) -> f32 {
    let mut z = seed
        .wrapping_add(salt.wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add(0x6a09_e667_f3bc_c909);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// The caster's hand in the world.
pub fn hand_of(t: &GlobalTransform, caster: &MagicCaster) -> DVec3 {
    t.translation() + (t.rotation() * caster.hand).as_dvec3()
}

/// Casting from the off hand: the caster says so, or holds a spell focus there with a weapon
/// in the other (aimed-melee.md 3.7, "weapon + magic").
pub fn casts_off_hand(caster: &MagicCaster, loadout: Option<&Loadout>) -> bool {
    caster.off_hand || loadout.is_some_and(|l| l.off == ItemClass::SpellFocus)
}

/// Where a cast leaves: [`hand_of`], or for an off-hand cast (`off`, or [`casts_off_hand`])
/// the off hand's bone (`hand_l`) when the body has a pose, else the hand mirrored.
pub fn cast_hand(
    t: &GlobalTransform,
    caster: &MagicCaster,
    pose: Option<&SkeletonPose>,
    loadout: Option<&Loadout>,
    off: bool,
) -> DVec3 {
    if !off && !casts_off_hand(caster, loadout) {
        return hand_of(t, caster);
    }
    if let Some(p) = pose
        && let Some(i) = p.index_of("hand_l")
        && let Some((at, _)) = p.bone_world(t, i)
    {
        return at;
    }
    let h = caster.hand;
    t.translation() + (t.rotation() * Vec3::new(-h.x, h.y, h.z)).as_dvec3()
}

/// Velocity that lobs from `from` to `to` at `speed` under gravity `g` (the low arc), or a 45°
/// throw when out of reach.
pub fn lob_velocity(from: DVec3, to: DVec3, speed: f32, g: f32) -> Vec3 {
    let d = (to - from).as_vec3();
    let flat = Vec3::new(d.x, 0.0, d.z);
    let x = flat.length().max(0.01);
    let y = d.y;
    let dir = flat / x;
    let v2 = speed * speed;
    let disc = v2 * v2 - g * (g * x * x + 2.0 * y * v2);
    let angle = if disc >= 0.0 {
        ((v2 - disc.sqrt()) / (g * x)).atan()
    } else {
        std::f32::consts::FRAC_PI_4
    };
    dir * speed * angle.cos() + Vec3::Y * speed * angle.sin()
}

// ------------------------------------------------------------------------------- casting

/// Mana comes back over time.
pub fn regen_mana(time: Res<Time>, mut q: Query<(&mut MagicCaster, Option<&LocalTimeScale>)>) {
    for (mut c, local) in &mut q {
        let dt = time.delta_for(local);
        if c.mana < c.max_mana {
            c.mana = (c.mana + c.mana_regen * dt).min(c.max_mana);
        }
    }
}

/// Start the casts asked for: compile, telegraph, and begin gathering.
#[allow(clippy::too_many_arguments)]
pub fn begin_casts(
    mut requests: MessageReader<CastRequest>,
    casters: Query<(&MagicCaster, &GlobalTransform, Has<Casting>)>,
    mut queue: ResMut<MagicQueue>,
    mut attacks: MessageWriter<AttackEvent>,
    mut casts: MessageWriter<SpellCast>,
    mut started: Query<&mut Casting>,
    mut commands_out: ResMut<CastStarts>,
    mut anim: MessageWriter<AnimationTrigger>,
) {
    for r in requests.read() {
        let Ok((caster, t, busy)) = casters.get(r.caster) else {
            continue;
        };
        if busy {
            continue;
        }
        let design = match &r.spell {
            SpellChoice::Selected => caster.grown(caster.selected),
            SpellChoice::Slot(i) => caster.grown(*i),
            SpellChoice::Design(d) => Some(d.clone()),
        };
        let Some(design) = design else { continue };
        let spell = match caster.compile(&design, r.charge) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("cast refused: {e}");
                continue;
            }
        };
        let at = hand_of(t, caster);
        if caster.mana < spell.cost.mana {
            casts.write(SpellCast {
                caster: r.caster,
                spell,
                outcome: CastOutcome::Spent,
                at,
            });
            continue;
        }
        let release_at = if r.gathered {
            spell.cast_secs.min(0.15)
        } else {
            spell.cast_secs * (1.0 + spell.charge_secs * r.charge.clamp(0.0, 1.0))
        };
        attacks.write(AttackEvent {
            attacker: r.caster,
            target: match r.aim {
                Aim::Entity(e) => Some(e),
                _ => None,
            },
            windup_secs: release_at,
            reach: spell.carrier.range,
            kind: AttackKind::Spell,
        });
        queue.looks.push(LookCmd::Charge {
            caster: r.caster,
            look: spell.weave.key.clone(),
            secs: release_at,
            scale: (0.4 + 0.25 * spell.power.size()).min(2.0),
        });
        // The body's part: a throw, a two-handed heave, or a held channel (the animation
        // module plays them once it has them; until then they do nothing).
        anim.write(AnimationTrigger {
            entity: r.caster,
            name: cast_gesture(spell.cast).into(),
        });
        let casting = Casting {
            spell,
            aim: r.aim,
            t: 0.0,
            release_at,
            echo_in: None,
            off_hand: r.off_hand,
            seen_at: None,
        };
        if let Ok(mut c) = started.get_mut(r.caster) {
            *c = casting;
        } else {
            commands_out.0.push((r.caster, casting));
        }
    }
}

/// The animation trigger for casting a spell of this form: `cast_one_hand` (thrown: bolts,
/// lobs, chains, touches, blinks), `cast_two_hand` (heaved: areas, walls, waves, rain, cones,
/// runes, pillars) or `channel` (held: beams, auras, orbits).
pub fn cast_gesture(cast: CastType) -> &'static str {
    match cast {
        CastType::Bolt
        | CastType::Lob
        | CastType::Chain
        | CastType::Touch
        | CastType::Imbue
        | CastType::OnSelf => "cast_one_hand",
        CastType::Beam | CastType::Aura | CastType::Orbit | CastType::Tether => "channel",
        CastType::Area
        | CastType::Wall
        | CastType::Wave
        | CastType::Rain
        | CastType::Cone
        | CastType::Rune
        | CastType::Summon => "cast_two_hand",
    }
}

/// Casts started this step whose `Casting` component must be inserted.
#[derive(Resource, Default, Debug)]
pub struct CastStarts(pub Vec<(Entity, Casting)>);

/// Gather casts, then release (or fizzle, or backfire). Pays mana and health, trains the
/// subclass, and turns the spell into its carriers.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn advance_casts(
    time: Res<Time>,
    gravity: Res<Gravity>,
    mut casters: Query<(
        Entity,
        &mut MagicCaster,
        &mut Casting,
        &GlobalTransform,
        Option<&LocalTimeScale>,
        Option<&mut Vitality>,
        Option<&Faction>,
        (Option<&SkeletonPose>, Option<&Loadout>),
    )>,
    targets: Query<&GlobalTransform>,
    queries: Res<PhysicsQueries>,
    mut queue: ResMut<MagicQueue>,
    mut hits: ResMut<HitQueue>,
    mut casts: MessageWriter<SpellCast>,
    mut noise: MessageWriter<NoiseEvent>,
    mut gains: MessageWriter<crate::caster::MasteryGained>,
    mut counter: Local<u64>,
) {
    for (e, mut caster, mut casting, t, local, vitality, faction, (pose, loadout)) in &mut casters {
        let dt = time.delta_for(local);
        if let Some(echo) = casting.echo_in.as_mut() {
            *echo -= dt;
            if *echo > 0.0 {
                continue;
            }
        } else {
            casting.t += dt;
            if let Aim::Entity(target) = casting.aim
                && let Ok(g) = targets.get(target)
            {
                casting.seen_at = Some(g.translation() + DVec3::Y * 1.0);
            }
            if casting.t < casting.release_at {
                continue;
            }
        }
        let echo = casting.echo_in.is_some();
        let spell = casting.spell.clone();
        let hand = cast_hand(t, &caster, pose, loadout, casting.off_hand);
        *counter += 1;
        let seed = caster.seed ^ counter.wrapping_mul(0x2545_f491_4f6c_dd1d);
        // Aim: a direction from the hand.
        let aim_point = match casting.aim {
            // (A blink aimed only by a direction goes as far as it can.)
            Aim::Direction(d) => {
                let reach = spell.carrier.range.max(spell.payload.blink_m).max(1.0);
                hand + (d.normalize_or(Vec3::NEG_Z) * reach).as_dvec3()
            }
            Aim::Point(p) => p,
            // A body gone while the spell gathered (killed by another): where it was last.
            Aim::Entity(target) => targets
                .get(target)
                .map(|g| g.translation() + DVec3::Y * 1.0)
                .ok()
                .or(casting.seen_at)
                .unwrap_or(hand + DVec3::NEG_Z * 10.0),
        };
        let target = match casting.aim {
            Aim::Entity(x) => Some(x),
            _ => None,
        };
        if !echo {
            // Pay first: a fizzle still costs half.
            let fizzled = !caster.steady && roll(seed, 1) < spell.fizzle;
            let pay = if fizzled { 0.5 } else { 1.0 };
            caster.mana = (caster.mana - spell.cost.mana * pay).max(0.0);
            if spell.cost.health > 0.0
                && let Some(mut v) = vitality
            {
                v.health = (v.health - spell.cost.health * pay).max(1.0);
            }
            caster.corruption = (caster.corruption + spell.cost.corruption * pay).min(100.0);
            let sub = MagicCaster::subclass_of(&spell.design);
            if let Some(m) = caster.train(sub, spell.complexity, !fizzled) {
                gains.write(crate::caster::MasteryGained {
                    caster: e,
                    subclass: sub,
                    mastery: m,
                });
            }
            if fizzled {
                queue.looks.push(LookCmd::Burst {
                    at: hand,
                    up: Vec3::Y,
                    look: spell.weave.key.clone(),
                    role: Role::Fizzle,
                    scale: 0.6,
                    color: spell.weave.color,
                });
                casts.write(SpellCast {
                    caster: e,
                    spell,
                    outcome: CastOutcome::Fizzled,
                    at: hand,
                });
                queue.despawn_casting(e);
                continue;
            }
            if !caster.steady && roll(seed, 2) < spell.backfire {
                // It goes off in the caster's hands.
                hits.0.push(PendingHit {
                    cast: CastInfo {
                        spell: spell.clone(),
                        caster: None,
                        side: u32::MAX,
                        faction: None,
                        depth: 0,
                        scale: 0.6,
                        seed: seed as u32,
                        reaction: false,
                    },
                    target: Some(e),
                    point: hand,
                    normal: Vec3::Y,
                    dir: Vec3::Y,
                    multi: false,
                    impact: true,
                    reacts: true,
                    expire: false,
                });
                casts.write(SpellCast {
                    caster: e,
                    spell,
                    outcome: CastOutcome::Backfired,
                    at: hand,
                });
                queue.despawn_casting(e);
                continue;
            }
            if spell.noise_m > 0.0 {
                noise.write(NoiseEvent {
                    position: hand,
                    radius: spell.noise_m,
                    source: Some(e),
                });
            }
            casts.write(SpellCast {
                caster: e,
                spell: spell.clone(),
                outcome: CastOutcome::Released,
                at: hand,
            });
        }
        let info = CastInfo {
            spell: spell.clone(),
            caster: Some(e),
            side: caster.side,
            faction: faction.copied(),
            depth: 0,
            scale: if echo { 0.7 } else { 1.0 },
            seed: seed as u32,
            reaction: false,
        };
        let (rot, body) = (t.rotation(), t.translation());
        let forward = rot * Vec3::NEG_Z;
        release(
            &info,
            hand,
            body,
            forward,
            aim_point,
            target,
            gravity.at(hand).length(),
            &queries,
            &mut queue,
            &mut hits,
        );
        if spell.echo && !echo {
            casting.echo_in = Some(0.6);
        } else {
            queue.despawn_casting(e);
        }
    }
}

impl MagicQueue {
    /// Remove a finished cast from its caster.
    fn despawn_casting(&mut self, caster: Entity) {
        self.looks.push(LookCmd::EndCharge { caster });
        self.looks.push(LookCmd::RemoveCasting { caster });
    }
}

/// Turn a released spell into its carriers (or apply it at once for touch and self).
#[allow(clippy::too_many_arguments)]
pub fn release(
    info: &CastInfo,
    hand: DVec3,
    body: DVec3,
    forward: Vec3,
    aim_point: DVec3,
    target: Option<Entity>,
    g: f32,
    queries: &PhysicsQueries,
    queue: &mut MagicQueue,
    hits: &mut HitQueue,
) {
    let spell = &info.spell;
    let c = &spell.carrier;
    let to_aim = (aim_point - hand).as_vec3();
    let dir = to_aim.normalize_or(forward);
    let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
    let exclude = info.caster.map(QueryFilter::excluding).unwrap_or_default();
    // Phantasm: copies of the caster round it (Mirror image) or where it points (Decoy), in
    // place of anything else a self or summon cast would do (minds.rs).
    if spell.payload.phantasms > 0 && matches!(spell.cast, CastType::OnSelf | CastType::Summon) {
        let at = if spell.cast == CastType::OnSelf {
            body
        } else {
            ground_below(
                queries,
                crate::summon::place(body, aim_point, c.range),
                &exclude,
            )
        };
        queue.phantasms.push(crate::minds::NewPhantasm {
            cast: info.clone(),
            at,
            facing: flat,
            around_caster: spell.cast == CastType::OnSelf,
            target,
        });
        return;
    }
    // Imbue: the spell rides on its caster's weapon and its creatures' (blades.rs).
    if spell.cast == CastType::Imbue {
        queue.imbues.push(info.clone());
        return;
    }
    // Conjured blades (blades.rs): an arch over the head, a fall along the aim, spirit blades
    // beside the caster, a blade in the hand. A bolt of blades is a missile like any other,
    // with a sword drawn on it (a glintblade).
    if spell.payload.blades > 0 {
        let shape = match spell.cast {
            CastType::Orbit => Some(crate::blades::BladeShape::Phalanx),
            CastType::Rain => Some(crate::blades::BladeShape::Fall),
            CastType::Summon => Some(crate::blades::BladeShape::Spirit),
            CastType::OnSelf => Some(crate::blades::BladeShape::Bound),
            _ => None,
        };
        if let Some(shape) = shape {
            let aim = if shape == crate::blades::BladeShape::Fall {
                ground_below(queries, aim_point, &exclude)
            } else {
                aim_point
            };
            queue.blades.push(crate::blades::NewBlades {
                cast: info.clone(),
                shape,
                hand,
                body: if shape == crate::blades::BladeShape::Fall {
                    ground_below(queries, body, &exclude)
                } else {
                    body
                },
                facing: flat,
                aim,
                target,
            });
            return;
        }
    }
    // A pact's hellgate opens where the caster points (demons.rs).
    if spell.payload.demons > 0 && spell.cast == CastType::Summon {
        let spot = crate::summon::place(body, aim_point, c.range);
        queue.hellgates.push(crate::demons::NewHellgate {
            cast: info.clone(),
            at: ground_below(queries, spot, &exclude),
            facing: flat,
            meteor: false,
        });
        return;
    }
    // A summon stands where the caster points, within its range: a golem, an elemental or a
    // wisp, by its weave (summon.rs). A weave that makes no creature casts as an area.
    if spell.cast == CastType::Summon
        && let Some(kind) = crate::summon::kind_of(spell)
    {
        let spot = crate::summon::place(body, aim_point, c.range);
        let nm = crate::summon::NewMinion {
            kind,
            cast: info.clone(),
            at: ground_below(queries, spot, &exclude),
            facing: flat,
            on: target,
        };
        // A raise looks for a fallen body there first (undead.rs).
        if spell.payload.raise > 0.0 {
            queue.raises.push(nm);
        } else {
            queue.minions.push(nm);
        }
        return;
    }
    // Stone cast on an area (or on the caster) is a pillar rising under it: it lifts what
    // stands there by pushing it, so it takes the place of the field and of any lift.
    if spell.payload.stone_m > 0.0
        && matches!(
            spell.cast,
            CastType::Area | CastType::Summon | CastType::Rune | CastType::OnSelf
        )
    {
        let under = if spell.cast == CastType::OnSelf {
            body
        } else {
            aim_point
        };
        let at = ground_below(queries, under, &exclude);
        queue
            .stones
            .push(pillar(spell, at, flat, info.seed as u64, info.scale));
        return;
    }
    // Space (space.rs): a portal end opens where it aims; a blink slides its caster there (a
    // stage's blink to where the stage went off); a recall rewinds its caster's path. Whatever
    // else the spell does still goes on.
    let p = &spell.payload;
    if p.portal_secs > 0.0 {
        let reach = if c.range > 0.0 { c.range } else { 30.0 };
        let at = ground_below(
            queries,
            crate::space::within(body, aim_point, reach),
            &exclude,
        );
        let back = body - at;
        queue.gates.push(crate::space::NewGate {
            cast: info.clone(),
            at,
            to_caster: Vec3::new(back.x as f32, 0.0, back.z as f32).normalize_or(-flat),
        });
        return;
    }
    if let Some(me) = info.caster
        && (p.blink_m > 0.0 || p.recall_secs > 0.0)
    {
        let look = spell.weave.key.clone();
        let color = |plain: [f32; 3]| {
            if spell.weave.elements.is_empty() {
                plain
            } else {
                spell.weave.color
            }
        };
        if p.recall_secs > 0.0 {
            let color = color(crate::space::RECALL_COLOR);
            queue.travels.push(crate::space::Travel::Recall {
                body: me,
                secs: p.recall_secs,
                look,
                color,
            });
        } else {
            let color = color(crate::space::BLINK_COLOR);
            let to = if info.depth > 0 {
                ground_below(queries, body, &exclude)
            } else {
                crate::space::blink_target(queries, body, aim_point, forward, p.blink_m, &exclude)
            };
            queue.travels.push(crate::space::Travel::Blink {
                body: me,
                to,
                look,
                color,
            });
        }
        let more = p.damage > 0.0
            || p.force.is_some()
            || p.heal > 0.0
            || p.ward > 0.0
            || p.time.is_some()
            || p.veil_secs > 0.0
            || p.fear_secs > 0.0
            || p.calm_secs > 0.0
            || p.light_lm > 0.0
            || p.stone_m > 0.0;
        if !more {
            return;
        }
    }
    // A grip takes hold of something along its aim (or tears a stone out of the ground), or
    // as an orbit tears a ring of stones round its caster (kinesis.rs).
    if p.grip_kg > 0.0 {
        queue.grips.push(crate::kinesis::Grip::new(
            info.clone(),
            match target {
                Some(t) => Aim::Entity(t),
                None => Aim::Point(aim_point),
            },
        ));
        return;
    }
    // A ward cast as a cone is a shield held up before the caster (wards.rs); one that also
    // heals still heals.
    if p.shield_secs > 0.0 {
        let aim = match target {
            Some(t) => Aim::Entity(t),
            None => Aim::Point(aim_point),
        };
        crate::wards::raise_shield(info, body, aim, aim_point, forward, queue);
        if p.heal <= 0.0 {
            return;
        }
    }
    // A ward cast as an area is a dome, as a wall a row of panels (wards.rs); one that also
    // heals still heals inside it.
    if spell.payload.ward > 0.0 && matches!(spell.cast, CastType::Area | CastType::Wall) {
        crate::wards::raise(info, aim_point, flat, queries, &exclude, queue);
        if spell.payload.heal <= 0.0 {
            return;
        }
    }
    match spell.cast {
        CastType::Bolt | CastType::Lob => {
            let n = c.count.max(1);
            for i in 0..n {
                let k = if n > 1 {
                    i as f32 / (n - 1) as f32 - 0.5
                } else {
                    0.0
                };
                let yaw = Quat::from_rotation_y((c.spread_deg * k).to_radians());
                let lift = if n > 2 {
                    Quat::from_axis_angle(
                        flat.cross(Vec3::Y).normalize_or(Vec3::X),
                        0.04 * ((i % 2) as f32 - 0.5),
                    )
                } else {
                    Quat::IDENTITY
                };
                let vel = if spell.cast == CastType::Lob {
                    yaw * lob_velocity(hand, aim_point, c.speed, g * c.gravity.max(0.01))
                } else {
                    lift * yaw * dir * c.speed
                };
                queue.spawns.push(Spawn::Missile(
                    Missile {
                        cast: CastInfo {
                            seed: info.seed.wrapping_add(i * 7919),
                            ..info.clone()
                        },
                        vel,
                        traveled: 0.0,
                        age: -c.delay - 0.03 * i as f32,
                        hits: Vec::new(),
                        pierce_left: c.pierce,
                        bounce_left: c.bounce,
                        target,
                        returning: false,
                        orbit: None,
                        forked: false,
                        spent: None,
                    },
                    hand,
                ));
            }
        }
        CastType::Orbit => {
            let n = c.count.max(1);
            for i in 0..n {
                queue.spawns.push(Spawn::Missile(
                    Missile {
                        cast: info.clone(),
                        vel: Vec3::ZERO,
                        traveled: 0.0,
                        age: 0.0,
                        hits: Vec::new(),
                        pierce_left: u32::MAX,
                        bounce_left: 0,
                        target: None,
                        returning: false,
                        orbit: Some(Orbit {
                            angle: std::f32::consts::TAU * i as f32 / n as f32,
                            radius: 1.7,
                            height: 1.2,
                            rate: c.speed,
                        }),
                        forked: false,
                        spent: None,
                    },
                    body + DVec3::Y * 1.2,
                ));
            }
        }
        // A tether is a beam that links to its body on its first tick (tether.rs).
        CastType::Beam | CastType::Tether => {
            queue.spawns.push(Spawn::Beam(
                BeamCast {
                    cast: info.clone(),
                    aim: match target {
                        Some(t) => Aim::Entity(t),
                        None => Aim::Point(aim_point),
                    },
                    age: -c.delay,
                    duration: c.duration.max(0.2),
                    next_tick: 0.0,
                    end: aim_point,
                    drawn: false,
                    drawn_until: 0.0,
                    tether: None,
                },
                hand,
            ));
        }
        CastType::Chain => {
            // The first hop goes from the hand to the aimed body (or the nearest in front).
            queue.spawns.push(Spawn::Chain(ChainArc {
                cast: info.clone(),
                at: hand,
                hit: Vec::new(),
                jumps_left: c.chain.max(1) + 1,
                next_in: c.delay,
                from_reaction: false,
                first: target,
            }));
        }
        CastType::Rain => {
            let center = ground_below(queries, aim_point, &exclude);
            queue.spawns.push(Spawn::Rain(Rainfall {
                cast: info.clone(),
                center,
                age: -c.delay,
                duration: c.duration.max(0.5),
                dropped: 0,
                toward: flat,
            }));
        }
        CastType::OnSelf | CastType::Touch | CastType::Imbue => {
            if spell.cast == CastType::OnSelf {
                if let Some(me) = info.caster {
                    hits.0.push(PendingHit {
                        cast: info.clone(),
                        target: Some(me),
                        point: body + DVec3::Y,
                        normal: Vec3::Y,
                        dir: forward,
                        multi: false,
                        impact: false,
                        reacts: false,
                        expire: false,
                    });
                }
                return;
            }
            let hit = queries.shapecast(
                &ColliderShape::Sphere { radius: c.radius },
                hand,
                Quat::IDENTITY,
                dir,
                c.range,
                &exclude,
            );
            if let Some(h) = hit {
                hits.0.push(PendingHit {
                    cast: info.clone(),
                    target: Some(h.entity),
                    point: h.point,
                    normal: h.normal,
                    dir,
                    multi: false,
                    impact: true,
                    reacts: true,
                    expire: false,
                });
            }
        }
        CastType::Cone => {
            queue.spawns.push(Spawn::Field(
                Field {
                    cone: Some((dir, c.cone_deg)),
                    ..field(info, FieldKind::Burst, c.range, 0.0)
                },
                hand,
            ));
            queue.looks.push(LookCmd::Burst {
                at: hand,
                up: dir,
                look: spell.weave.key.clone(),
                role: Role::Cone,
                scale: c.range / 4.0,
                color: spell.weave.color,
            });
        }
        CastType::Wave => {
            let start = ground_below(queries, body + (flat * 1.5).as_dvec3(), &exclude);
            // Wave + stone splits the ground instead: a fault line runs (cataclysm.rs).
            if spell.payload.fault {
                queue.cataclysm.push(crate::cataclysm::NewCataclysm::Fault {
                    cast: info.clone(),
                    start,
                    dir: flat,
                });
                return;
            }
            queue.spawns.push(Spawn::Field(
                Field {
                    wave: Some((flat, c.speed, c.range)),
                    ..field(
                        info,
                        FieldKind::Lasting,
                        c.radius,
                        c.range / c.speed.max(0.1),
                    )
                },
                start,
            ));
        }
        CastType::Aura => {
            queue.spawns.push(Spawn::Field(
                Field {
                    follow: info.caster,
                    ..field(info, FieldKind::Lasting, c.radius, c.duration)
                },
                body,
            ));
        }
        CastType::Wall => {
            let at = ground_below(queries, aim_point, &exclude);
            let along = Vec3::new(-flat.z, 0.0, flat.x);
            queue.spawns.push(Spawn::Field(
                Field {
                    wall: Some((along, c.radius)),
                    ..field(info, FieldKind::Lasting, 0.8, c.duration)
                },
                at,
            ));
        }
        CastType::Rune => {
            let at = ground_below(queries, aim_point, &exclude);
            queue.spawns.push(Spawn::Field(
                field(info, FieldKind::Rune, c.radius, c.duration),
                at,
            ));
        }
        CastType::Area | CastType::Summon => {
            let at = ground_below(queries, aim_point, &exclude);
            let kind = if c.duration > 0.0 && spell.cast == CastType::Area {
                FieldKind::Lasting
            } else {
                FieldKind::Burst
            };
            queue
                .spawns
                .push(Spawn::Field(field(info, kind, c.radius, c.duration), at));
        }
    }
}

/// The pillar a stone spell raises at `at`: taller still with lift, as the list's Pillar.
pub fn pillar(spell: &Spell, at: DVec3, facing: Vec3, seed: u64, scale: f32) -> NewStone {
    use crate::stone::{StoneKind, stone_size};
    let lifted = spell
        .payload
        .force
        .is_some_and(|(d, _)| d == crate::effects::ForceDir::Up);
    // As wide as the area's heart: Size makes a broader column, not a taller one.
    let girth = (spell.carrier.radius * 0.45).clamp(1.0, 3.0);
    let mut size = stone_size(StoneKind::Pillar, spell.payload.stone_m, girth);
    size.y *= if lifted { 1.4 } else { 1.0 } * scale.clamp(0.6, 1.5);
    NewStone {
        kind: StoneKind::Pillar,
        at,
        along: facing,
        size,
        owner: None,
        life: spell.payload.stone_secs.max(4.0),
        seed,
    }
}

/// A field of `kind` from a cast.
pub fn field(info: &CastInfo, kind: FieldKind, radius: f32, duration: f32) -> Field {
    Field {
        cast: info.clone(),
        kind,
        radius,
        age: -info.spell.carrier.delay,
        duration,
        next_tick: 0.0,
        follow: None,
        cone: None,
        wall: None,
        wave: None,
        cooldowns: Vec::new(),
        met_at: -10.0,
        meets: 0,
        spent: None,
    }
}

/// The ground (or whatever solid) under a point, or the point itself.
pub fn ground_below(queries: &PhysicsQueries, p: DVec3, filter: &QueryFilter) -> DVec3 {
    queries
        .raycast(p + DVec3::Y * 2.0, Vec3::NEG_Y, 30.0, filter)
        .map(|h| h.point)
        .unwrap_or(p)
}

// ----------------------------------------------------------------------------- missiles

/// Fly missiles: steer, sweep, and queue what they hit.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn fly_missiles(
    time: Res<Time>,
    gravity: Res<Gravity>,
    queries: Res<PhysicsQueries>,
    mut missiles: Query<(Entity, &mut Missile, &mut SpellPos), Without<crate::kinesis::Caught>>,
    creatures: Query<(Entity, &GlobalTransform, Allegiance, Option<&Vitality>), CreatureFilter>,
    anchors: Query<&GlobalTransform>,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    mut traces: MessageWriter<ProjectileTrace>,
    factions: Option<Res<Factions>>,
    (mut barriers, mut struck, gates): (
        Query<(
            Entity,
            &mut crate::wards::Barrier,
            Option<&crate::wards::HeldShield>,
        )>,
        MessageWriter<crate::wards::WardStruck>,
        Query<(Entity, &crate::space::Gate, &Transform)>,
    ),
) {
    let factions = factions.as_deref();
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let doors = crate::space::doors(gates.iter());
    for (e, mut m, mut sp) in &mut missiles {
        if let Some(t) = m.spent.as_mut() {
            *t += dt;
            if *t > 1.6 {
                queue.despawn.push(e);
            }
            continue;
        }
        m.age += dt;
        let spell = m.cast.spell.clone();
        let c = spell.carrier;
        // Orbiting orbs ride their caster.
        if let Some(mut o) = m.orbit {
            if m.age > c.duration.max(1.0) {
                spend(&mut m, e, &mut queue);
                continue;
            }
            o.angle += o.rate * dt;
            m.orbit = Some(o);
            let center = m
                .cast
                .caster
                .and_then(|c| anchors.get(c).ok())
                .map(|g| g.translation())
                .unwrap_or(sp.pos);
            let at = center
                + DVec3::new(
                    (o.angle.cos() * o.radius) as f64,
                    o.height as f64,
                    (o.angle.sin() * o.radius) as f64,
                );
            sp.up = (at - sp.pos).as_vec3().normalize_or(Vec3::Y);
            sp.pos = at;
            for (cd_e, cd) in m.hits.iter_mut() {
                let _ = cd_e;
                *cd -= dt;
            }
            m.hits.retain(|(_, cd)| *cd > 0.0);
            for (other, gt, other_caster, v) in &creatures {
                if Some(other) == m.cast.caster
                    || (spell.payload.harmful()
                        && m.cast.friend(
                            other,
                            other_caster.caster,
                            other_caster.faction,
                            factions,
                        ))
                    || v.is_some_and(|v| v.down())
                    || m.hits.iter().any(|(h, _)| *h == other)
                {
                    continue;
                }
                let body = gt.translation() + DVec3::Y * 1.0;
                if body.distance(at) < (c.radius + 0.6) as f64 {
                    m.hits.push((other, 0.6));
                    hits.0.push(PendingHit {
                        cast: m.cast.clone(),
                        target: Some(other),
                        point: at,
                        normal: Vec3::Y,
                        dir: sp.up,
                        multi: true,
                        impact: false,
                        reacts: true,
                        expire: false,
                    });
                }
            }
            continue;
        }
        if m.age < 0.0 {
            // Waiting out a delay (or a volley's stagger) at the hand.
            continue;
        }
        let g = gravity.at(sp.pos);
        // Seek: turn toward the nearest live creature ahead, at a limited rate.
        if c.seek_turn > 0.0 && !m.returning {
            let pick = m.target.filter(|t| {
                creatures
                    .get(*t)
                    .is_ok_and(|(_, _, _, v)| !v.is_some_and(|v| v.down()))
            });
            let pick = pick.or_else(|| {
                let fwd = m.vel.normalize_or(Vec3::NEG_Z);
                creatures
                    .iter()
                    .filter(|(o, _, oc, v)| {
                        !m.cast.friend(*o, oc.caster, oc.faction, factions)
                            && !v.is_some_and(|v| v.down())
                    })
                    .filter_map(|(o, gt, _, _)| {
                        let to = (gt.translation() + DVec3::Y - sp.pos).as_vec3();
                        let d = to.length();
                        (d < 30.0 && to.normalize_or(fwd).dot(fwd) > 0.5).then_some((o, d))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
                    .map(|(o, _)| o)
            });
            m.target = pick;
            if let Some(t) = pick
                && let Ok((_, gt, _, _)) = creatures.get(t)
            {
                let to = (gt.translation() + DVec3::Y * 1.1 - sp.pos).as_vec3();
                let speed = m.vel.length().max(0.1);
                let want = to.normalize_or(m.vel / speed);
                let have = m.vel / speed;
                let angle = have.angle_between(want);
                let turn = (c.seek_turn * dt).min(angle);
                if angle > 1e-4 {
                    let axis = have.cross(want).normalize_or(Vec3::Y);
                    m.vel = Quat::from_axis_angle(axis, turn) * have * speed;
                }
            }
        }
        // Boomerang: back to the caster after its range.
        if m.returning
            && let Some(home) = m.cast.caster.and_then(|c| anchors.get(c).ok())
        {
            let to = (home.translation() + DVec3::Y * 1.3 - sp.pos).as_vec3();
            if to.length() < 1.0 {
                spend(&mut m, e, &mut queue);
                continue;
            }
            m.vel = to.normalize_or(Vec3::Y) * c.speed;
        }
        m.vel += g * c.gravity * dt;
        let step = m.vel * dt;
        let len = step.length();
        if len <= 1e-6 {
            continue;
        }
        let dir = step / len;
        let mut filter = m
            .cast
            .caster
            .filter(|_| m.age < 0.25 || !m.returning)
            .map(QueryFilter::excluding)
            .unwrap_or_default();
        filter.exclude.extend(m.hits.iter().map(|(h, _)| *h));
        let from = sp.pos;
        // A spell that spares its own side flies through them (a student's bolt past the golem
        // fighting beside the skeleton it is aimed at), rather than bursting harmlessly.
        if spell.payload.harmful()
            && (spell.safeguard || spell.filter == crate::design::TargetFilter::Enemies)
        {
            filter.exclude.extend(
                creatures
                    .iter()
                    .filter(|(o, gt, oc, _)| {
                        Some(*o) != m.cast.caster
                            && gt.translation().distance(from) < len as f64 + 4.0
                            && m.cast.friend(*o, oc.caster, oc.faction, factions)
                    })
                    .map(|(o, ..)| o),
            );
        }
        let hit = queries.shapecast(
            &ColliderShape::Sphere {
                radius: c.radius.max(0.05),
            },
            from,
            Quat::IDENTITY,
            dir,
            len,
            &filter,
        );
        // A portal end in the way first (space.rs): through a linked one it flies on out of
        // the other end; an end alone stops it.
        let reach = hit.map_or(len, |h| h.distance);
        let gate = (!doors.is_empty())
            .then(|| {
                let skip: Vec<Entity> = m.hits.iter().map(|(h, _)| *h).collect();
                crate::space::first_crossing(
                    &doors,
                    from,
                    from + (dir * reach).as_dvec3(),
                    c.radius * 0.5,
                    &skip,
                )
            })
            .flatten();
        let reach = gate.map_or(reach, |(_, t)| reach * t as f32);
        // A ward against it in the way first: it bursts on the ward's face (wards.rs).
        if let Some((be, t, n)) = crate::wards::barrier_in_way(
            barriers.iter().map(|(e, b, _)| (e, b)),
            &m.cast,
            from,
            dir,
            reach,
            factions,
        ) && let Ok((_, mut b, held)) = barriers.get_mut(be)
        {
            let at = from + (dir * t).as_dvec3();
            let first = m.cast.caster;
            let first_at = first
                .and_then(|c| anchors.get(c).ok())
                .map(|g| g.translation() + DVec3::Y * 1.1);
            // A shield just raised sends it back as its holder's: a perfect block.
            if let Some(h) = held
                && crate::wards::HeldShield::perfect(&b)
            {
                let face = b.kind.frame().1 * Vec3::Z;
                let aim_at = match h.aim {
                    Aim::Point(p) => p,
                    Aim::Entity(t) => anchors
                        .get(t)
                        .map(|g| g.translation() + DVec3::Y * 1.1)
                        .unwrap_or(at + (face * 30.0).as_dvec3()),
                    Aim::Direction(d) => at + (d * 30.0).as_dvec3(),
                };
                let back_to = crate::wards::reflect_target(face, at, first_at, aim_at);
                let out = at + (n * (c.radius + 0.05)).as_dvec3();
                let speed = m.vel.length().max(c.speed) * crate::wards::REFLECT_SPEED;
                let to = (back_to - out).as_vec3().normalize_or(face);
                m.vel = if c.gravity > 0.0 {
                    lob_velocity(out, back_to, speed, g.length() * c.gravity)
                } else {
                    to * speed
                };
                m.cast.caster = b.cast.caster;
                m.cast.side = b.cast.side;
                m.cast.faction = b.cast.faction;
                m.hits.clear();
                m.traveled = 0.0;
                m.returning = false;
                m.target = first.filter(|_| first_at == Some(back_to));
                struck.write(crate::wards::WardStruck {
                    ward: be,
                    barrier: true,
                    point: at,
                    dir,
                    taken: 0.0,
                    share: 0.0,
                    broke: false,
                    perfect: true,
                });
                queue.looks.push(LookCmd::Burst {
                    at,
                    up: n,
                    look: "light".into(),
                    role: Role::Impact,
                    scale: 1.4,
                    color: [1.0, 0.92, 0.7],
                });
                traces.write(ProjectileTrace {
                    from,
                    to: at,
                    radius: c.radius * 2.0,
                    speed: m.vel.length(),
                    shooter: first,
                });
                sp.pos = out;
                sp.up = m.vel.normalize_or(to);
                continue;
            }
            let (taken, share, broke) =
                b.strike(spell.payload.damage * m.cast.scale, spell.element());
            struck.write(crate::wards::WardStruck {
                ward: be,
                barrier: true,
                point: at,
                dir,
                taken,
                share,
                broke,
                perfect: false,
            });
            // An elemental ward answers a caster who strikes it from close by (burn, chill,
            // shock), as a warded body does.
            if let Some(el) = b.element
                && let Some(att) = first
                && m.cast.depth == 0
                && first_at.is_some_and(|p| p.distance(at) <= crate::wards::PAYBACK_M as f64)
            {
                hits.0.push(PendingHit {
                    cast: CastInfo {
                        spell: crate::wards::payback_spell(el),
                        caster: b.cast.caster,
                        side: b.cast.side,
                        faction: b.cast.faction,
                        depth: 1,
                        scale: 0.5,
                        seed: m.cast.seed.wrapping_mul(31).wrapping_add(7),
                        reaction: true,
                    },
                    target: Some(att),
                    point: first_at.unwrap_or(at),
                    normal: -dir,
                    dir: -dir,
                    multi: false,
                    impact: false,
                    reacts: false,
                    expire: false,
                });
            }
            queue.looks.push(LookCmd::Burst {
                at,
                up: n,
                look: spell.weave.key.clone(),
                role: Role::Impact,
                scale: (c.radius / 0.16).clamp(0.5, 2.5),
                color: spell.weave.color,
            });
            traces.write(ProjectileTrace {
                from,
                to: at,
                radius: c.radius * 2.0,
                speed: m.vel.length(),
                shooter: m.cast.caster,
            });
            sp.pos = at;
            sp.up = dir;
            spend(&mut m, e, &mut queue);
            continue;
        }
        if let Some((i, _)) = gate {
            let door = doors[i];
            let at = from + (dir * reach).as_dvec3();
            traces.write(ProjectileTrace {
                from,
                to: at,
                radius: c.radius * 2.0,
                speed: m.vel.length(),
                shooter: m.cast.caster,
            });
            sp.pos = at;
            sp.up = dir;
            match door.carry() {
                Some(map) => {
                    // A new missile comes out of the other end (its trail starts there), and
                    // never goes back into either end.
                    let (exit, frame) = door.exit.unwrap();
                    let out = crate::space::carry_point(&map, at)
                        + (frame.normal() * (c.radius + 0.05)).as_dvec3();
                    let mut through = m.clone();
                    through.vel = crate::space::capped(crate::space::carry_vector(&map, m.vel));
                    through.traveled += reach;
                    through.target = None;
                    through.hits.push((door.entity, 99.0));
                    through.hits.push((exit, 99.0));
                    queue.spawns.push(Spawn::Missile(through, out));
                    crate::space::passage_looks(
                        &mut queue,
                        &door,
                        (c.radius / 0.16).clamp(0.5, 1.5),
                    );
                }
                None => {
                    hits.0.push(PendingHit {
                        cast: m.cast.clone(),
                        target: None,
                        point: at,
                        normal: door.frame.normal(),
                        dir,
                        multi: false,
                        impact: true,
                        reacts: false,
                        expire: false,
                    });
                    queue.gate_pulses.push(door.entity);
                }
            }
            spend(&mut m, e, &mut queue);
            continue;
        }
        let to = hit
            .map(|h| from + (dir * h.distance).as_dvec3())
            .unwrap_or(from + step.as_dvec3());
        traces.write(ProjectileTrace {
            from,
            to,
            radius: c.radius * 2.0,
            speed: m.vel.length(),
            shooter: m.cast.caster,
        });
        sp.pos = to;
        sp.up = dir;
        m.traveled += len;
        if let Some(h) = hit {
            let creature = creatures.get(h.entity).is_ok();
            if creature && m.pierce_left > 0 {
                m.pierce_left -= 1;
                m.hits.push((h.entity, 99.0));
                hits.0.push(PendingHit {
                    cast: m.cast.clone(),
                    target: Some(h.entity),
                    point: h.point,
                    normal: h.normal,
                    dir,
                    multi: false,
                    impact: false,
                    reacts: true,
                    expire: false,
                });
                continue;
            }
            if c.boomerang && !m.returning {
                m.returning = true;
                m.hits.clear();
                m.hits.push((h.entity, 99.0));
                hits.0.push(PendingHit {
                    cast: m.cast.clone(),
                    target: Some(h.entity),
                    point: h.point,
                    normal: h.normal,
                    dir,
                    multi: false,
                    impact: false,
                    reacts: true,
                    expire: false,
                });
                continue;
            }
            if !creature && m.bounce_left > 0 {
                m.bounce_left -= 1;
                let n = h.normal.normalize_or(Vec3::Y);
                m.vel = (m.vel - 2.0 * m.vel.dot(n) * n) * 0.85;
                sp.pos = h.point + (n * (c.radius + 0.05)).as_dvec3();
                continue;
            }
            hits.0.push(PendingHit {
                cast: m.cast.clone(),
                target: Some(h.entity),
                point: h.point,
                normal: h.normal,
                dir,
                multi: false,
                impact: true,
                reacts: true,
                expire: false,
            });
            // A rain drop bursts where it lands and strikes what stands round it as well
            // ("thirty exploding fireballs over an army", spell-catalog.md).
            if spell.cast == CastType::Rain {
                splash(
                    &m.cast,
                    h.point,
                    Some(h.entity),
                    dir,
                    c.radius * RAIN_SPLASH,
                    &creatures,
                    factions,
                    &mut hits,
                );
            }
            // Fork: children fan out from the hit, sparing what it hit.
            if c.fork > 1 && !m.forked && m.cast.depth < MAX_STAGE_DEPTH {
                for i in 0..c.fork {
                    let k = i as f32 / (c.fork - 1) as f32 - 0.5;
                    let yaw = Quat::from_rotation_y(k * 1.2);
                    let mut child = Missile {
                        cast: CastInfo {
                            scale: m.cast.scale * 0.5,
                            seed: m.cast.seed.wrapping_add(31 * (i + 1)),
                            ..m.cast.clone()
                        },
                        vel: yaw * Vec3::new(dir.x, 0.15, dir.z).normalize_or(dir) * c.speed * 0.8,
                        traveled: 0.0,
                        age: 0.0,
                        hits: vec![(h.entity, 99.0)],
                        pierce_left: 0,
                        bounce_left: 0,
                        target: None,
                        returning: false,
                        orbit: None,
                        forked: true,
                        spent: None,
                    };
                    child.traveled = c.range * 0.6;
                    queue
                        .spawns
                        .push(Spawn::Missile(child, h.point + (h.normal * 0.3).as_dvec3()));
                }
            }
            // Chain: lightning leaps on from the body it hit.
            if c.chain > 0 && spell.cast != CastType::Chain && creature {
                queue.spawns.push(Spawn::Chain(ChainArc {
                    cast: m.cast.clone(),
                    at: h.point,
                    hit: vec![h.entity],
                    jumps_left: c.chain,
                    next_in: 0.1,
                    from_reaction: false,
                    first: None,
                }));
            }
            spend(&mut m, e, &mut queue);
            continue;
        }
        if m.traveled >= c.range {
            if c.boomerang && !m.returning {
                m.returning = true;
                m.hits.clear();
                continue;
            }
            // Out of range: a lob or a rain drop bursts where it is; a bolt fades out (and
            // fires its Expire stages).
            let lands = spell.cast == CastType::Lob || spell.cast == CastType::Rain;
            hits.0.push(PendingHit {
                cast: m.cast.clone(),
                target: None,
                point: sp.pos,
                normal: Vec3::Y,
                dir,
                multi: false,
                impact: lands,
                reacts: false,
                expire: !lands,
            });
            if spell.cast == CastType::Rain {
                splash(
                    &m.cast,
                    sp.pos,
                    None,
                    dir,
                    c.radius * RAIN_SPLASH,
                    &creatures,
                    factions,
                    &mut hits,
                );
            }
            spend(&mut m, e, &mut queue);
        }
    }
}

fn spend(m: &mut Missile, e: Entity, queue: &mut MagicQueue) {
    m.spent = Some(0.0);
    queue.looks.push(LookCmd::Stop { entity: e });
}

// ------------------------------------------------------------------------------- fields

/// Tick fields: move auras and waves, burst bursts, wake runes, tick lasting ones.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn tick_fields(
    time: Res<Time>,
    queries: Res<PhysicsQueries>,
    mut fields: Query<(Entity, &mut Field, &mut SpellPos)>,
    creatures: Query<(Entity, &GlobalTransform, Allegiance, Option<&Vitality>), CreatureFilter>,
    props: Query<(Entity, &GlobalTransform, &RigidBody), Without<Vitality>>,
    anchors: Query<&GlobalTransform>,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    mut scratch: Local<Vec<Entity>>,
    factions: Option<Res<Factions>>,
) {
    let factions = factions.as_deref();
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (e, mut f, mut sp) in &mut fields {
        if let Some(t) = f.spent.as_mut() {
            *t += dt;
            if *t > 2.0 {
                queue.despawn.push(e);
            }
            continue;
        }
        f.age += dt;
        if f.age < 0.0 {
            continue;
        }
        if let Some(follow) = f.follow
            && let Ok(g) = anchors.get(follow)
        {
            sp.pos = g.translation();
        }
        if let Some((dir, speed, _range)) = f.wave {
            let next = sp.pos + (dir * speed * dt).as_dvec3();
            sp.pos = ground_below(&queries, next, &QueryFilter::default());
        }
        for (_, cd) in f.cooldowns.iter_mut() {
            *cd -= dt;
        }
        f.cooldowns.retain(|(_, cd)| *cd > 0.0);
        let spell = f.cast.spell.clone();
        let harmful = spell.payload.harmful();
        // Who is inside.
        scratch.clear();
        for (o, gt, oc, v) in &creatures {
            if v.is_some_and(|v| v.down()) {
                continue;
            }
            let ours = f.cast.friend(o, oc.caster, oc.faction, factions);
            if harmful && (Some(o) == f.cast.caster || (ours && spell.safeguard)) {
                continue;
            }
            if !harmful && !ours && spell.filter == crate::design::TargetFilter::Allies {
                continue;
            }
            let p = gt.translation();
            let inside = if let Some((dir, half)) = f.cone {
                let to = (p + DVec3::Y - sp.pos).as_vec3();
                to.length() <= f.radius && to.normalize_or(dir).dot(dir) >= half.to_radians().cos()
            } else if let Some((along, half_len)) = f.wall {
                let d = (p - sp.pos).as_vec3();
                let a = d.dot(along).clamp(-half_len, half_len);
                let off = d - along * a;
                Vec3::new(off.x, 0.0, off.z).length() <= f.radius + 0.4 && d.y.abs() < 3.0
            } else {
                let d = p - sp.pos;
                DVec3::new(d.x, 0.0, d.z).length() <= (f.radius + 0.3) as f64 && d.y.abs() < 3.5
            };
            if inside {
                scratch.push(o);
            }
        }
        // Force spells move loose things too: crates, barrels, rubble.
        if spell.payload.force.is_some() && f.cone.is_none() && f.wall.is_none() {
            for (o, gt, body) in &props {
                if body.kind != BodyKind::Dynamic || creatures.contains(o) {
                    continue;
                }
                let d = gt.translation() - sp.pos;
                if DVec3::new(d.x, 0.0, d.z).length() <= (f.radius + 0.5) as f64 && d.y.abs() < 4.0
                {
                    scratch.push(o);
                }
            }
        }
        scratch.sort_unstable();
        scratch.dedup();
        match f.kind {
            FieldKind::Rune => {
                if f.age > 0.8 && !scratch.is_empty() {
                    f.kind = FieldKind::Burst;
                } else {
                    if f.age > f.duration {
                        f.spent = Some(0.0);
                        queue.looks.push(LookCmd::Stop { entity: e });
                    }
                    continue;
                }
            }
            FieldKind::Lasting => {}
            FieldKind::Burst => {}
        }
        if f.kind == FieldKind::Burst {
            if f.cone.is_none() {
                let look = &spell.weave.key;
                if f.age > 0.85 {
                    // A rune that was waiting: its own look goes out.
                    queue.looks.push(LookCmd::Stop { entity: e });
                }
                queue.looks.push(LookCmd::Burst {
                    at: sp.pos,
                    up: Vec3::Y,
                    look: look.clone(),
                    role: Role::Impact,
                    // Grows slower than the area, like a bolt's blast (hits.rs).
                    scale: (f.radius / 2.0).powf(0.7).clamp(0.2, 2.5),
                    color: spell.weave.color,
                });
            }
            for &o in scratch.iter() {
                let p = anchors
                    .get(o)
                    .map(|g| g.translation() + DVec3::Y * 0.5)
                    .unwrap_or(sp.pos);
                hits.0.push(PendingHit {
                    cast: f.cast.clone(),
                    target: Some(o),
                    point: p,
                    normal: Vec3::Y,
                    dir: (p - sp.pos).as_vec3().normalize_or(Vec3::Y),
                    multi: f.cast.depth > 0,
                    impact: false,
                    reacts: true,
                    expire: false,
                });
            }
            // Its own impact (stages, the world's reaction) at its center.
            hits.0.push(PendingHit {
                cast: f.cast.clone(),
                target: None,
                point: sp.pos,
                normal: Vec3::Y,
                dir: Vec3::Y,
                multi: false,
                impact: true,
                reacts: false,
                expire: false,
            });
            f.spent = Some(0.0);
            queue.looks.push(LookCmd::Stop { entity: e });
            continue;
        }
        // Lasting: tick everyone inside once per tick.
        f.next_tick -= dt;
        if f.next_tick <= 0.0 {
            f.next_tick += spell.carrier.tick.max(0.1);
            for &o in scratch.iter() {
                if f.cooldowns.iter().any(|(c, _)| *c == o) {
                    continue;
                }
                f.cooldowns.push((o, spell.carrier.tick.max(0.1) * 0.9));
                let p = anchors
                    .get(o)
                    .map(|g| g.translation() + DVec3::Y)
                    .unwrap_or(sp.pos);
                hits.0.push(PendingHit {
                    cast: f.cast.clone(),
                    target: Some(o),
                    point: p,
                    normal: Vec3::Y,
                    dir: (p - sp.pos).as_vec3().normalize_or(Vec3::Y),
                    multi: true,
                    impact: false,
                    reacts: true,
                    expire: false,
                });
            }
        }
        let done = f.age > f.duration
            || f.wave
                .is_some_and(|(_, speed, range)| f.age * speed > range);
        if done {
            f.spent = Some(0.0);
            queue.looks.push(LookCmd::Stop { entity: e });
            hits.0.push(PendingHit {
                cast: f.cast.clone(),
                target: None,
                point: sp.pos,
                normal: Vec3::Y,
                dir: Vec3::Y,
                multi: false,
                impact: false,
                reacts: false,
                expire: true,
            });
        }
    }
}

// -------------------------------------------------------------------------------- beams

/// Hold beams on their aim and hit what they touch each tick.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn tick_beams(
    time: Res<Time>,
    queries: Res<PhysicsQueries>,
    mut beams: Query<(Entity, &mut BeamCast, &mut SpellPos)>,
    casters: Query<(&GlobalTransform, &MagicCaster)>,
    anchors: Query<&GlobalTransform>,
    creatures: Query<(), CreatureFilter>,
    bodies: Query<&RigidBody>,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    (mut barriers, mut struck, factions, gates): (
        Query<(Entity, &mut crate::wards::Barrier)>,
        MessageWriter<crate::wards::WardStruck>,
        Option<Res<Factions>>,
        Query<(Entity, &crate::space::Gate, &Transform)>,
    ),
) {
    let factions = factions.as_deref();
    let dt = time.delta_secs();
    let doors = if beams.is_empty() {
        Vec::new()
    } else {
        crate::space::doors(gates.iter())
    };
    for (e, mut b, mut sp) in &mut beams {
        b.age += dt;
        if b.age < 0.0 {
            continue;
        }
        let spell = b.cast.spell.clone();
        let from = b
            .cast
            .caster
            .and_then(|c| casters.get(c).ok())
            .map(|(g, c)| hand_of(g, c))
            .unwrap_or(sp.pos);
        sp.pos = from;
        if spell.cast == CastType::Tether {
            // It links to its body on its first tick: something alive or loose, never the
            // floor. With nothing there it fizzles.
            if b.tether.is_none() {
                let (aimed, point) = match b.aim {
                    Aim::Entity(t) => match anchors.get(t) {
                        // A body out of reach can't be linked (it would snap at once).
                        Ok(g) => {
                            let at = g.translation() + DVec3::Y * 1.1;
                            let near = at.distance(from) <= spell.carrier.range as f64;
                            (near.then_some(t), at)
                        }
                        Err(_) => (None, b.end),
                    },
                    Aim::Point(p) => (None, p),
                    Aim::Direction(d) => (None, from + (d * spell.carrier.range).as_dvec3()),
                };
                let exclude = b
                    .cast
                    .caster
                    .map(QueryFilter::excluding)
                    .unwrap_or_default();
                let linked = crate::tether::link_target(
                    &queries,
                    from,
                    point,
                    aimed,
                    spell.carrier.range,
                    &exclude,
                    |x| {
                        creatures.contains(x)
                            || bodies.get(x).is_ok_and(|r| r.kind == BodyKind::Dynamic)
                    },
                );
                match linked {
                    Some(t) => {
                        b.tether = Some(crate::tether::TetherLink::new(t, spell.carrier.range));
                        b.aim = Aim::Entity(t);
                    }
                    None => {
                        queue.looks.push(LookCmd::Burst {
                            at: point,
                            up: Vec3::Y,
                            look: spell.weave.key.clone(),
                            role: Role::Fizzle,
                            scale: 0.5,
                            color: spell.weave.color,
                        });
                        queue.despawn.push(e);
                        continue;
                    }
                }
            }
            let to = b
                .tether
                .and_then(|l| anchors.get(l.target).ok())
                .map(|g| g.translation() + DVec3::Y * 1.1);
            let over = crate::tether::tick_tether(
                &mut b,
                from,
                to,
                dt,
                &queries,
                |x| creatures.contains(x),
                &mut hits,
                &mut queue,
            );
            if over {
                queue.despawn.push(e);
            }
            continue;
        }
        let aim = match b.aim {
            Aim::Entity(t) => anchors
                .get(t)
                .map(|g| g.translation() + DVec3::Y * 1.1)
                .unwrap_or(b.end),
            Aim::Point(p) => p,
            Aim::Direction(d) => from + (d * spell.carrier.range).as_dvec3(),
        };
        let dir = (aim - from).as_vec3().normalize_or(Vec3::NEG_Z);
        let filter = b
            .cast
            .caster
            .map(QueryFilter::excluding)
            .unwrap_or_default();
        // Its way: from the hand until it hits something, a ward or a portal end; through a
        // linked end it bends on out of the other one for the rest of its range (Portal 2's
        // laser); an end alone stops it.
        let mut start = from;
        let mut dir = dir;
        let mut range = spell.carrier.range;
        let mut bent: Option<(DVec3, DVec3)> = None;
        let (hit, ward, stop) = loop {
            let hit = queries.raycast(start, dir, range, &filter);
            let reach = hit.map_or(range, |h| h.distance);
            let gate = (!doors.is_empty() && bent.is_none())
                .then(|| {
                    crate::space::first_crossing(
                        &doors,
                        start,
                        start + (dir * reach).as_dvec3(),
                        0.0,
                        &[],
                    )
                })
                .flatten();
            let reach = gate.map_or(reach, |(_, t)| reach * t as f32);
            // A ward against it in the way stops it at its face and drains on every tick.
            let ward =
                crate::wards::barrier_in_way(barriers.iter(), &b.cast, start, dir, reach, factions);
            if let Some((i, _)) = gate
                && ward.is_none()
            {
                let door = doors[i];
                let at = start + (dir * reach).as_dvec3();
                let Some(map) = door.carry() else {
                    break (None, None, Some(at));
                };
                let out = crate::space::carry_point(&map, at);
                bent = Some((at, out));
                start = out;
                dir = crate::space::carry_vector(&map, dir).normalize_or(dir);
                range = (range - reach).max(0.0);
                crate::space::pulse_through(&mut queue, &door);
                continue;
            }
            let hit = if ward.is_some() { None } else { hit };
            break (hit, ward, None);
        };
        b.end = match (ward, stop) {
            (Some((_, t, _)), _) => start + (dir * t).as_dvec3(),
            (None, Some(at)) => at,
            _ => hit
                .map(|h| h.point)
                .unwrap_or(start + (dir * range).as_dvec3()),
        };
        // Drawn once for its whole duration; a channel held past that (the spell bar keeps
        // extending it) draws again every half second, from where the hand is now.
        let held_on = b.duration > b.drawn_until + 0.1 && b.age + 0.05 >= b.drawn_until;
        if !b.drawn || held_on {
            let secs = if b.drawn {
                0.5
            } else {
                (b.duration - b.age).max(0.1)
            };
            b.drawn = true;
            b.drawn_until = b.age + secs;
            let pieces = match bent {
                Some((into, out)) => vec![(from, into), (out, b.end)],
                None => vec![(from, b.end)],
            };
            for (a, z) in pieces {
                queue.looks.push(LookCmd::Beam {
                    from: a,
                    to: z,
                    look: spell.weave.key.clone(),
                    color: spell.weave.color,
                    width: (spell.carrier.radius * 1.5).max(0.08),
                    secs,
                });
            }
        }
        b.next_tick -= dt;
        if b.next_tick <= 0.0 {
            b.next_tick += spell.carrier.tick.max(0.05);
            if let Some((be, _, _)) = ward
                && let Ok((_, mut w)) = barriers.get_mut(be)
            {
                let (taken, share, broke) =
                    w.strike(spell.payload.damage * b.cast.scale, spell.element());
                struck.write(crate::wards::WardStruck {
                    ward: be,
                    barrier: true,
                    point: b.end,
                    dir,
                    taken,
                    share,
                    broke,
                    perfect: false,
                });
            }
            if let Some(h) = hit {
                hits.0.push(PendingHit {
                    cast: b.cast.clone(),
                    target: Some(h.entity),
                    point: h.point,
                    normal: h.normal,
                    dir,
                    multi: true,
                    impact: false,
                    reacts: true,
                    expire: false,
                });
            }
        }
        if b.age >= b.duration {
            if let Some(h) = hit {
                hits.0.push(PendingHit {
                    cast: b.cast.clone(),
                    target: Some(h.entity),
                    point: h.point,
                    normal: h.normal,
                    dir,
                    multi: false,
                    impact: true,
                    reacts: false,
                    expire: false,
                });
            }
            queue.despawn.push(e);
        }
    }
}

// ------------------------------------------------------------------------------- chains

/// Hop chains on to the nearest creature not yet hit, a tenth of a second apart.
#[allow(clippy::type_complexity)]
pub fn tick_chains(
    time: Res<Time>,
    mut chains: Query<(Entity, &mut ChainArc)>,
    creatures: Query<
        (
            Entity,
            &GlobalTransform,
            Allegiance,
            Option<&Vitality>,
            Option<&Afflictions>,
        ),
        CreatureFilter,
    >,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    factions: Option<Res<Factions>>,
) {
    let factions = factions.as_deref();
    let dt = time.delta_secs();
    for (e, mut ch) in &mut chains {
        ch.next_in -= dt;
        if ch.next_in > 0.0 {
            continue;
        }
        let spell = ch.cast.spell.clone();
        let reach = if ch.hit.is_empty() {
            spell.carrier.range.max(spell.carrier.chain_radius)
        } else {
            spell.carrier.chain_radius
        };
        // Fatal bonds bind foes' fates, never the warlock's own.
        let bonds = spell.payload.bond_secs > 0.0;
        // The first hop goes to the aimed body when it is in reach; then prefer the shocked and
        // the bleeding (blood lightning), then the nearest.
        let aimed = ch.first.filter(|_| ch.hit.is_empty());
        let next = creatures
            .iter()
            .filter(|(o, _, oc, v, _)| {
                Some(*o) != ch.cast.caster
                    && !ch.hit.contains(o)
                    && !v.is_some_and(|v| v.down())
                    && !((spell.safeguard || bonds)
                        && ch.cast.friend(*o, oc.caster, oc.faction, factions))
            })
            .filter_map(|(o, gt, _, _, aff)| {
                let d = (gt.translation() + DVec3::Y - ch.at).length() as f32;
                let primed = aff.is_some_and(|a| a.primed_for_chain());
                let first = if aimed == Some(o) { 1.0e4 } else { 0.0 };
                (d <= reach).then_some((
                    o,
                    d - if primed { 4.0 } else { 0.0 } - first,
                    gt.translation(),
                ))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let Some((o, _, p)) = next else {
            queue.despawn.push(e);
            continue;
        };
        let to = p + DVec3::Y * 1.1;
        queue.looks.push(LookCmd::Beam {
            from: ch.at,
            to,
            look: spell.weave.key.clone(),
            color: spell.weave.color,
            width: 0.12,
            secs: 0.25,
        });
        hits.0.push(PendingHit {
            cast: CastInfo {
                scale: ch.cast.scale * if ch.hit.is_empty() { 1.0 } else { 0.8 },
                ..ch.cast.clone()
            },
            target: Some(o),
            point: to,
            normal: Vec3::Y,
            dir: (to - ch.at).as_vec3().normalize_or(Vec3::Y),
            multi: !ch.hit.is_empty(),
            impact: false,
            reacts: !ch.from_reaction,
            expire: false,
        });
        // Fatal bonds: every body it hops to is chained to the one before (chains.rs).
        if spell.payload.bond_secs > 0.0 {
            queue.bond_hops.push(crate::chains::BondHop {
                cast: ch.cast.clone(),
                to: o,
            });
        }
        ch.cast.scale *= if ch.hit.is_empty() { 1.0 } else { 0.8 };
        ch.hit.push(o);
        ch.at = to;
        ch.next_in = 0.1;
        ch.jumps_left = ch.jumps_left.saturating_sub(1);
        if ch.jumps_left == 0 {
            queue.despawn.push(e);
        }
    }
}

// --------------------------------------------------------------------------------- rain

/// Drop rain over its area: missiles that fall from the sky.
pub fn tick_rain(
    time: Res<Time>,
    mut rains: Query<(Entity, &mut Rainfall)>,
    mut queue: ResMut<MagicQueue>,
) {
    let dt = time.delta_secs();
    for (e, mut r) in &mut rains {
        r.age += dt;
        if r.age < 0.0 {
            continue;
        }
        let spell = r.cast.spell.clone();
        let c = spell.carrier;
        let due = ((r.age / r.duration).min(1.0) * c.count as f32).ceil() as u32;
        let mut drop = None;
        while r.dropped < due {
            let i = r.dropped;
            r.dropped += 1;
            let seed = r.cast.seed as u64 ^ (i as u64) << 20;
            let a = roll(seed, 11) * std::f32::consts::TAU;
            let d = roll(seed, 12).sqrt() * c.radius;
            let ground = r.center + DVec3::new((a.cos() * d) as f64, 0.0, (a.sin() * d) as f64);
            // Rain + stone drops meteors: the first right on the aim, the rest of a swarm round
            // it (cataclysm.rs).
            if spell.payload.meteor_m > 0.0 {
                queue
                    .cataclysm
                    .push(crate::cataclysm::NewCataclysm::Meteor {
                        cast: CastInfo {
                            seed: r.cast.seed.wrapping_add(i * 104_729),
                            ..r.cast.clone()
                        },
                        ground: if i == 0 { r.center } else { ground },
                        toward: r.toward,
                    });
                continue;
            }
            let high = ground + DVec3::new(-3.0, 16.0 + 4.0 * roll(seed, 13) as f64, 2.0);
            let vel = (ground - high).as_vec3().normalize_or(Vec3::NEG_Y) * c.speed;
            let drop = drop.get_or_insert_with(|| drop_of(&spell)).clone();
            queue.spawns.push(Spawn::Missile(
                Missile {
                    cast: CastInfo {
                        seed: r.cast.seed.wrapping_add(i * 104_729),
                        spell: drop,
                        ..r.cast.clone()
                    },
                    vel,
                    traveled: 0.0,
                    age: 0.0,
                    hits: Vec::new(),
                    pierce_left: 0,
                    bounce_left: 0,
                    target: None,
                    returning: false,
                    orbit: None,
                    forked: true,
                    spent: None,
                },
                high,
            ));
        }
        if r.age >= r.duration {
            queue.despawn.push(e);
        }
    }
}

/// A rain's drop as it falls: the rain's spell with a drop's girth, a fifth of a metre at size
/// 1 (a lob's), since the rain's own carrier radius is the patch of ground the drops come down
/// over. A drop swept with that radius (14 m for an Ascended's rain) struck anything within it
/// from 14 m up, burst at the largest scale and cost a millisecond or more of physics a step.
pub fn drop_of(spell: &Spell) -> Arc<Spell> {
    let mut s = spell.clone();
    s.carrier.radius *= RAIN_DROP_SHARE;
    Arc::new(s)
}

/// A rain drop's radius as a share of the rain's (5 m at size 1).
pub const RAIN_DROP_SHARE: f32 = 0.04;
/// How far round where a rain drop lands it strikes, in drop radii: 1.6 m at size 1, 4.5 m for
/// an Ascended's rain.
pub const RAIN_SPLASH: f32 = 8.0;

/// A drop's splash: a hit on each creature standing within `reach` of where it landed (across
/// the ground, and up to a body's height above or below), other than the one it struck and
/// those the spell spares. Hits checks the filters again; this only keeps the queue short.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn splash(
    cast: &CastInfo,
    at: DVec3,
    struck: Option<Entity>,
    dir: Vec3,
    reach: f32,
    creatures: &Query<(Entity, &GlobalTransform, Allegiance, Option<&Vitality>), CreatureFilter>,
    factions: Option<&Factions>,
    hits: &mut HitQueue,
) {
    let spell = &cast.spell;
    let harmful = spell.payload.harmful();
    for (o, gt, oc, v) in creatures {
        if Some(o) == struck || v.is_some_and(|v| v.down()) {
            continue;
        }
        if harmful
            && (Some(o) == cast.caster
                || (spell.safeguard && cast.friend(o, oc.caster, oc.faction, factions)))
        {
            continue;
        }
        let body = gt.translation() + DVec3::Y;
        let d = body - at;
        if DVec3::new(d.x, 0.0, d.z).length() > reach as f64 || d.y.abs() > 2.5 {
            continue;
        }
        hits.0.push(PendingHit {
            cast: cast.clone(),
            target: Some(o),
            point: body,
            normal: Vec3::Y,
            dir: d.as_vec3().normalize_or(dir),
            multi: true,
            impact: false,
            reacts: true,
            expire: false,
        });
    }
}

// ------------------------------------------------------------------------------ follows

/// A status look riding on a body: flames on the burning, frost motes on the chilled, an ice
/// shell on the frozen, sparks on the shocked. Fades once the status ends.
#[derive(Component, Debug, Clone)]
pub struct FollowLook {
    pub owner: Entity,
    pub status: Status,
    /// Seconds since the status ended, or None while it lasts.
    pub fading: Option<f32>,
    /// Seconds to the next spark (shocked).
    pub next_arc: f32,
}

impl FollowLook {
    pub fn new(owner: Entity, status: Status) -> Self {
        Self {
            owner,
            status,
            fading: None,
            next_arc: 0.0,
        }
    }

    /// Height above the owner's feet.
    pub fn height(&self) -> f64 {
        match self.status {
            Status::Frozen => 0.95,
            Status::Burning => 0.7,
            _ => 1.1,
        }
    }
}

/// Keep status looks on their bodies; spark the shocked; let them go when the status ends.
#[allow(clippy::type_complexity)]
pub fn follow_looks(
    time: Res<Time>,
    mut looks: Query<(
        Entity,
        &mut FollowLook,
        &mut SpellPos,
        Option<&mut MeshRenderer>,
        Option<&mut engine_world::SpellLook>,
    )>,
    owners: Query<(&GlobalTransform, Option<&Afflictions>)>,
    mut queue: ResMut<MagicQueue>,
) {
    let dt = time.delta_secs();
    for (e, mut f, mut sp, mesh, look) in &mut looks {
        if let Some(t) = f.fading.as_mut() {
            *t += dt;
            if *t > 1.5 {
                queue.despawn.push(e);
            }
            continue;
        }
        let owner = owners.get(f.owner).ok();
        let on = owner.is_some_and(|(_, a)| a.is_some_and(|a| a.has(f.status)));
        if let Some((g, _)) = owner {
            sp.pos = g.translation() + DVec3::Y * f.height();
        }
        if !on {
            f.fading = Some(0.0);
            queue.looks.push(LookCmd::Stop { entity: e });
            if let Some(mut m) = mesh {
                m.visible = false;
                if f.status == Status::Frozen {
                    // The ice shatters.
                    queue.looks.push(LookCmd::Burst {
                        at: sp.pos,
                        up: Vec3::Y,
                        look: "frost".into(),
                        role: Role::Impact,
                        scale: 1.4,
                        color: Element::Frost.color(),
                    });
                }
            }
            continue;
        }
        // The look's scale follows how bad the status is (vfx restyles on a changed scale):
        // a lick of flame for a short burn, a blaze for a long one.
        if let (Some(mut look), Some((_, Some(a)))) = (look, owner) {
            let want = status_scale(f.status, &a.statuses());
            if (look.scale - want).abs() > 0.05 {
                look.scale = want;
            }
        }
        if f.status == Status::Shocked {
            f.next_arc -= dt;
            if f.next_arc <= 0.0 {
                f.next_arc = 0.11;
                let k = (time.elapsed_secs() * 37.0).sin();
                let a = Vec3::new(k * 0.5, 0.6, (1.0 - k.abs()) * 0.5);
                let b = Vec3::new(-k * 0.4, -0.7, -(1.0 - k.abs()) * 0.4);
                queue.looks.push(LookCmd::Beam {
                    from: sp.pos + a.as_dvec3(),
                    to: sp.pos + b.as_dvec3(),
                    look: "storm".into(),
                    color: Element::Storm.color(),
                    width: 0.05,
                    secs: 0.09,
                });
            }
        }
    }
}

/// A status look's size from the engine's view of the status: 0.5 at its least, 1.2 at full
/// (burning, bleeding, rot and dread by level; chill by stacks toward the freeze at 5; frozen
/// and shocked stay 1).
pub fn status_scale(status: Status, st: &engine_world::Statuses) -> f32 {
    let level = match status {
        Status::Burning => st.burning,
        Status::Bleeding => st.bleeding,
        Status::Rot => st.rot,
        Status::Dread => st.dread,
        Status::Chilled => st.chill as f32 / 5.0,
        // On or off: the look's own size.
        Status::Frozen | Status::Shocked => return 1.0,
    };
    0.5 + 0.7 * level.clamp(0.0, 1.0)
}

/// A look standing along a wall, going out with it (and changing with it when spells meet).
#[derive(Component, Debug, Clone)]
pub struct LookPiece {
    pub owner: Entity,
    pub look: String,
    pub color: [f32; 3],
    pub fading: Option<f32>,
    /// The piece before it along the wall (storm arcs leap to it).
    pub prev: Option<Entity>,
    /// The wall's direction and this piece's share of its length, m.
    pub along: Vec3,
    pub width: f32,
    /// Its wall is stone: the rock is the look, so no particle look is attached.
    pub stone: bool,
}

/// How big a wall piece's particle look is: a fire curtain stands tall.
fn wall_look_scale(look: &str) -> f32 {
    if crate::walls::stuff(look) == crate::walls::Stuff::Coals {
        1.7
    } else {
        1.0
    }
}

/// Keep wall pieces in step with their wall.
pub fn tend_pieces(
    time: Res<Time>,
    mut pieces: Query<(Entity, &mut LookPiece)>,
    fields: Query<&Field>,
    mut queue: ResMut<MagicQueue>,
) {
    let dt = time.delta_secs();
    for (e, mut p) in &mut pieces {
        if let Some(t) = p.fading.as_mut() {
            *t += dt;
            if *t > 2.0 {
                queue.despawn.push(e);
            }
            continue;
        }
        match fields.get(p.owner) {
            Ok(f) if f.spent.is_none() => {
                if f.cast.spell.weave.key != p.look && !p.stone {
                    p.look = f.cast.spell.weave.key.clone();
                    p.color = f.cast.spell.weave.color;
                    queue.looks.push(LookCmd::Attach {
                        entity: e,
                        look: p.look.clone(),
                        role: Role::Wall,
                        scale: wall_look_scale(&p.look),
                        color: p.color,
                        power_j: f.cast.spell.power.energy_j as f32,
                    });
                }
            }
            _ => {
                p.fading = Some(0.0);
                queue.looks.push(LookCmd::Stop { entity: e });
            }
        }
    }
}

// -------------------------------------------------------------------------------- flush

/// The step's one system with `Commands`: spawn carriers, insert components, despawn the spent,
/// move blinkers, and hand the looks to the particles module.
pub fn flush(
    mut commands: Commands,
    mut queue: ResMut<MagicQueue>,
    mut starts: ResMut<CastStarts>,
    mut freezes: MessageWriter<FreezeRequest>,
    mut growth: MessageWriter<GrowthPulse>,
) {
    for (e, casting) in starts.0.drain(..) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.try_insert(casting);
        }
    }
    let mut looks = Vec::with_capacity(queue.looks.len());
    for l in std::mem::take(&mut queue.looks) {
        match l {
            LookCmd::RemoveCasting { caster } => {
                if let Ok(mut ec) = commands.get_entity(caster) {
                    ec.try_remove::<Casting>();
                }
            }
            l => looks.push(l),
        }
    }
    let mut pieces = Vec::new();
    for s in std::mem::take(&mut queue.spawns) {
        let (name, pos, up) = match &s {
            Spawn::Missile(m, at) => ("Spell missile", *at, m.vel.normalize_or(Vec3::Y)),
            Spawn::Field(f, at) => ("Spell field", *at, f.cone.map(|c| c.0).unwrap_or(Vec3::Y)),
            Spawn::Beam(_, at) => ("Spell beam", *at, Vec3::Y),
            Spawn::Chain(c) => ("Spell chain", c.at, Vec3::Y),
            Spawn::Rain(r) => ("Spell rain", r.center, Vec3::Y),
            Spawn::Follow(f, at) => ("Spell status", *at + DVec3::Y * f.height(), Vec3::Y),
        };
        let mut ec = commands.spawn((
            Name::new(name),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_arc(Vec3::Y, up)),
            SpellPos { pos, up },
        ));
        let id = ec.id();
        match s {
            Spawn::Missile(m, _) => {
                // A glintblade: the sword is drawn on the missile, its element a trail
                // behind it (half a bolt's look, so the blade reads).
                let blade = m.cast.spell.payload.blades > 0;
                if blade {
                    ec.insert(crate::blades::BladeRide {
                        length: m.cast.spell.payload.blade_m.max(0.5),
                        color: m.cast.spell.weave.color,
                        drawn: None,
                        broke: false,
                    });
                }
                // A hook pays out its chain behind it (chains.rs); a demonic meteor is a rock
                // of black iron and fire (demons.rs).
                if m.cast.spell.payload.hook && !m.forked {
                    ec.insert(crate::chains::HookRide::default());
                }
                if m.cast.spell.payload.demons > 0 {
                    ec.insert(crate::demons::MeteorRide::default());
                }
                looks.push(LookCmd::Attach {
                    entity: id,
                    look: m.cast.spell.weave.key.clone(),
                    role: Role::Bolt,
                    scale: (m.cast.spell.carrier.radius / 0.16 * m.cast.scale.sqrt())
                        .clamp(0.2, 30.0)
                        * if blade { 0.5 } else { 1.0 },
                    color: m.cast.spell.weave.color,
                    power_j: m.cast.spell.power.energy_j as f32 * m.cast.scale,
                });
                ec.insert(m);
            }
            Spawn::Field(f, _) => {
                // Lasting fields, auras and runes show a look on the entity; bursts show
                // theirs where they go off (tick_fields). A wall gets pieces along its length.
                if f.kind != FieldKind::Burst {
                    let role = if f.follow.is_some() {
                        Role::Aura
                    } else {
                        Role::Field
                    };
                    let look = f.cast.spell.weave.key.clone();
                    let color = f.cast.spell.weave.color;
                    let power_j = f.cast.spell.power.energy_j as f32 * f.cast.scale;
                    // A lasting frost field or wall freezes the water under it for as long
                    // as it stands and a while after; life and blight fields work on plants
                    // (auras and waves move on, so they leave the ground alone).
                    let still = f.follow.is_none() && f.wave.is_none();
                    let freeze = (still && f.cast.spell.element() == Some(Element::Frost))
                        .then_some(f.duration + 10.0);
                    let grow = crate::hits::growth_of(&f.cast.spell, f.cast.scale);
                    if still && grow != 0.0 {
                        growth.write(GrowthPulse {
                            center: pos,
                            radius_m: f.radius.max(1.0),
                            amount: grow,
                            secs: f.duration.max(2.0),
                            source: f.cast.caster,
                        });
                    }
                    if let Some((along, half)) = f.wall {
                        // A wall shows only along its pieces.
                        let pay = &f.cast.spell.payload;
                        pieces.push(NewWall {
                            owner: id,
                            at: pos,
                            along,
                            half,
                            look,
                            color,
                            power_j,
                            freeze,
                            source: f.cast.caster,
                            stone: (pay.stone_m > 0.0).then_some(pay.stone_m),
                        });
                    } else {
                        if let Some(secs) = freeze {
                            freezes.write(FreezeRequest {
                                center: pos,
                                radius_m: f.radius.max(1.0),
                                secs,
                                source: f.cast.caster,
                            });
                        }
                        looks.push(LookCmd::Attach {
                            entity: id,
                            look: look.clone(),
                            role,
                            scale: (f.radius / 2.0).clamp(0.2, 30.0),
                            color,
                            power_j,
                        });
                    }
                }
                ec.insert(f);
            }
            Spawn::Beam(b, _) => {
                ec.insert(b);
            }
            Spawn::Chain(c) => {
                ec.insert(c);
            }
            Spawn::Rain(r) => {
                ec.insert(r);
            }
            Spawn::Follow(f, _) => {
                let s = f.status;
                looks.push(LookCmd::Attach {
                    entity: id,
                    look: s.key().into(),
                    role: Role::Status,
                    scale: 1.0,
                    color: crate::looks::status_color(s),
                    power_j: 0.0,
                });
                if s == Status::Frozen {
                    ec.insert(crate::looks::ice_shell());
                }
                ec.insert(f);
            }
        }
    }
    for NewWall {
        owner,
        at,
        along,
        half,
        look,
        color,
        power_j,
        freeze,
        source,
        stone,
    } in pieces
    {
        let n = ((2.0 * half / 1.5).ceil() as i32).clamp(2, 12);
        let width = 2.0 * half / n as f32;
        let mut prev = None;
        for k in 0..=n {
            let x = -half + 2.0 * half * k as f32 / n as f32;
            let p = at + (along * x).as_dvec3();
            if let Some(secs) = freeze {
                freezes.write(FreezeRequest {
                    center: p,
                    radius_m: width * 0.8,
                    secs,
                    source,
                });
            }
            let id = commands
                .spawn((
                    Name::new("Spell wall piece"),
                    Transform::from_translation(p),
                    LookPiece {
                        owner,
                        look: look.clone(),
                        color,
                        fading: None,
                        prev,
                        along,
                        width,
                        stone: stone.is_some(),
                    },
                ))
                .id();
            prev = Some(id);
            // From where it stands, never from entity ids.
            let seed = p.x.to_bits() ^ p.z.to_bits().rotate_left(17) ^ k as u64;
            if let Some(stone_m) = stone {
                // A block of rough stone per piece: real, solid, and gone with its piece.
                use crate::stone::{StoneKind, stone_size};
                crate::stone::spawn_stone(
                    &mut commands,
                    NewStone {
                        kind: StoneKind::Wall,
                        at: p,
                        along,
                        size: stone_size(StoneKind::Wall, stone_m, width),
                        owner: Some(id),
                        life: f32::INFINITY,
                        seed,
                    },
                );
                continue;
            }
            looks.push(LookCmd::Attach {
                entity: id,
                look: look.clone(),
                role: Role::Wall,
                scale: wall_look_scale(&look),
                color,
                power_j,
            });
            crate::walls::spawn_body(
                &mut commands,
                crate::walls::NewBody {
                    piece: id,
                    look: look.clone(),
                    color,
                    along,
                    width,
                    seed,
                },
            );
        }
    }
    for s in std::mem::take(&mut queue.stones) {
        crate::stone::spawn_stone(&mut commands, s);
    }
    for m in std::mem::take(&mut queue.minions) {
        crate::summon::spawn_minion(&mut commands, m, &mut looks);
    }
    for (e, size) in queue.solids.drain(..) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.try_insert(crate::stone::solid(size));
        }
    }
    for b in std::mem::take(&mut queue.wall_bodies) {
        crate::walls::spawn_body(&mut commands, b);
    }
    for e in queue.despawn.drain(..) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.try_despawn();
        }
    }
    for (e, a) in queue.afflict.drain(..) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.try_insert_if_new(a);
        }
    }
    for (e, st) in queue.statuses.drain(..) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.try_insert(st);
        }
    }
    for (e, scale) in queue.time_scales.drain(..) {
        if let Ok(mut ec) = commands.get_entity(e) {
            match scale {
                Some(s) => {
                    ec.try_insert(s);
                }
                None => {
                    ec.try_remove::<LocalTimeScale>();
                }
            }
        }
    }
    let moves = std::mem::take(&mut queue.moves);
    if !looks.is_empty() || !moves.is_empty() {
        commands.queue(move |world: &mut World| {
            for (e, to) in moves {
                if let Some(mut t) = world.get_mut::<Transform>(e) {
                    t.translation = to;
                }
            }
            let looks = crate::looks::own_or_stone(world, looks);
            crate::looks::write_contracts(world, &looks);
            crate::looks::apply(world, looks);
        });
    }
}

/// A wall to stand up along its pieces (from the flush).
struct NewWall {
    owner: Entity,
    at: DVec3,
    along: Vec3,
    half: f32,
    look: String,
    color: [f32; 3],
    power_j: f32,
    /// Seconds to freeze the water under it (frost walls).
    freeze: Option<f32>,
    source: Option<Entity>,
    /// Height of its stone blocks, m (a stone wall).
    stone: Option<f32>,
}

/// Copy carriers' positions into their `Transform`s for drawing.
pub fn sync_transforms(mut q: Query<(&SpellPos, &mut Transform), Changed<SpellPos>>) {
    for (sp, mut t) in &mut q {
        t.translation = sp.pos;
        t.rotation = Quat::from_rotation_arc(Vec3::Y, sp.up.normalize_or(Vec3::Y));
    }
}

/// Carriers keep world positions of their own; move them when the world re-anchors.
#[allow(clippy::type_complexity)]
pub fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut q: ParamSet<(
        Query<&mut SpellPos>,
        Query<&mut Missile>,
        Query<&mut Field>,
        Query<&mut BeamCast>,
        Query<&mut ChainArc>,
        Query<&mut Rainfall>,
        Query<&mut Afflictions>,
        Query<&mut crate::stone::StoneBody>,
    )>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    for mut p in &mut q.p0() {
        p.pos = s.point(p.pos);
        p.up = s.direction(p.up);
    }
    for mut m in &mut q.p1() {
        m.vel = s.direction(m.vel);
    }
    for mut f in &mut q.p2() {
        if let Some((d, h)) = f.cone {
            f.cone = Some((s.direction(d), h));
        }
        if let Some((d, h)) = f.wall {
            f.wall = Some((s.direction(d), h));
        }
        if let Some((d, v, r)) = f.wave {
            f.wave = Some((s.direction(d), v, r));
        }
    }
    for mut b in &mut q.p3() {
        b.end = s.point(b.end);
        if let Aim::Point(p) = b.aim {
            b.aim = Aim::Point(s.point(p));
        }
    }
    for mut c in &mut q.p4() {
        c.at = s.point(c.at);
    }
    for mut r in &mut q.p5() {
        r.center = s.point(r.center);
        r.toward = s.direction(r.toward);
    }
    for mut a in &mut q.p6() {
        a.last_pos = a.last_pos.map(|p| s.point(p));
    }
    for mut st in &mut q.p7() {
        st.base = s.point(st.base);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lobs_land_where_aimed() {
        let from = DVec3::new(0.0, 1.5, 0.0);
        let to = DVec3::new(0.0, 0.0, -20.0);
        let g = 9.81;
        let v = lob_velocity(from, to, 17.0, g);
        // Integrate until it comes back down to the target's height.
        let (mut p, mut vel, dt) = (from, v, 1.0 / 240.0);
        while vel.y > 0.0 || p.y > to.y {
            p += (vel * dt).as_dvec3();
            vel.y -= g * dt;
        }
        assert!(p.distance(to) < 0.6, "{p}");
        // Out of reach: thrown at 45 degrees.
        let far = lob_velocity(from, DVec3::new(0.0, 0.0, -500.0), 17.0, g);
        assert!((far.y - far.z.abs()).abs() < 0.01);
    }

    #[test]
    fn rolls_are_even_and_repeatable() {
        let n = 10_000;
        let mean: f32 = (0..n).map(|i| roll(42, i)).sum::<f32>() / n as f32;
        assert!((mean - 0.5).abs() < 0.02);
        assert_eq!(roll(7, 3), roll(7, 3));
        assert_ne!(roll(7, 3), roll(8, 3));
    }

    #[test]
    fn factions_decide_friends_when_both_have_one() {
        use crate::compile::{CastContext, compile};
        use crate::design::SpellDesign;
        use crate::schools::{Mastery, Power};
        let spell = compile(
            &SpellDesign::new(&["fire"], CastType::Bolt),
            CastContext::new(Mastery::Adept, Power::default()),
        )
        .expect("compiles");
        let mut world = World::new();
        let (me, other) = (world.spawn_empty().id(), world.spawn_empty().id());
        let cast = |faction| CastInfo {
            spell: spell.clone(),
            caster: Some(me),
            side: 1,
            faction,
            depth: 0,
            scale: 1.0,
            seed: 0,
            reaction: false,
        };
        let same_side = MagicCaster {
            side: 1,
            ..MagicCaster::new(10.0, Mastery::Adept)
        };
        let factions = engine_world::faction::default_factions();
        // No factions: magic's sides.
        assert!(cast(None).friend(other, Some(&same_side), None, Some(&factions)));
        assert!(!cast(None).friend(other, None, None, Some(&factions)));
        assert!(
            cast(None).friend(me, None, None, None),
            "its caster always is"
        );
        // Both have one: the engine's relations win over magic's sides.
        let enemy = cast(Some(Faction::ENEMY));
        assert!(!enemy.friend(
            other,
            Some(&same_side),
            Some(&Faction::PLAYER),
            Some(&factions)
        ));
        assert!(enemy.friend(other, None, Some(&Faction::PREY), Some(&factions)));
        assert!(enemy.friend(other, None, Some(&Faction::ENEMY), None));
    }
}
