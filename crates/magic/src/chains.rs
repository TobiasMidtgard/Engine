//! Pact chains (round-15-plan.md): iron chains that magic draws itself, link by link, for the
//! Pactbinding effect `chains`. The cast type decides what the chains do:
//!
//! * **Binding** (Area, Rain, Rune, Wave, Wall, Cone, Touch, Summon): under each body the spell
//!   lands on, a sigil flares on the floor and four chains burst out of it to the body's wrists
//!   and ankles (a minion's fists and feet, a skinned body's hand and foot bones), pull taut and
//!   hold it: the body is rooted (`Statuses::rooted`) for [`crate::compile::Payload::bind_secs`]
//!   (Morgana's Soul Shackles, the design's "Chains pin them"). When the binding ends the chains
//!   let go and fall to the floor.
//! * **Hook** (Bolt, Lob): the bolt pays out a chain from the hand behind it. When it bites a
//!   body the chain reels in over [`REEL_SECS`], dragging the body to the caster (Thresh's Death
//!   Sentence, Scorpion's spear); a miss reels the chain back in empty.
//! * **Bonds** (Chain): each body the chain hops to is chained to the one before for
//!   [`crate::compile::Payload::bond_secs`]; a share ([`BOND_SHARE`]) of the damage any of them
//!   takes is dealt to the others (Dota 2's Fatal Bonds).
//!
//! A chain is a Verlet line ([`Chain`]): points that keep their spacing, ends pinned to a floor
//! point, a hand, a body, a skeleton's bone or a minion's piece ([`End`]), so it sags while slack,
//! runs straight when taut, swings when what it holds moves and drops when it lets go. Each link
//! is the tether's iron torus, a quarter turn from the one before, with an ember glow.

use std::collections::HashMap;

use engine_core::math::{DVec3, Mat3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    MaterialPreset, MaterialRef, MeshRef, MeshRenderer, PointLight, Shape, StandardMaterial,
};
use engine_world::animation::SkeletonPose;
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{DamageEvent, WorldShift};

use crate::caster::{MagicCaster, Vitality};
use crate::looks::{LookCmd, Role};
use crate::runtime::{CastInfo, MagicQueue, Missile, SpellPos, ground_below, hand_of, roll};
use crate::summon::{Minion, MinionPart};

// ------------------------------------------------------------------------------- numbers

/// Spacing of a chain's links, m: a heavy chain, read from across the hall.
pub const LINK_M: f32 = 0.12;
/// Seconds a chain's head takes to fly from the floor (or the hand) to what it binds.
pub const SHOOT_SECS: f32 = 0.24;
/// Seconds a binding's chains take to pull taut once they bite.
pub const TAUT_SECS: f32 = 0.35;
/// Seconds a chain that let go takes to fall and fade.
pub const FALL_SECS: f32 = 1.1;
/// Seconds a hook takes to drag its catch in, and to reel back in empty.
pub const REEL_SECS: f32 = 0.55;
pub const RETRACT_SECS: f32 = 0.3;
/// A dragged body stops this far in front of its caster, m.
pub const DRAG_STOP_M: f32 = 1.8;
/// How far from a bound body its sigil's chains come out of the floor, m.
pub const SIGIL_M: f32 = 0.95;
/// The share of damage one bonded body takes that the others take too.
pub const BOND_SHARE: f32 = 0.25;
/// The chains' ember glow and the sigils' fire.
pub const EMBER: [f32; 3] = [1.0, 0.22, 0.05];
/// A link's glow, nits: a dull heat in the seams, so the iron reads dark.
pub const LINK_GLOW: f32 = 60.0;
/// A sigil's glow, nits: a ring of fire on the floor in the daylit hall.
pub const SIGIL_GLOW: f32 = 7_000.0;
/// Verlet passes per step.
const PASSES: usize = 12;

// ------------------------------------------------------------------------------- the chain

/// Where a chain's end is held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum End {
    /// A point in the world (a sigil on the floor).
    At(DVec3),
    /// A point in a body's own frame (its feet at the origin).
    Body(Entity, Vec3),
    /// A bone of a skinned body.
    Bone(Entity, usize),
    /// A piece of a minion (a fist, a foot, a core).
    Piece(Entity),
    /// A caster's casting hand.
    Hand(Entity),
}

/// One drawn chain: a Verlet line of points with a link between each pair.
#[derive(Component, Debug, Clone)]
pub struct Chain {
    pub a: End,
    pub b: End,
    /// Points from `a` to `b`, and where each was the step before.
    pub points: Vec<DVec3>,
    pub prev: Vec<DVec3>,
    pub links: Vec<Entity>,
    pub age: f32,
    /// Seconds the head takes to fly from `a` to `b`.
    pub shoot: f32,
    /// How long it wants to be against the span between its ends (above 1 sags, below pulls
    /// taut), unless it has a length of its own.
    pub slack: f32,
    pub length: Option<f32>,
    /// It lies on the floor (or falls to it) no lower than this.
    pub floor: Option<f64>,
    /// Seconds since it let go of both ends; it falls and fades.
    pub loose: Option<f32>,
    /// Reeling in: `b` slides to `a` over this many seconds (since `reel_from`).
    pub reel: Option<(f32, f32, DVec3)>,
    /// A jolt that shakes it (a bound body straining), fading.
    pub jolt: f32,
    pub seed: u64,
    /// Its ends, last resolved.
    pub a_at: DVec3,
    pub b_at: DVec3,
}

impl Chain {
    pub fn new(a: End, b: End, a_at: DVec3, b_at: DVec3, n: usize, seed: u64) -> Self {
        let n = n.max(2);
        let points = vec![a_at; n + 1];
        Self {
            a,
            b,
            prev: points.clone(),
            points,
            links: Vec::new(),
            age: 0.0,
            shoot: SHOOT_SECS,
            slack: 1.05,
            length: None,
            floor: None,
            loose: None,
            reel: None,
            jolt: 0.0,
            seed,
            a_at,
            b_at,
        }
    }

    /// Links in a chain that spans `m` metres with some sag.
    pub fn links_for(m: f32) -> usize {
        ((m * 1.15 / LINK_M).ceil() as usize).clamp(4, 160)
    }

    /// Where the head is: flying out from `a` to `b` while it shoots, then on `b`; reeling back
    /// to `a` when reeled.
    pub fn head(&self) -> DVec3 {
        if let Some((t, secs, from)) = self.reel {
            let k = smooth((t / secs.max(1e-3)).min(1.0));
            return from.lerp(self.a_at, k as f64);
        }
        let k = smooth((self.age / self.shoot.max(1e-3)).min(1.0));
        self.a_at.lerp(self.b_at, k as f64)
    }

    /// The span between its ends now, m.
    pub fn span(&self) -> f32 {
        self.a_at.distance(self.head()) as f32
    }

    /// One Verlet step: points swing under gravity, the ends hold where they are held, every
    /// pair keeps its spacing.
    pub fn step(&mut self, dt: f32, g: f32) {
        let n = self.points.len() - 1;
        let dt = dt.clamp(0.0, 1.0 / 20.0) as f64;
        let held = self.loose.is_none();
        let head = self.head();
        // Interior points move on and fall.
        let damp = if held { 0.96 } else { 0.9 };
        let shake = self.jolt.max(0.0);
        for i in 0..=n {
            if held && (i == 0 || i == n) {
                continue;
            }
            let v = (self.points[i] - self.prev[i]) * damp;
            self.prev[i] = self.points[i];
            let wob = if shake > 0.0 && i > 0 && i < n {
                let s = self.seed.wrapping_add(i as u64);
                let u = (self.age * 37.0 + i as f32 * 0.9).sin();
                DVec3::new(
                    ((roll(s, 1) - 0.5) * u) as f64,
                    (u * 0.5) as f64,
                    ((roll(s, 2) - 0.5) * u) as f64,
                ) * (0.012 * shake) as f64
            } else {
                DVec3::ZERO
            };
            self.points[i] += v + DVec3::new(0.0, -(g as f64) * dt * dt, 0.0) + wob;
        }
        if held {
            self.points[0] = self.a_at;
            self.points[n] = head;
        }
        let want = match self.length {
            Some(l) => l.max(self.span() * 0.2),
            None if self.age < self.shoot => self.span() * 1.02,
            None => self.span() * self.slack,
        };
        let rest = (want / n as f32).max(0.005) as f64;
        for _ in 0..PASSES {
            for i in 0..n {
                let (pa, pb) = (self.points[i], self.points[i + 1]);
                let d = pb - pa;
                let len = d.length();
                if len < 1e-9 {
                    continue;
                }
                let wa = if held && i == 0 { 0.0 } else { 1.0 };
                let wb = if held && i + 1 == n { 0.0 } else { 1.0 };
                let w = wa + wb;
                if w == 0.0 {
                    continue;
                }
                let fix = d * ((len - rest) / len / w);
                self.points[i] += fix * wa;
                self.points[i + 1] -= fix * wb;
            }
            if let Some(floor) = self.floor {
                for p in &mut self.points {
                    p.y = p.y.max(floor + 0.03);
                }
            }
        }
        // Pulled taut, it runs straight between its ends (a few passes can't pull twenty links
        // straight against gravity on their own).
        if held && (want as f64) < self.a_at.distance(head) {
            for i in 1..n {
                let straight = self.a_at.lerp(head, i as f64 / n as f64);
                self.points[i] = self.points[i].lerp(straight, 0.6);
            }
        }
        self.jolt = (self.jolt - dt as f32 * 2.5).max(0.0);
    }

    /// Each link's transform: centred between its two points, along the line, a quarter turn
    /// from the one before, shrinking to nothing as a loose chain fades.
    pub fn link_poses(&self) -> Vec<(DVec3, Quat, Vec3)> {
        let n = self.points.len() - 1;
        let fade = self.loose.map_or(1.0, |t| {
            1.0 - ((t - FALL_SECS * 0.55) / (FALL_SECS * 0.45)).clamp(0.0, 1.0)
        });
        let span = (self.head() - self.a_at)
            .as_vec3()
            .normalize_or(Vec3::NEG_Y);
        let side = span.cross(Vec3::Y).normalize_or(Vec3::X);
        (0..n)
            .map(|i| {
                let (a, b) = (self.points[i], self.points[i + 1]);
                let dir = (b - a).as_vec3().normalize_or(span);
                let normal = if i % 2 == 0 {
                    side
                } else {
                    dir.cross(side).normalize_or(Vec3::Y)
                };
                let normal = (normal - dir * normal.dot(dir)).normalize_or(Vec3::Y);
                let z = dir.cross(normal);
                let r = LINK_M * 0.55 * fade;
                // A link reaches through the one before and the one after: as long inside as
                // the spacing, never shorter than its own shape.
                let along = (r * 1.35).max((b - a).length() as f32 / 1.52 * fade);
                (
                    a.lerp(b, 0.5),
                    Quat::from_mat3(&Mat3::from_cols(dir, normal, z)),
                    Vec3::new(along, r, r),
                )
            })
            .collect()
    }

    /// Done: let go, fallen and faded.
    pub fn gone(&self) -> bool {
        self.loose.is_some_and(|t| t >= FALL_SECS)
    }

    /// Let go of both ends: it drops where it hangs.
    pub fn let_go(&mut self) {
        if self.loose.is_none() {
            self.loose = Some(0.0);
        }
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The chains' iron: dark, oily steel, faintly hot in the seams.
pub fn iron() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::RustySteel)
        .with_tint(0.34, 0.31, 0.3)
        .with_metallic(0.9)
        .with_roughness(0.42)
        .with_emissive(EMBER, LINK_GLOW)
}

/// Put a chain into the world: its links and itself.
pub fn spawn_chain(commands: &mut Commands, mut chain: Chain) -> Entity {
    let material = iron();
    let n = chain.points.len() - 1;
    chain.links = (0..n)
        .map(|_| {
            commands
                .spawn((
                    Name::new("Chain link"),
                    Transform::from_translation(chain.a_at).with_scale(Vec3::ZERO),
                    MeshRenderer::new(
                        MeshRef::Shape(Shape::Torus {
                            radius: 1.0,
                            tube: 0.24,
                            segments: 10,
                        }),
                        MaterialRef::Inline(material),
                    ),
                    ChainLink,
                ))
                .id()
        })
        .collect();
    commands.spawn((Name::new("Pact chain"), chain)).id()
}

/// A drawn link of a [`Chain`].
#[derive(Component, Debug, Clone, Copy)]
pub struct ChainLink;

// ------------------------------------------------------------------------------- owners

/// A body held by chains from a sigil on the floor.
#[derive(Component, Debug, Clone)]
pub struct Binding {
    pub target: Entity,
    pub cast: CastInfo,
    pub age: f32,
    pub secs: f32,
    pub chains: Vec<Entity>,
    /// The sigil's rings.
    pub sigil: Vec<Entity>,
    pub center: DVec3,
}

impl Binding {
    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// A hook on a bolt, or reeling in what it caught.
#[derive(Component, Debug, Clone)]
pub struct Hook {
    pub cast: CastInfo,
    pub missile: Entity,
    pub chain: Entity,
    pub caught: Option<Entity>,
    /// Seconds since it bit (dragging) or since it missed (reeling back empty).
    pub reel: Option<f32>,
}

/// Bodies chained together that share their pain.
#[derive(Component, Debug, Clone)]
pub struct Bond {
    pub cast: CastInfo,
    pub members: Vec<Entity>,
    /// The chain between each member and the one before it.
    pub chains: Vec<Entity>,
    pub age: f32,
    pub secs: f32,
    pub share: f32,
}

impl Bond {
    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// A bolt that pays out a hook's chain.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HookRide {
    pub hook: Option<Entity>,
}

/// A binding to start (from a hit).
#[derive(Debug, Clone)]
pub struct NewBind {
    pub cast: CastInfo,
    pub target: Entity,
    pub secs: f32,
}

/// A hook that bit a body (from a hit).
#[derive(Debug, Clone)]
pub struct NewHook {
    pub cast: CastInfo,
    pub target: Entity,
}

/// A bond's chain hopped on to another body.
#[derive(Debug, Clone)]
pub struct BondHop {
    pub cast: CastInfo,
    pub to: Entity,
}

/// Damage a bond passes on, waiting to be dealt.
#[derive(Resource, Debug, Default)]
pub struct BondPain(pub Vec<DamageEvent>);

// ------------------------------------------------------------------------------- body points

/// Where on a body chains bite: its wrists and ankles (left, right, left, right).
pub fn limb_ends(
    target: Entity,
    minion: Option<&Minion>,
    pieces: &Query<&MinionPart>,
    pose: Option<&SkeletonPose>,
    height: f32,
) -> [End; 4] {
    if let Some(m) = minion {
        let want = crate::summon::limb_parts(m.kind);
        let find = |i: usize| {
            m.parts
                .iter()
                .copied()
                .find(|p| pieces.get(*p).is_ok_and(|mp| mp.index == i))
        };
        let mut out = [End::Body(target, Vec3::Y * height * 0.5); 4];
        for (o, &i) in out.iter_mut().zip(want.iter()) {
            if let Some(p) = find(i) {
                *o = End::Piece(p);
            }
        }
        return out;
    }
    if let Some(p) = pose {
        let bones = ["hand_l", "hand_r", "foot_l", "foot_r"].map(|n| p.index_of(n));
        if bones.iter().all(Option::is_some) {
            return bones.map(|b| End::Bone(target, b.unwrap_or(0)));
        }
    }
    let h = height / 1.8;
    [
        End::Body(target, Vec3::new(-0.3, 0.95, 0.0) * h),
        End::Body(target, Vec3::new(0.3, 0.95, 0.0) * h),
        End::Body(target, Vec3::new(-0.13, 0.08, 0.0) * h),
        End::Body(target, Vec3::new(0.13, 0.08, 0.0) * h),
    ]
}

/// A body's chest (where a bond's chains hold), from its feet.
pub fn chest_of(minion: Option<&Minion>) -> Vec3 {
    match minion {
        Some(m) => Vec3::Y * crate::summon::chest_height(m.kind, m.size),
        None => Vec3::Y * 1.2,
    }
}

/// Everything an [`End`] may need to be found.
#[derive(bevy_ecs::system::SystemParam)]
pub struct Ends<'w, 's> {
    tfs: Query<'w, 's, &'static Transform, (Without<ChainLink>, Without<SigilRing>)>,
    parents: Query<'w, 's, &'static ChildOf>,
    spells: Query<'w, 's, &'static SpellPos>,
    poses: Query<'w, 's, (&'static GlobalTransform, &'static SkeletonPose)>,
    casters: Query<'w, 's, (&'static GlobalTransform, &'static MagicCaster)>,
}

impl Ends<'_, '_> {
    /// Where an end is now, or None when what held it is gone.
    pub fn at(&self, end: End) -> Option<DVec3> {
        match end {
            End::At(p) => Some(p),
            End::Body(e, off) => {
                if let Ok(sp) = self.spells.get(e) {
                    return Some(sp.pos + off.as_dvec3());
                }
                let tf = self.tfs.get(e).ok()?;
                Some(tf.translation + (tf.rotation * (off * tf.scale)).as_dvec3())
            }
            End::Bone(e, i) => {
                let (g, p) = self.poses.get(e).ok()?;
                p.bone_world(g, i).map(|b| b.0)
            }
            End::Piece(p) => {
                let local = self.tfs.get(p).ok()?;
                let root = self.parents.get(p).ok()?.parent();
                let rt = self.tfs.get(root).ok()?;
                Some(
                    rt.translation
                        + (rt.rotation * (local.translation.as_vec3() * rt.scale)).as_dvec3(),
                )
            }
            End::Hand(c) => {
                let (g, mc) = self.casters.get(c).ok()?;
                Some(hand_of(g, mc))
            }
        }
    }
}

// ------------------------------------------------------------------------------- systems

/// Start the bindings, hooks and bonds the hits asked for.
#[allow(clippy::too_many_arguments)]
pub fn spawn_pacts(
    mut queue: ResMut<MagicQueue>,
    queries: Res<PhysicsQueries>,
    ends: Ends,
    bodies: Query<(Option<&Minion>, Option<&SkeletonPose>, &GlobalTransform)>,
    pieces: Query<&MinionPart>,
    mut bindings: Query<&mut Binding>,
    mut hooks: Query<&mut Hook>,
    mut bonds: Query<(Entity, &mut Bond)>,
    mut chains: Query<&mut Chain>,
    rides: Query<(Entity, &Missile, &HookRide)>,
    mut commands: Commands,
) {
    // Hooks ride their bolts from the moment they fly.
    for (m_e, m, ride) in &rides {
        if ride.hook.is_some() || m.spent.is_some() {
            continue;
        }
        let Some(caster) = m.cast.caster else {
            continue;
        };
        let Some(from) = ends.at(End::Hand(caster)) else {
            continue;
        };
        let to = ends.at(End::Body(m_e, Vec3::ZERO)).unwrap_or(from);
        let mut c = Chain::new(
            End::Hand(caster),
            End::Body(m_e, Vec3::ZERO),
            from,
            to,
            Chain::links_for(m.cast.spell.carrier.range.min(30.0)),
            m.cast.seed as u64,
        );
        // It pays out behind the bolt: always as long as the way to it.
        c.shoot = 0.0;
        c.slack = 1.0;
        let chain = spawn_chain(&mut commands, c);
        let hook = commands
            .spawn((
                Name::new("Chain hook"),
                Hook {
                    cast: m.cast.clone(),
                    missile: m_e,
                    chain,
                    caught: None,
                    reel: None,
                },
            ))
            .id();
        commands.entity(m_e).insert(HookRide { hook: Some(hook) });
    }
    for nh in std::mem::take(&mut queue.hooks) {
        // The hook on the bolt that bit: it holds the body now and starts to drag.
        let Some(mut h) = hooks.iter_mut().find(|h| {
            h.caught.is_none()
                && h.reel.is_none()
                && h.cast.seed == nh.cast.seed
                && h.cast.caster == nh.cast.caster
        }) else {
            continue;
        };
        h.caught = Some(nh.target);
        h.reel = Some(0.0);
        let chest = chest_of(bodies.get(nh.target).ok().and_then(|b| b.0));
        if let Ok(mut c) = chains.get_mut(h.chain) {
            c.b = End::Body(nh.target, chest);
            c.slack = 0.98;
        }
    }
    for nb in std::mem::take(&mut queue.binds) {
        let Ok((minion, pose, g)) = bodies.get(nb.target) else {
            continue;
        };
        // A body already bound is held longer, not bound twice.
        if let Some(mut b) = bindings.iter_mut().find(|b| b.target == nb.target) {
            b.secs = b.secs.max(b.age + nb.secs);
            continue;
        }
        let feet = g.translation();
        let center = ground_below(
            &queries,
            feet + DVec3::Y * 0.5,
            &QueryFilter::excluding(nb.target),
        );
        let height = minion.map_or(1.8, |m| crate::summon::body_height(m.kind, m.size));
        let limbs = limb_ends(nb.target, minion, &pieces, pose, height);
        let yaw = g.rotation() * Vec3::NEG_Z;
        let fwd = Vec3::new(yaw.x, 0.0, yaw.z).normalize_or(Vec3::NEG_Z);
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        // Wrists from the sigil's front corners, ankles from its back ones: the arms are hauled
        // out and down, the legs held apart.
        let spots = [
            (-right + fwd * 0.6).normalize(),
            (right + fwd * 0.6).normalize(),
            (-right - fwd).normalize(),
            (right - fwd).normalize(),
        ];
        let r = SIGIL_M * (height / 1.8).clamp(0.7, 1.8);
        let seed = nb.cast.seed as u64 ^ nb.target.to_bits();
        let chains_made: Vec<Entity> = limbs
            .iter()
            .zip(spots)
            .enumerate()
            .map(|(i, (&end, spot))| {
                let a_at = center + (spot * r).as_dvec3();
                let b_at = ends.at(end).unwrap_or(feet + DVec3::Y);
                let mut c = Chain::new(
                    End::At(a_at),
                    end,
                    a_at,
                    b_at,
                    Chain::links_for(a_at.distance(b_at) as f32),
                    seed.wrapping_add(i as u64 * 7919),
                );
                c.slack = 1.3;
                c.floor = Some(center.y);
                // They burst out one after another.
                c.age = -0.05 * i as f32;
                spawn_chain(&mut commands, c)
            })
            .collect();
        let sigil = spawn_sigil(&mut commands, center, r, nb.target.to_bits());
        queue.looks.push(LookCmd::Burst {
            at: center + DVec3::Y * 0.1,
            up: Vec3::Y,
            look: "fire".into(),
            role: Role::Impact,
            scale: 0.5 * r,
            color: EMBER,
        });
        queue.looks.push(LookCmd::Effect {
            name: "sparks",
            at: center + DVec3::Y * 0.2,
            up: Vec3::Y,
            scale: r,
        });
        commands.spawn((
            Name::new("Binding"),
            Transform::from_translation(center + DVec3::Y * 0.4),
            PointLight {
                color: EMBER,
                intensity_lm: 1800.0,
                range: 4.5,
                radius: 0.3,
            },
            Binding {
                target: nb.target,
                cast: nb.cast,
                age: 0.0,
                secs: nb.secs,
                chains: chains_made,
                sigil,
                center,
            },
        ));
    }
    for hop in std::mem::take(&mut queue.bond_hops) {
        let chest = |e: Entity| chest_of(bodies.get(e).ok().and_then(|b| b.0));
        match bonds.iter_mut().find(|(_, b)| {
            b.cast.seed == hop.cast.seed && b.cast.caster == hop.cast.caster && b.age < b.secs
        }) {
            Some((_, mut b)) => {
                if b.members.contains(&hop.to) {
                    continue;
                }
                let Some(&last) = b.members.last() else {
                    continue;
                };
                let (a, z) = (
                    End::Body(last, chest(last)),
                    End::Body(hop.to, chest(hop.to)),
                );
                let (Some(a_at), Some(b_at)) = (ends.at(a), ends.at(z)) else {
                    continue;
                };
                let d = a_at.distance(b_at) as f32;
                let mut c = Chain::new(
                    a,
                    z,
                    a_at,
                    b_at,
                    Chain::links_for(d * 1.2),
                    hop.cast.seed as u64 ^ hop.to.to_bits(),
                );
                // A bond's chain is a little longer than the way between them when made: it
                // sags while they stand, pulls taut as they part.
                c.length = Some(d * 1.12 + 0.6);
                c.shoot = 0.18;
                let chain = spawn_chain(&mut commands, c);
                b.members.push(hop.to);
                b.chains.push(chain);
            }
            None => {
                let secs = hop.cast.spell.payload.bond_secs;
                commands.spawn((
                    Name::new("Bond"),
                    Bond {
                        cast: hop.cast,
                        members: vec![hop.to],
                        chains: Vec::new(),
                        age: 0.0,
                        secs,
                        share: BOND_SHARE,
                    },
                ));
            }
        }
    }
}

/// A sigil on the floor: two rings of fire.
fn spawn_sigil(commands: &mut Commands, center: DVec3, r: f32, seed: u64) -> Vec<Entity> {
    let glow = StandardMaterial::default()
        .with_tint(EMBER[0], EMBER[1], EMBER[2])
        .with_emissive(EMBER, SIGIL_GLOW);
    let spin = Quat::from_rotation_y(roll(seed, 11) * std::f32::consts::TAU);
    [(1.0, 0.035), (0.72, 0.025)]
        .iter()
        .map(|&(k, tube)| {
            commands
                .spawn((
                    Name::new("Chain sigil"),
                    Transform::from_translation(center + DVec3::Y * 0.02)
                        .with_rotation(spin)
                        .with_scale(Vec3::new(0.0, 1.0, 0.0)),
                    MeshRenderer::new(
                        MeshRef::Shape(Shape::Torus {
                            radius: 1.0,
                            tube: tube / (r * k),
                            segments: 48,
                        }),
                        MaterialRef::Inline(glow),
                    )
                    .without_shadows(),
                    SigilRing { radius: r * k },
                ))
                .id()
        })
        .collect()
}

/// A ring of a binding's sigil.
#[derive(Component, Debug, Clone, Copy)]
pub struct SigilRing {
    pub radius: f32,
}

/// Bindings hold, then let go; hooks drag; bonds last; every chain swings and is drawn.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_pacts(
    time: Res<Time>,
    ends: Ends,
    mut bindings: Query<(Entity, &mut Binding)>,
    mut hooks: Query<(Entity, &mut Hook)>,
    mut bonds: Query<(Entity, &mut Bond)>,
    mut chains: Query<(Entity, &mut Chain)>,
    mut links: Query<&mut Transform, (With<ChainLink>, Without<SigilRing>)>,
    mut rings: Query<(&SigilRing, &mut Transform), Without<ChainLink>>,
    missiles: Query<&Missile>,
    vitals: Query<&Vitality>,
    statuses: Query<&engine_world::Statuses>,
    mut minions: Query<&mut Minion>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let alive = |e: Entity| vitals.get(e).map_or(true, |v| !v.down());
    for (e, mut b) in &mut bindings {
        b.age += dt;
        let held = alive(b.target) && ends.at(End::Body(b.target, Vec3::ZERO)).is_some();
        let done = b.age >= b.secs || !held;
        // The sigil opens out, burns, and closes when the chains let go.
        let open = smooth(b.age / 0.2);
        let close = if done { 0.0 } else { 1.0 };
        for &r in &b.sigil {
            if let Ok((ring, mut tf)) = rings.get_mut(r) {
                let k = ring.radius * open * close;
                tf.scale = Vec3::new(k, 1.0, k);
                tf.rotation *= Quat::from_rotation_y(dt * 0.6);
            }
        }
        // Held bodies strain at their chains now and then.
        let straining = statuses.get(b.target).is_ok_and(|s| s.rooted);
        for &c in &b.chains {
            if let Ok((_, mut ch)) = chains.get_mut(c) {
                if done {
                    ch.let_go();
                } else {
                    let t = (ch.age - ch.shoot).max(0.0);
                    ch.slack = 1.3 - 0.42 * smooth(t / TAUT_SECS);
                    let beat = (b.age * 1.3 + roll(ch.seed, 4) * 3.0).fract();
                    if straining && beat < dt * 1.3 {
                        ch.jolt = 1.0;
                    }
                }
            }
        }
        if done {
            for &r in &b.sigil {
                commands.entity(r).despawn();
            }
            commands.entity(e).despawn();
        }
    }
    for (e, mut h) in &mut hooks {
        // Spent a beat ago and nothing bit: a hit lands in the frame after the bolt is spent.
        let landed = missiles
            .get(h.missile)
            .map_or(true, |m| m.spent.is_some_and(|s| s > 0.05));
        let Ok((_, mut ch)) = chains.get_mut(h.chain) else {
            commands.entity(e).despawn();
            continue;
        };
        match (h.caught, h.reel) {
            (Some(t), Some(r)) => {
                let r = r + dt;
                h.reel = Some(r);
                // Drag a creature magic moves itself; anything else the pull's impulse throws.
                if let (Some(c), Ok(mut m)) = (h.cast.caster, minions.get_mut(t))
                    && let Some(to) = ends.at(End::Body(c, Vec3::ZERO))
                {
                    let flat = DVec3::new(to.x - m.pos.x, 0.0, to.z - m.pos.z);
                    let d = flat.length();
                    if d > DRAG_STOP_M as f64 {
                        let left = (REEL_SECS - r).max(dt);
                        let step = ((d - DRAG_STOP_M as f64) * (dt / left).min(1.0) as f64)
                            .min(14.0 * dt as f64);
                        m.pos += flat / d * step;
                    }
                }
                if r >= REEL_SECS || !alive(t) {
                    ch.let_go();
                    commands.entity(e).despawn();
                }
            }
            (None, None) if landed => {
                // A miss: the chain reels back in empty.
                h.reel = Some(0.0);
                let from = ch.b_at;
                ch.reel = Some((0.0, RETRACT_SECS, from));
                ch.b = End::At(from);
            }
            (None, Some(r)) => {
                let r = r + dt;
                h.reel = Some(r);
                if let Some((t, _, _)) = &mut ch.reel {
                    *t = r;
                }
                if r >= RETRACT_SECS {
                    for &l in &ch.links {
                        commands.entity(l).despawn();
                    }
                    commands.entity(h.chain).despawn();
                    commands.entity(e).despawn();
                }
            }
            _ => {}
        }
    }
    for (e, mut b) in &mut bonds {
        b.age += dt;
        let done = b.age >= b.secs;
        let members = b.members.clone();
        for (i, &c) in b.chains.iter().enumerate() {
            if let Ok((_, mut ch)) = chains.get_mut(c)
                && (done || !alive(members[i]) || !alive(members[i + 1]))
            {
                ch.let_go();
            }
        }
        if done {
            commands.entity(e).despawn();
        }
    }
    // Every chain: find its ends, swing, draw its links.
    let g = 9.81;
    for (e, mut ch) in &mut chains {
        ch.age += dt;
        if let Some(t) = ch.loose.as_mut() {
            *t += dt;
        }
        if ch.loose.is_none() {
            match (ends.at(ch.a), ends.at(ch.b)) {
                (Some(a), Some(b)) => {
                    ch.a_at = a;
                    ch.b_at = b;
                }
                _ => ch.let_go(),
            }
        }
        if ch.age < 0.0 {
            continue;
        }
        ch.step(dt, g);
        if ch.gone() {
            for &l in &ch.links {
                commands.entity(l).despawn();
            }
            commands.entity(e).despawn();
            continue;
        }
        for (&l, (at, rot, scale)) in ch.links.iter().zip(ch.link_poses()) {
            if let Ok(mut tf) = links.get_mut(l) {
                tf.translation = at;
                tf.rotation = rot;
                tf.scale = scale;
            }
        }
        // A chain that just bit throws sparks off the body.
        if ch.loose.is_none() && ch.age >= ch.shoot && ch.age - dt < ch.shoot && ch.reel.is_none() {
            queue.looks.push(LookCmd::Effect {
                name: "sparks",
                at: ch.b_at,
                up: (ch.b_at - ch.a_at).as_vec3().normalize_or(Vec3::Y),
                scale: 0.4,
            });
        }
    }
}

/// A bonded body hurt passes a share of it on to the others.
pub fn share_bonds(
    mut events: MessageReader<DamageEvent>,
    bonds: Query<(Entity, &Bond)>,
    tfs: Query<&GlobalTransform>,
    vitals: Query<&Vitality>,
    mut pain: ResMut<BondPain>,
    mut queue: ResMut<MagicQueue>,
) {
    if bonds.is_empty() {
        events.clear();
        return;
    }
    let bonded: HashMap<Entity, Entity> = bonds
        .iter()
        .flat_map(|(be, b)| b.members.iter().map(move |&m| (m, be)))
        .collect();
    for d in events.read() {
        // What a bond passed on isn't passed on again.
        if d.amount <= 0.0 || d.source.is_some_and(|s| bonds.contains(s)) {
            continue;
        }
        let Some(&be) = bonded.get(&d.target) else {
            continue;
        };
        let Ok((_, b)) = bonds.get(be) else { continue };
        for &o in &b.members {
            if o == d.target || vitals.get(o).is_ok_and(|v| v.down()) {
                continue;
            }
            let Ok(g) = tfs.get(o) else { continue };
            let at = g.translation() + DVec3::Y;
            pain.0.push(DamageEvent {
                target: o,
                amount: d.amount * b.share,
                kind: d.kind,
                point: at,
                impulse: Vec3::ZERO,
                source: Some(be),
            });
            queue.looks.push(LookCmd::Burst {
                at,
                up: Vec3::Y,
                look: "blood".into(),
                role: Role::Impact,
                scale: 0.35,
                color: EMBER,
            });
        }
    }
}

/// Deal what bonds passed on.
pub fn pay_bonds(mut pain: ResMut<BondPain>, mut out: MessageWriter<DamageEvent>) {
    for d in pain.0.drain(..) {
        out.write(d);
    }
}

/// Chains move with the world's origin.
pub fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut chains: Query<&mut Chain>,
    mut bindings: Query<&mut Binding>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    for mut c in &mut chains {
        if let Some(f) = c.floor {
            c.floor = Some(s.point(DVec3::new(c.a_at.x, f, c.a_at.z)).y);
        }
        let c = &mut *c;
        for p in c.points.iter_mut().chain(c.prev.iter_mut()) {
            *p = s.point(*p);
        }
        c.a_at = s.point(c.a_at);
        c.b_at = s.point(c.b_at);
        if let End::At(p) = &mut c.a {
            *p = s.point(*p);
        }
        if let End::At(p) = &mut c.b {
            *p = s.point(*p);
        }
        if let Some((_, _, from)) = &mut c.reel {
            *from = s.point(*from);
        }
    }
    for mut b in &mut bindings {
        b.center = s.point(b.center);
    }
}

/// Every pact chain goes at once (the hall's restart): bindings and their sigils, hooks, bonds
/// and every drawn chain and link.
pub fn clear(world: &mut World) {
    let mut gone: Vec<Entity> = Vec::new();
    gone.extend(world.query_filtered::<Entity, With<Chain>>().iter(world));
    gone.extend(
        world
            .query_filtered::<Entity, With<ChainLink>>()
            .iter(world),
    );
    gone.extend(world.query_filtered::<Entity, With<Binding>>().iter(world));
    gone.extend(
        world
            .query_filtered::<Entity, With<SigilRing>>()
            .iter(world),
    );
    gone.extend(world.query_filtered::<Entity, With<Hook>>().iter(world));
    gone.extend(world.query_filtered::<Entity, With<Bond>>().iter(world));
    for e in gone {
        if let Ok(ec) = world.get_entity_mut(e) {
            ec.despawn();
        }
    }
    if let Some(mut pain) = world.get_resource_mut::<BondPain>() {
        pain.0.clear();
    }
}

pub fn register(app: &mut App) {
    app.init_resource::<BondPain>().register_api(
        "magic.chains",
        "Pact chains now: bindings (whom they hold, seconds left, chains), hooks (whose, what \
         they caught) and bonds (who shares whose pain, seconds left).",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Binding)>();
            let binds: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, b)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "target": b.target.to_bits(),
                        "left_s": b.left(),
                        "chains": b.chains.len(),
                        "at": [b.center.x, b.center.y, b.center.z],
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Hook)>();
            let hooks: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, h)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "caster": h.cast.caster.map(|c| c.to_bits()),
                        "caught": h.caught.map(|c| c.to_bits()),
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Bond)>();
            let bonds: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, b)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "members": b.members.iter().map(|m| m.to_bits()).collect::<Vec<_>>(),
                        "share": b.share,
                        "left_s": b.left(),
                    })
                })
                .collect();
            Ok(serde_json::json!({ "bindings": binds, "hooks": hooks, "bonds": bonds }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(a: DVec3, b: DVec3) -> Chain {
        let mut c = Chain::new(End::At(a), End::At(b), a, b, 20, 7);
        c.age = 1.0;
        c
    }

    #[test]
    fn a_slack_chain_sags_and_a_taut_one_runs_straight() {
        let (a, b) = (DVec3::new(0.0, 2.0, 0.0), DVec3::new(2.0, 2.0, 0.0));
        let mut slack = chain(a, b);
        slack.slack = 1.4;
        let mut taut = chain(a, b);
        taut.slack = 0.9;
        for _ in 0..240 {
            slack.step(1.0 / 60.0, 9.81);
            taut.step(1.0 / 60.0, 9.81);
        }
        let low = |c: &Chain| c.points.iter().map(|p| p.y).fold(f64::MAX, f64::min);
        assert!(low(&slack) < 1.6, "{}", low(&slack));
        assert!(low(&taut) > 1.97, "{}", low(&taut));
        // Its ends stay where they are held.
        assert!(slack.points[0].distance(a) < 1e-9);
        assert!(slack.points[20].distance(b) < 1e-9);
    }

    #[test]
    fn a_chain_shoots_out_then_holds_its_end() {
        let (a, b) = (DVec3::ZERO, DVec3::new(0.0, 1.5, 1.0));
        let mut c = Chain::new(End::At(a), End::At(b), a, b, 14, 3);
        c.step(1.0 / 60.0, 9.81);
        let early = c.points[14];
        assert!(early.distance(a) < 0.2, "{early}");
        for _ in 0..30 {
            c.age += 1.0 / 60.0;
            c.step(1.0 / 60.0, 9.81);
        }
        assert!(c.points[14].distance(b) < 1e-6);
    }

    #[test]
    fn a_loose_chain_falls_to_the_floor_and_fades() {
        let (a, b) = (DVec3::new(0.0, 0.0, 0.0), DVec3::new(0.0, 1.8, 1.0));
        let mut c = chain(a, b);
        c.floor = Some(0.0);
        c.let_go();
        for _ in 0..90 {
            c.loose = c.loose.map(|t| t + 1.0 / 60.0);
            c.step(1.0 / 60.0, 9.81);
        }
        let high = c.points.iter().map(|p| p.y).fold(f64::MIN, f64::max);
        assert!(high < 0.4, "{high}");
        assert!(c.gone());
        assert!(c.link_poses().iter().all(|l| l.2.length() < 1e-4));
    }

    #[test]
    fn links_turn_a_quarter_turn_one_from_the_next() {
        let (a, b) = (DVec3::ZERO, DVec3::new(3.0, 0.0, 0.0));
        let mut c = chain(a, b);
        c.slack = 1.0;
        for _ in 0..60 {
            c.step(1.0 / 60.0, 0.0);
        }
        let poses = c.link_poses();
        let n0 = poses[4].1 * Vec3::Y;
        let n1 = poses[5].1 * Vec3::Y;
        assert!(n0.dot(n1).abs() < 0.1, "{n0} {n1}");
        // Each runs along the chain.
        assert!((poses[4].1 * Vec3::X).dot(Vec3::X) > 0.99);
    }
}
