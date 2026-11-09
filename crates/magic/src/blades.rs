//! Conjured blades and imbued weapons (round-14-plan.md): swords of hard light that magic draws
//! itself, and spells that ride on weapons.
//!
//! * **Glintblade** (Bolt + blade): a [`Missile`] like any bolt, with a sword drawn on it
//!   ([`BladeRide`]). It turns a little toward what it was thrown at and breaks into shards on
//!   whatever it hits (Elden Ring's glintblades).
//! * **Phalanx** (Orbit + blade): blades in an arch over the caster's head ([`Flight::Arch`]).
//!   A foe within [`PHALANX_REACH_M`] draws one every [`PHALANX_EVERY`] s, loosed at it as a
//!   glintblade (Elden Ring's Glintblade Phalanx).
//! * **Bladefall** (Rain + blade): blades drop point first along the caster's aim, one every
//!   [`FALL_EVERY`] s; each cuts what stands where it lands and sticks in the ground a while
//!   (Path of Exile's Bladefall).
//! * **Spirit blade** (Summon + blade): a sword that flies beside its caster and fights on its
//!   own (Guild Wars 2's spirit weapons): it darts at a foe and cuts at it from a new angle each
//!   time. It has health and a side, so the foe's creatures strike back.
//! * **Bound blade** (Self + blade): a blade in the caster's hand (Skyrim's bound sword). While
//!   it lasts the body's `Loadout` says it holds a one-handed blade (two-handed with Size), so
//!   Combat and Animation treat it as one, and the blade's spell rides on its hits.
//! * **Imbue** (the cast type): the spell rides on its caster's weapon and its creatures'
//!   within [`IMBUE_REACH_M`] ([`Imbued`]). Each melee blow they land (a `DamageEvent` from them
//!   of kind Slash, Pierce or Blunt, whoever's melee sent it: Movement, NPC, Combat or magic's
//!   own creatures) lands the spell on the struck body as well, at most once per body every
//!   [`IMBUE_EVERY`] s. The weapon glows with it: a skeleton's blade, a golem's fists, a spirit
//!   blade; on a skinned body, the hand.

use std::f32::consts::PI;
use std::sync::Arc;

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{
    MaterialRef, Mesh, MeshRef, MeshRenderer, NoVirtualGeometry, PointLight, StandardMaterial,
    Vertex,
};
use engine_world::animation::SkeletonPose;
use engine_world::combat::{ItemClass, Loadout};
use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody};
use engine_world::{DamageEvent, DamageKind, Factions, WorldShift};

use crate::caster::{MagicCaster, Vitality};
use crate::compile::Spell;
use crate::design::{CastType, TargetFilter};
use crate::looks::{LookCmd, Role};
use crate::runtime::{
    Allegiance, CastInfo, CreatureFilter, HitQueue, MagicQueue, Missile, PendingHit, roll,
};
use crate::summon::{Minion, MinionPart};
use crate::wards::{WardMeshes, glass_burst, ward_material};

// ------------------------------------------------------------------------------- numbers

/// A conjured blade's glow, nits: about the brightest a thin glowing thing gets in the daylit
/// hall before its colour washes to white (magic-hall-pictures: violet glass read at 7 000 to
/// 15 000).
pub const BLADE_GLOW: f32 = 10_000.0;
/// The light a lasting blade (a phalanx's, a spirit blade, one in the hand) throws, lm.
pub const BLADE_LIGHT_LM: f32 = 700.0;
/// A phalanx loosens a blade at a foe this near its caster, m.
pub const PHALANX_REACH_M: f32 = 9.0;
/// Seconds between a phalanx's blades.
pub const PHALANX_EVERY: f32 = 0.35;
/// A loosed or thrown blade's speed, m/s.
pub const LOOSE_SPEED: f32 = 24.0;
/// Seconds between a bladefall's blades.
pub const FALL_EVERY: f32 = 0.08;
/// A falling blade drops from this high over where it lands, m, at this speed, m/s.
pub const FALL_FROM_M: f32 = 9.0;
pub const FALL_SPEED: f32 = 30.0;
/// A fallen blade stands in the ground this long before it fades, s.
pub const STUCK_SECS: f32 = 2.2;
/// A falling blade cuts bodies this near where it lands, m.
pub const FALL_CUT_M: f32 = 1.0;
/// A spirit blade looks this far for a foe, and strays at most this far from its caster, m.
pub const SPIRIT_SEE_M: f32 = 16.0;
pub const SPIRIT_LEASH_M: f32 = 22.0;
/// A spirit blade goes for a foe that isn't a creature only this near its caster, m.
pub const SPIRIT_GUARD_REACH_M: f32 = 7.0;
/// A spirit blade's flight, m/s.
pub const SPIRIT_PACE: f32 = 7.5;
/// A spirit blade's cut: the whole sweep, the moment it lands, and the rest after, s.
pub const SWING_SECS: f32 = 0.42;
pub const SWING_HIT: f32 = 0.2;
pub const SPIRIT_REST: f32 = 0.5;
/// A spirit blade's health per metre of blade, in its caster's own blows (`Power::damage`): a
/// pack of the dead at it breaks one in a few seconds, a lone skeleton takes a dozen cuts.
pub const SPIRIT_TOUGHNESS: f32 = 4.0;
/// A spirit blade keeps this far from its foe between cuts, m.
pub const SPIRIT_GUARD_M: f32 = 1.5;
/// An imbue reaches its caster's creatures this near, m.
pub const IMBUE_REACH_M: f32 = 15.0;
/// An imbued weapon lands its spell on one body at most this often, s.
pub const IMBUE_EVERY: f32 = 0.4;
/// Shards a breaking blade bursts into.
pub const BREAK_SHARDS: u32 = 9;
/// Seconds a blade takes to fade at the end of its time.
pub const FADE_SECS: f32 = 0.45;

// ------------------------------------------------------------------------------- data

/// What a blade cast makes, from `runtime::release` (a glintblade is a plain missile).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BladeShape {
    Phalanx,
    Fall,
    Spirit,
    Bound,
}

/// Blades to make, from `runtime::release`.
#[derive(Debug, Clone)]
pub struct NewBlades {
    pub cast: CastInfo,
    pub shape: BladeShape,
    pub hand: DVec3,
    pub body: DVec3,
    /// The caster's flat facing.
    pub facing: Vec3,
    pub aim: DVec3,
    pub target: Option<Entity>,
}

/// A sword drawn on a glintblade missile (any missile of a blade spell).
#[derive(Component, Debug, Clone)]
pub struct BladeRide {
    pub length: f32,
    pub color: [f32; 3],
    /// The drawn sword (a child), once drawn.
    pub drawn: Option<Entity>,
    /// It broke into shards (the missile was spent).
    pub broke: bool,
}

/// What a conjured blade is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Flight {
    /// In the phalanx's arch over its caster's head, waiting for a foe.
    Arch,
    /// A bladefall's blade not yet dropped: it waits `delay` s more.
    Waiting { delay: f32, ground: DVec3 },
    /// Dropping point first onto `ground`.
    Falling { ground: DVec3 },
    /// Point in the ground, `t` s.
    Stuck { t: f32 },
    /// A spirit blade: beside its caster or at its foe; `swing` s into a cut.
    Spirit { swing: Option<f32> },
    /// In its caster's hand (the off hand for the second of two).
    Held { off: bool },
}

/// A conjured blade that magic flies itself (a phalanx's, a bladefall's, a spirit blade, one in
/// the hand). Its transform is the grip, its +Y the point.
#[derive(Component, Debug, Clone)]
pub struct Blade {
    pub cast: CastInfo,
    pub owner: Option<Entity>,
    pub flight: Flight,
    pub pos: DVec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub age: f32,
    pub life: f32,
    pub length: f32,
    pub color: [f32; 3],
    /// Its place in the phalanx: index and count.
    pub slot: (u32, u32),
    pub target: Option<Entity>,
    pub look_in: f32,
    pub cooldown: f32,
    /// Cuts made (a spirit blade cuts from a new angle each time).
    pub cuts: u32,
    /// What it strikes with: a loosed phalanx blade's flight (a Bolt), or a spirit blade's or a
    /// falling blade's cut.
    pub strike: Arc<Spell>,
    /// Fading out (its time is up), s into the fade.
    pub fading: Option<f32>,
    /// A blade in the hand: what the hand's `Loadout` was before (None: it had none).
    pub before: Option<Option<Loadout>>,
}

impl Blade {
    /// Seconds before it goes.
    pub fn left(&self) -> f32 {
        (self.life - self.age).max(0.0)
    }

    pub fn kind(&self) -> &'static str {
        match self.flight {
            Flight::Arch => "phalanx",
            Flight::Waiting { .. } | Flight::Falling { .. } | Flight::Stuck { .. } => "bladefall",
            Flight::Spirit { .. } => "spirit",
            Flight::Held { .. } => "bound",
        }
    }
}

/// A body whose weapon carries a spell (Imbue, or a bound blade's own).
#[derive(Component, Debug, Clone)]
pub struct Imbued {
    pub cast: CastInfo,
    pub secs: f32,
    pub age: f32,
    /// Bodies struck lately, with seconds before they can be struck again.
    pub every: Vec<(Entity, f32)>,
    /// From a blade in the hand (the blade glows, not the hand).
    pub from_blade: bool,
    /// The look drawn now: the pieces carrying it and the weave's key, or none yet.
    drawn: Option<(Vec<Entity>, String)>,
    /// Weapon pieces whose material it changed, with what they were.
    restore: Vec<(Entity, MaterialRef)>,
}

impl Imbued {
    pub fn new(cast: CastInfo, secs: f32, from_blade: bool) -> Self {
        Self {
            cast,
            secs,
            age: 0.0,
            every: Vec::new(),
            from_blade,
            drawn: None,
            restore: Vec::new(),
        }
    }

    pub fn left(&self) -> f32 {
        (self.secs - self.age).max(0.0)
    }
}

/// A light that rides an imbued body's hand.
#[derive(Component, Debug, Clone, Copy)]
pub struct HandGlow {
    pub owner: Entity,
}

/// The blade mesh, built once.
#[derive(Resource, Debug, Default)]
pub struct BladeMeshes {
    blade: Option<Handle<Mesh>>,
}

impl BladeMeshes {
    pub fn blade(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        *self.blade.get_or_insert_with(|| meshes.add(blade_mesh()))
    }
}

// ------------------------------------------------------------------------------- shapes

/// A sword of unit length along +Y from the pommel (y = 0) to the point (y = 1): pommel, grip,
/// crossguard and a broad diamond-section blade. Vertex alpha makes the blade's flat translucent
/// and its edges and ridge bright, so it reads as hard light; the hilt is solid. Two crossed
/// fringes of fading light round the blade (in its flat and across it) give it a glow from any
/// side, so it reads at 20 m as a sword of light, not a hairline.
pub fn blade_mesh() -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let quad = |m: &mut Mesh, p: [Vec3; 4], a: [f32; 4]| {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Z);
        let base = m.vertices.len() as u32;
        for k in 0..4 {
            m.vertices
                .push(Vertex::new(p[k], n, Vec2::ZERO).with_color(Vec4::new(1.0, 1.0, 1.0, a[k])));
        }
        m.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    // A box from `lo` to `hi`, solid.
    let cuboid = |m: &mut Mesh, lo: Vec3, hi: Vec3, a: f32| {
        let c = |x: usize, y: usize, z: usize| {
            Vec3::new(
                if x == 0 { lo.x } else { hi.x },
                if y == 0 { lo.y } else { hi.y },
                if z == 0 { lo.z } else { hi.z },
            )
        };
        let faces = [
            [c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)],
            [c(1, 0, 0), c(0, 0, 0), c(0, 1, 0), c(1, 1, 0)],
            [c(1, 0, 1), c(1, 0, 0), c(1, 1, 0), c(1, 1, 1)],
            [c(0, 0, 0), c(0, 0, 1), c(0, 1, 1), c(0, 1, 0)],
            [c(0, 1, 1), c(1, 1, 1), c(1, 1, 0), c(0, 1, 0)],
            [c(0, 0, 0), c(1, 0, 0), c(1, 0, 1), c(0, 0, 1)],
        ];
        for f in faces {
            quad(m, f, [a; 4]);
        }
    };
    // Pommel, grip, guard.
    cuboid(
        &mut m,
        Vec3::new(-0.028, 0.0, -0.022),
        Vec3::new(0.028, 0.045, 0.022),
        1.0,
    );
    cuboid(
        &mut m,
        Vec3::new(-0.016, 0.045, -0.014),
        Vec3::new(0.016, 0.2, 0.014),
        0.9,
    );
    cuboid(
        &mut m,
        Vec3::new(-0.115, 0.2, -0.024),
        Vec3::new(0.115, 0.235, 0.024),
        1.0,
    );
    // The blade: an edge each side, a ridge each face, tapering to the point. Rows up the
    // blade: (height, half width).
    let rows: [(f32, f32); 5] = [
        (0.235, 0.052),
        (0.5, 0.048),
        (0.78, 0.039),
        (0.92, 0.023),
        (1.0, 0.0),
    ];
    let ridge = 0.012;
    for w in rows.windows(2) {
        let (y0, h0) = w[0];
        let (y1, h1) = w[1];
        let t0 = ridge * (h0 / 0.052).max(0.15);
        let t1 = ridge * (h1 / 0.052).max(0.15);
        for (sx, sz) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            // From the edge (bright) in to the ridge (bright), the flat between them dimmer:
            // two strips per quarter.
            let edge0 = Vec3::new(sx * h0, y0, 0.0);
            let edge1 = Vec3::new(sx * h1, y1, 0.0);
            let mid0 = Vec3::new(sx * h0 * 0.5, y0, sz * t0 * 0.5);
            let mid1 = Vec3::new(sx * h1 * 0.5, y1, sz * t1 * 0.5);
            let ridge0 = Vec3::new(0.0, y0, sz * t0);
            let ridge1 = Vec3::new(0.0, y1, sz * t1);
            // Wind each quad so its face points out of the blade.
            let flip = sx * sz < 0.0;
            let strip = |p: [Vec3; 4], al: [f32; 4]| {
                if flip {
                    ([p[1], p[0], p[3], p[2]], [al[1], al[0], al[3], al[2]])
                } else {
                    (p, al)
                }
            };
            let (p, al) = strip([edge0, mid0, mid1, edge1], [1.0, 0.35, 0.35, 1.0]);
            quad(&mut m, p, al);
            let (p, al) = strip([mid0, ridge0, ridge1, mid1], [0.35, 0.8, 0.8, 0.35]);
            quad(&mut m, p, al);
        }
    }
    // The glow: from each edge out to nothing, in the blade's flat (x) and across it (z), and
    // on past the point.
    const FRINGE: f32 = 0.085;
    for w in rows.windows(2) {
        let (y0, h0) = w[0];
        let (y1, h1) = w[1];
        let y1x = if h1 == 0.0 { y1 + FRINGE } else { y1 };
        for axis in [Vec3::X, Vec3::Z] {
            // Across the blade the fringe hugs its thickness, not its width.
            let k = if axis == Vec3::X { 1.0 } else { 0.45 };
            for side in [1.0, -1.0] {
                let in0 = Vec3::new(0.0, y0, 0.0) + axis * side * h0 * k;
                let in1 = Vec3::new(0.0, y1, 0.0) + axis * side * h1 * k;
                let out0 = in0 + axis * side * FRINGE;
                let out1 = Vec3::new(0.0, y1x, 0.0) + axis * side * (h1 * k + FRINGE * 0.6);
                quad(&mut m, [in0, out0, out1, in1], [0.42, 0.0, 0.0, 0.42]);
            }
        }
    }
    m
}

/// A blade's material: hard light in `color` (black under its glow, so it is all glow), as a
/// ward's.
pub fn blade_renderer(mesh: Handle<Mesh>, color: [f32; 3], glow: f32) -> MeshRenderer {
    MeshRenderer {
        mesh: MeshRef::Handle(mesh),
        material: MaterialRef::Inline(ward_material(color, crate::wards::glow_step(glow))),
        cast_shadows: false,
        visible: true,
    }
}

/// The colour of a blade spell: its weave's.
pub fn blade_color(spell: &Spell) -> [f32; 3] {
    spell.weave.color
}

/// The look key of a blade spell's flight and breaking: its element, or force for a plain one.
pub fn blade_look(spell: &Spell) -> String {
    spell.weave.key.clone()
}

/// Where blade `i` of `n` waits over its caster's head, in the caster's frame (x right, y up,
/// −z forward), and how it leans: an arch from one shoulder over the head to the other, a
/// little behind, each blade pointing up and out along it, tipped forward.
pub fn arch_slot(i: u32, n: u32, length: f32) -> (Vec3, Quat) {
    let span = 1.0_f32.min(0.28 * (n.max(2) - 1) as f32) * 1.15;
    let k = if n > 1 {
        i as f32 / (n - 1) as f32 * 2.0 - 1.0
    } else {
        0.0
    };
    let a = span * k;
    let r = 0.62 + 0.12 * length;
    let at = Vec3::new(a.sin() * r, 1.72 + a.cos() * r * 0.75, 0.32);
    let lean = Quat::from_rotation_z(-a * 0.85) * Quat::from_rotation_x(-0.35);
    (at, lean)
}

/// Where a bladefall's blades land: along the line from `from` to `to`, widening a little and
/// scattered either side.
pub fn fall_spots(from: DVec3, to: DVec3, n: u32, seed: u64) -> Vec<DVec3> {
    let d = to - from;
    let flat = DVec3::new(d.x, 0.0, d.z);
    let len = flat.length().max(0.5);
    let along = flat / len;
    let across = DVec3::new(-along.z, 0.0, along.x);
    (0..n)
        .map(|i| {
            let k = if n > 1 {
                i as f64 / (n - 1) as f64
            } else {
                1.0
            };
            let side = (roll(seed, 11 + i as u64) as f64 * 2.0 - 1.0) * (0.35 + 0.9 * k);
            let jitter = (roll(seed, 51 + i as u64) as f64 - 0.5) * 0.5;
            from + along * (len * k + jitter) + across * side
        })
        .collect()
}

/// Which way a spirit blade's point aims through its cut, `t` 0..1 of the sweep: from raised
/// over its shoulder through the foe and on down past it. Each cut comes from a new angle (it
/// turns the plane of the sweep round the line to the foe), as a fencer varies the line.
pub fn swing_point(to_foe: Vec3, t: f32, cut: u32) -> Vec3 {
    let to_foe = to_foe.normalize_or(Vec3::NEG_Z);
    let across = to_foe.cross(Vec3::Y).normalize_or(Vec3::X);
    let turn = [0.0, 1.1, -1.1, 0.55, -0.55, PI * 0.5][cut as usize % 6];
    let axis = Quat::from_axis_angle(to_foe, turn) * across;
    let raised = (Vec3::Y * 0.9 - to_foe * 0.45).normalize();
    let raised = Quat::from_axis_angle(to_foe, turn) * raised;
    let e = t * t * (3.0 - 2.0 * t);
    Quat::from_axis_angle(axis, -2.5 * e) * raised
}

/// The cut a spirit blade or a falling blade makes: the blade spell as a blow on one body, its
/// element with it, never on its own side.
pub fn cut_of(spell: &Spell, damage_k: f32) -> Arc<Spell> {
    let mut s = spell.clone();
    s.cast = CastType::Touch;
    s.stages.clear();
    s.payload.damage *= damage_k;
    s.payload.blades = 0;
    s.carrier.radius = 0.3;
    s.filter = TargetFilter::Enemies;
    Arc::new(s)
}

/// A phalanx's loosed blade: the spell as a glintblade (a Bolt) that seeks hard.
pub fn loose_of(spell: &Spell) -> Arc<Spell> {
    let mut s = spell.clone();
    s.cast = CastType::Bolt;
    s.carrier.speed = LOOSE_SPEED;
    s.carrier.range = PHALANX_REACH_M * 3.0;
    s.carrier.radius = 0.12;
    s.carrier.count = 1;
    s.carrier.seek_turn = 4.0;
    s.payload.blades = 1;
    Arc::new(s)
}

fn yaw_rot(facing: Vec3) -> Quat {
    Quat::from_rotation_y((-facing.x).atan2(-facing.z))
}

/// A spirit blade's body for blows and bolts to find.
fn spirit_body(length: f32) -> (Collider, RigidBody) {
    (
        Collider {
            shape: ColliderShape::Sphere {
                radius: 0.3 * length.max(0.8),
            },
            density: 400.0,
            ..Default::default()
        },
        RigidBody {
            kind: BodyKind::Kinematic,
            ..Default::default()
        },
    )
}

// ------------------------------------------------------------------------------- systems

/// Make the blades asked for this step, and put imbues on their bodies.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn spawn_blades(
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
    owners: Query<(&GlobalTransform, Option<&Loadout>)>,
    existing: Query<(Entity, &Blade)>,
    minions: Query<(Entity, &Minion, &GlobalTransform)>,
    mut imbued: Query<&mut Imbued>,
    (mut bm, mut meshes): (ResMut<BladeMeshes>, Option<ResMut<Assets<Mesh>>>),
) {
    let imbues = std::mem::take(&mut queue.imbues);
    for cast in imbues {
        let Some(caster) = cast.caster else { continue };
        // What rides on a blow is a touch of the spell: it lands where the blow did, with a
        // small burst (a touch's), so the melee isn't hidden in fire.
        let look_spell = cast.spell.clone();
        let cast = CastInfo {
            spell: cut_of(&cast.spell, 1.0),
            ..cast
        };
        let secs = cast.spell.payload.imbue_secs.max(1.0);
        let at = owners
            .get(caster)
            .map(|o| o.0.translation())
            .unwrap_or(DVec3::ZERO);
        let mut bodies = vec![caster];
        bodies.extend(
            minions
                .iter()
                .filter(|(_, m, g)| {
                    m.cast.caster == Some(caster)
                        && g.translation().distance(at) <= IMBUE_REACH_M as f64
                })
                .map(|(e, ..)| e),
        );
        bodies.extend(
            existing
                .iter()
                .filter(|(_, b)| {
                    b.owner == Some(caster) && matches!(b.flight, Flight::Spirit { .. })
                })
                .map(|(e, _)| e),
        );
        for b in bodies {
            if let Ok(mut im) = imbued.get_mut(b) {
                im.cast = cast.clone();
                im.secs = secs;
                im.age = 0.0;
                im.from_blade = false;
            } else {
                commands
                    .entity(b)
                    .try_insert(Imbued::new(cast.clone(), secs, false));
            }
        }
        queue.looks.push(LookCmd::Burst {
            at: at + DVec3::Y * 1.1,
            up: Vec3::Y,
            look: blade_look(&look_spell),
            role: Role::Impact,
            scale: 0.6,
            color: look_spell.weave.color,
        });
    }
    if queue.blades.is_empty() {
        return;
    }
    let mesh = meshes.as_deref_mut().map(|m| bm.blade(m));
    for nb in std::mem::take(&mut queue.blades) {
        let spell = nb.cast.spell.clone();
        let p = &spell.payload;
        let n = p.blades.max(1);
        let length = p.blade_m.max(0.5);
        let color = blade_color(&spell);
        let seed = nb.cast.seed as u64;
        let owner = nb.cast.caster;
        // A new phalanx or a new blade in the hand takes the old one's place; spirit blades
        // past six go oldest first.
        if let Some(o) = owner {
            let mut mine: Vec<(Entity, f32)> = existing
                .iter()
                .filter(|(_, b)| {
                    b.owner == Some(o)
                        && b.fading.is_none()
                        && match nb.shape {
                            BladeShape::Phalanx => b.flight == Flight::Arch,
                            BladeShape::Bound => matches!(b.flight, Flight::Held { .. }),
                            BladeShape::Spirit => matches!(b.flight, Flight::Spirit { .. }),
                            BladeShape::Fall => false,
                        }
                })
                .map(|(e, b)| (e, b.age))
                .collect();
            mine.sort_by(|a, b| b.1.total_cmp(&a.1));
            let keep = if nb.shape == BladeShape::Spirit {
                6usize.saturating_sub(n as usize)
            } else {
                0
            };
            for (e, _) in mine.into_iter().skip(keep) {
                commands.entity(e).try_insert(Expire);
            }
        }
        let (from, rot) = owner
            .and_then(|o| owners.get(o).ok())
            .map(|(g, _)| (g.translation(), g.rotation()))
            .unwrap_or((nb.body, yaw_rot(nb.facing)));
        let base = |flight: Flight, pos: DVec3, rot: Quat, life: f32, strike: Arc<Spell>| Blade {
            cast: nb.cast.clone(),
            owner,
            flight,
            pos,
            rot,
            vel: Vec3::ZERO,
            age: 0.0,
            life,
            length,
            color,
            slot: (0, n),
            target: nb.target,
            look_in: 0.0,
            cooldown: 0.0,
            cuts: 0,
            strike,
            fading: None,
            before: None,
        };
        match nb.shape {
            BladeShape::Phalanx => {
                let loose = loose_of(&spell);
                for i in 0..n {
                    // They rise out of the caster's hand into the arch.
                    let mut b = base(Flight::Arch, nb.hand, rot, p.blade_secs, loose.clone());
                    b.slot = (i, n);
                    b.cooldown = 0.25 + 0.05 * i as f32;
                    spawn_blade(&mut commands, &mut queue, b, mesh, "Phalanx blade", 0.35);
                }
            }
            BladeShape::Fall => {
                let start = nb.body + (nb.facing * 2.5).as_dvec3();
                let cut = Arc::new(spell.as_ref().clone());
                let fall = Quat::from_rotation_arc(Vec3::Y, Vec3::NEG_Y);
                for (i, spot) in fall_spots(start, nb.aim, n, seed).into_iter().enumerate() {
                    let twist = Quat::from_rotation_y(roll(seed, 300 + i as u64) * PI);
                    let mut b = base(
                        Flight::Waiting {
                            delay: FALL_EVERY * i as f32,
                            ground: spot,
                        },
                        spot + DVec3::Y * FALL_FROM_M as f64,
                        twist * fall,
                        FALL_EVERY * n as f32 + STUCK_SECS + 1.0,
                        cut.clone(),
                    );
                    b.slot = (i as u32, n);
                    spawn_blade(&mut commands, &mut queue, b, mesh, "Falling blade", 0.0);
                }
            }
            BladeShape::Spirit => {
                let cut = cut_of(&spell, 0.45);
                for i in 0..n {
                    let side = if (i + (seed as u32 & 1)).is_multiple_of(2) {
                        1.0
                    } else {
                        -1.0
                    };
                    let at = from
                        + (rot * Vec3::new(side * (0.9 + 0.4 * (i / 2) as f32), 1.5, 0.2))
                            .as_dvec3();
                    let mut b = base(
                        Flight::Spirit { swing: None },
                        nb.hand,
                        rot,
                        p.blade_secs,
                        cut.clone(),
                    );
                    b.slot = (i, n);
                    b.pos = nb.hand + (at - nb.hand) * 0.2;
                    b.look_in = 0.1 * i as f32;
                    let health = (spell.power.damage() * SPIRIT_TOUGHNESS).max(40.0) * length;
                    let e = spawn_blade(&mut commands, &mut queue, b, mesh, "Spirit blade", 0.45);
                    let mut caster = MagicCaster {
                        learns: false,
                        side: nb.cast.side,
                        mana: 0.0,
                        max_mana: 0.0,
                        mana_regen: 0.0,
                        hand: Vec3::ZERO,
                        ..MagicCaster::default()
                    };
                    caster.seed = seed.wrapping_add(i as u64 * 7919);
                    commands
                        .entity(e)
                        .insert((caster, Vitality::new(health), spirit_body(length)));
                    if let Some(f) = nb.cast.faction {
                        commands.entity(e).insert(f);
                    }
                }
            }
            BladeShape::Bound => {
                let Some(o) = owner else { continue };
                let before = owners.get(o).ok().and_then(|(_, l)| l.copied());
                let two_handed = length >= 1.35 && n == 1;
                let main = if two_handed {
                    ItemClass::TwoHandedBlade
                } else {
                    ItemClass::OneHandedBlade
                };
                commands.entity(o).try_insert(Loadout {
                    main,
                    off: if n >= 2 {
                        ItemClass::OneHandedBlade
                    } else {
                        before.map(|l| l.off).unwrap_or(ItemClass::None)
                    },
                    two_handed,
                });
                // The blade's spell rides on what the hand strikes, as an imbue does, a little
                // under a thrown blade's cut: the weapon's own blow is Combat's.
                let ride = CastInfo {
                    spell: cut_of(&spell, 0.4),
                    ..nb.cast.clone()
                };
                commands
                    .entity(o)
                    .try_insert(Imbued::new(ride, p.blade_secs, true));
                for i in 0..n.min(2) {
                    let mut b = base(
                        Flight::Held { off: i == 1 },
                        nb.hand,
                        rot,
                        p.blade_secs,
                        cut_of(&spell, 0.4),
                    );
                    b.slot = (i, n);
                    if i == 0 {
                        b.before = Some(before);
                    }
                    spawn_blade(&mut commands, &mut queue, b, mesh, "Bound blade", 0.5);
                }
            }
        }
    }
}

/// Marks a blade to fade out now (replaced, or its holder gone).
#[derive(Component, Debug, Clone, Copy)]
pub struct Expire;

fn spawn_blade(
    commands: &mut Commands,
    queue: &mut MagicQueue,
    b: Blade,
    mesh: Option<Handle<Mesh>>,
    name: &'static str,
    aura: f32,
) -> Entity {
    let look = blade_look(&b.cast.spell);
    let woven = !b.cast.spell.weave.elements.is_empty();
    let color = b.color;
    let hidden = matches!(b.flight, Flight::Waiting { .. });
    let mut ec = commands.spawn((
        Name::new(name),
        Transform::from_translation(b.pos)
            .with_rotation(b.rot)
            .with_scale(Vec3::splat(b.length)),
        NoVirtualGeometry,
    ));
    if let Some(mesh) = mesh {
        let mut r = blade_renderer(mesh, color, BLADE_GLOW);
        r.visible = !hidden;
        ec.insert(r);
    }
    // A blade that lasts lights what is round it (a bladefall's twenty don't).
    if !matches!(b.flight, Flight::Waiting { .. }) {
        ec.insert(PointLight {
            color,
            intensity_lm: BLADE_LIGHT_LM,
            range: 4.0,
            radius: 0.05,
        });
    }
    let e = ec.insert(b).id();
    // A woven blade wears its element: flames lick a fire blade, frost smokes off an ice one.
    if woven && aura > 0.0 {
        queue.looks.push(LookCmd::Attach {
            entity: e,
            look,
            role: Role::Aura,
            scale: aura,
            color,
            power_j: 0.0,
        });
    }
    e
}

/// Fly every conjured blade: the phalanx keeps its arch and looses at foes, the bladefall
/// drops and sticks, spirit blades fight, blades in the hand keep their time; each fades at the
/// end of it (a spirit blade cut down breaks into shards).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_blades(
    time: Res<Time>,
    factions: Option<Res<Factions>>,
    mut blades: Query<(
        Entity,
        &mut Blade,
        &mut Transform,
        Option<&Vitality>,
        Has<Expire>,
        Option<&mut MeshRenderer>,
    )>,
    owners: Query<(&GlobalTransform, Option<&Vitality>, Option<&SkeletonPose>)>,
    bodies: Query<Bodies, CreatureFilter>,
    mut hits: ResMut<HitQueue>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
    (mut wm, mut meshes): (ResMut<WardMeshes>, Option<ResMut<Assets<Mesh>>>),
    mut next_loose: Local<Vec<(Entity, f32)>>,
) {
    if blades.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let factions = factions.as_deref();
    // A phalanx looses one blade every [`PHALANX_EVERY`] s, in turn: seconds before each
    // caster's next.
    next_loose.retain_mut(|(_, t)| {
        *t -= dt;
        *t > 0.0
    });
    for (e, mut b, mut tf, vit, expire, shown) in &mut blades {
        b.age += dt;
        b.cooldown -= dt;
        let owner = b.owner.and_then(|o| owners.get(o).ok());
        let owner_gone = b.owner.is_some() && owner.is_none_or(|o| o.1.is_some_and(|v| v.down()));
        // A spirit blade cut down breaks.
        if vit.is_some_and(|v| v.down()) {
            break_blade(
                &mut commands,
                &mut wm,
                meshes.as_deref_mut(),
                &mut queue,
                e,
                &b,
            );
            continue;
        }
        let held = matches!(b.flight, Flight::Held { .. });
        if b.fading.is_none() && (b.age >= b.life || expire || (owner_gone && !b.is_falling())) {
            b.fading = Some(0.0);
            queue.looks.push(LookCmd::Stop { entity: e });
            if held && let Some(before) = b.before.take() {
                restore_loadout(&mut commands, b.owner, before, owner_gone);
            }
        }
        if let Some(f) = b.fading.as_mut() {
            *f += dt;
            if *f >= FADE_SECS {
                commands.entity(e).despawn();
                continue;
            }
        }
        let fading = b.fading.is_some();
        let flight = b.flight;
        match flight {
            Flight::Arch => {
                let Some((og, ..)) = owner else {
                    continue;
                };
                let (yaw, ..) = og.rotation().to_euler(EulerRot::YXZ);
                let frame = Quat::from_rotation_y(yaw);
                let (slot, lean) = arch_slot(b.slot.0, b.slot.1, b.length);
                // Each blade bobs a little on its own.
                let bob = 0.04 * (b.age * 2.1 + b.slot.0 as f32 * 1.7).sin();
                let goal = og.translation() + (frame * (slot + Vec3::Y * bob)).as_dvec3();
                let k = (1.0 - (-9.0 * dt).exp()) as f64;
                let at = b.pos;
                b.pos = at + (goal - at) * k;
                b.rot = b
                    .rot
                    .slerp(frame * lean, (1.0 - (-8.0 * dt).exp()).min(1.0));
                // A foe near: loose this blade at it, one per caster each step, in turn.
                let owner_e = b.owner.unwrap_or(e);
                if !fading
                    && b.cooldown <= 0.0
                    && b.age > 0.4
                    && !next_loose.iter().any(|(o, _)| *o == owner_e)
                {
                    let near = nearest_foe(
                        &b.cast,
                        og.translation(),
                        PHALANX_REACH_M,
                        e,
                        &bodies,
                        factions,
                        false,
                    );
                    if let Some((foe, at)) = near {
                        next_loose.push((owner_e, PHALANX_EVERY));
                        let from = b.pos + (b.rot * Vec3::Y * b.length * 0.5).as_dvec3();
                        let aim = at + DVec3::Y * 1.1;
                        let dir = (aim - from).as_vec3().normalize_or(Vec3::NEG_Z);
                        queue.spawns.push(crate::runtime::Spawn::Missile(
                            Missile {
                                cast: CastInfo {
                                    spell: b.strike.clone(),
                                    seed: b.cast.seed.wrapping_add(b.slot.0 * 7919),
                                    ..b.cast.clone()
                                },
                                vel: dir * LOOSE_SPEED,
                                traveled: 0.0,
                                age: 0.0,
                                hits: Vec::new(),
                                pierce_left: 0,
                                bounce_left: 0,
                                target: Some(foe),
                                returning: false,
                                orbit: None,
                                forked: false,
                                spent: None,
                            },
                            from,
                        ));
                        queue.looks.push(LookCmd::Stop { entity: e });
                        commands.entity(e).despawn();
                        continue;
                    }
                }
            }
            Flight::Waiting { delay, ground } => {
                if b.age >= delay {
                    b.flight = Flight::Falling { ground };
                    b.vel = Vec3::NEG_Y * FALL_SPEED;
                    if let Some(mut r) = shown {
                        r.visible = true;
                    }
                }
            }
            Flight::Falling { ground } => {
                let step = (FALL_SPEED * dt) as f64;
                // The grip comes to rest a little over a third of the blade above the ground:
                // the point sinks in.
                let rest = ground.y + (b.length * 0.62) as f64;
                b.pos.y -= step;
                if b.pos.y <= rest {
                    b.pos.y = rest;
                    b.flight = Flight::Stuck { t: 0.0 };
                    b.life = b.age + STUCK_SECS;
                    for (t, g, v, al, _) in &bodies {
                        let d = g.translation() - ground;
                        let flat = (d.x * d.x + d.z * d.z).sqrt();
                        if Some(t) == b.owner
                            || flat > FALL_CUT_M as f64
                            || d.y.abs() > 2.0
                            || v.is_some_and(|v| v.down())
                            || (b.cast.spell.payload.harmful()
                                && b.cast.spell.safeguard
                                && b.cast.friend(t, al.caster, al.faction, factions))
                        {
                            continue;
                        }
                        hits.0.push(PendingHit {
                            cast: CastInfo {
                                spell: b.strike.clone(),
                                ..b.cast.clone()
                            },
                            target: Some(t),
                            point: ground + DVec3::Y * 0.9,
                            normal: Vec3::Y,
                            dir: Vec3::NEG_Y,
                            multi: true,
                            impact: false,
                            reacts: true,
                            expire: false,
                        });
                    }
                    // A small burst of its light where it bites, every other blade, so twenty
                    // of them don't fill the floor with dust and ice (the first film did).
                    if b.slot.0.is_multiple_of(2) {
                        queue.looks.push(LookCmd::Burst {
                            at: ground + DVec3::Y * 0.15,
                            up: Vec3::Y,
                            look: blade_look(&b.cast.spell),
                            role: Role::Impact,
                            scale: 0.16 * b.length,
                            color: b.color,
                        });
                    }
                }
            }
            Flight::Stuck { t } => {
                b.flight = Flight::Stuck { t: t + dt };
            }
            Flight::Spirit { swing } => {
                let Some((og, ..)) = owner else {
                    continue;
                };
                let home = og.translation();
                b.look_in -= dt;
                if b.look_in <= 0.0 || b.target.is_some_and(|t| bodies.get(t).is_err()) {
                    b.look_in = 0.4;
                    // Creatures first, as the dead go for summons: a spirit blade fences
                    // with the foe's creatures anywhere within its leash, and with a body that
                    // isn't one only when it comes near its caster (it guards, it doesn't go
                    // hunting their master).
                    b.target =
                        nearest_foe(&b.cast, b.pos, SPIRIT_SEE_M, e, &bodies, factions, true)
                            .filter(|(f, at)| {
                                let creature = bodies.get(*f).is_ok_and(|x| x.4);
                                at.distance(home)
                                    <= if creature {
                                        SPIRIT_LEASH_M
                                    } else {
                                        SPIRIT_GUARD_REACH_M
                                    } as f64
                            })
                            .map(|(f, _)| f);
                }
                let foe = b
                    .target
                    .and_then(|t| bodies.get(t).ok())
                    .filter(|f| !f.2.is_some_and(|v| v.down()))
                    .map(|f| (f.0, f.1.translation() + DVec3::Y * 1.15));
                let (yaw, ..) = og.rotation().to_euler(EulerRot::YXZ);
                let frame = Quat::from_rotation_y(yaw);
                let side = if b.slot.0.is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                };
                let bob = 0.07 * (b.age * 1.9 + b.slot.0 as f32).sin();
                let (goal, point_dir) = match (foe, swing) {
                    (Some((_, chest)), Some(t)) => {
                        // Lunge through the cut.
                        let to = (chest - b.pos).as_vec3();
                        let k = (t / SWING_SECS).clamp(0.0, 1.0);
                        let lunge = (PI * k).sin() * 0.55;
                        (
                            chest
                                - (to.normalize_or(Vec3::Z) * (SPIRIT_GUARD_M - lunge)).as_dvec3(),
                            swing_point(to, k, b.cuts),
                        )
                    }
                    (Some((_, chest)), None) => {
                        // En garde: on the caster's side of the foe, each blade of a pair off
                        // to its own flank, point at it, swaying round it a little.
                        let back = (home - chest).as_vec3();
                        let back = Vec3::new(back.x, 0.0, back.z).normalize_or(Vec3::Z);
                        let flank = side * (0.75 + 0.3 * (b.slot.0 / 2) as f32)
                            + 0.35 * (b.age * 0.7 + b.slot.0 as f32 * 2.1).sin();
                        let round = Quat::from_rotation_y(flank) * back;
                        (
                            chest + (round * SPIRIT_GUARD_M + Vec3::Y * (0.25 + bob)).as_dvec3(),
                            (chest - b.pos).as_vec3().normalize_or(Vec3::Y),
                        )
                    }
                    _ => (
                        home + (frame
                            * Vec3::new(
                                side * (0.95 + 0.4 * (b.slot.0 / 2) as f32),
                                1.55 + bob,
                                0.15,
                            ))
                        .as_dvec3(),
                        (frame * Vec3::new(side * 0.15, 1.0, -0.25)).normalize(),
                    ),
                };
                let to = (goal - b.pos).as_vec3();
                let pace = if swing.is_some() {
                    SPIRIT_PACE * 1.6
                } else {
                    SPIRIT_PACE
                };
                let want = (to * 5.0).clamp_length_max(pace);
                b.vel = b.vel.lerp(want, (1.0 - (-10.0 * dt).exp()).min(1.0));
                let step = (b.vel * dt).as_dvec3();
                b.pos += step;
                let want_rot = Quat::from_rotation_arc(Vec3::Y, point_dir);
                let turn = if swing.is_some() { 30.0 } else { 9.0 };
                b.rot = b.rot.slerp(want_rot, (1.0 - (-turn * dt).exp()).min(1.0));
                // Cut: when near, start a sweep; land it mid-sweep.
                let mut next = swing;
                if let Some((t_e, chest)) = foe {
                    match swing {
                        None => {
                            if !fading
                                && b.cooldown <= 0.0
                                && b.pos.distance(chest) <= (SPIRIT_GUARD_M + 0.6) as f64
                            {
                                next = Some(0.0);
                            }
                        }
                        Some(t) => {
                            let t2 = t + dt;
                            if t < SWING_HIT && t2 >= SWING_HIT {
                                let dir = (chest - b.pos).as_vec3().normalize_or(Vec3::NEG_Z);
                                hits.0.push(PendingHit {
                                    cast: CastInfo {
                                        spell: b.strike.clone(),
                                        caster: Some(e),
                                        seed: b.cast.seed.wrapping_add(b.cuts * 7919),
                                        ..b.cast.clone()
                                    },
                                    target: Some(t_e),
                                    point: chest,
                                    normal: -dir,
                                    dir,
                                    multi: false,
                                    impact: true,
                                    reacts: true,
                                    expire: false,
                                });
                            }
                            if t2 >= SWING_SECS {
                                next = None;
                                b.cuts += 1;
                                b.cooldown = SPIRIT_REST;
                            } else {
                                next = Some(t2);
                            }
                        }
                    }
                } else {
                    next = None;
                }
                b.flight = Flight::Spirit { swing: next };
            }
            Flight::Held { .. } => {
                // Posed in `hold_blades` (after the animation), here only its time is kept.
                if owner.is_none() {
                    continue;
                }
            }
        }
        if !held {
            tf.translation = b.pos;
            tf.rotation = b.rot;
        }
        // It fades by thinning to a line of light and drawing back into its hilt.
        let left = match b.fading {
            Some(f) => (1.0 - f / FADE_SECS).max(0.02),
            None => 1.0,
        };
        let l = b.length;
        tf.scale = Vec3::new(l * left, l * (0.4 + 0.6 * left), l * left);
    }
}

impl Blade {
    fn is_falling(&self) -> bool {
        matches!(
            self.flight,
            Flight::Waiting { .. } | Flight::Falling { .. } | Flight::Stuck { .. }
        )
    }
}

fn restore_loadout(
    commands: &mut Commands,
    owner: Option<Entity>,
    before: Option<Loadout>,
    gone: bool,
) {
    let Some(o) = owner else { return };
    if gone {
        return;
    }
    let Ok(mut ec) = commands.get_entity(o) else {
        return;
    };
    match before {
        Some(l) => {
            ec.try_insert(l);
        }
        None => {
            ec.try_remove::<Loadout>();
        }
    }
}

/// What blades look at: a body, where, its health, its side, and whether it is a creature.
type Bodies = (
    Entity,
    &'static GlobalTransform,
    Option<&'static Vitality>,
    Allegiance,
    Has<Minion>,
);

/// The nearest body `cast` would strike within `reach` of `at`, not `me` (a creature before any
/// other body, with `creatures_first`).
#[allow(clippy::too_many_arguments)]
fn nearest_foe(
    cast: &CastInfo,
    at: DVec3,
    reach: f32,
    me: Entity,
    bodies: &Query<Bodies, CreatureFilter>,
    factions: Option<&Factions>,
    creatures_first: bool,
) -> Option<(Entity, DVec3)> {
    bodies
        .iter()
        .filter(|(b, g, v, al, _)| {
            *b != me
                && Some(*b) != cast.caster
                && !v.is_some_and(|v| v.down())
                && g.translation().distance(at) <= reach as f64
                && !cast.friend(*b, al.caster, al.faction, factions)
        })
        .min_by(|a, b| {
            (creatures_first && !a.4)
                .cmp(&(creatures_first && !b.4))
                .then(
                    a.1.translation()
                        .distance(at)
                        .total_cmp(&b.1.translation().distance(at)),
                )
                .then(a.0.cmp(&b.0))
        })
        .map(|(b, g, ..)| (b, g.translation()))
}

/// A blade breaks: shards of its light, a flash, and it is gone.
fn break_blade(
    commands: &mut Commands,
    wm: &mut WardMeshes,
    meshes: Option<&mut Assets<Mesh>>,
    queue: &mut MagicQueue,
    e: Entity,
    b: &Blade,
) {
    let mid = b.pos + (b.rot * Vec3::Y * b.length * 0.5).as_dvec3();
    if let Some(meshes) = meshes {
        glass_burst(
            commands,
            wm,
            meshes,
            mid - DVec3::Y * 0.35,
            0.7,
            b.color,
            BREAK_SHARDS,
            e.to_bits() ^ b.cast.seed as u64,
        );
    }
    queue.looks.push(LookCmd::Stop { entity: e });
    queue.looks.push(LookCmd::Burst {
        at: mid,
        up: Vec3::Y,
        look: blade_look(&b.cast.spell),
        role: Role::Impact,
        scale: 0.45,
        color: b.color,
    });
    if let (Some(before), Some(o)) = (b.before, b.owner) {
        restore_loadout(commands, Some(o), before, false);
    }
    commands.entity(e).despawn();
}

/// Glintblades: draw the sword on each blade missile, and break it into shards when it is
/// spent.
#[allow(clippy::type_complexity)]
pub fn ride_blades(
    mut commands: Commands,
    mut rides: Query<(Entity, &mut BladeRide, &Missile, &GlobalTransform)>,
    mut shown: Query<&mut MeshRenderer>,
    (mut bm, mut wm, mut meshes): (
        ResMut<BladeMeshes>,
        ResMut<WardMeshes>,
        Option<ResMut<Assets<Mesh>>>,
    ),
) {
    for (e, mut r, m, g) in &mut rides {
        if r.drawn.is_none()
            && m.spent.is_none()
            && let Some(meshes) = meshes.as_deref_mut()
        {
            let mesh = bm.blade(meshes);
            // The missile's +Y is its flight: the blade's middle on the missile, point ahead.
            let child = commands
                .spawn((
                    Name::new("Glintblade"),
                    Transform::from_translation(DVec3::NEG_Y * (r.length * 0.55) as f64)
                        .with_scale(Vec3::splat(r.length)),
                    blade_renderer(mesh, r.color, BLADE_GLOW),
                    NoVirtualGeometry,
                    ChildOf(e),
                ))
                .id();
            r.drawn = Some(child);
        }
        if m.spent.is_some() && !r.broke {
            r.broke = true;
            if let Some(c) = r.drawn
                && let Ok(mut s) = shown.get_mut(c)
            {
                s.visible = false;
            }
            if let Some(meshes) = meshes.as_deref_mut() {
                glass_burst(
                    &mut commands,
                    &mut wm,
                    meshes,
                    g.translation() - DVec3::Y * 0.3,
                    0.6,
                    r.color,
                    BREAK_SHARDS,
                    e.to_bits() ^ m.cast.seed as u64,
                );
            }
        }
    }
}

/// Blades in the hand: posed on the hand after the animation (the hand bone and the line
/// across the knuckles give the grip; with no pose, at the cast hand, point forward and
/// down), and lights on imbued hands.
#[allow(clippy::type_complexity)]
pub fn hold_blades(
    mut blades: Query<(&Blade, &mut Transform), Without<HandGlow>>,
    mut glows: Query<(&HandGlow, &mut Transform), Without<Blade>>,
    owners: Query<
        (&Transform, Option<&SkeletonPose>, Option<&MagicCaster>),
        (Without<Blade>, Without<HandGlow>),
    >,
) {
    // A body is a root: its transform now is where it stands this frame (its global one is
    // still last frame's until propagation).
    for (b, mut tf) in &mut blades {
        let Flight::Held { off } = b.flight else {
            continue;
        };
        let Some((t, pose, caster)) = b.owner.and_then(|o| owners.get(o).ok()) else {
            continue;
        };
        let g = GlobalTransform::from(*t);
        let (grip, point) = grip_of(&g, pose, caster, off);
        // The grip is a fifth of the way up the blade from its pommel.
        tf.translation = grip - (point * b.length * 0.12).as_dvec3();
        tf.rotation = Quat::from_rotation_arc(Vec3::Y, point);
    }
    for (h, mut tf) in &mut glows {
        let Ok((t, pose, caster)) = owners.get(h.owner) else {
            continue;
        };
        let g = GlobalTransform::from(*t);
        let off = caster.is_some_and(|c| c.off_hand);
        tf.translation = grip_of(&g, pose, caster, off).0;
    }
}

/// Where a hand holds a blade and which way its point goes: from the pose, the hand bone and
/// the line from the little finger's knuckle to the index finger's (a hammer grip: the blade
/// leaves the fist on the thumb's side); else the cast hand, point forward and down.
pub fn grip_of(
    g: &GlobalTransform,
    pose: Option<&SkeletonPose>,
    caster: Option<&MagicCaster>,
    off: bool,
) -> (DVec3, Vec3) {
    let s = if off { "l" } else { "r" };
    if let Some(p) = pose {
        let bone = |n: &str| {
            p.index_of(&format!("{n}_{s}"))
                .and_then(|i| p.bone_world(g, i))
                .map(|b| b.0)
        };
        if let Some(hand) = bone("hand") {
            let fwd = g.rotation() * Vec3::NEG_Z;
            let point = match (bone("index_01"), bone("pinky_01")) {
                (Some(i), Some(k)) => {
                    let across = (i - k).as_vec3().normalize_or(fwd);
                    // Held a little forward of the knuckle line, as a relaxed wrist does.
                    (across + fwd * 0.6).normalize_or(fwd)
                }
                _ => (fwd + Vec3::NEG_Y * 0.7).normalize(),
            };
            let palm = match bone("middle_01") {
                Some(m) => hand.lerp(m, 0.7),
                None => hand,
            };
            return (palm, point);
        }
    }
    let h = caster.map(|c| c.hand).unwrap_or(Vec3::new(0.25, 1.1, -0.3));
    let h = if off { Vec3::new(-h.x, h.y, h.z) } else { h };
    let at = g.translation() + (g.rotation() * Vec3::new(h.x, 1.0, -0.15)).as_dvec3();
    let fwd = g.rotation() * Vec3::NEG_Z;
    (at, (fwd + Vec3::NEG_Y * 0.55).normalize())
}

/// Imbued weapons strike: a melee blow from an imbued body lands its spell on the struck body
/// too. A blow is a `DamageEvent` from it of kind Slash, Pierce or Blunt: any from a fighting
/// creature (a golem's fists, a skeleton's blade, a spirit blade's cut); from anyone else only
/// the ones no spell of theirs made (Movement's, NPC AI's or Combat's melee), so a caster's own
/// stone or force bolt doesn't set it off.
pub fn strike_imbued(
    time: Res<Time>,
    mut damage: MessageReader<DamageEvent>,
    mut spells: MessageReader<crate::hits::SpellHit>,
    mut imbued: Query<(&mut Imbued, Option<&Minion>, Has<Blade>)>,
    mut hits: ResMut<HitQueue>,
) {
    let dt = time.delta_secs();
    for (mut im, ..) in &mut imbued {
        im.every.retain_mut(|(_, t)| {
            *t -= dt;
            *t > 0.0
        });
    }
    let by_spell: Vec<(Option<Entity>, Option<Entity>)> =
        spells.read().map(|h| (h.caster, h.target)).collect();
    for ev in damage.read() {
        let Some(src) = ev.source else { continue };
        if src == ev.target
            || !matches!(
                ev.kind,
                DamageKind::Slash | DamageKind::Pierce | DamageKind::Blunt
            )
        {
            continue;
        }
        let Ok((mut im, minion, is_blade)) = imbued.get_mut(src) else {
            continue;
        };
        let fighter = is_blade
            || minion.is_some_and(|m| {
                matches!(
                    m.kind,
                    crate::summon::MinionKind::Golem | crate::summon::MinionKind::Skeleton
                )
            });
        let spell_made = by_spell.contains(&(Some(src), Some(ev.target)));
        if (!fighter && spell_made)
            || (minion.is_some() && !fighter)
            || im.left() <= 0.0
            || im.every.iter().any(|(t, _)| *t == ev.target)
        {
            continue;
        }
        im.every.push((ev.target, IMBUE_EVERY));
        let dir = ev.impulse.normalize_or(Vec3::NEG_Z);
        hits.0.push(PendingHit {
            cast: CastInfo {
                caster: Some(src),
                depth: 1,
                ..im.cast.clone()
            },
            target: Some(ev.target),
            point: ev.point,
            normal: -dir,
            dir,
            multi: true,
            impact: true,
            reacts: true,
            expire: false,
        });
    }
}

/// An imbued creature's weapon: the element's colour, glowing.
pub const IMBUE_GLOW: f32 = 12_000.0;

/// Imbued weapons glow: a creature's blade or fists take the element's colour and wear its
/// look, a spirit blade takes its colour, a body's hand carries a light and the look; when the
/// imbue ends they go back.
#[allow(clippy::type_complexity)]
pub fn tend_imbued(
    time: Res<Time>,
    mut commands: Commands,
    mut imbued: Query<(Entity, &mut Imbued, Option<&Minion>, Has<Blade>)>,
    mut pieces: Query<&mut MeshRenderer>,
    parts: Query<&MinionPart>,
    glows: Query<(Entity, &HandGlow)>,
    mut queue: ResMut<MagicQueue>,
) {
    let dt = time.delta_secs();
    for (e, mut im, minion, is_blade) in &mut imbued {
        im.age += dt;
        let key = blade_look(&im.cast.spell);
        let color = im.cast.spell.weave.color;
        let ended = im.age >= im.secs;
        let redraw = im.drawn.as_ref().is_some_and(|(_, k)| *k != key);
        if (ended || redraw)
            && let Some((on, _)) = im.drawn.take()
        {
            for p in on {
                queue.looks.push(LookCmd::Stop { entity: p });
            }
            for (p, m) in std::mem::take(&mut im.restore) {
                if let Ok(mut r) = pieces.get_mut(p) {
                    r.material = m;
                }
            }
            for (g, h) in &glows {
                if h.owner == e {
                    commands.entity(g).despawn();
                }
            }
        }
        if ended {
            commands.entity(e).try_remove::<Imbued>();
            continue;
        }
        if im.drawn.is_some() || im.from_blade {
            continue;
        }
        let mut on = Vec::new();
        let mut aura = 0.35;
        if let Some(m) = minion {
            let strikes = crate::summon::weapon_parts(m.kind);
            if m.kind == crate::summon::MinionKind::Golem {
                aura = 0.55 * m.size;
            }
            for &p in &m.parts {
                if !parts.get(p).is_ok_and(|mp| strikes.contains(&mp.index)) {
                    continue;
                }
                if let Ok(mut r) = pieces.get_mut(p) {
                    im.restore.push((p, r.material.clone()));
                    r.material = MaterialRef::Inline(
                        StandardMaterial::default()
                            .with_tint(color[0], color[1], color[2])
                            .with_emissive(color, IMBUE_GLOW),
                    );
                }
                on.push(p);
            }
        } else if is_blade {
            // A spirit blade takes the imbue's colour.
            if let Ok(mut r) = pieces.get_mut(e) {
                im.restore.push((e, r.material.clone()));
                r.material =
                    MaterialRef::Inline(ward_material(color, crate::wards::glow_step(BLADE_GLOW)));
            }
            on.push(e);
        } else {
            // A body: its hand glows (posed on the hand in `hold_blades`).
            let h = commands
                .spawn((
                    Name::new("Imbued hand"),
                    Transform::default(),
                    HandGlow { owner: e },
                    PointLight {
                        color,
                        intensity_lm: 900.0,
                        range: 3.5,
                        radius: 0.05,
                    },
                ))
                .id();
            on.push(h);
        }
        for &p in &on {
            queue.looks.push(LookCmd::Attach {
                entity: p,
                look: key.clone(),
                role: Role::Aura,
                scale: aura,
                color,
                power_j: 0.0,
            });
        }
        im.drawn = Some((on, key));
    }
}

/// Carriers keep world positions of their own; move them when the world re-anchors.
pub fn follow_world_shift(mut shifts: MessageReader<WorldShift>, mut q: Query<&mut Blade>) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    for mut b in &mut q {
        b.pos = s.point(b.pos);
        b.vel = s.direction(b.vel);
        b.flight = match b.flight {
            Flight::Waiting { delay, ground } => Flight::Waiting {
                delay,
                ground: s.point(ground),
            },
            Flight::Falling { ground } => Flight::Falling {
                ground: s.point(ground),
            },
            f => f,
        };
    }
}

/// Every conjured blade gone, every imbue off and every hand given back (the hall's restart).
pub fn clear(world: &mut World) {
    let mut q = world.query::<(Entity, &Blade)>();
    let gone: Vec<(Entity, Option<Entity>, Option<Option<Loadout>>)> =
        q.iter(world).map(|(e, b)| (e, b.owner, b.before)).collect();
    for (e, owner, before) in gone {
        if let (Some(o), Some(before)) = (owner, before)
            && let Ok(mut ec) = world.get_entity_mut(o)
        {
            match before {
                Some(l) => {
                    ec.insert(l);
                }
                None => {
                    ec.remove::<Loadout>();
                }
            }
        }
        world.despawn(e);
    }
    let mut q = world.query_filtered::<Entity, With<HandGlow>>();
    let glows: Vec<Entity> = q.iter(world).collect();
    for e in glows {
        world.despawn(e);
    }
    let mut q = world.query::<(Entity, &Imbued)>();
    let imbued: Vec<(Entity, Vec<(Entity, MaterialRef)>)> = q
        .iter(world)
        .map(|(e, im)| (e, im.restore.clone()))
        .collect();
    for (e, restore) in imbued {
        for (p, m) in restore {
            if let Some(mut r) = world.get_mut::<MeshRenderer>(p) {
                r.material = m;
            }
        }
        world.entity_mut(e).remove::<Imbued>();
    }
}

/// `magic.blades`: conjured blades and imbued bodies, for scripts and AI agents.
pub fn register(app: &mut App) {
    app.init_resource::<BladeMeshes>().register_api(
        "magic.blades",
        "Conjured blades now (phalanx, bladefall, spirit or bound: whose, where, seconds left, \
         whom they go for) and imbued bodies (the spell on their weapon, seconds left).",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Blade)>();
            let blades: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, b)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "kind": b.kind(),
                        "owner": b.owner.map(|o| o.to_bits()),
                        "at": [b.pos.x, b.pos.y, b.pos.z],
                        "left_s": b.left(),
                        "target": b.target.map(|o| o.to_bits()),
                        "length_m": b.length,
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Imbued)>();
            let imbued: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, im)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "spell": im.cast.spell.name,
                        "left_s": im.left(),
                        "from_blade": im.from_blade,
                    })
                })
                .collect();
            Ok(serde_json::json!({ "blades": blades, "imbued": imbued }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caster::MagicCaster;
    use crate::design::{Modifier, SpellDesign};
    use crate::schools::Mastery;

    fn spell(cast: CastType, mastery: Mastery, mods: &[Modifier]) -> Arc<Spell> {
        let mut d = SpellDesign::new(&["blade", "fire"], cast);
        for m in mods {
            d = d.with(*m);
        }
        MagicCaster::new(40.0, mastery).compile(&d, 0.0).unwrap()
    }

    #[test]
    fn the_sword_is_a_closed_list_of_triangles_a_unit_long() {
        let m = blade_mesh();
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        // The point at 1, its glow a little past it.
        let solid = |v: &&Vertex| v.color[3] > 0.0;
        let top = m
            .vertices
            .iter()
            .filter(solid)
            .map(|v| v.position[1])
            .fold(0.0, f32::max);
        assert!((top - 1.0).abs() < 1e-5);
        let glow = m.vertices.iter().map(|v| v.position[1]).fold(0.0, f32::max);
        assert!(glow > 1.0 && glow < 1.1);
        // The edges are bright, the flats between them dimmer.
        let alphas: Vec<f32> = m.vertices.iter().map(|v| v.color[3]).collect();
        assert!(alphas.iter().any(|&a| a < 0.5) && alphas.iter().any(|&a| a >= 1.0));
    }

    #[test]
    fn the_arch_runs_from_shoulder_over_the_head_to_shoulder() {
        let n = 5;
        let slots: Vec<Vec3> = (0..n).map(|i| arch_slot(i, n, 1.0).0).collect();
        assert!(slots[0].x < -0.5 && slots[4].x > 0.5);
        assert!(slots[2].x.abs() < 1e-5);
        assert!(slots[2].y > slots[0].y + 0.2, "highest over the head");
        assert!((slots[0].x + slots[4].x).abs() < 1e-5, "even each side");
        assert!(slots.iter().all(|s| s.y > 1.7));
    }

    #[test]
    fn a_bladefall_lands_along_its_line_widening_away() {
        let from = DVec3::new(0.0, 0.0, -2.0);
        let to = DVec3::new(0.0, 0.0, -12.0);
        let spots = fall_spots(from, to, 10, 7);
        assert_eq!(spots.len(), 10);
        assert!(spots[0].z > -3.0 && spots[9].z < -11.0);
        assert!(spots[0].x.abs() <= 0.36 && spots.iter().all(|s| s.x.abs() <= 1.26));
    }

    #[test]
    fn a_spirit_blade_cuts_from_a_new_angle_each_time_and_through_its_foe() {
        let to = Vec3::NEG_Z;
        let start = swing_point(to, 0.0, 0);
        let end = swing_point(to, 1.0, 0);
        assert!(start.y > 0.5, "raised");
        assert!(end.y < start.y - 0.8, "swept down through");
        let other = swing_point(to, 0.0, 1);
        assert!(other.dot(start) < 0.9, "another line");
    }

    #[test]
    fn the_cast_type_shapes_the_blades() {
        let bolt = spell(CastType::Bolt, Mastery::Expert, &[]);
        assert_eq!(bolt.payload.blades, 1);
        assert!(
            bolt.carrier.seek_turn > 0.0,
            "a glintblade turns toward its target"
        );
        assert!(bolt.payload.damage > 0.0);
        assert!((0.9..1.2).contains(&bolt.payload.blade_m));
        let phalanx = |m| spell(CastType::Orbit, m, &[]).payload.blades;
        assert_eq!(phalanx(Mastery::Adept), 2);
        assert_eq!(phalanx(Mastery::Expert), 3);
        assert_eq!(phalanx(Mastery::Master), 4);
        let orbit = spell(CastType::Orbit, Mastery::Expert, &[]);
        assert!((orbit.payload.blade_secs - 30.0).abs() < 1e-3);
        let fall = spell(CastType::Rain, Mastery::Expert, &[]);
        assert_eq!(fall.payload.blades, 10);
        let spirits = spell(
            CastType::Summon,
            Mastery::Expert,
            &[Modifier::Split { count: 3 }],
        );
        assert_eq!(spirits.payload.blades, 3);
        assert!((spirits.payload.blade_secs - 20.0).abs() < 1e-3);
        let bound = spell(
            CastType::OnSelf,
            Mastery::Expert,
            &[Modifier::Split { count: 4 }],
        );
        assert_eq!(bound.payload.blades, 2, "two hands at most");
        let great = spell(
            CastType::OnSelf,
            Mastery::Expert,
            &[Modifier::Size { factor: 2.5 }],
        );
        assert!(great.payload.blade_m >= 1.35, "{}", great.payload.blade_m);
        let imbue = spell(CastType::Imbue, Mastery::Expert, &[]);
        assert_eq!(imbue.payload.blades, 0);
        assert!((imbue.payload.imbue_secs - 30.0).abs() < 1e-3);
        assert_eq!(bolt.name, "Fire glintblade");
        assert_eq!(orbit.name, "Fire phalanx");
        assert_eq!(fall.name, "Fire bladefall");
        assert_eq!(spirits.name, "Fire spirit blade");
        assert_eq!(bound.name, "Bound fire blade");
        let plain = MagicCaster::new(40.0, Mastery::Expert)
            .compile(&SpellDesign::new(&["blade"], CastType::Orbit), 0.0)
            .unwrap();
        assert_eq!(plain.name, "Glintblade phalanx");
    }

    #[test]
    fn a_cut_strikes_one_foe_and_a_loosed_blade_flies_as_a_glintblade() {
        let s = spell(CastType::Summon, Mastery::Expert, &[]);
        let c = cut_of(&s, 0.5);
        assert_eq!(c.cast, CastType::Touch);
        assert_eq!(c.payload.blades, 0);
        assert!((c.payload.damage - s.payload.damage * 0.5).abs() < 1e-3);
        let l = loose_of(&spell(CastType::Orbit, Mastery::Expert, &[]));
        assert_eq!(l.cast, CastType::Bolt);
        assert_eq!(l.payload.blades, 1);
        assert_eq!(l.carrier.count, 1);
    }
}
