//! Living wood (round-17-plan.md): Vitalism's Wilds, the effect `wilds`. The cast type decides
//! what grows:
//!
//! * **Entangle** (Area, Rune, Cone, Rain, Wave, Touch): under each foe the spell lands on, five
//!   bark roots break out of heaved earth, tip first, and coil up its legs and round its body.
//!   The body is held (`Statuses::rooted`) for [`crate::compile::Payload::roots_secs`], then the
//!   roots slide back down into the ground (Baldur's Gate 3's Entangle, Diablo IV's Poison
//!   Creeper, Freya's Entangled Grip). An area also sends up writhing free roots across its
//!   ground, so it reads where nobody stands.
//! * **Seed** (Bolt, Lob): a seed that bursts into roots where it lands, holding what it struck
//!   and anyone within [`SEED_M`] (an Impact stage of an entangle, compile.rs).
//! * **Thorn hedge** (Wall): interlaced thorny brambles along the wall's line. Bodies push through
//!   it, but each tick it scratches them and snags them for [`HEDGE_SNAG`] s, so they crawl
//!   (Baldur's Gate 3's Wall of Thorns).
//! * **Bark skin** (Self): bark plates grow over the caster from the feet up and take
//!   [`BARK_SHARE`] off the harm it takes (less against fire), then crack off and fall.
//! * **Treant** (Summon): a walking tree that guards and smashes ([`build_treant`]; summon.rs
//!   walks it as it walks a golem). Its slam roots what it strikes.
//!
//! Fire burns roots: a fire hit on a body the roots hold frees it, and its roots glow like embers
//! and crumble. A fire spell through a hedge sets it alight: the fire runs along it both ways
//! and it is gone.
//!
//! A root is a chain of tapered bark segments (a capsule each) along a path from wilds_art.rs,
//! with thorns and leaves riding its segments. It grows by sliding out along its path, tip
//! first, so it seems to push out of the earth and wind round the leg; it lets go by sliding
//! back in.

use std::f32::consts::{PI, TAU};

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Mat3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    Image, MaterialPreset, MaterialRef, Mesh, MeshRef, MeshRenderer, Shape, StandardMaterial,
};
use engine_world::animation::SkeletonPose;
use engine_world::physics::PhysicsQueries;

use crate::caster::Vitality;
use crate::chains::{End, Ends, limb_ends};
use crate::compile::Spell;
use crate::design::CastType;
use crate::looks::{LookCmd, Role};
use crate::runtime::{CastInfo, MagicQueue, roll};
use crate::summon::{Minion, MinionKind, MinionPart};
use crate::wilds_art as art;

// ------------------------------------------------------------------------------- numbers

/// Living wood's colour for its dust and clods (the stone look in this brown), linear RGB.
pub const SOIL: [f32; 3] = [0.3, 0.2, 0.11];
/// A root's segments are this long, m.
pub const SEG_M: f32 = 0.13;
/// A hedge's brambles have longer segments, m (there are many of them).
pub const HEDGE_SEG_M: f32 = 0.22;
/// Brambles start this far apart along a hedge, in each of its two rows, m.
pub const HEDGE_STEP: f32 = 0.28;
/// How fast a root slides out of the ground, and back in, m/s.
pub const GROW_M_S: f32 = 4.5;
pub const RETRACT_M_S: f32 = 3.0;
/// Seconds burnt roots glow and crumble.
pub const BURN_SECS: f32 = 0.9;
/// Seconds of each half-second tick a hedge's thorns hold what pushes through them.
pub const HEDGE_SNAG: f32 = 0.35;
/// A hedge's height, m.
pub const HEDGE_M: f32 = 1.8;
/// The share of harm bark skin takes off, and off fire's.
pub const BARK_SHARE: f32 = 0.4;
pub const BARK_FIRE_SHARE: f32 = 0.15;
/// How far round where a seed lands its roots hold, m.
pub const SEED_M: f32 = 2.0;
/// Roots up each leg of a body held.
pub const LEG_ROOTS: u32 = 3;
/// Seconds free roots stand before they sink back.
pub const PATCH_HOLD: f32 = 1.8;
/// A treant is this much taller than a golem of its size.
pub const TREANT_SCALE: f32 = 1.55;
/// A treant's crown: boughs, and leaf clusters round each.
pub const CROWN_BOUGHS: u64 = 9;
pub const CROWN_LEAVES: u64 = 8;
/// A treant's eyes: a green glow, nits (a small glowing mesh in the daylit hall).
pub const TREANT_EYES: [f32; 3] = [0.45, 1.0, 0.3];
pub const TREANT_EYE_GLOW: f32 = 9_000.0;

/// Cast types whose wilds hold what they land on with roots.
pub fn roots_hold(cast: CastType) -> bool {
    matches!(
        cast,
        CastType::Area
            | CastType::Rune
            | CastType::Cone
            | CastType::Rain
            | CastType::Wave
            | CastType::Touch
            | CastType::Bolt
            | CastType::Lob
    )
}

// ------------------------------------------------------------------------------- materials

/// Root bark: the bark preset, a dark living brown that stands out on pale bone and stone,
/// mapped in the world so thin segments keep it.
pub fn bark() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::Bark)
        .with_triplanar_world()
        .with_texture_scale(0.5)
        .with_tint(0.5, 0.4, 0.3)
}

/// Bark that burns: near black, glowing like embers.
pub fn charred() -> StandardMaterial {
    StandardMaterial::color(0.05, 0.04, 0.035)
        .with_roughness(0.95)
        .with_emissive([1.0, 0.28, 0.05], 1_500.0)
}

pub fn thorn() -> StandardMaterial {
    StandardMaterial::color(0.16, 0.11, 0.07).with_roughness(0.55)
}

/// The earth roots heave up as they break out.
pub fn soil() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::ForestSoil)
        .with_triplanar_world()
        .with_texture_scale(0.8)
}

/// Bark skin's plates and a treant's body: paler, older bark.
pub fn old_bark() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::Bark)
        .with_triplanar_world()
        .with_texture_scale(0.6)
        .with_tint(0.85, 0.78, 0.66)
}

fn leaf_material(picture: Handle<Image>) -> StandardMaterial {
    StandardMaterial::default()
        .with_base_color_texture(picture)
        .with_alpha_mask(0.5)
        .with_subsurface(0.35)
        .with_wind_sway(0.03)
        .with_roughness(0.6)
}

fn capsule() -> MeshRef {
    // A unit capsule 2 long with a radius of 0.5, scaled to each segment.
    MeshRef::Shape(Shape::Capsule {
        radius: 0.5,
        height: 2.0,
        segments: 10,
    })
}

fn cone() -> MeshRef {
    MeshRef::Shape(Shape::Cone {
        radius: 0.5,
        height: 1.0,
        segments: 8,
    })
}

/// The leaf picture and its card, made once.
#[derive(Resource, Default)]
pub struct WildsMeshes {
    card: Option<Handle<Mesh>>,
    leaf: Option<Handle<Image>>,
}

impl WildsMeshes {
    /// The leaf picture and its card are made (lib.rs makes them at startup).
    pub fn ready(&self) -> bool {
        self.card.is_some() && self.leaf.is_some()
    }

    pub fn card(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        *self
            .card
            .get_or_insert_with(|| meshes.add(art::card_mesh()))
    }

    pub fn leaf(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        *self
            .leaf
            .get_or_insert_with(|| images.add(art::leaf_image()))
    }
}

// ------------------------------------------------------------------------------- the root

/// What rides a root's segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rider {
    /// A thorn out of the bark at this angle round the segment, this long against its girth.
    Thorn { phi: f32, length: f32 },
    /// A leaf card off the bark toward `dir` (in the segment's frame), turned `rot`, this big.
    Leaf { dir: Vec3, rot: Quat, size: f32 },
    /// A knob round the segment (a vertebra on a spine of bone), this big against its girth.
    Knob { size: f32, turn: f32 },
}

/// What a root is made of and what rides it: bark with thorns and leaves (the Wilds'), or bone
/// with spurs and knobs (bones.rs).
#[derive(Debug, Clone, Copy)]
pub struct RootLook {
    pub wood: StandardMaterial,
    pub spur: StandardMaterial,
    /// A thorn (spur) every this many segments (0: none), this long against the girth.
    pub spur_every: usize,
    pub spur_length: (f32, f32),
    /// A leaf every this many segments (0: none), about this big.
    pub leaf_every: usize,
    pub leaf_size: f32,
    /// A knob every this many segments (0: none), this big against the girth.
    pub knob_every: usize,
    pub knob: f32,
    /// Bone: grave dust flies where it breaks out, not earth.
    pub bone: bool,
}

impl RootLook {
    /// Bark with thorns every `thorn_every` segments and leaves every `leaf_every`.
    pub fn bark(thorn_every: usize, leaf_every: usize, leaf_size: f32) -> Self {
        Self {
            wood: bark(),
            spur: thorn(),
            spur_every: thorn_every,
            spur_length: (2.4, 3.8),
            leaf_every,
            leaf_size,
            knob_every: 0,
            knob: 0.0,
            bone: false,
        }
    }
}

/// A thorn or a leaf on a root.
#[derive(Debug, Clone, Copy)]
pub struct Extra {
    pub entity: Entity,
    pub seg: usize,
    pub rider: Rider,
}

/// One root: a path of points [`SEG_M`] apart in its own frame (its entity's), drawn as a
/// segment between each pair, sliding out along the path as it grows.
#[derive(Component, Debug, Clone)]
pub struct Root {
    pub pts: Vec<Vec3>,
    pub step: f32,
    pub segs: Vec<Entity>,
    pub extras: Vec<Extra>,
    /// Its girth at the ground, m.
    pub girth: f32,
    pub age: f32,
    /// Seconds before it breaks out.
    pub delay: f32,
    /// Seconds after it broke out that it lets go (None: it holds until told).
    pub until: Option<f32>,
    /// Seconds since it caught fire.
    pub burnt: Option<f32>,
    /// The length drawn last, m.
    pub shown: f32,
    /// Its dust has flown.
    pub burst: bool,
    /// Bone (bones.rs): grave dust where it breaks out.
    pub bone: bool,
}

impl Root {
    /// Its whole length, m.
    pub fn total(&self) -> f32 {
        (self.pts.len().saturating_sub(1)) as f32 * self.step
    }

    /// Seconds since it broke out (negative before).
    pub fn out(&self) -> f32 {
        self.age - self.delay
    }

    /// How much of it is out of the ground now, m.
    pub fn length_now(&self) -> f32 {
        let t = self.out();
        if t <= 0.0 {
            return 0.0;
        }
        let total = self.total();
        let grown = (t * GROW_M_S).min(total);
        match self.until {
            Some(u) if t > u => (grown.min(total) - (t - u) * RETRACT_M_S).max(0.0),
            _ => grown,
        }
    }

    /// Let go now: it slides back into the ground.
    pub fn let_go(&mut self) {
        if self.until.is_none() {
            self.until = Some(self.out().max(0.0));
        }
    }

    /// Gone back into the ground, or burnt away.
    pub fn gone(&self) -> bool {
        self.burnt.is_some_and(|b| b > BURN_SECS)
            || self.until.is_some_and(|u| self.out() > u) && self.length_now() <= 0.0
    }

    /// A point `a` metres along its path (before its start, on under the ground along its
    /// first segment).
    pub fn at(&self, a: f32) -> Vec3 {
        let n = self.pts.len();
        if n < 2 {
            return self.pts.first().copied().unwrap_or(Vec3::ZERO);
        }
        if a <= 0.0 {
            let d = (self.pts[1] - self.pts[0]).normalize_or(Vec3::Y);
            return self.pts[0] + d * a;
        }
        let f = a / self.step;
        let i = (f.floor() as usize).min(n - 2);
        self.pts[i].lerp(self.pts[i + 1], (f - i as f32).min(1.0))
    }

    /// Each segment's place, turn and scale (its unit capsule) with `length` out of the ground,
    /// and the turn and girth of each, or None where it is still under the ground.
    pub fn poses(&self, length: f32) -> Vec<Option<(Vec3, Quat, f32)>> {
        let total = self.total();
        let off = total - length;
        let n = self.pts.len().saturating_sub(1);
        (0..n)
            .map(|i| {
                let a = i as f32 * self.step - off;
                if a + self.step <= 0.0 || length <= 0.0 {
                    return None;
                }
                let (p, q) = (self.at(a), self.at(a + self.step));
                let r = art::girth(self.girth, (i as f32 + 0.5) / n as f32);
                let rot = Quat::from_rotation_arc(Vec3::Y, (q - p).normalize_or(Vec3::Y));
                Some(((p + q) * 0.5, rot, r))
            })
            .collect()
    }
}

/// A root's segment, thorn or leaf, or a clod of earth (moved only by wilds.rs).
#[derive(Component, Debug, Clone, Copy)]
pub struct WildPiece;

/// A leaf waiting for its card and picture.
#[derive(Component, Debug, Clone, Copy)]
pub struct Leaf;

/// Roots holding a body.
#[derive(Component, Debug, Clone)]
pub struct Tangle {
    pub target: Entity,
    pub roots: Vec<Entity>,
    pub age: f32,
    pub secs: f32,
    /// Seconds since it let go.
    pub done: Option<f32>,
}

impl Tangle {
    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// Free roots writhing up across an area.
#[derive(Component, Debug, Clone)]
pub struct Patch {
    pub roots: Vec<Entity>,
    pub age: f32,
    /// Seconds since its roots began to sink.
    pub sinking: Option<f32>,
}

/// A thorn hedge along a wall.
#[derive(Component, Debug, Clone)]
pub struct Hedge {
    /// The wall field it grows with.
    pub field: Entity,
    pub roots: Vec<Entity>,
    pub age: f32,
    pub done: Option<f32>,
}

/// Bark on a body.
#[derive(Component, Debug, Clone)]
pub struct BarkSkin {
    pub target: Entity,
    pub plates: Vec<(Entity, Plate)>,
    pub age: f32,
    pub secs: f32,
}

impl BarkSkin {
    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// Where a bark plate sits: on a bone (from it toward its child) of a skinned body, or on the
/// body's own trunk; `along` the bone (0..1), `around` it (radians from the body's front), `out`
/// from its axis (m), and its size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plate {
    pub bones: Option<(usize, usize)>,
    pub along: f32,
    pub around: f32,
    pub out: f32,
    pub size: Vec3,
    /// When it grows in, s after the skin starts (from the feet up).
    pub at: f32,
}

/// A plate cracked off, falling.
#[derive(Component, Debug, Clone, Copy)]
pub struct Falling {
    pub vel: Vec3,
    pub spin: Vec3,
    pub age: f32,
    pub floor: f64,
}

// ------------------------------------------------------------------------------- requests

/// Roots to grow round a body (from a hit).
#[derive(Debug, Clone)]
pub struct NewRoots {
    pub cast: CastInfo,
    pub target: Entity,
    pub secs: f32,
}

/// Free roots to send up across an area (from a field).
#[derive(Debug, Clone)]
pub struct NewPatch {
    pub cast: CastInfo,
    pub at: DVec3,
    pub radius: f32,
}

/// A hedge to grow along a wall field.
#[derive(Debug, Clone)]
pub struct NewHedge {
    pub cast: CastInfo,
    pub field: Entity,
    pub at: DVec3,
    pub along: Vec3,
    pub half: f32,
}

/// Bark to grow over a body (from a hit on itself).
#[derive(Debug, Clone)]
pub struct NewBark {
    pub cast: CastInfo,
    pub target: Entity,
    pub secs: f32,
}

// ------------------------------------------------------------------------------- building

/// A root's entity, its segments and riders (spawned hidden: it grows when its time comes).
#[allow(clippy::too_many_arguments)]
fn spawn_root(
    commands: &mut Commands,
    origin: DVec3,
    path: &[Vec3],
    step: f32,
    girth: f32,
    delay: f32,
    until: Option<f32>,
    seed: u64,
    thorn_every: usize,
    leaf_every: usize,
    leaf_size: f32,
) -> Entity {
    grow_root(
        commands,
        origin,
        path,
        step,
        girth,
        delay,
        until,
        seed,
        &RootLook::bark(thorn_every, leaf_every, leaf_size),
    )
}

/// A root of any make (bark or bone), spawned hidden: it grows when its time comes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn grow_root(
    commands: &mut Commands,
    origin: DVec3,
    path: &[Vec3],
    step: f32,
    girth: f32,
    delay: f32,
    until: Option<f32>,
    seed: u64,
    look: &RootLook,
) -> Entity {
    let (thorn_every, leaf_every, leaf_size) = (look.spur_every, look.leaf_every, look.leaf_size);
    let pts = art::resample(path, step);
    let root = commands
        .spawn((Name::new("Root"), Transform::from_translation(origin)))
        .id();
    let hidden = Transform::from_scale(Vec3::ZERO);
    let n = pts.len().saturating_sub(1);
    let mut segs = Vec::with_capacity(n);
    for _ in 0..n {
        segs.push(
            commands
                .spawn((
                    Name::new("Root segment"),
                    hidden,
                    MeshRenderer::new(capsule(), MaterialRef::Inline(look.wood)),
                    WildPiece,
                    ChildOf(root),
                ))
                .id(),
        );
    }
    let mut extras = Vec::new();
    for i in 0..n {
        let h = |salt: u64| roll(seed ^ (i as u64).wrapping_mul(0x9e37), salt);
        // No thorns or leaves on the first stretch (under the ground and just out of it).
        if i < 2 {
            continue;
        }
        if thorn_every > 0 && i % thorn_every == thorn_every / 2 {
            let e = commands
                .spawn((
                    Name::new("Thorn"),
                    hidden,
                    MeshRenderer::new(cone(), MaterialRef::Inline(look.spur)),
                    WildPiece,
                    ChildOf(root),
                ))
                .id();
            let (lo, hi) = look.spur_length;
            extras.push(Extra {
                entity: e,
                seg: i,
                rider: Rider::Thorn {
                    phi: h(1) * TAU,
                    length: lo + (hi - lo) * h(2),
                },
            });
        }
        if look.knob_every > 0 && i % look.knob_every == 0 {
            let e = commands
                .spawn((
                    Name::new("Knob"),
                    hidden,
                    MeshRenderer::new(
                        MeshRef::Shape(Shape::Rock {
                            size: [1.0, 0.8, 1.0],
                            seed: ((seed ^ i as u64) % 100_000) as u32,
                            detail: 1,
                        }),
                        MaterialRef::Inline(look.wood),
                    ),
                    WildPiece,
                    ChildOf(root),
                ))
                .id();
            extras.push(Extra {
                entity: e,
                seg: i,
                rider: Rider::Knob {
                    size: look.knob * (0.85 + 0.3 * h(9)),
                    turn: h(10) * TAU,
                },
            });
        }
        if leaf_every > 0 && i % leaf_every == leaf_every - 1 {
            let a = h(3) * TAU;
            let e = commands
                .spawn((
                    Name::new("Leaf"),
                    hidden,
                    MeshRenderer::new(
                        MeshRef::Shape(Shape::Quad { size: [1.0, 1.0] }),
                        MaterialRef::Inline(StandardMaterial::color(0.1, 0.25, 0.04)),
                    ),
                    WildPiece,
                    Leaf,
                    ChildOf(root),
                ))
                .id();
            extras.push(Extra {
                entity: e,
                seg: i,
                rider: Rider::Leaf {
                    dir: Vec3::new(a.cos(), 0.3 * (h(4) - 0.5), a.sin()).normalize(),
                    rot: Quat::from_euler(EulerRot::YXZ, h(5) * TAU, h(6) * PI, h(7) * TAU),
                    size: leaf_size * (0.75 + 0.5 * h(8)),
                },
            });
        }
    }
    commands.entity(root).insert(Root {
        pts,
        step,
        segs,
        extras,
        girth,
        age: 0.0,
        delay,
        until,
        burnt: None,
        shown: -1.0,
        burst: false,
        bone: look.bone,
    });
    root
}

/// A clod of heaved earth where a root breaks out (a child of `parent`, `at` in its frame).
fn spawn_clod(commands: &mut Commands, parent: Entity, at: Vec3, size: f32, seed: u64) -> Entity {
    heave(commands, parent, at, size, seed, soil())
}

/// A clod of heaved ground of `material` (a child of `parent`, `at` in its frame).
pub(crate) fn heave(
    commands: &mut Commands,
    parent: Entity,
    at: Vec3,
    size: f32,
    seed: u64,
    material: StandardMaterial,
) -> Entity {
    let s = Vec3::new(size, size * 0.32, size * (0.8 + 0.4 * roll(seed, 2)));
    commands
        .spawn((
            Name::new("Heaved earth"),
            Transform::from_translation((at + Vec3::Y * s.y * 0.2).as_dvec3())
                .with_rotation(Quat::from_rotation_y(roll(seed, 3) * TAU)),
            MeshRenderer::new(
                MeshRef::Shape(Shape::Rock {
                    size: s.to_array(),
                    seed: (seed % 100_000) as u32,
                    detail: 2,
                }),
                MaterialRef::Inline(material),
            ),
            WildPiece,
            ChildOf(parent),
        ))
        .id()
}

/// Draw a root with `length` of it out of the ground.
pub(crate) fn draw_root(
    r: &Root,
    length: f32,
    crumble: f32,
    tfs: &mut Query<&mut Transform, With<WildPiece>>,
) {
    let poses = r.poses(length);
    let squash = 1.0 - crumble;
    for (i, pose) in poses.iter().enumerate() {
        let Ok(mut tf) = tfs.get_mut(r.segs[i]) else {
            continue;
        };
        match *pose {
            Some((c, rot, g)) => {
                let w = 2.0 * g * squash;
                tf.translation = (c - Vec3::Y * 0.3 * crumble * crumble).as_dvec3();
                tf.rotation = rot;
                tf.scale = Vec3::new(w, r.step * 1.3 / 2.0 * squash.max(0.2), w);
            }
            None => tf.scale = Vec3::ZERO,
        }
    }
    for x in &r.extras {
        let Ok(mut tf) = tfs.get_mut(x.entity) else {
            continue;
        };
        let Some(Some((c, rot, g))) = poses.get(x.seg).copied() else {
            tf.scale = Vec3::ZERO;
            continue;
        };
        match x.rider {
            Rider::Thorn { phi, length } => {
                let out = Vec3::new(phi.cos(), 0.0, phi.sin());
                // Raked toward the tip, as a rose's.
                let dir = (out + Vec3::Y * 0.45).normalize();
                let h = g * length;
                tf.translation = (c + rot * (dir * (g * 0.8 + h * 0.5))).as_dvec3();
                tf.rotation = rot * Quat::from_rotation_arc(Vec3::Y, dir);
                tf.scale = Vec3::new(g * 0.9, h, g * 0.9) * squash;
            }
            Rider::Leaf {
                dir,
                rot: turn,
                size,
            } => {
                tf.translation = (c + rot * (dir * (g + size * 0.35))).as_dvec3();
                tf.rotation = rot * turn;
                tf.scale = Vec3::splat(size * squash);
            }
            Rider::Knob { size, turn } => {
                tf.translation = c.as_dvec3();
                tf.rotation = rot * Quat::from_rotation_y(turn);
                tf.scale = Vec3::splat(2.0 * g * size * squash);
            }
        }
    }
}

// ------------------------------------------------------------------------------- systems

/// Start the tangles, patches, hedges and bark the hits and fields asked for, and set fire to
/// roots that fire struck.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn spawn_wilds(
    mut queue: ResMut<MagicQueue>,
    queries: Res<PhysicsQueries>,
    ends: Ends,
    bodies: Query<(Option<&Minion>, Option<&SkeletonPose>, &GlobalTransform)>,
    pieces: Query<&MinionPart>,
    creatures: Query<(), Or<(With<Vitality>, With<Minion>, With<MinionPart>)>>,
    mut tangles: Query<&mut Tangle>,
    mut roots: Query<&mut Root>,
    mut skins: Query<&mut BarkSkin>,
    mut commands: Commands,
) {
    // The floor under a point, past whoever stands there (an area aimed at a body is placed on
    // top of it).
    let floor = |p: DVec3| {
        crate::cataclysm::ground_under(&queries, p + DVec3::Y * 2.0, 30.0, Vec::new(), &|e| {
            creatures.contains(e)
        })
        .map_or(p, |h| h.point)
    };
    for t in std::mem::take(&mut queue.root_burns) {
        for mut tg in tangles.iter_mut().filter(|tg| tg.target == t) {
            if tg.done.is_none() {
                tg.done = Some(0.0);
            }
            for &r in &tg.roots {
                if let Ok(mut r) = roots.get_mut(r)
                    && r.burnt.is_none()
                {
                    r.burnt = Some(0.0);
                }
            }
        }
    }
    let mut looks = Vec::new();
    for nr in std::mem::take(&mut queue.roots) {
        let Ok((minion, pose, g)) = bodies.get(nr.target) else {
            continue;
        };
        // A body already held is held longer, not grown round twice.
        if let Some(mut tg) = tangles
            .iter_mut()
            .find(|tg| tg.target == nr.target && tg.done.is_none())
        {
            tg.secs = tg.secs.max(tg.age + nr.secs);
            continue;
        }
        let body = g.translation();
        let ground = floor(body + DVec3::Y * 0.5);
        let height = minion.map_or(1.8, |m| crate::summon::body_height(m.kind, m.size));
        let hs = (height / 1.8).clamp(0.5, 3.0);
        let limbs = limb_ends(nr.target, minion, &pieces, pose, height);
        let ankle = |e: End, side: f32| {
            ends.at(e).unwrap_or_else(|| {
                body + (g.rotation() * Vec3::new(0.13 * side * hs, 0.08, 0.0)).as_dvec3()
            })
        };
        let feet = [ankle(limbs[2], -1.0), ankle(limbs[3], 1.0)];
        let origin = DVec3::new(body.x, ground.y, body.z);
        let local = |p: DVec3| {
            let d = p - origin;
            Vec3::new(d.x as f32, 0.0, d.z as f32)
        };
        let seed = nr.cast.seed as u64 ^ nr.target.to_bits().wrapping_mul(0x2545_f491);
        // Snug round the leg: a skeleton's are bone, a golem's are stone pillars.
        let leg_r = match minion.map(|m| m.kind) {
            Some(MinionKind::Skeleton | MinionKind::SkeletalMage) => 0.06,
            Some(MinionKind::Golem | MinionKind::Treant) => 0.2,
            Some(_) => 0.1,
            None => 0.075,
        } * hs;
        let tangle = commands
            .spawn((Name::new("Tangle"), Transform::from_translation(origin)))
            .id();
        let mut ids = Vec::new();
        let mut k = 0u64;
        // Three roots up each leg, round it in turn, out of the earth on all sides of it.
        for (i, &f) in feet.iter().enumerate() {
            let foot = local(f);
            for j in 0..LEG_ROOTS {
                k += 1;
                let h = |salt: u64| roll(seed ^ k.wrapping_mul(0x51ed), salt);
                let girth = 0.072 * hs * (0.85 + 0.3 * h(1));
                let angle = (j as f32 + 0.6 * h(2)) / LEG_ROOTS as f32 * TAU + i as f32;
                let path = art::coil(
                    foot,
                    angle,
                    (0.5 + 0.35 * h(3)) * hs,
                    leg_r + girth * 0.9,
                    (0.36 + 0.2 * h(4)) * height,
                    1.6 + 0.8 * h(5),
                    (i + j as usize).is_multiple_of(2),
                );
                let start = path[0].with_y(0.0);
                ids.push(spawn_root(
                    &mut commands,
                    origin,
                    &path,
                    SEG_M,
                    girth,
                    0.05 * k as f32,
                    None,
                    seed ^ k,
                    3,
                    3,
                    0.17 * hs,
                ));
                spawn_clod(&mut commands, tangle, start, 0.32 * hs, seed ^ k);
            }
        }
        // A thick one round both legs, up to the hips.
        {
            k += 1;
            let h = |salt: u64| roll(seed ^ k.wrapping_mul(0x51ed), salt);
            let mid = (local(feet[0]) + local(feet[1])) * 0.5;
            let spread = local(feet[0]).distance(local(feet[1])) * 0.5;
            let girth = 0.11 * hs;
            let path = art::coil(
                mid,
                h(2) * TAU,
                (0.8 + 0.3 * h(3)) * hs,
                spread + leg_r + girth,
                0.62 * height,
                1.1,
                h(5) < 0.5,
            );
            let start = path[0].with_y(0.0);
            ids.push(spawn_root(
                &mut commands,
                origin,
                &path,
                SEG_M,
                girth,
                0.12,
                None,
                seed ^ k,
                2,
                3,
                0.2 * hs,
            ));
            spawn_clod(&mut commands, tangle, start, 0.45 * hs, seed ^ k);
        }
        commands.entity(tangle).insert(Tangle {
            target: nr.target,
            roots: ids,
            age: 0.0,
            secs: nr.secs,
            done: None,
        });
        looks.push(LookCmd::Burst {
            at: origin,
            up: Vec3::Y,
            look: "stone".into(),
            role: Role::Impact,
            scale: 0.9 * hs,
            color: SOIL,
        });
    }
    for np in std::mem::take(&mut queue.patches) {
        let seed = np.cast.seed as u64 ^ np.at.x.to_bits() ^ (np.at.z.to_bits() << 5);
        let n = (np.radius * np.radius * 0.9).round().clamp(2.0, 14.0) as u64;
        let center = floor(np.at + DVec3::Y * 0.5);
        let patch = commands
            .spawn((Name::new("Root patch"), Transform::from_translation(center)))
            .id();
        let mut ids = Vec::new();
        for i in 0..n {
            let h = |salt: u64| roll(seed ^ i.wrapping_mul(0x7f4a_7c15), salt);
            let a = h(1) * TAU;
            let d = np.radius * (0.15 + 0.85 * h(2).sqrt());
            let base = Vec3::new(a.cos() * d, 0.0, a.sin() * d);
            let lean = Vec3::new(a.cos(), 0.0, a.sin()) * if h(3) < 0.3 { -1.0 } else { 1.0 };
            let path = art::writhe(
                base,
                0.9 + 0.7 * h(4),
                lean,
                1.6 + 1.3 * h(5),
                (seed ^ i) as u32,
            );
            ids.push(spawn_root(
                &mut commands,
                center,
                &path,
                SEG_M,
                0.07 + 0.03 * h(6),
                0.3 * h(7),
                Some(PATCH_HOLD + 0.5 * h(8)),
                seed ^ i,
                2,
                5,
                0.2,
            ));
            spawn_clod(&mut commands, patch, base, 0.38, seed ^ i);
        }
        commands.entity(patch).insert(Patch {
            roots: ids,
            age: 0.0,
            sinking: None,
        });
    }
    for nh in std::mem::take(&mut queue.hedges) {
        let seed = nh.cast.seed as u64 ^ 0x4ed9e;
        let center = floor(nh.at + DVec3::Y * 0.5);
        let along = Vec3::new(nh.along.x, 0.0, nh.along.z).normalize_or(Vec3::X);
        let side = Vec3::new(-along.z, 0.0, along.x);
        let hedge = commands
            .spawn((
                Name::new("Thorn hedge"),
                Transform::from_translation(center),
            ))
            .id();
        let half = nh.half.max(0.5);
        let mut ids = Vec::new();
        let grow_in = |u: f32| 0.5 * (u.abs() / half).min(1.0);
        // Brambles in two rows, each arching over from its own side of the line toward the
        // other's, so they cross and lace into a thicket; low ones fill the bottom.
        let n = (2.0 * half / HEDGE_STEP).ceil() as u64;
        for i in 0..n {
            for row in 0..2u64 {
                let id = i * 2 + row;
                let h = |salt: u64| roll(seed ^ id.wrapping_mul(0x2f1b), salt);
                let ua = -half + (i as f32 + 0.5 * row as f32) * HEDGE_STEP + 0.15 * (h(1) - 0.5);
                let way = if h(9) < 0.5 { 1.0 } else { -1.0 };
                let ub = (ua + way * (0.7 + 1.1 * h(2))).clamp(-half - 0.2, half + 0.2);
                let off = if row == 0 { -0.3 } else { 0.3 };
                let a = along * ua + side * (off + 0.2 * (h(3) - 0.5));
                let b = along * ub + side * (-0.6 * off + 0.3 * (h(4) - 0.5));
                let path = art::bramble(a, b, HEDGE_M * (0.4 + 0.65 * h(5)), id as u32);
                ids.push(spawn_root(
                    &mut commands,
                    center,
                    &path,
                    HEDGE_SEG_M,
                    0.045 + 0.025 * h(6),
                    grow_in((ua + ub) * 0.5),
                    None,
                    seed ^ id,
                    3,
                    2,
                    0.36,
                ));
            }
        }
        // Stems standing up out of it, leaning either way.
        let m = (2.0 * half / 0.38).ceil() as u64;
        for i in 0..m {
            let h = |salt: u64| roll(seed ^ (i + 1000).wrapping_mul(0x2f1b), salt);
            let u = -half + (i as f32 + 0.5) * 0.38;
            let base = along * u + side * 0.25 * (h(1) - 0.5) * 2.0;
            let lean = side * if h(2) < 0.5 { -1.0 } else { 1.0 } + along * (h(3) - 0.5);
            let path = art::writhe(
                base,
                HEDGE_M * (0.8 + 0.35 * h(4)),
                lean,
                0.7 + 0.8 * h(5),
                (seed ^ (i + 1000)) as u32,
            );
            ids.push(spawn_root(
                &mut commands,
                center,
                &path,
                HEDGE_SEG_M,
                0.065,
                grow_in(u) + 0.15,
                None,
                seed ^ (i + 1000),
                3,
                2,
                0.4,
            ));
            if i % 2 == 0 {
                spawn_clod(&mut commands, hedge, base, 0.45, seed ^ (i + 1000));
            }
        }
        commands.entity(hedge).insert(Hedge {
            field: nh.field,
            roots: ids,
            age: 0.0,
            done: None,
        });
    }
    for nb in std::mem::take(&mut queue.barks) {
        let Ok((minion, pose, _)) = bodies.get(nb.target) else {
            continue;
        };
        if let Some(mut s) = skins.iter_mut().find(|s| s.target == nb.target) {
            s.secs = s.secs.max(s.age + nb.secs);
            continue;
        }
        let height = minion.map_or(1.8, |m| crate::summon::body_height(m.kind, m.size));
        let plates = plates_for(pose, height, nb.cast.seed as u64);
        let skin_plates = plates
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let e = commands
                    .spawn((
                        Name::new("Bark plate"),
                        Transform::from_scale(Vec3::ZERO),
                        MeshRenderer::new(
                            MeshRef::Shape(Shape::Rock {
                                size: p.size.to_array(),
                                seed: ((nb.cast.seed as u64 + i as u64 * 7919) % 100_000) as u32,
                                detail: 2,
                            }),
                            MaterialRef::Inline(old_bark()),
                        ),
                        WildPiece,
                    ))
                    .id();
                (e, p)
            })
            .collect();
        commands.spawn((
            Name::new("Bark skin"),
            BarkSkin {
                target: nb.target,
                plates: skin_plates,
                age: 0.0,
                secs: nb.secs,
            },
        ));
    }
    queue.looks.extend(looks);
}

/// The bones bark covers on a skinned body: from a bone toward its child, the limb's radius,
/// plates round it.
const BARK_BONES: [(&str, &str, f32, u32); 12] = [
    ("lower_leg_l", "foot_l", 0.065, 3),
    ("lower_leg_r", "foot_r", 0.065, 3),
    ("upper_leg_l", "lower_leg_l", 0.085, 3),
    ("upper_leg_r", "lower_leg_r", 0.085, 3),
    ("hips", "spine", 0.15, 5),
    ("spine", "chest", 0.15, 5),
    ("chest", "neck", 0.16, 5),
    ("clavicle_l", "upper_arm_l", 0.07, 2),
    ("clavicle_r", "upper_arm_r", 0.07, 2),
    ("upper_arm_l", "lower_arm_l", 0.06, 3),
    ("upper_arm_r", "lower_arm_r", 0.06, 3),
    ("lower_arm_l", "hand_l", 0.05, 3),
];

/// Where bark grows on a body: round its bones when it has a skeleton with them, else round its
/// trunk in rows, from the feet up.
pub fn plates_for(pose: Option<&SkeletonPose>, height: f32, seed: u64) -> Vec<Plate> {
    let mut out = Vec::new();
    let mut extra = BARK_BONES.to_vec();
    extra.push(("lower_arm_r", "hand_r", 0.05, 3));
    if let Some(p) = pose {
        let found: Vec<_> = extra
            .iter()
            .filter_map(|&(a, b, r, n)| Some((p.index_of(a)?, p.index_of(b)?, r, n)))
            .collect();
        if found.len() == extra.len() {
            for (row, &(a, b, r, n)) in found.iter().enumerate() {
                for j in 0..n {
                    let h = |salt: u64| roll(seed ^ ((row * 16 + j as usize) as u64), salt);
                    out.push(Plate {
                        bones: Some((a, b)),
                        along: 0.5 + 0.1 * (h(1) - 0.5),
                        around: (j as f32 + 0.3 * h(2)) / n as f32 * TAU,
                        out: r,
                        size: Vec3::new(
                            (0.1 + 0.6 * r) * (0.85 + 0.3 * h(3)),
                            0.2 + 0.4 * r,
                            0.045,
                        ),
                        at: 0.025 * out.len() as f32 * 0.6,
                    });
                }
            }
            return out;
        }
    }
    let hs = height / 1.8;
    for row in 0..5 {
        let along = 0.12 + 0.17 * row as f32;
        let n = 6;
        for j in 0..n {
            let h = |salt: u64| roll(seed ^ ((row * 16 + j) as u64), salt);
            out.push(Plate {
                bones: None,
                along,
                around: (j as f32 + 0.5 * (row % 2) as f32 + 0.2 * h(2)) / n as f32 * TAU,
                out: 0.2 * hs,
                size: Vec3::new(0.2 * hs, 0.32 * hs, 0.05) * (0.85 + 0.3 * h(3)),
                at: 0.1 * row as f32 + 0.02 * j as f32,
            });
        }
    }
    out
}

/// A plate's place and turn on a body now: `axis_from` to `axis_to` is the bone (or trunk) it
/// sits round, `front` the body's front.
pub fn plate_pose(p: &Plate, axis_from: Vec3, axis_to: Vec3, front: Vec3) -> (Vec3, Quat) {
    let axis = (axis_to - axis_from).normalize_or(Vec3::Y);
    let f = (front - axis * front.dot(axis)).normalize_or(axis.any_orthonormal_vector());
    let n = Quat::from_axis_angle(axis, p.around) * f;
    let at = axis_from.lerp(axis_to, p.along) + n * (p.out + p.size.z * 0.4);
    let x = axis.cross(n).normalize_or(Vec3::X);
    (at, Quat::from_mat3(&Mat3::from_cols(x, axis, n)))
}

/// Grow, hold and let go: tangles, patches and hedges and their roots, falling bark.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_wilds(
    time: Res<Time>,
    mut tangles: Query<(Entity, &mut Tangle)>,
    mut patches: Query<(Entity, &mut Patch)>,
    mut hedges: Query<(Entity, &mut Hedge)>,
    mut roots: Query<(Entity, &mut Root, &GlobalTransform)>,
    mut tfs: Query<&mut Transform, With<WildPiece>>,
    mut renderers: Query<&mut MeshRenderer, With<WildPiece>>,
    mut falling: Query<(Entity, &mut Falling)>,
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
    let up = |v: Option<&Vitality>| v.is_none_or(|v| !v.down());
    // Owners: let their roots go when their time is up.
    for (e, mut tg) in &mut tangles {
        tg.age += dt;
        let held = alive.get(tg.target).is_ok() && up(vitals.get(tg.target).ok());
        if tg.done.is_none() && (tg.age >= tg.secs || !held) {
            tg.done = Some(0.0);
        }
        if let Some(d) = tg.done.as_mut() {
            *d += dt;
            let d = *d;
            for &r in &tg.roots {
                if let Ok((_, mut r, _)) = roots.get_mut(r) {
                    r.let_go();
                }
            }
            sink(&children, e, d, &mut tfs);
            if tg.roots.iter().all(|r| roots.get(*r).is_err()) && d > 0.8 {
                commands.entity(e).despawn();
            }
        }
    }
    for (e, mut p) in &mut patches {
        p.age += dt;
        let left = p.roots.iter().filter(|r| roots.get(**r).is_ok()).count();
        if p.sinking.is_none()
            && p.roots
                .iter()
                .filter_map(|r| roots.get(*r).ok())
                .all(|(_, r, _)| r.until.is_some_and(|u| r.out() > u))
        {
            p.sinking = Some(0.0);
        }
        if let Some(d) = p.sinking.as_mut() {
            *d += dt;
            sink(&children, e, *d, &mut tfs);
            if left == 0 && *d > 0.8 {
                commands.entity(e).despawn();
            }
        }
    }
    for (e, mut hg) in &mut hedges {
        hg.age += dt;
        let standing = fields.get(hg.field).is_ok_and(|f| f.spent.is_none());
        if hg.done.is_none() && !standing && hg.age > 0.5 {
            hg.done = Some(0.0);
            for (i, &r) in hg.roots.iter().enumerate() {
                if let Ok((_, mut r, _)) = roots.get_mut(r) {
                    // They let go a little apart, so the hedge sinks unevenly.
                    r.until = Some(r.out().max(0.0) + 0.3 * roll(i as u64, 9));
                }
            }
        }
        if let Some(d) = hg.done.as_mut() {
            *d += dt;
            sink(&children, e, *d, &mut tfs);
            if *d > 2.5 {
                commands.entity(e).despawn();
            }
        }
    }
    // Roots: grow, hold, slide back or burn.
    for (e, mut r, g) in &mut roots {
        r.age += dt;
        if r.out() >= 0.0 && !r.burst {
            r.burst = true;
            if r.until.is_some() || r.girth > 0.06 {
                // Earth flies where it breaks out.
                let p = r.at(0.0);
                let (look, color) = if r.bone {
                    ("raise", crate::undead::GRAVE_DUST)
                } else {
                    ("stone", SOIL)
                };
                queue.looks.push(LookCmd::Burst {
                    at: g.translation() + p.with_y(0.0).as_dvec3(),
                    up: Vec3::Y,
                    look: look.into(),
                    role: Role::Impact,
                    scale: 0.45,
                    color,
                });
            }
        }
        // A root set to burn later (a hedge the fire runs along) waits below 0.
        let burning = r.burnt.as_mut().and_then(|b| {
            let was = *b;
            *b += dt;
            (*b > 0.0).then_some((was <= 0.0, *b))
        });
        if let Some((first, b)) = burning {
            if first {
                for &s in &r.segs {
                    if let Ok(mut mr) = renderers.get_mut(s) {
                        mr.material = MaterialRef::Inline(charred());
                    }
                }
                let mid = r.at(r.shown.max(0.0) * 0.5);
                queue.looks.push(LookCmd::Burst {
                    at: g.translation() + mid.as_dvec3(),
                    up: Vec3::Y,
                    look: "fire".into(),
                    role: Role::Impact,
                    scale: 0.5,
                    color: [1.0, 0.45, 0.1],
                });
            }
            let shown = r.shown.max(0.0);
            draw_root(&r, shown, (b / BURN_SECS).clamp(0.0, 1.0), &mut tfs);
            if b > BURN_SECS {
                commands.entity(e).despawn();
            }
            continue;
        }
        let len = r.length_now();
        if (len - r.shown).abs() > 1e-4 {
            draw_root(&r, len, 0.0, &mut tfs);
            r.shown = len;
        }
        if r.gone() {
            commands.entity(e).despawn();
        }
    }
    // Bark that cracked off falls and is gone.
    for (e, mut f) in &mut falling {
        f.age += dt;
        f.vel.y -= 9.8 * dt;
        if let Ok(mut tf) = tfs.get_mut(e) {
            let mut p = tf.translation + (f.vel * dt).as_dvec3();
            if p.y < f.floor {
                p.y = f.floor;
                f.vel *= 0.3;
                f.vel.y = f.vel.y.abs() * 0.2;
            }
            tf.translation = p;
            tf.rotation = Quat::from_scaled_axis(f.spin * dt) * tf.rotation;
            if f.age > 1.0 {
                let s = (1.0 - (f.age - 1.0) / 0.5).max(0.0);
                tf.scale = tf.scale.normalize_or_zero() * tf.scale.length().min(s * 1.8);
            }
        }
        if f.age > 1.5 {
            commands.entity(e).despawn();
        }
    }
}

/// How fast fire runs along a hedge, m/s.
pub const HEDGE_BURN_M_S: f32 = 6.0;

/// Fire through a thorn hedge sets it alight: the fire runs both ways along it from where it went
/// in, each bramble charring and crumbling as it comes, and the hedge's wall is gone.
pub fn burn_hedges(
    missiles: Query<(&crate::runtime::Missile, &crate::runtime::SpellPos)>,
    mut hedges: Query<&mut Hedge>,
    mut fields: Query<(&mut crate::runtime::Field, &crate::runtime::SpellPos)>,
    mut roots: Query<&mut Root>,
) {
    for mut hg in &mut hedges {
        if hg.done.is_some() {
            continue;
        }
        let Ok((mut f, fp)) = fields.get_mut(hg.field) else {
            continue;
        };
        let Some((along, half)) = f.wall else {
            continue;
        };
        if f.spent.is_some() {
            continue;
        }
        let lit = missiles.iter().find_map(|(m, mp)| {
            if m.spent.is_some() || m.cast.spell.element() != Some(crate::schools::Element::Fire) {
                return None;
            }
            let v = (mp.pos - fp.pos).as_vec3();
            let a = v.dot(along).clamp(-half, half);
            let off = v - along * a;
            (Vec3::new(off.x, 0.0, off.z).length() < f.radius + 0.4
                && v.y > -0.5
                && v.y < HEDGE_M + 0.6)
                .then_some(a)
        });
        let Some(a) = lit else {
            continue;
        };
        f.spent = Some(0.0);
        hg.done = Some(0.0);
        for &r in &hg.roots {
            if let Ok(mut r) = roots.get_mut(r)
                && r.burnt.is_none()
            {
                let mid = r.at(r.total() * 0.5).dot(along);
                r.burnt = Some(-(mid - a).abs() / HEDGE_BURN_M_S);
            }
        }
    }
}

/// An owner's clods rise as its roots break out, and sink when they let go.
pub(crate) fn sink(
    children: &Query<&Children>,
    owner: Entity,
    d: f32,
    tfs: &mut Query<&mut Transform, With<WildPiece>>,
) {
    let Ok(kids) = children.get(owner) else {
        return;
    };
    let s = (1.0 - d / 0.8).clamp(0.0, 1.0);
    for k in kids.iter() {
        if let Ok(mut tf) = tfs.get_mut(k) {
            tf.scale = Vec3::splat(s);
        }
    }
}

/// Bark rides its body's bones; when its time is up it cracks off and falls.
#[allow(clippy::type_complexity)]
pub fn hold_bark(
    time: Res<Time>,
    mut skins: Query<(Entity, &mut BarkSkin)>,
    bodies: Query<(&GlobalTransform, Option<&SkeletonPose>, Option<&Vitality>)>,
    mut tfs: Query<&mut Transform, With<WildPiece>>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut s) in &mut skins {
        s.age += dt;
        let body = bodies.get(s.target).ok();
        let down = body.is_none_or(|(_, _, v)| v.is_some_and(|v| v.down()));
        let Some((g, pose, _)) = body else {
            for (p, _) in &s.plates {
                commands.entity(*p).despawn();
            }
            commands.entity(e).despawn();
            continue;
        };
        let front = g.rotation() * Vec3::NEG_Z;
        let feet = g.translation();
        let height = 1.8;
        let ending = s.age >= s.secs || down;
        for (pe, p) in &s.plates {
            let (a, b) = match (p.bones, pose) {
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
            let (pos, rot) = if p.bones.is_some() {
                plate_pose(p, a, b, front)
            } else {
                let mut q = *p;
                q.along = 0.0;
                plate_pose(&q, a, b, front)
            };
            let grow = ((s.age - p.at) / 0.18).clamp(0.0, 1.0);
            let Ok(mut tf) = tfs.get_mut(*pe) else {
                continue;
            };
            tf.translation = feet + pos.as_dvec3();
            tf.rotation = rot;
            tf.scale = Vec3::splat(grow);
            if ending {
                let out = (pos - Vec3::new(0.0, pos.y, 0.0)).normalize_or(front);
                commands.entity(*pe).insert(Falling {
                    vel: out * 1.4 + Vec3::Y * 1.2,
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
        if ending {
            queue.looks.push(LookCmd::Burst {
                at: feet + DVec3::Y * 1.0,
                up: Vec3::Y,
                look: "stone".into(),
                role: Role::Impact,
                scale: 0.8,
                color: SOIL,
            });
            commands.entity(e).despawn();
        }
    }
}

/// Leaves get their card and picture.
pub fn dress_wilds(
    mut leaves: Query<&mut MeshRenderer, Added<Leaf>>,
    mut wm: ResMut<WildsMeshes>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    images: Option<ResMut<Assets<Image>>>,
) {
    let (Some(mut meshes), Some(mut images)) = (meshes, images) else {
        return;
    };
    for mut r in &mut leaves {
        r.mesh = MeshRef::Handle(wm.card(&mut meshes));
        r.material = MaterialRef::Inline(leaf_material(wm.leaf(&mut images)));
    }
}

// ------------------------------------------------------------------------------- treant

/// A treant's body: the golem's pieces in bark (a gnarled trunk, root legs, branch arms), a
/// crown of leaves and branches over its head and shoulders, root toes, twig fingers and green
/// eyes. `part` makes a posed piece; extras ride the pieces as their children.
pub fn build_treant(
    commands: &mut Commands,
    root: Entity,
    k: f32,
    seed: u64,
    parts: &mut Vec<Entity>,
) {
    use crate::summon::{ARM, CORE, EYE, GOLEM_SIZES, HEAD, LEG, PELVIS, PLATE, TORSO};
    let wood = old_bark().with_tint(0.58, 0.48, 0.37);
    // Knots and burls: rough bark-covered lumps.
    let knot = |size: Vec3, salt: u64| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Rock {
                size: size.to_array(),
                seed: ((seed + salt * 7919) % 100_000) as u32,
                detail: 2,
            }),
            MaterialRef::Inline(wood),
        )
    };
    // A limb or the trunk: a round bough along the piece's length (its y).
    let bough = |radius: f32, length: f32| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Capsule {
                radius,
                height: length.max(2.0 * radius + 0.01),
                segments: 12,
            }),
            MaterialRef::Inline(wood),
        )
    };
    // The pieces summon.rs walks (a golem's), drawn as a tree: a tall trunk on a knotted
    // pelvis, a stump of a head, boughs for limbs, root knots for feet and fists.
    for (i, s) in GOLEM_SIZES.iter().enumerate() {
        let size = Vec3::from_array(*s) * k;
        let limb = |j: usize| LEG.iter().chain(ARM.iter()).any(|&l| l + j == i);
        let r = match i {
            PELVIS => knot(Vec3::new(0.78, 0.5, 0.6) * k, 1),
            TORSO => bough(0.38 * k, 1.6 * k),
            HEAD => bough(0.24 * k, 0.72 * k),
            _ if (PLATE[0]..=PLATE[1]).contains(&i) => {
                knot(Vec3::new(0.42, 0.3, 0.44) * k, i as u64)
            }
            _ if limb(0) => bough(0.15 * k, size.y * 1.12),
            _ if limb(1) => bough(0.13 * k, size.y * 1.12),
            _ => knot(size * 0.62, i as u64),
        };
        let id = commands
            .spawn((
                Name::new("Minion piece"),
                Transform::default(),
                r,
                MinionPart { index: i, size },
                ChildOf(root),
            ))
            .id();
        parts.push(id);
    }
    let glow = |r: f32, nits: f32| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Sphere {
                radius: r,
                segments: 12,
            }),
            MaterialRef::Inline(
                StandardMaterial::default()
                    .with_tint(TREANT_EYES[0], TREANT_EYES[1], TREANT_EYES[2])
                    .with_emissive(TREANT_EYES, nits),
            ),
        )
    };
    // The golem's core sits deep in the trunk, out of sight; the eyes glow from the stump.
    for (index, r) in [
        (CORE, glow(0.02 * k, 200.0)),
        (EYE[0], glow(0.045 * k, TREANT_EYE_GLOW)),
        (EYE[1], glow(0.045 * k, TREANT_EYE_GLOW)),
    ] {
        let id = commands
            .spawn((
                Name::new("Minion piece"),
                Transform::default(),
                r,
                MinionPart {
                    index,
                    size: Vec3::ZERO,
                },
                ChildOf(root),
            ))
            .id();
        parts.push(id);
    }
    let child =
        |commands: &mut Commands, parent: Entity, tf: Transform, r: MeshRenderer, leaf: bool| {
            let mut ec = commands.spawn((
                Name::new("Treant growth"),
                tf,
                r,
                WildPiece,
                ChildOf(parent),
            ));
            if leaf {
                ec.insert(Leaf);
            }
        };
    let h = |salt: u64| roll(seed ^ 0x7ee, salt);
    let branch = |len: f32, r: f32| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Cone {
                radius: r,
                height: len,
                segments: 8,
            }),
            MaterialRef::Inline(wood),
        )
    };
    let leaf_card = || {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Quad { size: [1.0, 1.0] }),
            MaterialRef::Inline(StandardMaterial::color(0.05, 0.12, 0.02)),
        )
    };
    let along = |dir: Vec3, len: f32, from: Vec3| {
        let d = dir.normalize();
        Transform::from_translation((from + d * len * 0.5).as_dvec3())
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, d))
    };
    let leaf_at = |at: Vec3, size: f32, salt: u64| {
        Transform::from_translation(at.as_dvec3())
            .with_rotation(Quat::from_euler(
                EulerRot::YXZ,
                h(salt) * TAU,
                (h(salt + 1) - 0.5) * 1.6,
                h(salt + 2) * TAU,
            ))
            .with_scale(Vec3::splat(size))
    };
    // A crown: boughs up and out of the stump, each in a cloud of leaf clusters.
    let head = parts[HEAD];
    let torso = parts[TORSO];
    for j in 0..CROWN_BOUGHS {
        let a = j as f32 / CROWN_BOUGHS as f32 * TAU + 0.5 * h(j);
        let out = 0.55 + 0.45 * h(10 + j);
        let dir = Vec3::new(a.cos() * out, 1.0, a.sin() * out * 0.8 - 0.1);
        let len = (0.8 + 0.6 * h(20 + j)) * k;
        let from = Vec3::new(0.0, 0.2, 0.0) * k;
        child(
            commands,
            head,
            along(dir, len, from),
            branch(len, 0.075 * k),
            false,
        );
        let d = dir.normalize();
        let side = d.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = side.cross(d);
        for c in 0..CROWN_LEAVES {
            let salt = 100 + j * 31 + c * 7;
            let t = 0.45 + 0.65 * h(salt + 3);
            let spread = 0.35 * k * t;
            let at = from
                + d * len * t
                + side * spread * (h(salt + 4) - 0.5) * 2.0
                + up * spread * (h(salt + 5) - 0.3) * 1.6;
            child(
                commands,
                head,
                leaf_at(at, (0.45 + 0.25 * h(salt + 6)) * k, salt),
                leaf_card(),
                true,
            );
        }
    }
    // Twiggy boughs off the shoulders, leafed at their ends.
    for (i, &p) in PLATE.iter().enumerate() {
        let side = if i == 0 { -1.0 } else { 1.0 };
        let shoulder = parts[p];
        for j in 0..3 {
            let dir = Vec3::new(side * (0.8 + 0.3 * h(60 + j)), 0.9, 0.3 * (h(70 + j) - 0.5));
            let len = (0.45 + 0.3 * h(80 + j)) * k;
            child(
                commands,
                shoulder,
                along(dir, len, Vec3::ZERO),
                branch(len, 0.045 * k),
                false,
            );
            for c in 0..3 {
                let salt = 400 + j * 13 + c * 5 + i as u64 * 97;
                let at = dir.normalize() * len * (0.7 + 0.35 * h(salt + 3))
                    + Vec3::new(0.0, 0.15 * (h(salt + 4) - 0.5), 0.2 * (h(salt + 5) - 0.5)) * k;
                child(
                    commands,
                    shoulder,
                    leaf_at(at, 0.4 * k, salt),
                    leaf_card(),
                    true,
                );
            }
        }
    }
    // Burls on the trunk, and a broken-off branch.
    for j in 0..4 {
        let a = h(130 + j) * TAU;
        let y = (-0.4 + 0.8 * h(140 + j)) * k;
        let at = Vec3::new(a.cos() * 0.36, y, a.sin() * 0.3) * k;
        child(
            commands,
            torso,
            Transform::from_translation(at.as_dvec3()),
            knot(
                Vec3::new(0.28, 0.34, 0.24) * k * (0.7 + 0.5 * h(150 + j)),
                50 + j,
            ),
            false,
        );
    }
    child(
        commands,
        torso,
        along(
            Vec3::new(-0.6, 0.5, 0.4),
            0.45 * k,
            Vec3::new(-0.3, 0.3, 0.2) * k,
        ),
        branch(0.45 * k, 0.06 * k),
        false,
    );
    // Twig fingers off each hand, root toes off each foot.
    for i in 0..2 {
        let fist = parts[ARM[i] + 2];
        for j in 0..4 {
            let a = (j as f32 - 1.5) * 0.35;
            let dir = Vec3::new(a, -1.0, -0.25);
            let len = 0.5 * k;
            child(
                commands,
                fist,
                along(dir, len, Vec3::new(0.0, -0.1, 0.0) * k),
                branch(len, 0.045 * k),
                false,
            );
        }
        let foot = parts[LEG[i] + 2];
        for j in 0..4 {
            let a = (j as f32 - 1.5) * 0.55;
            let dir = Vec3::new(a.sin(), -0.25, -a.cos());
            let len = 0.6 * k;
            child(
                commands,
                foot,
                along(dir, len, Vec3::new(0.0, 0.0, -0.1) * k),
                branch(len, 0.08 * k),
                false,
            );
        }
    }
}

// ------------------------------------------------------------------------------- upkeep

pub fn clear(world: &mut World) {
    let mut gone: Vec<Entity> = Vec::new();
    gone.extend(world.query_filtered::<Entity, With<Tangle>>().iter(world));
    gone.extend(world.query_filtered::<Entity, With<Patch>>().iter(world));
    gone.extend(world.query_filtered::<Entity, With<Hedge>>().iter(world));
    gone.extend(world.query_filtered::<Entity, With<Root>>().iter(world));
    gone.extend(world.query_filtered::<Entity, With<Falling>>().iter(world));
    let skins: Vec<(Entity, Vec<Entity>)> = world
        .query::<(Entity, &BarkSkin)>()
        .iter(world)
        .map(|(e, s)| (e, s.plates.iter().map(|p| p.0).collect()))
        .collect();
    for (e, plates) in skins {
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
    app.init_resource::<WildsMeshes>().register_api(
        "magic.wilds",
        "Living wood now: tangles (whom roots hold, seconds left, roots), root patches, thorn \
         hedges and bark skins (whom, seconds left, plates).",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Tangle)>();
            let tangles: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, t)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "target": t.target.to_bits(),
                        "left_s": t.left(),
                        "roots": t.roots.len(),
                        "done": t.done.is_some(),
                    })
                })
                .collect();
            let patches = world.query::<&Patch>().iter(world).count();
            let mut q = world.query::<(Entity, &Hedge)>();
            let hedges: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, h)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "roots": h.roots.len(),
                        "done": h.done.is_some(),
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &BarkSkin)>();
            let barks: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, s)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "target": s.target.to_bits(),
                        "left_s": s.left(),
                        "plates": s.plates.len(),
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "tangles": tangles,
                "patches": patches,
                "hedges": hedges,
                "barks": barks,
            }))
        },
    );
}

/// A spell's treant scale: its golem size times [`TREANT_SCALE`], so 3.4 m tall at the least
/// and 4.7 m at the most.
pub fn treant_size(spell: &Spell, scale: f32) -> f32 {
    (crate::summon::size_of(spell, scale) * TREANT_SCALE).clamp(1.3, 1.8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: Vec<Vec3>) -> Root {
        Root {
            pts: art::resample(&path, SEG_M),
            step: SEG_M,
            segs: Vec::new(),
            extras: Vec::new(),
            girth: 0.05,
            age: 0.0,
            delay: 0.1,
            until: None,
            burnt: None,
            shown: 0.0,
            burst: false,
            bone: false,
        }
    }

    #[test]
    fn a_root_slides_out_tip_first_holds_and_slides_back_in() {
        let mut r = root(art::coil(Vec3::ZERO, 0.0, 0.7, 0.12, 0.6, 1.4, true));
        let total = r.total();
        assert!(total > 1.0, "{total}");
        assert_eq!(r.length_now(), 0.0);
        r.age = 0.1 + 0.1;
        let part = r.length_now();
        assert!((part - 0.45).abs() < 1e-3, "{part}");
        // Out a little: only its last segments (the tip) are drawn, and the tip is at the
        // path's start, just breaking the ground.
        let poses = r.poses(part);
        let shown = poses.iter().filter(|p| p.is_some()).count();
        assert!(shown > 0 && shown < poses.len() / 2, "{shown}");
        assert!(poses[0].is_none() && poses.last().unwrap().is_some());
        // Fully out: every segment drawn where the path runs.
        r.age = 5.0;
        assert_eq!(r.length_now(), total);
        let poses = r.poses(total);
        assert!(poses.iter().all(Option::is_some));
        let (c, _, g) = poses[0].unwrap();
        assert!(c.distance(r.at(SEG_M * 0.5)) < 1e-3);
        assert!((g - art::girth(0.05, 0.5 / poses.len() as f32)).abs() < 1e-6);
        // Let go: it slides back in and is gone.
        r.let_go();
        r.age += 0.2;
        assert!(r.length_now() < total && !r.gone());
        r.age += 5.0;
        assert_eq!(r.length_now(), 0.0);
        assert!(r.gone());
    }

    #[test]
    fn plates_cover_a_trunk_from_the_feet_up() {
        let plates = plates_for(None, 1.8, 3);
        assert_eq!(plates.len(), 30);
        assert!(plates.windows(2).all(|w| w[0].at <= w[1].at + 1e-6));
        let p = plates[0];
        let (at, rot) = plate_pose(&p, Vec3::ZERO, Vec3::Y, Vec3::NEG_Z);
        // Out from the axis, facing out.
        let flat = Vec3::new(at.x, 0.0, at.z).length();
        assert!((flat - (p.out + p.size.z * 0.4)).abs() < 1e-3, "{flat}");
        let n = rot * Vec3::Z;
        assert!(n.dot(Vec3::new(at.x, 0.0, at.z).normalize()) > 0.99);
        assert!((rot * Vec3::Y).dot(Vec3::Y) > 0.99);
    }

    #[test]
    fn wilds_hold_with_area_and_missile_casts_only() {
        assert!(roots_hold(CastType::Area) && roots_hold(CastType::Bolt));
        assert!(!roots_hold(CastType::Wall) && !roots_hold(CastType::OnSelf));
        assert!(!roots_hold(CastType::Summon));
    }
}
