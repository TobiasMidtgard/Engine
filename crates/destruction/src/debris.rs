//! Debris: chunks, chips and dust thrown off by destruction.
//!
//! Heavy chunks become physics-contract bodies (`RigidBody` + `Collider` + `Velocity`) when a
//! physics backend is installed, so they stack and collide with everything (up to a budget, see
//! `DestructionSettings::max_physics_debris`). Without one, and always for light chunks and
//! chips, a built-in lightweight solver tumbles them onto the ground (the
//! `Ground` query), lets them settle and sleep, and piles them up: chunks at rest are boxes
//! that later debris lands on, so a collapse leaves a heap rather than pieces sunk into each
//! other. Everything sinks out of sight after its lifetime and a budget caps how much debris
//! exists at once.

use std::collections::{HashMap, HashSet};

use bevy_tasks::{Task, block_on, poll_once};

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    Billboard, MaterialRef, Mesh, MeshRef, MeshRenderer, Sprite, SpriteAlpha, SpriteAtlas,
};
use engine_world::physics::{
    BodyKind, Collider, ColliderShape, PhysicsBackend, RigidBody, Velocity,
};
use engine_world::{
    ContactKind, Env, GroundContact, GroundSample, NoiseEvent, SimulationRange, SurfaceMaterials,
};

use crate::assets::DestructionAssets;
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::textures::DUST_FRAMES;
use crate::voxel::{VoxelState, in_rock, rock_contact, rock_depth, rock_side};
use crate::wall::WallState;
use crate::{DebrisMode, DestructionSettings, DestructionStats};

/// A piece of debris. Registered so tools and AI agents can see and tune it.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Debris {
    pub material: DestructibleMaterial,
    pub mass_kg: f32,
    /// Seconds since it broke off.
    pub age: f32,
    /// Seconds before it sinks out of sight and is removed.
    pub lifetime: f32,
    /// A small chip (cheap, short-lived) rather than a chunk.
    pub chip: bool,
    /// Hardest landing so far, m/s.
    pub max_impact_speed: f32,
}

impl Default for Debris {
    fn default() -> Self {
        Self {
            material: DestructibleMaterial::Concrete,
            mass_kg: 1.0,
            age: 0.0,
            lifetime: 30.0,
            chip: false,
            max_impact_speed: 0.0,
        }
    }
}

/// Meshes owned by one piece of debris (freed when it despawns): its own, and its fracture
/// faces' when a child draws them in another material.
#[derive(Component, Debug, Clone, Copy)]
pub struct DebrisMesh(pub Handle<Mesh>, pub Option<Handle<Mesh>>);

impl DebrisMesh {
    pub fn free(&self, meshes: &mut Assets<Mesh>) {
        meshes.remove(self.0);
        if let Some(h) = self.1 {
            meshes.remove(h);
        }
    }
}

/// State of a body moved by the built-in debris solver.
#[derive(Component, Debug, Clone)]
#[require(DebrisPose)]
pub struct DebrisBody {
    pub velocity: Vec3,
    pub angular: Vec3,
    pub inv_mass: f32,
    /// Inverse principal moments of inertia (local axes).
    pub inv_inertia: Vec3,
    /// Contact points in local space.
    pub points: Vec<Vec3>,
    pub friction: f32,
    pub restitution: f32,
    pub asleep: bool,
    /// Seconds spent nearly still.
    pub still: f32,
    /// Lifetime over: sinking into the ground, no more collisions.
    pub sinking: bool,
    /// Normal speed of the most recent landing, m/s (read by impact fracture).
    pub last_impact: f32,
    /// Whether it lands on voxel rock.
    pub rock: RockContact,
    /// The resting chunk it lies on, if any: it falls again when that one goes.
    pub on: Option<Entity>,
    /// The last ground sample under it and where it was taken: reused, along the ground's
    /// tangent plane, until it moves off that spot.
    pub ground: Option<(DVec3, GroundSample)>,
    /// Seconds in contact since it last woke: one that keeps rocking long after landing is put
    /// to sleep anyway.
    pub awake_secs: f32,
}

/// Where the built-in solver holds a [`DebrisBody`]: its own copy of the pose, stepped without
/// touching `Transform` (which dozens of other modules' systems read or write, so a step that
/// borrowed it would run alone), then copied into `Transform` by a small system after the step,
/// as Avian keeps `Position` and `Rotation` and writes `Transform` back in its sync set.
#[derive(Component, Debug, Clone, Default)]
pub struct DebrisPose {
    /// The pose the solver steps.
    pub now: Transform,
    /// The pose it last wrote into `Transform`: a `Transform` that differs was moved by
    /// something else (a world shift, a reset, a test), and the solver carries on from there.
    written: Option<Transform>,
}

/// Landings found by the debris step, written out as messages after it (see [`DebrisPose`]).
#[derive(Resource, Default)]
pub struct DebrisLandings {
    contacts: Vec<GroundContact>,
    noises: Vec<NoiseEvent>,
}

/// Take in debris `Transform`s moved by something other than the solver (before its step).
#[allow(clippy::type_complexity)]
pub fn pose_debris_system(
    mut q: Query<
        (&Transform, &mut DebrisPose),
        (
            With<DebrisBody>,
            Or<(Changed<Transform>, Added<DebrisPose>)>,
        ),
    >,
) {
    for (t, mut pose) in &mut q {
        if pose.written != Some(*t) {
            // Taken in quietly: nothing moved that the step has to write back.
            let pose = pose.bypass_change_detection();
            pose.now = *t;
            pose.written = Some(*t);
        }
    }
}

/// Copy the poses the debris step moved into `Transform`, and send its landings.
pub fn place_debris_system(
    mut q: Query<(&mut Transform, &mut DebrisPose), Changed<DebrisPose>>,
    mut landings: ResMut<DebrisLandings>,
    mut contacts: MessageWriter<GroundContact>,
    mut noise: MessageWriter<NoiseEvent>,
) {
    for (mut t, mut pose) in &mut q {
        let pose = pose.bypass_change_detection();
        *t = pose.now;
        pose.written = Some(pose.now);
    }
    if !landings.contacts.is_empty() {
        contacts.write_batch(std::mem::take(&mut landings.contacts));
    }
    if !landings.noises.is_empty() {
        noise.write_batch(std::mem::take(&mut landings.noises));
    }
}

impl DebrisBody {
    /// Area of its broadest side (m²), from the box around its contact points.
    pub fn broad_area(&self) -> f32 {
        let (lo, hi) = self.points.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        if lo.x > hi.x {
            return 0.0;
        }
        let e = hi - lo;
        let (big, small) = (e.max_element(), e.min_element());
        big * (e.x + e.y + e.z - big - small)
    }
}

/// Whether debris lands on voxel rock as on the ground.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RockContact {
    /// Not looked at yet: decided on its first step.
    #[default]
    Unknown,
    /// Lands on rock.
    Lands,
    /// Started out buried in rock deeper than [`BURIED_M`]: lands on it once it has fallen
    /// clear.
    Buried,
    /// Cut from the rock at this point (a cave-in's boulders, a blast's rubble): passes through
    /// rock until it has moved [`LEAVE_M`] from there and is clear of it, so it falls out of
    /// the hole it came from instead of hanging on the hole's rims; then lands on rock (a
    /// ledge, a cave's floor) like anything else.
    Leaving(DVec3),
}

/// How deep debris may start out inside voxel rock and still land on it (a building piece
/// resting on rock sits up to the ground tolerance in it), m.
pub const BURIED_M: f32 = 0.15;
/// How far rock pushes a point inside it out: up to a floor's top, out of a wall's face, down
/// out of a ceiling; deeper, it passes through, m.
const ROCK_REACH_M: f32 = 0.6;
/// How far debris cut from rock moves before it may land on rock, m (see
/// [`RockContact::Leaving`]).
pub const LEAVE_M: f32 = 0.5;
/// Spacing of the points checked for rock before debris cut from rock may land, m.
const LEAVE_LATTICE_M: f32 = 0.2;

impl DebrisBody {
    /// Box-shaped body of `mass_kg` with half extents `half`, touching the ground at `points`.
    pub fn new(mass_kg: f32, half: Vec3, points: Vec<Vec3>, friction: f32) -> Self {
        let m = mass_kg.max(1e-3);
        let h = half.max(Vec3::splat(0.005));
        let inertia = Vec3::new(
            h.y * h.y + h.z * h.z,
            h.x * h.x + h.z * h.z,
            h.x * h.x + h.y * h.y,
        ) * (m / 3.0);
        Self {
            velocity: Vec3::ZERO,
            angular: Vec3::ZERO,
            inv_mass: 1.0 / m,
            inv_inertia: inertia.recip(),
            points,
            friction,
            restitution: 0.15,
            asleep: false,
            still: 0.0,
            sinking: false,
            last_impact: 0.0,
            rock: RockContact::Unknown,
            on: None,
            ground: None,
            awake_secs: 0.0,
        }
    }

    /// Push the body (wakes it).
    pub fn kick(&mut self, impulse: Vec3, angular: Vec3) {
        self.velocity += impulse * self.inv_mass;
        self.angular += angular;
        self.asleep = false;
        self.still = 0.0;
        self.awake_secs = 0.0;
    }
}

/// Glowing hot fragments (sparks): fade instead of sinking.
#[derive(Component, Debug, Clone, Copy)]
pub struct Spark;

/// A dust sprite's quad, as a multiple of its puff's size: the soft cloud fades out well
/// inside its quad, so the quad is larger than the mesh puff it replaces.
const DUST_SPRITE_SCALE: f32 = 2.2;

/// A dust sprite's opacity at its thickest.
const DUST_SPRITE_ALPHA: f32 = 0.8;

/// How much wider a dust puff ends than it starts.
const DUST_SPREAD: f32 = 6.0;
/// How fast a spreading puff thins: opacity goes as (start size / size)^this.
const DUST_THINNING: f32 = 0.7;

/// A puff of dust that grows, rises a little, drifts with the wind and fades.
#[derive(Component, Debug, Clone, Copy)]
pub struct DustPuff {
    pub age: f32,
    pub life: f32,
    pub start_size: f32,
    pub end_size: f32,
    pub velocity: Vec3,
    pub darkness: f32,
    pub opacity: f32,
}

/// What a piece of debris looks like.
#[derive(Debug, Clone)]
pub enum DebrisLook {
    /// Its own mesh (vertex colored) drawn with the material's surface material.
    Mesh(Mesh),
    /// A broken piece: its outside (if any) in the surface material, its fracture faces in
    /// the material's broken-face material (see `fracture::ConvexPoly::broken_meshes`).
    Broken { outside: Option<Mesh>, inside: Mesh },
    /// A shared chip mesh scaled to `size` meters.
    Chip { size: f32, variant: usize },
    /// A glowing spark of `size` meters.
    Spark { size: f32 },
    /// Keep the look of the entity being turned into debris (see [`DebrisSpawn::reuse`]).
    Keep,
}

/// A request to spawn debris (filled by walls, voxels, fracture; spawned once per step).
#[derive(Debug, Clone)]
pub struct DebrisSpawn {
    pub position: DVec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub angular: Vec3,
    pub look: DebrisLook,
    /// Half extents of the local bounds (inertia, contact points).
    pub half_extents: Vec3,
    /// Hull for physics colliders and built-in contacts (local; empty = box corners).
    pub hull: Vec<Vec3>,
    pub mass_kg: f32,
    pub material: DestructibleMaterial,
    /// Shatter into smaller pieces when it lands harder than this, m/s (0 = never).
    pub shatter_speed: f32,
    /// Seconds before it sinks away (`None` = the settings' chunk or chip lifetime).
    pub lifetime: Option<f32>,
    /// Turn this existing entity (a building piece that lost support) into the debris instead
    /// of spawning a new one.
    pub reuse: Option<Entity>,
    /// Scale of a reused entity (its hull is in unscaled local units).
    pub scale: Option<Vec3>,
    /// Spawn already lying still on the ground (see [`settle`]).
    pub resting: bool,
    /// Cut from voxel rock (a cave-in's boulders, a blast's rubble): it falls out of its hole
    /// through the rock around it (see [`RockContact::Leaving`]).
    pub from_rock: bool,
}

impl Default for DebrisSpawn {
    fn default() -> Self {
        Self {
            position: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            velocity: Vec3::ZERO,
            angular: Vec3::ZERO,
            look: DebrisLook::Chip {
                size: 0.05,
                variant: 0,
            },
            half_extents: Vec3::splat(0.025),
            hull: Vec::new(),
            mass_kg: 0.1,
            material: DestructibleMaterial::Concrete,
            shatter_speed: 0.0,
            lifetime: None,
            reuse: None,
            scale: None,
            resting: false,
            from_rock: false,
        }
    }
}

/// Move a debris request to where it would come to rest: thrown a little along its velocity,
/// lying on its flattest side on the ground, motionless. Used to build ready-made aftermaths
/// (a showcase that should look destroyed from its first frame).
pub fn settle(
    item: &mut DebrisSpawn,
    ground: impl Fn(DVec3) -> f64,
    rng: &mut engine_core::math::Rng,
) {
    let h = item.half_extents.max(Vec3::splat(1e-3));
    let scale = item.scale.unwrap_or(Vec3::ONE);
    let pts: Vec<Vec3> = if item.hull.is_empty() {
        box_corners(h)
    } else {
        item.hull.iter().map(|p| *p * scale).collect()
    };
    let thin = if h.x <= h.y && h.x <= h.z {
        Vec3::X
    } else if h.y <= h.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let side = if rng.chance(0.5) { thin } else { -thin };
    let tilt = Quat::from_euler(
        engine_core::math::EulerRot::XYZ,
        rng.range(-0.08, 0.08),
        0.0,
        rng.range(-0.08, 0.08),
    );
    let rotation = Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU))
        * tilt
        * Quat::from_rotation_arc(side, Vec3::Y);
    let v = Vec3::new(item.velocity.x, 0.0, item.velocity.z);
    let reach = if matches!(item.look, DebrisLook::Chip { .. }) {
        2.5
    } else {
        4.0
    };
    let travel = (v * rng.range(0.12, 0.3)).clamp_length_max(reach);
    let mut p = item.position + travel.as_dvec3();
    let low = pts
        .iter()
        .map(|q| (rotation * *q).y)
        .fold(f32::MAX, f32::min);
    p.y = ground(p) - low as f64 - 0.005;
    item.position = p;
    item.rotation = rotation;
    item.velocity = Vec3::ZERO;
    item.angular = Vec3::ZERO;
    item.resting = true;
}

/// Debris waiting to be spawned this step.
#[derive(Resource, Debug, Default)]
pub struct PendingDebris {
    pub items: Vec<DebrisSpawn>,
    pub dust: Vec<DustRequest>,
    /// Debris being shattered on the bake threads (see [`crate::hits::Ctx::later`]), spawned
    /// once made: a frame or so after the hit.
    pub later: DebrisJobs,
}

/// Debris pieces being made on the bake threads.
#[derive(Default)]
pub struct DebrisJobs(pub Vec<Task<Vec<DebrisSpawn>>>);

impl std::fmt::Debug for DebrisJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DebrisJobs({} pending)", self.0.len())
    }
}

impl PendingDebris {
    /// Move the debris made so far into `items`.
    pub fn collect_later(&mut self) {
        let mut i = 0;
        while i < self.later.0.len() {
            match block_on(poll_once(&mut self.later.0[i])) {
                Some(items) => {
                    // Finished: dropping it cancels nothing.
                    drop(self.later.0.swap_remove(i));
                    self.items.extend(items);
                }
                None => i += 1,
            }
        }
    }

    /// Wait for all the debris being made and move it into `items` (before a world shift,
    /// or anything that changes the pending pieces as a whole).
    pub fn finish_later(&mut self) {
        for task in self.later.0.drain(..) {
            self.items.extend(block_on(task));
        }
    }
}

/// A dust puff request.
#[derive(Debug, Clone, Copy)]
pub struct DustRequest {
    pub position: DVec3,
    pub velocity: Vec3,
    pub size: f32,
    pub darkness: f32,
    pub count: u32,
}

/// Debris that shatters on a hard landing (pieces come from its box and material).
#[derive(Component, Debug, Clone, Copy)]
pub struct ShatterOnImpact {
    pub speed: f32,
    pub half_extents: Vec3,
}

fn box_corners(h: Vec3) -> Vec<Vec3> {
    let mut v = Vec::with_capacity(8);
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                v.push(Vec3::new(x, y, z) * h);
            }
        }
    }
    v
}

/// Spawn everything in [`PendingDebris`].
#[allow(clippy::too_many_arguments)]
pub fn spawn_debris_system(
    mut commands: Commands,
    mut pending: ResMut<PendingDebris>,
    assets: Option<Res<DestructionAssets>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    settings: Res<DestructionSettings>,
    backend: Option<Res<PhysicsBackend>>,
    table: Res<DestructionMaterials>,
    surfaces: Res<SurfaceMaterials>,
    mut stats: ResMut<DestructionStats>,
    physics_debris: Query<(), (With<Debris>, With<RigidBody>)>,
) {
    if !pending.later.0.is_empty() {
        pending.collect_later();
    }
    if pending.items.is_empty() && pending.dust.is_empty() {
        return;
    }
    let (Some(assets), Some(mut meshes)) = (assets, meshes) else {
        pending.items.clear();
        pending.dust.clear();
        return;
    };
    let use_physics = match settings.debris_mode {
        DebrisMode::Builtin => false,
        DebrisMode::Physics => true,
        DebrisMode::Auto => backend.is_some(),
    };
    // Which chunks get physics bodies. A collapse breaks off hundreds at once, and each awake
    // body costs the physics step: like Chaos's size-specific data and Frostbite's debris
    // particles, light fragments go to the built-in solver (they land on the ground, rock,
    // floors and resting rubble), and past the body budget only the heaviest get bodies. A
    // building piece that gives way stays solid whatever it weighs (budget allowing).
    let mut bodies = vec![false; pending.items.len()];
    if use_physics {
        let mut wanted: Vec<usize> = (0..pending.items.len())
            .filter(|&k| {
                let i = &pending.items[k];
                !matches!(i.look, DebrisLook::Chip { .. } | DebrisLook::Spark { .. })
                    && (i.mass_kg >= settings.physics_min_kg || i.reuse.is_some())
            })
            .collect();
        let room = settings
            .max_physics_debris
            .saturating_sub(physics_debris.iter().count());
        if wanted.len() > room {
            let items = &pending.items;
            wanted.sort_by(|a, b| items[*b].mass_kg.total_cmp(&items[*a].mass_kg));
            wanted.truncate(room);
        }
        for k in wanted {
            bodies[k] = true;
        }
    }
    for (item, solid) in pending.items.drain(..).zip(bodies) {
        let props = table.get(item.material);
        let surface = surfaces.id_or_default(&props.surface);
        let friction = surfaces.get(surface).friction;
        let mut inner = None;
        let (renderer, chip, owned, scale) = match &item.look {
            DebrisLook::Mesh(mesh) => {
                let h = meshes.add(mesh.clone());
                (
                    Some(MeshRenderer::new(
                        MeshRef::Handle(h),
                        MaterialRef::Handle(assets.surface(item.material)),
                    )),
                    false,
                    Some(h),
                    Vec3::ONE,
                )
            }
            DebrisLook::Broken { outside, inside } => {
                let hi = meshes.add(inside.clone());
                let broken = MeshRenderer::new(
                    MeshRef::Handle(hi),
                    MaterialRef::Handle(assets.broken(item.material)),
                );
                match outside {
                    Some(o) => {
                        let ho = meshes.add(o.clone());
                        inner = Some((hi, broken));
                        (
                            Some(MeshRenderer::new(
                                MeshRef::Handle(ho),
                                MaterialRef::Handle(assets.surface(item.material)),
                            )),
                            false,
                            Some(ho),
                            Vec3::ONE,
                        )
                    }
                    None => (Some(broken), false, Some(hi), Vec3::ONE),
                }
            }
            DebrisLook::Chip { size, variant } => (
                Some(MeshRenderer::new(
                    MeshRef::Handle(assets.chips[variant % assets.chips.len()]),
                    MaterialRef::Handle(assets.chip_material(item.material)),
                )),
                true,
                None,
                Vec3::splat(*size),
            ),
            DebrisLook::Spark { size } => (
                Some(
                    MeshRenderer::new(
                        MeshRef::Handle(assets.chips[0]),
                        MaterialRef::Handle(assets.spark),
                    )
                    .without_shadows(),
                ),
                true,
                None,
                Vec3::splat(*size),
            ),
            DebrisLook::Keep => (None, false, None, Vec3::ONE),
        };
        let lifetime = item.lifetime.unwrap_or(if chip {
            settings.chip_lifetime_secs
        } else {
            settings.debris_lifetime_secs
        });
        let transform = Transform {
            translation: item.position,
            rotation: item.rotation,
            scale,
        };
        let debris = Debris {
            material: item.material,
            mass_kg: item.mass_kg,
            age: 0.0,
            lifetime,
            chip,
            max_impact_speed: 0.0,
        };
        let hull = if item.hull.is_empty() {
            box_corners(item.half_extents)
        } else {
            item.hull.clone()
        };
        let mut e = match item.reuse {
            Some(existing) => {
                let Ok(mut e) = commands.get_entity(existing) else {
                    continue;
                };
                // Keep its look and scale; it is a root entity in world space from now on.
                e.insert((
                    Transform {
                        scale: item.scale.unwrap_or(Vec3::ONE),
                        ..transform
                    },
                    debris,
                    TransformInterpolation::default(),
                ));
                e
            }
            None => commands.spawn((transform, debris, TransformInterpolation::default())),
        };
        if let Some(r) = renderer {
            e.insert(r);
        }
        if let Some(h) = owned {
            e.insert(DebrisMesh(h, inner.as_ref().map(|(hi, _)| *hi)));
        }
        if let Some((_, r)) = inner {
            let parent = e.id();
            e.commands()
                .spawn((Transform::IDENTITY, r, ChildOf(parent)));
        }
        if matches!(item.look, DebrisLook::Spark { .. }) {
            e.insert(Spark);
        }
        if item.shatter_speed > 0.0 {
            e.insert(ShatterOnImpact {
                speed: item.shatter_speed,
                half_extents: item.half_extents,
            });
        }
        if !chip && solid {
            e.insert((
                RigidBody {
                    kind: BodyKind::Dynamic,
                    mass_kg: Some(item.mass_kg),
                    linear_damping: 0.02,
                    angular_damping: 0.1,
                    ..RigidBody::default()
                },
                Collider {
                    shape: ColliderShape::ConvexHull { points: hull },
                    surface,
                    density: props.density,
                    ..Collider::default()
                },
                Velocity {
                    linear: item.velocity,
                    angular: item.angular,
                },
            ));
        } else {
            // Chips touch the ground at a few corners; chunks at their hull's extreme points.
            let points = if chip {
                vec![
                    Vec3::new(0.5, -0.4, 0.3),
                    Vec3::new(-0.5, -0.4, 0.3),
                    Vec3::new(0.0, -0.4, -0.5),
                    Vec3::new(0.0, 0.5, 0.0),
                ]
            } else {
                reduce_points(&hull, 12)
            };
            let half = if chip {
                Vec3::splat(0.5)
            } else {
                item.half_extents
            };
            let mut body = DebrisBody::new(item.mass_kg, half, points, friction);
            body.velocity = item.velocity;
            body.angular = item.angular;
            body.asleep = item.resting;
            if item.from_rock {
                body.rock = RockContact::Leaving(item.position);
            }
            // A piece that stood with a collider loses it: this solver moves it now.
            e.insert(body).remove::<Collider>();
        }
        if chip {
            stats.chips_spawned += 1;
        } else {
            stats.chunks_spawned += 1;
        }
    }
    let mut rng_state = stats.chunks_spawned as u32 ^ 0x9e37_79b9;
    let mut next = move || {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 17;
        rng_state ^= rng_state << 5;
        (rng_state as f32 / u32::MAX as f32) - 0.5
    };
    if settings.dust {
        for d in pending.dust.drain(..) {
            // Big clouds are the smoke module's job (SmokeEmission); puffs stay small and many.
            let spread = d.size;
            let count = (d.count as f32 * (d.size / 0.8).max(1.0)).round() as u32;
            for _ in 0..count.clamp(1, 14) {
                let offset = Vec3::new(next(), next() * 0.5 + 0.25, next()) * spread;
                let size = d.size.min(0.8) * (0.6 + 0.4 * (next() + 0.5));
                let transform = Transform::from_translation(d.position + offset.as_dvec3())
                    .with_scale(Vec3::splat(size * 0.4));
                let mut puff = commands.spawn(transform);
                match assets.dust_sprite {
                    // A soft camera-facing cloud, lit by the sun and the sky.
                    Some(image) => {
                        let c = DestructionAssets::dust_color(d.darkness);
                        let frame = ((next() + 0.5) * DUST_FRAMES as f32) as u32;
                        puff.insert(
                            Sprite::new(image, [DUST_SPRITE_SCALE; 2])
                                .with_atlas(SpriteAtlas::grid(DUST_FRAMES, 1))
                                .with_frame(frame.min(DUST_FRAMES - 1))
                                .with_billboard(Billboard::FaceCamera)
                                .with_alpha(SpriteAlpha::Blend)
                                .with_color([c[0], c[1], c[2], 0.0])
                                .lit()
                                .with_roundness(0.8)
                                .with_translucency(0.5),
                        );
                    }
                    None => {
                        puff.insert(
                            MeshRenderer::new(
                                MeshRef::Handle(assets.dust_mesh),
                                MaterialRef::Handle(assets.dust(d.darkness, 1.0)),
                            )
                            .without_shadows(),
                        );
                    }
                }
                puff.insert(DustPuff {
                    age: 0.0,
                    life: 2.5 + 1.5 * (next() + 0.5),
                    start_size: size * 0.4,
                    end_size: size * 0.4 * DUST_SPREAD,
                    velocity: d.velocity + Vec3::new(next(), 0.3 + next() * 0.2, next()) * 0.8,
                    darkness: d.darkness,
                    opacity: 1.0,
                });
            }
        }
    } else {
        pending.dust.clear();
    }
}

/// At most `n` points of `hull`: the extremes along a spread of directions.
pub fn reduce_points(hull: &[Vec3], n: usize) -> Vec<Vec3> {
    if hull.len() <= n {
        return hull.to_vec();
    }
    let dirs = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(-1.0, -1.0, 1.0),
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(1.0, -1.0, 1.0),
        Vec3::new(-1.0, -1.0, -1.0),
        Vec3::new(1.0, 1.0, -1.0),
        Vec3::new(-1.0, 1.0, 1.0),
    ];
    let mut out: Vec<Vec3> = Vec::with_capacity(n);
    for d in dirs {
        if out.len() >= n {
            break;
        }
        let best = hull
            .iter()
            .copied()
            .max_by(|a, b| a.dot(d).total_cmp(&b.dot(d)))
            .unwrap_or(Vec3::ZERO);
        if !out.iter().any(|p| p.distance_squared(best) < 1e-8) {
            out.push(best);
        }
    }
    out
}

/// Lightest chunk other debris lands on, kg: chips and pebbles are left to sink in.
const SUPPORT_MIN_KG: f32 = 2.0;
/// How far below a resting chunk's top a point is still pushed up onto it; deeper, it is
/// beside or under the chunk, not on it, m.
const SUPPORT_REACH_M: f32 = 0.25;
/// Size of the buckets resting chunks are sorted into, m.
const SUPPORT_CELL_M: f64 = 1.0;

/// Most contacts the solver keeps per body and step.
const MAX_CONTACTS: usize = 64;
/// Steps along each edge of the lattice a falling chunk's box is sampled at against resting
/// chunks (a chip just uses its corners).
const LATTICE: usize = 4;

/// Points of an `n`-step lattice (0..=n on each axis) on the surface of a box.
fn box_lattice(n: usize) -> impl Iterator<Item = (f32, f32, f32)> {
    (0..=n).flat_map(move |i| {
        (0..=n).flat_map(move |j| {
            (0..=n)
                .filter(move |&k| [i, j, k].iter().any(|&c| c == 0 || c == n))
                .map(move |k| (i as f32, j as f32, k as f32))
        })
    })
}

/// A chunk at rest, as a box later debris lands on.
struct Support {
    entity: Entity,
    center: DVec3,
    /// World to box space.
    inv: Quat,
    half: Vec3,
}

impl Support {
    /// World height of the box's top straight above or below `p` (`p.y` ignored), if the
    /// vertical line through `p` goes through it.
    fn top_at(&self, p: DVec3) -> Option<f64> {
        let o = self.inv
            * Vec3::new(
                (p.x - self.center.x) as f32,
                0.0,
                (p.z - self.center.z) as f32,
            );
        let d = self.inv * Vec3::Y;
        let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
        for k in 0..3 {
            if d[k].abs() < 1e-6 {
                if o[k].abs() > self.half[k] {
                    return None;
                }
                continue;
            }
            let (a, b) = ((-self.half[k] - o[k]) / d[k], (self.half[k] - o[k]) / d[k]);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
        (t0 <= t1).then_some(self.center.y + t1 as f64)
    }
}

/// Resting chunks, bucketed by where they lie.
#[derive(Default)]
struct Rubble {
    supports: Vec<Support>,
    cells: HashMap<(i64, i64), Vec<usize>>,
    entities: HashSet<Entity>,
}

impl Rubble {
    fn cell(p: DVec3) -> (i64, i64) {
        (
            (p.x / SUPPORT_CELL_M).floor() as i64,
            (p.z / SUPPORT_CELL_M).floor() as i64,
        )
    }

    fn add(&mut self, s: Support) {
        let r = s.half.length() as f64;
        let (lo, hi) = (
            Self::cell(s.center - DVec3::splat(r)),
            Self::cell(s.center + DVec3::splat(r)),
        );
        let k = self.supports.len();
        for x in lo.0..=hi.0 {
            for z in lo.1..=hi.1 {
                self.cells.entry((x, z)).or_default().push(k);
            }
        }
        self.entities.insert(s.entity);
        self.supports.push(s);
    }

    /// The resting chunks in the buckets within `r` of `p`, each once, other than `skip`.
    fn candidates(&self, p: DVec3, r: f64, skip: Entity) -> Vec<usize> {
        let (lo, hi) = (
            Self::cell(p - DVec3::splat(r)),
            Self::cell(p + DVec3::splat(r)),
        );
        let mut ids: Vec<usize> = (lo.0..=hi.0)
            .flat_map(|x| (lo.1..=hi.1).map(move |z| (x, z)))
            .filter_map(|c| self.cells.get(&c))
            .flatten()
            .copied()
            .filter(|&k| {
                // Close enough across, and with its top no lower than the bottom of `p ± r`.
                let s = &self.supports[k];
                let size = s.half.length() as f64;
                s.entity != skip
                    && (s.center.x - p.x).hypot(s.center.z - p.z) <= r + size
                    && s.center.y + size >= p.y - r
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// The highest resting top under `p` within [`SUPPORT_REACH_M`] above it among `ids` (from
    /// [`candidates`](Self::candidates)), and whose it is.
    fn top_among(&self, ids: &[usize], p: DVec3) -> Option<(f64, Entity)> {
        ids.iter()
            .map(|&k| &self.supports[k])
            .filter_map(|s| s.top_at(p).map(|h| (h, s.entity)))
            .filter(|(h, _)| *h - p.y <= SUPPORT_REACH_M as f64)
            .max_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// Whether any resting chunk lies within `r` of `p` (by bucket).
    fn near(&self, p: DVec3, r: f64) -> bool {
        if self.supports.is_empty() {
            return false;
        }
        let (lo, hi) = (
            Self::cell(p - DVec3::splat(r)),
            Self::cell(p + DVec3::splat(r)),
        );
        (lo.0..=hi.0).any(|x| (lo.1..=hi.1).any(|z| self.cells.contains_key(&(x, z))))
    }

    fn contains(&self, e: Entity) -> bool {
        self.entities.contains(&e)
    }
}

/// How far under a floor's top a debris point still counts as on it, meters: deeper than a
/// step's fall, shallower than the room below.
const FLOOR_REACH_M: f64 = 0.25;

/// A wall laid flat (a floor, roof deck or hatch cover) that debris lands on.
struct Floor<'a> {
    affine: engine_core::math::DAffine3,
    inv: engine_core::math::DAffine3,
    state: &'a WallState,
}

impl<'a> Floor<'a> {
    fn of(gt: &GlobalTransform, state: &'a WallState) -> Option<Self> {
        ((gt.rotation() * Vec3::Z).y > 0.9).then(|| Self {
            affine: gt.affine(),
            inv: gt.affine().inverse(),
            state,
        })
    }

    /// Height of its top over `p` when `p` is on or just under it, over a solid cell.
    fn top_under(&self, p: DVec3) -> Option<f64> {
        let g = &self.state.grid;
        let lp = self.inv.transform_point3(p);
        let idx = g.cell_at(engine_core::math::Vec2::new(lp.x as f32, lp.y as f32))?;
        if !g.solid[idx] {
            return None;
        }
        let top = (g.thickness * 0.5 - g.front[idx]) as f64;
        (lp.z <= top && lp.z >= top - FLOOR_REACH_M)
            .then(|| self.affine.transform_point3(DVec3::new(lp.x, lp.y, top)).y)
    }

    fn bounds(&self, margin: f64) -> (DVec3, DVec3) {
        let g = &self.state.grid;
        let size = DVec3::new(
            g.cell.x as f64 * g.nx as f64,
            g.cell.y as f64 * g.ny as f64,
            g.thickness as f64,
        );
        let o = DVec3::new(g.origin.x as f64, g.origin.y as f64, -size.z * 0.5);
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for c in 0..8 {
            let k = DVec3::new((c & 1) as f64, ((c >> 1) & 1) as f64, (c >> 2) as f64);
            let w = self.affine.transform_point3(o + size * k);
            lo = lo.min(w);
            hi = hi.max(w);
        }
        (lo - DVec3::splat(margin), hi + DVec3::splat(margin))
    }
}

/// Quadratic air drag of loose debris, `½ ρ_air C_d` (kg/m³) for a tumbling body: deceleration
/// is `AIR_DRAG · area / mass · speed²`. A 5 cm stone chip still falls at about 30 m/s; a
/// 30 cm drywall flake at about 11 m/s.
const AIR_DRAG: f32 = 0.9;

/// How far a piece moves before the ground under it is sampled again, meters (farther while it
/// is high above the ground).
const GROUND_REUSE_M: f64 = 0.3;

/// Seconds in contact after which a piece that is only rocking or creeping settles anyway.
const SETTLE_SECS: f32 = 6.0;

/// The built-in debris solver: gravity, ground and voxel rock contacts with friction and
/// restitution, sleep. Runs in `FixedUpdate` (`SimSet::Physics`), so it slows down with time
/// dilation.
#[allow(clippy::too_many_arguments)]
pub fn simulate_debris_system(
    time: Res<Time>,
    env: Env,
    mut q: Query<(Entity, &mut DebrisPose, &mut DebrisBody, &mut Debris)>,
    volumes: Query<(&GlobalTransform, &VoxelState)>,
    rock_changed: Query<(&GlobalTransform, &VoxelState), Changed<VoxelState>>,
    walls: Query<(&GlobalTransform, &WallState)>,
    walls_changed: Query<(&GlobalTransform, &WallState), Changed<WallState>>,
    mut landings: ResMut<DebrisLandings>,
    range: Option<Res<SimulationRange>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    // Beyond the simulation range of every focus, debris rests where it is: it stops stepping
    // and carries on when someone comes back.
    let range = range.map_or_else(SimulationRange::default, |r| *r);
    let rests = |p: DVec3| range.is_resting(&env.focus, p);
    let setup = tracing::info_span!("debris setup").entered();
    let rock: Vec<(&GlobalTransform, &VoxelState)> = volumes.iter().collect();
    let rock_bounds: Vec<(DVec3, DVec3)> = rock
        .iter()
        .map(|(gt, state)| state.world_bounds(gt, ROCK_REACH_M as f64 + 0.5))
        .collect();
    // Rock dug or blasted: whatever rests on or by it looks again.
    let mut changed: Vec<(DVec3, DVec3)> = rock_changed
        .iter()
        .map(|(gt, state)| state.world_bounds(gt, 1.0))
        .collect();
    // Floors and roof decks (walls laid flat, front face up) hold debris up on their solid
    // cells; whatever rests on one that was shot or blasted looks again.
    let floors: Vec<(Floor, (DVec3, DVec3))> = walls
        .iter()
        .filter_map(|(gt, s)| Floor::of(gt, s))
        .map(|f| {
            let b = f.bounds(FLOOR_REACH_M + 0.05);
            (f, b)
        })
        .collect();
    changed.extend(
        walls_changed
            .iter()
            .filter_map(|(gt, s)| Floor::of(gt, s))
            .map(|f| f.bounds(0.5)),
    );
    // Chunks at rest are what the rest land on.
    let mut rubble = Rubble::default();
    for (entity, pose, body, debris) in &q {
        let t = &pose.now;
        if !body.asleep
            || body.sinking
            || debris.chip
            || debris.mass_kg < SUPPORT_MIN_KG
            || rests(t.translation)
        {
            continue;
        }
        // Its box: the bounds of its contact points (its hull's extremes).
        let Some((lo, hi)) = body.points.iter().fold(None, |b: Option<(Vec3, Vec3)>, p| {
            let p = *p * t.scale;
            Some(b.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))))
        }) else {
            continue;
        };
        rubble.add(Support {
            entity,
            center: t.translation + (t.rotation * ((lo + hi) * 0.5)).as_dvec3(),
            inv: t.rotation.inverse(),
            half: (hi - lo) * 0.5,
        });
    }
    // Pieces step in parallel (each reads only the rubble and rock as they were at the start of
    // the step); their landings are written afterwards.
    let hits = std::sync::Mutex::new((Vec::new(), Vec::new()));
    drop(setup);
    let _step = tracing::info_span!("debris step").entered();
    q.par_iter_mut()
        .for_each(|(entity, mut pose, mut body, mut debris)| {
            let t = &pose.now;
            if !body.sinking && rests(t.translation) {
                return;
            }
            // What it lay on was dug, blasted, woken, or sank away: it falls again. Asleep and
            // undisturbed, it is only looked at (not written, so nothing redraws or re-propagates it).
            if body.asleep && !body.sinking {
                let lost = body.on.is_some_and(|s| !rubble.contains(s));
                let p = t.translation;
                if !lost
                    && !changed
                        .iter()
                        .any(|(lo, hi)| p.cmpge(*lo).all() && p.cmple(*hi).all())
                {
                    return;
                }
            }
            let (t, body) = (&mut pose.now, &mut *body);
            if body.sinking {
                t.translation -= env.up(t.translation).as_dvec3() * (0.35 * dt) as f64;
                return;
            }
            if body.asleep {
                body.asleep = false;
                body.still = 0.0;
                body.awake_secs = 0.0;
                body.on = None;
            }
            let g = env.gravity(t.translation);
            let up = env.up(t.translation);
            body.velocity += g * dt;
            // Air drag on its broadest face: light flakes and chips slow down hard and land near
            // where they broke off instead of sailing across the yard.
            let drag = AIR_DRAG * body.broad_area() * body.inv_mass;
            body.velocity /= 1.0 + 0.02 * dt + drag * body.velocity.length() * dt;
            body.angular *= 1.0 / (1.0 + 0.15 * dt);
            t.translation += (body.velocity * dt).as_dvec3();
            let w = body.angular;
            if w.length_squared() > 1e-12 {
                let dq = Quat::from_xyzw(w.x, w.y, w.z, 0.0) * t.rotation;
                t.rotation = Quat::from_xyzw(
                    t.rotation.x + 0.5 * dt * dq.x,
                    t.rotation.y + 0.5 * dt * dq.y,
                    t.rotation.z + 0.5 * dt * dq.z,
                    t.rotation.w + 0.5 * dt * dq.w,
                )
                .normalize();
            }
            // The ground under it, sampled again only once it has moved off the spot of the last
            // sample (the higher above the ground, the farther): terrain samples were most of
            // the solver's cost.
            let p = t.translation;
            let (ground_at, ground) = match body.ground {
                Some((at, g))
                    if (p.x - at.x).hypot(p.z - at.z)
                        < GROUND_REUSE_M.max(0.5 * (p.y - g.height)) =>
                {
                    (at, g)
                }
                _ => {
                    let Some(g) = env.ground(p) else {
                        return;
                    };
                    body.ground = Some((p, g));
                    (p, g)
                }
            };
            let n = ground.normal.normalize_or(up);
            let scale = t.scale;
            let rot = t.rotation;
            let center = t.translation;
            let reach = body
                .points
                .iter()
                .map(|p| (*p * scale).length())
                .fold(0.0f32, f32::max) as f64;
            // Only the rock it could touch (rock queries were a large part of every piece's
            // step, near rock or not).
            let rock: Vec<(&GlobalTransform, &VoxelState)> = rock
                .iter()
                .zip(&rock_bounds)
                .filter(|(_, (lo, hi))| {
                    (center + DVec3::splat(reach)).cmpge(*lo).all()
                        && (center - DVec3::splat(reach)).cmple(*hi).all()
                })
                .map(|(r, _)| *r)
                .collect();
            // Out of its hole: past the rims and all of it clear of the rock (its hull's extremes,
            // its center and a lattice over its box no coarser than a thin shelf), it lands on rock
            // from now on.
            if let RockContact::Leaving(from) = body.rock
                && center.distance(from) >= LEAVE_M as f64
            {
                let inside = |local: Vec3| in_rock(&rock, center + (rot * local).as_dvec3());
                let (lo, hi) = body.points.iter().fold(
                    (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), p| (lo.min(*p * scale), hi.max(*p * scale)),
                );
                let steps =
                    (((hi - lo).max_element() / LEAVE_LATTICE_M).ceil() as usize).clamp(1, 8);
                let clear = !body.points.is_empty()
                    && !body.points.iter().any(|p| inside(*p * scale))
                    && !inside(Vec3::ZERO)
                    && !box_lattice(steps).any(|(i, j, k)| {
                        inside(lo + (hi - lo) * Vec3::new(i, j, k) / steps as f32)
                    });
                if clear {
                    body.rock = RockContact::Lands;
                }
            }
            if !rock.is_empty() && matches!(body.rock, RockContact::Unknown | RockContact::Buried) {
                let buried = body
                    .points
                    .iter()
                    .map(|p| rock_depth(&rock, center + (rot * (*p * scale)).as_dvec3()))
                    .fold(0.0, f32::max);
                let limit = if body.rock == RockContact::Unknown {
                    BURIED_M
                } else {
                    0.0
                };
                body.rock = if buried <= limit {
                    RockContact::Lands
                } else {
                    RockContact::Buried
                };
            }
            let on_rock = body.rock == RockContact::Lands && !rock.is_empty();
            // Collect penetrating points.
            let mut touching = [(Vec3::ZERO, 0.0f32, Vec3::ZERO); MAX_CONTACTS];
            let mut count = 0;
            let mut touch = |r: Vec3, depth: f32, normal: Vec3| {
                if depth > 0.0 && count < MAX_CONTACTS {
                    touching[count] = (r, depth, normal);
                    count += 1;
                }
            };
            // The ground under each point from that one sample, along the ground's tangent plane
            // (a piece spans a meter or two).
            let (slope_x, slope_z) = {
                let ny = n.y.max(0.3) as f64;
                (-(n.x as f64) / ny, -(n.z as f64) / ny)
            };
            let near_floors: Vec<&Floor> = floors
                .iter()
                .filter(|(_, (lo, hi))| {
                    (center + DVec3::splat(reach)).cmpge(*lo).all()
                        && (center - DVec3::splat(reach)).cmple(*hi).all()
                })
                .map(|(f, _)| f)
                .collect();
            let plane = |q: DVec3| {
                let d = q - ground_at;
                ground.height + (slope_x * d.x + slope_z * d.z).clamp(-reach - 2.0, reach + 2.0)
            };
            // Clear of the ground, floors, rock and piles by more than its own size: it can't be
            // touching anything, so it only flies on.
            let steepest = slope_x.hypot(slope_z);
            if near_floors.is_empty()
                && !on_rock
                && center.y - plane(center) > reach * (1.0 + steepest) + 0.05
                && !rubble.near(center, reach)
            {
                body.still = 0.0;
                body.on = None;
                return;
            }
            // The ground and rock, at its hull's extreme points: rock floors hold it up like the
            // ground, rock walls and ceilings push it out of their face, so it slides down a cliff
            // onto the ledge below instead of catching on the cliff's edge.
            for p in &body.points {
                let r = rot * (*p * scale);
                let wp = center + r.as_dvec3();
                let mut h = plane(wp);
                for f in &near_floors {
                    if let Some(top) = f.top_under(wp) {
                        h = h.max(top);
                    }
                }
                if on_rock {
                    match rock_side(&rock, wp, ROCK_REACH_M) {
                        Some((normal, depth)) => touch(r, depth, normal),
                        None => {
                            if let Some(r) = rock_contact(&rock, wp, ROCK_REACH_M) {
                                h = h.max(r);
                            }
                        }
                    }
                }
                touch(r, ((h - wp.y) as f32) * n.y.max(0.2), n);
            }
            // Resting chunks, box on box: a lattice over the lower half of its box, so it lies
            // across an edge below it instead of tipping through it.
            let mut on = None;
            let piles = if body.points.is_empty() {
                Vec::new()
            } else {
                rubble.candidates(center, reach, entity)
            };
            if !piles.is_empty() {
                let (lo, hi) = body.points.iter().fold(
                    (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), p| (lo.min(*p * scale), hi.max(*p * scale)),
                );
                let mut best = 0.0f32;
                let steps = if debris.chip { 1 } else { LATTICE };
                for (i, j, k) in box_lattice(steps) {
                    let local = lo + (hi - lo) * Vec3::new(i, j, k) / steps as f32;
                    let r = rot * local;
                    if r.dot(up) > 0.0 {
                        continue;
                    }
                    let wp = center + r.as_dvec3();
                    if let Some((top, s)) = rubble.top_among(&piles, wp) {
                        let depth = (top - wp.y) as f32;
                        if depth > best {
                            best = depth;
                            on = Some(s);
                        }
                        touch(r, depth, n);
                    }
                }
            }
            if count == 0 {
                body.still = 0.0;
                body.on = None;
                return;
            }
            body.on = on;
            let inv_i_world = |v: Vec3, b: &DebrisBody| rot * (b.inv_inertia * (rot.inverse() * v));
            let impact = -(body.velocity.dot(n)).min(0.0);
            body.last_impact = impact;
            debris.max_impact_speed = debris.max_impact_speed.max(impact);
            if impact > 2.5 && debris.mass_kg > 30.0 {
                let mut hits = hits.lock().unwrap_or_else(|e| e.into_inner());
                hits.0.push(GroundContact {
                    position: center,
                    normal: n,
                    size: [0.5, 0.5],
                    yaw: 0.0,
                    load_kg: debris.mass_kg,
                    speed: body.velocity.length(),
                    kind: ContactKind::Body,
                    surface: Some(ground.surface),
                    entity: Some(entity),
                });
                hits.1.push(NoiseEvent {
                    position: center,
                    radius: (debris.mass_kg.sqrt() * impact).clamp(5.0, 80.0),
                    source: Some(entity),
                });
            }
            // Each contact pushes along its own normal: the ground's, or a rock face's.
            let bounce = body.restitution;
            let targets: [f32; MAX_CONTACTS] = std::array::from_fn(|k| {
                let speed = -(body.velocity.dot(touching[k].2)).min(0.0);
                if speed > 1.5 {
                    -bounce * speed.min(8.0)
                } else {
                    0.0
                }
            });
            let mut normal_impulse = [0.0f32; MAX_CONTACTS];
            for _ in 0..4 {
                for k in 0..count {
                    let (r, _, nk) = touching[k];
                    let v = body.velocity + body.angular.cross(r);
                    let vn = v.dot(nk);
                    let rn = r.cross(nk);
                    let kn = body.inv_mass + inv_i_world(rn, body).cross(r).dot(nk);
                    let mut j = (targets[k] - vn) / kn.max(1e-6);
                    let old = normal_impulse[k];
                    normal_impulse[k] = (old + j).max(0.0);
                    j = normal_impulse[k] - old;
                    let imp = nk * j;
                    body.velocity += imp * body.inv_mass;
                    let dw = inv_i_world(r.cross(imp), body);
                    body.angular += dw;
                    // Friction.
                    let v = body.velocity + body.angular.cross(r);
                    let vt = v - nk * v.dot(nk);
                    let speed_t = vt.length();
                    if speed_t > 1e-5 {
                        let tdir = vt / speed_t;
                        let rt = r.cross(tdir);
                        let kt = body.inv_mass + inv_i_world(rt, body).cross(r).dot(tdir);
                        let jt = (-speed_t / kt.max(1e-6)).max(-body.friction * normal_impulse[k]);
                        let imp = tdir * jt;
                        body.velocity += imp * body.inv_mass;
                        let dw = inv_i_world(r.cross(imp), body);
                        body.angular += dw;
                    }
                }
            }
            // Rolling resistance so pieces settle instead of rocking forever.
            body.angular *= 1.0 / (1.0 + 2.5 * dt);
            // Out of whatever it sank into: enough along each normal for its deepest point there.
            let mut push = Vec3::ZERO;
            for &(_, depth, normal) in &touching[..count] {
                let need = depth - 0.002 - push.dot(normal);
                if need > 0.0 {
                    push += normal * need;
                }
            }
            t.translation += (push * 0.8).as_dvec3();
            body.awake_secs += dt;
            let (v2, w2) = (
                body.velocity.length_squared(),
                body.angular.length_squared(),
            );
            if v2 < 0.12 * 0.12 && w2 < 0.3 * 0.3 {
                body.still += dt;
                if body.still > 0.5 {
                    body.asleep = true;
                    body.velocity = Vec3::ZERO;
                    body.angular = Vec3::ZERO;
                }
            } else if body.awake_secs > SETTLE_SECS && v2 < 0.5 * 0.5 && w2 < 1.5 * 1.5 {
                // Still rocking or creeping long after it landed: it settles where it is.
                body.asleep = true;
                body.velocity = Vec3::ZERO;
                body.angular = Vec3::ZERO;
            } else {
                body.still = 0.0;
            }
        });
    // In entity order, whichever thread found them.
    let (mut ground_contacts, mut noises): (Vec<GroundContact>, Vec<NoiseEvent>) =
        hits.into_inner().unwrap_or_else(|e| e.into_inner());
    ground_contacts.sort_by_key(|c| c.entity);
    noises.sort_by_key(|n| n.source);
    landings.contacts.extend(ground_contacts);
    landings.noises.extend(noises);
}

/// Age debris, enforce the budget, sink and remove expired pieces, and animate dust.
#[allow(clippy::too_many_arguments)]
pub fn age_debris_system(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<DestructionSettings>,
    env: Env,
    assets: Option<Res<DestructionAssets>>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut q: Query<(
        Entity,
        &mut Debris,
        Option<&mut DebrisBody>,
        Option<&DebrisMesh>,
        Option<&RigidBody>,
        Option<&Spark>,
        &Transform,
    )>,
    mut dust: Query<
        (
            Entity,
            &mut DustPuff,
            &mut Transform,
            Option<&mut MeshRenderer>,
            Option<&mut Sprite>,
        ),
        Without<Debris>,
    >,
    mut stats: ResMut<DestructionStats>,
    mut order: Local<Vec<(f32, Entity)>>,
) {
    let dt = time.delta_secs();
    let (mut chunks, mut chips) = (0usize, 0usize);
    // Pieces not yet sinking: what the budget counts (sinking ones are already on their way out).
    let (mut live_chunks, mut live_chips) = (0usize, 0usize);
    for (_, d, ..) in &q {
        let live = (d.age < d.lifetime) as usize;
        if d.chip {
            chips += 1;
            live_chips += live;
        } else {
            chunks += 1;
            live_chunks += live;
        }
    }
    // Over budget: the oldest start sinking now.
    for (chip, count, max) in [
        (false, live_chunks, settings.max_debris),
        (true, live_chips, settings.max_chips),
    ] {
        if count <= max {
            continue;
        }
        order.clear();
        for (e, d, ..) in &q {
            if d.chip == chip && d.age < d.lifetime {
                order.push((d.age / d.lifetime.max(1e-3), e));
            }
        }
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        for &(_, e) in order.iter().take(count - max) {
            if let Ok((_, mut d, ..)) = q.get_mut(e) {
                d.age = d.lifetime;
            }
        }
    }
    let sink = settings.sink_secs.max(0.1);
    for (e, mut d, body, mesh, rigid, spark, _) in &mut q {
        d.age += dt;
        if spark.is_some() && d.age >= d.lifetime {
            // Sparks burn out instead of sinking.
            commands.entity(e).despawn();
            continue;
        }
        if d.age < d.lifetime {
            continue;
        }
        if d.age >= d.lifetime + sink {
            if let (Some(m), Some(meshes)) = (mesh, meshes.as_deref_mut()) {
                m.free(meshes);
            }
            commands.entity(e).despawn();
            stats.despawned += 1;
            continue;
        }
        match body {
            Some(mut b) => {
                b.sinking = true;
                b.asleep = false;
            }
            None => {
                if rigid.is_some() {
                    // Hand the body back from physics to the sinking animation.
                    commands
                        .entity(e)
                        .remove::<(RigidBody, Collider, Velocity)>()
                        .insert(DebrisBody {
                            sinking: true,
                            ..DebrisBody::new(d.mass_kg, Vec3::ONE, Vec::new(), 0.5)
                        });
                }
            }
        }
    }
    stats.chunks = chunks;
    stats.chips = chips;

    let Some(assets) = assets else {
        return;
    };
    let mut puffs = 0;
    for (e, mut p, mut t, mesh, sprite) in &mut dust {
        let (p, t) = (&mut *p, &mut *t);
        puffs += 1;
        p.age += dt;
        if p.age >= p.life {
            commands.entity(e).despawn();
            continue;
        }
        let k = p.age / p.life;
        let grow = 1.0 - (1.0 - k).powi(3);
        let size = p.start_size + (p.end_size - p.start_size) * grow;
        t.scale = Vec3::splat(size);
        let wind = env.wind(t.translation);
        p.velocity += (wind - p.velocity) * (1.0 - (-1.2 * dt).exp());
        p.velocity.y *= 1.0 / (1.0 + 0.6 * dt);
        t.translation += (p.velocity * dt).as_dvec3();
        // The same dust spread over a growing cloud: thinner as it grows, so a puff hazes
        // out instead of hanging in the air as a solid ball.
        let thin = (p.start_size / size).powf(DUST_THINNING);
        let opacity = p.opacity * thin * (1.0 - k) * (k * 8.0).min(1.0);
        if let Some(mut r) = mesh {
            let handle = assets.dust(p.darkness, opacity);
            if r.material != MaterialRef::Handle(handle) {
                r.material = MaterialRef::Handle(handle);
            }
        }
        if let Some(mut sprite) = sprite {
            sprite.color[3] = opacity * DUST_SPRITE_ALPHA;
        }
    }
    stats.dust = puffs;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_points_keeps_extremes() {
        let pts: Vec<Vec3> = (0..100)
            .map(|i| {
                let a = i as f32 * 0.37;
                Vec3::new(a.cos(), (a * 1.3).sin(), (a * 0.7).cos()) * 2.0
            })
            .collect();
        let r = reduce_points(&pts, 10);
        assert!(r.len() <= 10 && r.len() >= 6);
        let max_x = pts.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        assert!(r.iter().any(|p| (p.x - max_x).abs() < 1e-6));
    }

    #[test]
    fn body_inertia_is_positive() {
        let b = DebrisBody::new(10.0, Vec3::new(0.5, 0.2, 0.1), box_corners(Vec3::ONE), 0.6);
        assert!(b.inv_inertia.min_element() > 0.0);
        assert!((b.inv_mass - 0.1).abs() < 1e-6);
    }
}
