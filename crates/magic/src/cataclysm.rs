//! Cataclysm (round-12-plan.md): rock from the sky and ground that splits.
//!
//! * **Meteor** (Rain + stone; with fire it burns): a glowing ring marks where it will land, a
//!   hot boulder falls on a slant out of the sky before the caster, and where it strikes it
//!   blasts everyone near, friend or foe, throws bodies off their feet and loose things away,
//!   and leaves a crater: a rim of broken rock, the boulder half buried and cooling, hot debris
//!   settling round it. Split makes a swarm, one rock after another.
//! * **Fault line** (Wave + stone): a crack runs along the ground from before the caster,
//!   heaving slabs up on both sides and throwing whoever stands on it; when it reaches its end
//!   the whole line bursts again (the aftershock), then the slabs grind down and it closes.
//!
//! Both work through the spell pipeline: their blows are `PendingHit`s (wards, shields,
//! statuses and reactions all apply), their blasts `WorldImpulse`s the rest of the engine hears.

use std::collections::HashMap;
use std::f32::consts::TAU;

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    MaterialPreset, MaterialRef, MeshRef, MeshRenderer, PointLight, Shape, StandardMaterial,
};
use engine_world::animation::{Ragdoll, SkeletonPose};
use engine_world::physics::{
    ApplyImpulse, BodyKind, CharacterController, Collider, ColliderShape, PhysicsQueries,
    QueryFilter, RigidBody, Velocity,
};
use engine_world::{
    EditOp, EditShape, Factions, NoiseEvent, TerrainEdit, WorldImpulse, WorldShift,
};

use crate::looks::{LookCmd, Role};
use crate::runtime::{CastInfo, CreatureFilter, HitQueue, MagicQueue, PendingHit, SpellPos, roll};
use crate::schools::Element;
use crate::summon::Rubble;
use crate::wards::{Barrier, Paint, WardStruck, against, ward_material};

/// How far before the target (along the caster's line) and above it a meteor starts, m.
pub const FALL_BEYOND: f64 = 28.0;
pub const FALL_HEIGHT: f64 = 55.0;
/// Seconds a crater's boulder takes to cool from yellow heat to dark rock.
pub const COOL_SECS: f32 = 10.0;
/// Seconds a crater stays before its rim sinks away.
pub const CRATER_SECS: f32 = 12.0;
/// Seconds a body thrown down by a meteor lies (a fault line's: 0.3 s less).
pub const KNOCK_SECS: f32 = 1.4;
/// A fault line's path is sampled every this many metres.
pub const PATH_STEP: f32 = 0.5;
/// Seconds from the crack reaching its end to the aftershock.
pub const AFTERSHOCK_SECS: f32 = 0.8;
/// Seconds from the aftershock to the slabs grinding back down, and how long that takes.
pub const CLOSE_AFTER: f32 = 2.2;
pub const CLOSE_SECS: f32 = 1.2;
/// The telegraph's colour, and the hot rock's.
pub const MARK_COLOR: [f32; 3] = [1.0, 0.32, 0.06];
pub const HOT_COLOR: [f32; 3] = [1.0, 0.45, 0.12];
/// A hot rock's glow, nits (a lit mesh: ward lattices use 11 000).
pub const HOT_GLOW: f32 = 26_000.0;

/// The blast round a meteor of `size` m across, m.
pub fn blast_radius(size: f32) -> f32 {
    2.2 * size + 1.5
}

/// The crater a meteor of `size` m digs, m.
pub fn crater_radius(size: f32) -> f32 {
    1.1 * size + 0.7
}

/// How much of a blast's blow reaches `d` m from its middle (radius `r`): full near the middle,
/// 40% at the edge.
pub fn falloff(d: f32, r: f32) -> f32 {
    let k = (d / r.max(0.1)).clamp(0.0, 1.0);
    1.0 - 0.6 * k * k
}

/// Something Cataclysm puts into the world (queued by the cast and the rain; spawned by
/// [`spawn_cataclysm`]).
#[derive(Debug, Clone)]
pub enum NewCataclysm {
    /// A rock to fall on `ground`, coming in along `toward` (the caster's line, flat).
    Meteor {
        cast: CastInfo,
        ground: DVec3,
        toward: Vec3,
    },
    /// A crack to run from `start` along `dir` (flat).
    Fault {
        cast: CastInfo,
        start: DVec3,
        dir: Vec3,
    },
}

/// A rock falling from the sky.
#[derive(Component, Debug, Clone)]
pub struct Meteor {
    pub cast: CastInfo,
    pub from: DVec3,
    pub ground: DVec3,
    pub age: f32,
    pub fall_secs: f32,
    /// Across, m.
    pub size: f32,
    pub spin: Vec3,
    /// Its ring on the ground and its rock.
    pub mark: Entity,
    pub rock: Entity,
}

impl Meteor {
    /// Where it is `age` s into its fall.
    pub fn at(&self) -> DVec3 {
        let t = (self.age / self.fall_secs.max(0.05)).clamp(0.0, 1.0) as f64;
        self.from + (self.ground - self.from) * t
    }

    /// Seconds until it lands.
    pub fn eta(&self) -> f32 {
        (self.fall_secs - self.age).max(0.0)
    }
}

/// The glowing ring where a meteor will land.
#[derive(Component, Debug, Clone, Copy)]
pub struct MeteorMark {
    pub meteor: Entity,
    pub radius: f32,
    pub age: f32,
    /// Seconds since its meteor landed (it fades), or None while it falls.
    pub fading: Option<f32>,
}

/// A falling meteor's rock (a child, so it tumbles while its carrier's look points along the
/// fall).
#[derive(Component, Debug, Clone, Copy)]
pub struct MeteorRock;

/// What a meteor leaves: a rim of broken rock round the boulder, cooling.
#[derive(Component, Debug, Clone)]
pub struct Crater {
    pub at: DVec3,
    pub radius: f32,
    pub age: f32,
    pub burning: bool,
    /// The boulder's rock (it glows as it cools).
    pub boulder: Entity,
    /// The boulder and the rim's rocks (they sink with it).
    pub parts: Vec<Entity>,
    pub glow: [f32; 3],
}

/// A short-lived light (a meteor striking).
#[derive(Component, Debug, Clone, Copy)]
pub struct QuakeFlash {
    pub age: f32,
    pub peak: f32,
    pub secs: f32,
}

/// A body thrown off its feet: limp (`Ragdoll`) for `secs`, then up again.
#[derive(Component, Debug, Clone, Copy)]
pub struct Knocked {
    pub secs: f32,
    /// It was already limp (downed in a duel): leave its `Ragdoll` alone.
    pub had_ragdoll: bool,
    pub started: bool,
}

/// A crack running along the ground.
#[derive(Component, Debug, Clone)]
pub struct Fault {
    pub cast: CastInfo,
    /// The crack's jagged line on the ground, every [`PATH_STEP`] m.
    pub path: Vec<DVec3>,
    /// Metres opened so far.
    pub front: f32,
    pub speed: f32,
    pub age: f32,
    /// Path points opened.
    pub opened: usize,
    pub hit: Vec<Entity>,
    /// When the crack reached its end, the aftershock went, and it began to close.
    pub ended: Option<f32>,
    pub shook: bool,
    pub closing: Option<f32>,
    pub pieces: Vec<Entity>,
}

impl Fault {
    /// The crack's length, m.
    pub fn length(&self) -> f32 {
        PATH_STEP * self.path.len().saturating_sub(1) as f32
    }
}

/// A piece of a fault line: the dark gap, or a slab heaved up beside it.
#[derive(Component, Debug, Clone, Copy)]
pub struct FaultPiece {
    pub base: DVec3,
    pub rest: Quat,
    pub slab: bool,
    pub age: f32,
    /// How far up it heaves, m.
    pub lift: f32,
    /// Seconds since the aftershock kicked it.
    pub kick: Option<f32>,
    /// Seconds since it began to sink.
    pub sink: Option<f32>,
}

/// A blast to apply this step: from a meteor striking, a crack opening or the aftershock.
#[derive(Debug, Clone)]
pub struct Blast {
    pub cast: CastInfo,
    pub at: DVec3,
    pub radius: f32,
    /// Throws bodies within this share of the radius off their feet.
    pub knock_share: f32,
    pub knock_secs: f32,
    /// Outward and upward throw on loose bodies at the middle, m/s.
    pub throw: f32,
    /// Hits only near this segment (a crack's), not the whole circle.
    pub line: Option<(DVec3, DVec3)>,
    /// Who it leaves alone (already hit by this crack).
    pub spare: Vec<Entity>,
    /// A meteor's own strike (the meteor, its rock, its size m; 0 when it burst on a ward):
    /// crater, debris, flash, the world's blast.
    pub meteor: Option<(Entity, Entity, f32)>,
}

/// This step's blasts.
#[derive(Resource, Debug, Default)]
pub struct Blasts(pub Vec<Blast>);

/// Who a fault line's blasts struck, by fault (filled by [`apply_blasts`], read next step).
#[derive(Resource, Debug, Default)]
pub struct FaultHits(pub Vec<(Entity, Entity)>);

fn hot_rock(glow: f32) -> StandardMaterial {
    let mut m = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
    m.emissive = HOT_COLOR;
    m.emissive_strength = glow;
    m
}

/// Put the queued meteors and faults into the world.
pub fn spawn_cataclysm(
    mut commands: Commands,
    mut queue: ResMut<MagicQueue>,
    queries: Res<PhysicsQueries>,
    creatures: Query<(), CreatureFilter>,
    rigid: Query<&RigidBody>,
) {
    // Bodies that stand on the ground, not ground: creatures and loose things.
    let mover = |e: Entity| {
        creatures.contains(e) || rigid.get(e).is_ok_and(|r| r.kind == BodyKind::Dynamic)
    };
    for n in std::mem::take(&mut queue.cataclysm) {
        match n {
            NewCataclysm::Meteor {
                cast,
                ground,
                toward,
            } => {
                // Aimed at a body, it lands at its feet.
                let exclude = cast.caster.into_iter().collect();
                let ground = ground_under(&queries, ground + DVec3::Y * 2.5, 8.0, exclude, &mover)
                    .map_or(ground, |h| h.point);
                spawn_meteor(&mut commands, &mut queue, cast, ground, toward)
            }
            NewCataclysm::Fault { cast, start, dir } => {
                let path = fault_path(&queries, &cast, start, dir, &mover);
                if path.len() < 2 {
                    continue;
                }
                commands.spawn((
                    Name::new("Fault line"),
                    Transform::from_translation(start),
                    Fault {
                        speed: cast.spell.carrier.speed.max(1.0),
                        cast,
                        path,
                        front: 0.0,
                        age: 0.0,
                        opened: 0,
                        hit: Vec::new(),
                        ended: None,
                        shook: false,
                        closing: None,
                        pieces: Vec::new(),
                    },
                ));
            }
        }
    }
}

fn spawn_meteor(
    commands: &mut Commands,
    queue: &mut MagicQueue,
    cast: CastInfo,
    ground: DVec3,
    toward: Vec3,
) {
    let spell = cast.spell.clone();
    let size = spell.payload.meteor_m.max(0.3) * cast.scale.sqrt().clamp(0.5, 1.5);
    let flat = Vec3::new(toward.x, 0.0, toward.z).normalize_or(Vec3::NEG_Z);
    // It comes in from the sky before the caster, a little to one side, so the caster sees it.
    let side = Vec3::new(-flat.z, 0.0, flat.x) * (roll(cast.seed as u64, 3) - 0.5) * 12.0;
    let from = ground
        + ((flat + side / FALL_BEYOND as f32) * FALL_BEYOND as f32).as_dvec3()
        + DVec3::Y * FALL_HEIGHT;
    let fall_secs = ((from - ground).length() as f32 / spell.carrier.speed.max(5.0)).max(0.3);
    let seed = cast.seed as u64;
    let spin = Vec3::new(
        roll(seed, 5) - 0.5,
        roll(seed, 6) - 0.5,
        roll(seed, 7) - 0.5,
    )
    .normalize_or(Vec3::X)
        * (2.0 + 3.0 * roll(seed, 8));
    let up = (ground - from).as_vec3().normalize_or(Vec3::NEG_Y);
    let radius = blast_radius(size);
    let mark = commands
        .spawn((
            Name::new("Meteor mark"),
            Transform::from_translation(ground + DVec3::Y * 0.04),
            MeshRenderer {
                mesh: MeshRef::Shape(Shape::Torus {
                    radius,
                    tube: 0.07,
                    segments: 64,
                }),
                material: MaterialRef::Inline(ward_material(MARK_COLOR, 0.0)),
                cast_shadows: false,
                visible: true,
            },
        ))
        .id();
    let rock = commands
        .spawn((
            Name::new("Meteor rock"),
            Transform::default(),
            MeshRenderer {
                mesh: MeshRef::Shape(Shape::Rock {
                    size: [size, size * 0.85, size * 0.95],
                    seed: (seed % 100_000) as u32,
                    detail: 3,
                }),
                material: MaterialRef::Inline(hot_rock(HOT_GLOW)),
                cast_shadows: true,
                visible: true,
            },
            MeteorRock,
        ))
        .id();
    let e = commands
        .spawn((
            Name::new("Meteor"),
            Transform::from_translation(from).with_rotation(Quat::from_rotation_arc(Vec3::Y, -up)),
            SpellPos { pos: from, up: -up },
            Meteor {
                cast,
                from,
                ground,
                age: 0.0,
                fall_secs,
                size,
                spin,
                mark,
                rock,
            },
        ))
        .id();
    commands.entity(rock).insert(ChildOf(e));
    commands.entity(mark).insert(MeteorMark {
        meteor: e,
        radius,
        age: 0.0,
        fading: None,
    });
    // Its fire (or dust) trails behind it: the spell's bolt look, scaled to the rock.
    queue.looks.push(LookCmd::Attach {
        entity: e,
        look: spell.weave.key.clone(),
        role: Role::Bolt,
        scale: (size / 0.25).clamp(1.0, 10.0),
        color: spell.weave.color,
        power_j: spell.power.energy_j as f32,
    });
}

/// A fault line's jagged path along the ground: every [`PATH_STEP`] m from `start` along `dir`,
/// wandering a little to either side, stopping where the ground steps up or drops (a wall, a
/// ledge).
pub fn fault_path(
    queries: &PhysicsQueries,
    cast: &CastInfo,
    start: DVec3,
    dir: Vec3,
    mover: &dyn Fn(Entity) -> bool,
) -> Vec<DVec3> {
    let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::Z);
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    let exclude: Vec<Entity> = cast.caster.into_iter().collect();
    let len = cast.spell.carrier.range.max(2.0);
    let steps = (len / PATH_STEP).round() as usize;
    let mut path = Vec::with_capacity(steps + 1);
    let mut wander = 0.0f32;
    let mut last_y = start.y;
    for i in 0..=steps {
        let s = i as f32 * PATH_STEP;
        if i > 0 {
            wander += (roll(cast.seed as u64 ^ 0xfa17, i as u64) - 0.5) * 0.36;
            wander = wander.clamp(-0.45, 0.45);
        }
        let flat = start + (dir * s + side * wander).as_dvec3();
        let hit = ground_under(queries, flat + DVec3::Y * 1.2, 3.0, exclude.clone(), mover);
        let Some(h) = hit else { break };
        if i > 0 && ((h.point.y - last_y).abs() > 0.6 || h.normal.y < 0.7) {
            break;
        }
        last_y = h.point.y;
        path.push(h.point);
    }
    path
}

/// The ground under `from` within `max` m: past creatures and loose things standing on it.
pub fn ground_under(
    queries: &PhysicsQueries,
    from: DVec3,
    max: f32,
    exclude: Vec<Entity>,
    mover: &dyn Fn(Entity) -> bool,
) -> Option<engine_world::physics::Hit> {
    let mut filter = QueryFilter {
        exclude,
        ..Default::default()
    };
    for _ in 0..6 {
        let h = queries.raycast(from, Vec3::NEG_Y, max, &filter)?;
        if !mover(h.entity) {
            return Some(h);
        }
        filter.exclude.push(h.entity);
    }
    None
}

/// Bring meteors down: a ward in the way takes the blow (and bursts it there), the ground
/// takes the rest.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn fall_meteors(
    time: Res<Time>,
    mut meteors: Query<(Entity, &mut Meteor, &mut SpellPos)>,
    mut rocks: Query<&mut Transform, With<MeteorRock>>,
    mut barriers: Query<(Entity, &mut Barrier)>,
    mut struck: MessageWriter<WardStruck>,
    factions: Option<Res<Factions>>,
    mut blasts: ResMut<Blasts>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let factions = factions.as_deref();
    for (e, mut m, mut sp) in &mut meteors {
        let was = m.at();
        m.age += dt;
        let now = m.at();
        let step = now - was;
        let len = step.length() as f32;
        let dir = step.as_vec3().normalize_or(Vec3::NEG_Y);
        if let Ok(mut tf) = rocks.get_mut(m.rock) {
            tf.rotation = Quat::from_scaled_axis(m.spin * dt) * tf.rotation;
        }
        // A ward in the way: the first one it crosses into takes the rock's blow.
        let mut stopped: Option<(DVec3, Vec3)> = None;
        if len > 0.0 {
            let mut first: Option<(f32, Vec3, Entity)> = None;
            for (be, b) in &barriers {
                if !b.stands() || !against(&b.cast, &m.cast, factions) {
                    continue;
                }
                if let Some((t, n)) = b.kind.crossing(was, dir, len + m.size * 0.5)
                    && first.is_none_or(|f| t < f.0)
                {
                    first = Some((t, n, be));
                }
            }
            if let Some((t, n, be)) = first
                && let Ok((_, mut b)) = barriers.get_mut(be)
            {
                let spell = m.cast.spell.clone();
                let point = was + (dir * t).as_dvec3();
                let (taken, share, broke) =
                    b.strike(spell.payload.damage * m.cast.scale * 1.5, spell.element());
                struck.write(WardStruck {
                    ward: be,
                    barrier: true,
                    point,
                    dir,
                    taken,
                    share,
                    broke,
                    perfect: false,
                });
                if !broke {
                    stopped = Some((point, n));
                }
            }
        }
        if let Some((point, _)) = stopped {
            // It bursts on the ward: the blast, but no crater.
            blasts.0.push(Blast {
                cast: m.cast.clone(),
                at: point,
                radius: blast_radius(m.size) * 0.6,
                knock_share: 0.0,
                knock_secs: 0.0,
                throw: 4.0,
                line: None,
                spare: Vec::new(),
                meteor: Some((e, m.rock, 0.0)),
            });
            commands.entity(e).remove::<Meteor>();
            continue;
        }
        sp.pos = now;
        if m.age >= m.fall_secs {
            blasts.0.push(Blast {
                cast: m.cast.clone(),
                at: m.ground,
                radius: blast_radius(m.size),
                knock_share: 0.7,
                knock_secs: KNOCK_SECS,
                throw: 9.0 + 2.0 * m.size,
                line: None,
                spare: Vec::new(),
                meteor: Some((e, m.rock, m.size)),
            });
            commands.entity(e).remove::<Meteor>();
        }
    }
}

/// Run the cracks: open the ground as the front passes, throw who stands on it, burst along it
/// in the aftershock, then close it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_faults(
    time: Res<Time>,
    mut faults: Query<(Entity, &mut Fault)>,
    mut pieces: Query<&mut FaultPiece>,
    mut struck_by: ResMut<FaultHits>,
    mut blasts: ResMut<Blasts>,
    mut queue: ResMut<MagicQueue>,
    mut world: MessageWriter<WorldImpulse>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let hits = std::mem::take(&mut struck_by.0);
    for (e, mut f) in &mut faults {
        for (fe, body) in &hits {
            if *fe == e && !f.hit.contains(body) {
                f.hit.push(*body);
            }
        }
        f.age += dt;
        let spell = f.cast.spell.clone();
        let fire = spell.element() == Some(Element::Fire);
        let frost = spell.element() == Some(Element::Frost);
        let reach = spell.carrier.radius.max(0.8);
        // ---- the front runs on
        if f.ended.is_none() {
            f.front += f.speed * dt;
            let open_to = ((f.front / PATH_STEP).floor() as usize + 1).min(f.path.len());
            while f.opened < open_to {
                let i = f.opened;
                f.opened += 1;
                if i == 0 {
                    continue;
                }
                let (a, b) = (f.path[i - 1], f.path[i]);
                let seed = f.cast.seed as u64 ^ (i as u64 * 7919);
                let look = (fire || frost).then(|| (spell.weave.key.clone(), spell.weave.color));
                let made = open_segment(
                    &mut commands,
                    &mut queue,
                    a,
                    b,
                    i,
                    seed,
                    (fire, frost),
                    look,
                );
                f.pieces.extend(made);
                // Whoever stands on it is thrown down.
                blasts.0.push(Blast {
                    cast: f.cast.clone(),
                    at: b,
                    radius: reach,
                    knock_share: 1.0,
                    knock_secs: KNOCK_SECS - 0.3,
                    throw: 5.0,
                    line: Some((a, b)),
                    spare: f.hit.iter().copied().chain(f.cast.caster).collect(),
                    meteor: None,
                });
            }
            if f.opened >= f.path.len() {
                f.ended = Some(f.age);
            }
        }
        // ---- the aftershock
        if let Some(end) = f.ended
            && !f.shook
            && f.age >= end + AFTERSHOCK_SECS
        {
            f.shook = true;
            let scale = 0.6;
            for (k, w) in f.path.windows(2).enumerate().step_by(4) {
                let b = w[1];
                queue.looks.push(LookCmd::Effect {
                    name: "impulse.dust",
                    at: b,
                    up: Vec3::Y,
                    scale: 0.7,
                });
                if fire || frost {
                    queue.looks.push(LookCmd::Burst {
                        at: b + DVec3::Y * 0.1,
                        up: Vec3::Y,
                        look: spell.weave.key.clone(),
                        role: Role::Impact,
                        scale: 0.9,
                        color: spell.weave.color,
                    });
                } else if k % 8 == 0 {
                    queue.looks.push(LookCmd::Effect {
                        name: "spell.stone.impact",
                        at: b,
                        up: Vec3::Y,
                        scale: 1.0,
                    });
                }
            }
            for w in f.path.windows(2) {
                blasts.0.push(Blast {
                    cast: CastInfo {
                        scale: f.cast.scale * scale,
                        ..f.cast.clone()
                    },
                    at: w[1],
                    radius: reach + 0.2,
                    knock_share: 0.0,
                    knock_secs: 0.0,
                    throw: 4.0,
                    line: Some((w[0], w[1])),
                    spare: f.cast.caster.into_iter().collect(),
                    meteor: None,
                });
            }
            let mid = f.path[f.path.len() / 2];
            world.write(WorldImpulse {
                center: mid,
                radius: f.length() * 0.5 + 3.0,
                strength: 14.0,
                energy_j: 2.0e5,
                heat_c: if fire { 600.0 } else { 0.0 },
                source: f.cast.caster,
            });
            for &p in &f.pieces {
                if let Ok(mut piece) = pieces.get_mut(p) {
                    piece.kick = Some(0.0);
                }
            }
        }
        // ---- it closes, then goes
        if f.shook
            && let Some(end) = f.ended
            && f.age >= end + AFTERSHOCK_SECS + CLOSE_AFTER
        {
            if f.closing.is_none() {
                f.closing = Some(0.0);
                for &p in &f.pieces {
                    if let Ok(mut piece) = pieces.get_mut(p) {
                        piece.sink = Some(0.0);
                    }
                }
            }
            if let Some(c) = f.closing.as_mut() {
                *c += dt;
                if *c > CLOSE_SECS + 0.1 {
                    for &p in &f.pieces {
                        commands.entity(p).try_despawn();
                    }
                    commands.entity(e).try_despawn();
                }
            }
        }
    }
}

/// Open one stretch of a crack from `a` to `b`: the dark gap (glowing with a fire weave's lava,
/// pale with frost), a slab heaved up and tilted on one side, and dust.
#[allow(clippy::too_many_arguments)]
fn open_segment(
    commands: &mut Commands,
    queue: &mut MagicQueue,
    a: DVec3,
    b: DVec3,
    i: usize,
    seed: u64,
    (fire, frost): (bool, bool),
    look: Option<(String, [f32; 3])>,
) -> Vec<Entity> {
    let mut made = Vec::new();
    let along = (b - a).as_vec3();
    let len = along.length().max(0.05);
    let dir = Vec3::new(along.x, 0.0, along.z).normalize_or(Vec3::Z);
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    let mid = a + (b - a) * 0.5;
    let yaw = Quat::from_rotation_arc(Vec3::Z, dir);
    // The gap: dark, a little wider in the middle of the line, glowing within with fire.
    let width = 0.26 + 0.14 * roll(seed, 1) + 0.12 * ((i as f32 * 0.37).sin() * 0.5 + 0.5);
    let mut gap = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
    gap.base_color = [0.035, 0.03, 0.028, 1.0];
    gap.perceptual_roughness = 1.0;
    if fire {
        gap.emissive = [1.0, 0.36, 0.06];
        gap.emissive_strength = 14_000.0;
    } else if frost {
        gap.emissive = [0.55, 0.8, 1.0];
        gap.emissive_strength = 3_000.0;
    }
    let crack = commands
        .spawn((
            Name::new("Fault crack"),
            Transform::from_translation(mid + DVec3::Y * 0.012)
                .with_rotation(yaw)
                .with_scale(Vec3::new(0.2, 1.0, 1.0)),
            MeshRenderer {
                mesh: MeshRef::Shape(Shape::Cuboid {
                    size: [width, 0.03, len * 1.25],
                }),
                material: MaterialRef::Inline(gap),
                cast_shadows: false,
                visible: true,
            },
            FaultPiece {
                base: mid + DVec3::Y * 0.012,
                rest: yaw,
                slab: false,
                age: 0.0,
                lift: 0.0,
                kick: None,
                sink: None,
            },
        ))
        .id();
    made.push(crack);
    // A slab of ground heaved up beside it, its near edge lifted, on alternating sides (two
    // here and there where the crack runs widest).
    let sides: &[f32] = if roll(seed, 2) < 0.35 {
        &[-1.0, 1.0]
    } else if i.is_multiple_of(2) {
        &[1.0]
    } else {
        &[-1.0]
    };
    for &s in sides {
        let w = 0.7 + 0.4 * roll(seed, 3 + s as u64);
        let d = 0.45 + 0.3 * roll(seed, 5);
        let h = 0.18 + 0.1 * roll(seed, 6);
        let tilt = (0.22 + 0.28 * roll(seed, 7 + (s > 0.0) as u64)) * s;
        let rest =
            yaw * Quat::from_rotation_y((roll(seed, 9) - 0.5) * 0.5) * Quat::from_rotation_z(-tilt);
        let base = mid + (side * s * (width * 0.5 + d * 0.45)).as_dvec3();
        let mut rock = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
        if frost {
            rock = rock.with_tint(0.78, 0.88, 1.0);
        }
        if fire {
            // The broken edge glows where it faces the lava.
            rock.emissive = [1.0, 0.3, 0.05];
            rock.emissive_strength = 1_200.0;
        }
        let slab = commands
            .spawn((
                Name::new("Fault slab"),
                Transform::from_translation(base - DVec3::Y * 0.3).with_rotation(rest),
                MeshRenderer {
                    mesh: MeshRef::Shape(Shape::Rock {
                        size: [d, h, w.max(len * 1.1)],
                        seed: ((seed ^ (s.to_bits() as u64)) % 100_000) as u32,
                        detail: 2,
                    }),
                    material: MaterialRef::Inline(rock),
                    cast_shadows: true,
                    visible: true,
                },
                FaultPiece {
                    base,
                    rest,
                    slab: true,
                    age: 0.0,
                    lift: 0.05 + 0.12 * roll(seed, 10),
                    kick: None,
                    sink: None,
                },
            ))
            .id();
        made.push(slab);
    }
    if i % 3 == 1 {
        queue.looks.push(LookCmd::Effect {
            name: "spell.stone.step",
            at: mid,
            up: Vec3::Y,
            scale: 1.1,
        });
    }
    if let Some((look, color)) = look.filter(|_| i % 4 == 2) {
        // Fire spits out of the opening crack (frost: a puff of ice).
        queue.looks.push(LookCmd::Burst {
            at: mid + DVec3::Y * 0.05,
            up: Vec3::Y,
            look,
            role: Role::Impact,
            scale: 0.6,
            color,
        });
    }
    made
}

/// Apply this step's blasts: blows (as spell hits), throws, knock-downs; and for a meteor's
/// strike the flash, the world's blast, the crater and its debris.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn apply_blasts(
    mut blasts: ResMut<Blasts>,
    mut fault_hits: ResMut<FaultHits>,
    faults: Query<(Entity, &Fault)>,
    creatures: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&CharacterController>,
            Option<&RigidBody>,
            Option<&Knocked>,
        ),
        CreatureFilter,
    >,
    loose: Query<(Entity, &GlobalTransform, &RigidBody), Without<CharacterController>>,
    queries: Res<PhysicsQueries>,
    colliders: Query<&Collider>,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    mut out: (
        MessageWriter<ApplyImpulse>,
        MessageWriter<WorldImpulse>,
        MessageWriter<NoiseEvent>,
        MessageWriter<TerrainEdit>,
    ),
    mut commands: Commands,
) {
    let (impulses, world, noise, edits) = &mut out;
    // A crack's stretches overlap: each body takes one blow from a crack in a step.
    let mut once: Vec<(u32, Entity)> = Vec::new();
    for b in std::mem::take(&mut blasts.0) {
        let spell = b.cast.spell.clone();
        let near = |p: DVec3| -> Option<f32> {
            match b.line {
                Some((a, c)) => {
                    let ab = (c - a).as_vec3();
                    let ap = (p - a).as_vec3();
                    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
                    let t = (flat(ap).dot(flat(ab)) / flat(ab).length_squared().max(1e-4))
                        .clamp(0.0, 1.0);
                    let d = (flat(ap) - flat(ab) * t).length();
                    (d <= b.radius && (p.y - a.y).abs() < 2.5).then_some(d)
                }
                None => {
                    let d = (p - b.at).length() as f32;
                    (d <= b.radius).then_some(d)
                }
            }
        };
        let fault = b
            .line
            .and_then(|_| faults.iter().find(|(_, f)| f.cast.seed == b.cast.seed))
            .map(|(e, _)| e);
        // ---- creatures and anything with health
        for (t, gt, cc, rb, knocked) in &creatures {
            if b.spare.contains(&t) {
                continue;
            }
            let p = gt.translation();
            let Some(d) = near(p) else { continue };
            if b.line.is_some() {
                if once.contains(&(b.cast.seed, t)) {
                    continue;
                }
                once.push((b.cast.seed, t));
            }
            let k = if b.line.is_some() {
                1.0
            } else {
                falloff(d, b.radius)
            };
            let out_dir =
                Vec3::new((p - b.at).x as f32, 0.0, (p - b.at).z as f32).normalize_or(Vec3::X);
            hits.0.push(PendingHit {
                cast: CastInfo {
                    scale: b.cast.scale * k,
                    ..b.cast.clone()
                },
                target: Some(t),
                point: p + DVec3::Y * 0.9,
                normal: -out_dir,
                dir: out_dir,
                multi: true,
                impact: false,
                reacts: true,
                expire: false,
            });
            if let Some(fe) = fault {
                fault_hits.0.push((fe, t));
            }
            // Thrown off its feet (a body with a skeleton goes limp a while).
            if b.knock_share > 0.0 && d <= b.radius * b.knock_share && knocked.is_none() {
                commands.entity(t).try_insert(Knocked {
                    secs: b.knock_secs,
                    had_ragdoll: false,
                    started: false,
                });
            }
            // A loose dummy on its base is thrown like a crate.
            if cc.is_none()
                && let Some(rb) = rb
                && rb.kind == BodyKind::Dynamic
            {
                let mass = rb.mass_kg.unwrap_or(60.0).max(1.0);
                let v = (out_dir * 0.6 + Vec3::Y) * b.throw * k;
                impulses.write(ApplyImpulse {
                    entity: t,
                    impulse: v * mass,
                    at: None,
                    angular: Vec3::new(out_dir.z, 0.0, -out_dir.x) * mass * 0.3 * k,
                });
            }
        }
        // ---- loose things: crates, barrels, rubble
        for (t, gt, rb) in &loose {
            if rb.kind != BodyKind::Dynamic || creatures.contains(t) || b.spare.contains(&t) {
                continue;
            }
            let p = gt.translation();
            let Some(d) = near(p) else { continue };
            if b.line.is_some() {
                if once.contains(&(b.cast.seed, t)) {
                    continue;
                }
                once.push((b.cast.seed, t));
            }
            let k = falloff(d, b.radius);
            let out_dir =
                Vec3::new((p - b.at).x as f32, 0.0, (p - b.at).z as f32).normalize_or(Vec3::X);
            let mass = rb.mass_kg.unwrap_or(30.0).max(1.0);
            let v =
                (out_dir * if b.line.is_some() { 0.3 } else { 0.9 } + Vec3::Y * 0.8) * b.throw * k;
            impulses.write(ApplyImpulse {
                entity: t,
                impulse: v * mass,
                at: None,
                angular: Vec3::new(out_dir.z, 0.3, -out_dir.x) * mass * 0.5 * k,
            });
        }
        // ---- a meteor's strike
        let Some((m, rock, size)) = b.meteor else {
            continue;
        };
        let fire = spell.element() == Some(Element::Fire);
        let frost = spell.element() == Some(Element::Frost);
        // The spell's own impact where it lands (scorch, freeze, plants), as one hit on the
        // ground.
        hits.0.push(PendingHit {
            cast: b.cast.clone(),
            target: None,
            point: b.at,
            normal: Vec3::Y,
            dir: Vec3::NEG_Y,
            multi: false,
            impact: true,
            reacts: false,
            expire: false,
        });
        let energy = 4.0e5 * size.max(0.3).powi(3) * b.cast.scale;
        let blast = WorldImpulse {
            center: b.at,
            radius: b.radius * 2.5,
            strength: (18.0 + 10.0 * size).min(45.0),
            energy_j: energy,
            heat_c: if fire { 900.0 } else { 0.0 },
            source: b.cast.caster,
        };
        if let Some(n) = blast.noise() {
            noise.write(n);
        }
        world.write(blast);
        queue.looks.push(LookCmd::Effect {
            name: if fire {
                "explosion.large"
            } else {
                "spell.stone.impact"
            },
            at: b.at,
            up: Vec3::Y,
            scale: (0.55 + 0.45 * size).clamp(0.6, 2.2),
        });
        queue.looks.push(LookCmd::Effect {
            name: "impulse.dust",
            at: b.at,
            up: Vec3::Y,
            scale: (0.8 + 0.6 * size).clamp(0.8, 2.5),
        });
        if fire || frost {
            queue.looks.push(LookCmd::Burst {
                at: b.at + DVec3::Y * 0.2,
                up: Vec3::Y,
                look: spell.weave.key.clone(),
                role: Role::Impact,
                scale: (1.0 + size).clamp(1.2, 3.0),
                color: spell.weave.color,
            });
        }
        queue.looks.push(LookCmd::Mark {
            at: b.at,
            normal: Vec3::Y,
            kind: if frost { "frost" } else { "scorch" },
            radius: b.radius * 0.8,
        });
        queue.looks.push(LookCmd::Stop { entity: m });
        commands.spawn((
            Name::new("Meteor strike"),
            Transform::from_translation(b.at + DVec3::Y * 1.5),
            PointLight {
                color: if frost { [0.6, 0.8, 1.0] } else { HOT_COLOR },
                intensity_lm: 0.0,
                range: (b.radius * 4.0).max(12.0),
                radius: 0.5,
            },
            QuakeFlash {
                age: 0.0,
                peak: 4.0e5 * size.max(0.5),
                secs: 0.5,
            },
        ));
        if size <= 0.0 {
            // It burst on a ward: the rock shatters there.
            commands.entity(m).try_despawn();
            continue;
        }
        // On the terrain itself (not a placed floor) the ground is dug out.
        let under = queries.raycast(
            b.at + DVec3::Y * 0.3,
            Vec3::NEG_Y,
            1.0,
            &QueryFilter::default(),
        );
        let terrain = under.is_some_and(|h| {
            colliders
                .get(h.entity)
                .is_ok_and(|c| matches!(c.shape, ColliderShape::Heightfield { .. }))
        });
        let cr = crater_radius(size);
        if terrain {
            edits.write(TerrainEdit {
                center: b.at + DVec3::Y * (cr * 0.55) as f64,
                shape: EditShape::Sphere { radius: cr },
                op: EditOp::Remove,
                surface: None,
                source: b.cast.caster,
            });
        }
        spawn_crater(&mut commands, &mut queue, &b, (m, rock), size, fire, frost);
    }
}

/// The crater a meteor leaves: its boulder half buried in the middle (the meteor's own rock,
/// cooling), a rim of broken rock round it, and hot debris thrown out.
fn spawn_crater(
    commands: &mut Commands,
    queue: &mut MagicQueue,
    b: &Blast,
    (m, rock): (Entity, Entity),
    size: f32,
    fire: bool,
    frost: bool,
) {
    let seed = b.cast.seed as u64 ^ 0xc4a7;
    let cr = crater_radius(size);
    let glow = if frost { [0.55, 0.8, 1.0] } else { HOT_COLOR };
    let mut parts = Vec::new();
    // The boulder: the meteor entity stays, its rock set into the ground.
    commands
        .entity(m)
        .try_remove::<(SpellPos, Meteor)>()
        .try_insert(
            Transform::from_translation(b.at - DVec3::Y * (size * 0.38) as f64)
                .with_rotation(Quat::from_rotation_y(roll(seed, 1) * TAU)),
        );
    parts.push(m);
    // The rim: broken rock heaved up and tipped outward.
    let n = (9.0 + 5.0 * size).round() as usize;
    let mut rim = StandardMaterial::preset(MaterialPreset::Rock).with_triplanar_world();
    if fire {
        rim.emissive = HOT_COLOR;
        rim.emissive_strength = 900.0;
    }
    for i in 0..n {
        let a = (i as f32 + 0.5 * roll(seed, 10 + i as u64)) / n as f32 * TAU;
        let out = Vec3::new(a.cos(), 0.0, a.sin());
        let r = cr * (0.85 + 0.25 * roll(seed, 40 + i as u64));
        let s = (0.22 + 0.22 * size) * (0.7 + 0.6 * roll(seed, 70 + i as u64));
        let at = b.at + (out * r).as_dvec3() - DVec3::Y * (s * 0.3) as f64;
        let rot = Quat::from_rotation_arc(Vec3::Y, (Vec3::Y + out * 0.7).normalize())
            * Quat::from_rotation_y(roll(seed, 100 + i as u64) * TAU);
        let e = commands
            .spawn((
                Name::new("Crater rim"),
                Transform::from_translation(at).with_rotation(rot),
                MeshRenderer {
                    mesh: MeshRef::Shape(Shape::Rock {
                        size: [s * 1.4, s, s * 1.1],
                        seed: ((seed + i as u64 * 31) % 100_000) as u32,
                        detail: 2,
                    }),
                    material: MaterialRef::Inline(rim),
                    cast_shadows: true,
                    visible: true,
                },
            ))
            .id();
        parts.push(e);
    }
    // Hot debris flung out: rocks that tumble, settle and sink away.
    let debris = (5.0 + 4.0 * size).round() as usize;
    let hot = {
        let mut m = hot_rock(if fire { 9_000.0 } else { 0.0 });
        if frost {
            m = StandardMaterial::preset(MaterialPreset::Rock)
                .with_triplanar_world()
                .with_tint(0.8, 0.9, 1.0);
        }
        m
    };
    for i in 0..debris {
        let a = roll(seed, 200 + i as u64) * TAU;
        let out = Vec3::new(a.cos(), 0.0, a.sin());
        let s = (0.12 + 0.12 * size) * (0.6 + 0.8 * roll(seed, 230 + i as u64));
        let speed = 5.0 + 6.0 * roll(seed, 260 + i as u64);
        let up = 4.0 + 5.0 * roll(seed, 290 + i as u64);
        commands.spawn((
            Name::new("Meteor debris"),
            Transform::from_translation(b.at + (out * 0.5 * size).as_dvec3() + DVec3::Y * 0.6)
                .with_rotation(Quat::from_rotation_y(a)),
            MeshRenderer {
                mesh: MeshRef::Shape(Shape::Rock {
                    size: [s, s * 0.8, s * 0.9],
                    seed: ((seed + 977 * i as u64) % 100_000) as u32,
                    detail: 1,
                }),
                material: MaterialRef::Inline(hot),
                cast_shadows: true,
                visible: true,
            },
            Collider {
                shape: ColliderShape::Cuboid {
                    half_extents: Vec3::new(s * 0.45, s * 0.36, s * 0.4),
                },
                density: 2600.0,
                ..Default::default()
            },
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some((s * s * s * 2000.0).clamp(1.0, 200.0)),
                linear_damping: 0.05,
                angular_damping: 0.3,
                ..Default::default()
            },
            Velocity {
                linear: out * speed + Vec3::Y * up,
                angular: Vec3::new(out.z, 0.4, -out.x) * 9.0,
            },
            Rubble {
                age: 0.0,
                life: 7.0 + 2.0 * roll(seed, 320 + i as u64),
            },
        ));
    }
    // A fire meteor leaves the crater burning: flames on its floor round the boulder.
    if fire {
        for i in 0..3 {
            let a = (i as f32 / 3.0 + roll(seed, 400)) * TAU;
            let r = (size * 0.55 + 0.3) * (0.8 + 0.4 * roll(seed, 401 + i as u64));
            queue.looks.push(LookCmd::Effect {
                name: "fire.torch",
                at: b.at + DVec3::new((a.cos() * r) as f64, 0.05, (a.sin() * r) as f64),
                up: Vec3::Y,
                scale: (1.2 + 0.5 * size).clamp(1.2, 2.6),
            });
        }
    }
    commands.spawn((
        Name::new("Crater"),
        Transform::from_translation(b.at),
        Crater {
            at: b.at,
            radius: cr,
            age: 0.0,
            burning: fire,
            boulder: rock,
            parts,
            glow,
        },
    ));
}

/// Keep Cataclysm's looks: the telegraph rings pulse faster and brighter as their rocks near
/// and fade once they land; craters cool and, in time, sink away; strikes' light fades; the
/// crack's pieces open, heave, kick in the aftershock and sink; the knocked-down lie limp, then
/// stand.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn tend_cataclysm(
    time: Res<Time>,
    mut marks: Query<(
        Entity,
        &mut MeteorMark,
        &mut Transform,
        Option<&mut MeshRenderer>,
    )>,
    meteors: Query<&Meteor>,
    mut craters: Query<(Entity, &mut Crater), Without<MeteorMark>>,
    mut parts: Query<
        (&mut Transform, Option<&mut MeshRenderer>),
        (Without<MeteorMark>, Without<FaultPiece>),
    >,
    mut pieces: Query<(&mut FaultPiece, &mut Transform), Without<MeteorMark>>,
    mut flashes: Query<(Entity, &mut QuakeFlash, &mut PointLight)>,
    mut knocked: Query<(Entity, &mut Knocked, Option<&Ragdoll>, Has<SkeletonPose>)>,
    (mut materials, mut painted): (
        Option<ResMut<Assets<StandardMaterial>>>,
        Local<HashMap<Entity, Handle<StandardMaterial>>>,
    ),
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    if let Some(mats) = materials.as_deref_mut() {
        painted.retain(|&e, h| {
            let live = marks.contains(e) || parts.contains(e);
            if !live {
                mats.remove(*h);
            }
            live
        });
    }
    let mut paint = Paint::new(materials.as_deref_mut(), &mut painted);
    // ---- telegraph rings
    for (e, mut mk, mut tf, r) in &mut marks {
        mk.age += dt;
        let (glow, cover) = match (meteors.get(mk.meteor), mk.fading.as_mut()) {
            (Ok(m), None) => {
                // Pulses from 2 a second to 9 as the rock nears, brightening.
                let near = 1.0 - (m.eta() / m.fall_secs.max(0.1)).clamp(0.0, 1.0);
                let rate = 2.0 + 7.0 * near * near;
                let pulse = 0.5 + 0.5 * (mk.age * rate * TAU).sin();
                let swell = (mk.age / 0.2).clamp(0.0, 1.0);
                tf.scale = Vec3::new(1.0, 1.0, 1.0) * (0.92 + 0.08 * pulse);
                ((6_000.0 + 22_000.0 * near) * (0.55 + 0.45 * pulse), swell)
            }
            (_, fading) => {
                let f = match fading {
                    Some(f) => {
                        *f += dt;
                        *f
                    }
                    None => {
                        mk.fading = Some(0.0);
                        0.0
                    }
                };
                if f > 0.35 {
                    commands.entity(e).try_despawn();
                    continue;
                }
                let k = 1.0 - f / 0.35;
                tf.scale = Vec3::splat(1.0 + 0.6 * f / 0.35);
                (30_000.0 * k, k)
            }
        };
        if let Some(mut r) = r {
            paint.glow_clear(e, &mut r, MARK_COLOR, glow, cover);
        }
    }
    // ---- craters cool, then sink away
    for (e, mut c) in &mut craters {
        c.age += dt;
        let heat = (1.0 - c.age / COOL_SECS).clamp(0.0, 1.0);
        // Yellow-white, to orange, to a dull red, to dark.
        let color = [
            c.glow[0],
            c.glow[1] * (0.35 + 0.65 * heat * heat),
            c.glow[2] * heat * heat,
        ];
        let boulder = c.boulder;
        if let Ok((_, Some(mut r))) = parts.get_mut(boulder) {
            paint.glow(boulder, &mut r, color, HOT_GLOW * heat.powf(1.5));
        }
        if c.age > CRATER_SECS {
            let sink = c.age - CRATER_SECS;
            for &p in &c.parts {
                if let Ok((mut tf, _)) = parts.get_mut(p) {
                    tf.translation.y -= (0.5 * dt) as f64;
                }
            }
            if sink > 3.0 {
                for &p in &c.parts {
                    commands.entity(p).try_despawn();
                }
                commands.entity(e).try_despawn();
            }
        }
    }
    // ---- the strikes' light
    for (e, mut f, mut light) in &mut flashes {
        f.age += dt;
        if f.age >= f.secs {
            commands.entity(e).try_despawn();
            continue;
        }
        let k = 1.0 - f.age / f.secs;
        light.intensity_lm = f.peak * k * k;
    }
    // ---- the crack's pieces
    for (mut p, mut tf) in &mut pieces {
        p.age += dt;
        if p.slab {
            // Heaves up fast with a small overshoot, settles; kicked again in the aftershock.
            let up = crate::stone::rise(p.age, 0.22);
            let kick = p.kick.map_or(0.0, |k| {
                let t = (k / 0.35).clamp(0.0, 1.0);
                (t * std::f32::consts::PI).sin() * 0.18
            });
            let sink = p.sink.map_or(0.0, |s| (s / CLOSE_SECS).clamp(0.0, 1.0));
            let y = -0.3 + (0.3 + p.lift) * up * (1.0 - sink) + kick - 0.25 * sink;
            tf.translation = p.base + DVec3::Y * y as f64;
            let wobble = p
                .kick
                .map_or(0.0, |k| (k * 40.0).sin() * 0.03 * (1.0 - k).max(0.0));
            tf.rotation = p.rest * Quat::from_rotation_x(wobble);
        } else {
            // The gap opens out to its width, then closes.
            let open = (p.age / 0.18).clamp(0.0, 1.0);
            let sink = p.sink.map_or(0.0, |s| (s / CLOSE_SECS).clamp(0.0, 1.0));
            tf.scale = Vec3::new((0.2 + 0.8 * open) * (1.0 - 0.9 * sink), 1.0, 1.0);
            tf.translation = p.base - DVec3::Y * (0.02 * sink) as f64;
        }
        if let Some(k) = p.kick.as_mut() {
            *k += dt;
        }
        if let Some(s) = p.sink.as_mut() {
            *s += dt;
        }
    }
    // ---- thrown down, then up again
    for (e, mut k, rag, skeleton) in &mut knocked {
        if !k.started {
            k.started = true;
            k.had_ragdoll = rag.is_some();
            if !k.had_ragdoll && skeleton {
                commands.entity(e).try_insert(Ragdoll { weight: 1.0 });
            }
        }
        k.secs -= dt;
        if k.secs <= 0.0 {
            if !k.had_ragdoll && skeleton {
                commands.entity(e).try_remove::<Ragdoll>();
            }
            commands.entity(e).try_remove::<Knocked>();
        }
    }
}

/// Cataclysm keeps world positions of its own; move them when the world re-anchors.
pub fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut meteors: Query<&mut Meteor>,
    mut faults: Query<&mut Fault>,
    mut craters: Query<&mut Crater>,
    mut pieces: Query<&mut FaultPiece>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    for mut m in &mut meteors {
        m.from = s.point(m.from);
        m.ground = s.point(m.ground);
    }
    for mut f in &mut faults {
        for p in f.path.iter_mut() {
            *p = s.point(*p);
        }
    }
    for mut c in &mut craters {
        c.at = s.point(c.at);
    }
    for mut p in &mut pieces {
        p.base = s.point(p.base);
    }
}

/// Take every meteor, ring, crater, crack and strike light away (the hall's restart).
pub fn clear(world: &mut World) {
    let mut q = world.query_filtered::<Entity, Or<(
        With<Meteor>,
        With<MeteorMark>,
        With<Crater>,
        With<Fault>,
        With<FaultPiece>,
        With<QuakeFlash>,
    )>>();
    let gone: Vec<Entity> = q.iter(world).collect();
    let mut parts = Vec::new();
    for &e in &gone {
        if let Some(c) = world.get::<Crater>(e) {
            parts.extend(c.parts.iter().copied());
        }
    }
    for e in gone.into_iter().chain(parts) {
        if let Ok(em) = world.get_entity_mut(e) {
            em.despawn();
        }
    }
    world.resource_mut::<Blasts>().0.clear();
    world.resource_mut::<FaultHits>().0.clear();
}

pub fn register(app: &mut App) {
    app.init_resource::<Blasts>()
        .init_resource::<FaultHits>()
        .register_api(
            "magic.cataclysm",
            "Cataclysm now: meteors in flight (where, where they land, seconds to landing, size \
             m, blast radius m), craters (where, radius, seconds since the strike, how hot the \
             boulder still is 0-1) and fault lines (start, end, metres opened of total, \
             whether the aftershock has gone, who it has thrown).",
            "{}",
            |world, _args| {
                let mut q = world.query::<(Entity, &Meteor)>();
                let meteors: Vec<serde_json::Value> = q
                    .iter(world)
                    .map(|(e, m)| {
                        let at = m.at();
                        serde_json::json!({
                            "id": e.to_bits(),
                            "at": [at.x, at.y, at.z],
                            "lands_at": [m.ground.x, m.ground.y, m.ground.z],
                            "eta_s": m.eta(),
                            "size_m": m.size,
                            "blast_m": blast_radius(m.size),
                        })
                    })
                    .collect();
                let mut q = world.query::<(Entity, &Crater)>();
                let craters: Vec<serde_json::Value> = q
                    .iter(world)
                    .map(|(e, c)| {
                        serde_json::json!({
                            "id": e.to_bits(),
                            "at": [c.at.x, c.at.y, c.at.z],
                            "radius_m": c.radius,
                            "age_s": c.age,
                            "heat": (1.0 - c.age / COOL_SECS).clamp(0.0, 1.0),
                        })
                    })
                    .collect();
                let mut q = world.query::<(Entity, &Fault)>();
                let faults: Vec<serde_json::Value> = q
                    .iter(world)
                    .map(|(e, f)| {
                        let a = f.path.first().copied().unwrap_or_default();
                        let b = f.path.last().copied().unwrap_or_default();
                        serde_json::json!({
                            "id": e.to_bits(),
                            "start": [a.x, a.y, a.z],
                            "end": [b.x, b.y, b.z],
                            "opened_m": (f.front).min(f.length()),
                            "length_m": f.length(),
                            "aftershock": f.shook,
                            "thrown": f.hit.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({
                    "meteors": meteors,
                    "craters": craters,
                    "faults": faults,
                }))
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blast_is_wider_than_the_rock_and_the_crater_between() {
        for size in [0.5, 1.0, 2.0] {
            let b = blast_radius(size);
            let c = crater_radius(size);
            assert!(b > 2.0 * size && b > c && c > size * 0.5, "{size}: {b} {c}");
        }
        assert_eq!(falloff(0.0, 4.0), 1.0);
        assert!((falloff(4.0, 4.0) - 0.4).abs() < 1e-6);
        assert!(falloff(2.0, 4.0) > 0.8);
    }
}
