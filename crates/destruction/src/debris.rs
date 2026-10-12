//! Debris: chunks, chips and dust thrown off by destruction.
//!
//! Large chunks become physics-contract bodies (`RigidBody` + `Collider` + `Velocity`) when a
//! physics backend is installed, so they stack and collide with everything. Without one, and
//! always for small chips, a built-in lightweight solver tumbles them onto the ground (the
//! `Ground` query), lets them settle and sleep. Everything sinks out of sight after its
//! lifetime and a budget caps how much debris exists at once.

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, MeshRenderer};
use engine_world::physics::{
    BodyKind, Collider, ColliderShape, PhysicsBackend, RigidBody, Velocity,
};
use engine_world::{ContactKind, Env, GroundContact, NoiseEvent, SurfaceMaterials};

use crate::assets::DestructionAssets;
use crate::material::{DestructibleMaterial, DestructionMaterials};
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

/// A mesh owned by one piece of debris (freed when it despawns).
#[derive(Component, Debug, Clone, Copy)]
pub struct DebrisMesh(pub Handle<Mesh>);

/// State of a body moved by the built-in debris solver.
#[derive(Component, Debug, Clone)]
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
}

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
        }
    }

    /// Push the body (wakes it).
    pub fn kick(&mut self, impulse: Vec3, angular: Vec3) {
        self.velocity += impulse * self.inv_mass;
        self.angular += angular;
        self.asleep = false;
        self.still = 0.0;
    }
}

/// Glowing hot fragments (sparks): fade instead of sinking.
#[derive(Component, Debug, Clone, Copy)]
pub struct Spark;

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
) {
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
    for item in pending.items.drain(..) {
        let props = table.get(item.material);
        let surface = surfaces.id_or_default(&props.surface);
        let friction = surfaces.get(surface).friction;
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
            e.insert(DebrisMesh(h));
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
        if use_physics && !chip {
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
            e.insert(body);
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
                commands.spawn((
                    Transform::from_translation(d.position + offset.as_dvec3())
                        .with_scale(Vec3::splat(size * 0.4)),
                    MeshRenderer::new(
                        MeshRef::Handle(assets.dust_mesh),
                        MaterialRef::Handle(assets.dust(d.darkness, 1.0)),
                    )
                    .without_shadows(),
                    DustPuff {
                        age: 0.0,
                        life: 2.5 + 1.5 * (next() + 0.5),
                        start_size: size * 0.4,
                        end_size: size * 1.6,
                        velocity: d.velocity + Vec3::new(next(), 0.3 + next() * 0.2, next()) * 0.8,
                        darkness: d.darkness,
                        opacity: 1.0,
                    },
                ));
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

/// The built-in debris solver: gravity, ground contacts with friction and restitution, sleep.
/// Runs in `FixedUpdate` (`SimSet::Physics`), so it slows down with time dilation.
pub fn simulate_debris_system(
    time: Res<Time>,
    env: Env,
    mut q: Query<(Entity, &mut Transform, &mut DebrisBody, &mut Debris)>,
    mut contacts: MessageWriter<GroundContact>,
    mut noise: MessageWriter<NoiseEvent>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, mut t, mut body, mut debris) in &mut q {
        let (t, body) = (&mut *t, &mut *body);
        if body.sinking {
            t.translation -= env.up(t.translation).as_dvec3() * (0.35 * dt) as f64;
            continue;
        }
        if body.asleep {
            continue;
        }
        let g = env.gravity(t.translation);
        let up = env.up(t.translation);
        body.velocity += g * dt;
        body.velocity *= 1.0 / (1.0 + 0.02 * dt);
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
        let Some(ground) = env.ground(t.translation) else {
            continue;
        };
        let n = ground.normal.normalize_or(up);
        let scale = t.scale;
        let rot = t.rotation;
        let center = t.translation;
        // Collect penetrating points.
        let mut touching: [(Vec3, f32); 16] = [(Vec3::ZERO, 0.0); 16];
        let mut count = 0;
        let mut deepest = 0.0f32;
        for p in &body.points {
            let r = rot * (*p * scale);
            let wp = center + r.as_dvec3();
            let h = env.ground_query.height(wp).unwrap_or(ground.height);
            let depth = ((h - wp.y) as f32) * n.y.max(0.2);
            if depth > 0.0 && count < touching.len() {
                touching[count] = (r, depth);
                count += 1;
                deepest = deepest.max(depth);
            }
        }
        if count == 0 {
            body.still = 0.0;
            continue;
        }
        let inv_i_world = |v: Vec3, b: &DebrisBody| rot * (b.inv_inertia * (rot.inverse() * v));
        let impact = -(body.velocity.dot(n)).min(0.0);
        body.last_impact = impact;
        debris.max_impact_speed = debris.max_impact_speed.max(impact);
        if impact > 2.5 && debris.mass_kg > 30.0 {
            contacts.write(GroundContact {
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
            noise.write(NoiseEvent {
                position: center,
                radius: (debris.mass_kg.sqrt() * impact).clamp(5.0, 80.0),
                source: Some(entity),
            });
        }
        let restitution = if impact > 1.5 { body.restitution } else { 0.0 };
        let mut normal_impulse = [0.0f32; 16];
        for _ in 0..4 {
            for k in 0..count {
                let (r, _) = touching[k];
                let v = body.velocity + body.angular.cross(r);
                let vn = v.dot(n);
                let rn = r.cross(n);
                let kn = body.inv_mass + inv_i_world(rn, body).cross(r).dot(n);
                let target = -restitution * impact.min(8.0);
                let mut j = (target - vn) / kn.max(1e-6);
                let old = normal_impulse[k];
                normal_impulse[k] = (old + j).max(0.0);
                j = normal_impulse[k] - old;
                let imp = n * j;
                body.velocity += imp * body.inv_mass;
                let dw = inv_i_world(r.cross(imp), body);
                body.angular += dw;
                // Friction.
                let v = body.velocity + body.angular.cross(r);
                let vt = v - n * v.dot(n);
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
        if deepest > 0.002 {
            t.translation += (n * (deepest - 0.002) * 0.8).as_dvec3();
        }
        if body.velocity.length_squared() < 0.12 * 0.12 && body.angular.length_squared() < 0.3 * 0.3
        {
            body.still += dt;
            if body.still > 0.5 {
                body.asleep = true;
                body.velocity = Vec3::ZERO;
                body.angular = Vec3::ZERO;
            }
        } else {
            body.still = 0.0;
        }
    }
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
    mut dust: Query<(Entity, &mut DustPuff, &mut Transform, &mut MeshRenderer), Without<Debris>>,
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
                meshes.remove(m.0);
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
    for (e, mut p, mut t, mut r) in &mut dust {
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
        let opacity = p.opacity * (1.0 - k).powf(1.5) * (k * 8.0).min(1.0);
        let handle = assets.dust(p.darkness, opacity);
        if r.material != MaterialRef::Handle(handle) {
            r.material = MaterialRef::Handle(handle);
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
