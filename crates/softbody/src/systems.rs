//! The plugin's systems: build bodies from their specs, gather colliders, react to world
//! events, step every body (in parallel) and send the results back to the world.

use std::collections::HashMap;
use std::time::Instant;

use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::physics::{
    ApplyImpulse, BodyKind, CharacterController, CharacterState, Collider, ColliderShape,
    MassProperties, RigidBody,
};
use engine_world::{
    ContactKind, Env, GroundContact, IgniteRequest, NoiseEvent, ProjectileTrace, SmokeEmission,
    SurfaceMaterials, WorldImpulse,
};

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::character::BoneColliders;
use crate::collide::{LocalCollider, LocalShape};
use crate::rope::{BridgePlank, SoftLoad};
use crate::settings::SoftBodyConfig;
use crate::solver::ColliderImpulse;
use crate::{Cloth, HairGroom, Rope, RopeBridge, SoftVolume, cloth, hair, rope, volume};

/// A spec that couldn't be built, and why. Removed (and the build retried) when the spec
/// changes.
#[derive(Component, Debug, Clone)]
pub struct SoftBodyFailed(pub String);

/// Results of the last step, sent to the world by [`emit_outputs`].
#[derive(Debug, Clone, Default)]
pub struct StepOutput {
    /// Reaction impulses per world collider index.
    pub impulses: Vec<(u32, ColliderImpulse)>,
    /// Reaction impulses on entities that rope ends are tied to (entity, impulse, frame
    /// position).
    pub pulls: Vec<(Entity, ColliderImpulse, DVec3)>,
    pub smoke: Vec<DVec3>,
    /// Landing speed and point.
    pub impact: Option<(f32, DVec3)>,
}

/// A collider as every soft body sees it this step.
#[derive(Debug, Clone, Copy)]
pub struct WorldCollider {
    pub shape: LocalShape,
    pub pos: DVec3,
    pub rot: Quat,
    pub prev_pos: DVec3,
    pub prev_rot: Quat,
    pub friction: f32,
    /// Collider entity (for ignore lists).
    pub entity: Entity,
    /// Rigid body to push back, when dynamic.
    pub body: Option<Entity>,
    /// That body's center of mass.
    pub com: DVec3,
}

impl WorldCollider {
    fn bound(&self) -> f64 {
        self.shape.bounding_radius() as f64 + (self.pos - self.prev_pos).length()
    }
}

/// Every collider soft bodies can touch this step, plus last step's poses.
#[derive(Resource, Default)]
pub struct ColliderWorld {
    pub list: Vec<WorldCollider>,
    prev: HashMap<(Entity, u32), (DVec3, Quat)>,
    seen: HashMap<(Entity, u32), (DVec3, Quat)>,
}

impl ColliderWorld {
    fn push(&mut self, key: (Entity, u32), mut c: WorldCollider) {
        let (pp, pr) = self.prev.get(&key).copied().unwrap_or((c.pos, c.rot));
        // A teleport (more than 10 m in a step) doesn't sweep.
        if (pp - c.pos).length() < 10.0 {
            c.prev_pos = pp;
            c.prev_rot = pr;
        } else {
            c.prev_pos = c.pos;
            c.prev_rot = c.rot;
        }
        self.seen.insert(key, (c.pos, c.rot));
        self.list.push(c);
    }
}

/// Loads standing on bridges this step: feet position, mass, the entity they stand on.
#[derive(Resource, Default)]
pub struct LoadList(pub Vec<(DVec3, f32, Option<Entity>)>);

/// World transform from `Transform` for root entities (fresh this step, even before
/// propagation) or `GlobalTransform` for children.
fn world_transform(
    e: Entity,
    q: &Query<(&GlobalTransform, Option<&Transform>, Option<&ChildOf>)>,
) -> Option<GlobalTransform> {
    let (g, t, parent) = q.get(e).ok()?;
    Some(match (t, parent) {
        (Some(t), None) => GlobalTransform::from(*t),
        _ => *g,
    })
}

fn convert_shape(
    shape: &ColliderShape,
    scale: Vec3,
    offset: Vec3,
    rot: Quat,
    out: &mut Vec<(LocalShape, Vec3, Quat)>,
) {
    let s = scale.abs();
    match shape {
        ColliderShape::Sphere { radius } => {
            out.push((
                LocalShape::Sphere {
                    radius: radius * s.max_element(),
                },
                offset,
                rot,
            ));
        }
        ColliderShape::Cuboid { half_extents } => {
            out.push((
                LocalShape::Cuboid {
                    half: *half_extents * s,
                },
                offset,
                rot,
            ));
        }
        ColliderShape::Capsule {
            radius,
            half_height,
        } => out.push((
            LocalShape::Capsule {
                half_height: half_height * s.y,
                radius: radius * s.x.max(s.z),
            },
            offset,
            rot,
        )),
        ColliderShape::Cylinder {
            radius,
            half_height,
        } => out.push((
            LocalShape::Cylinder {
                half_height: half_height * s.y,
                radius: radius * s.x.max(s.z),
            },
            offset,
            rot,
        )),
        ColliderShape::ConvexHull { points } => {
            if points.is_empty() {
                return;
            }
            let aabb = Aabb::from_points(points.iter().map(|p| *p * s));
            out.push((
                LocalShape::Cuboid {
                    half: aabb.half_extents(),
                },
                offset + rot * aabb.center(),
                rot,
            ));
        }
        ColliderShape::Compound(parts) => {
            for (o, r, part) in parts {
                convert_shape(part, scale, offset + rot * (*o * s), rot * *r, out);
            }
        }
        // Meshes and heightfields are static scenery: soft bodies rely on `Ground` for terrain
        // and skip arbitrary meshes.
        ColliderShape::TriMesh { .. } | ColliderShape::Heightfield { .. } => {}
    }
}

/// Collect physics colliders, character capsules and bone capsules.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn gather_colliders(
    mut world: ResMut<ColliderWorld>,
    colliders: Query<(Entity, &Collider, Option<&ChildOf>)>,
    transforms: Query<(&GlobalTransform, Option<&Transform>, Option<&ChildOf>)>,
    rigid: Query<(&RigidBody, Option<&MassProperties>)>,
    characters: Query<(Entity, &CharacterController), Without<BoneColliders>>,
    skinned: Query<(Entity, &BoneColliders, &SkeletonPose)>,
    materials: Res<SurfaceMaterials>,
    mut shapes: Local<Vec<(LocalShape, Vec3, Quat)>>,
    mut bone_shapes: Local<Vec<(LocalShape, DVec3, Quat)>>,
) {
    let world = &mut *world;
    world.list.clear();
    world.seen.clear();
    for (entity, collider, parent) in &colliders {
        if collider.sensor {
            continue;
        }
        let Some(t) = world_transform(entity, &transforms) else {
            continue;
        };
        // The rigid body this collider belongs to: itself or an ancestor.
        let mut body = None;
        let mut cursor = Some(entity);
        let mut next_parent = parent.map(|p| p.0);
        for _ in 0..4 {
            let Some(e) = cursor else { break };
            if let Ok((rb, mass)) = rigid.get(e) {
                body = Some((e, rb.kind, mass.copied()));
                break;
            }
            cursor = next_parent;
            next_parent = cursor
                .and_then(|c| transforms.get(c).ok())
                .and_then(|(_, _, p)| p.map(|p| p.0));
        }
        let dynamic = matches!(body, Some((_, BodyKind::Dynamic, _)));
        let com = match body {
            Some((b, _, mass)) => {
                let bt = world_transform(b, &transforms).unwrap_or(t);
                let c = mass.map(|m| m.center_of_mass).unwrap_or(Vec3::ZERO);
                bt.transform_point(c.as_dvec3())
            }
            None => t.translation(),
        };
        let friction = materials.get(collider.surface).friction;
        shapes.clear();
        convert_shape(
            &collider.shape,
            t.scale(),
            Vec3::ZERO,
            Quat::IDENTITY,
            &mut shapes,
        );
        let rot = t.rotation();
        for (k, (shape, offset, r)) in shapes.iter().enumerate() {
            world.push(
                (entity, k as u32),
                WorldCollider {
                    shape: *shape,
                    pos: t.translation() + (rot * *offset).as_dvec3(),
                    rot: rot * *r,
                    prev_pos: DVec3::ZERO,
                    prev_rot: Quat::IDENTITY,
                    friction,
                    entity,
                    body: dynamic.then(|| body.map(|b| b.0)).flatten(),
                    com,
                },
            );
        }
    }
    for (entity, cc) in &characters {
        let Some(t) = world_transform(entity, &transforms) else {
            continue;
        };
        world.push(
            (entity, 10_000),
            WorldCollider {
                shape: LocalShape::Capsule {
                    half_height: (cc.height * 0.5 - cc.radius).max(0.0),
                    radius: cc.radius,
                },
                pos: t.translation(),
                rot: Quat::IDENTITY,
                prev_pos: DVec3::ZERO,
                prev_rot: Quat::IDENTITY,
                friction: 0.6,
                entity,
                body: None,
                com: t.translation(),
            },
        );
    }
    for (entity, bones, pose) in &skinned {
        let Some(t) = world_transform(entity, &transforms) else {
            continue;
        };
        bone_shapes.clear();
        bones.resolve(&t, pose, &mut bone_shapes);
        for (k, (shape, pos, rot)) in bone_shapes.iter().enumerate() {
            world.push(
                (entity, 20_000 + k as u32),
                WorldCollider {
                    shape: *shape,
                    pos: *pos,
                    rot: *rot,
                    prev_pos: DVec3::ZERO,
                    prev_rot: Quat::IDENTITY,
                    friction: bones.friction,
                    entity,
                    body: None,
                    com: *pos,
                },
            );
        }
    }
    std::mem::swap(&mut world.prev, &mut world.seen);
}

/// Collect weights standing on soft decks: physics characters and [`SoftLoad`]s.
pub fn gather_loads(
    mut loads: ResMut<LoadList>,
    characters: Query<(&Transform, &CharacterController, Option<&CharacterState>)>,
    soft: Query<(&GlobalTransform, &SoftLoad)>,
) {
    loads.0.clear();
    for (t, cc, state) in &characters {
        let feet = t.translation - DVec3::Y * (cc.height * 0.5) as f64;
        loads
            .0
            .push((feet, cc.mass_kg, state.and_then(|s| s.ground_entity)));
    }
    for (t, l) in &soft {
        loads.0.push((t.translation(), l.mass_kg, None));
    }
}

/// The skeleton of `e` or its nearest ancestor that has one.
fn find_skeleton<'a>(
    e: Entity,
    parents: &Query<&ChildOf>,
    skeletons: &'a Query<(&GlobalTransform, &SkeletonPose)>,
) -> Option<(Entity, &'a GlobalTransform, &'a SkeletonPose)> {
    let mut cursor = Some(e);
    for _ in 0..8 {
        let c = cursor?;
        if let Ok((t, p)) = skeletons.get(c) {
            return Some((c, t, p));
        }
        cursor = parents.get(c).ok().map(|p| p.0);
    }
    None
}

fn report(commands: &mut Commands, e: Entity, what: &str, result: Result<SoftBodySim, String>) {
    match result {
        Ok(body) => {
            commands.entity(e).insert(body);
        }
        Err(reason) => {
            engine_core::log::warn!("{what} on entity {e} can't be built: {reason}");
            commands.entity(e).insert(SoftBodyFailed(reason));
        }
    }
}

type Unbuilt = (Without<SoftBodySim>, Without<SoftBodyFailed>);

/// Build simulations for new specs (after transforms propagate, so they start in place).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn build_bodies(
    mut commands: Commands,
    cloths: Query<(Entity, &Cloth, &GlobalTransform), Unbuilt>,
    ropes: Query<(Entity, &Rope, &GlobalTransform), Unbuilt>,
    bridges: Query<(Entity, &RopeBridge, &GlobalTransform), Unbuilt>,
    volumes: Query<(Entity, &SoftVolume, &GlobalTransform), Unbuilt>,
    grooms: Query<(Entity, &HairGroom), Unbuilt>,
    parents: Query<&ChildOf>,
    skeletons: Query<(&GlobalTransform, &SkeletonPose)>,
    all_transforms: Query<&GlobalTransform>,
) {
    for (e, spec, t) in &cloths {
        let skeleton = find_skeleton(e, &parents, &skeletons);
        if matches!(spec.pins, cloth::ClothPins::Bones { .. })
            && skeleton.is_some_and(|s| s.2.bones.is_empty())
        {
            continue; // the pose isn't evaluated yet
        }
        report(
            &mut commands,
            e,
            "Cloth",
            cloth::build(spec, e, t, skeleton),
        );
    }
    for (e, spec, t) in &ropes {
        let resolve = |other: Entity| all_transforms.get(other).ok().copied();
        report(
            &mut commands,
            e,
            "Rope",
            rope::build_rope(spec, e, t, resolve),
        );
    }
    for (e, spec, t) in &bridges {
        let material = engine_render::StandardMaterial::hex(spec.plank_color)
            .with_roughness(spec.plank_roughness);
        let result = rope::build_bridge(spec, e, t, |index, half| {
            commands
                .spawn((
                    Transform::default(),
                    TransformInterpolation::default(),
                    RigidBody::kinematic(),
                    Collider::cuboid(half).with_density(600.0),
                    engine_render::MeshRenderer::new(
                        engine_render::Shape::Cuboid {
                            size: (half * 2.0).to_array(),
                        },
                        material,
                    ),
                    BridgePlank { bridge: e, index },
                    ChildOf(e),
                ))
                .id()
        });
        report(&mut commands, e, "RopeBridge", result);
    }
    for (e, spec, t) in &volumes {
        report(&mut commands, e, "SoftVolume", volume::build(spec, t));
    }
    for (e, spec) in &grooms {
        let skeleton = find_skeleton(e, &parents, &skeletons);
        if skeleton.is_some_and(|s| s.2.bones.is_empty()) {
            continue;
        }
        report(&mut commands, e, "HairGroom", hair::build(spec, skeleton));
    }
}

/// When a spec changes (data edit, AI patch), throw the simulation away so it rebuilds.
#[allow(clippy::type_complexity)]
pub fn rebuild_changed(
    mut commands: Commands,
    changed: Query<
        (Entity, Option<&SoftBodySim>),
        (
            Or<(
                Changed<Cloth>,
                Changed<Rope>,
                Changed<RopeBridge>,
                Changed<SoftVolume>,
                Changed<HairGroom>,
            )>,
            Or<(With<SoftBodySim>, With<SoftBodyFailed>)>,
        ),
    >,
    added: Query<
        (),
        Or<(
            Added<Cloth>,
            Added<Rope>,
            Added<RopeBridge>,
            Added<SoftVolume>,
            Added<HairGroom>,
        )>,
    >,
) {
    for (e, body) in &changed {
        if added.contains(e) {
            continue;
        }
        if let Some(bridge) = body.and_then(|b| b.bridge.as_ref()) {
            for (plank, _, _) in &bridge.planks {
                commands.entity(*plank).despawn();
            }
        }
        commands
            .entity(e)
            .remove::<SoftBodySim>()
            .remove::<SoftBodyFailed>();
    }
}

/// Bullets, explosions and fire.
pub fn react_to_events(
    mut traces: MessageReader<ProjectileTrace>,
    mut impulses: MessageReader<WorldImpulse>,
    mut ignites: MessageReader<IgniteRequest>,
    mut bodies: Query<&mut SoftBodySim>,
) {
    for t in traces.read() {
        for mut body in &mut bodies {
            let reach = body.radius as f64 + t.radius as f64 + 0.1;
            if point_segment_distance(body.center, t.from, t.to) > reach {
                continue;
            }
            projectile(&mut body, t);
        }
    }
    for i in impulses.read() {
        for mut body in &mut bodies {
            let d = (body.center - i.center).length() - body.radius as f64;
            if d > i.radius as f64 {
                continue;
            }
            let center = body.local(i.center);
            let (radius, k) = (i.radius.max(0.1), push_factor(body.kind));
            body.sim.add_velocity(|p| {
                let off = p - center;
                let dist = off.length();
                off.normalize_or(Vec3::Y) * (i.strength * k * (1.0 - dist / radius).max(0.0))
            });
            let b = &mut *body;
            if i.heat_c > 300.0
                && let Some(c) = b.cloth.as_mut()
            {
                c.ignite(&b.sim, center, i.radius * 0.5);
            }
        }
    }
    for r in ignites.read() {
        for mut body in &mut bodies {
            if body.cloth.is_none()
                || (body.center - r.position).length() > (body.radius + r.radius) as f64
            {
                continue;
            }
            let at = body.local(r.position);
            let b = &mut *body;
            if let Some(c) = b.cloth.as_mut() {
                c.ignite(&b.sim, at, r.radius.max(0.05));
            }
        }
    }
}

/// How much of the air's velocity a body picks up from a blast.
fn push_factor(kind: SoftKind) -> f32 {
    match kind {
        SoftKind::Cloth | SoftKind::Hair => 0.6,
        SoftKind::Rope | SoftKind::Bridge => 0.3,
        SoftKind::Volume => 0.15,
    }
}

fn point_segment_distance(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

/// Apply one projectile to one body.
pub fn projectile(body: &mut SoftBodySim, t: &ProjectileTrace) {
    let from = body.local(t.from);
    let to = body.local(t.to);
    let dir = (to - from).normalize_or_zero();
    let b = body;
    match b.kind {
        SoftKind::Cloth => {
            if let (Some(c), Some(s)) = (b.cloth.as_mut(), b.surface.as_mut())
                && !c
                    .pierce(&mut b.sim, s, from, to, t.radius, t.speed)
                    .is_empty()
            {
                b.topology_version += 1;
            }
        }
        SoftKind::Rope | SoftKind::Bridge => {
            if let Some(state) = b.bridge.as_mut()
                && state.lines.cuttable
            {
                let hit = state.lines.segments_near(&b.sim, from, to, t.radius);
                if state.lines.cut(&mut b.sim, &hit) {
                    b.tubes = state.lines.tubes();
                    b.topology_version += 1;
                }
            }
        }
        SoftKind::Volume | SoftKind::Hair => {
            // A shove along the bullet near its path.
            let reach = 0.25 + t.radius * 4.0;
            let speed = (t.speed * 0.01).min(6.0);
            let (a, ab) = (from, to - from);
            b.sim.add_velocity(|p| {
                let s = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
                let d = (a + ab * s - p).length();
                dir * speed * (1.0 - d / reach).max(0.0)
            });
        }
    }
}

/// Everything a body's step reads.
struct StepContext<'a, 'w, 's> {
    time: &'a Time,
    config: &'a SoftBodyConfig,
    colliders: &'a ColliderWorld,
    env: &'a Env<'w>,
    transforms: &'a Query<
        'w,
        's,
        (
            &'static GlobalTransform,
            Option<&'static Transform>,
            Option<&'static ChildOf>,
        ),
    >,
    poses: &'a Query<'w, 's, &'static SkeletonPose>,
    scales: &'a Query<'w, 's, &'static LocalTimeScale>,
    loads: &'a [(DVec3, f32, Option<Entity>)],
}

/// Step every soft body (in parallel).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn step_bodies(
    time: Res<Time>,
    config: Res<SoftBodyConfig>,
    colliders: Res<ColliderWorld>,
    loads: Res<LoadList>,
    env: Env,
    transforms: Query<(
        &'static GlobalTransform,
        Option<&'static Transform>,
        Option<&'static ChildOf>,
    )>,
    poses: Query<&'static SkeletonPose>,
    scales: Query<&'static LocalTimeScale>,
    mut bodies: Query<(Entity, &mut SoftBodySim)>,
    mut diagnostics: ResMut<Diagnostics>,
) {
    if !config.enabled {
        // Hold still (drawing interpolates between the last two steps).
        for (_, mut b) in &mut bodies {
            let sim = &mut b.sim;
            sim.step_start.clone_from(&sim.pos);
        }
        return;
    }
    let ctx = StepContext {
        time: &time,
        config: &config,
        colliders: &colliders,
        env: &env,
        transforms: &transforms,
        poses: &poses,
        scales: &scales,
        loads: &loads.0,
    };
    let start = Instant::now();
    bodies.par_iter_mut().for_each(|(e, mut body)| {
        step_one(e, &mut body, &ctx);
    });
    let mut particles = 0;
    let mut count = 0;
    let mut frozen = 0;
    for (_, b) in &bodies {
        particles += b.particle_count();
        count += 1;
        frozen += b.frozen as usize;
    }
    diagnostics.set("softbody.step_ms", start.elapsed().as_secs_f64() * 1000.0);
    diagnostics.set("softbody.bodies", count as f64);
    diagnostics.set("softbody.particles", particles as f64);
    diagnostics.set("softbody.frozen", frozen as f64);
}

fn step_one(entity: Entity, body: &mut SoftBodySim, ctx: &StepContext) {
    let started = Instant::now();
    let b = body;
    b.out = StepOutput::default();
    // Level of detail.
    let focus = &ctx.env.focus;
    let dist = if focus.points.is_empty() {
        0.0
    } else {
        (focus.distance(b.center) - b.radius as f64).max(0.0) as f32
    };
    let owner = b.time_owner.unwrap_or(entity);
    let mut dt = ctx.time.delta_for(ctx.scales.get(owner).ok());
    if dist > ctx.config.freeze_distance {
        b.frozen = true;
        b.sim.step_start.clone_from(&b.sim.pos);
        return;
    }
    b.frozen = false;
    if dist > ctx.config.lod_distance {
        b.lod_debt += dt;
        if b.steps.is_multiple_of(2) {
            b.steps += 1;
            b.sim.step_start.clone_from(&b.sim.pos);
            return;
        }
        dt = b.lod_debt;
    }
    b.lod_debt = 0.0;

    // Keep coordinates small.
    if (b.center - b.sim.origin).length() > 50.0 {
        b.sim.rebase(b.center);
    }
    let origin = b.sim.origin;

    // Frames.
    for (i, src) in b.frames.iter().enumerate() {
        let pose = match *src {
            FrameSource::World => None,
            FrameSource::Entity(e) => {
                world_transform(e, ctx.transforms).map(|t| (t.translation(), t.rotation()))
            }
            FrameSource::Bone { owner, bone } => world_transform(owner, ctx.transforms)
                .zip(ctx.poses.get(owner).ok())
                .and_then(|(t, p)| p.bone_world(&t, bone)),
        };
        if let Some((p, r)) = pose {
            let local = (p - origin).as_vec3();
            if b.steps == 0 {
                b.sim.snap_frame(i as u16, local, r);
            } else {
                b.sim.set_frame(i as u16, local, r);
            }
        }
    }

    // Environment.
    let s = b.settings;
    let env = ctx.env;
    let p = &mut b.sim.params;
    p.substeps = ((s.substeps as f32 * ctx.config.quality).round() as u32).max(1);
    p.gravity = env.gravity(b.center) * s.gravity_scale;
    b.wind = env.wind(b.center) * s.wind_scale;
    p.wind = b.wind;
    p.damping = s.damping;
    p.friction = s.friction;
    p.max_speed = s.max_speed;
    let up = env.up(b.center);
    b.sim.ground.clear();
    if b.radius > 6.0 {
        for i in 0..b.sim.len() {
            let wp = origin + b.sim.pos[i].as_dvec3();
            b.sim.ground.push(ground_plane(env, wp, origin));
        }
    } else {
        b.sim.ground.push(ground_plane(env, b.center, origin));
    }

    // Colliders near the body.
    let mut local = std::mem::take(&mut b.scratch_colliders);
    local.clear();
    if s.collide {
        let reach = b.radius as f64 + s.max_speed as f64 * dt as f64 + 0.5;
        for (id, c) in ctx.colliders.list.iter().enumerate() {
            if (c.pos - b.center).length() > reach + c.bound() || b.ignore.contains(&c.entity) {
                continue;
            }
            local.push(LocalCollider {
                shape: c.shape,
                from_pos: (c.prev_pos - origin).as_vec3(),
                from_rot: c.prev_rot,
                to_pos: (c.pos - origin).as_vec3(),
                to_rot: c.rot,
                friction: c.friction,
                id: id as u32,
                dynamic: c.body.is_some(),
            });
        }
    }

    // Weight on bridge planks.
    if let Some(bridge) = b.bridge.as_ref()
        && !bridge.planks.is_empty()
    {
        let g = b.sim.params.gravity;
        for (feet, mass, ground) in ctx.loads {
            let hit = ground
                .and_then(|g| bridge.planks.iter().position(|p| p.0 == g))
                .map(|k| (k, 0.5))
                .or_else(|| bridge.plank_under(*feet));
            if let Some((k, across)) = hit {
                let (_, l, r) = bridge.planks[k];
                b.sim.loads.push((l, g * (mass * (1.0 - across))));
                b.sim.loads.push((r, g * (mass * across)));
            }
        }
    }

    // Wetness (cloth soaks up rain and dries slowly).
    if let Some(c) = b.cloth.as_mut() {
        let rain = if env.weather.is_raining() {
            env.weather.precipitation_mm_h
        } else {
            0.0
        };
        c.wetness = (c.wetness + dt * (rain * 0.004 - 0.003)).clamp(0.0, 1.0);
        b.sim.set_mass_scale(1.0 + 2.0 * c.wetness);
    }

    // Step (plus prewarm on the first update).
    let runs = 1 + std::mem::take(&mut b.pending_prewarm);
    let mut impulses: Vec<ColliderImpulse> = Vec::new();
    let mut pulls: Vec<ColliderImpulse> = vec![ColliderImpulse::default(); b.frames.len()];
    for run in 0..runs {
        let step_dt = if run == 0 { dt } else { 1.0 / 60.0 };
        b.sim.step(step_dt, &local);
        if run == 0 {
            impulses.clone_from(&b.sim.collider_impulses);
            pulls.clone_from(&b.sim.frame_impulses);
        }
    }
    if runs > 1 {
        b.sim.step_start.clone_from(&b.sim.pos);
    }
    b.steps += 1;

    // Cloth: tearing and fire.
    let mut changed = false;
    if let (Some(c), Some(surface)) = (b.cloth.as_mut(), b.surface.as_mut()) {
        changed |= c.tear_overstretched(&mut b.sim, surface);
        if c.flammable && b.steps.is_multiple_of(15) {
            let heat = &env.heat;
            let mut hot = Vec::new();
            for (t, tri) in c.triangles.iter().enumerate().step_by(3) {
                if !c.alive[t] || c.burn[t] > 0.0 {
                    continue;
                }
                let wp = origin + b.sim.pos[tri[0] as usize].as_dvec3();
                if heat.sample(wp) > 250.0 {
                    hot.push(b.sim.pos[tri[0] as usize]);
                }
            }
            for h in hot {
                c.ignite(&b.sim, h, c.cell * 1.5);
            }
        }
        let lost_before = c.lost;
        let smoke = c.update_burn(dt, b.steps, &mut b.sim, surface, up);
        changed |= c.lost != lost_before;
        b.out.smoke = smoke.into_iter().map(|p| origin + p.as_dvec3()).collect();
    }
    if changed {
        b.topology_version += 1;
    }

    // Bridge planks follow the deck.
    let rope_radius = b.tubes.first().map(|t| t.radius).unwrap_or(0.02);
    if let Some(bridge) = b.bridge.as_mut()
        && !bridge.planks.is_empty()
    {
        bridge.plank_poses = (0..bridge.planks.len())
            .map(|k| {
                let (c, q) = bridge.plank_pose(&b.sim, k, rope_radius);
                (origin + c.as_dvec3(), q)
            })
            .collect();
    }

    // Outputs.
    for (k, imp) in impulses.iter().enumerate() {
        if imp.linear != Vec3::ZERO {
            b.out.impulses.push((local[k].id, *imp));
        }
    }
    for (k, imp) in pulls.iter().enumerate() {
        if imp.linear == Vec3::ZERO {
            continue;
        }
        if let FrameSource::Entity(e) = b.frames[k] {
            let f = b.sim.frames[k];
            b.out.pulls.push((e, *imp, origin + f.to_pos.as_dvec3()));
        }
    }
    let (speed, at) = b.sim.ground_impact;
    if b.kind == SoftKind::Volume && speed > 1.5 {
        b.out.impact = Some((speed, origin + at.as_dvec3()));
    }
    b.update_bounds();
    b.scratch_colliders = local;
    b.step_ms = started.elapsed().as_secs_f32() * 1000.0;
}

fn ground_plane(env: &Env, at: DVec3, origin: DVec3) -> [f32; 4] {
    match env.ground(at) {
        Some(g) => {
            let n = g.normal.normalize_or(Vec3::Y);
            let q = (DVec3::new(at.x, g.height, at.z) - origin).as_vec3();
            [n.x, n.y, n.z, -n.dot(q)]
        }
        // No ground: a plane far below.
        None => [0.0, 1.0, 0.0, 1e6],
    }
}

/// Send reaction impulses, smoke, landings and noise to the world.
#[allow(clippy::too_many_arguments)]
pub fn emit_outputs(
    bodies: Query<&SoftBodySim>,
    colliders: Res<ColliderWorld>,
    rigid: Query<(&RigidBody, &GlobalTransform)>,
    mut apply: MessageWriter<ApplyImpulse>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut contacts: MessageWriter<GroundContact>,
    mut noise: MessageWriter<NoiseEvent>,
    mut sums: Local<HashMap<Entity, (Vec3, Vec3)>>,
) {
    sums.clear();
    for body in &bodies {
        for (id, imp) in &body.out.impulses {
            let Some(c) = colliders.list.get(*id as usize) else {
                continue;
            };
            let Some(target) = c.body else { continue };
            let angular = imp.angular + (c.pos - c.com).as_vec3().cross(imp.linear);
            let s = sums.entry(target).or_default();
            s.0 += imp.linear;
            s.1 += angular;
        }
        for (e, imp, frame_pos) in &body.out.pulls {
            let Ok((rb, t)) = rigid.get(*e) else { continue };
            if rb.kind != BodyKind::Dynamic {
                continue;
            }
            let angular = imp.angular + (*frame_pos - t.translation()).as_vec3().cross(imp.linear);
            let s = sums.entry(*e).or_default();
            s.0 += imp.linear;
            s.1 += angular;
        }
        for p in &body.out.smoke {
            smoke.write(SmokeEmission {
                position: *p,
                velocity: Vec3::Y * 0.8,
                radius: 0.15,
                mass_kg: 0.002,
                temperature_c: 300.0,
                darkness: 0.6,
            });
        }
        if let Some((speed, at)) = body.out.impact {
            let mass = body
                .sim
                .base_inv_mass
                .iter()
                .map(|w| if *w > 0.0 { 1.0 / w } else { 0.0 })
                .sum::<f32>();
            contacts.write(GroundContact {
                position: at,
                normal: Vec3::Y,
                size: [body.radius * 1.4; 2],
                yaw: 0.0,
                load_kg: mass,
                speed,
                kind: ContactKind::Body,
                surface: None,
                entity: None,
            });
            noise.write(NoiseEvent {
                position: at,
                radius: (speed * 3.0).min(30.0),
                source: None,
            });
        }
    }
    for (e, (linear, angular)) in sums.drain() {
        apply.write(ApplyImpulse {
            entity: e,
            impulse: linear,
            at: None,
            angular,
        });
    }
}

/// Move bridge planks to where the deck is.
pub fn drive_planks(
    bodies: Query<(&SoftBodySim, &GlobalTransform)>,
    mut planks: Query<&mut Transform, With<BridgePlank>>,
) {
    for (body, bridge_t) in &bodies {
        let Some(bridge) = body.bridge.as_ref() else {
            continue;
        };
        let inv_rot = bridge_t.rotation().conjugate();
        let inv_scale = bridge_t.scale().recip();
        for ((plank, _, _), (pos, rot)) in bridge.planks.iter().zip(&bridge.plank_poses) {
            if let Ok(mut t) = planks.get_mut(*plank) {
                let local = inv_rot * (*pos - bridge_t.translation()).as_vec3() * inv_scale;
                t.translation = local.as_dvec3();
                t.rotation = inv_rot * *rot;
            }
        }
    }
}
