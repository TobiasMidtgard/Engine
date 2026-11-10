//! Bone (round-18-plan.md): Necromancy's Undeath, the effect `bone`, the dark mirror of the
//! Wilds. The cast type decides what it makes:
//!
//! * **Bone prison** (Area, Rune, Cone, Rain, Wave, Touch): round each foe the spell lands on,
//!   eight great ribs break out of the floor and close over it like a hand, tips meeting over its
//!   head. It is held (`Statuses::rooted`) for [`crate::compile::Payload::prison_secs`], then
//!   the ribs slide back into the floor (Diablo IV's Bone Prison). A heavy blow on it from
//!   anyone but its jailers' side (a blunt one, or a shove past [`SHATTER_IMPULSE`]) shatters it
//!   into tumbling shards and frees what it held. Fire does nothing to bone. An area also thrusts
//!   bone spikes up across its ground.
//! * **Bone spear** (Bolt, Lob): a barbed spear of bone a little over [`SPEAR_M`] long, drawn on
//!   the missile ([`SpearRide`]); it pierces every body in its line ([`SPEAR_PIERCE`]), then
//!   stands quivering in the floor where it lands and crumbles (Diablo IV's Bone Spear).
//! * **Bone wall** (Wall): a ribcage along the wall's line ([`RIBCAGE_M`] high): pairs of ribs
//!   from either side arching over to meet along a spine that slides along the top as it grows.
//!   Bodies push through it, but each tick it snags them for [`RIBCAGE_SNAG`] s.
//! * **Bone armour** (Self): shards of bone swirl up round the caster for [`SWIRL_SECS`], then
//!   lock onto it as plates. They take [`ARMOUR_SHARE`] off weapon blows (slash, pierce, blunt),
//!   the mirror of bark skin, which takes it off magic; then they crack off and fall.
//! * **Bone dragon** (Summon): dragon.rs.
//!
//! Ribs, the spine and the spikes are roots of bone (wilds.rs grows them, tip first, out of the
//! floor and back in), with spurs and knobs where bark had thorns and leaves.

use std::f32::consts::TAU;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::animation::SkeletonPose;
use engine_world::physics::{PhysicsQueries, QueryFilter};

use crate::bone_art as art;
use crate::caster::Vitality;
use crate::design::CastType;
use crate::looks::{LookCmd, Role};
use crate::runtime::{CastInfo, MagicQueue, Missile, SpellPos, roll};
use crate::summon::{Minion, MinionPart};
use crate::wilds::{Falling, Patch, Root, RootLook, WildPiece};

// ------------------------------------------------------------------------------- numbers

/// Bone's colour for its dust and splinters: old ivory, linear RGB.
pub const BONE_DUST: [f32; 3] = [0.78, 0.72, 0.6];
/// The cold green-white of the grave's light in a bone dragon's heart and breath.
pub const GRAVE_FROST: [f32; 3] = [0.55, 1.0, 0.82];
/// Bodies a bone spear goes through.
pub const SPEAR_PIERCE: u32 = 12;
/// A bone spear's length, m.
pub const SPEAR_M: f32 = 1.6;
/// How deep a spear stands in what it struck, m.
pub const SPEAR_IN: f32 = 0.3;
/// Seconds a spear stands quivering before it crumbles, and how long crumbling takes.
pub const SPEAR_STAND: f32 = 1.7;
pub const SPEAR_CRUMBLE: f32 = 0.6;
/// A shove this hard (N·s) shatters a bone prison.
pub const SHATTER_IMPULSE: f32 = 80.0;
/// Ribs in a prison.
pub const PRISON_RIBS: u32 = 8;
/// A prison's or ribcage's ribs have segments this long, m.
pub const RIB_SEG_M: f32 = 0.15;
/// A ribcage's height, m, its half-width at the floor, and how far apart its ribs stand.
pub const RIBCAGE_M: f32 = 2.2;
pub const RIBCAGE_HALF_W: f32 = 1.05;
pub const RIBCAGE_STEP: f32 = 0.5;
/// Seconds of each half-second tick a ribcage holds what pushes through it.
pub const RIBCAGE_SNAG: f32 = 0.4;
/// The share of a weapon's blow bone armour takes off.
pub const ARMOUR_SHARE: f32 = 0.4;
/// Seconds the shards of bone armour swirl before they lock on.
pub const SWIRL_SECS: f32 = 0.8;
/// Seconds bone spikes across an area stand before they sink back.
pub const SPIKES_HOLD: f32 = 1.6;

/// Cast types whose bone closes a prison over what they land on.
pub fn prison_holds(cast: CastType) -> bool {
    matches!(
        cast,
        CastType::Area
            | CastType::Rune
            | CastType::Cone
            | CastType::Rain
            | CastType::Wave
            | CastType::Touch
    )
}

/// Ribs: great curved bones, spurred.
fn rib_look() -> RootLook {
    RootLook {
        wood: art::bone(),
        spur: art::dark_bone(),
        spur_every: 5,
        spur_length: (1.4, 2.4),
        leaf_every: 0,
        leaf_size: 0.0,
        knob_every: 0,
        knob: 0.0,
        bone: true,
    }
}

/// A spine: a chain of knobbed vertebrae, darker and older than the ribs it carries, with a
/// spike off every other one.
fn spine_look() -> RootLook {
    RootLook {
        wood: art::dark_bone().with_tint(0.62, 0.55, 0.44),
        spur_every: 2,
        spur_length: (1.6, 2.6),
        knob_every: 1,
        knob: 1.45,
        ..rib_look()
    }
}

/// Spikes: bare bone.
fn spike_look() -> RootLook {
    RootLook {
        spur_every: 0,
        ..rib_look()
    }
}

// ------------------------------------------------------------------------------- requests

/// A prison to close over a body (from a hit).
#[derive(Debug, Clone)]
pub struct NewCage {
    pub cast: CastInfo,
    pub target: Entity,
    pub secs: f32,
}

/// Bone armour to grow over a body (from a hit on itself).
#[derive(Debug, Clone)]
pub struct NewArmour {
    pub cast: CastInfo,
    pub target: Entity,
    pub secs: f32,
}

// ------------------------------------------------------------------------------- components

/// A prison of ribs round a body.
#[derive(Component, Debug, Clone)]
pub struct Cage {
    pub target: Entity,
    pub ribs: Vec<Entity>,
    pub age: f32,
    pub secs: f32,
    /// Seconds since it let go (or shattered).
    pub done: Option<f32>,
    /// A heavy blow shattered it (this frame: `Some(false)`, done: `Some(true)`).
    pub shatter: Option<bool>,
    /// The floor under it.
    pub floor: f64,
}

impl Cage {
    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// A ribcage along a wall.
#[derive(Component, Debug, Clone)]
pub struct Ribcage {
    /// The wall field it stands with.
    pub field: Entity,
    pub ribs: Vec<Entity>,
    pub age: f32,
    pub done: Option<f32>,
}

/// Bone armour on a body: its plates, each from its place in the swirl.
#[derive(Component, Debug, Clone)]
pub struct BoneArmour {
    pub target: Entity,
    pub plates: Vec<(Entity, crate::wilds::Plate, Swirl)>,
    pub age: f32,
    pub secs: f32,
}

impl BoneArmour {
    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// Where a shard of armour flies in the swirl before it locks on: its angle round the body at
/// the start, its height (of the body's), how far out, its tumble, and when it locks on, s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swirl {
    pub angle: f32,
    pub height: f32,
    pub out: f32,
    pub tumble: Vec3,
    pub lock: f32,
}

impl Swirl {
    /// The shard's place round the body's feet at `t` s into the swirl, and its turn.
    pub fn at(&self, t: f32, height: f32) -> (Vec3, Quat) {
        // Up out of the floor in a storm round the body, closing in as it rises.
        let a = self.angle + 7.5 * t;
        let rise = (t / (0.6 * SWIRL_SECS)).min(1.0);
        let r = self.out * (1.0 - 0.35 * rise);
        let y = height * self.height * (rise * (2.0 - rise)) + 0.25 * (a * 1.7).sin() * rise;
        (
            Vec3::new(a.cos() * r, y, a.sin() * r),
            Quat::from_scaled_axis(self.tumble * t),
        )
    }
}

/// A bone spear drawn on a missile.
#[derive(Component, Debug, Clone, Default)]
pub struct SpearRide {
    pub drawn: Option<Entity>,
    /// It stuck (or fell) when the missile was spent.
    pub done: bool,
}

/// A spent spear: falling until it strikes the floor, then standing in it, quivering, then
/// crumbling.
#[derive(Component, Debug, Clone, Copy)]
pub struct StuckSpear {
    pub vel: Vec3,
    /// Its point's heading.
    pub dir: Vec3,
    pub floor: f64,
    /// Seconds since it stuck (None: still falling).
    pub stuck: Option<f32>,
    pub age: f32,
    pub burst: bool,
}

// ------------------------------------------------------------------------------- building

/// A bone spear's pieces as children of `parent`, point along +Y, its middle at the origin: a
/// long bone for a shaft, knobbed at the butt, a barbed head of bone.
pub fn spear_pieces(commands: &mut Commands, parent: Entity, seed: u64) {
    let bone = art::bone();
    let dark = art::dark_bone();
    let half = SPEAR_M * 0.5;
    let mut piece = |shape: Shape, m: StandardMaterial, at: Vec3, rot: Quat| {
        commands.spawn((
            Name::new("Bone spear piece"),
            Transform::from_translation(at.as_dvec3()).with_rotation(rot),
            MeshRenderer::new(MeshRef::Shape(shape), MaterialRef::Inline(m)),
            ChildOf(parent),
        ));
    };
    piece(
        Shape::Capsule {
            radius: 0.034,
            height: SPEAR_M * 0.78,
            segments: 10,
        },
        bone,
        Vec3::new(0.0, -0.08, 0.0),
        Quat::IDENTITY,
    );
    // The butt: a bone's knuckled end.
    piece(
        Shape::Rock {
            size: [0.11, 0.13, 0.1],
            seed: (seed % 100_000) as u32,
            detail: 1,
        },
        bone,
        Vec3::new(0.0, -half + 0.05, 0.0),
        Quat::IDENTITY,
    );
    // A collar where the head is bound on.
    piece(
        Shape::Rock {
            size: [0.085, 0.07, 0.085],
            seed: ((seed + 7) % 100_000) as u32,
            detail: 1,
        },
        dark,
        Vec3::new(0.0, half - 0.36, 0.0),
        Quat::IDENTITY,
    );
    // The head: a long blade of bone, and barbs raked back off it.
    piece(
        Shape::Cone {
            radius: 0.06,
            height: 0.38,
            segments: 6,
        },
        bone,
        Vec3::new(0.0, half - 0.15, 0.0),
        Quat::IDENTITY,
    );
    for i in 0..3 {
        let a = i as f32 / 3.0 * TAU + 0.4;
        let out = Vec3::new(a.cos(), 0.0, a.sin());
        let dir = (out * 0.55 - Vec3::Y).normalize();
        piece(
            Shape::Cone {
                radius: 0.022,
                height: 0.17,
                segments: 5,
            },
            dark,
            Vec3::new(0.0, half - 0.3, 0.0) + out * 0.06 + dir * 0.06,
            Quat::from_rotation_arc(Vec3::Y, dir),
        );
    }
}

/// The floor under a point, past whoever stands there.
fn floor_under(queries: &PhysicsQueries, p: DVec3, creature: &dyn Fn(Entity) -> bool) -> DVec3 {
    crate::cataclysm::ground_under(queries, p + DVec3::Y * 2.0, 30.0, Vec::new(), creature)
        .map_or(p, |h| h.point)
}

/// What lies along a heading from a point, past creatures.
fn struck(
    queries: &PhysicsQueries,
    from: DVec3,
    dir: Vec3,
    max: f32,
    creature: &dyn Fn(Entity) -> bool,
) -> Option<engine_world::physics::Hit> {
    let mut filter = QueryFilter::default();
    for _ in 0..6 {
        let h = queries.raycast(from, dir, max, &filter)?;
        if !creature(h.entity) {
            return Some(h);
        }
        filter.exclude.push(h.entity);
    }
    None
}

// ------------------------------------------------------------------------------- systems

/// Close the prisons, raise the spikes and ribcages and grow the armour the hits and fields
/// asked for; mark the prisons heavy blows shattered.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn spawn_bones(
    mut queue: ResMut<MagicQueue>,
    queries: Res<PhysicsQueries>,
    bodies: Query<(Option<&Minion>, Option<&SkeletonPose>, &GlobalTransform)>,
    creatures: Query<(), Or<(With<Vitality>, With<Minion>, With<MinionPart>)>>,
    mut cages: Query<&mut Cage>,
    mut armours: Query<&mut BoneArmour>,
    mut commands: Commands,
) {
    let creature = |e: Entity| creatures.contains(e);
    for t in std::mem::take(&mut queue.cage_breaks) {
        for mut c in cages.iter_mut().filter(|c| c.target == t) {
            if c.done.is_none() && c.shatter.is_none() {
                c.shatter = Some(false);
            }
        }
    }
    let mut looks = Vec::new();
    for nc in std::mem::take(&mut queue.cages) {
        let Ok((minion, _, g)) = bodies.get(nc.target) else {
            continue;
        };
        // A body already caged is held longer, not caged twice.
        if let Some(mut c) = cages
            .iter_mut()
            .find(|c| c.target == nc.target && c.done.is_none())
        {
            c.secs = c.secs.max(c.age + nc.secs);
            continue;
        }
        let body = g.translation();
        let ground = floor_under(&queries, body + DVec3::Y * 0.5, &creature);
        let (height, radius, floats) = match minion {
            Some(m) => (
                crate::summon::body_height(m.kind, m.size),
                crate::summon::body_radius(m.kind, m.size),
                !m.kind.grounded(),
            ),
            None => (1.8, 0.36, false),
        };
        // A floating body's root is its middle.
        let top = (body.y - ground.y) as f32 + if floats { 0.6 * height } else { height };
        let hs = (top / 1.8).clamp(0.6, 2.2);
        let origin = DVec3::new(body.x, ground.y, body.z);
        let seed = nc.cast.seed as u64 ^ nc.target.to_bits().wrapping_mul(0x2545_f491);
        let cage = commands
            .spawn((
                Name::new("Bone prison"),
                Transform::from_translation(origin),
            ))
            .id();
        let base_r = radius + 0.32 * hs.sqrt();
        let mut ribs = Vec::new();
        let look = rib_look();
        for i in 0..PRISON_RIBS as u64 {
            let h = |salt: u64| roll(seed ^ (i + 1).wrapping_mul(0x51ed), salt);
            let angle = (i as f32 + 0.3 * h(1)) / PRISON_RIBS as f32 * TAU;
            let path = art::prison_rib(
                angle,
                base_r * (0.95 + 0.1 * h(2)),
                base_r * (1.05 + 0.1 * h(3)),
                top * (1.1 + 0.08 * h(4)) + 0.2,
                0.1 + 0.08 * h(5),
                0.45,
            );
            let start = path[0].with_y(0.0);
            ribs.push(crate::wilds::grow_root(
                &mut commands,
                origin,
                &path,
                RIB_SEG_M,
                (0.095 + 0.025 * h(6)) * hs.sqrt(),
                0.035 * i as f32,
                None,
                seed ^ i,
                &look,
            ));
            crate::wilds::heave(
                &mut commands,
                cage,
                start,
                0.42 * hs.sqrt(),
                seed ^ i,
                art::rubble(),
            );
        }
        commands.entity(cage).insert(Cage {
            target: nc.target,
            ribs,
            age: 0.0,
            secs: nc.secs,
            done: None,
            shatter: None,
            floor: ground.y,
        });
        looks.push(LookCmd::Burst {
            at: origin,
            up: Vec3::Y,
            look: "raise".into(),
            role: Role::Impact,
            scale: 1.0 * hs,
            color: crate::undead::GRAVE_DUST,
        });
    }
    for np in std::mem::take(&mut queue.spikes) {
        let seed = np.cast.seed as u64 ^ np.at.x.to_bits() ^ (np.at.z.to_bits() << 5);
        let n = (np.radius * np.radius * 0.8).round().clamp(3.0, 12.0) as u64;
        let center = floor_under(&queries, np.at + DVec3::Y * 0.5, &creature);
        let patch = commands
            .spawn((
                Name::new("Bone spikes"),
                Transform::from_translation(center),
            ))
            .id();
        let mut ids = Vec::new();
        let look = spike_look();
        for i in 0..n {
            let h = |salt: u64| roll(seed ^ i.wrapping_mul(0x7f4a_7c15), salt);
            let a = h(1) * TAU;
            let d = np.radius * (0.2 + 0.8 * h(2).sqrt());
            let base = Vec3::new(a.cos() * d, 0.0, a.sin() * d);
            // They lean out from the middle, as if something burst up under it.
            let path = art::spike(base, 0.7 + 0.7 * h(3), base, (seed ^ i) as u32);
            ids.push(crate::wilds::grow_root(
                &mut commands,
                center,
                &path,
                RIB_SEG_M * 0.7,
                0.07 + 0.035 * h(4),
                0.25 * h(5),
                Some(SPIKES_HOLD + 0.4 * h(6)),
                seed ^ i,
                &look,
            ));
            crate::wilds::heave(&mut commands, patch, base, 0.36, seed ^ i, art::rubble());
        }
        commands.entity(patch).insert(Patch {
            roots: ids,
            age: 0.0,
            sinking: None,
        });
    }
    for nh in std::mem::take(&mut queue.ribcages) {
        let seed = nh.cast.seed as u64 ^ 0xb0e5;
        let center = floor_under(&queries, nh.at + DVec3::Y * 0.5, &creature);
        let along = Vec3::new(nh.along.x, 0.0, nh.along.z).normalize_or(Vec3::X);
        let across = Vec3::new(-along.z, 0.0, along.x);
        let cage = commands
            .spawn((Name::new("Ribcage"), Transform::from_translation(center)))
            .id();
        let half = nh.half.max(0.6);
        let mut ids = Vec::new();
        // The spine first: up out of the floor past one end and along the ridge.
        let spine = art::spine(along, half, RIBCAGE_M);
        ids.push(crate::wilds::grow_root(
            &mut commands,
            center,
            &spine,
            RIB_SEG_M,
            0.16,
            0.0,
            None,
            seed,
            &spine_look(),
        ));
        // Where the spine's tip is when it passes `u` along the ridge, s.
        let reaches = |u: f32| (RIBCAGE_M + 0.7 + (u + half)) / crate::wilds::GROW_M_S;
        let look = rib_look();
        let n = (2.0 * half / RIBCAGE_STEP).floor() as u64 + 1;
        for i in 0..n {
            for (j, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                let id = i * 2 + j as u64;
                let h = |salt: u64| roll(seed ^ (id + 1).wrapping_mul(0x2f1b), salt);
                let u = (-half + i as f32 * RIBCAGE_STEP + 0.09 * side).clamp(-half, half);
                let path = art::wall_rib(
                    along,
                    across,
                    u,
                    side,
                    RIBCAGE_HALF_W * (0.95 + 0.1 * h(1)),
                    RIBCAGE_M * (0.97 + 0.05 * h(2)),
                    0.15 * (h(3) - 0.5),
                );
                // Each rib is out just as the spine slides over its tip.
                let len = crate::wilds_art::length(&path);
                let delay = (reaches(u) - len / crate::wilds::GROW_M_S * 0.85).max(0.0);
                let start = path[0].with_y(0.0);
                ids.push(crate::wilds::grow_root(
                    &mut commands,
                    center,
                    &path,
                    RIB_SEG_M,
                    0.095 + 0.02 * h(4),
                    delay,
                    None,
                    seed ^ id,
                    &look,
                ));
                if id % 2 == i % 2 {
                    crate::wilds::heave(&mut commands, cage, start, 0.4, seed ^ id, art::rubble());
                }
            }
        }
        commands.entity(cage).insert(Ribcage {
            field: nh.field,
            ribs: ids,
            age: 0.0,
            done: None,
        });
    }
    for na in std::mem::take(&mut queue.armours) {
        let Ok((minion, pose, _)) = bodies.get(na.target) else {
            continue;
        };
        if let Some(mut a) = armours.iter_mut().find(|a| a.target == na.target) {
            a.secs = a.secs.max(a.age + na.secs);
            continue;
        }
        let height = minion.map_or(1.8, |m| crate::summon::body_height(m.kind, m.size));
        let seed = na.cast.seed as u64 ^ 0xa4e0;
        let plates = crate::wilds::plates_for(pose, height, seed);
        let n = plates.len().max(1);
        let pieces = plates
            .into_iter()
            .enumerate()
            .map(|(i, mut p)| {
                let h = |salt: u64| roll(seed ^ (i as u64 + 1).wrapping_mul(0x9e37), salt);
                // Bone plates: bigger and thicker than bark's, fewer gaps.
                p.size *= Vec3::new(1.2, 1.1, 1.6);
                let e = commands
                    .spawn((
                        Name::new("Bone armour plate"),
                        Transform::from_scale(Vec3::ZERO),
                        MeshRenderer::new(
                            MeshRef::Shape(Shape::Rock {
                                size: p.size.to_array(),
                                seed: ((seed + i as u64 * 7919) % 100_000) as u32,
                                detail: 2,
                            }),
                            MaterialRef::Inline(art::bone()),
                        ),
                        WildPiece,
                    ))
                    .id();
                // Every third plate bears a spur.
                if i % 3 == 1 {
                    let len = 0.1 + 0.08 * h(1);
                    let dir = (Vec3::Z + Vec3::Y * 0.6).normalize();
                    commands.spawn((
                        Name::new("Bone spur"),
                        Transform::from_translation(
                            (Vec3::Z * p.size.z * 0.4 + dir * len * 0.45).as_dvec3(),
                        )
                        .with_rotation(Quat::from_rotation_arc(Vec3::Y, dir)),
                        MeshRenderer::new(
                            MeshRef::Shape(Shape::Cone {
                                radius: 0.025,
                                height: len,
                                segments: 6,
                            }),
                            MaterialRef::Inline(art::dark_bone()),
                        ),
                        WildPiece,
                        ChildOf(e),
                    ));
                }
                let swirl = Swirl {
                    angle: h(2) * TAU,
                    height: 0.15 + 0.8 * h(3),
                    out: 0.9 + 0.5 * h(4),
                    tumble: Vec3::new(h(5) - 0.5, h(6) - 0.5, h(7) - 0.5) * 18.0,
                    lock: SWIRL_SECS * (0.55 + 0.45 * (i as f32 / n as f32)),
                };
                (e, p, swirl)
            })
            .collect();
        commands.spawn((
            Name::new("Bone armour"),
            BoneArmour {
                target: na.target,
                plates: pieces,
                age: 0.0,
                secs: na.secs,
            },
        ));
        if let Ok((_, _, g)) = bodies.get(na.target) {
            looks.push(LookCmd::Burst {
                at: g.translation(),
                up: Vec3::Y,
                look: "bones".into(),
                role: Role::Impact,
                scale: 0.8,
                color: BONE_DUST,
            });
        }
    }
    queue.looks.extend(looks);
}

/// Prisons and ribcages hold and let go; a shattered prison flies apart into tumbling shards.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_bones(
    time: Res<Time>,
    mut cages: Query<(Entity, &mut Cage)>,
    mut ribcages: Query<(Entity, &mut Ribcage)>,
    mut roots: Query<(&mut Root, &GlobalTransform)>,
    mut tfs: Query<&mut Transform, With<WildPiece>>,
    fields: Query<&crate::runtime::Field>,
    vitals: Query<&Vitality>,
    alive: Query<&GlobalTransform, Without<Root>>,
    children: Query<&Children>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (e, mut c) in &mut cages {
        c.age += dt;
        let target = alive.get(c.target).ok().map(|g| g.translation());
        let held = target.is_some() && vitals.get(c.target).map_or(true, |v| !v.down());
        if c.shatter == Some(false) {
            c.shatter = Some(true);
            c.done = Some(0.0);
            let mid = target.unwrap_or(DVec3::ZERO);
            let seed = e.to_bits();
            for &r in &c.ribs {
                let Ok((root, g)) = roots.get(r) else {
                    continue;
                };
                let origin = g.translation();
                let pieces: Vec<Entity> = root
                    .segs
                    .iter()
                    .copied()
                    .chain(root.extras.iter().map(|x| x.entity))
                    .collect();
                for s in pieces {
                    let Ok(tf) = tfs.get(s) else {
                        continue;
                    };
                    if tf.scale.length_squared() < 1e-8 {
                        continue;
                    }
                    let at = origin + tf.translation;
                    let out = (at - mid).as_vec3();
                    let out = Vec3::new(out.x, 0.0, out.z).normalize_or(Vec3::X);
                    let h = |salt: u64| roll(seed ^ s.to_bits(), salt) - 0.5;
                    let fall = Falling {
                        vel: out * (2.2 + 2.0 * h(1)) + Vec3::Y * (1.6 + 1.6 * h(2)),
                        spin: Vec3::new(h(3), h(4), h(5)) * 14.0,
                        age: 0.4 * h(6),
                        floor: c.floor + 0.04,
                    };
                    let tf = Transform {
                        translation: at,
                        ..*tf
                    };
                    commands.entity(s).remove::<ChildOf>().insert((tf, fall));
                }
                commands.entity(r).despawn();
            }
            c.ribs.clear();
            for (look, scale) in [("bones", 1.3), ("raise", 1.0)] {
                queue.looks.push(LookCmd::Burst {
                    at: DVec3::new(mid.x, c.floor, mid.z),
                    up: Vec3::Y,
                    look: look.into(),
                    role: Role::Impact,
                    scale,
                    color: BONE_DUST,
                });
            }
        }
        if c.done.is_none() && (c.age >= c.secs || !held) {
            c.done = Some(0.0);
        }
        if let Some(d) = c.done.as_mut() {
            *d += dt;
            let d = *d;
            for &r in &c.ribs {
                if let Ok((mut r, _)) = roots.get_mut(r) {
                    r.let_go();
                }
            }
            crate::wilds::sink(&children, e, d, &mut tfs);
            if c.ribs.iter().all(|r| roots.get(*r).is_err()) && d > 0.8 {
                commands.entity(e).despawn();
            }
        }
    }
    for (e, mut rc) in &mut ribcages {
        rc.age += dt;
        let standing = fields.get(rc.field).is_ok_and(|f| f.spent.is_none());
        if rc.done.is_none() && !standing && rc.age > 0.5 {
            rc.done = Some(0.0);
            for (i, &r) in rc.ribs.iter().enumerate() {
                if let Ok((mut r, _)) = roots.get_mut(r) {
                    // They sink a little apart, the spine last.
                    r.until = Some(r.out().max(0.0) + 0.3 * roll(i as u64, 9));
                }
            }
        }
        if let Some(d) = rc.done.as_mut() {
            *d += dt;
            crate::wilds::sink(&children, e, *d, &mut tfs);
            if *d > 2.5 {
                commands.entity(e).despawn();
            }
        }
    }
}

/// Bone armour: its shards swirl up round the body, lock onto its bones, ride them, and when
/// its time is up crack off and fall.
#[allow(clippy::type_complexity)]
pub fn hold_armour(
    time: Res<Time>,
    mut armours: Query<(Entity, &mut BoneArmour)>,
    bodies: Query<(&GlobalTransform, Option<&SkeletonPose>, Option<&Vitality>)>,
    mut tfs: Query<&mut Transform, With<WildPiece>>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut a) in &mut armours {
        a.age += dt;
        let body = bodies.get(a.target).ok();
        let down = body.is_none_or(|(_, _, v)| v.is_some_and(|v| v.down()));
        let Some((g, pose, _)) = body else {
            for (p, ..) in &a.plates {
                commands.entity(*p).despawn();
            }
            commands.entity(e).despawn();
            continue;
        };
        let front = g.rotation() * Vec3::NEG_Z;
        let feet = g.translation();
        let height = 1.8;
        let ending = a.age >= a.secs || down;
        let mut locked = 0;
        for (pe, p, sw) in &a.plates {
            let (x, y) = match (p.bones, pose) {
                (Some((i, j)), Some(pose)) => {
                    match (pose.bone_world(g, i), pose.bone_world(g, j)) {
                        (Some(x), Some(y)) => ((x.0 - feet).as_vec3(), (y.0 - feet).as_vec3()),
                        _ => continue,
                    }
                }
                _ => (
                    Vec3::Y * p.along * height * 0.9,
                    Vec3::Y * (p.along * height * 0.9 + 0.3),
                ),
            };
            let (on, rot) = if p.bones.is_some() {
                crate::wilds::plate_pose(p, x, y, front)
            } else {
                let mut q = *p;
                q.along = 0.0;
                crate::wilds::plate_pose(&q, x, y, front)
            };
            // In the swirl, then onto the body.
            let (sw_at, sw_rot) = sw.at(a.age.min(sw.lock), height);
            let u = ((a.age - sw.lock) / 0.14).clamp(0.0, 1.0);
            let u = u * u * (3.0 - 2.0 * u);
            if u >= 1.0 {
                locked += 1;
            }
            let pos = sw_at.lerp(on, u);
            let Ok(mut tf) = tfs.get_mut(*pe) else {
                continue;
            };
            tf.translation = feet + pos.as_dvec3();
            tf.rotation = sw_rot.slerp(rot, u);
            tf.scale = Vec3::splat((a.age / 0.15).min(1.0) * (0.75 + 0.25 * u));
            if ending {
                let out = (pos - Vec3::new(0.0, pos.y, 0.0)).normalize_or(front);
                commands.entity(*pe).insert(Falling {
                    vel: out * 1.6 + Vec3::Y * 1.3,
                    spin: Vec3::new(
                        roll(pe.to_bits(), 1) - 0.5,
                        roll(pe.to_bits(), 2) - 0.5,
                        roll(pe.to_bits(), 3) - 0.5,
                    ) * 9.0,
                    age: 0.0,
                    floor: feet.y + p.size.y as f64 * 0.3,
                });
            }
        }
        // The last shard locks on: a puff of bone dust.
        let all = locked == a.plates.len();
        let was = a.age - dt < SWIRL_SECS + 0.14;
        if all && was && a.age >= SWIRL_SECS + 0.14 {
            queue.looks.push(LookCmd::Burst {
                at: feet + DVec3::Y * 1.1,
                up: Vec3::Y,
                look: "bones".into(),
                role: Role::Impact,
                scale: 0.6,
                color: BONE_DUST,
            });
        }
        if ending {
            queue.looks.push(LookCmd::Burst {
                at: feet + DVec3::Y * 1.0,
                up: Vec3::Y,
                look: "bones".into(),
                role: Role::Impact,
                scale: 0.8,
                color: BONE_DUST,
            });
            commands.entity(e).despawn();
        }
    }
}

/// Bone spears: drawn on their missiles; when one is spent it stands in what it struck, or falls
/// to the floor and sticks there.
#[allow(clippy::type_complexity)]
pub fn ride_spears(
    mut rides: Query<(Entity, &mut SpearRide, &Missile, &SpellPos)>,
    queries: Res<PhysicsQueries>,
    creatures: Query<(), Or<(With<Vitality>, With<Minion>, With<MinionPart>)>>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let creature = |e: Entity| creatures.contains(e);
    for (e, mut r, m, sp) in &mut rides {
        if r.drawn.is_none() && m.spent.is_none() {
            // The missile's +Y is its flight: the spear's point a little ahead of it.
            let child = commands
                .spawn((
                    Name::new("Bone spear"),
                    Transform::from_translation(DVec3::NEG_Y * (SPEAR_M * 0.35) as f64),
                    ChildOf(e),
                ))
                .id();
            spear_pieces(&mut commands, child, e.to_bits() ^ m.cast.seed as u64);
            r.drawn = Some(child);
        }
        if m.spent.is_some() && !r.done {
            r.done = true;
            if let Some(c) = r.drawn.take() {
                commands.entity(c).despawn();
            }
            let dir = m.vel.normalize_or(sp.up);
            let floor = floor_under(&queries, sp.pos, &creature).y;
            let tip = sp.pos + (dir * SPEAR_M * 0.15).as_dvec3();
            let (at, stuck) =
                match struck(&queries, tip - (dir * 0.5).as_dvec3(), dir, 1.0, &creature) {
                    Some(h) => (h.point, true),
                    None => (tip, false),
                };
            let center =
                at - (dir * (SPEAR_M * 0.5 - if stuck { SPEAR_IN } else { 0.0 })).as_dvec3();
            let spear = commands
                .spawn((
                    Name::new("Bone spear"),
                    Transform::from_translation(center)
                        .with_rotation(Quat::from_rotation_arc(Vec3::Y, dir)),
                    StuckSpear {
                        vel: m.vel * 0.35,
                        dir,
                        floor,
                        stuck: stuck.then_some(0.0),
                        age: 0.0,
                        burst: false,
                    },
                ))
                .id();
            spear_pieces(&mut commands, spear, e.to_bits() ^ m.cast.seed as u64);
            if stuck {
                queue.looks.push(LookCmd::Burst {
                    at,
                    up: -dir,
                    look: "raise".into(),
                    role: Role::Impact,
                    scale: 0.4,
                    color: crate::undead::GRAVE_DUST,
                });
            }
        }
    }
}

/// Spent spears fall, stick, quiver, and crumble.
pub fn run_spears(
    time: Res<Time>,
    mut spears: Query<(Entity, &mut StuckSpear, &mut Transform)>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (e, mut s, mut tf) in &mut spears {
        s.age += dt;
        match s.stuck {
            None => {
                s.vel.y -= 9.8 * dt;
                s.dir = s.vel.normalize_or(s.dir);
                tf.translation += (s.vel * dt).as_dvec3();
                tf.rotation = Quat::from_rotation_arc(Vec3::Y, s.dir);
                let tip = tf.translation + (s.dir * SPEAR_M * 0.5).as_dvec3();
                if tip.y <= s.floor || s.age > 4.0 {
                    // Into the floor point first.
                    let into = s.dir * (s.floor - tip.y) as f32 / s.dir.y.min(-0.2);
                    tf.translation += (into + s.dir * SPEAR_IN).as_dvec3();
                    s.stuck = Some(0.0);
                    queue.looks.push(LookCmd::Burst {
                        at: tf.translation + (s.dir * (SPEAR_M * 0.5 - SPEAR_IN)).as_dvec3(),
                        up: Vec3::Y,
                        look: "raise".into(),
                        role: Role::Impact,
                        scale: 0.4,
                        color: crate::undead::GRAVE_DUST,
                    });
                }
            }
            Some(t) => {
                let t = t + dt;
                s.stuck = Some(t);
                // It quivers where it stands, the shudder dying away.
                let axis = s.dir.cross(Vec3::Y).normalize_or(Vec3::X);
                let wobble = 0.11 * (-5.0 * t).exp() * (t * TAU * 9.0).sin();
                tf.rotation =
                    Quat::from_axis_angle(axis, wobble) * Quat::from_rotation_arc(Vec3::Y, s.dir);
                if t > SPEAR_STAND {
                    let c = ((t - SPEAR_STAND) / SPEAR_CRUMBLE).min(1.0);
                    if !s.burst {
                        s.burst = true;
                        queue.looks.push(LookCmd::Burst {
                            at: tf.translation,
                            up: Vec3::Y,
                            look: "bones".into(),
                            role: Role::Impact,
                            scale: 0.5,
                            color: BONE_DUST,
                        });
                    }
                    tf.scale = Vec3::new(1.0 - 0.6 * c, 1.0 - c * c, 1.0 - 0.6 * c).max(Vec3::ZERO);
                    tf.translation -= (s.dir * (0.5 * dt / SPEAR_CRUMBLE)).as_dvec3();
                    if c >= 1.0 {
                        commands.entity(e).despawn();
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------- upkeep

pub fn clear(world: &mut World) {
    let mut gone: Vec<Entity> = Vec::new();
    gone.extend(world.query_filtered::<Entity, With<Cage>>().iter(world));
    gone.extend(world.query_filtered::<Entity, With<Ribcage>>().iter(world));
    gone.extend(
        world
            .query_filtered::<Entity, With<StuckSpear>>()
            .iter(world),
    );
    let armours: Vec<(Entity, Vec<Entity>)> = world
        .query::<(Entity, &BoneArmour)>()
        .iter(world)
        .map(|(e, a)| (e, a.plates.iter().map(|p| p.0).collect()))
        .collect();
    for (e, plates) in armours {
        gone.push(e);
        gone.extend(plates);
    }
    for e in gone {
        if let Ok(ec) = world.get_entity_mut(e) {
            ec.despawn();
        }
    }
}

pub fn register(app: &mut App) {
    app.register_api(
        "magic.bones",
        "Bone now: prisons (whom they hold, seconds left, ribs, shattered), ribcages, bone \
         armour (whom, seconds left, plates) and spears standing in the floor.",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Cage)>();
            let cages: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, c)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "target": c.target.to_bits(),
                        "left_s": c.left(),
                        "ribs": c.ribs.len(),
                        "done": c.done.is_some(),
                        "shattered": c.shatter.is_some(),
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Ribcage)>();
            let ribcages: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, r)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "ribs": r.ribs.len(),
                        "done": r.done.is_some(),
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &BoneArmour)>();
            let armours: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, a)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "target": a.target.to_bits(),
                        "left_s": a.left(),
                        "plates": a.plates.len(),
                    })
                })
                .collect();
            let spears = world
                .query::<&StuckSpear>()
                .iter(world)
                .filter(|s| s.stuck.is_some())
                .count();
            Ok(serde_json::json!({
                "cages": cages,
                "ribcages": ribcages,
                "armours": armours,
                "spears": spears,
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bone_cages_with_area_casts_and_spears_with_missiles() {
        assert!(prison_holds(CastType::Area) && prison_holds(CastType::Touch));
        assert!(!prison_holds(CastType::Bolt) && !prison_holds(CastType::Wall));
        assert!(!prison_holds(CastType::OnSelf) && !prison_holds(CastType::Summon));
    }

    #[test]
    fn armour_shards_swirl_up_round_the_body_and_close_in() {
        let s = Swirl {
            angle: 0.0,
            height: 0.6,
            out: 1.2,
            tumble: Vec3::X,
            lock: 0.6,
        };
        let (a, _) = s.at(0.0, 1.8);
        assert!(a.y.abs() < 1e-3, "out of the floor");
        assert!((Vec3::new(a.x, 0.0, a.z).length() - 1.2).abs() < 1e-3);
        let (b, _) = s.at(0.5, 1.8);
        assert!(b.y > 0.8, "{b}");
        assert!(Vec3::new(b.x, 0.0, b.z).length() < 1.0, "closing in");
    }
}
