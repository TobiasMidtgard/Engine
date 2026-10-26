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
    ContactKind, Env, GroundContact, IgniteRequest, NoiseEvent, ProjectileTrace, Scalability,
    SimulationRange, SmokeEmission, SurfaceMaterials, WaterDisturbance, WorldImpulse, WorldShift,
};

use crate::body::{FrameSource, SoftBodySim, SoftKind};
use crate::character::BoneColliders;
use crate::collide::{LocalCollider, LocalShape};
use crate::field::{BAKE_REACH, FieldCache};
use crate::rope::{BridgePlank, SoftLoad};
use crate::settings::SoftBodyConfig;
use crate::solver::{ColliderImpulse, Sim, WaterSurface};
use crate::{Cloth, HairGroom, Rope, RopeBridge, SoftVolume, cloth, hair, rope, volume};

/// A frame turning further than this in one step (radians) has been teleported, not turned.
const TELEPORT_TURN: f32 = 0.8;

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
    /// Where it pushed the water's surface this step: point on the surface, velocity
    /// relative to the water, radius and mass (for `WaterDisturbance`).
    pub water: Vec<(DVec3, Vec3, f32, f32)>,
}

/// A collider as every soft body sees it this step.
#[derive(Debug, Clone)]
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
    /// Mesh and height-grid colliders baked for the solver.
    pub fields: FieldCache,
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

/// World transform from `Transform`s, fresh this step even before propagation: a root's own,
/// a child's composed onto its parent's (so cloth on a boat's mast keeps up with the boat
/// physics just moved). Falls back to `GlobalTransform` past a few levels.
fn world_transform(
    e: Entity,
    q: &Query<(&GlobalTransform, Option<&Transform>, Option<&ChildOf>)>,
) -> Option<GlobalTransform> {
    fresh_transform(e, q, 0)
}

fn fresh_transform(
    e: Entity,
    q: &Query<(&GlobalTransform, Option<&Transform>, Option<&ChildOf>)>,
    depth: u32,
) -> Option<GlobalTransform> {
    let (g, t, parent) = q.get(e).ok()?;
    Some(match (t, parent) {
        (Some(t), None) => GlobalTransform::from(*t),
        (Some(t), Some(parent)) if depth < 6 => fresh_transform(parent.0, q, depth + 1)
            .map(|p| p.mul_transform(t))
            .unwrap_or(*g),
        _ => *g,
    })
}

/// The dynamic rigid body that carries `e`: `e` itself or its nearest ancestor with one.
fn find_carrier(e: Entity, parents: &Query<&ChildOf>, rigid: &Query<&RigidBody>) -> Option<Entity> {
    let mut cursor = Some(e);
    for _ in 0..8 {
        let c = cursor?;
        if let Ok(rb) = rigid.get(c) {
            return (rb.kind == BodyKind::Dynamic).then_some(c);
        }
        cursor = parents.get(c).ok().map(|p| p.0);
    }
    None
}

/// The solver's shapes for a physics collider shape, posed in the collider's space. `grid`
/// answers meshes and height grids (with the part's scale, offset and rotation), or `None`
/// while they aren't baked.
fn convert_shape(
    shape: &ColliderShape,
    scale: Vec3,
    offset: Vec3,
    rot: Quat,
    out: &mut Vec<(LocalShape, Vec3, Quat)>,
    grid: &mut dyn FnMut(&ColliderShape, Vec3, Vec3, Quat) -> Option<LocalShape>,
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
                convert_shape(part, scale, offset + rot * (*o * s), rot * *r, out, grid);
            }
        }
        ColliderShape::TriMesh { .. } | ColliderShape::Heightfield { .. } => {
            if let Some(g) = grid(shape, scale, offset, rot) {
                out.push((g, offset, rot));
            }
        }
    }
}

/// How far a collider's shape reaches from its origin (unscaled), m.
fn shape_reach(shape: &ColliderShape) -> f32 {
    let farthest = |points: &mut dyn Iterator<Item = Vec3>| {
        points
            .map(|p| p.length_squared())
            .fold(0.0f32, f32::max)
            .sqrt()
    };
    match shape {
        ColliderShape::Sphere { radius } => *radius,
        ColliderShape::Cuboid { half_extents } => half_extents.length(),
        ColliderShape::Capsule {
            radius,
            half_height,
        }
        | ColliderShape::Cylinder {
            radius,
            half_height,
        } => radius + half_height,
        ColliderShape::ConvexHull { points } => farthest(&mut points.iter().copied()),
        ColliderShape::TriMesh { vertices, .. } => farthest(&mut vertices.iter().copied()),
        ColliderShape::Heightfield { heights, size, .. } => {
            let tall = heights.iter().fold(0.0f32, |m, h| m.max(h.abs()));
            Vec3::new(size[0] * 0.5, tall, size[1] * 0.5).length()
        }
        ColliderShape::Compound(parts) => parts
            .iter()
            .map(|(o, _, part)| o.length() + shape_reach(part))
            .fold(0.0, f32::max),
    }
}

/// Collect physics colliders (meshes and height grids baked once a soft body comes near),
/// character capsules and bone capsules.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn gather_colliders(
    mut world: ResMut<ColliderWorld>,
    colliders: Query<(Entity, Ref<Collider>, Option<&ChildOf>)>,
    transforms: Query<(&GlobalTransform, Option<&Transform>, Option<&ChildOf>)>,
    rigid: Query<(&RigidBody, Option<&MassProperties>)>,
    characters: Query<(Entity, &CharacterController), Without<BoneColliders>>,
    skinned: Query<(Entity, &BoneColliders, &SkeletonPose)>,
    materials: Res<SurfaceMaterials>,
    bodies: Query<&SoftBodySim>,
    mut shapes: Local<Vec<(LocalShape, Vec3, Quat)>>,
    mut bone_shapes: Local<Vec<(LocalShape, DVec3, Quat)>>,
    mut reaches: Local<HashMap<Entity, f32>>,
) {
    let world = &mut *world;
    world.list.clear();
    world.seen.clear();
    // Only colliders that can reach a body that's awake: resting bodies don't collide, and
    // the rest of the world's colliders aren't converted at all.
    let near_bodies: Vec<(DVec3, f64)> = bodies
        .iter()
        .filter(|b| !b.frozen)
        .map(|b| (b.center, b.radius as f64 + BAKE_REACH))
        .collect();
    for (entity, collider, parent) in &colliders {
        if collider.sensor || near_bodies.is_empty() {
            continue;
        }
        let Some(t) = world_transform(entity, &transforms) else {
            continue;
        };
        let reach = match reaches.get(&entity) {
            Some(r) if !collider.is_changed() => *r,
            _ => {
                let r = shape_reach(&collider.shape);
                reaches.insert(entity, r);
                r
            }
        } as f64
            * t.scale().max_element() as f64;
        if !near_bodies
            .iter()
            .any(|(b, body_reach)| (t.translation() - *b).length() < reach + body_reach)
        {
            continue;
        }
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
        let rot = t.rotation();
        let changed = collider.is_changed();
        let fields = &mut world.fields;
        let mut part = 0;
        shapes.clear();
        convert_shape(
            &collider.shape,
            t.scale(),
            Vec3::ZERO,
            Quat::IDENTITY,
            &mut shapes,
            &mut |shape, scale, offset, r| {
                part += 1;
                fields.shape((entity, part - 1), shape, scale, changed, |c, radius| {
                    let at = t.translation() + (rot * (offset + r * c)).as_dvec3();
                    near_bodies
                        .iter()
                        .any(|(b, reach)| (at - *b).length() < radius as f64 + reach)
                })
            },
        );
        for (k, (shape, offset, r)) in shapes.drain(..).enumerate() {
            world.push(
                (entity, k as u32),
                WorldCollider {
                    shape,
                    pos: t.translation() + (rot * offset).as_dvec3(),
                    rot: rot * r,
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
        for (k, (shape, pos, rot)) in bone_shapes.drain(..).enumerate() {
            world.push(
                (entity, 20_000 + k as u32),
                WorldCollider {
                    shape,
                    pos,
                    rot,
                    prev_pos: DVec3::ZERO,
                    prev_rot: Quat::IDENTITY,
                    friction: bones.friction,
                    entity,
                    body: None,
                    com: pos,
                },
            );
        }
    }
    world.fields.sweep();
    std::mem::swap(&mut world.prev, &mut world.seen);
}

/// `PreUpdate`: when the game's [`Scalability`] changes, substeps follow its effects quality
/// and the half-rate distance its view distance (how far `SimulationRange` reaches follows
/// it already).
pub fn follow_scalability(
    scalability: Option<Res<Scalability>>,
    mut config: ResMut<SoftBodyConfig>,
) {
    let Some(s) = scalability.filter(|s| s.is_changed()) else {
        return;
    };
    config.quality = s.effects.clamp(0.5, 1.5);
    config.lod_distance = SoftBodyConfig::default().lod_distance * s.view_distance.max(0.25);
}

/// Move every body, the collider history (for sweeps) and the gathered loads into the new
/// frame when the world is re-anchored. Runs in `WorldShiftSet::Apply`.
pub fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut bodies: Query<&mut SoftBodySim>,
    mut colliders: ResMut<ColliderWorld>,
    mut loads: ResMut<LoadList>,
    mut ropes: Query<&mut Rope>,
) {
    let Some(s) = shifts.read().copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    // World-anchored rope ends move with the world (without counting as a spec change, which
    // would rebuild the rope), so a later rebuild ties them in the right place.
    for mut rope in &mut ropes {
        let rope = rope.bypass_change_detection();
        for end in [&mut rope.end_a, &mut rope.end_b] {
            if let rope::RopeEnd::World { at } = end {
                *at = s.point(DVec3::from_array(*at)).to_array();
            }
        }
    }
    for mut b in &mut bodies {
        let b = &mut *b;
        b.sim.shift(&s);
        b.center = s.point(b.center);
        b.wind = s.direction(b.wind);
        if let Some(bridge) = b.bridge.as_mut() {
            for (p, r) in &mut bridge.plank_poses {
                (*p, *r) = (s.point(*p), s.orientation(*r));
            }
        }
    }
    let colliders = &mut *colliders;
    for (p, r) in colliders
        .prev
        .values_mut()
        .chain(colliders.seen.values_mut())
    {
        (*p, *r) = (s.point(*p), s.orientation(*r));
    }
    for c in &mut colliders.list {
        (c.pos, c.rot) = (s.point(c.pos), s.orientation(c.rot));
        (c.prev_pos, c.prev_rot) = (s.point(c.prev_pos), s.orientation(c.prev_rot));
        c.com = s.point(c.com);
    }
    for (p, _, _) in &mut loads.0 {
        *p = s.point(*p);
    }
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
    rigid: Query<&RigidBody>,
    skeletons: Query<(&GlobalTransform, &SkeletonPose)>,
    all_transforms: Query<&GlobalTransform>,
    meshes: Option<Res<engine_assets::Assets<engine_render::Mesh>>>,
    skins: Option<Res<hair::MeshSkins>>,
) {
    for (e, spec, t) in &cloths {
        if crate::gpu_cloth::runs_on_gpu(spec) {
            continue; // simulated and drawn by the render feature
        }
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
            cloth::build(
                spec,
                e,
                t,
                skeleton,
                find_carrier(e, &parents, &rigid).is_some(),
            ),
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
        let material = match &spec.plank_material {
            Some(key) => engine_render::MaterialRef::Asset(key.clone()),
            None => engine_render::MaterialRef::Inline(
                engine_render::StandardMaterial::hex(spec.plank_color)
                    .with_roughness(spec.plank_roughness),
            ),
        };
        let result = rope::build_bridge(spec, e, t, |index, half| {
            let plank = commands
                .spawn((
                    Transform::default(),
                    TransformInterpolation::default(),
                    RigidBody::kinematic(),
                    Collider::cuboid(half).with_density(600.0),
                    BridgePlank { bridge: e, index },
                    ChildOf(e),
                ))
                .id();
            match &spec.plank_mesh {
                // A unit mesh on a scaled child, so the collider keeps its true size.
                Some(key) => {
                    commands.spawn((
                        Transform::from_scale(half * 2.0),
                        engine_render::MeshRenderer::new(
                            engine_render::MeshRef::Asset(key.clone()),
                            material.clone(),
                        ),
                        ChildOf(plank),
                    ));
                }
                None => {
                    commands
                        .entity(plank)
                        .insert(engine_render::MeshRenderer::new(
                            engine_render::Shape::Cuboid {
                                size: (half * 2.0).to_array(),
                            },
                            material.clone(),
                        ));
                }
            }
            plank
        });
        report(&mut commands, e, "RopeBridge", result);
    }
    for (e, spec, t) in &volumes {
        report(&mut commands, e, "SoftVolume", volume::build(spec, t));
    }
    let meshes = meshes.as_deref();
    for (e, spec) in &grooms {
        let skeleton = find_skeleton(e, &parents, &skeletons);
        if skeleton.is_some_and(|s| s.2.bones.is_empty()) || spec.waiting_for_mesh(meshes) {
            continue; // the pose isn't evaluated (or the body mesh isn't built) yet
        }
        report(
            &mut commands,
            e,
            "HairGroom",
            hair::build(spec, skeleton, meshes, skins.as_deref()),
        );
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
    range: SimulationRange,
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
    range: Option<Res<SimulationRange>>,
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
        range: range.map_or_else(SimulationRange::default, |r| *r),
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
    // Beyond the simulation range a body rests in its last pose (waking again a little nearer
    // than it fell asleep, so one at the edge doesn't flicker), and costs nothing.
    let range = &ctx.range;
    let resting = if b.frozen {
        !range.is_active_sphere(focus, b.center, b.radius as f64)
    } else {
        !focus.points.is_empty() && dist as f64 > range.active_m * (1.0 + range.hysteresis)
    };
    if resting || dist > ctx.config.freeze_distance {
        if !b.frozen {
            b.frozen = true;
            b.sim.step_start.clone_from(&b.sim.pos);
        }
        return;
    }
    b.frozen = false;
    let far = dist > ctx.config.lod_distance;
    if far {
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
    let poses: Vec<Option<(Vec3, Quat)>> = b
        .frames
        .iter()
        .map(|src| {
            let pose = match *src {
                FrameSource::World => None,
                FrameSource::Entity(e) => {
                    world_transform(e, ctx.transforms).map(|t| (t.translation(), t.rotation()))
                }
                FrameSource::Bone { owner, bone } => world_transform(owner, ctx.transforms)
                    .zip(ctx.poses.get(owner).ok())
                    .and_then(|(t, p)| p.bone_world(&t, bone)),
                FrameSource::Blend(blend) => world_transform(blend.owner, ctx.transforms)
                    .zip(ctx.poses.get(blend.owner).ok())
                    .and_then(|(t, p)| blend.world(&t, p)),
            };
            pose.map(|(p, r)| ((p - origin).as_vec3(), r))
        })
        .collect();
    // A frame that jumps further in one step than anything moves (a respawn, a boat sent
    // back to its mooring) carries the whole body with it instead of dragging it across.
    let jump = (5.0f32).max(2.0 * b.radius);
    let teleport = (b.steps > 0)
        .then(|| {
            poses.iter().enumerate().find_map(|(i, pose)| {
                let (p, r) = (*pose)?;
                let f = &b.sim.frames[i];
                ((p - f.to_pos).length() > jump || r.angle_between(f.to_rot) > TELEPORT_TURN)
                    .then_some((i as u16, p, r))
            })
        })
        .flatten();
    if let Some((i, p, r)) = teleport {
        b.sim.carry(i, p, r);
    }
    for (i, pose) in poses.iter().enumerate() {
        if let Some((p, r)) = *pose {
            if b.steps == 0 || teleport.is_some() {
                b.sim.snap_frame(i as u16, p, r);
            } else {
                b.sim.set_frame(i as u16, p, r);
            }
        }
    }

    // Environment.
    let s = b.settings;
    let env = ctx.env;
    let p = &mut b.sim.params;
    // Far bodies step every other time over both steps' time, with the substeps of one: half
    // the work, a little softer where it's hard to see.
    let lod = if far { 0.5 } else { 1.0 };
    p.substeps = ((s.substeps as f32 * ctx.config.quality * lod).round() as u32).max(1);
    p.gravity = env.gravity(b.center) * s.gravity_scale;
    b.wind = env.wind(b.center) * s.wind_scale;
    p.wind = b.wind;
    p.damping = s.damping;
    p.friction = s.friction;
    p.max_speed = s.max_speed;
    p.buoyancy = s.buoyancy;
    p.water_drag = s.water_drag;
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
                shape: c.shape.clone(),
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

    // Water around the body, and how wet it gets.
    b.sim.water = water_surface(env, b.center, b.radius, origin);
    let rain = if env.weather.is_raining() {
        env.weather.precipitation_mm_h
    } else {
        0.0
    };
    let warmth = drying_warmth(env.temperature(b.center));
    soak(b, dt, rain, warmth);

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
    b.out.water = surface_pushes(&b.sim, origin);
    b.update_bounds();
    b.scratch_colliders = local;
    b.step_ms = started.elapsed().as_secs_f32() * 1000.0;
}

/// Patches of the water's surface a body's pushes are gathered into, m across.
const PUSH_CELL: f32 = 0.6;
/// Pushes sent per body per step, at most (the strongest).
const MAX_PUSHES: usize = 6;
/// Slower than this through the water (m/s), a particle makes no rings: cloth drifting with
/// the current or lying still on a pond leaves the surface alone.
const PUSH_SPEED: f32 = 0.12;

/// What crosses or moves along the water's surface, gathered into a few patches: where on
/// the surface (world), velocity relative to the water, radius and mass.
fn surface_pushes(sim: &Sim, origin: DVec3) -> Vec<(DVec3, Vec3, f32, f32)> {
    let Some(water) = sim.water else {
        return Vec::new();
    };
    let band = 2.0 * sim.params.radius.max(0.01);
    // Per patch: weighted position, velocity, mass, and the push (mass × speed).
    let mut cells: HashMap<(i32, i32), (Vec3, Vec3, f32, f32)> = HashMap::new();
    for (i, (&x, &v)) in sim.pos.iter().zip(&sim.vel).enumerate() {
        let w = sim.inv_mass[i];
        if w <= 0.0 || water.depth(x).abs() > band {
            continue;
        }
        let rel = v - water.current;
        let speed = rel.length();
        if speed < PUSH_SPEED {
            continue;
        }
        let m = 1.0 / w;
        let key = (
            (x.x / PUSH_CELL).floor() as i32,
            (x.z / PUSH_CELL).floor() as i32,
        );
        let c = cells.entry(key).or_default();
        c.0 += x * m;
        c.1 += rel * m;
        c.2 += m;
        c.3 += m * speed;
    }
    let mut pushes: Vec<_> = cells.into_values().collect();
    pushes.sort_by(|a, b| b.3.total_cmp(&a.3));
    pushes.truncate(MAX_PUSHES);
    pushes
        .into_iter()
        .map(|(x, v, m, _)| {
            let at = x / m;
            let on = Vec3::new(at.x, water.height(at), at.z);
            (origin + on.as_dvec3(), v / m, PUSH_CELL * 0.5, m)
        })
        .collect()
}

/// Seconds for a particle fully under water to soak through.
const SOAK_SECS: f32 = 4.0;
/// How fast wetness spreads along the fabric or strand (wicking), m²/s: a few centimeters past
/// the waterline in the first seconds.
const WICK: f32 = 5e-4;
/// How fast water runs down a hanging wet body, per second, on a vertical link.
const DRAIN: f32 = 0.02;
/// Wetness lost per second in still air at 15 °C.
const DRY: f32 = 0.003;

/// Water shallower than this (m) doesn't count: a film running down a stream's wall reports
/// a surface as high as the wall, and the surface over a body would slope up to meet it.
const MIN_WATER_DEPTH: f32 = 0.05;

/// The water surface over a body (3 × 3 samples across its bounds) in its local frame, or
/// `None` when no water is near. Where a sample falls on dry land (past a stream's bank), the
/// surface carries on level with the water's mean height there: what is on the bank stands
/// above it anyway.
fn water_surface(env: &Env, center: DVec3, radius: f32, origin: DVec3) -> Option<WaterSurface> {
    let sample = |p: DVec3| {
        env.water_query
            .sample(p)
            .filter(|w| w.depth >= MIN_WATER_DEPTH)
    };
    let mid = sample(center)?;
    if mid.surface_height < center.y - f64::from(radius) - 0.25 {
        return None;
    }
    let r = f64::from(radius.max(0.25));
    let mut heights = [None; 9];
    let mut current = Vec3::ZERO;
    let (mut sum, mut wet) = (0.0, 0);
    for (k, h) in heights.iter_mut().enumerate() {
        let p = center + DVec3::new((k % 3) as f64 - 1.0, 0.0, (k / 3) as f64 - 1.0) * r;
        if let Some(w) = sample(p) {
            let y = (w.surface_height - origin.y) as f32;
            *h = Some(y);
            sum += y;
            current += w.velocity;
            wet += 1;
        }
    }
    let level = sum / wet.max(1) as f32;
    let min = (center - origin).as_vec3() - Vec3::new(r as f32, 0.0, r as f32);
    Some(WaterSurface {
        min: Vec2::new(min.x, min.z),
        cell: Vec2::splat(r as f32),
        heights: heights.map(|h| h.unwrap_or(level)),
        current: current / wet.max(1) as f32,
    })
}

/// How fast cloth dries at an air temperature (°C), as a factor on [`DRY`].
pub(crate) fn drying_warmth(temperature_c: f32) -> f32 {
    ((temperature_c + 5.0) / 20.0).clamp(0.3, 2.0)
}

/// Wetness after `dt` out of the water: rain wets it (`rain_mm_h`), wind (m/s) and warmth
/// ([`drying_warmth`]) dry it.
pub(crate) fn in_the_open(wet: f32, dt: f32, rain_mm_h: f32, wind: f32, warmth: f32) -> f32 {
    let gain = rain_mm_h * 0.004;
    let loss = DRY * (1.0 + wind / 4.0) * warmth;
    (wet + dt * (gain - loss)).clamp(0.0, 1.0)
}

/// Wet what is under water or out in the rain, wick it along the body and dry the rest
/// (faster in wind and warmth); soaked particles weigh more.
fn soak(b: &mut SoftBodySim, dt: f32, rain_mm_h: f32, warmth: f32) {
    let n = b.sim.len();
    b.wet.resize(n, 0.0);
    let soak = b.settings.soak;
    if soak <= 0.0 {
        b.wet.iter_mut().for_each(|w| *w = 0.0);
        return;
    }
    let wind = b.wind.length();
    let r = b.sim.params.radius.max(0.005);
    for i in 0..n {
        // As far as it is in the water (across its thickness, as the solver's lift).
        let under = b.sim.water.map_or(0.0, |w| {
            ((w.depth(b.sim.pos[i]) + r) / (2.0 * r)).clamp(0.0, 1.0)
        });
        let w = &mut b.wet[i];
        *w = if under > 0.0 {
            (*w + dt * under / SOAK_SECS).clamp(0.0, 1.0)
        } else {
            in_the_open(*w, dt, rain_mm_h, wind, warmth)
        };
    }
    if b.wet.iter().any(|&w| w > 0.0) {
        let up = -b.sim.params.gravity.normalize_or(Vec3::NEG_Y);
        for d in &b.sim.distances {
            let (a, c) = (d.a as usize, d.b as usize);
            let rest = d.rest.max(1e-3);
            // Wicking evens it out; water runs down toward the lower end.
            let mut flow = (b.wet[a] - b.wet[c]) * (WICK * dt / (rest * rest)).min(0.1);
            let slope = ((b.sim.pos[a] - b.sim.pos[c]).dot(up) / rest).clamp(-1.0, 1.0);
            flow += DRAIN
                * dt
                * slope
                * if slope > 0.0 {
                    b.wet[a] * (1.0 - b.wet[c])
                } else {
                    b.wet[c] * (1.0 - b.wet[a])
                };
            b.wet[a] -= flow;
            b.wet[c] += flow;
        }
    }
    let wet = &b.wet;
    b.sim.set_mass_scales(|i| 1.0 + soak * wet[i]);
    if let Some(c) = b.cloth.as_mut() {
        c.wetness = wet.iter().sum::<f32>() / n.max(1) as f32;
    }
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
    bodies: Query<(Entity, &SoftBodySim)>,
    colliders: Res<ColliderWorld>,
    rigid: Query<&RigidBody>,
    parents: Query<&ChildOf>,
    poses: Query<(&GlobalTransform, Option<&Transform>, Option<&ChildOf>)>,
    mass: Query<&MassProperties>,
    mut apply: MessageWriter<ApplyImpulse>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut contacts: MessageWriter<GroundContact>,
    mut noise: MessageWriter<NoiseEvent>,
    mut water: MessageWriter<WaterDisturbance>,
    mut sums: Local<HashMap<Entity, (Vec3, Vec3)>>,
) {
    sums.clear();
    for (entity, body) in &bodies {
        for &(position, velocity, radius, mass_kg) in &body.out.water {
            water.write(WaterDisturbance {
                position,
                velocity,
                radius,
                mass_kg,
                source: Some(entity),
            });
        }
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
            // The frame may be a child of the body (a sail on a boat's rig).
            let Some(carrier) = find_carrier(*e, &parents, &rigid) else {
                continue;
            };
            let Some(t) = world_transform(carrier, &poses) else {
                continue;
            };
            let com = mass.get(carrier).map_or(t.translation(), |m| {
                t.transform_point(m.center_of_mass.as_dvec3())
            });
            let angular = imp.angular + (*frame_pos - com).as_vec3().cross(imp.linear);
            let s = sums.entry(carrier).or_default();
            s.0 += imp.linear;
            s.1 += angular;
        }
        // A burning triangle puffs about 1.5 times a second; a whole curtain alight gives
        // a few hundredths of a kilogram of smoke a second (a dozen smoke grenades at the
        // 2 g a puff it once had).
        for p in &body.out.smoke {
            smoke.write(SmokeEmission {
                position: *p,
                velocity: Vec3::Y * 0.8,
                radius: 0.12,
                mass_kg: 0.000_12,
                temperature_c: 300.0,
                darkness: 0.55,
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
