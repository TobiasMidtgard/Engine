//! Kinesis (round-10-plan.md, research-kinesis.md): take hold of things with force.
//!
//! A spell with the `grip` effect reaches out along its aim (a 14° cone, Half-Life 2's gravity
//! gun) and takes the best thing there: an enemy missile in flight (caught), a loose body (a
//! crate, a barrel, a dummy, a rock) or a creature. With nothing in reach it tears a rough
//! stone out of the ground in front of its caster (Control's Launch). What it holds pops up, comes
//! to a point before the hand and floats there, slowly tumbling, while the spell is held (the
//! spell bar's channel) or for its `grip_secs`; then it flies where the caster aims. Cast as an
//! orbit, it tears a ring of stones out of the ground around its caster that circle and stop the
//! missiles of others (Control's Shield) until they are flung at the aim one after another.
//!
//! The cast's energy sets what it can do: half of it lifts the heaviest thing 3 m and throws it
//! at 20 m/s ([`heaviest_kg`]), and the throw's half sets the speed ([`throw_speed`]), so a
//! novice fetches a cup and a level 60 mage throws a cart. Held bodies follow by velocity (set
//! every step to close the gap in [`ARRIVE_SECS`], capped), not by a spring, so a light thing
//! never ploughs through a heavy one; gravity is off while they are held. A thrown thing hits
//! creatures it passes by its momentum ([`blow`]) and sets off the spell's woven payload where
//! it first hits or lands (a fire grip's crate bursts in flame).
//!
//! Bodies move in the Gameplay set (the teleport lesson: a PostPhysics write is overwritten by
//! the off-frame step's write-back). Magic draws the line of force from the hand itself
//! ([`draw_grips`]): three thin strands twisting round the line while it holds.

use std::f32::consts::{PI, TAU};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialPreset, MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::animation::SkeletonPose;
use engine_world::physics::{
    ApplyImpulse, BodyKind, CharacterController, Collider, ColliderShape, MassProperties,
    PhysicsQueries, QueryFilter, RigidBody, Velocity,
};
use engine_world::{DamageEvent, DamageKind, Faction, Factions, Gravity};
use serde::Serialize;

use crate::caster::{Aim, MagicCaster, Vitality};
use crate::design::CastType;
use crate::looks::{LookCmd, Role};
use crate::runtime::{
    Allegiance, CastInfo, HitQueue, MagicQueue, Missile, PendingHit, SpellPos, hand_of, roll,
};
use crate::status::Afflictions;
use crate::summon::Rubble;

/// Half angle of the cone a grip takes things from (Half-Life 2: dot 0.97).
pub const CONE_DEG: f32 = 14.0;
/// The fastest a held thing moves to its point, m/s (Half-Life 2: 1000 in/s).
pub const HOLD_SPEED: f32 = 25.0;
/// Seconds a held thing takes to close the gap to its point (it is capped by the speed).
pub const ARRIVE_SECS: f32 = 0.12;
/// The fastest and slowest throw, m/s (Half-Life 2: 38 m/s up to 100 kg, 18 m/s at 600 kg).
pub const THROW_MAX: f32 = 38.0;
pub const THROW_MIN: f32 = 6.0;
/// On being taken, a thing first rises this far, in this long (Control's beat before it comes).
pub const POP_M: f32 = 0.6;
pub const POP_SECS: f32 = 0.3;
/// A torn stone takes this long to rise out of the ground.
pub const RISE_SECS: f32 = 0.22;
/// Held more than this far off its point for [`STUCK_SECS`], after [`GRACE_SECS`], it drops
/// (Half-Life 2: 12 in after a second).
pub const STUCK_M: f32 = 0.3;
pub const GRACE_SECS: f32 = 1.0;
pub const STUCK_SECS: f32 = 1.0;
/// The fastest a dropped thing leaves (so swinging the aim can't fling it), m/s.
pub const LET_GO_SPEED: f32 = 7.0;
/// Seconds a grip looks for something (an incoming missile) before it tears the ground.
pub const SEARCH_SECS: f32 = 0.35;
/// How far ahead a grip tears its stone when nothing is in reach, m.
pub const TEAR_M: f32 = 3.2;
/// A ring of torn stones: its radius, its turn (rad/s) and the gap between throws.
pub const RING_RADIUS: f32 = 1.9;
pub const RING_RATE: f32 = 1.1;
pub const RING_GAP: f32 = 0.09;
/// Stones in a ring (more with Split).
pub const RING_STONES: u32 = 8;
/// A ring's stone swings this far into a missile's way, m.
pub const RING_LUNGE_M: f32 = 1.3;
/// Missiles a ring's stone stops before it crumbles.
pub const RING_KNOCKS: u8 = 2;
/// A thrown thing stops hitting under this speed, and after this long.
pub const HURL_MIN_SPEED: f32 = 4.0;
pub const HURL_SECS: f32 = 4.0;
/// Seconds a torn stone lies before it sinks away.
pub const TORN_SECS: f32 = 14.0;
/// Stone's colour for its dust.
pub const STONE_COLOR: [f32; 3] = [0.42, 0.38, 0.33];
/// Strands in the line of force, and segments in each.
pub const STRANDS: usize = 3;
pub const STRAND_SEGMENTS: usize = 24;
/// Turns the strands make round each other from the hand to the thing.
pub const LINE_TURNS: f32 = 1.25;
/// How far the line bows up in its middle, as a share of its length (a cord under strain).
pub const LINE_BEND: f32 = 0.06;

/// The heaviest thing a cast of `energy_j` lifts, kg: half of the energy lifts it 3 m and
/// throws it at 20 m/s (230 J/kg), within 1 kg..20 t.
pub fn heaviest_kg(energy_j: f32) -> f32 {
    (0.5 * energy_j / 230.0).clamp(1.0, 20_000.0)
}

/// How fast `throw_j` throws `mass_kg`, m/s.
pub fn throw_speed(throw_j: f32, mass_kg: f32) -> f32 {
    (2.0 * throw_j / mass_kg.max(0.1))
        .sqrt()
        .clamp(THROW_MIN, THROW_MAX)
}

/// How far before the hand a thing of `radius` m floats (Half-Life 2: 24 in + 2 × its extent).
pub fn hold_distance(radius: f32) -> f32 {
    (0.6 + 2.0 * radius).clamp(1.0, 4.5)
}

/// The fastest a thing of `mass_kg` comes to the hand under a grip that lifts `limit_kg`: full
/// speed up to half the limit, slower near it.
pub fn hold_speed(mass_kg: f32, limit_kg: f32) -> f32 {
    if mass_kg <= 0.5 * limit_kg {
        HOLD_SPEED
    } else {
        (HOLD_SPEED * (0.5 * limit_kg / mass_kg.max(0.1)).sqrt()).max(3.0)
    }
}

/// The blow a thrown thing deals: the spell's damage scale by speed (to 1.6 × at 32 m/s) and
/// weight (a 40 kg crate is 1, within 0.35..2); nothing under [`HURL_MIN_SPEED`].
pub fn blow(damage: f32, speed: f32, mass_kg: f32) -> f32 {
    if speed < HURL_MIN_SPEED {
        return 0.0;
    }
    damage * (speed / 20.0).min(1.6) * (mass_kg / 40.0).powf(0.3).clamp(0.35, 2.0)
}

/// A shape's reach from its middle, m, and where its middle sits in the body's space.
pub fn extent(shape: &ColliderShape) -> (f32, Vec3) {
    match shape {
        ColliderShape::Sphere { radius } => (*radius, Vec3::ZERO),
        ColliderShape::Cuboid { half_extents } => (half_extents.length(), Vec3::ZERO),
        ColliderShape::Capsule {
            radius,
            half_height,
        } => (radius + half_height, Vec3::ZERO),
        ColliderShape::Cylinder {
            radius,
            half_height,
        } => (
            (radius * radius + half_height * half_height).sqrt(),
            Vec3::ZERO,
        ),
        ColliderShape::ConvexHull { points } => (
            points.iter().map(|p| p.length()).fold(0.1, f32::max),
            Vec3::ZERO,
        ),
        ColliderShape::Compound(parts) if !parts.is_empty() => {
            let mid = parts.iter().map(|p| p.0).sum::<Vec3>() / parts.len() as f32;
            let r = parts
                .iter()
                .map(|(at, _, s)| (*at - mid).length() + extent(s).0)
                .fold(0.1, f32::max);
            (r, mid)
        }
        _ => (0.5, Vec3::ZERO),
    }
}

/// A grip at work: what it holds, how long it holds, and where it throws.
#[derive(Component, Debug, Clone)]
pub struct Grip {
    pub cast: CastInfo,
    /// Where it throws (the spell bar moves it with the crosshair while the key is held).
    pub aim: Aim,
    pub age: f32,
    /// It throws (a ring flings its stones) once its age reaches this; the spell bar pushes it
    /// on while the key is held.
    pub duration: f32,
    /// A ring of torn stones (an orbit cast).
    pub ring: bool,
    pub held: Vec<HeldThing>,
    /// Seconds it has looked without finding anything.
    pub searched: f32,
    /// It has torn its stone (or its ring) out of the ground.
    pub tore: bool,
    /// Seconds to a ring's next throw.
    pub next_throw: f32,
    /// Seconds since everything was thrown or dropped (its line fades, then it goes).
    pub done: Option<f32>,
}

impl Grip {
    pub fn new(cast: CastInfo, aim: Aim) -> Self {
        let p = &cast.spell.payload;
        Self {
            ring: cast.spell.cast == CastType::Orbit,
            duration: p.grip_secs,
            cast,
            aim,
            age: 0.0,
            held: Vec::new(),
            searched: 0.0,
            tore: false,
            next_throw: 0.0,
            done: None,
        }
    }

    /// Something is held (not yet thrown).
    pub fn holding(&self) -> bool {
        self.held.iter().any(|h| !h.thrown)
    }
}

/// What a grip holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldKind {
    /// A loose physics body (crate, barrel, dummy, torn stone): moved by its velocity.
    Body,
    /// A creature: lifted helpless, moved by magic.
    Creature,
    /// A missile caught in flight.
    Missile,
}

/// One thing in a grip.
#[derive(Debug, Clone)]
pub struct HeldThing {
    pub entity: Entity,
    pub kind: HeldKind,
    pub mass: f32,
    pub radius: f32,
    /// From the body's origin to its middle, in body space.
    pub center: Vec3,
    pub since: f32,
    /// Where its middle was when taken.
    pub from: DVec3,
    /// Smoothed distance off its point, m.
    pub error: f32,
    pub stuck: f32,
    /// The body's own gravity scale, given back when it is let go.
    pub gravity_scale: f32,
    /// Too heavy to lift: it slides toward the caster instead.
    pub too_heavy: bool,
    /// Its place round a ring (rad), or its bob's phase.
    pub slot: f32,
    /// A ring stone's height above the caster's feet, m.
    pub height: f32,
    pub spin: Vec3,
    /// Missiles it has stopped (a ring's stone).
    pub knocks: u8,
    /// A knock's shove off its point, m (decays).
    pub kick: Vec3,
    /// A stone still rising out of the ground (no physics yet), with its size.
    pub rising: Option<Vec3>,
    pub thrown: bool,
    /// A caught missile's speed.
    pub speed: f32,
}

impl HeldThing {
    fn new(entity: Entity, kind: HeldKind, mass: f32, radius: f32, center: Vec3) -> Self {
        Self {
            entity,
            kind,
            mass,
            radius,
            center,
            since: 0.0,
            from: DVec3::ZERO,
            error: 0.0,
            stuck: 0.0,
            gravity_scale: 1.0,
            too_heavy: false,
            slot: 0.0,
            height: 1.2,
            spin: Vec3::ZERO,
            knocks: 0,
            kick: Vec3::ZERO,
            rising: None,
            thrown: false,
            speed: 0.0,
        }
    }
}

/// On a missile a grip holds: it stops flying until thrown.
#[derive(Component, Debug, Clone, Copy)]
pub struct Caught {
    pub by: Entity,
}

/// On a body or creature a grip holds.
#[derive(Component, Debug, Clone, Copy)]
pub struct Gripped {
    pub by: Entity,
}

/// A stone a grip tore out of the ground.
#[derive(Component, Debug, Clone, Copy)]
pub struct Torn;

/// A thrown thing in flight: it hits creatures it passes by its momentum, and sets off the
/// spell's woven payload where it first hits or lands.
#[derive(Component, Debug, Clone)]
pub struct Hurled {
    pub cast: CastInfo,
    pub mass: f32,
    pub radius: f32,
    pub center: Vec3,
    pub age: f32,
    pub hit: Vec<Entity>,
    pub last_speed: f32,
    /// Seconds it has been slower than [`HURL_MIN_SPEED`].
    pub slow: f32,
    /// The woven payload has gone off.
    pub burst: bool,
    /// A creature in flight, moved by magic: its velocity, m/s.
    pub flung: Option<Vec3>,
    /// Its look is on (a woven grip's trail).
    pub look: bool,
}

/// Bodies a grip may take, and creatures it may lift.
#[derive(bevy_ecs::query::QueryData)]
#[query_data(mutable)]
pub struct Thing {
    pub entity: Entity,
    pub transform: &'static mut Transform,
    pub global: &'static GlobalTransform,
    pub body: Option<&'static mut RigidBody>,
    pub collider: Option<&'static Collider>,
    pub mass: Option<&'static MassProperties>,
    pub velocity: Option<&'static mut Velocity>,
    pub rubble: Option<&'static mut Rubble>,
    pub character: Has<CharacterController>,
    pub alive: Has<Vitality>,
    pub skeleton: Has<SkeletonPose>,
    pub gripped: Option<&'static Gripped>,
    pub caster: Option<&'static MagicCaster>,
    pub faction: Option<&'static Faction>,
}

/// What a grip needs to know of a thing.
#[derive(Debug, Clone, Copy)]
pub struct ThingInfo {
    /// A loose physics body (moved by its velocity).
    pub dynamic: bool,
    /// A creature that isn't a loose body (moved by magic).
    pub creature: bool,
    /// Its reach from its middle, m.
    pub radius: f32,
    /// Its middle in body space.
    pub center: Vec3,
    /// Its middle in the world.
    pub middle: DVec3,
    pub weight: f32,
}

fn thing_info(
    global: &GlobalTransform,
    body: Option<&RigidBody>,
    collider: Option<&Collider>,
    mass: Option<&MassProperties>,
    alive: bool,
) -> ThingInfo {
    let dynamic = body.is_some_and(|b| b.kind == BodyKind::Dynamic);
    let creature = !dynamic && alive;
    let (radius, center) = if creature {
        (0.6, Vec3::Y * 1.0)
    } else {
        collider
            .map(|c| extent(&c.shape))
            .unwrap_or((0.5, Vec3::ZERO))
    };
    let weight = if let Some(m) = mass.filter(|m| m.mass_kg > 0.0) {
        m.mass_kg
    } else if let Some(kg) = body.and_then(|b| b.mass_kg) {
        kg
    } else if creature {
        75.0
    } else {
        let density = collider.map(|c| c.density).unwrap_or(1000.0);
        (4.0 / 3.0 * PI * radius * radius * radius * 0.5 * density).max(1.0)
    };
    ThingInfo {
        dynamic,
        creature,
        radius,
        center,
        middle: global.translation() + (global.rotation() * center).as_dvec3(),
        weight,
    }
}

/// A thing's [`ThingInfo`], from a read-only or a mutable query item.
macro_rules! info {
    ($t:expr) => {
        thing_info(
            $t.global,
            $t.body.as_deref(),
            $t.collider,
            $t.mass,
            $t.character || $t.alive || $t.skeleton,
        )
    };
}

/// Where a grip's aim points: the aimed body's middle, the aimed point, or far along a direction.
fn aim_point(
    aim: Aim,
    hand: DVec3,
    things: &Query<Thing, (Without<Missile>, Without<Grip>)>,
) -> DVec3 {
    match aim {
        Aim::Entity(t) => things.get(t).map(|t| info!(t).middle).unwrap_or(hand),
        Aim::Point(p) => p,
        Aim::Direction(d) => hand + (d * 60.0).as_dvec3(),
    }
}

/// The deterministic spin a thing tumbles with while held (slow) or thrown (fast).
fn spin(seed: u64, salt: u64, rate: f32) -> Vec3 {
    Vec3::new(
        roll(seed, salt) - 0.5,
        roll(seed, salt + 1) - 0.5,
        roll(seed, salt + 2) - 0.5,
    )
    .normalize_or(Vec3::Y)
        * rate
}

/// A stone torn out of the ground at `ground` (its top just under the surface; it rises out).
fn tear_stone(
    commands: &mut Commands,
    queue: &mut MagicQueue,
    ground: DVec3,
    size: Vec3,
    seed: u64,
) -> Entity {
    let yaw = roll(seed, 41) * TAU;
    let tilt = (roll(seed, 42) - 0.5) * 0.6;
    let rest = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(tilt);
    let material = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
    let e = commands
        .spawn((
            Name::new("Torn stone"),
            Transform::from_translation(ground - DVec3::Y * (size.y * 0.6) as f64)
                .with_rotation(rest),
            MeshRenderer::new(
                MeshRef::Shape(Shape::Rock {
                    size: size.to_array(),
                    seed: (seed % 100_000) as u32,
                    detail: 3,
                }),
                MaterialRef::Inline(material),
            ),
            Rubble {
                age: 0.0,
                life: TORN_SECS,
            },
            Torn,
        ))
        .id();
    queue.looks.push(LookCmd::Burst {
        at: ground + DVec3::Y * 0.05,
        up: Vec3::Y,
        look: "stone".into(),
        role: Role::Impact,
        scale: (size.x / 0.5).clamp(0.6, 1.8),
        color: STONE_COLOR,
    });
    queue.looks.push(LookCmd::Mark {
        at: ground + DVec3::Y * 0.02,
        normal: Vec3::Y,
        kind: "chip",
        radius: size.x * 0.9,
    });
    e
}

/// A torn stone's size for a grip of this power: half a metre for a level 50 caster, more with
/// more power, and a ring's stones smaller.
pub fn stone_size(power_size: f32, ring: bool, seed: u64) -> Vec3 {
    let base = (0.42 + 0.12 * power_size).clamp(0.38, 0.85) * if ring { 0.78 } else { 1.0 };
    let k = 0.85 + 0.3 * roll(seed, 51);
    Vec3::new(base * 1.15, base * 0.85, base) * k
}

/// A torn stone's mass: rock about half its box, within what the grip can lift.
fn stone_mass(size: Vec3, limit_kg: f32) -> f32 {
    (size.x * size.y * size.z * 2600.0 * 0.55).clamp(4.0, (limit_kg * 0.7).max(4.0))
}

/// Physics for a torn stone once it is out of the ground.
fn stone_body(size: Vec3, mass: f32) -> (Collider, RigidBody) {
    (
        Collider {
            shape: ColliderShape::Cuboid {
                half_extents: size * 0.42,
            },
            density: 2600.0,
            ..Default::default()
        },
        RigidBody {
            kind: BodyKind::Dynamic,
            mass_kg: Some(mass),
            linear_damping: 0.02,
            angular_damping: 0.15,
            gravity_scale: 0.0,
            ..Default::default()
        },
    )
}

/// Run grips: take hold of something (catch, lift or tear), hold it before the hand (or round a
/// ring), throw it when the time is up, and let go of what is stuck.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn grip(
    time: Res<Time>,
    gravity: Res<Gravity>,
    queries: Res<PhysicsQueries>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
    mut grips: Query<(Entity, &mut Grip)>,
    casters: Query<(
        &GlobalTransform,
        &MagicCaster,
        Option<&Faction>,
        Option<&crate::player::SpellBar>,
    )>,
    mut things: Query<Thing, (Without<Missile>, Without<Grip>)>,
    mut missiles: Query<(Entity, &mut Missile, &mut SpellPos, Has<Caught>)>,
    mut afflictions: Query<&mut Afflictions>,
    factions: Option<Res<Factions>>,
) {
    let factions = factions.as_deref();
    let dt = time.delta_secs();
    for mut g in std::mem::take(&mut queue.grips) {
        // A player who already let the key go tapped it: take hold and throw at once.
        if let Some(c) = g.cast.caster
            && let Ok((_, _, _, Some(bar))) = casters.get(c)
            && bar.held.is_none()
        {
            g.duration = 0.0;
        }
        commands.spawn((Name::new("Spell grip"), Transform::default(), g));
    }
    if dt <= 0.0 {
        return;
    }
    for (ge, mut g) in &mut grips {
        g.age += dt;
        if let Some(t) = g.done.as_mut() {
            *t += dt;
            if *t > 0.5 {
                commands.entity(ge).despawn();
            }
            continue;
        }
        let spell = g.cast.spell.clone();
        let p = &spell.payload;
        let seed = g.cast.seed as u64;
        let Some((cg, cm, cfaction, _)) = g.cast.caster.and_then(|c| casters.get(c).ok()) else {
            // Its caster is gone: everything drops.
            let held = std::mem::take(&mut g.held);
            for h in held.iter().filter(|h| !h.thrown) {
                let_go(h, &mut things, &mut missiles, &mut commands);
            }
            commands.entity(ge).despawn();
            continue;
        };
        let hand = hand_of(cg, cm);
        let feet = cg.translation();
        let forward = (cg.rotation() * Vec3::NEG_Z).normalize_or(Vec3::NEG_Z);
        let target_at = aim_point(g.aim, hand, &things);
        let aim_dir = (target_at - hand).as_vec3().normalize_or(forward);
        let exclude = g
            .cast
            .caster
            .map(QueryFilter::excluding)
            .unwrap_or_default();
        let up = -gravity.at(hand).normalize_or(Vec3::NEG_Y);
        let g_len = gravity.at(hand).length().max(0.1);

        // ---- taking hold
        if g.ring && !g.tore {
            g.tore = true;
            // Enough stones that one is always near enough to swing into a missile's way.
            let n = spell.carrier.count.clamp(RING_STONES, 14) as usize;
            for i in 0..n {
                let a = TAU * i as f32 / n as f32 + roll(seed, 300 + i as u64) * 0.3;
                let out = Vec3::new(a.cos(), 0.0, a.sin()) * RING_RADIUS;
                let ground =
                    crate::runtime::ground_below(&queries, feet + out.as_dvec3(), &exclude);
                let s = seed.wrapping_add(i as u64 * 7907);
                let size = stone_size(spell.power.size(), true, s);
                let e = tear_stone(&mut commands, &mut queue, ground, size, s);
                let mass = stone_mass(size, p.grip_kg);
                let mut h =
                    HeldThing::new(e, HeldKind::Body, mass, size.length() * 0.42, Vec3::ZERO);
                h.from = ground + DVec3::Y * (size.y * 0.5) as f64;
                h.rising = Some(size);
                h.slot = a;
                h.height = 0.9 + 0.7 * roll(seed, 400 + i as u64);
                h.spin = spin(s, 7, 0.9);
                h.gravity_scale = 1.0;
                g.held.push(h);
            }
        } else if !g.ring && g.held.is_empty() {
            match pick(
                &g, hand, aim_dir, &queries, &exclude, &things, &missiles, factions,
            ) {
                Some((e, kind)) => {
                    let h = take(
                        &g,
                        e,
                        kind,
                        hand,
                        &mut things,
                        &mut missiles,
                        &mut commands,
                        ge,
                    );
                    if let Some(h) = h {
                        if !matches!(spell.weave.key.as_str(), "force" | "arcane")
                            && kind != HeldKind::Missile
                        {
                            queue.looks.push(LookCmd::Attach {
                                entity: e,
                                look: spell.weave.key.clone(),
                                role: Role::Aura,
                                scale: (h.radius / 0.65).clamp(0.3, 3.0),
                                color: spell.weave.color,
                                power_j: spell.power.energy_j as f32,
                            });
                        }
                        g.held.push(h);
                    }
                }
                None => {
                    g.searched += dt;
                    if g.searched >= SEARCH_SECS && !g.tore {
                        g.tore = true;
                        // Nothing in reach: tear a stone out of the ground a few steps ahead
                        // (where the caster aims, if that is nearer), and bring it.
                        let flat = Vec3::new(aim_dir.x, 0.0, aim_dir.z).normalize_or(
                            Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z),
                        );
                        let near = feet + (flat * TEAR_M).as_dvec3();
                        let spot = match g.aim {
                            Aim::Point(pt) if (pt - feet).length() < TEAR_M as f64 + 1.0 => pt,
                            _ => near,
                        };
                        let ground = queries
                            .raycast(spot + DVec3::Y * 2.0, Vec3::NEG_Y, 6.0, &exclude)
                            .map(|h| h.point);
                        match ground {
                            Some(ground) => {
                                let size = stone_size(spell.power.size(), false, seed);
                                let e = tear_stone(&mut commands, &mut queue, ground, size, seed);
                                let mass = stone_mass(size, p.grip_kg);
                                let mut h = HeldThing::new(
                                    e,
                                    HeldKind::Body,
                                    mass,
                                    size.length() * 0.42,
                                    Vec3::ZERO,
                                );
                                h.from = ground + DVec3::Y * (size.y * 0.5) as f64;
                                h.rising = Some(size);
                                h.spin = spin(seed, 9, 0.7);
                                h.slot = roll(seed, 10) * TAU;
                                commands.entity(e).insert(Gripped { by: ge });
                                if !matches!(spell.weave.key.as_str(), "force" | "arcane") {
                                    queue.looks.push(LookCmd::Attach {
                                        entity: e,
                                        look: spell.weave.key.clone(),
                                        role: Role::Aura,
                                        scale: (h.radius / 0.65).clamp(0.3, 3.0),
                                        color: spell.weave.color,
                                        power_j: spell.power.energy_j as f32,
                                    });
                                }
                                g.held.push(h);
                            }
                            None => {
                                queue.looks.push(LookCmd::Burst {
                                    at: hand + (aim_dir * 1.5).as_dvec3(),
                                    up: Vec3::Y,
                                    look: spell.weave.key.clone(),
                                    role: Role::Fizzle,
                                    scale: 0.5,
                                    color: spell.weave.color,
                                });
                                g.done = Some(0.0);
                                continue;
                            }
                        }
                    }
                }
            }
        }

        // ---- holding
        let time_up = g.age >= g.duration;
        let ring = g.ring;
        let age = g.age;
        let reach = p.grip_m;
        let limit = p.grip_kg;
        let mut drop_list = Vec::new();
        for (i, h) in g.held.iter_mut().enumerate() {
            if h.thrown {
                continue;
            }
            h.since += dt;
            h.kick *= (1.0 - 6.0 * dt).max(0.0);
            let bob = Vec3::Y * (0.05 * (2.4 * h.since + h.slot).sin());
            let point = if ring {
                let a = h.slot + RING_RATE * age;
                feet + DVec3::Y * h.height as f64
                    + (Vec3::new(a.cos(), 0.0, a.sin()) * RING_RADIUS + bob + h.kick).as_dvec3()
            } else {
                let mut d = hold_distance(h.radius);
                if let Some(hit) = queries.raycast(
                    hand,
                    aim_dir,
                    d + h.radius,
                    &QueryFilter {
                        exclude: [exclude.exclude.clone(), vec![h.entity]].concat(),
                        ..Default::default()
                    },
                ) {
                    d = d.min((hit.distance - h.radius).max(0.5));
                }
                hand + (aim_dir * d + up * 0.25 + bob + h.kick).as_dvec3()
            };
            // Taken things first rise where they were, then come.
            let pop = (h.since / POP_SECS).clamp(0.0, 1.0);
            let point = if h.kind != HeldKind::Missile && pop < 1.0 {
                h.from + (up * POP_M * (1.0 - (1.0 - pop) * (1.0 - pop))).as_dvec3()
            } else {
                point
            };
            match h.kind {
                HeldKind::Body => {
                    let Ok(mut t) = things.get_mut(h.entity) else {
                        // A stone torn this step isn't in the world yet.
                        if h.rising.is_none() || h.since > 0.2 {
                            drop_list.push(i);
                        }
                        continue;
                    };
                    if let Some(r) = t.rubble.as_mut() {
                        r.age = r.age.min(1.0);
                    }
                    if let Some(size) = h.rising {
                        // A torn stone rises out of its scar by hand, then takes physics.
                        let k = (h.since / RISE_SECS).clamp(0.0, 1.0);
                        let buried = h.from - DVec3::Y * (size.y * 1.1) as f64;
                        t.transform.translation = buried.lerp(
                            h.from + DVec3::Y * 0.08,
                            (1.0 - (1.0 - k) * (1.0 - k)) as f64,
                        );
                        if k >= 1.0 {
                            h.rising = None;
                            commands.entity(h.entity).insert((
                                stone_body(size, h.mass),
                                Velocity {
                                    linear: up * 2.2,
                                    angular: h.spin,
                                },
                            ));
                            h.gravity_scale = 1.0;
                            h.since = h.since.min(RISE_SECS);
                        }
                        continue;
                    }
                    let at = t.global.translation() + (t.global.rotation() * h.center).as_dvec3();
                    let off = (point - at).as_vec3();
                    let err = off.length();
                    h.error += (err - h.error) * (dt / 0.25).min(1.0);
                    if h.since > GRACE_SECS && h.error > STUCK_M && !h.too_heavy {
                        h.stuck += dt;
                    } else {
                        h.stuck = 0.0;
                    }
                    let far = at.distance(hand) as f32 > reach * 1.5 + 2.0;
                    if h.stuck > STUCK_SECS || far {
                        drop_list.push(i);
                        continue;
                    }
                    if let Some(b) = t.body.as_mut()
                        && b.gravity_scale != 0.0
                        && !h.too_heavy
                    {
                        h.gravity_scale = b.gravity_scale;
                        b.gravity_scale = 0.0;
                    }
                    let speed = hold_speed(h.mass, limit);
                    let v = if h.too_heavy {
                        // Too heavy to lift: it drags along the ground toward the caster.
                        let flat = Vec3::new(off.x, 0.0, off.z).normalize_or(Vec3::ZERO);
                        let keep = t.velocity.as_ref().map_or(0.0, |v| v.linear.y);
                        flat * 2.0 + Vec3::Y * keep
                    } else {
                        (off / ARRIVE_SECS).clamp_length_max(speed)
                    };
                    let w = if h.too_heavy { Vec3::ZERO } else { h.spin };
                    match t.velocity.as_mut() {
                        Some(vel) => {
                            vel.linear = v;
                            vel.angular = vel.angular.lerp(w, (dt * 4.0).min(1.0));
                        }
                        None => {
                            commands.entity(h.entity).try_insert(Velocity {
                                linear: v,
                                angular: w,
                            });
                        }
                    }
                }
                HeldKind::Creature => {
                    let Ok(mut t) = things.get_mut(h.entity) else {
                        drop_list.push(i);
                        continue;
                    };
                    let at = t.transform.translation + DVec3::Y * h.center.y as f64;
                    let off = (point - at).as_vec3();
                    let step = (off / ARRIVE_SECS).clamp_length_max(hold_speed(h.mass, limit)) * dt;
                    t.transform.translation += step.as_dvec3();
                    // Helpless: it can't move off while held.
                    match afflictions.get_mut(h.entity) {
                        Ok(mut a) => a.rooted = a.rooted.max(0.2),
                        Err(_) => {
                            let mut a = Afflictions::default();
                            a.rooted = 0.2;
                            queue.afflict.push((h.entity, a));
                        }
                    }
                }
                HeldKind::Missile => {
                    let Ok((_, _, mut sp, _)) = missiles.get_mut(h.entity) else {
                        drop_list.push(i);
                        continue;
                    };
                    let off = (point - sp.pos).as_vec3();
                    let step = (off / ARRIVE_SECS).clamp_length_max(h.speed.max(HOLD_SPEED)) * dt;
                    sp.pos += step.as_dvec3();
                    // It spins in place, waiting.
                    sp.up = Quat::from_axis_angle(Vec3::Y, 9.0 * dt) * sp.up;
                }
            }
        }
        for i in drop_list.into_iter().rev() {
            let h = g.held.remove(i);
            let_go(&h, &mut things, &mut missiles, &mut commands);
            queue.looks.push(LookCmd::Stop { entity: h.entity });
        }

        // ---- throwing
        let mut throw_now: Vec<usize> = Vec::new();
        if time_up {
            if ring {
                g.next_throw -= dt;
                if g.next_throw <= 0.0
                    && let Some(i) = g.held.iter().position(|h| !h.thrown && h.rising.is_none())
                {
                    throw_now.push(i);
                    g.next_throw = RING_GAP;
                }
            } else {
                throw_now.extend(
                    g.held
                        .iter()
                        .enumerate()
                        .filter(|(_, h)| {
                            !h.thrown
                                && h.rising.is_none()
                                && (h.since >= POP_SECS || h.kind == HeldKind::Missile)
                        })
                        .map(|(i, _)| i),
                );
            }
        }
        let aim_entity = match g.aim {
            Aim::Entity(t) => Some(t),
            _ => None,
        };
        for i in throw_now {
            let cast = g.cast.clone();
            let h = &mut g.held[i];
            h.thrown = true;
            throw(
                h,
                &cast,
                target_at,
                aim_entity,
                g_len,
                up,
                cfaction.copied(),
                cm.side,
                &mut things,
                &mut missiles,
                &mut commands,
                &mut queue,
            );
        }
        let searching = !g.ring && g.held.is_empty() && !g.tore;
        if !searching && !g.holding() {
            g.done = Some(0.0);
        }
    }
}

/// The best thing in a grip's reach: an incoming missile, then the aimed body, then whatever
/// sits nearest the aim line, in sight.
#[allow(clippy::too_many_arguments)]
fn pick(
    g: &Grip,
    hand: DVec3,
    aim_dir: Vec3,
    queries: &PhysicsQueries,
    exclude: &QueryFilter,
    things: &Query<Thing, (Without<Missile>, Without<Grip>)>,
    missiles: &Query<(Entity, &mut Missile, &mut SpellPos, Has<Caught>)>,
    factions: Option<&Factions>,
) -> Option<(Entity, HeldKind)> {
    let reach = g.cast.spell.payload.grip_m;
    let cone = CONE_DEG.to_radians();
    let in_cone = |at: DVec3, slack: f32| -> Option<f32> {
        let to = (at - hand).as_vec3();
        let d = to.length();
        if d > reach || d < 0.05 {
            return None;
        }
        let a = to.angle_between(aim_dir);
        // A little more room close in, where a thing fills more of the view.
        (a <= cone + slack / d.max(0.5)).then_some(a.to_degrees() + 0.15 * d)
    };
    // A missile coming at the caster (or anyone's but the caster's own) is caught first.
    let mut best: Option<(f32, Entity, HeldKind)> = None;
    for (e, m, sp, caught) in missiles.iter() {
        if caught || m.spent.is_some() || m.orbit.is_some() || m.cast.caster == g.cast.caster {
            continue;
        }
        if let Some(score) = in_cone(sp.pos, 0.6)
            && best.is_none_or(|b| score < b.0)
        {
            best = Some((score, e, HeldKind::Missile));
        }
    }
    if let Some(b) = best {
        return Some((b.1, b.2));
    }
    let can_take = |t: &ThingReadOnlyItem, i: &ThingInfo| -> Option<HeldKind> {
        if t.gripped.is_some() || Some(t.entity) == g.cast.caster {
            return None;
        }
        if i.dynamic {
            return Some(HeldKind::Body);
        }
        // Creatures: only foes are lifted (an ally is helped by other spells).
        if i.creature {
            let friend = g.cast.friend(t.entity, t.caster, t.faction, factions);
            return (!friend).then_some(HeldKind::Creature);
        }
        None
    };
    let in_sight = |e: Entity, i: &ThingInfo| -> bool {
        let to = (i.middle - hand).as_vec3();
        let d = to.length();
        queries
            .raycast(hand, to / d.max(1e-3), d, exclude)
            .is_none_or(|h| h.entity == e || h.distance >= d - i.radius - 0.25)
    };
    if let Aim::Entity(aimed) = g.aim
        && let Ok(t) = things.get(aimed)
        && let i = info!(t)
        && let Some(kind) = can_take(&t, &i)
        && i.middle.distance(hand) <= reach as f64
    {
        return Some((aimed, kind));
    }
    for t in things.iter() {
        let i = info!(t);
        let Some(kind) = can_take(&t, &i) else {
            continue;
        };
        let Some(score) = in_cone(i.middle, i.radius) else {
            continue;
        };
        if best.is_some_and(|b| score >= b.0) || !in_sight(t.entity, &i) {
            continue;
        }
        best = Some((score, t.entity, kind));
    }
    best.map(|b| (b.1, b.2))
}

/// Take hold of `e`: catch a missile, or mark a body or creature held.
#[allow(clippy::too_many_arguments)]
fn take(
    g: &Grip,
    e: Entity,
    kind: HeldKind,
    hand: DVec3,
    things: &mut Query<Thing, (Without<Missile>, Without<Grip>)>,
    missiles: &mut Query<(Entity, &mut Missile, &mut SpellPos, Has<Caught>)>,
    commands: &mut Commands,
    grip: Entity,
) -> Option<HeldThing> {
    let seed = g.cast.seed as u64;
    match kind {
        HeldKind::Missile => {
            let (_, m, sp, _) = missiles.get(e).ok()?;
            let mut h = HeldThing::new(
                e,
                kind,
                1.0,
                m.cast.spell.carrier.radius.max(0.1),
                Vec3::ZERO,
            );
            h.from = sp.pos;
            h.since = POP_SECS;
            h.speed = m.vel.length().max(12.0);
            commands.entity(e).try_insert(Caught { by: grip });
            let _ = hand;
            Some(h)
        }
        HeldKind::Body | HeldKind::Creature => {
            let t = things.get(e).ok()?;
            let i = info!(t);
            let (radius, center, mass) = (i.radius, i.center, i.weight);
            let mut h = HeldThing::new(e, kind, mass, radius, center);
            h.from = i.middle;
            h.too_heavy = kind == HeldKind::Body && mass > g.cast.spell.payload.grip_kg;
            h.spin = spin(seed ^ e.to_bits(), 3, 0.6 + 0.5 / radius.max(0.3));
            h.slot = roll(seed, 11) * TAU;
            if kind == HeldKind::Creature && mass > g.cast.spell.payload.grip_kg {
                return None;
            }
            commands.entity(e).try_insert(Gripped { by: grip });
            commands.entity(e).try_remove::<Hurled>();
            Some(h)
        }
    }
}

/// Let go of a held thing without throwing it: a body keeps its way at no more than
/// [`LET_GO_SPEED`], a creature falls, a missile flies on as it was.
fn let_go(
    h: &HeldThing,
    things: &mut Query<Thing, (Without<Missile>, Without<Grip>)>,
    missiles: &mut Query<(Entity, &mut Missile, &mut SpellPos, Has<Caught>)>,
    commands: &mut Commands,
) {
    match h.kind {
        HeldKind::Missile => {
            if missiles.get(h.entity).is_ok() {
                commands.entity(h.entity).try_remove::<Caught>();
            }
        }
        HeldKind::Body | HeldKind::Creature => {
            if let Ok(mut t) = things.get_mut(h.entity) {
                if let Some(b) = t.body.as_mut()
                    && b.gravity_scale != h.gravity_scale
                {
                    b.gravity_scale = h.gravity_scale;
                }
                if let Some(v) = t.velocity.as_mut() {
                    v.linear = v.linear.clamp_length_max(LET_GO_SPEED);
                    v.angular = v.angular.clamp_length_max(4.0 * PI);
                }
                if let Some(size) = h.rising {
                    commands
                        .entity(h.entity)
                        .try_insert(stone_body(size, h.mass));
                }
            }
            commands.entity(h.entity).try_remove::<Gripped>();
        }
    }
}

/// Throw a held thing at `target`: a body flies at its throw speed, lifted against gravity for
/// the distance and tumbling; a creature is flung; a caught missile flies back as the
/// gripper's.
#[allow(clippy::too_many_arguments)]
fn throw(
    h: &mut HeldThing,
    cast: &CastInfo,
    target: DVec3,
    aimed: Option<Entity>,
    g: f32,
    up: Vec3,
    faction: Option<Faction>,
    side: u32,
    things: &mut Query<Thing, (Without<Missile>, Without<Grip>)>,
    missiles: &mut Query<(Entity, &mut Missile, &mut SpellPos, Has<Caught>)>,
    commands: &mut Commands,
    queue: &mut MagicQueue,
) {
    let spell = &cast.spell;
    let seed = cast.seed as u64 ^ h.entity.to_bits();
    match h.kind {
        HeldKind::Missile => {
            let Ok((_, mut m, sp, _)) = missiles.get_mut(h.entity) else {
                return;
            };
            let dir = (target - sp.pos)
                .as_vec3()
                .normalize_or(m.vel.normalize_or(Vec3::NEG_Z));
            m.vel = dir * h.speed * 1.15;
            m.cast.caster = cast.caster;
            m.cast.side = side;
            m.cast.faction = faction;
            m.hits.clear();
            m.traveled = 0.0;
            m.target = aimed;
            m.returning = false;
            commands.entity(h.entity).try_remove::<Caught>();
        }
        HeldKind::Body | HeldKind::Creature => {
            let Ok(mut t) = things.get_mut(h.entity) else {
                return;
            };
            let at = t.global.translation() + (t.global.rotation() * h.center).as_dvec3();
            let to = (target - at).as_vec3();
            let speed = if h.too_heavy {
                THROW_MIN * 0.5
            } else {
                throw_speed(spell.payload.throw_j, h.mass)
            };
            let dir = to.normalize_or(up);
            // Lift it against the drop over the distance, so it arrives at the aim's height.
            let flight = to.length() / speed.max(1.0);
            let v = dir * speed + up * (0.5 * g * flight).min(speed * 0.6);
            let w = spin(seed, 21, 4.0 + 6.0 * roll(seed, 24));
            if let Some(b) = t.body.as_mut()
                && b.gravity_scale != h.gravity_scale
            {
                b.gravity_scale = h.gravity_scale;
            }
            let creature = h.kind == HeldKind::Creature;
            if !creature {
                match t.velocity.as_mut() {
                    Some(vel) => {
                        vel.linear = v;
                        vel.angular = w;
                    }
                    None => {
                        commands.entity(h.entity).try_insert(Velocity {
                            linear: v,
                            angular: w,
                        });
                    }
                }
            }
            let woven = spell.payload.damage > 0.0 || !spell.payload.statuses.is_empty();
            commands.entity(h.entity).try_remove::<Gripped>();
            commands.entity(h.entity).try_insert(Hurled {
                cast: cast.clone(),
                mass: h.mass,
                radius: h.radius,
                center: h.center,
                age: 0.0,
                hit: Vec::new(),
                last_speed: speed,
                slow: 0.0,
                burst: !woven,
                flung: creature.then_some(v),
                look: false,
            });
            if !woven && !creature {
                queue.looks.push(LookCmd::Stop { entity: h.entity });
            }
        }
    }
}

/// Fly thrown things: move flung creatures, hit the creatures they pass, and burst where they
/// land.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn fly_hurled(
    time: Res<Time>,
    gravity: Res<Gravity>,
    queries: Res<PhysicsQueries>,
    mut hurled: Query<(
        Entity,
        &mut Hurled,
        &mut Transform,
        &GlobalTransform,
        Option<&Velocity>,
        Option<&RigidBody>,
    )>,
    creatures: Query<
        (
            Entity,
            &GlobalTransform,
            Allegiance,
            Option<&Vitality>,
            Option<&RigidBody>,
        ),
        (crate::runtime::CreatureFilter, Without<Hurled>),
    >,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    mut damage: MessageWriter<DamageEvent>,
    mut impulses: MessageWriter<ApplyImpulse>,
    mut commands: Commands,
    factions: Option<Res<Factions>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let _ = &factions;
    for (e, mut h, mut tf, gt, vel, body) in &mut hurled {
        h.age += dt;
        let spell = h.cast.spell.clone();
        let mut landed: Option<(DVec3, Vec3)> = None;
        let (at, v) = if let Some(mut fv) = h.flung {
            // A flung creature flies an arc magic moves, until it meets something.
            let at = tf.translation + DVec3::Y * h.center.y as f64;
            fv += gravity.at(at) * dt;
            let step = fv * dt;
            let len = step.length();
            match queries.raycast(
                at,
                step / len.max(1e-4),
                len + 0.4,
                &QueryFilter::excluding(e),
            ) {
                Some(hit) if len > 0.0 => {
                    tf.translation += (step / len * (hit.distance - 0.4).max(0.0)).as_dvec3();
                    landed = Some((hit.point, hit.normal));
                    h.flung = None;
                }
                _ => {
                    tf.translation += step.as_dvec3();
                    h.flung = Some(fv);
                }
            }
            (at, fv)
        } else {
            let at = gt.translation() + (gt.rotation() * h.center).as_dvec3();
            (at, vel.map_or(Vec3::ZERO, |v| v.linear))
        };
        let speed = v.length();
        let dir = v.normalize_or(Vec3::NEG_Z);
        // Creatures it passes take its blow, once each.
        if speed >= HURL_MIN_SPEED {
            for (c, cg, side, vit, rb) in &creatures {
                if Some(c) == h.cast.caster || h.hit.contains(&c) || vit.is_some_and(|v| v.down()) {
                    continue;
                }
                let _ = side;
                let chest = cg.translation() + DVec3::Y * 1.0;
                let reach = (h.radius + 0.55) as f64;
                // Its way this step, against the creature's middle.
                let seg = (v * dt).as_dvec3();
                let s = ((chest - at).dot(seg) / seg.length_squared().max(1e-9)).clamp(0.0, 1.0);
                if (at + seg * s).distance(chest) > reach && at.distance(chest) > reach {
                    continue;
                }
                h.hit.push(c);
                let amount = blow(spell.payload.grip_damage * h.cast.scale, speed, h.mass);
                let push = (dir * (h.mass * speed * 0.5).min(900.0)) + Vec3::Y * 40.0;
                damage.write(DamageEvent {
                    target: c,
                    amount,
                    kind: DamageKind::Blunt,
                    point: chest,
                    impulse: push,
                    source: h.cast.caster,
                });
                if rb.is_some_and(|b| b.kind == BodyKind::Dynamic) {
                    impulses.write(ApplyImpulse {
                        entity: c,
                        impulse: push,
                        at: Some(chest),
                        angular: Vec3::new(push.z, 0.0, -push.x) * 0.01,
                    });
                }
                queue.looks.push(LookCmd::Burst {
                    at: chest,
                    up: -dir,
                    look: "stone".into(),
                    role: Role::Impact,
                    scale: (h.radius / 0.65).clamp(0.5, 1.6),
                    color: STONE_COLOR,
                });
                if !h.burst {
                    h.burst = true;
                    hits.0.push(PendingHit {
                        cast: h.cast.clone(),
                        target: Some(c),
                        point: chest,
                        normal: -dir,
                        dir,
                        multi: false,
                        impact: true,
                        reacts: true,
                        expire: false,
                    });
                }
            }
        }
        // It struck the ground or a wall: it lost half its speed in a step.
        if landed.is_none()
            && h.flung.is_none()
            && h.last_speed > 6.0
            && speed < 0.45 * h.last_speed
            && h.age > 0.05
        {
            landed = Some((at, -dir));
        }
        if let Some((point, normal)) = landed {
            let hard = h.last_speed.max(speed);
            if hard > 7.0 {
                queue.looks.push(LookCmd::Burst {
                    at: point,
                    up: normal,
                    look: "stone".into(),
                    role: Role::Impact,
                    scale: (h.radius / 0.65 * (hard / 20.0).sqrt()).clamp(0.5, 2.0),
                    color: STONE_COLOR,
                });
            }
            if !h.burst {
                h.burst = true;
                hits.0.push(PendingHit {
                    cast: h.cast.clone(),
                    target: None,
                    point,
                    normal,
                    dir,
                    multi: false,
                    impact: true,
                    reacts: true,
                    expire: false,
                });
            }
            // A flung creature lands hard.
            if body.is_none() && h.cast.caster.is_some() {
                let amount = blow(spell.payload.grip_damage * h.cast.scale, hard, h.mass) * 0.6;
                if amount > 0.0 {
                    damage.write(DamageEvent {
                        target: e,
                        amount,
                        kind: DamageKind::Fall,
                        point,
                        impulse: Vec3::ZERO,
                        source: h.cast.caster,
                    });
                }
            }
        }
        if speed < HURL_MIN_SPEED {
            h.slow += dt;
        } else {
            h.slow = 0.0;
        }
        h.last_speed = speed;
        if h.age > HURL_SECS || h.slow > 0.3 || (h.flung.is_none() && body.is_none()) {
            if !h.burst {
                hits.0.push(PendingHit {
                    cast: h.cast.clone(),
                    target: None,
                    point: at,
                    normal: Vec3::Y,
                    dir,
                    multi: false,
                    impact: true,
                    reacts: true,
                    expire: false,
                });
            }
            queue.looks.push(LookCmd::Stop { entity: e });
            commands.entity(e).try_remove::<Hurled>();
        }
    }
}

/// A ring's stones stop the missiles of others that come at them: the nearest stone swings into
/// the missile's way (up to [`RING_LUNGE_M`]), the missile bursts on it, and the stone rocks
/// back; a stone crumbles after [`RING_KNOCKS`].
#[allow(clippy::type_complexity)]
pub fn guard_rings(
    mut grips: Query<&mut Grip>,
    places: Query<&GlobalTransform>,
    mut missiles: Query<(Entity, &mut Missile, &SpellPos), Without<Caught>>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    for mut g in &mut grips {
        if !g.ring || g.done.is_some() {
            continue;
        }
        let caster = g.cast.caster;
        let Some(feet) = caster
            .and_then(|c| places.get(c).ok())
            .map(|t| t.translation())
        else {
            continue;
        };
        for (me, mut m, sp) in &mut missiles {
            if m.spent.is_some() || m.cast.caster == caster || m.orbit.is_some() {
                continue;
            }
            // Only what comes into the ring's band.
            let off = sp.pos - feet;
            let flat = (off.x * off.x + off.z * off.z).sqrt() as f32;
            if flat > RING_RADIUS + 0.5 || !(0.3..2.6).contains(&off.y) {
                continue;
            }
            let near = g
                .held
                .iter_mut()
                .filter(|h| !h.thrown && h.rising.is_none())
                .filter_map(|h| {
                    let at = places.get(h.entity).ok()?.translation();
                    Some((at.distance(sp.pos) as f32, at, h))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let Some((dist, at, h)) = near else { continue };
            let contact = h.radius + m.cast.spell.carrier.radius + 0.12;
            if dist > contact.max(RING_LUNGE_M) {
                continue;
            }
            let dir = m.vel.normalize_or(Vec3::NEG_Z);
            // The stone swings to where the missile is; the missile bursts on its face.
            h.kick += (sp.pos - at).as_vec3() + dir * 0.35;
            h.knocks += 1;
            m.spent = Some(0.0);
            queue.looks.push(LookCmd::Stop { entity: me });
            queue.looks.push(LookCmd::Burst {
                at: sp.pos,
                up: -dir,
                look: m.cast.spell.weave.key.clone(),
                role: Role::Impact,
                scale: (m.cast.spell.carrier.radius / 0.16).clamp(0.4, 1.5),
                color: m.cast.spell.weave.color,
            });
            queue.looks.push(LookCmd::Burst {
                at: sp.pos,
                up: -dir,
                look: "stone".into(),
                role: Role::Impact,
                scale: 0.6,
                color: STONE_COLOR,
            });
        }
        let worn: Vec<usize> = g
            .held
            .iter()
            .enumerate()
            .filter(|(_, h)| !h.thrown && h.knocks >= RING_KNOCKS)
            .map(|(i, _)| i)
            .collect();
        for i in worn.into_iter().rev() {
            let h = g.held.remove(i);
            if let Ok(st) = places.get(h.entity) {
                queue.looks.push(LookCmd::Burst {
                    at: st.translation(),
                    up: Vec3::Y,
                    look: "stone".into(),
                    role: Role::Impact,
                    scale: 1.1,
                    color: STONE_COLOR,
                });
            }
            commands.entity(h.entity).try_despawn();
        }
    }
}

/// The line of force from a grip's hand to what it holds.
#[derive(Component, Debug, Clone)]
pub struct ForceLine {
    pub grip: Entity,
    pub pieces: Vec<Entity>,
    pub from: DVec3,
    pub to: DVec3,
    /// What it pulls on; the line follows it a moment after the throw as it fades.
    pub thing: Option<(Entity, Vec3)>,
    pub age: f32,
    /// Seconds since its grip let go.
    pub gone: Option<f32>,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct ForcePiece;

/// Fades a line out over this long once its grip has let go.
pub const LINE_FADE_SECS: f32 = 0.15;

/// A point on strand `k` of a line from `a` to `b`, `s` of the way along: the line bows up a
/// little in its middle, and the strands twist slowly round it, widest there.
pub fn strand_point(a: DVec3, b: DVec3, s: f32, k: usize, age: f32) -> DVec3 {
    let along = (b - a).as_vec3();
    let dir = along.normalize_or(Vec3::NEG_Z);
    let side = dir.cross(Vec3::Y).normalize_or(Vec3::X);
    let lift = side.cross(dir).normalize_or(Vec3::Y);
    let bow = (PI * s).sin();
    let centre = along * s + lift * (LINE_BEND * along.length() * bow);
    let r = 0.012 + 0.045 * bow;
    let phase = TAU * k as f32 / STRANDS as f32 + s * LINE_TURNS * TAU - age * 6.0;
    a + (centre + (side * phase.cos() + lift * phase.sin()) * r).as_dvec3()
}

/// Draw each single grip's line of force while it holds: thin glowing strands from the hand.
#[allow(clippy::type_complexity)]
pub fn draw_grips(
    time: Res<Time>,
    grips: Query<(Entity, &Grip)>,
    casters: Query<(&GlobalTransform, &MagicCaster)>,
    anchors: Query<&Transform, Without<ForcePiece>>,
    mut lines: Query<(Entity, &mut ForceLine)>,
    mut pieces: Query<&mut Transform, With<ForcePiece>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, g) in &grips {
        if g.ring || !g.holding() || lines.iter().any(|(_, l)| l.grip == e) {
            continue;
        }
        let c = g.cast.spell.weave.color;
        let material = StandardMaterial::default()
            .with_tint(c[0], c[1], c[2])
            .with_emissive(c, 7.0);
        let shape = Shape::Cylinder {
            radius: 1.0,
            height: 1.0,
            segments: 6,
        };
        let pieces: Vec<Entity> = (0..STRANDS * STRAND_SEGMENTS)
            .map(|_| {
                commands
                    .spawn((
                        Name::new("Force strand"),
                        Transform::default().with_scale(Vec3::ZERO),
                        MeshRenderer::new(MeshRef::Shape(shape), MaterialRef::Inline(material)),
                        ForcePiece,
                    ))
                    .id()
            })
            .collect();
        commands.spawn((
            Name::new("Force line"),
            Transform::default(),
            ForceLine {
                grip: e,
                pieces,
                from: DVec3::ZERO,
                to: DVec3::ZERO,
                thing: None,
                age: 0.0,
                gone: None,
            },
        ));
    }
    for (le, mut line) in &mut lines {
        line.age += dt;
        // The held thing's own Transform: physics wrote it this frame, while its
        // GlobalTransform is still last frame's and would leave the line a step behind.
        let at = |(e, center): (Entity, Vec3)| {
            anchors
                .get(e)
                .ok()
                .map(|t| t.translation + (t.rotation * center).as_dvec3())
        };
        let held = grips.get(line.grip).ok().and_then(|(_, g)| {
            let h = g.held.iter().find(|h| !h.thrown)?;
            let (cg, cm) = g.cast.caster.and_then(|c| casters.get(c).ok())?;
            Some((hand_of(cg, cm), (h.entity, h.center)))
        });
        match held {
            Some((from, thing)) => {
                line.from = from;
                line.thing = Some(thing);
                line.gone = None;
            }
            None => {
                let t = line.gone.map_or(0.0, |t| t + dt);
                line.gone = Some(t);
            }
        }
        if let Some(to) = line.thing.and_then(at) {
            line.to = to;
        }
        let fade = line
            .gone
            .map_or(1.0, |t| 1.0 - (t / LINE_FADE_SECS).min(1.0));
        if fade <= 0.0 {
            for &p in &line.pieces {
                commands.entity(p).try_despawn();
            }
            commands.entity(le).despawn();
            continue;
        }
        // It reaches out from the hand over its first moments.
        let grow = (line.age / 0.15).min(1.0);
        let thick = 0.011 * fade;
        for (i, &p) in line.pieces.iter().enumerate() {
            let taper = 1.0 - 0.45 * (i % STRAND_SEGMENTS) as f32 / STRAND_SEGMENTS as f32;
            let Ok(mut tf) = pieces.get_mut(p) else {
                continue;
            };
            let (k, j) = (i / STRAND_SEGMENTS, i % STRAND_SEGMENTS);
            let s0 = j as f32 / STRAND_SEGMENTS as f32 * grow;
            let s1 = (j + 1) as f32 / STRAND_SEGMENTS as f32 * grow;
            let a = strand_point(line.from, line.to, s0, k, line.age);
            let b = strand_point(line.from, line.to, s1, k, line.age);
            let seg = (b - a).as_vec3();
            let len = seg.length();
            tf.translation = (a + b) * 0.5;
            tf.rotation = Quat::from_rotation_arc(Vec3::Y, seg / len.max(1e-5));
            tf.scale = Vec3::new(thick * taper, len, thick * taper);
        }
    }
}

/// `magic.kinesis`: every grip and every thrown thing in flight.
pub fn register(app: &mut App) {
    app.register_api(
        "magic.kinesis",
        "Telekinesis now: each grip (caster, ring or single, seconds held and left, what it holds: \
         id, kind body/creature/missile, mass kg, too heavy, thrown) and each thrown thing in \
         flight (id, speed m/s, mass, who it has hit).",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Grip)>();
            let grips: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, g)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "caster": g.cast.caster.map(|c| c.to_bits()),
                        "ring": g.ring,
                        "age_s": g.age,
                        "secs_left": (g.duration - g.age).max(0.0),
                        "held": g.held.iter().map(|h| serde_json::json!({
                            "id": h.entity.to_bits(),
                            "kind": h.kind,
                            "mass_kg": h.mass,
                            "too_heavy": h.too_heavy,
                            "thrown": h.thrown,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Hurled, Option<&Velocity>)>();
            let flying: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, h, v)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "speed_mps": h.flung.or(v.map(|v| v.linear)).map_or(0.0, |v| v.length()),
                        "mass_kg": h.mass,
                        "hit": h.hit.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(serde_json::json!({ "grips": grips, "flying": flying }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_and_speed_come_from_the_casts_energy() {
        // Level 1 (1 kJ) lifts a cup, level 20 (about 14 kJ) a crate, level 50 (1 MJ) a cart.
        assert!(heaviest_kg(1.0e3) < 3.0);
        assert!((25.0..40.0).contains(&heaviest_kg(1.45e4)));
        assert!(heaviest_kg(1.0e6) > 2000.0);
        assert_eq!(heaviest_kg(1.0e12), 20_000.0);
        // A crate flies at the cap; a cart near its limit far slower; never under the floor.
        assert_eq!(throw_speed(5.0e5, 16.0), THROW_MAX);
        let cart = throw_speed(5.0e5, 2000.0);
        assert!((15.0..25.0).contains(&cart), "{cart}");
        assert_eq!(throw_speed(10.0, 500.0), THROW_MIN);
    }

    #[test]
    fn a_thing_floats_further_out_the_bigger_it_is() {
        assert_eq!(hold_distance(0.1), 1.0);
        assert!((hold_distance(0.65) - 1.9).abs() < 1e-5);
        assert_eq!(hold_distance(5.0), 4.5);
        assert_eq!(hold_speed(10.0, 100.0), HOLD_SPEED);
        assert!(hold_speed(100.0, 100.0) < HOLD_SPEED * 0.75);
    }

    #[test]
    fn a_blow_grows_with_speed_and_weight() {
        assert_eq!(blow(100.0, 3.0, 40.0), 0.0);
        let crate_hit = blow(100.0, 20.0, 40.0);
        assert!((crate_hit - 100.0).abs() < 1e-3);
        assert!(blow(100.0, 38.0, 40.0) > crate_hit);
        assert!(blow(100.0, 20.0, 400.0) > crate_hit);
        assert!(blow(100.0, 20.0, 2.0) < crate_hit * 0.5);
    }

    #[test]
    fn shapes_have_a_reach_and_a_middle() {
        let (r, c) = extent(&ColliderShape::Cuboid {
            half_extents: Vec3::splat(0.375),
        });
        assert!((r - 0.65).abs() < 0.01 && c == Vec3::ZERO);
        let dummy = ColliderShape::Compound(vec![(
            Vec3::new(0.0, 1.1, 0.0),
            Quat::IDENTITY,
            ColliderShape::Capsule {
                radius: 0.34,
                half_height: 0.55,
            },
        )]);
        let (r, c) = extent(&dummy);
        assert!((r - 0.89).abs() < 0.01 && (c.y - 1.1).abs() < 1e-5);
    }

    #[test]
    fn strands_leave_the_hand_and_reach_the_thing() {
        let (a, b) = (DVec3::ZERO, DVec3::new(0.0, 0.0, -3.0));
        for k in 0..STRANDS {
            assert!(strand_point(a, b, 0.0, k, 0.3).distance(a) < 0.03);
            assert!(strand_point(a, b, 1.0, k, 0.3).distance(b) < 0.03);
            let mid = strand_point(a, b, 0.5, k, 0.3);
            assert!((mid.z + 1.5).abs() < 0.01 && mid.distance(DVec3::new(0.0, 0.0, -1.5)) > 0.08);
        }
    }

    #[test]
    fn torn_stones_grow_with_power_and_a_rings_are_smaller() {
        let small = stone_size(0.3, false, 1);
        let big = stone_size(3.0, false, 1);
        assert!(big.x > small.x);
        assert!(stone_size(1.0, true, 1).x < stone_size(1.0, false, 1).x);
        assert!(stone_mass(Vec3::splat(0.5), 10.0) <= 7.0);
    }
}
