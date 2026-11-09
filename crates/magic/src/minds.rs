//! Minds (round-13-plan.md): the Mesmer's spells you can watch work on a fight.
//!
//! * **Phantasm** (Illusion): copies of the caster. Cast on yourself it is Mirror image, copies
//!   that step out of you to your sides and keep there; summoned it is Decoy, copies standing
//!   where you aim that walk at your foe. Each wears the caster's body, face and robe (the
//!   character module draws the same [`Appearance`]), faces the nearest foe and throws glamour
//!   bolts at it as the caster would, so skeletons, golems and thralls take it for a creature
//!   and go for it first (Guild Wars 2's clones). It has one point of health: any blow breaks
//!   it into violet glass. A copy of a spell with an element bursts with it as it breaks
//!   (Guild Wars 2's shatter).
//! * **Fear** (Domination, `fear`): a terrified creature drops its foe and runs from where the
//!   terror came from, swerving (WoW's fear), until it wears off; a hard blow shakes it off.
//!   The running is in `summon::run_minions` and `undead::run_thralls`; [`flee_dir`] steers.
//! * **Command** (Domination): a creature hit fights for the caster: its side and faction are
//!   the caster's while it holds, its glowing parts turn magenta, and a thread of light runs
//!   from the caster's hand to its head. Its old side attacks it. When the hold lets go it
//!   flashes and goes back.
//! * **Wrath** (Domination): the same hold with no master: a side of its own, so it strikes at
//!   whatever is near and everyone strikes back (Skyrim's Frenzy).
//!
//! `magic.minds` lists copies, held creatures and the terrified.

use std::f32::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::animation::BodyRequest;
use engine_world::character::Appearance;
use engine_world::physics::{
    BodyKind, Collider, ColliderShape, PhysicsQueries, QueryFilter, RigidBody,
};
use engine_world::{Faction, Factions};

use crate::caster::{Aim, CastRequest, MagicCaster, SpellChoice, Vitality};
use crate::design::{CastType, SpellDesign};
use crate::looks::{LookCmd, Role};
use crate::runtime::{
    Allegiance, CastInfo, HitQueue, MagicQueue, PendingHit, ground_below, hand_of, roll,
};
use crate::status::Afflictions;
use crate::summon::{Minion, MinionKind, MinionPart};
use crate::undead::{Thrall, ThrallEye};

/// A copy's violet: the glass it breaks into and the shimmer round it, linear RGB.
pub const MIND_COLOR: [f32; 3] = [0.78, 0.5, 1.0];
/// A commanded creature's glow and the thread that holds it.
pub const COMMAND_COLOR: [f32; 3] = [1.0, 0.28, 0.78];
/// A raging creature's glow.
pub const WRATH_COLOR: [f32; 3] = [1.0, 0.2, 0.12];
/// How far round its caster a mirror image keeps, m.
pub const MIRROR_M: f32 = 2.2;
/// A decoy walks at its foe until it is this near, then stands and casts, m.
pub const DECOY_KEEP_M: f32 = 6.5;
/// A copy throws a glamour bolt this often, s (with a little of its own jitter).
pub const GLAMOUR_EVERY: f32 = 1.6;
/// Copies look for foes this far, m.
pub const SEE_M: f32 = 24.0;
/// A copy walks at this pace, m/s (it strides out of its caster faster).
pub const PACE: f32 = 2.4;
/// The most copies one caster keeps (Guild Wars 2 keeps three; the design's Ascended four,
/// and a Decoy beside them).
pub const MOST_COPIES: usize = 6;
/// How much faster the terrified run than they walk.
pub const FLEE_PACE: f32 = 1.6;
/// Shards a broken copy bursts into, and their violet (deeper than the copies' shimmer, so it
/// stays violet as it glows).
pub const GLASS_SHARDS: u32 = 32;
pub const GLASS_COLOR: [f32; 3] = [0.62, 0.3, 1.0];
/// Pieces of a command's thread.
pub const THREAD_PIECES: usize = 18;
/// Sides from here up belong to one raging creature each.
const WRATH_SIDE: u32 = 0x4000_0000;

/// Copies to make, from `runtime::release`.
#[derive(Debug, Clone)]
pub struct NewPhantasm {
    pub cast: CastInfo,
    /// The caster's feet (Mirror image) or where it aimed (Decoy).
    pub at: DVec3,
    pub facing: Vec3,
    pub around_caster: bool,
    pub target: Option<Entity>,
}

/// A creature taken by Command or Wrath, from the hits.
#[derive(Debug, Clone)]
pub struct MindCmd {
    pub target: Entity,
    pub by: CastInfo,
    pub command_secs: f32,
    pub wrath_secs: f32,
}

/// A copy of a caster.
#[derive(Component, Debug, Clone)]
pub struct Phantasm {
    pub owner: Option<Entity>,
    pub cast: CastInfo,
    pub age: f32,
    pub life: f32,
    /// Keeps round its caster (Mirror image) rather than walking at its foe (Decoy).
    pub around: bool,
    /// Its place round its caster: the turn from the caster's facing, rad, and how far, m.
    pub slot: (f32, f32),
    pub target: Option<Entity>,
    pub look_in: f32,
    pub cooldown: f32,
    /// Its own jitter, 0..1.
    pub jitter: f32,
}

impl Phantasm {
    /// Seconds before it fades.
    pub fn left(&self) -> f32 {
        (self.life - self.age).max(0.0)
    }
}

/// What a held creature was before the hold: its cast's caster, side and faction, the side
/// and faction it shows, and its glowing pieces' materials.
#[derive(Debug, Clone)]
struct Own {
    caster: Option<Entity>,
    side: u32,
    faction: Option<Faction>,
    shown_side: u32,
    shown_faction: Option<Faction>,
    glow: Vec<(Entity, MaterialRef)>,
}

/// A creature held by Command (fights for `by`) or Wrath (fights everyone).
#[derive(Component, Debug, Clone)]
pub struct Commanded {
    /// Its master (none for Wrath).
    pub by: Option<Entity>,
    pub side: u32,
    pub faction: Option<Faction>,
    pub secs: f32,
    pub age: f32,
    pub wrath: bool,
    /// Height of its head over its feet, m (where the thread ties on).
    pub head: f32,
    /// The thread's pieces (Command).
    pub thread: Vec<Entity>,
    /// A command that takes over once a raging creature's wrath lets go: its master, side,
    /// faction and seconds.
    next: Option<(Option<Entity>, u32, Option<Faction>, f32)>,
    own: Option<Own>,
}

impl Commanded {
    /// Seconds before the hold lets go.
    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// A piece of a command's thread.
#[derive(Component, Debug, Clone, Copy)]
pub struct ThreadPiece;

/// Where a terrified body at `at` runs: away from `from`, swerving one way and the other
/// as it goes (WoW's fear runs in changing directions), never straight back at it.
pub fn flee_dir(at: DVec3, from: DVec3, age: f32, seed: u64) -> Vec3 {
    let d = at - from;
    let away = Vec3::new(d.x as f32, 0.0, d.z as f32).normalize_or(Vec3::new(
        roll(seed, 3) - 0.5,
        0.0,
        roll(seed, 4) - 0.5,
    ));
    let away = away.normalize_or(Vec3::Z);
    let phase = roll(seed, 5) * 2.0 * PI;
    let swerve = 0.75 * (age * 1.3 + phase).sin() + 0.25 * (age * 3.1 + 2.0 * phase).sin();
    Quat::from_rotation_y(swerve) * away
}

/// The place round its caster of copy `i` of `n`: to the sides first, then further round
/// (left, right, back left, back right), each a little further out.
pub fn mirror_slot(i: u32, n: u32) -> (f32, f32) {
    let side = if i.is_multiple_of(2) { 1.0 } else { -1.0 };
    let rank = (i / 2) as f32;
    let turn = if n == 1 {
        PI * 0.5
    } else {
        (0.42 + 0.3 * rank).min(0.95) * PI
    };
    (side * turn, MIRROR_M + 0.5 * rank)
}

/// Where a decoy `i` of `n` stands: in a line across the caster's aim, 1.7 m apart.
pub fn decoy_spot(at: DVec3, facing: Vec3, i: u32, n: u32) -> DVec3 {
    let across = Vec3::new(-facing.z, 0.0, facing.x).normalize_or(Vec3::X);
    let k = i as f32 - (n as f32 - 1.0) * 0.5;
    at + (across * 1.7 * k).as_dvec3()
}

/// The spell a copy throws: a glamour bolt, violet and harmless.
pub fn glamour() -> SpellDesign {
    SpellDesign::new(&["phantasm"], CastType::Bolt).named("Glamour")
}

/// A copy's body for spells and blows to find: a man's capsule.
fn copy_collider() -> (Collider, RigidBody) {
    (
        Collider {
            shape: ColliderShape::Compound(vec![(
                Vec3::new(0.0, 0.95, 0.0),
                Quat::IDENTITY,
                ColliderShape::Capsule {
                    radius: 0.28,
                    half_height: 0.6,
                },
            )]),
            density: 300.0,
            ..Default::default()
        },
        RigidBody {
            kind: BodyKind::Kinematic,
            ..Default::default()
        },
    )
}

/// Make the copies asked for: each a body like its caster's, with the caster's side.
#[allow(clippy::type_complexity)]
pub fn spawn_phantasms(
    mut queue: ResMut<MagicQueue>,
    queries: Res<PhysicsQueries>,
    owners: Query<(
        &GlobalTransform,
        Option<&Appearance>,
        Option<&BodyRequest>,
        Option<&MagicCaster>,
        Option<&Faction>,
    )>,
    copies: Query<(Entity, &Phantasm)>,
    mut commands: Commands,
) {
    if queue.phantasms.is_empty() {
        return;
    }
    let asked = std::mem::take(&mut queue.phantasms);
    for np in asked {
        let spell = &np.cast.spell;
        let n = spell.payload.phantasms.max(1);
        let owner = np.cast.caster.and_then(|o| owners.get(o).ok());
        // Past the most a caster keeps, its oldest copies break.
        if let Some(o) = np.cast.caster {
            let mut mine: Vec<(Entity, f32)> = copies
                .iter()
                .filter(|(_, p)| p.owner == Some(o))
                .map(|(e, p)| (e, p.age))
                .collect();
            mine.sort_by(|a, b| b.1.total_cmp(&a.1));
            let over = (mine.len() + n as usize).saturating_sub(MOST_COPIES);
            for (e, _) in mine.into_iter().take(over) {
                commands.entity(e).insert(Vitality {
                    health: 0.0,
                    ..Vitality::new(1.0)
                });
            }
        }
        let (from, rot) = owner
            .map(|(g, ..)| (g.translation(), g.rotation()))
            .unwrap_or((
                np.at,
                Quat::from_rotation_y((-np.facing.x).atan2(-np.facing.z)),
            ));
        for i in 0..n {
            let jitter = roll(np.cast.seed as u64 ^ 0x6d69_6e64, i as u64);
            let slot = mirror_slot(i, n);
            let (start, goal) = if np.around_caster {
                let place =
                    from + (rot * Quat::from_rotation_y(slot.0) * Vec3::NEG_Z * slot.1).as_dvec3();
                // They stride out of the caster: start a step from its middle.
                (from + (place - from) * 0.15, place)
            } else {
                let spot = decoy_spot(np.at, np.facing, i, n);
                (spot, spot)
            };
            let start = ground_below(&queries, start + DVec3::Y * 0.5, &QueryFilter::default());
            let yaw = (-np.facing.x).atan2(-np.facing.z);
            let mut caster = owner
                .and_then(|o| o.3.cloned())
                .unwrap_or_else(|| MagicCaster::new(40.0, spell.mastery));
            caster.learns = false;
            caster.grimoire.clear();
            caster.selected = 0;
            caster.side = np.cast.side;
            caster.seed = caster.seed.wrapping_mul(31).wrapping_add(7 + i as u64);
            caster.max_mana = 1.0e6;
            caster.mana = 1.0e6;
            caster.mana_regen = 1.0e5;
            let copy = commands
                .spawn((
                    Name::new(if np.around_caster {
                        "Mirror image"
                    } else {
                        "Decoy"
                    }),
                    Transform::from_translation(start).with_rotation(Quat::from_rotation_y(yaw)),
                    caster,
                    Vitality::new(1.0),
                    copy_collider(),
                    Phantasm {
                        owner: np.cast.caster,
                        cast: np.cast.clone(),
                        age: 0.0,
                        life: spell.payload.phantasm_secs * (0.92 + 0.16 * jitter),
                        around: np.around_caster,
                        slot,
                        target: np.target,
                        look_in: 0.0,
                        cooldown: 0.9 + 0.8 * jitter,
                        jitter,
                    },
                ))
                .id();
            if let Some((_, look, body, _, faction)) = owner {
                if let Some(a) = look {
                    commands.entity(copy).insert(a.clone());
                }
                if let Some(b) = body {
                    commands.entity(copy).insert(b.clone());
                }
                if let Some(f) = faction {
                    commands.entity(copy).insert(*f);
                }
            } else {
                commands
                    .entity(copy)
                    .insert(BodyRequest::human(np.cast.seed as u64));
            }
            // A ripple where it stands up, and a shimmer that stays on it.
            queue.looks.push(LookCmd::Burst {
                at: if np.around_caster { from } else { start } + DVec3::Y * 1.0,
                up: Vec3::Y,
                look: "force".into(),
                role: Role::Impact,
                scale: 0.7,
                color: MIND_COLOR,
            });
            queue.looks.push(LookCmd::Attach {
                entity: copy,
                look: "force".into(),
                role: Role::Aura,
                scale: 0.45,
                color: MIND_COLOR,
                power_j: 0.0,
            });
            let _ = goal;
        }
    }
}

/// Copies keep their place (Mirror image) or walk at their foe (Decoy), face it and throw
/// glamour bolts; a blow breaks one into glass, and its time up it fades.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn run_phantasms(
    time: Res<Time>,
    queries: Res<PhysicsQueries>,
    factions: Option<Res<Factions>>,
    mut copies: Query<(Entity, &mut Phantasm, &mut Transform, &Vitality)>,
    bodies: Query<(Entity, &GlobalTransform, &Vitality, Allegiance)>,
    anchors: Query<&GlobalTransform>,
    mut casts: MessageWriter<CastRequest>,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
    (mut wm, mut meshes): (
        ResMut<crate::wards::WardMeshes>,
        Option<ResMut<engine_assets::Assets<engine_render::Mesh>>>,
    ),
) {
    if copies.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    let all: Vec<Entity> = copies.iter().map(|(e, ..)| e).collect();
    for (e, mut p, mut tf, v) in &mut copies {
        p.age += dt;
        p.cooldown -= dt;
        let at = tf.translation;
        let owner = p
            .owner
            .and_then(|o| bodies.get(o).ok())
            .map(|b| (b.1, b.2.down()));
        let owner_down = p.owner.is_some() && owner.is_none_or(|o| o.1);
        let broke = v.down();
        if broke || p.age >= p.life || owner_down {
            // Glass: a broken copy bursts into glowing violet shards.
            if broke && let Some(meshes) = meshes.as_deref_mut() {
                crate::wards::glass_burst(
                    &mut commands,
                    &mut wm,
                    meshes,
                    at,
                    1.75,
                    GLASS_COLOR,
                    GLASS_SHARDS,
                    e.to_bits() ^ p.cast.seed as u64,
                );
            }
            vanish(
                e,
                &p,
                at,
                broke,
                &bodies,
                factions.as_deref(),
                &mut hits,
                &mut queue,
            );
            commands.entity(e).despawn();
            continue;
        }
        // Look for the nearest foe now and then.
        p.look_in -= dt;
        if p.look_in <= 0.0 || p.target.is_some_and(|t| bodies.get(t).is_err()) {
            p.look_in = 0.5;
            p.target = bodies
                .iter()
                .filter(|(b, bg, bv, al)| {
                    *b != e
                        && !all.contains(b)
                        && Some(*b) != p.owner
                        && !bv.down()
                        && bg.translation().distance(at) <= SEE_M as f64
                        && !p
                            .cast
                            .friend(*b, al.caster, al.faction, factions.as_deref())
                })
                .min_by(|a, b| {
                    a.1.translation()
                        .distance(at)
                        .total_cmp(&b.1.translation().distance(at))
                        .then(a.0.cmp(&b.0))
                })
                .map(|(b, ..)| b);
        }
        let foe = p
            .target
            .and_then(|t| anchors.get(t).ok())
            .map(|g| g.translation());
        // Where to stand.
        let goal = if p.around {
            owner.map(|(og, _)| {
                let (yaw, ..) = og.rotation().to_euler(EulerRot::YXZ);
                og.translation()
                    + (Quat::from_rotation_y(yaw + p.slot.0) * Vec3::NEG_Z * p.slot.1).as_dvec3()
            })
        } else {
            foe.and_then(|f| {
                let d = f - at;
                let flat = DVec3::new(d.x, 0.0, d.z);
                (flat.length() > DECOY_KEEP_M as f64)
                    .then(|| at + flat.normalize() * (flat.length() - DECOY_KEEP_M as f64))
            })
        };
        let mut moving = false;
        if let Some(goal) = goal {
            let to = DVec3::new(goal.x - at.x, 0.0, goal.z - at.z);
            let left = to.length();
            if left > 0.15 {
                moving = true;
                // Out of the caster at a stride, then at a walk.
                let pace = if p.age < 1.0 { PACE * 1.6 } else { PACE };
                let step = (pace * dt) as f64;
                let mut next = if left <= step {
                    DVec3::new(goal.x, at.y, goal.z)
                } else {
                    at + to / left * step
                };
                let floor =
                    ground_below(&queries, next + DVec3::Y * 0.6, &QueryFilter::excluding(e)).y;
                next.y = if (floor - at.y).abs() > 0.8 {
                    floor
                } else {
                    at.y + (floor - at.y).clamp(-3.0 * dt as f64, 3.0 * dt as f64)
                };
                tf.translation = next;
            }
        }
        // Face the foe (or the way it walks, or its caster's way).
        let face = foe
            .map(|f| (f - tf.translation).as_vec3())
            .or_else(|| goal.map(|g| (g - at).as_vec3()).filter(|_| moving))
            .or_else(|| owner.map(|o| o.0.rotation() * Vec3::NEG_Z));
        let mut facing_err = PI;
        if let Some(d) = face {
            let d = Vec3::new(d.x, 0.0, d.z);
            if d.length_squared() > 1e-4 {
                let want = Quat::from_rotation_y((-d.x).atan2(-d.z));
                let angle = tf.rotation.angle_between(want);
                let k = (5.0 * dt / angle.max(1e-4)).min(1.0);
                tf.rotation = tf.rotation.slerp(want, k);
                facing_err = tf.rotation.angle_between(want);
            }
        }
        // Glamour bolts at the foe, as its caster would throw.
        if let Some(t) = p.target
            && p.cooldown <= 0.0
            && facing_err < 0.4
            && (!moving || p.around)
        {
            p.cooldown =
                GLAMOUR_EVERY * (0.85 + 0.3 * roll(p.jitter.to_bits() as u64, p.age as u64));
            casts.write(CastRequest {
                caster: e,
                spell: SpellChoice::Design(glamour()),
                aim: Aim::Entity(t),
                charge: 0.0,
                gathered: false,
                off_hand: false,
            });
        }
    }
}

/// A copy goes: broken, it bursts into violet glass (and its element with it, on whoever
/// stands near); its time up, it fades in a shimmer.
#[allow(clippy::too_many_arguments)]
fn vanish(
    e: Entity,
    p: &Phantasm,
    at: DVec3,
    broke: bool,
    bodies: &Query<(Entity, &GlobalTransform, &Vitality, Allegiance)>,
    factions: Option<&Factions>,
    hits: &mut HitQueue,
    queue: &mut MagicQueue,
) {
    let chest = at + DVec3::Y * 1.1;
    queue.looks.push(LookCmd::Stop { entity: e });
    if !broke {
        queue.looks.push(LookCmd::Burst {
            at: chest,
            up: Vec3::Y,
            look: "force".into(),
            role: Role::Impact,
            scale: 0.5,
            color: MIND_COLOR,
        });
        return;
    }
    // The glass itself is magic's own (`wards::glass_burst`, from `run_phantasms`); a ripple of
    // light goes with it.
    queue.looks.push(LookCmd::Burst {
        at: chest,
        up: Vec3::Y,
        look: "force".into(),
        role: Role::Impact,
        scale: 0.6,
        color: MIND_COLOR,
    });
    // Shatter: a copy woven with an element bursts with it on its foes within 2.5 m.
    let spell = &p.cast.spell;
    if spell.weave.elements.is_empty() || spell.payload.damage <= 0.0 {
        return;
    }
    queue.looks.push(LookCmd::Burst {
        at: chest,
        up: Vec3::Y,
        look: spell.weave.key.clone(),
        role: Role::Impact,
        scale: 1.1,
        color: spell.weave.color,
    });
    for (b, bg, bv, al) in bodies.iter() {
        let bp = bg.translation();
        if b == e
            || bv.down()
            || bp.distance(at) > 2.5
            || p.cast.friend(b, al.caster, al.faction, factions)
        {
            continue;
        }
        let dir = (bp - at).as_vec3();
        hits.0.push(PendingHit {
            cast: CastInfo {
                scale: p.cast.scale * 0.6,
                depth: 1,
                ..p.cast.clone()
            },
            target: Some(b),
            point: bp + DVec3::Y * 1.0,
            normal: Vec3::Y,
            dir: Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::X),
            multi: false,
            impact: false,
            reacts: true,
            expire: false,
        });
    }
}

/// Command and Wrath take hold of the creatures the hits name.
pub fn take_minds(
    mut queue: ResMut<MagicQueue>,
    creatures: Query<(Option<&Minion>, Option<&Thrall>, &MagicCaster)>,
    mut held: Query<&mut Commanded>,
    mut commands: Commands,
) {
    if queue.minds.is_empty() {
        return;
    }
    let asked = std::mem::take(&mut queue.minds);
    for m in asked {
        let Ok((minion, thrall, c)) = creatures.get(m.target) else {
            continue;
        };
        if minion.is_none() && thrall.is_none() {
            continue;
        }
        let wrath = m.wrath_secs > 0.0 && m.command_secs <= 0.0;
        let secs = if wrath { m.wrath_secs } else { m.command_secs };
        if secs <= 0.0 {
            continue;
        }
        // A hold already on it lasts the longer; a command takes over from a wrath.
        if let Ok(mut h) = held.get_mut(m.target) {
            if h.wrath == wrath && (wrath || h.by == m.by.caster) {
                h.secs = h.secs.max(h.age + secs);
                continue;
            }
            if h.wrath && !wrath {
                h.secs = h.age;
                h.next = Some((m.by.caster, m.by.side, m.by.faction, secs));
            }
            continue;
        }
        // Its own side's commands mean nothing to it.
        if !wrath && c.side == m.by.side && m.by.faction.is_none() {
            continue;
        }
        let head = match minion.map(|m| (m.kind, m.size)) {
            Some((MinionKind::Golem, k)) => 2.35 * k,
            Some((_, k)) => 1.55 * k,
            None => 1.6,
        };
        commands.entity(m.target).insert(Commanded {
            by: if wrath { None } else { m.by.caster },
            side: if wrath {
                WRATH_SIDE + (m.target.to_bits() as u32 & 0x3fff_ffff)
            } else {
                m.by.side
            },
            faction: if wrath { None } else { m.by.faction },
            secs,
            age: 0.0,
            wrath,
            head,
            thread: Vec::new(),
            next: None,
            own: None,
        });
    }
}

/// Holds take their creatures over, run their time and let them go.
#[allow(clippy::type_complexity)]
pub fn tend_minds(
    time: Res<Time>,
    mut held: Query<(
        Entity,
        &mut Commanded,
        Option<&mut Minion>,
        Option<&mut Thrall>,
        &mut MagicCaster,
        Option<&Faction>,
        &GlobalTransform,
        Option<&Vitality>,
    )>,
    masters: Query<&Vitality, Without<Commanded>>,
    mut glows: Query<(&MinionPart, &mut MeshRenderer), Without<ThreadPiece>>,
    mut eyes: Query<
        (Entity, &ThrallEye, &mut MeshRenderer),
        (Without<MinionPart>, Without<ThreadPiece>),
    >,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut h, mut minion, mut thrall, mut caster, faction, g, v) in &mut held {
        let color = if h.wrath { WRATH_COLOR } else { COMMAND_COLOR };
        let glow_mat = || {
            MaterialRef::Inline(
                StandardMaterial::default()
                    .with_tint(color[0], color[1], color[2])
                    .with_emissive(color, 16_000.0),
            )
        };
        if h.own.is_none() {
            // Take it over: its cast and the side it shows become its master's.
            let cast = match (&minion, &thrall) {
                (Some(m), _) => &m.cast,
                (None, Some(t)) => &t.cast,
                _ => {
                    commands.entity(e).remove::<Commanded>();
                    continue;
                }
            };
            let mut own = Own {
                caster: cast.caster,
                side: cast.side,
                faction: cast.faction,
                shown_side: caster.side,
                shown_faction: faction.copied(),
                glow: Vec::new(),
            };
            let pieces: Vec<Entity> = minion.as_ref().map(|m| m.parts.clone()).unwrap_or_default();
            for p in pieces {
                if let Ok((part, mut r)) = glows.get_mut(p)
                    && part.size == Vec3::ZERO
                {
                    own.glow.push((p, r.material.clone()));
                    r.material = glow_mat();
                }
            }
            for (eye_e, eye, mut r) in &mut eyes {
                if eye.owner == e {
                    own.glow.push((eye_e, r.material.clone()));
                    r.material = glow_mat();
                }
            }
            let set = |c: &mut CastInfo| {
                c.caster = h.by;
                c.side = h.side;
                c.faction = h.faction;
            };
            if let Some(m) = minion.as_deref_mut() {
                set(&mut m.cast);
                m.target = None;
                m.look_in = 0.0;
            }
            if let Some(t) = thrall.as_deref_mut() {
                set(&mut t.cast);
                t.target = None;
            }
            caster.side = h.side;
            match h.faction {
                Some(f) => {
                    commands.entity(e).insert(f);
                }
                None if faction.is_some() => {
                    commands.entity(e).remove::<Faction>();
                }
                None => {}
            }
            h.own = Some(own);
            // A flash where it is taken, the hold's colour round it, and the thread.
            let at = g.translation() + DVec3::Y * h.head as f64;
            queue.looks.push(LookCmd::Burst {
                at,
                up: Vec3::Y,
                look: "force".into(),
                role: Role::Impact,
                scale: 0.9,
                color,
            });
            queue.looks.push(LookCmd::Attach {
                entity: e,
                look: "force".into(),
                role: Role::Aura,
                scale: if h.head > 2.0 { 0.9 } else { 0.55 },
                color,
                power_j: 0.0,
            });
            if !h.wrath && h.by.is_some() {
                let piece = MeshRenderer::new(
                    MeshRef::Shape(Shape::Cylinder {
                        radius: 1.0,
                        height: 1.0,
                        segments: 8,
                    }),
                    MaterialRef::Inline(
                        StandardMaterial::default()
                            .with_tint(color[0], color[1], color[2])
                            .with_emissive(color, 14_000.0),
                    ),
                );
                h.thread = (0..THREAD_PIECES)
                    .map(|_| {
                        commands
                            .spawn((
                                Name::new("Command thread"),
                                Transform::from_translation(at).with_scale(Vec3::ZERO),
                                piece.clone(),
                                ThreadPiece,
                            ))
                            .id()
                    })
                    .collect();
            }
            continue;
        }
        h.age += dt;
        let master_down =
            h.by.is_some_and(|b| masters.get(b).map_or(true, |v| v.down()));
        let down = v.is_some_and(|v| v.down());
        if h.age < h.secs && !master_down && !down {
            continue;
        }
        // Let go: back to its own side, its own glow, a flash.
        let own = h.own.take().expect("taken over above");
        let reset = |c: &mut CastInfo| {
            c.caster = own.caster;
            c.side = own.side;
            c.faction = own.faction;
        };
        if let Some(m) = minion.as_deref_mut() {
            reset(&mut m.cast);
            m.target = None;
            m.look_in = 0.0;
        }
        if let Some(t) = thrall.as_deref_mut() {
            reset(&mut t.cast);
            t.target = None;
        }
        caster.side = own.shown_side;
        match own.shown_faction {
            Some(f) => {
                commands.entity(e).insert(f);
            }
            None => {
                commands.entity(e).remove::<Faction>();
            }
        }
        for (p, m) in own.glow {
            if let Ok((_, mut r)) = glows.get_mut(p) {
                r.material = m;
            } else if let Ok((_, _, mut r)) = eyes.get_mut(p) {
                r.material = m;
            }
        }
        for p in h.thread.drain(..) {
            commands.entity(p).despawn();
        }
        queue.looks.push(LookCmd::Stop { entity: e });
        if !down {
            queue.looks.push(LookCmd::Burst {
                at: g.translation() + DVec3::Y * h.head as f64,
                up: Vec3::Y,
                look: "force".into(),
                role: Role::Impact,
                scale: 0.7,
                color,
            });
        }
        // A command waiting on the wrath takes it over at once.
        if let Some((by, side, faction, secs)) = h.next.take()
            && !down
            && !master_down
        {
            h.by = by;
            h.side = side;
            h.faction = faction;
            h.secs = secs;
            h.age = 0.0;
            h.wrath = false;
            continue;
        }
        commands.entity(e).remove::<Commanded>();
    }
}

/// A command's thread: from its master's hand to the held creature's head, sagging a little,
/// with a pulse running down it from the hand.
pub fn draw_threads(
    time: Res<Time>,
    held: Query<(&Commanded, &GlobalTransform)>,
    masters: Query<(&GlobalTransform, &MagicCaster), Without<Commanded>>,
    mut pieces: Query<&mut Transform, With<ThreadPiece>>,
) {
    let t = time.elapsed_secs();
    for (h, g) in &held {
        if h.thread.is_empty() {
            continue;
        }
        let Some((mg, mc)) = h.by.and_then(|b| masters.get(b).ok()) else {
            for &p in &h.thread {
                if let Ok(mut tf) = pieces.get_mut(p) {
                    tf.scale = Vec3::ZERO;
                }
            }
            continue;
        };
        let from = hand_of(mg, mc);
        let to = g.translation() + DVec3::Y * h.head as f64;
        let len = from.distance(to) as f32;
        let n = h.thread.len();
        // It draws out from the hand over the first half second.
        let reach = (h.age / 0.5).clamp(0.05, 1.0);
        let sag = (0.05 * len).min(0.8);
        let point = |s: f32| -> DVec3 {
            let s = s * reach;
            let wave = 0.05 * (s * 9.0 - t * 7.0).sin();
            from.lerp(to, s as f64) + DVec3::Y * (-(sag * 4.0 * s * (1.0 - s)) + wave) as f64
        };
        for (i, &p) in h.thread.iter().enumerate() {
            let Ok(mut tf) = pieces.get_mut(p) else {
                continue;
            };
            let a = point(i as f32 / n as f32);
            let b = point((i + 1) as f32 / n as f32);
            let d = (b - a).as_vec3();
            let l = d.length();
            if l < 1e-4 {
                tf.scale = Vec3::ZERO;
                continue;
            }
            // A pulse runs from the hand to the head about twice a second.
            let s = i as f32 / n as f32;
            let pulse = ((s - (t * 1.8).fract()) * 6.0).cos().max(0.0).powi(8);
            let r = 0.018 + 0.02 * pulse;
            tf.translation = a.lerp(b, 0.5);
            tf.rotation = Quat::from_rotation_arc(Vec3::Y, d / l);
            tf.scale = Vec3::new(r, l * 1.04, r);
        }
    }
}

/// Copies and threads go with the hall's restart.
pub fn clear(world: &mut World) {
    let mut q = world.query_filtered::<Entity, Or<(With<Phantasm>, With<ThreadPiece>)>>();
    let gone: Vec<Entity> = q.iter(world).collect();
    for e in gone {
        world.despawn(e);
    }
}

/// `magic.minds`.
pub fn register(app: &mut App) {
    app.register_api(
        "magic.minds",
        "Minds now: copies of casters (whose, where, seconds left, whom they face; Mirror image \
         or Decoy), creatures held by Command or Wrath (who holds them, seconds left) and the \
         terrified (seconds left, where they run from).",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Phantasm, &Transform)>();
            let copies: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, p, t)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "owner": p.owner.map(|o| o.to_bits()),
                        "at": [t.translation.x, t.translation.y, t.translation.z],
                        "left_s": p.left(),
                        "target": p.target.map(|o| o.to_bits()),
                        "kind": if p.around { "mirror_image" } else { "decoy" },
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Commanded)>();
            let held: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, h)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "by": h.by.map(|o| o.to_bits()),
                        "wrath": h.wrath,
                        "left_s": h.left(),
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Afflictions)>();
            let terrified: Vec<serde_json::Value> = q
                .iter(world)
                .filter(|(_, a)| a.dread > 0.0)
                .map(|(e, a)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "left_s": a.dread,
                        "from": a.dread_from.map(|p| [p.x, p.y, p.z]),
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "copies": copies,
                "held": held,
                "terrified": terrified,
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_images_stand_to_the_sides_then_round_the_back() {
        let a = mirror_slot(0, 3);
        let b = mirror_slot(1, 3);
        let c = mirror_slot(2, 3);
        assert!(a.0 > 0.0 && b.0 < 0.0 && (a.0 + b.0).abs() < 1e-5);
        assert!(c.0.abs() > a.0.abs() && c.1 > a.1);
        assert!(a.0 > PI * 0.3 && a.0 < PI * 0.6, "{}", a.0);
        let spots: Vec<DVec3> = (0..3)
            .map(|i| decoy_spot(DVec3::ZERO, Vec3::NEG_Z, i, 3))
            .collect();
        assert!((spots[0].x + spots[2].x).abs() < 1e-6 && spots[1].x.abs() < 1e-6);
        assert!((spots[2].x - spots[1].x - 1.7).abs() < 1e-5);
    }

    #[test]
    fn the_terrified_run_away_swerving() {
        let from = DVec3::ZERO;
        let at = DVec3::new(0.0, 0.0, 5.0);
        let mut seen = Vec::new();
        for i in 0..40 {
            let d = flee_dir(at, from, i as f32 * 0.25, 9);
            // Never back toward the terror.
            assert!(d.z > 0.0, "{d:?}");
            seen.push(d.x);
        }
        let lo = seen.iter().cloned().fold(f32::MAX, f32::min);
        let hi = seen.iter().cloned().fold(f32::MIN, f32::max);
        assert!(lo < -0.3 && hi > 0.3, "{lo} {hi}");
    }
}
