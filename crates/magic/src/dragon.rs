//! The bone dragon (round-18-plan.md): Bone cast as a Summon. Its bones fly up out of the floor
//! and lock together over [`ASSEMBLE_SECS`] (pelvis and spine first, then legs, tail, ribs, neck,
//! skull and wings, and last its heart rising into its ribcage and lighting); it rears and roars,
//! takes off, flies a circle over its foes and breathes grave frost down on them in a cone, lands
//! by them, bites and sweeps its tail, and flies again (World of Warcraft's frost wyrms, raised
//! from dead dragons' bones with the cold in their skeletal hearts; Skyrim's skeletal dragons).
//!
//! It is a minion (summon.rs) of [`PARTS`] pieces posed each step in its own frame (its middle at
//! the origin, facing -Z): no skeleton and no clips. Legs are two-bone IK to feet that walk a
//! four-beat gait on the floor and tuck in flight; the wings are two pieces each, an arm and a
//! hand of four fingers, with torn membrane (bone_art.rs) between, flapping with the hand lagging
//! the arm; the neck is a chain of vertebrae on a curve to the skull; the tail sways.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    MaterialRef, Mesh, MeshRef, MeshRenderer, PointLight, Shape, StandardMaterial,
};
use engine_world::physics::{
    BodyKind, Collider, ColliderShape, PhysicsQueries, QueryFilter, RigidBody,
};

use crate::bone_art as art;
use crate::bones::GRAVE_FROST;
use crate::compile::{CastContext, Spell, compile};
use crate::design::{CastType, SpellDesign, TargetFilter};
use crate::effects::ForceDir;
use crate::looks::{LookCmd, Role};
use crate::runtime::{CastInfo, HitQueue, MagicQueue, PendingHit, ground_below, release, roll};
use crate::summon::{Act, Minion, MinionPart, two_bone};

// ------------------------------------------------------------------------------- numbers

/// Seconds its bones take to fly together, and each bone's flight.
pub const ASSEMBLE_SECS: f32 = 2.6;
pub const FLY_SECS: f32 = 0.75;
/// Seconds it rears and roars once whole.
pub const REAR_SECS: f32 = 1.0;
/// Its middle's height over the floor standing, and flying, m (at size 1).
pub const STAND_H: f32 = 1.55;
pub const CRUISE_M: f32 = 5.4;
/// Its circle over its foes, m, and its speed on the wing, m/s.
pub const CIRCLE_M: f32 = 6.5;
pub const FLY_SPEED: f32 = 7.5;
/// Its walk, m/s, and turn on the floor, rad/s.
pub const WALK: f32 = 2.4;
pub const TURN: f32 = 2.2;
/// Seconds it fights on the floor before it flies again.
pub const GROUND_SECS: f32 = 6.0;
/// Its breath: how far it reaches, m, and a gout every so many seconds while it breathes.
pub const BREATH_M: f32 = 11.0;
pub const BREATH_EVERY: f32 = 0.22;
/// Where in its lap it breathes (its share of the circle).
pub const BREATH_FROM: f32 = 0.22;
pub const BREATH_TO: f32 = 0.62;
/// A bite: the whole of it and the moment the jaws close, s; its reach, m.
pub const BITE_SECS: f32 = 0.85;
pub const BITE_HIT: f32 = 0.5;
pub const BITE_M: f32 = 2.6;
/// A tail sweep: the whole of it and the moment it strikes, s; its reach, m.
pub const SWEEP_SECS: f32 = 1.25;
pub const SWEEP_HIT: f32 = 0.62;
pub const SWEEP_M: f32 = 3.6;
/// Health at size 1.
pub const HEALTH: f32 = 2000.0;
/// Its heart's glow and its eyes', nits.
pub const HEART_GLOW: f32 = 9_000.0;
pub const EYE_GLOW: f32 = 12_000.0;

// ------------------------------------------------------------------------------- pieces

const fn seq<const N: usize>(from: usize) -> [usize; N] {
    let mut out = [0; N];
    let mut i = 0;
    while i < N {
        out[i] = from + i;
        i += 1;
    }
    out
}

pub const PELVIS: usize = 0;
pub const CHEST: usize = 1;
/// Vertebrae from the shoulders to the hips.
pub const BACK: [usize; 6] = seq(2);
/// Vertebrae from the shoulders to the skull.
pub const NECK: [usize; 7] = seq(8);
pub const SKULL: usize = 15;
pub const JAW: usize = 16;
pub const HEART: usize = 17;
/// Ribs in pairs, front to back, left then right.
pub const RIBS: [usize; 10] = seq(18);
pub const TAIL: [usize; 11] = seq(28);
/// Upper leg, lower leg, foot; left then right.
pub const FORE: [usize; 2] = [39, 42];
pub const HIND: [usize; 2] = [45, 48];
/// A wing's arm and its hand of fingers; left then right.
pub const WING_ARM: [usize; 2] = [51, 52];
pub const WING_HAND: [usize; 2] = [53, 54];
pub const PARTS: usize = 55;

// Its build at size 1, in its frame (m).
const CHEST_P: Vec3 = Vec3::new(0.0, 0.08, -0.75);
const PELVIS_P: Vec3 = Vec3::new(0.0, 0.02, 0.75);
const NECK_BASE: Vec3 = Vec3::new(0.0, 0.2, -0.95);
const TAIL_BASE: Vec3 = Vec3::new(0.0, 0.04, 1.02);
const TAIL_SEG: f32 = 0.27;
const FORE_MOUNT: Vec3 = Vec3::new(0.34, -0.12, -0.62);
const HIND_MOUNT: Vec3 = Vec3::new(0.3, -0.06, 0.72);
const WING_ROOT: Vec3 = Vec3::new(0.26, 0.26, -0.58);
const FORE_UPPER: f32 = 0.72;
const FORE_LOWER: f32 = 0.7;
const HIND_THIGH: f32 = 0.74;
const HIND_SHIN: f32 = 0.74;
const ANKLE_Y: f32 = 0.09;
/// Where its skull's middle sits from where the neck meets it, in the skull's frame.
const SKULL_OFF: Vec3 = Vec3::new(0.0, 0.02, -0.15);
/// Its mouth from its skull's middle.
const MOUTH: Vec3 = Vec3::new(0.0, -0.1, -0.68);

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ramp(a: f32, b: f32, t: f32) -> f32 {
    smooth((t - a) / (b - a).max(1e-4))
}

fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

fn yaw_of(dir: Vec3) -> f32 {
    (-dir.x).atan2(-dir.z)
}

fn flat(d: DVec3) -> DVec3 {
    DVec3::new(d.x, 0.0, d.z)
}

/// A rib's curve at size 1 (the right one; its head on the spine at the origin), `len` its
/// length against the longest.
fn rib_curve(len: f32, side: f32) -> Vec<Vec3> {
    [
        (0.0, 0.0),
        (0.28, -0.03),
        (0.46, -0.2),
        (0.48, -0.46),
        (0.38, -0.7),
        (0.18, -0.84),
    ]
    .iter()
    .map(|&(x, y)| Vec3::new(x * side, y, 0.0) * len)
    .collect()
}

const RIB_LEN: [f32; 5] = [1.0, 1.06, 1.0, 0.86, 0.68];

// ------------------------------------------------------------------------------- the wings

/// The torn membranes of its wings, made once (lib.rs makes them at startup).
#[derive(Resource, Default)]
pub struct DragonMeshes {
    arm: [Option<Handle<Mesh>>; 2],
    hand: [Option<Handle<Mesh>>; 2],
}

impl DragonMeshes {
    pub fn ready(&mut self, meshes: &mut Assets<Mesh>) {
        for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
            if self.arm[i].is_none() {
                self.arm[i] = Some(meshes.add(art::arm_membrane(side, 11 + i as u32)));
            }
            if self.hand[i].is_none() {
                self.hand[i] = Some(meshes.add(art::hand_membrane(side, 23 + i as u32)));
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.arm.iter().chain(self.hand.iter()).all(Option::is_some)
    }
}

/// A wing's membrane waiting for its mesh: the arm's or the hand's, of the left (0) or right
/// (1) wing.
#[derive(Component, Debug, Clone, Copy)]
pub struct Membrane {
    pub hand: bool,
    pub side: usize,
}

/// Membranes get their torn meshes.
pub fn dress_dragons(
    mut q: Query<(&Membrane, &mut MeshRenderer), Added<Membrane>>,
    mut dm: ResMut<DragonMeshes>,
    meshes: Option<ResMut<Assets<Mesh>>>,
) {
    if q.is_empty() {
        return;
    }
    let Some(mut meshes) = meshes else {
        return;
    };
    dm.ready(&mut meshes);
    for (m, mut r) in &mut q {
        let h = if m.hand {
            dm.hand[m.side]
        } else {
            dm.arm[m.side]
        };
        if let Some(h) = h {
            r.mesh = MeshRef::Handle(h);
        }
    }
}

// ------------------------------------------------------------------------------- building

/// The dragon's own state beside its minion's.
#[derive(Component, Debug, Clone)]
pub struct Dragon {
    pub phase: Phase,
    /// Seconds in this phase.
    pub t: f32,
    /// Its breath of grave frost.
    pub breath: Arc<Spell>,
    /// Where its circle turns round, and which way (+1 counter-clockwise from above).
    pub center: DVec3,
    pub turn: f32,
    /// How far round its circle it has flown, laps.
    pub laps: f32,
    pub angle: f32,
    /// When it may breathe next (seconds into the phase), and gouts so far.
    pub next_breath: f32,
    pub gouts: u32,
    /// Where it comes down.
    pub land_at: DVec3,
    /// Its wingbeat, 0..1, and its walk.
    pub flap: f32,
    pub walk: f32,
    pub bank: f32,
    pub pitch: f32,
    /// Each piece's start in the floor (its frame) and turn, and when it flies up, s.
    pub scatter: Vec<(Vec3, Quat, f32)>,
    /// Its heart has lit.
    pub lit: bool,
    /// Bites and sweeps on the floor so far.
    pub blows: u32,
}

/// What it is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Assemble,
    Rear,
    TakeOff,
    Circle,
    Land,
    Ground,
}

impl Phase {
    pub fn flying(self) -> bool {
        matches!(self, Self::TakeOff | Self::Circle | Self::Land)
    }
}

/// When each piece flies up out of the floor to its place, s: the hips and spine first, then the
/// legs and tail, the ribs, the neck, the wings and skull, and last its heart.
pub fn assembly_delay(index: usize) -> f32 {
    let pos = |a: &[usize], i: usize| a.iter().position(|&x| x == i);
    if index == PELVIS {
        0.0
    } else if let Some(j) = pos(&BACK, index) {
        0.08 + 0.07 * j as f32
    } else if index == CHEST {
        0.5
    } else if let Some(j) = pos(&TAIL, index) {
        0.45 + 0.06 * j as f32
    } else if let Some(j) = pos(&RIBS, index) {
        0.72 + 0.07 * (j / 2) as f32 + 0.03 * (j % 2) as f32
    } else if let Some(j) = pos(&NECK, index) {
        0.95 + 0.06 * j as f32
    } else if index == SKULL {
        1.42
    } else if index == JAW {
        1.55
    } else if index == HEART {
        ASSEMBLE_SECS - FLY_SECS
    } else if let Some(i) = WING_ARM.iter().position(|&x| x == index) {
        1.3 + 0.06 * i as f32
    } else if let Some(i) = WING_HAND.iter().position(|&x| x == index) {
        1.52 + 0.06 * i as f32
    } else {
        // Legs: hind ones, then fore ones, each from the hip down.
        let (base, start) = if index >= HIND[0] {
            (0.32, HIND[0])
        } else {
            (0.58, FORE[0])
        };
        let j = index - start;
        base + 0.07 * (j % 3) as f32 + 0.05 * (j / 3) as f32
    }
}

impl Dragon {
    pub fn new(breath: Arc<Spell>, k: f32, seed: u64) -> Self {
        let scatter = (0..PARTS)
            .map(|i| {
                let h = |s: u64| roll(seed ^ (i as u64 + 1).wrapping_mul(0x9e37_79b9), s);
                let a = h(1) * TAU;
                let r = (0.6 + 1.9 * h(2).sqrt()) * k.sqrt();
                let at = Vec3::new(a.cos() * r, -STAND_H * k - 0.35, a.sin() * r);
                let rot = Quat::from_euler(EulerRot::YXZ, h(3) * TAU, h(4) * TAU, h(5) * TAU);
                (at, rot, assembly_delay(i))
            })
            .collect();
        Self {
            phase: Phase::Assemble,
            t: 0.0,
            breath,
            center: DVec3::ZERO,
            turn: 1.0,
            laps: 0.0,
            angle: 0.0,
            next_breath: 0.0,
            gouts: 0,
            land_at: DVec3::ZERO,
            flap: 0.0,
            walk: 0.0,
            bank: 0.0,
            pitch: 0.0,
            scatter,
            lit: false,
            blows: 0,
        }
    }
}

/// Its size from the summon's power: about 6.5 m from snout to tail at size 1.
pub fn dragon_size(spell: &Spell, scale: f32) -> f32 {
    (crate::summon::size_of(spell, scale) * 0.95).clamp(0.85, 1.35)
}

/// Its bite, its tail's sweep, and its breath: grave frost in a cone (frost, in the weave's
/// element if it has one), on its foes only.
pub fn attacks(spell: &Spell, k: f32) -> (Arc<Spell>, Option<Arc<Spell>>) {
    let blow = |kind: engine_world::DamageKind, damage: f32, impulse: f32| {
        let mut s = spell.clone();
        s.cast = CastType::Touch;
        s.stages.clear();
        s.payload.bone_dragon = false;
        s.payload.damage = spell.power.damage() * damage * k;
        s.payload.damage_kind = kind;
        s.payload.force = Some((ForceDir::Away, impulse * k));
        s.carrier.radius = 0.6;
        s.filter = TargetFilter::Enemies;
        Arc::new(s)
    };
    (
        blow(engine_world::DamageKind::Pierce, 0.5, 150.0),
        Some(blow(engine_world::DamageKind::Blunt, 0.35, 260.0)),
    )
}

pub fn breath(spell: &Spell, k: f32) -> Arc<Spell> {
    let keys: Vec<&str> = if spell.weave.elements.is_empty() {
        vec!["frost"]
    } else {
        spell.weave.elements.iter().map(|e| e.key()).collect()
    };
    let mut s = compile(
        &SpellDesign::new(&keys, CastType::Cone),
        CastContext::new(spell.mastery, spell.power),
    )
    .map(|s| (*s).clone())
    .unwrap_or_else(|_| {
        let mut s = spell.clone();
        s.cast = CastType::Cone;
        s
    });
    s.filter = TargetFilter::Enemies;
    s.stages.clear();
    s.carrier.range = BREATH_M * k.sqrt();
    s.carrier.cone_deg = 20.0;
    // A gout is a share of a frost cone: it breathes a dozen on a pass.
    s.payload.damage *= 0.18;
    if spell.weave.elements.is_empty() {
        s.weave.color = GRAVE_FROST;
    }
    s.name = "Grave frost".into();
    Arc::new(s)
}

/// Put a bone dragon's body on a fresh minion root: its pieces and their bones, its membranes,
/// its collider comes when it is whole.
pub fn build(commands: &mut Commands, root: Entity, k: f32, seed: u64, parts: &mut Vec<Entity>) {
    let bone = art::bone();
    let dark = art::dark_bone();
    let rock = |size: Vec3, salt: u64, m: StandardMaterial| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Rock {
                size: (size * k).to_array(),
                seed: ((seed ^ salt.wrapping_mul(7919)) % 100_000) as u32,
                detail: 2,
            }),
            MaterialRef::Inline(m),
        )
    };
    let cap = |r: f32, len: f32, m: StandardMaterial| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Capsule {
                radius: r * k,
                height: (len * k).max(2.0 * r * k + 0.01),
                segments: 10,
            }),
            MaterialRef::Inline(m),
        )
    };
    let cone = |r: f32, h: f32, m: StandardMaterial| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Cone {
                radius: r * k,
                height: h * k,
                segments: 6,
            }),
            MaterialRef::Inline(m),
        )
    };
    let glow = |r: f32, nits: f32| {
        MeshRenderer::new(
            MeshRef::Shape(Shape::Sphere {
                radius: r * k,
                segments: 14,
            }),
            MaterialRef::Inline(
                StandardMaterial::default()
                    .with_tint(GRAVE_FROST[0], GRAVE_FROST[1], GRAVE_FROST[2])
                    .with_emissive(GRAVE_FROST, nits),
            ),
        )
        .without_shadows()
    };
    // A child laid from `a` to `b` (a capsule's or cone's length along its Y).
    let along = |a: Vec3, b: Vec3| {
        let d = b - a;
        Transform::from_translation(((a + b) * 0.5 * k).as_dvec3())
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, d.normalize_or(Vec3::Y)))
    };
    let at = |p: Vec3| Transform::from_translation((p * k).as_dvec3());
    let kid = |commands: &mut Commands, parent: Entity, tf: Transform, r: MeshRenderer| {
        commands.spawn((Name::new("Dragon bone"), tf, r, ChildOf(parent)));
    };
    // A spike from `from` toward `dir`, `len` long.
    let spike =
        |commands: &mut Commands, parent: Entity, from: Vec3, dir: Vec3, r: f32, len: f32| {
            let d = dir.normalize();
            kid(
                commands,
                parent,
                along(from, from + d * len),
                cone(r, len, dark),
            );
        };
    for i in 0..PARTS {
        let (r, size) = match i {
            PELVIS => (
                rock(Vec3::new(0.5, 0.3, 0.46), 1, bone),
                Vec3::new(0.5, 0.3, 0.46),
            ),
            CHEST => (
                rock(Vec3::new(0.4, 0.3, 0.34), 2, bone),
                Vec3::new(0.4, 0.3, 0.34),
            ),
            SKULL => (
                rock(Vec3::new(0.34, 0.3, 0.4), 3, bone),
                Vec3::new(0.34, 0.3, 0.4),
            ),
            JAW => (rock(Vec3::splat(0.08), 4, bone), Vec3::new(0.24, 0.1, 0.6)),
            HEART => (glow(0.14, HEART_GLOW), Vec3::ZERO),
            _ if BACK.contains(&i) => {
                let s = Vec3::new(0.17, 0.15, 0.22);
                (rock(s, i as u64, bone), s)
            }
            _ if NECK.contains(&i) => {
                let f = 1.0 - 0.04 * (i - NECK[0]) as f32;
                let s = Vec3::new(0.15, 0.16, 0.17) * f;
                (rock(s, i as u64, bone), s)
            }
            _ if TAIL.contains(&i) => {
                let j = i - TAIL[0];
                let f = (1.0 - 0.075 * j as f32).max(0.3);
                if j == TAIL.len() - 1 {
                    // A spade of bone at its tip.
                    let s = Vec3::new(0.26, 0.05, 0.36);
                    (rock(s, i as u64, bone), s)
                } else {
                    let s = Vec3::new(0.14, 0.12, 0.21) * f;
                    (rock(s, i as u64, bone), s)
                }
            }
            _ if RIBS.contains(&i) => (
                rock(Vec3::splat(0.07), i as u64, bone),
                Vec3::new(0.1, 0.8, 0.5),
            ),
            _ if WING_ARM.contains(&i) => (
                rock(Vec3::splat(0.13), i as u64, bone),
                Vec3::new(1.3, 0.1, 0.1),
            ),
            _ if WING_HAND.contains(&i) => (
                rock(Vec3::splat(0.1), i as u64, bone),
                Vec3::new(1.4, 0.05, 1.4),
            ),
            _ => {
                // Legs: the bones of the limb, its foot.
                let base = if i >= HIND[0] { HIND[0] } else { FORE[0] };
                let j = (i - base) % 3;
                let hind = i >= HIND[0];
                match j {
                    0 => {
                        let len = if hind { HIND_THIGH } else { FORE_UPPER };
                        (cap(0.066, len, bone), Vec3::new(0.13, len, 0.13))
                    }
                    1 => {
                        let len = if hind { HIND_SHIN } else { FORE_LOWER };
                        (cap(0.05, len, bone), Vec3::new(0.1, len, 0.1))
                    }
                    _ => (
                        rock(Vec3::new(0.2, 0.07, 0.24), i as u64, bone),
                        Vec3::new(0.2, 0.07, 0.24),
                    ),
                }
            }
        };
        let id = commands
            .spawn((
                Name::new("Dragon piece"),
                Transform::from_scale(Vec3::ZERO),
                r,
                MinionPart {
                    index: i,
                    size: size * k,
                },
                ChildOf(root),
            ))
            .id();
        parts.push(id);
        let h = |s: u64| roll(seed ^ (i as u64 + 1).wrapping_mul(0x51ed), s);
        match i {
            PELVIS => {
                for s in [-1.0f32, 1.0] {
                    kid(
                        commands,
                        id,
                        at(Vec3::new(0.22 * s, 0.1, -0.04))
                            .with_rotation(Quat::from_rotation_z(-0.4 * s)),
                        rock(Vec3::new(0.1, 0.34, 0.3), 30 + s as u64, bone),
                    );
                }
            }
            CHEST => {
                // The shoulder blades.
                for s in [-1.0f32, 1.0] {
                    kid(
                        commands,
                        id,
                        at(Vec3::new(0.27 * s, 0.12, 0.06))
                            .with_rotation(Quat::from_rotation_z(0.35 * s)),
                        rock(Vec3::new(0.08, 0.42, 0.3), 40 + s as u64, bone),
                    );
                }
                spike(
                    commands,
                    id,
                    Vec3::new(0.0, 0.12, 0.0),
                    Vec3::new(0.0, 1.0, 0.45),
                    0.04,
                    0.34,
                );
            }
            SKULL => {
                // The snout and the upper jaw's edge.
                kid(
                    commands,
                    id,
                    along(Vec3::new(0.0, -0.05, -0.08), Vec3::new(0.0, -0.08, -0.7)),
                    cap(0.085, 0.62, bone),
                );
                kid(
                    commands,
                    id,
                    at(Vec3::new(0.0, -0.11, -0.36)),
                    rock(Vec3::new(0.21, 0.08, 0.52), 50, bone),
                );
                for s in [-1.0f32, 1.0] {
                    // Brows over deep sockets, the grave's light in them.
                    kid(
                        commands,
                        id,
                        at(Vec3::new(0.1 * s, 0.1, -0.14)),
                        rock(Vec3::new(0.12, 0.06, 0.12), 51 + s as u64, bone),
                    );
                    kid(
                        commands,
                        id,
                        at(Vec3::new(0.115 * s, 0.035, -0.17)),
                        MeshRenderer::new(
                            MeshRef::Shape(Shape::Sphere {
                                radius: 0.058 * k,
                                segments: 10,
                            }),
                            MaterialRef::Inline(
                                StandardMaterial::color(0.03, 0.025, 0.02).with_roughness(0.9),
                            ),
                        ),
                    );
                    kid(
                        commands,
                        id,
                        at(Vec3::new(0.13 * s, 0.035, -0.2)),
                        glow(0.03, EYE_GLOW),
                    );
                    // Horns swept back off the crown, and a lesser pair below them.
                    spike(
                        commands,
                        id,
                        Vec3::new(0.1 * s, 0.12, 0.06),
                        Vec3::new(0.35 * s, 0.42, 1.0),
                        0.055,
                        0.75,
                    );
                    spike(
                        commands,
                        id,
                        Vec3::new(0.15 * s, 0.0, 0.08),
                        Vec3::new(0.85 * s, -0.12, 0.6),
                        0.035,
                        0.36,
                    );
                    // Teeth down the upper jaw, fangs at the front.
                    for j in 0..5 {
                        let z = -0.2 - 0.09 * j as f32;
                        let len = if j == 4 { 0.13 } else { 0.08 };
                        kid(
                            commands,
                            id,
                            along(
                                Vec3::new(0.075 * s, -0.13, z),
                                Vec3::new(0.07 * s, -0.13 - len, z),
                            ),
                            cone(0.016, len, bone),
                        );
                    }
                }
                for j in 0..3 {
                    spike(
                        commands,
                        id,
                        Vec3::new(0.0, 0.14, -0.06 + 0.09 * j as f32),
                        Vec3::new(0.0, 1.0, 0.7),
                        0.022,
                        0.14,
                    );
                }
                // The lit jaw: the grave's light deep in its throat.
                kid(
                    commands,
                    id,
                    at(Vec3::new(0.0, -0.13, -0.1)),
                    glow(0.055, 5_000.0),
                );
            }
            JAW => {
                for s in [-1.0f32, 1.0] {
                    kid(
                        commands,
                        id,
                        along(
                            Vec3::new(0.11 * s, -0.03, 0.02),
                            Vec3::new(0.035 * s, -0.07, -0.6),
                        ),
                        cap(0.034, 0.62, bone),
                    );
                    for j in 0..4 {
                        let z = -0.2 - 0.09 * j as f32;
                        let x = 0.1 * s - 0.065 * s * (j as f32 / 3.0);
                        kid(
                            commands,
                            id,
                            along(Vec3::new(x, -0.03, z), Vec3::new(x, 0.04, z)),
                            cone(0.014, 0.07, bone),
                        );
                    }
                }
                kid(
                    commands,
                    id,
                    at(Vec3::new(0.0, -0.07, -0.6)),
                    rock(Vec3::new(0.1, 0.06, 0.09), 60, bone),
                );
            }
            _ if BACK.contains(&i) => {
                let j = i - BACK[0];
                let len = 0.32 - 0.025 * j as f32;
                spike(
                    commands,
                    id,
                    Vec3::new(0.0, 0.08, 0.0),
                    Vec3::new(0.0, 1.0, 0.5),
                    0.035,
                    len,
                );
                // The transverse processes.
                for s in [-1.0f32, 1.0] {
                    spike(
                        commands,
                        id,
                        Vec3::ZERO,
                        Vec3::new(s, 0.1, 0.1),
                        0.025,
                        0.15,
                    );
                }
            }
            _ if NECK.contains(&i) => {
                let f = 1.0 - 0.04 * (i - NECK[0]) as f32;
                spike(
                    commands,
                    id,
                    Vec3::new(0.0, 0.07, 0.0),
                    Vec3::new(0.0, 1.0, 0.65),
                    0.025 * f,
                    0.19 * f,
                );
            }
            _ if TAIL.contains(&i) => {
                let j = i - TAIL[0];
                let f = (1.0 - 0.075 * j as f32).max(0.3);
                if j < 8 {
                    spike(
                        commands,
                        id,
                        Vec3::new(0.0, 0.05 * f, 0.0),
                        Vec3::new(0.0, 1.0, 0.55),
                        0.028 * f,
                        0.22 * f,
                    );
                }
                if j == TAIL.len() - 1 {
                    for s in [-1.0f32, 1.0] {
                        spike(
                            commands,
                            id,
                            Vec3::new(0.08 * s, 0.0, 0.05),
                            Vec3::new(s, 0.0, 0.6),
                            0.03,
                            0.22,
                        );
                    }
                }
            }
            _ if RIBS.contains(&i) => {
                let j = (i - RIBS[0]) / 2;
                let side = if (i - RIBS[0]).is_multiple_of(2) {
                    -1.0
                } else {
                    1.0
                };
                let pts = rib_curve(RIB_LEN[j], side);
                let n = pts.len() - 1;
                for (s, w) in pts.windows(2).enumerate() {
                    let r = 0.034 - 0.012 * s as f32 / n as f32;
                    kid(
                        commands,
                        id,
                        along(w[0], w[1]),
                        cap(r, w[0].distance(w[1]) + 0.05, bone),
                    );
                }
            }
            _ if WING_ARM.contains(&i) => {
                let wing = i - WING_ARM[0];
                let s = if wing == 0 { -1.0 } else { 1.0 };
                let w = art::WRIST * Vec3::new(s, 1.0, 1.0);
                // The arm's two long bones, kinked at the elbow.
                let elbow = Vec3::new(0.62 * s, 0.07, 0.12);
                kid(
                    commands,
                    id,
                    along(Vec3::ZERO, elbow),
                    cap(0.058, elbow.length() + 0.06, bone),
                );
                kid(
                    commands,
                    id,
                    along(elbow, w),
                    cap(0.045, elbow.distance(w) + 0.05, bone),
                );
                kid(
                    commands,
                    id,
                    at(elbow),
                    rock(Vec3::splat(0.1), 70 + wing as u64, bone),
                );
                commands.spawn((
                    Name::new("Wing membrane"),
                    Transform::from_scale(Vec3::splat(k)),
                    MeshRenderer::new(
                        MeshRef::Shape(Shape::Quad { size: [0.01, 0.01] }),
                        MaterialRef::Inline(art::membrane()),
                    ),
                    Membrane {
                        hand: false,
                        side: wing,
                    },
                    ChildOf(id),
                ));
            }
            _ if WING_HAND.contains(&i) => {
                let wing = i - WING_HAND[0];
                let s = if wing == 0 { -1.0 } else { 1.0 };
                for (f, tip) in art::FINGERS.iter().enumerate() {
                    let tip = *tip * Vec3::new(s, 1.0, 1.0);
                    // Each finger in two bones, bowed back a little at its knuckle.
                    let knuckle = tip * 0.45 + Vec3::new(0.0, 0.04, -0.06 * (1.0 - f as f32 / 3.0));
                    let r = 0.036 - 0.004 * f as f32;
                    kid(
                        commands,
                        id,
                        along(Vec3::ZERO, knuckle),
                        cap(r, knuckle.length() + 0.04, bone),
                    );
                    kid(
                        commands,
                        id,
                        along(knuckle, tip),
                        cap(r * 0.7, knuckle.distance(tip) + 0.03, bone),
                    );
                    kid(
                        commands,
                        id,
                        at(knuckle),
                        rock(Vec3::splat(0.055), 80 + f as u64, bone),
                    );
                }
                // The thumb's claw at the wrist, hooked forward.
                spike(
                    commands,
                    id,
                    Vec3::ZERO,
                    Vec3::new(0.15 * s, 0.25, -1.0),
                    0.03,
                    0.22,
                );
                commands.spawn((
                    Name::new("Wing membrane"),
                    Transform::from_scale(Vec3::splat(k)),
                    MeshRenderer::new(
                        MeshRef::Shape(Shape::Quad { size: [0.01, 0.01] }),
                        MaterialRef::Inline(art::membrane()),
                    ),
                    Membrane {
                        hand: true,
                        side: wing,
                    },
                    ChildOf(id),
                ));
            }
            HEART => {}
            _ => {
                // Legs: knuckled ends on the long bones; claws on the feet.
                let base = if i >= HIND[0] { HIND[0] } else { FORE[0] };
                let j = (i - base) % 3;
                let hind = i >= HIND[0];
                match j {
                    0 | 1 => {
                        let len = match (hind, j) {
                            (true, 0) => HIND_THIGH,
                            (true, _) => HIND_SHIN,
                            (false, 0) => FORE_UPPER,
                            _ => FORE_LOWER,
                        };
                        let r = if j == 0 { 0.11 } else { 0.085 };
                        for e in [-0.5f32, 0.5] {
                            kid(
                                commands,
                                id,
                                at(Vec3::new(0.0, e * len, 0.0)),
                                rock(Vec3::splat(r), 90 + (i * 2) as u64 + (e > 0.0) as u64, bone),
                            );
                        }
                    }
                    _ => {
                        for c in 0..4 {
                            let a = (c as f32 - 1.5) * 0.32 + 0.1 * (h(c as u64) - 0.5);
                            let dir = Vec3::new(a.sin(), -0.35, -a.cos());
                            spike(
                                commands,
                                id,
                                Vec3::new(a.sin() * 0.08, -0.01, -0.09),
                                dir,
                                0.022,
                                0.17,
                            );
                        }
                    }
                }
            }
        }
    }
    // The grave's cold light from its heart on what is round it, lit when the heart comes.
    commands.entity(root).insert(PointLight {
        color: GRAVE_FROST,
        intensity_lm: 0.0,
        range: 7.0 * k,
        radius: 0.2,
    });
}

/// Its body for physics once whole: a capsule along it.
pub fn collider(k: f32) -> (Collider, RigidBody) {
    (
        Collider {
            shape: ColliderShape::Compound(vec![(
                Vec3::ZERO,
                Quat::from_rotation_x(PI * 0.5),
                ColliderShape::Capsule {
                    radius: 0.55 * k,
                    half_height: 0.85 * k,
                },
            )]),
            density: 900.0,
            ..Default::default()
        },
        RigidBody {
            kind: BodyKind::Kinematic,
            ..Default::default()
        },
    )
}

// ------------------------------------------------------------------------------- the pose

/// A wing's set: the arm's flap up (rad) and sweep back, the hand's fold down from the arm and
/// sweep back from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WingPose {
    pub flap: f32,
    pub sweep: f32,
    pub fold: f32,
    pub fold_sweep: f32,
}

impl WingPose {
    /// Folded along its flanks, half open, as it stands.
    pub const FOLDED: Self = Self {
        flap: 0.95,
        sweep: 1.2,
        fold: -2.0,
        fold_sweep: 1.0,
    };
    /// Spread wide and still (a glide, its rearing).
    pub const SPREAD: Self = Self {
        flap: 0.12,
        sweep: 0.08,
        fold: -0.05,
        fold_sweep: 0.04,
    };

    /// A wingbeat at `phase` 0..1 (0 the top of the stroke), `amp` its swing, rad.
    pub fn beat(phase: f32, amp: f32) -> Self {
        let p = phase * TAU;
        let arm = 0.12 + amp * p.cos();
        // The hand lags the arm: it is still coming up as the arm goes down.
        let tip = 0.12 + 1.25 * amp * (p - 0.8).cos();
        Self {
            flap: arm,
            sweep: 0.12 - 0.18 * p.sin(),
            fold: tip - arm,
            fold_sweep: 0.06 + 0.1 * p.sin().max(0.0),
        }
    }

    pub fn lerp(self, o: Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self {
            flap: l(self.flap, o.flap),
            sweep: l(self.sweep, o.sweep),
            fold: l(self.fold, o.fold),
            fold_sweep: l(self.fold_sweep, o.fold_sweep),
        }
    }
}

/// How it holds itself this step, at size 1, in its frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stance {
    pub wings: WingPose,
    /// Where its neck meets its skull, and the skull's turn.
    pub head: Vec3,
    pub look: Quat,
    pub jaw: f32,
    /// Its ankles: fore left, fore right, hind left, hind right.
    pub feet: [Vec3; 4],
    /// 0: the tail droops and curls; 1: it streams out straight.
    pub tail_lift: f32,
    /// The tail's swing to the side, rad (a sweep).
    pub tail_swing: f32,
    /// Its heart's glow, 0..1 and more.
    pub heart: f32,
    pub age: f32,
}

/// One piece's pose in its body's frame.
pub type Pose = (usize, Vec3, Quat, Vec3);

fn bone_pose(i: usize, a: Vec3, b: Vec3) -> Pose {
    let d = b - a;
    (
        i,
        (a + b) * 0.5,
        Quat::from_rotation_arc(Vec3::Y, d.normalize_or(Vec3::NEG_Y)),
        Vec3::ONE,
    )
}

/// Its pieces' poses in its own frame at size `k`.
pub fn pose_dragon(k: f32, st: &Stance) -> Vec<Pose> {
    let mut out: Vec<Pose> = Vec::with_capacity(PARTS);
    let spine = |s: f32| CHEST_P.lerp(PELVIS_P, s) + Vec3::Y * (0.1 * (PI * s).sin());
    out.push((PELVIS, PELVIS_P, Quat::IDENTITY, Vec3::ONE));
    out.push((CHEST, CHEST_P, Quat::IDENTITY, Vec3::ONE));
    for (j, &b) in BACK.iter().enumerate() {
        let s = (j as f32 + 0.5) / BACK.len() as f32;
        out.push((b, spine(s), Quat::IDENTITY, Vec3::ONE));
    }
    for j in 0..5 {
        let s = 0.06 + 0.16 * j as f32;
        let p = spine(s);
        for (side_i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
            out.push((
                RIBS[j * 2 + side_i],
                p + Vec3::new(0.05 * side, -0.03, 0.0),
                Quat::from_rotation_x(-0.22),
                Vec3::ONE,
            ));
        }
    }
    let pulse = 1.0 + 0.1 * (st.age * 4.4).sin();
    out.push((
        HEART,
        spine(0.32) + Vec3::new(0.0, -0.32, 0.0),
        Quat::IDENTITY,
        Vec3::splat((st.heart * pulse).max(0.0)),
    ));
    // The neck: a curve from the shoulders, out forward and up into the skull from behind it.
    let n0 = NECK_BASE;
    let h = st.head;
    let c1 = n0 + Vec3::new(0.0, 0.05, -0.5);
    let c2 = h + st.look * Vec3::new(0.0, -0.05, 0.5) + Vec3::new(0.0, -0.2, 0.0);
    let curve = |t: f32| {
        let u = 1.0 - t;
        n0 * (u * u * u) + c1 * (3.0 * u * u * t) + c2 * (3.0 * u * t * t) + h * (t * t * t)
    };
    for (j, &v) in NECK.iter().enumerate() {
        let t = (j as f32 + 0.5) / NECK.len() as f32;
        let p = curve(t);
        let tangent =
            (curve((t + 0.04).min(1.0)) - curve((t - 0.04).max(0.0))).normalize_or(Vec3::NEG_Z);
        out.push((
            v,
            p,
            Quat::from_rotation_arc(Vec3::NEG_Z, tangent),
            Vec3::ONE,
        ));
    }
    out.push((SKULL, h + st.look * SKULL_OFF, st.look, Vec3::ONE));
    let hinge = h + st.look * Vec3::new(0.0, -0.1, -0.04);
    out.push((
        JAW,
        hinge,
        st.look * Quat::from_rotation_x(-0.75 * st.jaw.clamp(0.0, 1.0)),
        Vec3::ONE,
    ));
    // The tail: back from the hips, drooping and curling when it stands, streaming out behind
    // it in flight, swaying, swung to the side in a sweep.
    let mut p = TAIL_BASE;
    let n = TAIL.len();
    for (j, &t) in TAIL.iter().enumerate() {
        let f = j as f32 / n as f32;
        let sway = (0.32 * (1.0 - 0.55 * st.tail_lift)) * (st.age * 1.9 - j as f32 * 0.55).sin();
        let yaw = sway * (0.3 + f) + st.tail_swing * (0.25 + 0.75 * f);
        let droop = (1.0 - st.tail_lift) * (0.2 + 0.05 * j as f32 - 0.09 * f * f * 10.0).max(-0.2)
            + st.tail_lift * 0.03 * (st.age * 2.6 - j as f32 * 0.8).sin();
        let dir = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(droop) * Vec3::Z;
        let q = p + dir * TAIL_SEG;
        out.push((
            t,
            (p + q) * 0.5,
            Quat::from_rotation_arc(Vec3::Z, dir),
            Vec3::ONE,
        ));
        p = q;
    }
    // Legs: two-bone IK from the shoulder or hip to the ankle; fore elbows back, hind knees
    // forward.
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        let mount = FORE_MOUNT * Vec3::new(s, 1.0, 1.0);
        let ankle = st.feet[i];
        let (elbow, wrist) = two_bone(
            mount,
            ankle,
            FORE_UPPER,
            FORE_LOWER,
            Vec3::new(0.0, 0.2, 1.0),
        );
        out.push(bone_pose(FORE[i], mount, elbow));
        out.push(bone_pose(FORE[i] + 1, elbow, wrist));
        out.push((
            FORE[i] + 2,
            wrist - Vec3::Y * ANKLE_Y * 0.6,
            Quat::IDENTITY,
            Vec3::ONE,
        ));
        let mount = HIND_MOUNT * Vec3::new(s, 1.0, 1.0);
        let ankle = st.feet[2 + i];
        let (knee, heel) = two_bone(
            mount,
            ankle,
            HIND_THIGH,
            HIND_SHIN,
            Vec3::new(0.0, 0.2, -1.0),
        );
        out.push(bone_pose(HIND[i], mount, knee));
        out.push(bone_pose(HIND[i] + 1, knee, heel));
        out.push((
            HIND[i] + 2,
            heel - Vec3::Y * ANKLE_Y * 0.6,
            Quat::IDENTITY,
            Vec3::ONE,
        ));
    }
    // Wings: the arm turns at its root (flap about the body's long axis, sweep about its
    // up); the hand rides the wrist, turned from the arm.
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        let w = st.wings;
        let root = WING_ROOT * Vec3::new(s, 1.0, 1.0);
        let qa = Quat::from_rotation_z(s * w.flap) * Quat::from_rotation_y(-s * w.sweep);
        out.push((WING_ARM[i], root, qa, Vec3::ONE));
        let wrist = root + qa * (art::WRIST * Vec3::new(s, 1.0, 1.0));
        let qh = qa * Quat::from_rotation_z(s * w.fold) * Quat::from_rotation_y(-s * w.fold_sweep);
        out.push((WING_HAND[i], wrist, qh, Vec3::ONE));
    }
    for p in out.iter_mut() {
        p.1 *= k;
    }
    // Made by limb, read by index.
    out.sort_by_key(|p| p.0);
    out
}

/// Where its feet stand on the floor at size 1 in its level frame (under its shoulders and hips,
/// a little wide), walking a four-beat gait: `phase` 0..1 through a stride, `stride` m.
pub fn walking_feet(phase: f32, stride: f32, lift: f32) -> [Vec3; 4] {
    let duty = 0.72;
    let foot = |mount: Vec3, offset: f32, fore: bool| {
        let rest = Vec3::new(
            mount.x * 1.15,
            -STAND_H + ANKLE_Y,
            mount.z + if fore { -0.08 } else { 0.06 },
        );
        let u = (phase + offset).fract();
        let half = 0.5 * stride * duty;
        if u < duty {
            rest + Vec3::new(0.0, 0.0, -half + 2.0 * half * (u / duty))
        } else {
            let w = (u - duty) / (1.0 - duty);
            rest + Vec3::new(0.0, lift * (PI * w).sin(), half - 2.0 * half * smooth(w))
        }
    };
    [
        foot(FORE_MOUNT * Vec3::new(-1.0, 1.0, 1.0), 0.0, true),
        foot(FORE_MOUNT, 0.5, true),
        foot(HIND_MOUNT * Vec3::new(-1.0, 1.0, 1.0), 0.75, false),
        foot(HIND_MOUNT, 0.25, false),
    ]
}

/// Its feet tucked up in flight: fore ones forward under its chest, hind ones trailing.
pub fn tucked_feet() -> [Vec3; 4] {
    let f = |m: Vec3, fore: bool| {
        if fore {
            m + Vec3::new(0.02 * m.x.signum(), -0.62, -0.35)
        } else {
            m + Vec3::new(0.0, -0.32, 0.85)
        }
    };
    [
        f(FORE_MOUNT * Vec3::new(-1.0, 1.0, 1.0), true),
        f(FORE_MOUNT, true),
        f(HIND_MOUNT * Vec3::new(-1.0, 1.0, 1.0), false),
        f(HIND_MOUNT, false),
    ]
}

/// The skull's turn to look along `dir` (its frame), held within a reach of its neck.
fn look_along(dir: Vec3, yaw_max: f32) -> Quat {
    let d = dir.normalize_or(Vec3::NEG_Z);
    let yaw = (-d.x).atan2(-d.z).clamp(-yaw_max, yaw_max);
    let pitch = d.y.clamp(-0.95, 0.95).asin().clamp(-1.1, 0.6);
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
}

// ------------------------------------------------------------------------------- living

/// What the dragon needs from the minions' step.
pub struct Life<'a, 'w, 's> {
    pub e: Entity,
    pub dt: f32,
    pub g: f32,
    pub queries: &'a PhysicsQueries,
    pub exclude: &'a QueryFilter,
    pub target: Option<(Entity, DVec3)>,
    pub owner: Option<(DVec3, Quat)>,
    pub held: bool,
    /// Its foes near enough to strike: who, where.
    pub foes: &'a [(Entity, DVec3)],
    pub hits: &'a mut HitQueue,
    pub queue: &'a mut MagicQueue,
    pub commands: &'a mut Commands<'w, 's>,
}

fn minion_cast(m: &Minion, e: Entity, spell: &Arc<Spell>, scale: f32) -> CastInfo {
    CastInfo {
        spell: spell.clone(),
        caster: Some(e),
        scale,
        seed: m.cast.seed.wrapping_add(m.attacks * 7919),
        ..m.cast.clone()
    }
}

/// A bone dragon lives a step.
pub fn live_dragon(
    m: &mut Minion,
    d: &mut Dragon,
    tf: &mut Transform,
    life: &mut Life,
    parts: &mut Query<(&MinionPart, &mut Transform), Without<Minion>>,
) {
    let (e, dt) = (life.e, life.dt);
    let k = m.size;
    d.t += dt;
    let floor = ground_below(life.queries, m.pos, life.exclude).y;
    let stand = floor + (STAND_H * k) as f64;
    let target = life.target;
    let yaw_before = m.yaw;
    let mut wings = WingPose::FOLDED;
    let mut head = Vec3::new(0.0, 1.15, -1.95);
    let mut look = Quat::from_rotation_x(-0.15);
    let mut jaw = 0.12;
    let mut feet_level = Some(walking_feet(d.walk, 0.0, 0.0));
    let mut feet_body = None;
    let mut tail_lift = 0.0;
    let mut tail_swing = 0.0;
    let mut heart = if d.lit { 1.0 } else { 0.0 };
    let mut lift = 0.0f32;
    let mut pitch_want = 0.0;
    // Chains or ice bring it down where it is.
    if life.held && d.phase.flying() && d.phase != Phase::Land {
        d.phase = Phase::Land;
        d.t = 0.0;
        d.land_at = DVec3::new(m.pos.x, stand, m.pos.z);
    }
    match d.phase {
        Phase::Assemble => {
            m.vel = Vec3::ZERO;
            m.pos = DVec3::new(m.pos.x, stand, m.pos.z);
            let rise = ramp(ASSEMBLE_SECS - 0.5, ASSEMBLE_SECS, d.t);
            heart = rise;
            if d.t >= ASSEMBLE_SECS {
                d.phase = Phase::Rear;
                d.t = 0.0;
                d.lit = true;
                life.commands
                    .entity(e)
                    .insert((collider(k), crate::caster::Vitality::new(HEALTH * k)));
                life.commands.entity(e).insert(PointLight {
                    color: GRAVE_FROST,
                    intensity_lm: 1600.0,
                    range: 8.0 * k,
                    radius: 0.25,
                });
                life.queue.looks.push(LookCmd::Burst {
                    at: m.pos,
                    up: Vec3::Y,
                    look: "frost".into(),
                    role: Role::Impact,
                    scale: 0.9 * k,
                    color: GRAVE_FROST,
                });
            }
        }
        Phase::Rear => {
            // It rears on its hind legs, wings flung wide, and roars its cold.
            let u = (d.t / REAR_SECS).min(1.0);
            let up = (PI * u).sin();
            pitch_want = 0.42 * up;
            lift = 0.35 * up;
            wings = WingPose::FOLDED.lerp(
                WingPose {
                    flap: 0.55,
                    sweep: -0.05,
                    fold: 0.15,
                    fold_sweep: 0.0,
                },
                smooth(u * 2.5),
            );
            head = Vec3::new(0.0, 1.55, -1.7);
            look = Quat::from_rotation_x(0.55 * up);
            jaw = ramp(0.15, 0.4, u) * (1.0 - ramp(0.8, 1.0, u));
            heart = 1.0 + 0.5 * up;
            let fore =
                |s: f32| FORE_MOUNT * Vec3::new(s, 1.0, 1.0) + Vec3::new(0.06 * s, -0.55, -0.4);
            let mut f = walking_feet(0.0, 0.0, 0.0);
            f[0] = f[0].lerp(fore(-1.0), up);
            f[1] = f[1].lerp(fore(1.0), up);
            feet_level = Some(f);
            if d.t >= REAR_SECS {
                d.phase = Phase::TakeOff;
                d.t = 0.0;
            }
        }
        Phase::TakeOff => {
            // Up on hard wingbeats, leaning into the climb.
            let fwd = Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z;
            let climb = if (m.pos.y - floor) < (CRUISE_M * k) as f64 {
                3.6
            } else {
                0.0
            };
            let want = fwd * (3.0 + 2.5 * smooth(d.t / 1.5)) + Vec3::Y * climb;
            m.vel = m.vel.lerp(want, (2.5 * dt).min(1.0));
            d.flap = (d.flap + dt * 1.45).fract();
            wings = WingPose::beat(d.flap, 0.85);
            let tuck = smooth(d.t / 0.7);
            feet_level = None;
            feet_body = Some(lerp_feet(walking_feet(0.0, 0.0, 0.0), tucked_feet(), tuck));
            tail_lift = tuck;
            head = Vec3::new(0.0, 0.6, -2.3);
            heart = 1.0;
            if (m.pos.y - floor) >= (CRUISE_M * k * 0.9) as f64 || d.t > 3.0 {
                d.phase = Phase::Circle;
                d.t = 0.0;
                d.laps = 0.0;
                d.gouts = 0;
                d.next_breath = 0.0;
                d.center = target
                    .map(|t| t.1)
                    .or(life.owner.map(|o| o.0))
                    .unwrap_or(m.pos + (fwd * CIRCLE_M).as_dvec3());
                let rel = flat(m.pos - d.center).as_vec3();
                d.angle = rel.z.atan2(rel.x);
                // Round the way it already flies.
                d.turn = if rel.cross(m.vel).y < 0.0 { 1.0 } else { -1.0 };
            }
        }
        Phase::Circle => {
            if let Some((_, tp)) = target {
                let c = flat(tp) + DVec3::Y * d.center.y;
                d.center = d.center.lerp(c, (0.9 * dt).min(1.0) as f64);
            }
            let r = CIRCLE_M * k.sqrt();
            let rel = flat(m.pos - d.center).as_vec3();
            let angle = rel.z.atan2(rel.x);
            d.laps += wrap(angle - d.angle) * -d.turn / TAU;
            d.angle = angle;
            let lead = angle - d.turn * 0.6;
            let alt = floor + (CRUISE_M * k) as f64;
            let goal = DVec3::new(
                d.center.x + (lead.cos() * r) as f64,
                alt,
                d.center.z + (lead.sin() * r) as f64,
            );
            let want = (goal - m.pos).as_vec3().normalize_or(m.vel) * FLY_SPEED * k.sqrt();
            m.vel = m.vel.lerp(want, (1.7 * dt).min(1.0));
            feet_level = None;
            feet_body = Some(tucked_feet());
            tail_lift = 1.0;
            heart = 1.0;
            head = Vec3::new(0.0, 0.32, -2.4);
            look = Quat::from_rotation_x(-0.12);
            let breathing = target.is_some() && (BREATH_FROM..BREATH_TO).contains(&d.laps);
            let amp = if breathing { 0.45 } else { 0.62 };
            d.flap = (d.flap + dt * if breathing { 1.1 } else { 0.85 }).fract();
            wings = WingPose::beat(d.flap, amp);
            if breathing && let Some((te, tp)) = target {
                // It turns its head down to its foe and breathes.
                let rot = body_rotation(m.yaw, d.pitch, d.bank);
                let aim = tp + DVec3::Y * 0.5;
                let to = rot.inverse() * (aim - m.pos).as_vec3();
                look = look_along(to, 1.25);
                let dir = to.normalize_or(Vec3::NEG_Z);
                head = Vec3::new(0.0, 0.25, -2.25) + Vec3::new(dir.x, 0.0, 0.0) * 0.5;
                jaw = 0.9;
                heart = 1.35;
                if d.t >= d.next_breath {
                    d.next_breath = d.t + BREATH_EVERY;
                    d.gouts += 1;
                    m.attacks += 1;
                    let skull = head + look * SKULL_OFF;
                    let mouth_local = (skull + look * MOUTH) * k;
                    let mouth = m.pos + (rot * mouth_local).as_dvec3();
                    let fwd = (aim - mouth).as_vec3().normalize_or(Vec3::NEG_Y);
                    let info = minion_cast(m, e, &d.breath, 1.0);
                    release(
                        &info,
                        mouth,
                        m.pos,
                        fwd,
                        aim,
                        Some(te),
                        life.g,
                        life.queries,
                        life.queue,
                        life.hits,
                    );
                }
            }
            if d.laps >= 1.0 {
                match target.or(life.owner.map(|o| (e, o.0))) {
                    Some((_, tp)) => {
                        d.phase = Phase::Land;
                        d.t = 0.0;
                        // Down beside its foe, facing it.
                        let from = flat(m.pos - tp).normalize_or(DVec3::Z);
                        let spot = tp + from * (BITE_M * k * 0.85) as f64;
                        let g = ground_below(life.queries, spot + DVec3::Y, life.exclude);
                        d.land_at = g + DVec3::Y * (STAND_H * k) as f64;
                    }
                    None => d.laps = 0.0,
                }
            }
        }
        Phase::Land => {
            let to = (d.land_at - m.pos).as_vec3();
            let dist = to.length();
            let speed = (dist * 1.3).clamp(0.6, FLY_SPEED * k.sqrt());
            m.vel = m
                .vel
                .lerp(to.normalize_or(Vec3::NEG_Y) * speed, (2.6 * dt).min(1.0));
            // It back-beats its wings as it comes down, legs reaching for the floor.
            let near = 1.0 - ((m.pos.y - stand) as f32 / (3.0 * k)).clamp(0.0, 1.0);
            d.flap = (d.flap + dt * (0.9 + 0.6 * near)).fract();
            wings = WingPose::beat(d.flap, 0.6 + 0.25 * near);
            wings.sweep -= 0.35 * near;
            feet_level = None;
            feet_body = Some(lerp_feet(tucked_feet(), walking_feet(0.0, 0.0, 0.0), near));
            tail_lift = 1.0 - 0.6 * near;
            head = Vec3::new(0.0, 0.5 + 0.5 * near, -2.25);
            pitch_want = 0.25 * near;
            if dist < 0.3 || d.t > 4.5 {
                m.pos = d.land_at;
                m.vel = Vec3::ZERO;
                d.phase = Phase::Ground;
                d.t = 0.0;
                m.act = Act::Ready;
                m.cooldown = 0.25;
                for (look, scale) in [("stone", 1.4), ("raise", 1.0)] {
                    life.queue.looks.push(LookCmd::Burst {
                        at: DVec3::new(m.pos.x, floor, m.pos.z),
                        up: Vec3::Y,
                        look: look.into(),
                        role: Role::Impact,
                        scale: scale * k,
                        color: crate::undead::GRAVE_DUST,
                    });
                }
            }
        }
        Phase::Ground => {
            m.pos.y = stand;
            heart = 1.0;
            let fwd = Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z;
            let mut act = m.act;
            let mut lunge = 0.0;
            let mut swing = 0.0;
            match &mut act {
                Act::Bite { t } => {
                    let before = *t;
                    *t += dt;
                    jaw = ramp(0.05, 0.3, *t) * (1.0 - ramp(BITE_HIT - 0.06, BITE_HIT, *t));
                    lunge = ramp(0.1, BITE_HIT, *t) * (1.0 - ramp(BITE_HIT + 0.1, BITE_SECS, *t));
                    if before < BITE_HIT
                        && *t >= BITE_HIT
                        && let Some((te, tp)) = target
                        && crate::summon::flat_distance(tp, m.pos) <= (BITE_M * k + 0.6) as f64
                    {
                        let at = tp + DVec3::Y;
                        life.hits.0.push(PendingHit {
                            cast: minion_cast(m, e, &m.attack, 1.0),
                            target: Some(te),
                            point: at,
                            normal: -fwd,
                            dir: fwd,
                            multi: false,
                            impact: true,
                            reacts: true,
                            expire: false,
                        });
                        life.queue.looks.push(LookCmd::Burst {
                            at,
                            up: -fwd,
                            look: "bones".into(),
                            role: Role::Impact,
                            scale: 0.6 * k,
                            color: crate::bones::BONE_DUST,
                        });
                    }
                    if *t >= BITE_SECS {
                        act = Act::Ready;
                        m.cooldown = 0.45;
                    }
                }
                Act::Slam { t } => {
                    // Its tail sweeps round behind it and across, the body turning into it.
                    let before = *t;
                    *t += dt;
                    let u = (*t / SWEEP_SECS).min(1.0);
                    swing = (PI * u).sin() * if m.flank > 0.0 { 1.0 } else { -1.0 };
                    if before < SWEEP_HIT && *t >= SWEEP_HIT {
                        let pelvis =
                            m.pos + (Quat::from_rotation_y(m.yaw) * (PELVIS_P * k)).as_dvec3();
                        for &(fe, fp) in life.foes {
                            let rel = flat(fp - pelvis);
                            if rel.length() > (SWEEP_M * k) as f64 {
                                continue;
                            }
                            let dir = rel.as_vec3().normalize_or(-fwd);
                            if let Some(slam) = m.slam.clone() {
                                life.hits.0.push(PendingHit {
                                    cast: minion_cast(m, e, &slam, 1.0),
                                    target: Some(fe),
                                    point: fp + DVec3::Y * 0.8,
                                    normal: -dir,
                                    dir,
                                    multi: false,
                                    impact: true,
                                    reacts: true,
                                    expire: false,
                                });
                            }
                        }
                        life.queue.looks.push(LookCmd::Burst {
                            at: DVec3::new(pelvis.x, floor, pelvis.z),
                            up: Vec3::Y,
                            look: "stone".into(),
                            role: Role::Impact,
                            scale: 1.1 * k,
                            color: crate::undead::GRAVE_DUST,
                        });
                    }
                    if *t >= SWEEP_SECS {
                        act = Act::Ready;
                        m.cooldown = 0.5;
                        m.flank = -m.flank;
                    }
                }
                _ => {}
            }
            tail_swing = 1.5 * swing;
            // Walk to its foe and face it; bite what is before it, sweep what is round it.
            let mut speed = 0.0;
            if act == Act::Ready
                && !life.held
                && let Some((_, tp)) = target
            {
                {
                    let to = flat(tp - m.pos).as_vec3();
                    let dist = to.length();
                    let diff = wrap(yaw_of(to.normalize_or(fwd)) - m.yaw);
                    m.yaw += diff.clamp(-TURN * dt, TURN * dt);
                    let reach = BITE_M * k;
                    let behind = life.foes.iter().filter(|(_, fp)| {
                        let r = flat(*fp - m.pos);
                        r.length() < (SWEEP_M * k) as f64
                            && r.as_vec3().normalize_or(fwd).dot(fwd) < 0.2
                    });
                    let around = behind.count();
                    if m.cooldown <= 0.0 && (around > 0 || d.blows % 3 == 2) && dist < reach + 1.0 {
                        d.blows += 1;
                        m.attacks += 1;
                        act = Act::Slam { t: 0.0 };
                    } else if m.cooldown <= 0.0 && dist <= reach + 0.2 && diff.abs() < 0.5 {
                        d.blows += 1;
                        m.attacks += 1;
                        act = Act::Bite { t: 0.0 };
                    } else if dist > reach {
                        speed = WALK * k * diff.cos().max(0.0) * ((dist - reach) / 0.8).min(1.0);
                    }
                }
            }
            let fwd = Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z;
            m.vel = m.vel.lerp(fwd * speed, (5.0 * dt).min(1.0));
            let step = (m.vel * dt).as_dvec3();
            if step.length_squared() > 1e-10 {
                let g = ground_below(life.queries, m.pos + step, life.exclude);
                m.pos = g + DVec3::Y * (STAND_H * k) as f64;
            }
            m.act = act;
            let run = m.vel.length() / (WALK * k).max(1e-3);
            let hz = 0.55 + 0.5 * run;
            d.walk = (d.walk + dt * hz).fract();
            let stride = m.vel.length() / hz / k;
            feet_level = Some(walking_feet(d.walk, stride, 0.16 + 0.1 * run));
            // Its head on its foe, low and forward in a bite.
            if let Some((_, tp)) = target {
                let rot = Quat::from_rotation_y(m.yaw);
                let to = rot.inverse() * (tp + DVec3::Y - (m.pos + DVec3::Y)).as_vec3();
                look = look_along(to, 0.8);
            }
            head = Vec3::new(0.0, 1.15, -1.95).lerp(Vec3::new(0.0, 0.3, -2.6), lunge);
            look = look.slerp(Quat::from_rotation_x(-0.35), lunge * 0.6);
            // Wings half open as it fights, flung wider as it sweeps.
            wings = WingPose::FOLDED.lerp(WingPose::SPREAD, 0.25 * swing.abs());
            if d.t > GROUND_SECS && act == Act::Ready && !life.held {
                d.phase = Phase::TakeOff;
                d.t = 0.0;
                d.flap = 0.0;
            }
        }
    }
    if d.phase.flying() {
        m.pos += (m.vel * dt).as_dvec3();
        // Never through the floor.
        let low = floor + (1.2 * k) as f64;
        if m.pos.y < low && d.phase != Phase::Land {
            m.pos.y = low;
        }
        let v = Vec3::new(m.vel.x, 0.0, m.vel.z);
        if v.length() > 0.5 {
            let diff = wrap(yaw_of(v.normalize()) - m.yaw);
            m.yaw += diff.clamp(-2.0 * dt, 2.0 * dt);
        }
        let sp = m.vel.length().max(1.0);
        pitch_want += (m.vel.y / sp * 0.7).clamp(-0.35, 0.45);
    }
    // Bank into its turns.
    let yaw_rate = wrap(m.yaw - yaw_before) / dt.max(1e-4);
    let bank_want = if d.phase.flying() {
        (yaw_rate * 0.45).clamp(-0.6, 0.6)
    } else {
        0.0
    };
    d.bank += (bank_want - d.bank) * (3.0 * dt).min(1.0);
    d.pitch += (pitch_want - d.pitch) * (3.0 * dt).min(1.0);
    let rot = body_rotation(m.yaw, d.pitch, d.bank);
    tf.translation = m.pos + DVec3::Y * (lift * k) as f64;
    tf.rotation = rot;
    // Feet on the floor are placed level and turned into its tilted frame.
    let feet = match (feet_level, feet_body) {
        (_, Some(f)) => f,
        (Some(f), None) => {
            let tilt = Quat::from_rotation_x(d.pitch) * Quat::from_rotation_z(d.bank);
            let inv = tilt.inverse();
            let lifted = Vec3::Y * lift;
            f.map(|p| inv * (p - lifted))
        }
        (None, None) => tucked_feet(),
    };
    let st = Stance {
        wings,
        head,
        look,
        jaw,
        feet,
        tail_lift,
        tail_swing,
        heart,
        age: m.age,
    };
    let pose = pose_dragon(k, &st);
    let assembling = d.phase == Phase::Assemble;
    for &p in &m.parts {
        let Ok((part, mut ptf)) = parts.get_mut(p) else {
            continue;
        };
        let Some(&(_, at, r, scale)) = pose.get(part.index).filter(|q| q.0 == part.index) else {
            continue;
        };
        let (mut at, mut r, mut scale) = (at, r, scale);
        if assembling && let Some(&(from, from_rot, delay)) = d.scatter.get(part.index) {
            let u = ((d.t - delay) / FLY_SECS).clamp(0.0, 1.0);
            if d.t < delay {
                // Still in the floor where it will burst up from.
                (at, r, scale) = (from, from_rot, Vec3::ZERO);
            } else {
                // Up out of the floor on an arc, tumbling into its place.
                let e = smooth(u);
                let apex = from.lerp(at, 0.5) + Vec3::Y * (1.3 * k);
                let a = from.lerp(apex, e);
                let b = apex.lerp(at, e);
                at = a.lerp(b, e);
                r = from_rot.slerp(r, e);
                if part.index == HEART {
                    scale = Vec3::splat(0.6 + 0.4 * heart);
                }
            }
        }
        ptf.translation = at.as_dvec3();
        ptf.rotation = r;
        ptf.scale = scale;
    }
    // Bones bursting up out of the floor as they come.
    if assembling {
        let before = d.t - dt;
        for &(from, _, delay) in d.scatter.iter().step_by(5) {
            if before < delay && d.t >= delay {
                let at = tf.translation + (tf.rotation * from).as_dvec3();
                life.queue.looks.push(LookCmd::Burst {
                    at: DVec3::new(at.x, floor, at.z),
                    up: Vec3::Y,
                    look: "raise".into(),
                    role: Role::Impact,
                    scale: 0.45,
                    color: crate::undead::GRAVE_DUST,
                });
            }
        }
    }
}

fn lerp_feet(a: [Vec3; 4], b: [Vec3; 4], t: f32) -> [Vec3; 4] {
    [
        a[0].lerp(b[0], t),
        a[1].lerp(b[1], t),
        a[2].lerp(b[2], t),
        a[3].lerp(b[3], t),
    ]
}

/// Its body's turn: heading, then pitch (nose up), then bank (into a left turn, its right wing
/// up).
pub fn body_rotation(yaw: f32, pitch: f32, bank: f32) -> Quat {
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Quat::from_rotation_z(bank)
}

pub fn register(app: &mut App) {
    app.init_resource::<DragonMeshes>().register_api(
        "magic.dragons",
        "Bone dragons now: whose, where, what they do (assembling, rearing, taking off, \
         circling, landing, on the floor), laps of the circle, gouts of breath, health.",
        "{}",
        |world, _args| {
            let mut q =
                world.query::<(Entity, &Dragon, &Minion, Option<&crate::caster::Vitality>)>();
            let dragons: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, d, m, v)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "phase": format!("{:?}", d.phase),
                        "pos": [m.pos.x, m.pos.y, m.pos.z],
                        "laps": d.laps,
                        "gouts": d.gouts,
                        "health": v.map(|v| v.health),
                    })
                })
                .collect();
            Ok(serde_json::json!({ "dragons": dragons }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stance() -> Stance {
        Stance {
            wings: WingPose::FOLDED,
            head: Vec3::new(0.0, 1.15, -1.95),
            look: Quat::IDENTITY,
            jaw: 0.0,
            feet: walking_feet(0.0, 0.0, 0.0),
            tail_lift: 0.0,
            tail_swing: 0.0,
            heart: 1.0,
            age: 0.0,
        }
    }

    #[test]
    fn every_piece_is_posed_once_in_order() {
        let pose = pose_dragon(1.0, &stance());
        assert_eq!(pose.len(), PARTS);
        for (i, p) in pose.iter().enumerate() {
            assert_eq!(p.0, i, "piece {i} out of order");
            assert!(p.1.is_finite() && p.2.is_finite());
        }
    }

    #[test]
    fn it_stands_on_its_feet_with_its_head_high_and_its_tail_behind() {
        let pose = pose_dragon(1.0, &stance());
        for f in [FORE[0] + 2, FORE[1] + 2, HIND[0] + 2, HIND[1] + 2] {
            let y = pose[f].1.y;
            assert!((y + STAND_H).abs() < 0.12, "foot {f} at {y}");
        }
        assert!(pose[SKULL].1.y > 1.0 && pose[SKULL].1.z < -1.8);
        let tip = pose[*TAIL.last().unwrap()].1;
        assert!(tip.z > 3.0, "{tip}");
        // Long: snout to tail tip.
        let snout = pose[SKULL].1.z - 0.7;
        assert!(tip.z - snout > 5.5, "{}", tip.z - snout);
    }

    #[test]
    fn a_wingbeat_swings_the_tip_further_than_the_arm_and_behind_it() {
        let top = WingPose::beat(0.0, 0.8);
        let bottom = WingPose::beat(0.5, 0.8);
        assert!(top.flap > 0.8 && bottom.flap < -0.6);
        let tip = |w: WingPose| w.flap + w.fold;
        // The tip lags: at the top of the stroke it is still rising.
        assert!(tip(WingPose::beat(0.1, 0.8)) > tip(WingPose::beat(0.0, 0.8)) - 0.2);
        // Over a whole beat the tip swings further than the arm.
        let swing = |f: &dyn Fn(WingPose) -> f32| {
            let v: Vec<f32> = (0..32)
                .map(|i| f(WingPose::beat(i as f32 / 32.0, 0.8)))
                .collect();
            v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(swing(&tip) > swing(&|w: WingPose| w.flap));
        // Spread, its wingspan is wide; folded, it is narrow.
        let wide = |w: WingPose| {
            let mut st = stance();
            st.wings = w;
            let p = pose_dragon(1.0, &st);
            let (l, r) = (p[WING_HAND[0]], p[WING_HAND[1]]);
            let reach = |q: Pose, s: f32| {
                (q.1 + q.2 * (art::FINGERS[0] * Vec3::new(s, 1.0, 1.0)))
                    .x
                    .abs()
            };
            reach(l, -1.0) + reach(r, 1.0)
        };
        assert!(wide(WingPose::SPREAD) > 6.0, "{}", wide(WingPose::SPREAD));
        assert!(wide(WingPose::FOLDED) < wide(WingPose::SPREAD) * 0.6);
    }

    #[test]
    fn its_bones_come_in_order_and_the_heart_last() {
        assert_eq!(assembly_delay(PELVIS), 0.0);
        assert!(assembly_delay(BACK[0]) < assembly_delay(NECK[0]));
        assert!(assembly_delay(NECK[6]) < assembly_delay(SKULL));
        for i in 0..PARTS {
            assert!(assembly_delay(i) + FLY_SECS <= ASSEMBLE_SECS + 1e-4, "{i}");
            if i != HEART {
                assert!(assembly_delay(i) < assembly_delay(HEART), "{i}");
            }
        }
    }
}
