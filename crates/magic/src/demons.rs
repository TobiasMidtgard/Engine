//! A pact's demons (round-15-plan.md): the Pactbinding effect `demon`. The cast type decides
//! how they come:
//!
//! * **Hellgate** (Summon): a gate of black horned stone opens where the caster points, its
//!   middle a churning red-black membrane with fire round its rim ([`Hellgate`]). Two
//!   hellhounds leap out of it first (one more per Split), then an imp every [`GATE_IMP_EVERY`]
//!   s while the caster has fewer than its limit. The gate has health: foes' creatures go for it
//!   like any foe, and broken, it closes (Doom's gore nests). Its demons stay when it closes,
//!   until their own time ends.
//! * **Demonic meteor** (Bolt, Lob): the bolt is a rock of black stone and fire ([`MeteorRide`]);
//!   where it lands it bursts and three imps leap out of the crater (World of Warcraft's Hand of
//!   Gul'dan).
//!
//! The demons are minions (summon.rs): they take their summoner's side, fight creatures first,
//! are hurt, imbued, commanded and terrified like any, and burn away to ash when they fall.
//!
//! * **Imp** ([`MinionKind::Imp`]): a crimson, horned little devil on bat wings that beat. It
//!   flies 1.6 to 2.6 m up, keeps [`IMP_KEEP_M`] from its foe, darts aside between throws and
//!   hurls a firebolt every [`IMP_EVERY`] s after winding up with fire in its hand. Fragile.
//! * **Hellhound** ([`MinionKind::Hellhound`]): a hound of charred stone with fire in its cracks
//!   and a mane of flame. It runs in a trot (diagonal pairs of legs together, Overgrowth's and
//!   Spore's procedural gaits), pounces on its foe from up to [`POUNCE_MAX_M`] in an arc and bites
//!   (pierce, sets fire). Its legs are two-bone IK from shoulder and hip to paws a gait phase
//!   puts on the floor; its hind legs bend at a hock, as a dog's do.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{
    Image, MaterialRef, Mesh, MeshRef, MeshRenderer, PointLight, Shape, StandardMaterial, Vertex,
};
use engine_world::WorldShift;
use engine_world::physics::{
    BodyKind, Collider, ColliderShape, PhysicsQueries, QueryFilter, RigidBody,
};

use crate::caster::{MagicCaster, Vitality};
use crate::compile::{CastContext, Spell, compile};
use crate::design::{CastType, SpellDesign, TargetFilter};
use crate::effects::ForceDir;
use crate::looks::{LookCmd, Role};
use crate::pact_art::{self, VortexLayer};
use crate::runtime::{
    CastInfo, HitQueue, MagicQueue, Missile, PendingHit, ground_below, release, roll,
};
use crate::schools::Mastery;
use crate::summon::{Act, Minion, MinionKind, MinionPart, NewMinion, two_bone};

// ------------------------------------------------------------------------------- numbers

/// A gate's opening: its stones climb out of the floor, then the membrane opens, s.
pub const GATE_OPEN_SECS: f32 = 1.0;
/// And its closing, s.
pub const GATE_CLOSE_SECS: f32 = 0.8;
/// When the first hound leaps out, and the time between hounds, s.
pub const GATE_FIRST_HOUND: f32 = 1.1;
pub const GATE_HOUND_EVERY: f32 = 0.8;
/// When the first imp comes, and the time between imps, s.
pub const GATE_FIRST_IMP: f32 = 3.0;
pub const GATE_IMP_EVERY: f32 = 2.5;
/// A gate's size, m: its opening's half-width and half-height, and its middle's height.
pub const GATE_RX: f32 = 1.15;
pub const GATE_RY: f32 = 1.45;
pub const GATE_MID: f32 = 1.6;
/// A gate's health (times its size).
pub const GATE_HEALTH: f32 = 450.0;
/// Its light, lm.
pub const GATE_LIGHT_LM: f32 = 7000.0;
/// The vortex's glow (its swirl; the arms and the eye's rim glow hotter), and the rim fire's,
/// nits.
pub const VORTEX_GLOW: f32 = 1100.0;
pub const RIM_GLOW: f32 = 3200.0;
/// How fast the vortex turns, rad/s: its swirl slowly and its arms faster, both the way that
/// draws the spirals in towards the eye (which turns against them).
pub const SWIRL_TURN: f32 = 0.3;
pub const ARMS_TURN: f32 = 0.9;
/// Hellstone's grain (and its cracks' tile), m, and the cracks' glow, nits.
pub const HELLSTONE_TILE_M: f32 = 1.5;
pub const CRACK_GLOW: f32 = 1600.0;
/// Fires burn at a gate's feet (a torch fire lasts 8 s), lit again before they go out, s.
pub const GATE_FIRE_EVERY: f32 = 6.5;

/// An imp flies this high over the floor (bobbing), m.
pub const IMP_HOVER_M: f32 = 2.1;
/// It keeps this far from its foe, m, and throws a firebolt every few seconds after a wind-up.
pub const IMP_KEEP_M: f32 = 7.0;
pub const IMP_EVERY: f32 = 1.5;
pub const IMP_WINDUP: f32 = 0.4;
/// Its fastest flight, m/s, and how often it darts to a new spot, s.
pub const IMP_PACE: f32 = 6.5;
pub const IMP_DART_EVERY: f32 = 1.7;
/// Wingbeats a second.
pub const IMP_FLAP_HZ: f32 = 5.5;
/// Imps' health and hounds' (times their size).
pub const IMP_HEALTH: f32 = 90.0;
pub const HOUND_HEALTH: f32 = 340.0;

/// A hound's run, m/s (times its size), and its turn, rad/s.
pub const HOUND_RUN: f32 = 6.0;
pub const HOUND_TURN: f32 = 5.0;
/// It pounces on a foe between these distances, m, every few seconds at most.
pub const POUNCE_MIN_M: f32 = 2.4;
pub const POUNCE_MAX_M: f32 = 6.5;
pub const POUNCE_SECS: f32 = 0.55;
pub const POUNCE_H: f32 = 0.9;
pub const POUNCE_EVERY: f32 = 3.5;
/// A bite: the jaws open, snap shut at the hit, and it draws back, s.
pub const BITE_SECS: f32 = 0.55;
pub const BITE_HIT: f32 = 0.26;
pub const BITE_REST: f32 = 0.45;
/// Out of a gate it leaps this far forward, m.
pub const GATE_LEAP_M: f32 = 3.2;

/// Hellfire's colours: the hounds' cracks, the imps' eyes.
pub const HELLFIRE: [f32; 3] = crate::combine::DEMON_COLOR;
pub const EYE_GLOW: [f32; 3] = [1.0, 0.72, 0.18];

// ------------------------------------------------------------------------------- the gate

/// A hellgate to open, or a demonic meteor that landed (from a cast or a hit).
#[derive(Debug, Clone)]
pub struct NewHellgate {
    pub cast: CastInfo,
    /// The floor where it stands (where the meteor struck).
    pub at: DVec3,
    /// Which way it faces (its demons come out this way).
    pub facing: Vec3,
    pub meteor: bool,
}

/// An open hellgate.
#[derive(Component, Debug, Clone)]
pub struct Hellgate {
    pub cast: CastInfo,
    pub at: DVec3,
    pub facing: Vec3,
    pub age: f32,
    pub life: f32,
    pub size: f32,
    /// Hounds and imps it has sent.
    pub hounds: u32,
    pub imps: u32,
    /// Its stones (climbing out of the floor) and its membrane: the holder that stretches it
    /// to the gate's oval, then the vortex's layers (swirl, arms, eye) and the rim's fire.
    pub stones: Vec<(Entity, Vec3, Quat)>,
    pub membrane: Vec<Entity>,
    /// When the fires at its feet were last lit, s of its age.
    pub fires_lit: Option<f32>,
    /// Seconds since it started to close.
    pub closing: Option<f32>,
}

impl Hellgate {
    pub fn left(&self) -> f32 {
        (self.life - self.age).max(0.0)
    }

    /// How open it is, 0..1.
    pub fn open(&self) -> f32 {
        let opening = smooth((self.age - 0.45) / (GATE_OPEN_SECS - 0.45));
        let closing = self
            .closing
            .map_or(1.0, |t| 1.0 - smooth(t / (GATE_CLOSE_SECS * 0.6)));
        opening * closing
    }
}

/// A demonic meteor's rock, drawn on its bolt.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct MeteorRide {
    pub rock: Option<Entity>,
}

/// What a pact's pieces are drawn with, made once: an imp's wing, the disc a gate's vortex is
/// painted on, the vortex's pictures and hellstone's cracks (`pact_art`).
#[derive(Resource, Debug, Default)]
pub struct DemonMeshes {
    wing: Option<Handle<Mesh>>,
    disc: Option<Handle<Mesh>>,
    vortex: Option<[Handle<Image>; 3]>,
    cracks: Option<Handle<Image>>,
}

impl DemonMeshes {
    pub fn wing(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        *self.wing.get_or_insert_with(|| meshes.add(wing_mesh()))
    }

    pub fn disc(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        *self
            .disc
            .get_or_insert_with(|| meshes.add(pact_art::disc_mesh()))
    }

    pub fn vortex(&mut self, layer: VortexLayer, images: &mut Assets<Image>) -> Handle<Image> {
        let all = self
            .vortex
            .get_or_insert_with(|| VortexLayer::ALL.map(|l| images.add(l.image())));
        all[layer as usize]
    }

    pub fn cracks(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        *self
            .cracks
            .get_or_insert_with(|| images.add(pact_art::crack_image()))
    }
}

/// An imp's wing, waiting for its mesh.
#[derive(Component, Debug, Clone, Copy)]
pub struct Wing;

/// A layer of a gate's vortex, waiting for its disc and picture.
#[derive(Component, Debug, Clone, Copy)]
pub struct Vortex(pub VortexLayer);

/// A piece of hellstone (a gate's stone or horn, a hound's body, a meteor's rock), waiting for
/// its glowing cracks.
#[derive(Component, Debug, Clone, Copy)]
pub struct Hellstone;

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

/// The stone of a gate and of a hound: black basalt, its grain fine and anchored to the piece
/// (hounds run), with fire glowing out of its cracks once they are drawn (`dress_demons`).
pub fn hellstone() -> StandardMaterial {
    crate::summon::body_material(Some(crate::schools::Element::Fire))
        .with_tint(0.07, 0.06, 0.055)
        .with_texture_scale(HELLSTONE_TILE_M)
        .with_triplanar()
}

/// Hellstone with its cracks glowing.
pub fn cracked_hellstone(cracks: Handle<Image>) -> StandardMaterial {
    hellstone()
        .with_emissive([1.0, 1.0, 1.0], CRACK_GLOW)
        .with_emissive_texture(cracks)
}

/// A vortex layer's material: its picture glowing, cut out where it is open.
fn vortex_material(layer: VortexLayer, picture: Handle<Image>) -> StandardMaterial {
    let nits = match layer {
        VortexLayer::Swirl => VORTEX_GLOW,
        VortexLayer::Arms => VORTEX_GLOW * 2.6,
        VortexLayer::Eye => VORTEX_GLOW * 2.2,
    };
    let m = StandardMaterial::default()
        .with_tint(0.03, 0.005, 0.004)
        .with_roughness(0.9)
        .with_base_color_texture(picture)
        .with_emissive([1.0, 1.0, 1.0], nits)
        .with_emissive_texture(picture);
    match layer {
        VortexLayer::Swirl => m,
        _ => m.with_alpha_mask(0.5),
    }
}

fn glow_material(color: [f32; 3], nits: f32) -> StandardMaterial {
    StandardMaterial::default()
        .with_tint(color[0], color[1], color[2])
        .with_emissive(color, nits)
}

fn shape(s: Shape, m: StandardMaterial) -> MeshRenderer {
    MeshRenderer::new(MeshRef::Shape(s), MaterialRef::Inline(m))
}

fn capsule(r: f32, h: f32) -> Shape {
    Shape::Capsule {
        radius: r,
        height: h.max(2.0 * r),
        segments: 12,
    }
}

fn cone(r: f32, h: f32) -> Shape {
    Shape::Cone {
        radius: r,
        height: h,
        segments: 10,
    }
}

fn sphere(r: f32) -> Shape {
    Shape::Sphere {
        radius: r,
        segments: 14,
    }
}

fn rock(size: Vec3, seed: u64) -> Shape {
    Shape::Rock {
        size: size.to_array(),
        seed: (seed % 100_000) as u32,
        detail: 2,
    }
}

/// Open the gates and burst the meteors the casts and hits asked for.
pub fn spawn_hellgates(mut queue: ResMut<MagicQueue>, mut commands: Commands) {
    for g in std::mem::take(&mut queue.hellgates) {
        let k = crate::summon::size_of(&g.cast.spell, g.cast.scale);
        let facing = Vec3::new(g.facing.x, 0.0, g.facing.z).normalize_or(Vec3::NEG_Z);
        if g.meteor {
            meteor_lands(&g, k, facing, &mut queue);
            continue;
        }
        let seed = g.cast.seed as u64;
        let rot = Quat::from_rotation_y(yaw_of(facing));
        let root = commands
            .spawn((
                Name::new("Hellgate"),
                Transform::from_translation(g.at).with_rotation(rot),
                MagicCaster {
                    learns: false,
                    side: g.cast.side,
                    seed: g.cast.seed as u64,
                    mana: 0.0,
                    max_mana: 0.0,
                    mana_regen: 0.0,
                    ..MagicCaster::default()
                },
                Vitality::new(GATE_HEALTH * k),
                PointLight {
                    color: HELLFIRE,
                    intensity_lm: 0.0,
                    range: 11.0,
                    radius: 0.6,
                },
            ))
            .id();
        if let Some(f) = g.cast.faction {
            commands.entity(root).insert(f);
        }
        // The rim: jagged shards of hellstone round an upright oval, jutting out, the biggest
        // at its foot, horns at its crown and shoulders sweeping out and up.
        let stone = hellstone();
        let mut stones = Vec::new();
        let n = 18;
        for i in 0..n {
            let a = (i as f32 + 0.3 * (roll(seed, 80 + i as u64) - 0.5)) / n as f32 * TAU;
            let (s, c) = a.sin_cos();
            let foot = (1.0 - c).max(0.0) * 0.5;
            let w = (0.27 + 0.16 * foot + 0.1 * roll(seed, 20 + i as u64)) * k;
            let size = Vec3::new(w, w * (1.7 + 0.8 * roll(seed, 40 + i as u64)), w * 0.8);
            let out = Vec3::new(s, c, 0.0);
            // Shards lean a little off the ring's line, fore and aft and round it.
            let tilt = Vec3::new(
                c * 0.35 * (roll(seed, 100 + i as u64) - 0.5),
                -s * 0.35 * (roll(seed, 100 + i as u64) - 0.5),
                0.4 * (roll(seed, 120 + i as u64) - 0.5),
            );
            let lean = Quat::from_rotation_arc(Vec3::Y, (out + tilt).normalize())
                * Quat::from_rotation_y(roll(seed, 60 + i as u64) * TAU);
            // Mostly outside the ring: the vortex fills it.
            let at = Vec3::new(s * GATE_RX, GATE_MID + c * GATE_RY, 0.0) * k + out * size.y * 0.2;
            let e = commands
                .spawn((
                    Name::new("Hellgate stone"),
                    Transform::from_translation(at.as_dvec3()).with_rotation(lean),
                    shape(rock(size, seed.wrapping_add(i as u64 * 7919)), stone),
                    Hellstone,
                    ChildOf(root),
                ))
                .id();
            stones.push((e, at, lean));
        }
        for (i, (x, y, out)) in [
            (0.0, 1.0, Vec3::new(0.0, 1.0, 0.35)),
            (-0.55, 0.85, Vec3::new(-0.7, 0.8, 0.2)),
            (0.55, 0.85, Vec3::new(0.7, 0.8, 0.2)),
            (-1.0, 0.2, Vec3::new(-1.0, 0.45, 0.1)),
            (1.0, 0.2, Vec3::new(1.0, 0.45, 0.1)),
        ]
        .into_iter()
        .enumerate()
        {
            let at = Vec3::new(x * GATE_RX * 1.12, GATE_MID + y * GATE_RY * 1.12, 0.0) * k;
            let len = if i == 0 { 1.0 } else { 0.72 } * k;
            let lean = Quat::from_rotation_arc(Vec3::Y, out.normalize());
            let e = commands
                .spawn((
                    Name::new("Hellgate horn"),
                    Transform::from_translation((at + lean * Vec3::Y * len * 0.5).as_dvec3())
                        .with_rotation(lean),
                    shape(cone(0.13 * k, len), stone),
                    Hellstone,
                    ChildOf(root),
                ))
                .id();
            stones.push((e, at + lean * Vec3::Y * len * 0.5, lean));
        }
        // The membrane: a vortex of hellfire in the ring. The holder stretches round discs to
        // the ring's oval (so the layers turn inside it); the gate faces -Z, so the nearer
        // layers sit at smaller z: the swirl at the back, the bright arms turning the other way
        // over it, the black eye at its heart, and a ring of fire where it meets the stones.
        let holder = commands
            .spawn((
                Name::new("Hellgate membrane"),
                Transform::from_translation(DVec3::new(0.0, (GATE_MID * k) as f64, 0.0))
                    .with_scale(Vec3::splat(1e-3)),
                ChildOf(root),
            ))
            .id();
        let mut membrane = vec![holder];
        for (layer, z) in [
            (VortexLayer::Swirl, 0.05),
            (VortexLayer::Arms, 0.02),
            (VortexLayer::Eye, -0.02),
        ] {
            membrane.push(
                commands
                    .spawn((
                        Name::new("Hellgate vortex"),
                        Transform::from_translation(DVec3::new(0.0, 0.0, z)),
                        // A stand-in until `dress_demons` paints it.
                        MeshRenderer::new(
                            MeshRef::Shape(Shape::Quad { size: [0.01, 0.01] }),
                            MaterialRef::Inline(glow_material([0.4, 0.02, 0.01], 0.0)),
                        )
                        .without_shadows(),
                        Vortex(layer),
                        ChildOf(holder),
                    ))
                    .id(),
            );
        }
        membrane.push(
            commands
                .spawn((
                    Name::new("Hellgate rim fire"),
                    Transform::from_rotation(Quat::from_rotation_x(PI * 0.5)),
                    MeshRenderer::new(
                        MeshRef::Shape(Shape::Torus {
                            radius: 1.0,
                            tube: 0.05,
                            segments: 48,
                        }),
                        MaterialRef::Inline(glow_material([1.0, 0.2, 0.03], RIM_GLOW)),
                    )
                    .without_shadows(),
                    ChildOf(holder),
                ))
                .id(),
        );
        // A solid frame bolts strike.
        commands.entity(root).insert((
            Collider {
                shape: ColliderShape::Compound(vec![(
                    Vec3::new(0.0, GATE_MID * k, 0.0),
                    Quat::IDENTITY,
                    ColliderShape::Cuboid {
                        half_extents: Vec3::new(GATE_RX * 1.1, GATE_RY * 1.1, 0.25) * k,
                    },
                )]),
                ..Default::default()
            },
            RigidBody {
                kind: BodyKind::Kinematic,
                ..Default::default()
            },
            Hellgate {
                life: g.cast.spell.payload.demon_secs.max(GATE_OPEN_SECS + 1.0),
                cast: g.cast,
                at: g.at,
                facing,
                age: 0.0,
                size: k,
                hounds: 0,
                imps: 0,
                stones,
                membrane,
                fires_lit: None,
                closing: None,
            },
        ));
        queue.looks.push(LookCmd::Burst {
            at: g.at,
            up: Vec3::Y,
            look: "stone".into(),
            role: Role::Impact,
            scale: 1.6 * k,
            color: HELLFIRE,
        });
        queue.looks.push(LookCmd::Effect {
            name: "embers",
            at: g.at + DVec3::Y * 0.5,
            up: Vec3::Y,
            scale: 1.5 * k,
        });
    }
}

/// A meteor landed: a burst of fire and stone, and three imps leap out of the crater.
fn meteor_lands(g: &NewHellgate, k: f32, facing: Vec3, queue: &mut MagicQueue) {
    queue.looks.push(LookCmd::Burst {
        at: g.at + DVec3::Y * 0.2,
        up: Vec3::Y,
        look: "stone".into(),
        role: Role::Impact,
        scale: 1.4 * k,
        color: HELLFIRE,
    });
    queue.looks.push(LookCmd::Burst {
        at: g.at + DVec3::Y * 0.4,
        up: Vec3::Y,
        look: "fire".into(),
        role: Role::Impact,
        scale: 1.3 * k,
        color: HELLFIRE,
    });
    queue.looks.push(LookCmd::Effect {
        name: "embers",
        at: g.at + DVec3::Y * 0.3,
        up: Vec3::Y,
        scale: 1.2 * k,
    });
    queue.looks.push(LookCmd::Mark {
        at: g.at,
        normal: Vec3::Y,
        kind: "scorch",
        radius: 1.3 * k,
    });
    let n = g.cast.spell.payload.demons.max(1);
    let right = Vec3::new(-facing.z, 0.0, facing.x);
    for i in 0..n {
        let a = (i as f32 - (n - 1) as f32 * 0.5) * 0.9;
        let dir = (facing * a.cos() + right * a.sin()).normalize_or(facing);
        queue.minions.push(NewMinion {
            kind: MinionKind::Imp,
            cast: CastInfo {
                seed: g.cast.seed.wrapping_add(i * 7919 + 1),
                ..g.cast.clone()
            },
            at: g.at + (dir * 0.4).as_dvec3(),
            facing: dir,
            on: None,
        });
    }
}

/// Gates open, send their demons, and close at the end of their time or their health.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_hellgates(
    time: Res<Time>,
    mut gates: Query<(Entity, &mut Hellgate, &Vitality, &mut PointLight)>,
    mut pieces: Query<&mut Transform, (Without<Hellgate>, Without<Minion>)>,
    minions: Query<&Minion>,
    mut queue: ResMut<MagicQueue>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut g, vit, mut light) in &mut gates {
        g.age += dt;
        let k = g.size;
        if g.closing.is_none() && (g.age >= g.life || vit.down()) {
            g.closing = Some(0.0);
            queue.looks.push(LookCmd::Burst {
                at: g.at + DVec3::Y * (GATE_MID * k) as f64,
                up: g.facing,
                look: "fire".into(),
                role: Role::Impact,
                scale: 1.4 * k,
                color: HELLFIRE,
            });
        }
        if let Some(t) = g.closing.as_mut() {
            *t += dt;
            if *t >= GATE_CLOSE_SECS {
                queue.looks.push(LookCmd::Burst {
                    at: g.at,
                    up: Vec3::Y,
                    look: "stone".into(),
                    role: Role::Impact,
                    scale: 1.2 * k,
                    color: HELLFIRE,
                });
                commands.entity(e).despawn();
                continue;
            }
        }
        // Stones climb out of the floor one after another, and sink back as it closes.
        let n = g.stones.len().max(1);
        for (i, &(s, at, rot)) in g.stones.iter().enumerate() {
            let start = 0.05 + 0.4 * i as f32 / n as f32;
            let up = smooth((g.age - start) / 0.35);
            let down = g.closing.map_or(0.0, |t| smooth((t - 0.2) / 0.6));
            let sunk = (1.0 - up + down).clamp(0.0, 1.0);
            if let Ok(mut tf) = pieces.get_mut(s) {
                let depth = (at.y + 0.6 * k) * sunk;
                tf.translation = (at - Vec3::Y * depth).as_dvec3();
                tf.rotation = rot;
            }
        }
        // The membrane opens out of a point and swirls: the holder stretches it to the oval and
        // breathes, the swirl turns slowly one way, the arms faster the other, the eye pulses.
        let open = g.open();
        let breathe = 1.0 + 0.025 * (g.age * 2.7).sin();
        for (i, &m) in g.membrane.iter().enumerate() {
            let Ok(mut tf) = pieces.get_mut(m) else {
                continue;
            };
            match i {
                0 => {
                    let r = (k * open * breathe).max(1e-3);
                    tf.scale = Vec3::new(GATE_RX * r, GATE_RY * r, 1.0);
                }
                1 => tf.rotation = Quat::from_rotation_z(g.age * SWIRL_TURN),
                2 => {
                    tf.rotation = Quat::from_rotation_z(g.age * ARMS_TURN);
                    tf.scale = Vec3::splat(0.97);
                }
                3 => {
                    let pulse = 0.3 + 0.035 * (g.age * 2.3).sin() + 0.015 * (g.age * 5.1).sin();
                    tf.rotation = Quat::from_rotation_z(-g.age * ARMS_TURN * 0.5);
                    tf.scale = Vec3::new(pulse, pulse, 1.0);
                }
                _ => {}
            }
        }
        // Fires burn at its feet while it stands, lit again before they go out.
        if open > 0.5
            && g.closing.is_none()
            && g.left() > 2.0
            && g.fires_lit.is_none_or(|t| g.age - t >= GATE_FIRE_EVERY)
        {
            g.fires_lit = Some(g.age);
            let right = Vec3::new(-g.facing.z, 0.0, g.facing.x);
            for side in [-1.0f32, 1.0] {
                let at = g.at
                    + (right * side * GATE_RX * 0.85 * k + g.facing * 0.3 * k).as_dvec3()
                    + DVec3::Y * 0.05;
                queue.looks.push(LookCmd::Effect {
                    name: "fire.torch",
                    at,
                    up: Vec3::Y,
                    scale: 2.4 * k,
                });
            }
        }
        light.intensity_lm = GATE_LIGHT_LM * open * (1.0 + 0.12 * (g.age * 7.0).sin());
        if g.closing.is_some() || open < 0.95 {
            continue;
        }
        // Out come the hounds, then imps while the caster has fewer than its limit.
        let hounds = g.cast.spell.payload.demons.max(1);
        let caster = g.cast.caster;
        let mastery = g.cast.spell.mastery;
        let living = |kind: MinionKind| {
            minions
                .iter()
                .filter(|m| m.kind == kind && m.cast.caster == caster && m.age < m.life)
                .count()
        };
        let mut send = None;
        if g.hounds < hounds && g.age >= GATE_FIRST_HOUND + g.hounds as f32 * GATE_HOUND_EVERY {
            g.hounds += 1;
            send = Some(MinionKind::Hellhound);
        } else if g.age >= GATE_FIRST_IMP + g.imps as f32 * GATE_IMP_EVERY {
            g.imps += 1;
            if living(MinionKind::Imp) < crate::summon::limit(MinionKind::Imp, mastery) {
                send = Some(MinionKind::Imp);
            }
        }
        if let Some(kind) = send {
            let nth = g.hounds * 31 + g.imps;
            let side = (roll(g.cast.seed as u64, 300 + nth as u64) - 0.5) * 0.9;
            let right = Vec3::new(-g.facing.z, 0.0, g.facing.x);
            let dir = (g.facing + right * side).normalize_or(g.facing);
            queue.minions.push(NewMinion {
                kind,
                cast: CastInfo {
                    seed: g.cast.seed.wrapping_add(nth * 104_729 + 17),
                    ..g.cast.clone()
                },
                at: g.at,
                facing: dir,
                on: None,
            });
            queue.looks.push(LookCmd::Burst {
                at: g.at + DVec3::Y * (GATE_MID * k) as f64 + (g.facing * 0.3).as_dvec3(),
                up: g.facing,
                look: "fire".into(),
                role: Role::Impact,
                scale: 0.8 * k,
                color: HELLFIRE,
            });
        }
    }
}

/// A demonic meteor's rock rides its bolt, turning over as it flies.
pub fn ride_meteors(
    time: Res<Time>,
    mut rides: Query<(Entity, &Missile, &mut MeteorRide)>,
    mut rocks: Query<&mut Transform, Without<MeteorRide>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, m, mut ride) in &mut rides {
        match ride.rock {
            None if m.spent.is_none() => {
                let k = crate::summon::size_of(&m.cast.spell, m.cast.scale);
                let size = Vec3::splat(0.55 * k);
                let r = commands
                    .spawn((
                        Name::new("Demonic meteor"),
                        Transform::default(),
                        shape(rock(size, m.cast.seed as u64), hellstone()),
                        Hellstone,
                        ChildOf(e),
                    ))
                    .id();
                ride.rock = Some(r);
            }
            Some(r) if m.spent.is_some() => {
                commands.entity(r).despawn();
                ride.rock = None;
            }
            Some(r) => {
                if let Ok(mut tf) = rocks.get_mut(r) {
                    tf.rotation *=
                        Quat::from_rotation_x(dt * 7.0) * Quat::from_rotation_z(dt * 3.0);
                    // The rock's base sits at its origin: centre it on the bolt.
                    tf.translation = tf.rotation.as_dquat() * DVec3::new(0.0, -0.25, 0.0);
                }
            }
            None => {}
        }
    }
}

/// Pieces get their meshes and pictures once they can be made: imps' wings, the gates'
/// vortex layers, hellstone's cracks.
#[allow(clippy::type_complexity)]
pub fn dress_demons(
    mut pieces: Query<
        (
            &mut MeshRenderer,
            Option<&Wing>,
            Option<&Vortex>,
            Option<&Hellstone>,
        ),
        Or<(Added<Wing>, Added<Vortex>, Added<Hellstone>)>,
    >,
    mut dm: ResMut<DemonMeshes>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    images: Option<ResMut<Assets<Image>>>,
) {
    let Some(mut meshes) = meshes else { return };
    let mut images = images;
    for (mut r, wing, vortex, stone) in &mut pieces {
        if wing.is_some() {
            r.mesh = MeshRef::Handle(dm.wing(&mut meshes));
        }
        let Some(images) = images.as_deref_mut() else {
            continue;
        };
        if let Some(&Vortex(layer)) = vortex {
            let picture = dm.vortex(layer, images);
            r.mesh = MeshRef::Handle(dm.disc(&mut meshes));
            r.material = MaterialRef::Inline(vortex_material(layer, picture));
        } else if stone.is_some() {
            r.material = MaterialRef::Inline(cracked_hellstone(dm.cracks(images)));
        }
    }
}

/// Gates move with the world's origin.
pub fn follow_world_shift(mut shifts: MessageReader<WorldShift>, mut q: Query<&mut Hellgate>) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    for mut g in &mut q {
        g.at = s.point(g.at);
        g.facing = s.direction(g.facing);
    }
}

// ------------------------------------------------------------------------------- bodies

/// An imp's pieces, in order.
pub const IMP_PELVIS: usize = 0;
pub const IMP_TORSO: usize = 1;
pub const IMP_HEAD: usize = 2;
pub const IMP_SNOUT: usize = 3;
pub const IMP_HORN: [usize; 2] = [4, 5];
pub const IMP_EYE: [usize; 2] = [6, 7];
pub const IMP_EAR: [usize; 2] = [8, 9];
/// Upper arm, forearm, hand; left then right.
pub const IMP_ARM: [usize; 2] = [10, 13];
/// Thigh, shin, foot; left then right.
pub const IMP_LEG: [usize; 2] = [16, 19];
pub const IMP_TAIL: [usize; 4] = [22, 23, 24, 25];
pub const IMP_WING: [usize; 2] = [26, 27];
pub const IMP_FIRE: usize = 28;
pub const IMP_PARTS: usize = 29;

/// A hound's pieces, in order.
pub const HOUND_CHEST: usize = 0;
pub const HOUND_WAIST: usize = 1;
pub const HOUND_HAUNCH: usize = 2;
pub const HOUND_NECK: usize = 3;
pub const HOUND_HEAD: usize = 4;
pub const HOUND_MUZZLE: usize = 5;
pub const HOUND_JAW: usize = 6;
pub const HOUND_EYE: [usize; 2] = [7, 8];
pub const HOUND_HORN: [usize; 2] = [9, 10];
/// Upper leg, lower leg, paw; left then right.
pub const HOUND_FORE: [usize; 2] = [11, 14];
/// Thigh, shin, hock, paw; left then right.
pub const HOUND_HIND: [usize; 2] = [17, 21];
pub const HOUND_TAIL: [usize; 3] = [25, 26, 27];
/// Spikes along its back, shoulders to rump.
pub const HOUND_SPINE: [usize; 4] = [28, 29, 30, 31];
pub const HOUND_PARTS: usize = 32;

/// A hound's pieces of hellstone (all but its eyes, horns and spikes), which glow in their
/// cracks.
pub fn hound_stone(index: usize) -> bool {
    !HOUND_EYE.contains(&index) && !HOUND_HORN.contains(&index) && !HOUND_SPINE.contains(&index)
}

/// The pieces chains bite on: an imp's hands and feet, a hound's four paws.
pub fn limb_parts(kind: MinionKind) -> [usize; 4] {
    match kind {
        MinionKind::Imp => [
            IMP_ARM[0] + 2,
            IMP_ARM[1] + 2,
            IMP_LEG[0] + 2,
            IMP_LEG[1] + 2,
        ],
        _ => [
            HOUND_FORE[0] + 2,
            HOUND_FORE[1] + 2,
            HOUND_HIND[0] + 3,
            HOUND_HIND[1] + 3,
        ],
    }
}

// Imp proportions at size 1 (about 0.9 m from its feet to its horns), m.
const IMP_SHOULDER: Vec3 = Vec3::new(0.115, 0.29, -0.02);
const IMP_HIP: Vec3 = Vec3::new(0.07, -0.03, 0.02);
const IMP_NECK: Vec3 = Vec3::new(0.0, 0.34, -0.04);
const IMP_WING_ROOT: Vec3 = Vec3::new(0.05, 0.27, 0.08);
const IMP_TAIL_ROOT: Vec3 = Vec3::new(0.0, -0.04, 0.08);
const IMP_UPPER: f32 = 0.14;
const IMP_FORE: f32 = 0.13;
const IMP_THIGH: f32 = 0.14;
const IMP_SHIN: f32 = 0.15;

/// The pieces of an imp at size `k`: (index, mesh). None is a rubble piece: demons burn to ash.
pub fn imp_parts(k: f32) -> Vec<(usize, MeshRenderer)> {
    let skin = StandardMaterial::default()
        .with_tint(0.46, 0.05, 0.035)
        .with_roughness(0.48)
        .with_emissive([1.0, 0.12, 0.03], 2.0);
    let dark = StandardMaterial::default()
        .with_tint(0.06, 0.05, 0.045)
        .with_roughness(0.3);
    let membrane = StandardMaterial::default()
        .with_tint(0.24, 0.03, 0.025)
        .with_roughness(0.62)
        .with_emissive([1.0, 0.1, 0.03], 1.2);
    let mut out = vec![
        (IMP_PELVIS, shape(sphere(0.085 * k), skin)),
        (IMP_TORSO, shape(capsule(0.1 * k, 0.33 * k), skin)),
        (IMP_HEAD, shape(sphere(0.09 * k), skin)),
        (IMP_SNOUT, shape(capsule(0.045 * k, 0.12 * k), skin)),
    ];
    for i in 0..2 {
        out.push((IMP_HORN[i], shape(cone(0.026 * k, 0.15 * k), dark)));
        out.push((
            IMP_EYE[i],
            shape(sphere(0.019 * k), glow_material(EYE_GLOW, 1200.0)).without_shadows(),
        ));
        out.push((IMP_EAR[i], shape(cone(0.024 * k, 0.09 * k), skin)));
        out.push((IMP_ARM[i], shape(capsule(0.032 * k, IMP_UPPER * k), skin)));
        out.push((
            IMP_ARM[i] + 1,
            shape(capsule(0.028 * k, IMP_FORE * k), skin),
        ));
        out.push((IMP_ARM[i] + 2, shape(cone(0.035 * k, 0.08 * k), dark)));
        out.push((IMP_LEG[i], shape(capsule(0.04 * k, IMP_THIGH * k), skin)));
        out.push((
            IMP_LEG[i] + 1,
            shape(capsule(0.032 * k, IMP_SHIN * k), skin),
        ));
        out.push((IMP_LEG[i] + 2, shape(cone(0.03 * k, 0.09 * k), dark)));
        out.push((
            IMP_WING[i],
            MeshRenderer::new(
                MeshRef::Shape(Shape::Quad { size: [0.01, 0.01] }),
                MaterialRef::Inline(membrane),
            ),
        ));
    }
    for (j, &t) in IMP_TAIL.iter().enumerate() {
        if j == 3 {
            out.push((t, shape(cone(0.03 * k, 0.08 * k), dark)));
        } else {
            let r = (0.022 - 0.004 * j as f32) * k;
            out.push((t, shape(capsule(r, 0.13 * k), skin)));
        }
    }
    out.push((
        IMP_FIRE,
        shape(sphere(0.06 * k), glow_material([1.0, 0.45, 0.1], 7000.0)).without_shadows(),
    ));
    out
}

// Hound proportions at size 1 (about 1.7 m from nose to rump, 0.9 m at the shoulder), m.
const SHOULDER: Vec3 = Vec3::new(0.15, 0.68, -0.4);
const HIP: Vec3 = Vec3::new(0.16, 0.7, 0.4);
const PIVOT: Vec3 = Vec3::new(0.0, 0.68, 0.0);
const FORE_UPPER: f32 = 0.34;
const FORE_LOWER: f32 = 0.32;
const HIND_THIGH: f32 = 0.34;
const HIND_SHIN: f32 = 0.33;
const HIND_HOCK: f32 = 0.2;
const PAW_Y: f32 = 0.05;

/// The pieces of a hound at size `k`: a heavy beast of hellstone, deep in the chest, thick in
/// the neck and legs, a crest of spikes down its back.
pub fn hound_parts(k: f32, seed: u64) -> Vec<(usize, MeshRenderer)> {
    let stone = hellstone();
    let dark = StandardMaterial::default()
        .with_tint(0.05, 0.045, 0.04)
        .with_roughness(0.32);
    let r = |i: usize, s: [f32; 3]| {
        (
            i,
            shape(
                rock(Vec3::from_array(s) * k, seed.wrapping_add(i as u64 * 7919)),
                stone,
            ),
        )
    };
    let limb = |i: usize, radius: f32, len: f32| (i, shape(capsule(radius * k, len * k), stone));
    let mut out = vec![
        r(HOUND_CHEST, [0.54, 0.64, 0.6]),
        r(HOUND_WAIST, [0.4, 0.5, 0.42]),
        r(HOUND_HAUNCH, [0.5, 0.5, 0.5]),
        r(HOUND_NECK, [0.32, 0.44, 0.34]),
        r(HOUND_HEAD, [0.34, 0.36, 0.3]),
        r(HOUND_MUZZLE, [0.2, 0.32, 0.16]),
        r(HOUND_JAW, [0.17, 0.28, 0.08]),
    ];
    for i in 0..2 {
        out.push((
            HOUND_EYE[i],
            shape(sphere(0.035 * k), glow_material(EYE_GLOW, 2500.0)).without_shadows(),
        ));
        out.push((HOUND_HORN[i], shape(cone(0.05 * k, 0.3 * k), dark)));
        out.push(limb(HOUND_FORE[i], 0.1, FORE_UPPER));
        out.push(limb(HOUND_FORE[i] + 1, 0.066, FORE_LOWER));
        out.push(r(HOUND_FORE[i] + 2, [0.15, 0.1, 0.2]));
        out.push(limb(HOUND_HIND[i], 0.13, HIND_THIGH));
        out.push(limb(HOUND_HIND[i] + 1, 0.075, HIND_SHIN));
        out.push(limb(HOUND_HIND[i] + 2, 0.055, HIND_HOCK));
        out.push(r(HOUND_HIND[i] + 3, [0.15, 0.1, 0.2]));
    }
    for (j, &t) in HOUND_TAIL.iter().enumerate() {
        out.push(limb(t, 0.06 - 0.014 * j as f32, 0.24));
    }
    for (j, &t) in HOUND_SPINE.iter().enumerate() {
        let len = [0.24, 0.2, 0.15, 0.12][j];
        out.push((t, shape(cone(0.055 * k, len * k), dark)));
    }
    out
}

/// Put a demon's body on a fresh minion root: its pieces, light, collider and looks.
#[allow(clippy::too_many_arguments)]
pub fn build(
    commands: &mut Commands,
    root: Entity,
    kind: MinionKind,
    k: f32,
    seed: u64,
    spell: &Spell,
    parts: &mut Vec<Entity>,
    looks: &mut Vec<LookCmd>,
) {
    let pieces = if kind == MinionKind::Imp {
        imp_parts(k)
    } else {
        hound_parts(k, seed)
    };
    for (index, r) in pieces {
        let id = commands
            .spawn((
                Name::new("Demon piece"),
                Transform::default(),
                r,
                MinionPart {
                    index,
                    size: Vec3::ZERO,
                },
                ChildOf(root),
            ))
            .id();
        if kind == MinionKind::Hellhound && hound_stone(index) {
            commands.entity(id).insert(Hellstone);
        }
        if kind == MinionKind::Imp && IMP_WING.contains(&index) {
            commands.entity(id).insert(Wing);
            // The wing's bones: a dark line along its leading edge and each finger.
            let wing = id;
            for (a, b) in WING_BONES {
                let (a, b) = (Vec3::from_array(a), Vec3::from_array(b));
                let d = b - a;
                commands.spawn((
                    Name::new("Wing bone"),
                    Transform::from_translation(((a + b) * 0.5).as_dvec3())
                        .with_rotation(Quat::from_rotation_arc(Vec3::Y, d.normalize())),
                    shape(
                        capsule(0.012, d.length()),
                        StandardMaterial::default()
                            .with_tint(0.1, 0.02, 0.02)
                            .with_roughness(0.4),
                    ),
                    ChildOf(wing),
                ));
            }
        }
        parts.push(id);
    }
    let color = spell.weave.color;
    match kind {
        MinionKind::Imp => {
            commands.entity(root).insert((
                PointLight {
                    color: [1.0, 0.4, 0.1],
                    intensity_lm: 300.0,
                    range: 3.0,
                    radius: 0.1,
                },
                Collider {
                    shape: ColliderShape::Sphere { radius: 0.32 * k },
                    ..Default::default()
                },
                RigidBody {
                    kind: BodyKind::Kinematic,
                    ..Default::default()
                },
            ));
        }
        _ => {
            commands.entity(root).insert((
                PointLight {
                    color: HELLFIRE,
                    intensity_lm: 900.0,
                    range: 4.5,
                    radius: 0.2,
                },
                Collider {
                    shape: ColliderShape::Compound(vec![(
                        Vec3::new(0.0, 0.66 * k, 0.0),
                        Quat::from_rotation_x(PI * 0.5),
                        ColliderShape::Capsule {
                            radius: 0.34 * k,
                            half_height: 0.5 * k,
                        },
                    )]),
                    density: 1800.0,
                    ..Default::default()
                },
                RigidBody {
                    kind: BodyKind::Kinematic,
                    ..Default::default()
                },
            ));
            // A mane of fire along its neck and back.
            if let Some(&neck) = parts.get(HOUND_NECK) {
                looks.push(LookCmd::Attach {
                    entity: neck,
                    look: "fire".into(),
                    role: Role::Aura,
                    scale: 0.55 * k,
                    color,
                    power_j: spell.power.energy_j as f32 * 0.1,
                });
            }
        }
    }
}

/// An imp's firebolt and a hound's bite: in the weave's element, fire for a plain pact.
pub fn attacks(kind: MinionKind, spell: &Spell, k: f32) -> Arc<Spell> {
    let keys: Vec<&str> = if spell.weave.elements.is_empty() {
        vec!["fire"]
    } else {
        spell.weave.elements.iter().map(|e| e.key()).collect()
    };
    let cast = if kind == MinionKind::Imp {
        CastType::Bolt
    } else {
        CastType::Touch
    };
    let mut s = compile(
        &SpellDesign::new(&keys, cast),
        CastContext::new(spell.mastery, spell.power),
    )
    .map(|s| (*s).clone())
    .unwrap_or_else(|_| {
        let mut s = spell.clone();
        s.cast = cast;
        s
    });
    s.filter = TargetFilter::Enemies;
    s.stages.clear();
    if kind == MinionKind::Hellhound {
        // Fangs: a hard bite that sets fire, and a shove.
        s.payload.damage = spell.power.damage() * 0.22 * k;
        s.payload.damage_kind = engine_world::DamageKind::Pierce;
        s.payload.force = Some((ForceDir::Away, 70.0 * k));
        s.carrier.radius = 0.4;
    } else {
        // An imp's bolt is a little one: a quarter of its master's.
        s.payload.damage *= 0.25;
        s.carrier.radius *= 0.7;
        s.carrier.speed *= 0.85;
    }
    Arc::new(s)
}

/// How many of a kind of demon a caster keeps.
pub fn limit(kind: MinionKind, mastery: Mastery) -> usize {
    match kind {
        MinionKind::Imp => match mastery {
            Mastery::Novice => 2,
            Mastery::Apprentice | Mastery::Adept => 3,
            Mastery::Expert => 4,
            Mastery::Master => 5,
            _ => 6,
        },
        _ => {
            if mastery >= Mastery::Master {
                3
            } else {
                2
            }
        }
    }
}

// ------------------------------------------------------------------------------- the wing

/// A right wing's bones, at size 1: its arm to the wrist, then three fingers (m, in the wing's
/// frame: span along +X, back along +Z).
const WING_BONES: [([f32; 3], [f32; 3]); 4] = [
    ([0.0, 0.0, 0.0], [0.24, 0.07, -0.03]),
    ([0.24, 0.07, -0.03], [0.52, 0.12, 0.03]),
    ([0.24, 0.07, -0.03], [0.46, 0.02, 0.24]),
    ([0.24, 0.07, -0.03], [0.28, -0.04, 0.34]),
];

/// A bat's wing (the right one, at size 1): a membrane stretched between the arm and three
/// fingers, its trailing edge scalloped between their tips; both faces.
pub fn wing_mesh() -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let wrist = Vec3::new(0.24, 0.07, -0.03);
    let rim = [
        Vec3::new(0.52, 0.12, 0.03),
        Vec3::new(0.42, 0.06, 0.12),
        Vec3::new(0.46, 0.02, 0.24),
        Vec3::new(0.33, 0.0, 0.22),
        Vec3::new(0.28, -0.04, 0.34),
        Vec3::new(0.15, -0.03, 0.22),
        Vec3::new(0.02, -0.05, 0.2),
        Vec3::new(0.0, 0.0, 0.0),
    ];
    for w in rim.windows(2) {
        let (a, b, c) = (wrist, w[0], w[1]);
        let n = (b - a).cross(c - a).normalize_or(Vec3::Y);
        for (tri, n) in [([a, b, c], n), ([a, c, b], -n)] {
            let base = m.vertices.len() as u32;
            for p in tri {
                m.vertices.push(Vertex::new(p, n, Vec2::new(p.x, p.z)));
            }
            m.indices.extend([base, base + 1, base + 2]);
        }
    }
    m
}

// ------------------------------------------------------------------------------- living

/// What a demon needs from the minions' step.
pub struct Life<'a> {
    pub e: Entity,
    pub dt: f32,
    pub g: f32,
    pub queries: &'a PhysicsQueries,
    pub exclude: &'a QueryFilter,
    pub target: Option<(Entity, DVec3)>,
    pub owner: Option<(DVec3, Quat)>,
    pub held: bool,
    /// Running from a terror there.
    pub fleeing: Option<DVec3>,
    /// Walkers near it (to keep out of): who, where, how much room.
    pub crowd: &'a [(Entity, DVec3, f32)],
    pub hits: &'a mut HitQueue,
    pub queue: &'a mut MagicQueue,
}

/// One piece's pose in its body's frame.
type Pose = (usize, Vec3, Quat, Vec3);

fn bone(i: usize, a: Vec3, b: Vec3, fallback: Vec3) -> Pose {
    let d = b - a;
    let dir = d.normalize_or(fallback);
    (
        i,
        (a + b) * 0.5,
        Quat::from_rotation_arc(Vec3::Y, dir),
        Vec3::ONE,
    )
}

fn set_parts(
    m: &Minion,
    pose: &[Pose],
    parts: &mut Query<(&MinionPart, &mut Transform), Without<Minion>>,
) {
    for &p in &m.parts {
        if let Ok((part, mut tf)) = parts.get_mut(p)
            && let Some(&(_, at, rot, scale)) = pose.iter().find(|q| q.0 == part.index)
        {
            tf.translation = at.as_dvec3();
            tf.rotation = rot;
            tf.scale = scale;
        }
    }
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

/// An imp lives a step: it flies to a spot off its foe's flank, darts to a new one now and
/// then, and throws firebolts.
pub fn live_imp(
    m: &mut Minion,
    tf: &mut Transform,
    life: &mut Life,
    parts: &mut Query<(&MinionPart, &mut Transform), Without<Minion>>,
) {
    let (e, dt) = (life.e, life.dt);
    let k = m.size;
    // Out of a crater or a gate it climbs to its height first.
    let ground = ground_below(life.queries, m.pos + DVec3::Y * 0.5, life.exclude);
    let seed = m.cast.seed as u64;
    let dart = (m.age / IMP_DART_EVERY).floor() as u64;
    let jitter = |s: u64| roll(seed, dart * 13 + s) - 0.5;
    let hover = IMP_HOVER_M + 0.5 * jitter(1) * 2.0;
    let spot = match (life.target, life.owner, life.fleeing) {
        (_, _, Some(from)) => {
            let away = flat(m.pos - from).normalize_or(DVec3::Z);
            Some(m.pos + away * 4.0)
        }
        (Some((_, tp)), owner, _) => {
            let home = owner.map_or(m.pos, |o| o.0);
            let back = flat(home - tp).normalize_or(flat(m.pos - tp).normalize_or(DVec3::Z));
            let side = DVec3::new(-back.z, 0.0, back.x);
            Some(
                tp + back * (IMP_KEEP_M + 1.5 * jitter(2)) as f64
                    + side * ((2.5 * m.flank) + 3.0 * jitter(3)) as f64,
            )
        }
        (None, Some((op, orot)), _) => Some(
            op + (orot * Vec3::new((1.8 + jitter(4)) * m.flank, 0.0, 0.8 + jitter(5))).as_dvec3(),
        ),
        _ => None,
    };
    let goal = spot
        .map(|p| {
            ground_below(life.queries, p + DVec3::Y, life.exclude)
                + DVec3::Y * (hover * k.sqrt()) as f64
        })
        .unwrap_or(m.pos);
    // Chains drag it down to the floor.
    let goal = if life.held {
        DVec3::new(m.pos.x, ground.y + (0.9 * k) as f64, m.pos.z)
    } else {
        goal
    };
    let pull = 1.0 - (-2.6 * dt).exp();
    let want = ((goal - m.pos) * pull as f64 / dt.max(1e-4) as f64)
        .as_vec3()
        .clamp_length_max(IMP_PACE * k.sqrt());
    m.vel = m.vel.lerp(want, (6.0 * dt).min(1.0));
    m.pos += (m.vel * dt).as_dvec3();
    // Face its foe (or where it flies).
    let face = life
        .target
        .map(|(_, tp)| (tp - m.pos).as_vec3())
        .unwrap_or(m.vel);
    let face =
        Vec3::new(face.x, 0.0, face.z).normalize_or(Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z);
    let diff = wrap(yaw_of(face) - m.yaw);
    m.yaw += diff.clamp(-4.0 * dt, 4.0 * dt);
    // Throw.
    let mut act = m.act;
    let mut windup = 0.0;
    let mut thrown = 0.0;
    match &mut act {
        Act::Shoot { t } => {
            *t += dt;
            windup = (*t / IMP_WINDUP).min(1.0);
            if *t >= IMP_WINDUP {
                if let Some((te, tp)) = life.target {
                    let hand = hand_world(m, tf);
                    let aim = tp + DVec3::Y;
                    let face = (aim - hand).as_vec3().normalize_or(face);
                    let info = minion_cast(m, e, &m.attack, 1.0);
                    release(
                        &info,
                        hand,
                        m.pos,
                        face,
                        aim,
                        Some(te),
                        life.g,
                        life.queries,
                        life.queue,
                        life.hits,
                    );
                }
                life.queue.looks.push(LookCmd::EndCharge { caster: e });
                act = Act::Ready;
                m.cooldown = IMP_EVERY - IMP_WINDUP + 0.4 * (roll(seed, m.attacks as u64) - 0.5);
                m.leap_in = 0.35;
            }
        }
        _ => {
            if let Some((_, tp)) = life.target
                && m.cooldown <= 0.0
                && !life.held
                && life.fleeing.is_none()
                && diff.abs() < 0.6
                && tp.distance(m.pos) <= crate::summon::SHOOT_M as f64
            {
                m.attacks += 1;
                act = Act::Shoot { t: 0.0 };
                life.queue.looks.push(LookCmd::Charge {
                    caster: e,
                    look: m.attack.weave.key.clone(),
                    secs: IMP_WINDUP,
                    scale: 0.25 * k,
                });
            }
        }
    }
    if act == Act::Ready && m.leap_in > 0.0 {
        m.leap_in = (m.leap_in - dt).max(0.0);
        thrown = m.leap_in / 0.35;
    }
    m.act = act;
    // Wings beat faster as it darts, slower as it hangs.
    let speed = m.vel.length() / (IMP_PACE * k.sqrt()).max(1e-3);
    m.phase = (m.phase + dt * IMP_FLAP_HZ * (1.0 + 0.6 * speed)).fract();
    let bob = 0.06 * k * (m.phase * TAU).sin();
    tf.translation = m.pos + DVec3::Y * bob as f64;
    tf.rotation = Quat::from_rotation_y(m.yaw);
    let pose = pose_imp(k, m.phase, speed, windup, thrown, m.age, life.held);
    set_parts(m, &pose, parts);
}

/// Where an imp's throwing hand is in the world.
fn hand_world(m: &Minion, tf: &Transform) -> DVec3 {
    let pose = pose_imp(m.size, m.phase, 0.0, 1.0, 0.0, m.age, false);
    let hand = pose
        .iter()
        .find(|p| p.0 == IMP_FIRE)
        .map(|p| p.1)
        .unwrap_or(Vec3::Y * 0.3);
    tf.translation + (tf.rotation * hand).as_dvec3()
}

/// An imp's pose in its own frame (its middle at the origin, facing -Z): `flap` 0..1 through a
/// wingbeat, `speed` 0..1 of its fastest, `windup` 0..1 through a throw's wind-up, `thrown`
/// 1..0 through the follow-through.
pub fn pose_imp(
    k: f32,
    flap: f32,
    speed: f32,
    windup: f32,
    thrown: f32,
    age: f32,
    held: bool,
) -> Vec<Pose> {
    let lean = Quat::from_rotation_x(-(0.18 + 0.35 * speed.min(1.0)));
    let at = |v: Vec3| lean * (v * k);
    let mut out = Vec::with_capacity(IMP_PARTS);
    let pelvis = at(Vec3::ZERO);
    let neck = at(IMP_NECK);
    out.push((IMP_PELVIS, pelvis, lean, Vec3::ONE));
    out.push(bone(IMP_TORSO, pelvis, neck, Vec3::Y));
    // Head: up from the neck, looking ahead; the snout juts forward.
    let head = neck + lean * (Vec3::new(0.0, 0.08, -0.02) * k);
    let look = lean.inverse() * Quat::from_rotation_x(0.12);
    let hr = look * lean;
    out.push((IMP_HEAD, head, hr, Vec3::new(1.0, 0.95, 1.12)));
    out.push((
        IMP_SNOUT,
        head + hr * (Vec3::new(0.0, -0.03, -0.08) * k),
        hr * Quat::from_rotation_x(-PI * 0.5),
        Vec3::ONE,
    ));
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        // Horns sweep up and back; ears out to the side.
        let horn_dir = hr * Vec3::new(0.35 * s, 0.75, 0.55).normalize();
        let horn_at = head + hr * (Vec3::new(0.045 * s, 0.06, 0.0) * k);
        out.push((
            IMP_HORN[i],
            horn_at + horn_dir * 0.075 * k,
            Quat::from_rotation_arc(Vec3::Y, horn_dir),
            Vec3::ONE,
        ));
        out.push((
            IMP_EYE[i],
            head + hr * (Vec3::new(0.035 * s, 0.015, -0.075) * k),
            hr,
            Vec3::ONE,
        ));
        let ear = hr * Vec3::new(s, 0.25, 0.2).normalize();
        out.push((
            IMP_EAR[i],
            head + hr * (Vec3::new(0.08 * s, 0.02, 0.01) * k) + ear * 0.04 * k,
            Quat::from_rotation_arc(Vec3::Y, ear),
            Vec3::ONE,
        ));
        // Arms: claws held forward; the right one winds back behind its head and throws.
        let shoulder = at(Vec3::new(
            IMP_SHOULDER.x * s,
            IMP_SHOULDER.y,
            IMP_SHOULDER.z,
        ));
        let hang = 0.25 * (flap * TAU).sin();
        let mut up = Vec3::new(0.35 * s, -0.8 + hang * 0.3, -0.45).normalize();
        let mut fore = Vec3::new(0.1 * s, -0.25, -1.0).normalize();
        if held {
            up = Vec3::new(0.85 * s, -0.5, 0.0).normalize();
            fore = Vec3::new(0.7 * s, -0.7, 0.0).normalize();
        } else if s > 0.0 {
            let back_up = Vec3::new(0.35, 0.65, 0.65).normalize();
            let back_fore = Vec3::new(0.0, 0.95, -0.3).normalize();
            let out_up = Vec3::new(0.15, 0.1, -1.0).normalize();
            let out_fore = Vec3::new(0.0, -0.1, -1.0).normalize();
            let w = smooth(windup);
            up = up.lerp(back_up, w).lerp(out_up, thrown).normalize();
            fore = fore.lerp(back_fore, w).lerp(out_fore, thrown).normalize();
        }
        let elbow = shoulder + lean * up * IMP_UPPER * k;
        let wrist = elbow + lean * fore * IMP_FORE * k;
        out.push(bone(IMP_ARM[i], shoulder, elbow, Vec3::NEG_Y));
        out.push(bone(IMP_ARM[i] + 1, elbow, wrist, Vec3::NEG_Y));
        let claw = (wrist - elbow).normalize_or(Vec3::NEG_Y);
        out.push((
            IMP_ARM[i] + 2,
            wrist + claw * 0.03 * k,
            Quat::from_rotation_arc(Vec3::Y, claw),
            Vec3::ONE,
        ));
        if s > 0.0 {
            // The fire in its hand swells through the wind-up.
            let f = if windup > 0.0 {
                0.3 + 0.7 * windup
            } else {
                0.0
            };
            out.push((
                IMP_FIRE,
                wrist + claw * 0.06 * k,
                Quat::IDENTITY,
                Vec3::splat(f.max(1e-3)),
            ));
        }
        // Legs dangle, knees forward, feet back; they swing a little with the wings.
        let hip = at(Vec3::new(IMP_HIP.x * s, IMP_HIP.y, IMP_HIP.z));
        let kick = 0.2 * ((flap + 0.25 * i as f32) * TAU).sin();
        let thigh = Vec3::new(0.12 * s, -0.85, -0.45 - kick).normalize();
        let shin = Vec3::new(0.05 * s, -0.8, 0.55 + kick).normalize();
        let knee = hip + lean * thigh * IMP_THIGH * k;
        let ankle = knee + lean * shin * IMP_SHIN * k;
        out.push(bone(IMP_LEG[i], hip, knee, Vec3::NEG_Y));
        out.push(bone(IMP_LEG[i] + 1, knee, ankle, Vec3::NEG_Y));
        let toe = lean * Vec3::new(0.0, -0.5, -0.85).normalize();
        out.push((
            IMP_LEG[i] + 2,
            ankle + toe * 0.035 * k,
            Quat::from_rotation_arc(Vec3::Y, toe),
            Vec3::ONE,
        ));
        // Wings: hinged on the back, beating about the body's long axis, swept back on the
        // downstroke. The left is the right one mirrored.
        let beat = (flap * TAU).sin();
        let lift = if held { -0.9 } else { 0.25 + 0.85 * beat };
        let sweep = if held { 0.5 } else { 0.25 - 0.3 * beat };
        let root = at(Vec3::new(
            IMP_WING_ROOT.x * s,
            IMP_WING_ROOT.y,
            IMP_WING_ROOT.z,
        ));
        let rot = lean * Quat::from_rotation_z(lift * s) * Quat::from_rotation_y(-sweep * s);
        out.push((
            IMP_WING[i],
            root,
            rot,
            Vec3::new(s * k * 1.15, k * 1.15, k * 1.15),
        ));
    }
    // A tail that curls and sways.
    let mut p = at(IMP_TAIL_ROOT);
    let mut dir = lean * Vec3::new(0.0, -0.55, 0.85).normalize();
    for (j, &t) in IMP_TAIL.iter().enumerate() {
        let sway = 0.45 * (age * 2.6 - j as f32 * 0.8).sin();
        dir = (Quat::from_rotation_y(sway * 0.5) * Quat::from_rotation_x(0.35) * dir).normalize();
        if j == 3 {
            out.push((
                t,
                p + dir * 0.04 * k,
                Quat::from_rotation_arc(Vec3::Y, dir),
                Vec3::ONE,
            ));
        } else {
            let q = p + dir * 0.11 * k;
            out.push(bone(t, p, q, Vec3::NEG_Y));
            p = q;
        }
    }
    out
}

/// A hound lives a step: it runs at its foe, pounces from a few metres off and bites.
pub fn live_hound(
    m: &mut Minion,
    tf: &mut Transform,
    life: &mut Life,
    parts: &mut Query<(&MinionPart, &mut Transform), Without<Minion>>,
) {
    let (e, dt) = (life.e, life.dt);
    let k = m.size;
    let fwd = Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z;
    let bite_reach = 1.15 * k + 0.25;
    let mut act = m.act;
    let mut air = 0.0;
    let mut jaw = 0.0;
    let mut lunge = 0.0;
    m.leap_in = (m.leap_in - dt).max(0.0);
    match &mut act {
        Act::Pounce { t, from, to } => {
            *t += dt;
            let u = (*t / POUNCE_SECS).min(1.0);
            let flat_at = from.lerp(*to, u as f64);
            m.pos = DVec3::new(flat_at.x, from.y + (to.y - from.y) * u as f64, flat_at.z);
            air = u;
            jaw = ramp(0.5, 0.85, u);
            let d = flat(*to - *from).as_vec3();
            if d.length() > 0.1 {
                m.yaw = yaw_of(d.normalize());
            }
            if u >= 1.0 {
                m.pos = ground_below(life.queries, m.pos + DVec3::Y * 0.6, life.exclude);
                if let Some((te, tp)) = life.target
                    && crate::summon::flat_distance(tp, m.pos) <= (bite_reach + 0.5) as f64
                {
                    bite(m, e, te, tp, fwd, life);
                }
                life.queue.looks.push(LookCmd::Burst {
                    at: m.pos,
                    up: Vec3::Y,
                    look: "stone".into(),
                    role: Role::Impact,
                    scale: 0.45 * k,
                    color: HELLFIRE,
                });
                act = Act::Ready;
                m.cooldown = 0.35;
                // Out of the gate it runs and springs at its foe at once; after a pounce at a
                // foe it waits for the next.
                m.leap_in = if m.attacks == 0 { 0.3 } else { POUNCE_EVERY };
            }
        }
        Act::Bite { t } => {
            let before = *t;
            *t += dt;
            jaw = ramp(0.0, 0.16, *t) * (1.0 - ramp(BITE_HIT - 0.04, BITE_HIT, *t));
            lunge = ramp(0.05, BITE_HIT, *t) * (1.0 - ramp(BITE_HIT + 0.05, BITE_SECS, *t));
            if before < BITE_HIT
                && *t >= BITE_HIT
                && let Some((te, tp)) = life.target
                && crate::summon::flat_distance(tp, m.pos) <= (bite_reach + 0.35) as f64
            {
                bite(m, e, te, tp, fwd, life);
            }
            if *t >= BITE_SECS {
                act = Act::Ready;
                m.cooldown = BITE_REST;
            }
        }
        _ => {}
    }
    let mut speed = 0.0;
    if act == Act::Ready && !life.held {
        let idle = Vec3::new(1.4 * m.flank, 0.0, 1.2);
        let goal = match (life.fleeing, life.target, life.owner) {
            (Some(from), ..) => {
                let away = flat(m.pos - from).normalize_or(DVec3::Z);
                Some((m.pos + away * 4.0, 0.0))
            }
            (None, Some((_, tp)), _) => Some((tp, bite_reach)),
            (None, None, Some((op, orot))) => Some((op + (orot * idle).as_dvec3(), 1.0)),
            _ => None,
        };
        if let Some((goal, stop)) = goal {
            let to = flat(goal - m.pos).as_vec3();
            let dist = to.length();
            if dist > 0.05 {
                let diff = wrap(yaw_of(to / dist) - m.yaw);
                m.yaw += diff.clamp(-HOUND_TURN * dt, HOUND_TURN * dt);
                if let Some((_, tp)) = life.target
                    && life.fleeing.is_none()
                {
                    if (POUNCE_MIN_M..=POUNCE_MAX_M).contains(&dist)
                        && diff.abs() < 0.3
                        && m.leap_in <= 0.0
                        && m.cooldown <= 0.0
                    {
                        // Land just short of the foe, jaws first.
                        let dir = flat(tp - m.pos).normalize_or(fwd.as_dvec3());
                        let to = tp - dir * (bite_reach * 0.8) as f64;
                        let to = ground_below(life.queries, to + DVec3::Y * 0.6, life.exclude);
                        m.attacks += 1;
                        act = Act::Pounce {
                            t: 0.0,
                            from: m.pos,
                            to,
                        };
                    } else if dist <= stop + 0.15 && diff.abs() < 0.5 && m.cooldown <= 0.0 {
                        m.attacks += 1;
                        act = Act::Bite { t: 0.0 };
                    }
                }
                if act == Act::Ready && dist > stop {
                    speed = HOUND_RUN
                        * k
                        * diff.cos().max(0.0)
                        * ((dist - stop) / 0.8).min(1.0)
                        * if life.fleeing.is_some() { 0.8 } else { 1.0 };
                }
            }
        }
    }
    let fwd = Quat::from_rotation_y(m.yaw) * Vec3::NEG_Z;
    let pouncing = matches!(act, Act::Pounce { .. });
    if !pouncing {
        m.vel = m.vel.lerp(fwd * speed, (6.0 * dt).min(1.0));
        // Keep out of other walkers' way.
        let mut shove = DVec3::ZERO;
        if !life.held {
            let r = crate::summon::body_radius(m.kind, k) as f64;
            for (o, op, or) in life.crowd {
                if *o == e {
                    continue;
                }
                let d = flat(m.pos - *op);
                let gap = r + *or as f64 - d.length();
                if gap > 0.0 {
                    shove += d.normalize_or(DVec3::X) * gap;
                }
            }
            // Nor into its foe's body: it bites from in front of it.
            if let Some((_, tp)) = life.target {
                let d = flat(m.pos - tp);
                let gap = r + 0.35 - d.length();
                if gap > 0.0 {
                    shove += d.normalize_or(-fwd.as_dvec3()) * gap;
                }
            }
        } else {
            m.vel = Vec3::ZERO;
        }
        let step = (m.vel * dt).as_dvec3() + shove * (6.0 * dt).min(1.0) as f64;
        if step.length_squared() > 1e-10 {
            m.pos = ground_below(life.queries, m.pos + step + DVec3::Y * 0.5, life.exclude);
        }
    }
    m.act = act;
    // The gait: a trot whose stride follows its speed.
    let run = m.vel.length() / (HOUND_RUN * k).max(1e-3);
    let hz = 1.3 + 1.6 * run;
    m.phase = (m.phase + dt * hz).fract();
    let lift = if pouncing {
        POUNCE_H * k * (PI * air).sin()
    } else {
        0.0
    };
    tf.translation = m.pos + DVec3::Y * lift as f64;
    tf.rotation = Quat::from_rotation_y(m.yaw);
    let stride = m.vel.length() / hz;
    let pose = pose_hound(
        k,
        m.phase,
        run,
        stride,
        if pouncing { Some(air) } else { None },
        jaw,
        lunge,
        m.age,
        life.held,
    );
    set_parts(m, &pose, parts);
}

/// A hound's bite lands.
fn bite(m: &Minion, e: Entity, te: Entity, tp: DVec3, fwd: Vec3, life: &mut Life) {
    let at = tp + DVec3::Y * 0.9;
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
        look: m.attack.weave.key.clone(),
        role: Role::Impact,
        scale: 0.45 * m.size,
        color: m.attack.weave.color,
    });
}

/// A hound's pose in its own frame (its feet on the floor at the origin, facing -Z): `phase`
/// 0..1 through a stride, `run` 0..1 of its fastest, `stride` m; `air` 0..1 through a pounce;
/// `jaw` open 0..1; `lunge` 0..1 of a bite's reach.
#[allow(clippy::too_many_arguments)]
pub fn pose_hound(
    k: f32,
    phase: f32,
    run: f32,
    stride: f32,
    air: Option<f32>,
    jaw: f32,
    lunge: f32,
    age: f32,
    held: bool,
) -> Vec<Pose> {
    let mut out = Vec::with_capacity(HOUND_PARTS);
    // The body pitches nose-down as it runs, rears up as it leaps and dives as it lands;
    // it bobs twice a stride; a held one crouches.
    let pitch = match air {
        Some(u) => 0.35 * (1.0 - u) - 0.3 * u,
        None => -0.08 * run,
    };
    let crouch = if held { 0.14 } else { 0.0 };
    let bob = if air.is_some() {
        0.0
    } else {
        0.035 * run * (phase * 2.0 * TAU).cos()
    };
    let body = Quat::from_rotation_x(pitch);
    let pivot = PIVOT * k + Vec3::Y * ((bob - crouch) * k);
    let at = |v: Vec3| pivot + body * ((v - PIVOT) * k);
    // Torso pieces lie along the spine (a rock's length is its Y).
    let along = body * Quat::from_rotation_x(-PI * 0.5);
    out.push((HOUND_CHEST, at(Vec3::new(0.0, 0.7, -0.3)), along, Vec3::ONE));
    out.push((
        HOUND_WAIST,
        at(Vec3::new(0.0, 0.66, 0.06)),
        along,
        Vec3::ONE,
    ));
    out.push((
        HOUND_HAUNCH,
        at(Vec3::new(0.0, 0.7, 0.38)),
        along,
        Vec3::ONE,
    ));
    // Neck up and forward to the head; the head lunges for a bite.
    let reach = Vec3::new(0.0, -0.05, -0.16) * lunge;
    let neck_a = at(Vec3::new(0.0, 0.8, -0.5));
    let head = at(Vec3::new(0.0, 1.0, -0.74) + reach);
    out.push(bone(HOUND_NECK, neck_a, head, Vec3::Y));
    let look = body * Quat::from_rotation_x(-0.15 - 0.2 * lunge);
    let skull = look * Quat::from_rotation_x(-PI * 0.5);
    out.push((HOUND_HEAD, head, skull, Vec3::ONE));
    out.push((
        HOUND_MUZZLE,
        head + look * (Vec3::new(0.0, -0.03, -0.24) * k),
        skull,
        Vec3::ONE,
    ));
    let hinge = head + look * (Vec3::new(0.0, -0.08, -0.08) * k);
    let jr = look * Quat::from_rotation_x(-0.75 * jaw.clamp(0.0, 1.0));
    out.push((
        HOUND_JAW,
        hinge + jr * (Vec3::new(0.0, -0.02, -0.13) * k),
        jr * Quat::from_rotation_x(-PI * 0.5),
        Vec3::ONE,
    ));
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        out.push((
            HOUND_EYE[i],
            head + look * (Vec3::new(0.1 * s, 0.07, -0.18) * k),
            look,
            Vec3::ONE,
        ));
        let horn = look * Vec3::new(0.3 * s, 0.5, 0.8).normalize();
        out.push((
            HOUND_HORN[i],
            head + look * (Vec3::new(0.1 * s, 0.12, 0.02) * k) + horn * 0.14 * k,
            Quat::from_rotation_arc(Vec3::Y, horn),
            Vec3::ONE,
        ));
    }
    // A crest of spikes down its back, swept back.
    for (j, &t) in HOUND_SPINE.iter().enumerate() {
        let (z, y) = [(-0.42, 0.99), (-0.18, 0.97), (0.08, 0.9), (0.32, 0.95)][j];
        let len = [0.24, 0.2, 0.15, 0.12][j];
        let dir = body * Vec3::new(0.0, 1.0, 0.55).normalize();
        out.push((
            t,
            at(Vec3::new(0.0, y, z)) + dir * len * 0.5 * k,
            Quat::from_rotation_arc(Vec3::Y, dir),
            Vec3::ONE,
        ));
    }
    // Legs: a trot moves diagonal pairs together (fore left with hind right). Each paw is
    // planted for 60% of a stride and swings forward, lifted, for the rest.
    let duty = 0.6;
    let foot = |offset: f32, mount: Vec3, fore: bool| -> Vec3 {
        let rest = Vec3::new(mount.x, PAW_Y, mount.z + if fore { -0.04 } else { 0.06 }) * k;
        if let Some(u) = air {
            // Fore paws reach ahead, hind ones trail, as it flies; all come down to land.
            let tuck = (PI * u).sin();
            let (dz, dy) = if fore { (-0.45, 0.35) } else { (0.45, 0.25) };
            return rest + Vec3::new(0.0, dy * tuck, dz * tuck) * k;
        }
        if held {
            return rest + Vec3::new(mount.x.signum() * 0.08, 0.0, 0.0) * k;
        }
        let u = (phase + offset).fract();
        let half = 0.5 * stride * duty;
        if u < duty {
            rest + Vec3::new(0.0, 0.0, -half + 2.0 * half * (u / duty))
        } else {
            let w = (u - duty) / (1.0 - duty);
            rest + Vec3::new(
                0.0,
                (0.08 + 0.12 * run) * k * (PI * w).sin(),
                half - 2.0 * half * smooth(w),
            )
        }
    };
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        // Fore legs: shoulder to paw, the elbow behind.
        let mount = Vec3::new(SHOULDER.x * s, SHOULDER.y, SHOULDER.z);
        let paw = foot(if i == 0 { 0.0 } else { 0.5 }, mount, true);
        let sh = at(mount);
        let ankle = paw + Vec3::Y * PAW_Y * k;
        let (elbow, wrist) = two_bone(sh, ankle, FORE_UPPER * k, FORE_LOWER * k, Vec3::Z);
        out.push(bone(HOUND_FORE[i], sh, elbow, Vec3::NEG_Y));
        out.push(bone(HOUND_FORE[i] + 1, elbow, wrist, Vec3::NEG_Y));
        out.push((
            HOUND_FORE[i] + 2,
            paw,
            Quat::from_rotation_y(0.0),
            Vec3::ONE,
        ));
        // Hind legs: hip to stifle (forward) to hock (back) to paw.
        let mount = Vec3::new(HIP.x * s, HIP.y, HIP.z);
        let paw = foot(if i == 0 { 0.5 } else { 0.0 }, mount, false);
        let hip = at(mount);
        let ankle = paw + Vec3::Y * PAW_Y * k;
        let hock =
            ankle + Vec3::new(0.0, HIND_HOCK * 0.92, HIND_HOCK * 0.38).normalize() * HIND_HOCK * k;
        let (stifle, hock) = two_bone(hip, hock, HIND_THIGH * k, HIND_SHIN * k, Vec3::NEG_Z);
        out.push(bone(HOUND_HIND[i], hip, stifle, Vec3::NEG_Y));
        out.push(bone(HOUND_HIND[i] + 1, stifle, hock, Vec3::NEG_Y));
        out.push(bone(HOUND_HIND[i] + 2, hock, ankle, Vec3::NEG_Y));
        out.push((HOUND_HIND[i] + 3, paw, Quat::IDENTITY, Vec3::ONE));
    }
    // The tail streams out behind as it runs, droops and sways as it stands.
    let mut p = at(Vec3::new(0.0, 0.76, 0.58));
    let mut dir = body * Vec3::new(0.0, 0.25 - 0.75 * (1.0 - run), 1.0).normalize();
    for (j, &t) in HOUND_TAIL.iter().enumerate() {
        let sway = 0.35 * (age * 3.4 - j as f32 * 0.9).sin() * (1.0 - 0.6 * run);
        dir = (Quat::from_rotation_y(sway) * dir).normalize();
        let q = p + dir * 0.2 * k;
        out.push(bone(t, p, q, Vec3::Z));
        p = q;
    }
    out
}

/// The pact ends: every open gate closes and every demon falls in fire (the hall's period).
pub fn dismiss(world: &mut World) {
    let mut q = world.query::<&mut Hellgate>();
    for mut g in q.iter_mut(world) {
        g.closing.get_or_insert(0.0);
    }
    let mut q = world.query::<&mut Minion>();
    for mut m in q.iter_mut(world) {
        if m.kind.demon() {
            m.life = m.life.min(m.age);
        }
    }
}

/// Every hellgate closes at once (the hall's restart); the demons are minions and go with them.
pub fn clear(world: &mut World) {
    let gone: Vec<Entity> = world
        .query_filtered::<Entity, With<Hellgate>>()
        .iter(world)
        .collect();
    for e in gone {
        if let Ok(ec) = world.get_entity_mut(e) {
            ec.despawn();
        }
    }
}

pub fn register(app: &mut App) {
    app.init_resource::<DemonMeshes>().register_api(
        "magic.demons",
        "A pact's demons now: open hellgates (where, seconds left, health, hounds and imps \
         sent) and the imps and hellhounds (whose, where, what they do, health).",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Hellgate, &Vitality)>();
            let gates: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, g, v)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "caster": g.cast.caster.map(|c| c.to_bits()),
                        "at": [g.at.x, g.at.y, g.at.z],
                        "left_s": g.left(),
                        "open": g.open(),
                        "health": v.health,
                        "hounds": g.hounds,
                        "imps": g.imps,
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, &Minion, Option<&Vitality>)>();
            let demons: Vec<serde_json::Value> = q
                .iter(world)
                .filter(|(_, m, _)| matches!(m.kind, MinionKind::Imp | MinionKind::Hellhound))
                .map(|(e, m, v)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "kind": format!("{:?}", m.kind),
                        "caster": m.cast.caster.map(|c| c.to_bits()),
                        "at": [m.pos.x, m.pos.y, m.pos.z],
                        "act": format!("{:?}", m.act),
                        "health": v.map(|v| v.health),
                        "target": m.target.map(|t| t.to_bits()),
                    })
                })
                .collect();
            Ok(serde_json::json!({ "gates": gates, "demons": demons }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trotting_hound_moves_diagonal_pairs_together() {
        // Half a stride in, the fore left and hind right paws are lifted together, the other
        // pair planted.
        let pose = pose_hound(1.0, 0.8, 1.0, 1.0, None, 0.0, 0.0, 0.0, false);
        let paw = |i: usize| pose.iter().find(|p| p.0 == i).unwrap().1;
        let fl = paw(HOUND_FORE[0] + 2);
        let hr = paw(HOUND_HIND[1] + 3);
        let fr = paw(HOUND_FORE[1] + 2);
        let hl = paw(HOUND_HIND[0] + 3);
        assert!(fl.y > PAW_Y + 0.05 && hr.y > PAW_Y + 0.05, "{fl} {hr}");
        assert!(
            (fr.y - PAW_Y).abs() < 1e-4 && (hl.y - PAW_Y).abs() < 1e-4,
            "{fr} {hl}"
        );
    }

    #[test]
    fn a_hounds_legs_reach_its_paws() {
        for phase in [0.0, 0.3, 0.7] {
            let pose = pose_hound(1.2, phase, 0.6, 0.8, None, 0.0, 0.0, 0.0, false);
            let get = |i: usize| pose.iter().find(|p| p.0 == i).unwrap();
            for fore in HOUND_FORE {
                // The lower fore leg's far end is at the paw's ankle.
                let (_, mid, rot, _) = *get(fore + 1);
                let end = mid + rot * Vec3::Y * FORE_LOWER * 1.2 * 0.5;
                let paw = get(fore + 2).1;
                assert!(
                    end.distance(paw + Vec3::Y * PAW_Y * 1.2) < 0.03,
                    "{end} {paw}"
                );
            }
        }
    }

    #[test]
    fn an_imp_winds_its_throwing_arm_back_behind_its_head() {
        let rest = pose_imp(1.0, 0.0, 0.0, 0.0, 0.0, 0.0, false);
        let wound = pose_imp(1.0, 0.0, 0.0, 1.0, 0.0, 0.0, false);
        let fire = |p: &[Pose]| *p.iter().find(|q| q.0 == IMP_FIRE).unwrap();
        assert!(fire(&wound).1.y > fire(&rest).1.y + 0.15);
        // Its fire shows only while it winds up.
        assert!(fire(&rest).3.x < 0.01 && fire(&wound).3.x > 0.9);
    }

    #[test]
    fn imp_wings_beat_up_and_down() {
        let up = pose_imp(1.0, 0.25, 0.0, 0.0, 0.0, 0.0, false);
        let down = pose_imp(1.0, 0.75, 0.0, 0.0, 0.0, 0.0, false);
        let tip = |p: &[Pose]| {
            let w = p.iter().find(|q| q.0 == IMP_WING[1]).unwrap();
            w.1 + w.2 * (Vec3::new(0.52, 0.12, 0.03) * w.3)
        };
        assert!(
            tip(&up).y > tip(&down).y + 0.3,
            "{} {}",
            tip(&up),
            tip(&down)
        );
    }

    #[test]
    fn a_wing_is_two_sided() {
        let m = wing_mesh();
        assert_eq!(m.indices.len() % 6, 0);
        assert!(m.vertices.iter().any(|v| v.normal[1] > 0.5));
        assert!(m.vertices.iter().any(|v| v.normal[1] < -0.5));
    }
}
