//! Life (round 19, `engine-design/magic/round-19-plan.md`): healing you can see.
//!
//! * **Wounds.** A golem or a treant sheds a piece at each mark of [`SHED_AT`] (three quarters,
//!   half and a quarter of its health): a shoulder plate, then the other, then its left arm
//!   from the elbow ([`SHED_TIERS`]). The piece drops to the floor as rubble ([`ShedPiece`]) and
//!   lies there. Healing back past a mark flies it up from where it lies into its place
//!   ([`Mend`]), and a ring of light closes round it where it seats.
//! * **Revive.** A living golem or treant that falls leaves its pieces lying for
//!   [`REVIVE_SECS`] ([`Fallen`]). A heal from its own side that lands within [`REVIVE_M`] of
//!   where it fell puts it back: its pieces rise off the floor and fly together over
//!   [`ASSEMBLE_SECS`], its eyes light last, and it stands with [`REVIVE_HEALTH`] of its health.
//!   A creature's healing (a wisp's) mends the living but raises no one.
//! * **The dead burn.** Healing that reaches a skeleton, a skeletal mage, a bone golem or a bone
//!   dragon hurts it by as much as it would have healed ([`SEAR`]; hits.rs), and gold fire bursts
//!   off its bones.
//! * **Glows.** A healed body gets a ring of light that runs up it from its feet and closes over
//!   its head, and motes of light rising off it ([`HealRing`], [`Mote`]), at most one every
//!   [`GLOW_EVERY`] per body so a healing circle's ticks don't pile up.
//! * **Healing circle.** An area or rune of healing lays a ring of light on the floor at its
//!   edge; blossoms open across it from the middle out among leaves, and a wave of light runs out
//!   from its middle every [`PULSE_EVERY`] while it lasts ([`Circle`]). Then the blossoms close
//!   and go. A wave that passes over a fallen creature of its side revives it.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{Image, MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::physics::{
    BodyKind, Collider, ColliderShape, PhysicsQueries, QueryFilter, RigidBody, Velocity,
};

use crate::caster::Vitality;
use crate::looks::{LookCmd, Role};
use crate::runtime::{MagicQueue, roll};
use crate::summon::{
    ARM, Act, CORE, EYE, Minion, MinionKind, MinionPart, NewMinion, PLATE, Rubble, gait_of,
};

/// Health shares at which a golem or treant sheds a piece.
pub const SHED_AT: [f32; 3] = [0.75, 0.5, 0.25];
/// What it sheds at each mark (its pieces by index): a shoulder plate, the other, then its left
/// forearm and fist.
pub const SHED_TIERS: [&[usize]; 3] = [&[PLATE[0]], &[PLATE[1]], &[ARM[0] + 1, ARM[0] + 2]];
/// How far past a mark healing must bring it before the piece comes back (so a body hovering
/// on a mark doesn't flicker a piece off and on).
pub const MEND_OVER: f32 = 0.04;
/// Seconds a shed piece takes to fly back into place.
pub const MEND_SECS: f32 = 0.9;
/// Seconds a fallen golem or treant can be revived (its pieces lie that long).
pub const REVIVE_SECS: f32 = 14.0;
/// How near where it fell a heal must land to revive it, m.
pub const REVIVE_M: f32 = 2.5;
/// Seconds a revived body's pieces take to fly together (each starts a little after the last).
pub const ASSEMBLE_SECS: f32 = 1.6;
/// The share of its health a revived body stands with.
pub const REVIVE_HEALTH: f32 = 0.5;
/// Healing on the dead hurts them by this much of the heal: a healing circle burns a skeleton
/// in it down over a few of its waves rather than at once.
pub const SEAR: f32 = 0.4;
/// The most of its health one touch of healing burns off the dead, so a skeleton in a circle
/// burns over three of its waves, gold fire each time.
pub const SEAR_CAP: f32 = 0.34;
/// Light's colour: warm gold.
pub const LIFE: [f32; 3] = [1.0, 0.82, 0.42];
/// A glow's brightness, nits: bright enough to read in the daylit hall (magic-hall-pictures).
pub const LIFE_NITS: f32 = 14_000.0;
/// A ring of light's colour: a deeper gold than [`LIFE`], so it blooms gold rather than cream.
pub const RING: [f32; 3] = [1.0, 0.62, 0.2];
/// A ring's (and a mote's) brightness, nits: a line of light, not a hoop.
pub const RING_NITS: f32 = 32_000.0;
/// Least seconds between two glows on one body.
pub const GLOW_EVERY: f32 = 0.45;
/// Seconds a ring of light takes to run up a body.
pub const RING_SECS: f32 = 0.85;
/// Motes of light off a healed body.
pub const MOTES: u32 = 9;
/// Seconds between a healing circle's waves.
pub const PULSE_EVERY: f32 = 1.0;
/// Seconds a wave takes to run out to the circle's edge.
pub const WAVE_SECS: f32 = 0.8;
/// Seconds a burst of healing (an area with no linger) keeps its blossoms.
pub const BURST_SECS: f32 = 2.2;

/// Whether healing burns this creature (it is dead and walking).
pub fn burns(m: &Minion) -> bool {
    m.kind.skeletal() || m.kind == MinionKind::BoneDragon || m.cast.spell.payload.raise > 0.0
}

/// A heal that landed this step (hits.rs): on a body, or on the floor or rubble.
#[derive(Debug, Clone)]
pub struct HealAt {
    pub at: DVec3,
    pub target: Option<Entity>,
    /// Health it gave (or took from the dead).
    pub amount: f32,
    /// It burned the dead.
    pub sear: bool,
    /// The healer's side: it revives that side's fallen.
    pub side: u32,
    /// A mage's heal revives; a wisp's light, or a drop of healing rain, mends the living but
    /// raises no one.
    pub revives: bool,
    /// The body's height and girth, for its glow (it may be gone by the time it shows).
    pub height: f32,
    pub radius: f32,
}

/// An area of healing to lay on the floor (runtime's flush).
#[derive(Debug, Clone)]
pub struct NewCircle {
    /// The lasting field it shows (None: a burst, gone in [`BURST_SECS`]).
    pub field: Option<Entity>,
    pub at: DVec3,
    pub radius: f32,
    pub side: u32,
    pub seed: u32,
}

/// A golem's or treant's wounds: pieces shed, pieces flying back.
#[derive(Component, Debug, Clone, Default)]
pub struct Wounds {
    /// Pieces shed, by mark: each piece's index and the rubble lying where it fell.
    pub shed: Vec<Vec<(usize, Entity)>>,
    pub mends: Vec<Mend>,
    /// Seconds left of a revive's assembly (its glowing pieces stay dark until it's done).
    pub assembling: f32,
}

impl Wounds {
    /// Pieces hidden now (shed and not flying back).
    pub fn hidden(&self) -> impl Iterator<Item = usize> + '_ {
        self.shed.iter().flatten().map(|(i, _)| *i)
    }
}

/// A piece flying back into place from where it lay.
#[derive(Debug, Clone, Copy)]
pub struct Mend {
    pub index: usize,
    pub from: DVec3,
    pub from_rot: Quat,
    pub age: f32,
    /// Seconds before it lifts.
    pub delay: f32,
    pub secs: f32,
}

/// A piece a creature shed, lying on the floor.
#[derive(Component, Debug, Clone, Copy)]
pub struct ShedPiece {
    pub owner: Entity,
    pub index: usize,
    /// Seconds its owner has been gone.
    pub orphan: f32,
}

/// A living golem or treant that fell: its pieces lie where they fell until it is revived or
/// they sink ([`REVIVE_SECS`]).
#[derive(Component, Debug, Clone)]
pub struct Fallen {
    /// The creature it was.
    pub was: Entity,
    /// How to make it again.
    pub new: NewMinion,
    /// Seconds of its summons left when it fell.
    pub left: f32,
    pub age: f32,
    /// Its pieces lying on the floor, by index.
    pub pieces: Vec<(usize, Entity)>,
}

/// A body revived this step: its health and time to set once it is made.
#[derive(Component, Debug, Clone, Copy)]
pub struct Revived {
    pub left: f32,
}

/// A ring of light running up a healed body.
#[derive(Component, Debug, Clone, Copy)]
pub struct HealRing {
    pub target: Option<Entity>,
    /// Its feet when it started (it follows the body while the body lives).
    pub base: DVec3,
    pub height: f32,
    pub radius: f32,
    pub age: f32,
    /// Runs down from the head instead (closing over a mended piece runs both ways).
    pub down: bool,
}

/// A mote of light rising off a healed body.
#[derive(Component, Debug, Clone, Copy)]
pub struct Mote {
    pub center: DVec3,
    pub radius: f32,
    pub phase: f32,
    pub rise: f32,
    pub age: f32,
    pub life: f32,
    pub size: f32,
}

/// A healing circle on the floor.
#[derive(Component, Debug, Clone)]
pub struct Circle {
    pub field: Option<Entity>,
    pub center: DVec3,
    pub radius: f32,
    pub side: u32,
    pub age: f32,
    pub pulse_in: f32,
    /// Its blossoms and leaves: entity, when it opens (s), full size.
    pub blooms: Vec<(Entity, f32, f32)>,
    pub ring: Entity,
    /// Seconds since it began to close (None while it lasts).
    pub closing: Option<f32>,
}

/// A wave of light running out across a healing circle.
#[derive(Component, Debug, Clone, Copy)]
pub struct Wave {
    pub center: DVec3,
    pub radius: f32,
    pub age: f32,
}

/// The blossom picture (life_art.rs), painted at startup.
#[derive(Resource, Default)]
pub struct LifeMeshes {
    blossom: Option<Handle<Image>>,
}

impl LifeMeshes {
    pub fn blossom(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        *self
            .blossom
            .get_or_insert_with(|| images.add(crate::life_art::blossom_image()))
    }
}

fn flat(d: DVec3) -> f64 {
    DVec3::new(d.x, 0.0, d.z).length()
}

/// The floor under a point, past bodies and pieces.
fn floor_under(queries: &PhysicsQueries, at: DVec3, solid: &dyn Fn(Entity) -> bool) -> DVec3 {
    let mut filter = QueryFilter::default();
    for _ in 0..8 {
        let Some(h) = queries.raycast(at + DVec3::Y * 0.5, Vec3::NEG_Y, 8.0, &filter) else {
            return at;
        };
        if solid(h.entity) {
            return h.point;
        }
        filter.exclude.push(h.entity);
    }
    at
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn light(color: [f32; 3], nits: f32) -> StandardMaterial {
    StandardMaterial::default()
        .with_tint(color[0], color[1], color[2])
        .with_emissive(color, nits)
}

fn ring_mesh(tube: f32) -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Shape(Shape::Torus {
            radius: 1.0,
            tube,
            segments: 48,
        }),
        MaterialRef::Inline(light(RING, RING_NITS)),
    )
    .without_shadows()
}

fn mote_mesh() -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Shape(Shape::Sphere {
            radius: 1.0,
            segments: 8,
        }),
        MaterialRef::Inline(light([1.0, 0.84, 0.46], RING_NITS)),
    )
    .without_shadows()
}

/// How tall and wide a body is, for its glow: a creature by its kind, anything else as a
/// person.
pub fn body_of(m: Option<&Minion>) -> (f32, f32) {
    match m {
        Some(m) => (
            crate::summon::body_height(m.kind, m.size),
            crate::summon::body_radius(m.kind, m.size) * 1.2,
        ),
        None => (1.8, 0.5),
    }
}

/// A ring of light up a body and motes rising off it.
fn glow(
    commands: &mut Commands,
    base: DVec3,
    height: f32,
    radius: f32,
    target: Option<Entity>,
    seed: u64,
    down: bool,
) {
    commands.spawn((
        Name::new("Healing light"),
        Transform::from_translation(base).with_scale(Vec3::ZERO),
        ring_mesh(0.028),
        HealRing {
            target,
            base,
            height,
            radius,
            age: 0.0,
            down,
        },
    ));
    for i in 0..MOTES {
        let r = |s: u64| roll(seed ^ (i as u64 * 977), s);
        commands.spawn((
            Name::new("Mote of light"),
            Transform::from_translation(base).with_scale(Vec3::ZERO),
            mote_mesh(),
            Mote {
                center: base,
                radius: radius * (0.5 + 0.6 * r(1)),
                phase: r(2) * TAU,
                rise: height * (0.6 + 0.6 * r(3)),
                age: -0.3 * r(4),
                life: 1.0 + 0.6 * r(5),
                size: 0.03 + 0.03 * r(6),
            },
        ));
    }
}

/// Heals landed: glows on the healed, gold fire off the dead, revives near fallen pieces, and
/// healing circles laid.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn land_heals(
    time: Res<Time>,
    mut queue: ResMut<MagicQueue>,
    bodies: Query<(&GlobalTransform, Option<&Minion>)>,
    mut fallen: Query<(Entity, &mut Fallen)>,
    pieces: Query<&Transform, With<Rubble>>,
    wm: Option<ResMut<crate::wilds::WildsMeshes>>,
    lm: Option<ResMut<LifeMeshes>>,
    meshes: Option<ResMut<Assets<engine_render::Mesh>>>,
    images: Option<ResMut<Assets<Image>>>,
    mut last: Local<HashMap<Entity, f32>>,
    mut clock: Local<f32>,
    (queries, creatures): (
        Res<PhysicsQueries>,
        Query<(), Or<(With<Vitality>, With<Minion>, With<MinionPart>, With<Rubble>)>>,
    ),
    mut commands: Commands,
) {
    *clock += time.delta_secs();
    let now = *clock;
    let heals = std::mem::take(&mut queue.heals);
    // An area cast at a body goes off at its middle: lay the circle on the floor under it.
    let solid = |e: Entity| !creatures.contains(e);
    let circles: Vec<NewCircle> = std::mem::take(&mut queue.circles)
        .into_iter()
        .map(|mut c| {
            c.at = floor_under(&queries, c.at, &solid);
            c
        })
        .collect();
    if heals.is_empty() && circles.is_empty() {
        return;
    }
    last.retain(|_, t| now - *t < 5.0);
    let mut looks = Vec::new();
    let mut revived: Vec<Entity> = Vec::new();
    let lie = |p: Entity| pieces.get(p).ok().map(|t| (t.translation, t.rotation));
    for h in heals {
        let body = h.target.map(|t| bodies.get(t).ok());
        if let Some(t) = h.target
            && h.amount > 0.0
            && body
                .flatten()
                .is_none_or(|(_, m)| m.is_none_or(|m| m.kind != MinionKind::Wisp))
            && last.get(&t).is_none_or(|&at| now - at >= GLOW_EVERY)
        {
            last.insert(t, now);
            let (height, radius) = (h.height, h.radius);
            // Where it stands now, or where the heal found it (a skeleton it burned to bones).
            let base = body.flatten().map_or(h.at, |(g, _)| g.translation());
            if h.sear {
                // Gold fire off the bones.
                looks.push(LookCmd::Burst {
                    at: base + DVec3::Y * (height * 0.55) as f64,
                    up: Vec3::Y,
                    look: "radiance".into(),
                    role: Role::Impact,
                    scale: (height / 1.75).clamp(0.6, 2.0) * 0.8,
                    color: LIFE,
                });
                glow(
                    &mut commands,
                    base,
                    height,
                    radius * 0.8,
                    Some(t),
                    t.to_bits() ^ now.to_bits() as u64,
                    true,
                );
            } else {
                glow(
                    &mut commands,
                    base,
                    height,
                    radius,
                    Some(t),
                    t.to_bits() ^ now.to_bits() as u64,
                    false,
                );
            }
        }
        if !h.sear && h.revives {
            for (fe, f) in &mut fallen {
                if f.new.cast.side == h.side
                    && !revived.contains(&fe)
                    && near(&f, &lie, h.at, REVIVE_M)
                {
                    revived.push(fe);
                }
            }
        }
    }
    for fe in revived {
        if let Ok((_, f)) = fallen.get(fe) {
            revive(&mut commands, fe, f, &lie, &mut looks);
        }
    }
    if !circles.is_empty() {
        let (Some(mut wm), Some(mut lm), Some(mut meshes), Some(mut images)) =
            (wm, lm, meshes, images)
        else {
            // No renderer (headless tests): the circle still pulses and revives.
            for c in circles {
                lay_circle(&mut commands, c, None);
            }
            queue.looks.extend(looks);
            return;
        };
        let card = wm.card(&mut meshes);
        let leaf = wm.leaf(&mut images);
        let blossom = lm.blossom(&mut images);
        for c in circles {
            lay_circle(&mut commands, c, Some((card, leaf, blossom)));
        }
    }
    queue.looks.extend(looks);
}

/// Whether a fallen creature's pieces lie within `reach` of a point.
fn near(f: &Fallen, lie: &dyn Fn(Entity) -> Option<(DVec3, Quat)>, at: DVec3, reach: f32) -> bool {
    let reach = reach as f64;
    f.new.at.distance(at) <= reach
        || f.pieces
            .iter()
            .any(|(_, p)| lie(*p).is_some_and(|(t, _)| t.distance(at) <= reach))
}

/// Make the fallen creature again: its pieces fly together from where they lie.
fn revive(
    commands: &mut Commands,
    fe: Entity,
    f: &Fallen,
    lie: &dyn Fn(Entity) -> Option<(DVec3, Quat)>,
    looks: &mut Vec<LookCmd>,
) {
    let id = crate::summon::spawn_minion(commands, f.new.clone(), looks);
    let n = f.pieces.len().max(1) as f32;
    let mut mends = Vec::new();
    // Feet first, head and plates last: each piece lifts a little after the one before it.
    let mut order: Vec<(usize, Entity)> = f.pieces.clone();
    order.sort_by_key(|(i, _)| *i);
    for (j, (index, p)) in order.iter().enumerate() {
        let Some((from, from_rot)) = lie(*p) else {
            continue;
        };
        mends.push(Mend {
            index: *index,
            from,
            from_rot,
            age: 0.0,
            delay: j as f32 / n * ASSEMBLE_SECS * 0.45,
            secs: ASSEMBLE_SECS * 0.55,
        });
        commands.entity(*p).despawn();
    }
    commands.entity(id).insert((
        Wounds {
            shed: Vec::new(),
            mends,
            assembling: ASSEMBLE_SECS,
        },
        Revived { left: f.left },
    ));
    looks.push(LookCmd::Burst {
        at: f.new.at + DVec3::Y,
        up: Vec3::Y,
        look: "life".into(),
        role: Role::Impact,
        scale: 1.6,
        color: LIFE,
    });
    commands.entity(fe).despawn();
}

/// Lay a healing circle: its ring at the edge, and blossoms and leaves across it that open from
/// the middle out.
fn lay_circle(
    commands: &mut Commands,
    c: NewCircle,
    art: Option<(Handle<engine_render::Mesh>, Handle<Image>, Handle<Image>)>,
) {
    let ground = c.at + DVec3::Y * 0.04;
    let ring = commands
        .spawn((
            Name::new("Healing circle"),
            Transform::from_translation(ground).with_scale(Vec3::ZERO),
            ring_mesh(0.016),
        ))
        .id();
    let mut blooms = Vec::new();
    if let Some((card, leaf, blossom)) = art {
        let flower = StandardMaterial::default()
            .with_base_color_texture(blossom)
            .with_alpha_mask(0.5)
            .with_emissive([1.0, 0.88, 0.6], 1_400.0)
            .with_roughness(0.5);
        let green = StandardMaterial::default()
            .with_base_color_texture(leaf)
            .with_alpha_mask(0.5)
            .with_subsurface(0.3)
            .with_roughness(0.6);
        let count = ((c.radius * c.radius * 7.0) as usize).clamp(14, 90);
        let seed = c.seed as u64;
        for i in 0..count {
            let r = |s: u64| roll(seed ^ (i as u64).wrapping_mul(7919), s);
            // Even over the disc, kept off the very edge where the ring runs.
            let d = c.radius * 0.94 * r(1).sqrt();
            let a = r(2) * TAU;
            let is_leaf = r(3) < 0.35;
            let size = if is_leaf {
                0.42 + 0.2 * r(4)
            } else {
                0.24 + 0.16 * r(4)
            };
            let at = ground
                + DVec3::new(
                    (a.cos() * d) as f64,
                    (0.01 + 0.03 * r(5)) as f64,
                    (a.sin() * d) as f64,
                );
            // Laid on the floor facing up, turned at random, tipped a little.
            let rot = Quat::from_rotation_y(r(6) * TAU)
                * Quat::from_rotation_x(-PI / 2.0 + (r(7) - 0.5) * 0.5);
            let e = commands
                .spawn((
                    Name::new(if is_leaf { "Circle leaf" } else { "Blossom" }),
                    Transform::from_translation(at)
                        .with_rotation(rot)
                        .with_scale(Vec3::ZERO),
                    MeshRenderer::new(
                        MeshRef::Handle(card),
                        MaterialRef::Inline(if is_leaf { green } else { flower }),
                    )
                    .without_shadows(),
                ))
                .id();
            // From the middle out, over 0.6 s.
            blooms.push((e, 0.6 * d / c.radius.max(0.1) + 0.08 * r(8), size));
        }
    }
    commands.spawn((
        Name::new("Healing circle"),
        Transform::from_translation(ground),
        Circle {
            field: c.field,
            center: ground,
            radius: c.radius,
            side: c.side,
            age: 0.0,
            pulse_in: 0.0,
            blooms,
            ring,
            closing: None,
        },
    ));
}

/// Shed pieces as a golem or treant is hurt, fly them back as it heals, and fly a revived one
/// together. Runs after `run_minions` has posed the pieces, and moves them on from there.
#[allow(clippy::type_complexity)]
pub fn wound_minions(
    time: Res<Time>,
    mut minions: Query<
        (
            Entity,
            &mut Minion,
            &Transform,
            Option<&mut Vitality>,
            Option<&mut Wounds>,
            Option<&Revived>,
        ),
        Without<MinionPart>,
    >,
    mut parts: Query<(&MinionPart, &mut Transform, &MeshRenderer), Without<Minion>>,
    lying: Query<&Transform, (With<ShedPiece>, Without<Minion>, Without<MinionPart>)>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut m, tf, v, wounds, revived) in &mut minions {
        if !m.kind.golemlike() || m.cast.spell.payload.raise > 0.0 {
            continue;
        }
        let Some(mut w) = wounds else {
            commands.entity(e).insert(Wounds::default());
            continue;
        };
        let k = m.size;
        if let Some(r) = revived {
            // Standing already: its pieces are flying together, not climbing out of the floor.
            let rise = gait_of(m.kind).rise_secs;
            if let Act::Rise { t } = &mut m.act {
                *t = (*t).max(rise - 1e-3);
            }
            if let Some(mut v) = v {
                v.health = v.max * REVIVE_HEALTH;
                let age = m.age;
                m.life = age + r.left.max(1.0);
                commands.entity(e).remove::<Revived>();
            }
            continue;
        }
        let frac = v
            .as_ref()
            .map_or(1.0, |v| if v.max > 0.0 { v.health / v.max } else { 1.0 });
        let down = v.as_ref().is_some_and(|v| v.down());
        // Shed: one mark a step at most.
        let want = SHED_AT.iter().filter(|&&t| frac < t).count();
        if !down && w.mends.is_empty() && w.assembling <= 0.0 && w.shed.len() < want {
            let tier = w.shed.len();
            let mut lying_now = Vec::new();
            for &index in SHED_TIERS[tier] {
                let Some(&p) = m.parts.get(index) else {
                    continue;
                };
                let Ok((part, ptf, r)) = parts.get(p) else {
                    continue;
                };
                let at = tf.translation + (tf.rotation * ptf.translation.as_vec3()).as_dvec3();
                let rot = tf.rotation * ptf.rotation;
                let rr = |s: u64| roll(m.cast.seed as u64 ^ p.to_bits() ^ tier as u64, s) - 0.5;
                let out = (tf.rotation * ptf.translation.as_vec3()) * Vec3::new(1.0, 0.0, 1.0);
                let out = out.normalize_or(Vec3::X);
                let volume = part.size.x * part.size.y * part.size.z;
                let piece = commands
                    .spawn((
                        Name::new("Shed piece"),
                        Transform::from_translation(at).with_rotation(rot),
                        r.clone(),
                        Collider {
                            shape: ColliderShape::Cuboid {
                                half_extents: (part.size * 0.4).max(Vec3::splat(0.02)),
                            },
                            density: 2600.0,
                            ..Default::default()
                        },
                        RigidBody {
                            kind: BodyKind::Dynamic,
                            mass_kg: Some((volume * 1200.0).clamp(15.0, 400.0)),
                            linear_damping: 0.05,
                            angular_damping: 0.2,
                            ..Default::default()
                        },
                        Velocity {
                            linear: out * (1.4 + rr(1)) + Vec3::Y * (1.2 + rr(2)),
                            angular: Vec3::new(rr(3), rr(4), rr(5)) * 5.0,
                        },
                        Rubble {
                            age: 0.0,
                            life: 600.0,
                        },
                        ShedPiece {
                            owner: e,
                            index,
                            orphan: 0.0,
                        },
                    ))
                    .id();
                lying_now.push((index, piece));
                queue.looks.push(LookCmd::Burst {
                    at,
                    up: Vec3::Y,
                    look: "stone".into(),
                    role: Role::Impact,
                    scale: 0.45 * k,
                    color: if m.kind == MinionKind::Treant {
                        [0.42, 0.3, 0.18]
                    } else {
                        [0.62, 0.58, 0.52]
                    },
                });
            }
            w.shed.push(lying_now);
        }
        // Mend: healed back past a mark, the last piece shed flies back up from where it lies.
        let keep = SHED_AT.iter().filter(|&&t| frac < t + MEND_OVER).count();
        if !down && w.shed.len() > keep {
            let tier = w.shed.pop().unwrap_or_default();
            for (index, piece) in tier {
                let (from, from_rot) = lying
                    .get(piece)
                    .map(|t| (t.translation, t.rotation))
                    .unwrap_or((tf.translation, tf.rotation));
                w.mends.push(Mend {
                    index,
                    from,
                    from_rot,
                    age: 0.0,
                    delay: 0.0,
                    secs: MEND_SECS,
                });
                commands.entity(piece).despawn();
            }
        }
        // Hide what is shed.
        let hidden: Vec<usize> = w.hidden().collect();
        for index in hidden {
            if let Some(&p) = m.parts.get(index)
                && let Ok((_, mut ptf, _)) = parts.get_mut(p)
            {
                ptf.scale = Vec3::ZERO;
            }
        }
        // Fly mends from where they lay to where the body holds them now.
        let inv = tf.rotation.inverse();
        let mut seated = Vec::new();
        for mend in &mut w.mends {
            mend.age += dt;
            let Some(&p) = m.parts.get(mend.index) else {
                continue;
            };
            let Ok((_, mut ptf, _)) = parts.get_mut(p) else {
                continue;
            };
            let to = tf.translation + (tf.rotation * ptf.translation.as_vec3()).as_dvec3();
            let to_rot = tf.rotation * ptf.rotation;
            let s = smooth((mend.age - mend.delay) / mend.secs);
            let lift = (PI * s).sin() * 0.7 * k;
            let at = mend.from.lerp(to, s as f64) + DVec3::Y * lift as f64;
            let rot = mend.from_rot.slerp(to_rot, s);
            ptf.translation = (inv * (at - tf.translation).as_vec3()).as_dvec3();
            ptf.rotation = inv * rot;
            if mend.age >= mend.delay + mend.secs {
                seated.push((mend.index, to));
            }
        }
        for (index, at) in &seated {
            w.mends.retain(|m| m.index != *index);
            if w.assembling <= 0.0 {
                // A ring of light closes round it where it seats.
                glow(
                    &mut commands,
                    *at - DVec3::Y * (0.4 * k) as f64,
                    0.8 * k,
                    0.55 * k,
                    None,
                    e.to_bits() ^ *index as u64,
                    false,
                );
            }
        }
        // A revive's glowing pieces (heart, eyes) stay dark until the body is together.
        if w.assembling > 0.0 {
            w.assembling -= dt;
            let lit = w.assembling <= 0.0;
            for index in [CORE, EYE[0], EYE[1]] {
                if let Some(&p) = m.parts.get(index)
                    && let Ok((_, mut ptf, _)) = parts.get_mut(p)
                    && !lit
                {
                    ptf.scale = Vec3::ZERO;
                }
            }
            if lit {
                let (height, radius) = body_of(Some(&m));
                glow(
                    &mut commands,
                    tf.translation,
                    height,
                    radius,
                    Some(e),
                    e.to_bits(),
                    false,
                );
            }
        }
    }
}

/// Rings run up healed bodies and motes rise; fallen creatures' pieces age; shed pieces whose
/// creature fell join its fallen pieces.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn run_glows(
    time: Res<Time>,
    mut rings: Query<(Entity, &mut HealRing, &mut Transform), Without<Mote>>,
    mut motes: Query<(Entity, &mut Mote, &mut Transform), Without<HealRing>>,
    bodies: Query<&GlobalTransform, Without<HealRing>>,
    mut fallen: Query<(Entity, &mut Fallen)>,
    mut shed: Query<(Entity, &mut ShedPiece, &mut Rubble)>,
    alive: Query<(), With<Minion>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut r, mut tf) in &mut rings {
        r.age += dt;
        if let Some(t) = r.target
            && let Ok(g) = bodies.get(t)
        {
            r.base = g.translation();
        }
        let s = (r.age / RING_SECS).clamp(0.0, 1.0);
        let up = if r.down { 1.0 - smooth(s) } else { smooth(s) };
        // It opens out at the feet, holds round the body, and closes in over the head.
        let open = smooth(s / 0.2);
        let close = 1.0 - smooth((s - 0.7) / 0.3);
        let width = r.radius * open * (0.35 + 0.65 * close);
        tf.translation = r.base + DVec3::Y * (up * r.height) as f64;
        tf.scale =
            Vec3::new(width, width.clamp(0.4, 1.0), width) * if s >= 1.0 { 0.0 } else { 1.0 };
        if r.age >= RING_SECS {
            commands.entity(e).despawn();
        }
    }
    for (e, mut m, mut tf) in &mut motes {
        m.age += dt;
        if m.age < 0.0 {
            continue;
        }
        let s = m.age / m.life;
        let a = m.phase + s * 3.2;
        let r = m.radius * (1.0 - 0.5 * s);
        tf.translation = m.center
            + DVec3::new(
                (a.cos() * r) as f64,
                (smooth(s) * m.rise) as f64,
                (a.sin() * r) as f64,
            );
        let size = m.size * smooth(s / 0.15) * (1.0 - smooth((s - 0.6) / 0.4));
        tf.scale = Vec3::splat(size);
        if s >= 1.0 {
            commands.entity(e).despawn();
        }
    }
    for (e, mut f) in &mut fallen {
        f.age += dt;
        if f.age > REVIVE_SECS {
            commands.entity(e).despawn();
        }
    }
    for (e, mut s, mut rubble) in &mut shed {
        if alive.contains(s.owner) {
            continue;
        }
        s.orphan += dt;
        // Its creature fell: it lies with the rest of its pieces, to rise with them.
        if let Some((_, mut f)) = fallen.iter_mut().find(|(_, f)| f.was == s.owner) {
            if !f.pieces.iter().any(|(_, p)| *p == e) {
                f.pieces.push((s.index, e));
            }
            rubble.life = rubble.age + (REVIVE_SECS - f.age).max(0.5);
            commands.entity(e).remove::<ShedPiece>();
        } else if s.orphan > 0.25 {
            rubble.life = rubble.age + 3.0;
            commands.entity(e).remove::<ShedPiece>();
        }
    }
}

/// Healing circles: the ring opens, blossoms open from the middle out, waves run out from the
/// middle and revive what fell under them; when the field is gone the blossoms close.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn run_circles(
    time: Res<Time>,
    mut circles: Query<(Entity, &mut Circle)>,
    mut waves: Query<(Entity, &mut Wave, &mut Transform), Without<Circle>>,
    mut shapes: Query<&mut Transform, (Without<Circle>, Without<Wave>, Without<Rubble>)>,
    fields: Query<(), With<crate::runtime::Field>>,
    fallen: Query<(Entity, &Fallen)>,
    pieces: Query<&Transform, (With<Rubble>, Without<Circle>, Without<Wave>)>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let mut revive_now = Vec::new();
    let lie = |p: Entity| pieces.get(p).ok().map(|t| (t.translation, t.rotation));
    for (e, mut c) in &mut circles {
        c.age += dt;
        let lasting = c.field.is_some_and(|f| fields.contains(f));
        if c.closing.is_none()
            && (if c.field.is_some() {
                !lasting
            } else {
                c.age > BURST_SECS
            })
        {
            c.closing = Some(0.0);
        }
        let shut = c.closing.map_or(1.0, |t| 1.0 - smooth(t / 0.6));
        if let Some(t) = &mut c.closing {
            *t += dt;
        }
        if let Ok(mut tf) = shapes.get_mut(c.ring) {
            let r = c.radius * smooth(c.age / 0.35);
            tf.scale = Vec3::new(r, 1.0, r) * shut;
        }
        let age = c.age;
        for &(b, at, size) in &c.blooms {
            if let Ok(mut tf) = shapes.get_mut(b) {
                let t = ((age - at) / 0.35).clamp(0.0, 1.0);
                // Opens with a little overshoot, as a flower springs open.
                let open = smooth(t) * (1.0 + 0.15 * (PI * t).sin());
                tf.scale = Vec3::splat(size * open * shut);
            }
        }
        if c.closing.is_none() {
            c.pulse_in -= dt;
            if c.pulse_in <= 0.0 {
                c.pulse_in = PULSE_EVERY;
                commands.spawn((
                    Name::new("Wave of light"),
                    Transform::from_translation(c.center).with_scale(Vec3::ZERO),
                    ring_mesh(0.012),
                    Wave {
                        center: c.center,
                        radius: c.radius,
                        age: 0.0,
                    },
                ));
                for (fe, f) in &fallen {
                    if f.new.cast.side == c.side
                        && !revive_now.iter().any(|(x, _)| *x == fe)
                        && flat(f.new.at - c.center) <= c.radius as f64
                    {
                        revive_now.push((fe, f.clone()));
                    }
                }
            }
        }
        if c.closing.is_some_and(|t| t > 0.7) {
            for &(b, _, _) in &c.blooms {
                commands.entity(b).despawn();
            }
            commands.entity(c.ring).despawn();
            commands.entity(e).despawn();
        }
    }
    for (e, mut w, mut tf) in &mut waves {
        w.age += dt;
        let s = (w.age / WAVE_SECS).clamp(0.0, 1.0);
        let r = w.radius * (0.1 + 0.9 * smooth(s));
        let fade = 1.0 - smooth((s - 0.6) / 0.4);
        tf.translation = w.center + DVec3::Y * 0.03;
        tf.scale = Vec3::new(r, fade.max(0.01), r) * if s >= 1.0 { 0.0 } else { 1.0 };
        if s >= 1.0 {
            commands.entity(e).despawn();
        }
    }
    let mut looks = Vec::new();
    for (fe, f) in revive_now {
        revive(&mut commands, fe, &f, &lie, &mut looks);
    }
    queue.looks.extend(looks);
}

/// Life keeps world positions of its own (where pieces lay, where a creature fell, circles,
/// glows); move them when the world re-anchors.
#[allow(clippy::type_complexity)]
pub fn follow_world_shift(
    mut shifts: MessageReader<engine_world::WorldShift>,
    mut q: ParamSet<(
        Query<&mut Wounds>,
        Query<&mut Fallen>,
        Query<&mut Circle>,
        Query<&mut Wave>,
        Query<&mut HealRing>,
        Query<&mut Mote>,
    )>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    for mut w in &mut q.p0() {
        for m in w.mends.iter_mut() {
            m.from = s.point(m.from);
        }
    }
    for mut f in &mut q.p1() {
        f.new.at = s.point(f.new.at);
    }
    for mut c in &mut q.p2() {
        c.center = s.point(c.center);
    }
    for mut w in &mut q.p3() {
        w.center = s.point(w.center);
    }
    for mut r in &mut q.p4() {
        r.base = s.point(r.base);
    }
    for mut m in &mut q.p5() {
        m.center = s.point(m.center);
    }
}

/// Everything of life's goes (the hall's restart).
pub fn clear(world: &mut World) {
    let mut gone = Vec::new();
    let mut q = world.query::<(Entity, &Circle)>();
    for (e, c) in q.iter(world) {
        gone.push(e);
        gone.push(c.ring);
        gone.extend(c.blooms.iter().map(|b| b.0));
    }
    let mut q = world.query_filtered::<Entity, Or<(
        With<Wave>,
        With<HealRing>,
        With<Mote>,
        With<Fallen>,
        With<ShedPiece>,
    )>>();
    gone.extend(q.iter(world));
    for e in gone {
        if let Ok(ec) = world.get_entity_mut(e) {
            ec.despawn();
        }
    }
}

/// `magic.life`: what healing is doing now.
pub fn register(app: &mut App) {
    app.init_resource::<LifeMeshes>();
    app.register_api(
        "magic.life",
        "Life now: creatures' shed pieces and pieces flying back, fallen creatures that can be \
         revived (seconds left), healing circles and glows.",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Minion, &Wounds)>();
            let wounds: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, m, w)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "kind": format!("{:?}", m.kind),
                        "shed": w.shed.len(),
                        "mending": w.mends.len(),
                        "assembling": w.assembling > 0.0,
                    })
                })
                .collect();
            let mut q = world.query::<&Fallen>();
            let fallen: Vec<serde_json::Value> = q
                .iter(world)
                .map(|f| {
                    serde_json::json!({
                        "kind": format!("{:?}", f.new.kind),
                        "pieces": f.pieces.len(),
                        "left_s": REVIVE_SECS - f.age,
                    })
                })
                .collect();
            let circles = world.query::<&Circle>().iter(world).count();
            let glows = world.query::<&HealRing>().iter(world).count();
            Ok(serde_json::json!({
                "wounds": wounds,
                "fallen": fallen,
                "circles": circles,
                "glows": glows,
            }))
        },
    );
}
