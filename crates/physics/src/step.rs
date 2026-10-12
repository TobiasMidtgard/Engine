//! The fixed-step system: read the ECS, step every region, write the results back.

use std::time::Instant;

use engine_core::ecs::system::SystemState;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{
    ApplyImpulse, BodyKind, CharacterController, CharacterMove, CharacterState, Collider,
    ContactEvent, ExternalForce, Joint, MassProperties, RigidBody, Velocity,
};
use engine_world::{
    ContactKind, FocusPoints, Gravity, GroundContact, NoiseEvent, SurfaceMaterials, Water,
    WorldImpulse,
};
use rapier3d::prelude::{Pose, QueryFilter as RFilter, SharedShape};

use crate::character;
use crate::convert;
use crate::settings::PhysicsSettings;
use crate::sim::{BodyWrite, MaterialCache, RegionId, Simulation};
use crate::{PhysicsStats, SharedSimulation};

/// World pose (translation, rotation, scale) of `e`, composing `Transform`s up the hierarchy.
/// `GlobalTransform` is only updated in `PostUpdate`, so the step computes poses itself.
pub(crate) fn world_pose(world: &World, e: Entity) -> Option<Transform> {
    let t = *world.get::<Transform>(e)?;
    match world.get::<ChildOf>(e) {
        Some(p) => Some(world_pose(world, p.parent())?.mul_transform(&t)),
        None => Some(t),
    }
}

/// The nearest entity at or above `e` (including `e`) with a `RigidBody`.
fn owning_body(world: &World, e: Entity) -> Option<(Entity, BodyKind)> {
    let mut cur = e;
    loop {
        if let Some(rb) = world.get::<RigidBody>(cur) {
            return Some((cur, rb.kind));
        }
        cur = world.get::<ChildOf>(cur)?.parent();
    }
}

/// `e` and its descendants that carry a `Collider` and belong to `body` (stopping at
/// descendants that are bodies themselves).
fn colliders_of(world: &World, body: Entity, out: &mut Vec<Entity>) {
    let mut stack = vec![body];
    while let Some(e) = stack.pop() {
        if e != body && world.get::<RigidBody>(e).is_some() {
            continue;
        }
        if world.get::<Collider>(e).is_some() {
            out.push(e);
        }
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
}

type Removed<'w, 's> = (
    RemovedComponents<'w, 's, RigidBody>,
    RemovedComponents<'w, 's, Collider>,
    RemovedComponents<'w, 's, Joint>,
    RemovedComponents<'w, 's, CharacterController>,
);

/// Entities whose physics components were removed, noted every frame (`Last`) so removals in
/// frames without a fixed step aren't missed.
#[derive(Resource, Default)]
pub(crate) struct PendingRemovals(Vec<Entity>);

pub(crate) fn note_removals(
    mut pending: ResMut<PendingRemovals>,
    mut bodies: RemovedComponents<RigidBody>,
    mut colliders: RemovedComponents<Collider>,
    mut joints: RemovedComponents<Joint>,
    mut characters: RemovedComponents<CharacterController>,
) {
    pending.0.extend(bodies.read());
    pending.0.extend(colliders.read());
    pending.0.extend(joints.read());
    pending.0.extend(characters.read());
}

/// Cached ECS queries for [`physics_step`].
#[allow(clippy::type_complexity)]
pub(crate) struct StepQueries {
    removed: SystemState<Removed<'static, 'static>>,
    inbox: SystemState<(
        MessageReader<'static, 'static, ApplyImpulse>,
        MessageReader<'static, 'static, WorldImpulse>,
    )>,
    changed_bodies: QueryState<Entity, Changed<RigidBody>>,
    changed_colliders:
        QueryState<Entity, (With<Collider>, Or<(Changed<Collider>, Changed<ChildOf>)>)>,
    moved_colliders: QueryState<Entity, (With<Collider>, Changed<Transform>)>,
    changed_joints: QueryState<Entity, Changed<Joint>>,
    changed_characters: QueryState<Entity, Changed<CharacterController>>,
    body_inputs: QueryState<
        (
            Entity,
            &'static Transform,
            Option<&'static Velocity>,
            Option<&'static ExternalForce>,
        ),
        (
            With<RigidBody>,
            Or<(
                Changed<Transform>,
                Changed<Velocity>,
                Changed<ExternalForce>,
            )>,
        ),
    >,
    time_scaled: QueryState<(Entity, &'static LocalTimeScale), With<RigidBody>>,
    character_inputs: QueryState<
        (
            Entity,
            &'static Transform,
            Option<&'static CharacterMove>,
            Option<&'static LocalTimeScale>,
        ),
        With<CharacterController>,
    >,
}

impl FromWorld for StepQueries {
    fn from_world(world: &mut World) -> Self {
        Self {
            removed: SystemState::new(world),
            inbox: SystemState::new(world),
            changed_bodies: world.query_filtered(),
            changed_colliders: world.query_filtered(),
            moved_colliders: world.query_filtered(),
            changed_joints: world.query_filtered(),
            changed_characters: world.query_filtered(),
            body_inputs: world.query_filtered(),
            time_scaled: world.query_filtered(),
            character_inputs: world.query_filtered(),
        }
    }
}

/// The physics step (`FixedUpdate`, `SimSet::Physics`).
pub(crate) fn physics_step(world: &mut World, mut q: Local<Option<StepQueries>>) {
    let started = Instant::now();
    if q.is_none() {
        *q = Some(StepQueries::from_world(world));
    }
    let q = q.as_mut().unwrap();
    let shared = world.resource::<SharedSimulation>().0.clone();
    let mut sim = shared.write();

    // ---------------------------------------------------------------- settings & environment
    let settings = world.resource::<PhysicsSettings>().clone();
    if settings != sim.settings {
        sim.settings = settings.clone();
        for r in sim.regions.iter_mut().flatten() {
            settings.apply_to(&mut r.world.integration_parameters);
        }
    }
    let materials_changed =
        world.is_resource_changed::<SurfaceMaterials>() || sim.materials.0.is_empty();
    if materials_changed {
        let table = world.resource::<SurfaceMaterials>();
        sim.materials = MaterialCache(
            table
                .materials
                .iter()
                .map(|m| (m.friction, m.restitution))
                .collect(),
        );
        sim.refresh_materials();
    }
    let gravity = world.resource::<Gravity>().clone();
    let foci = world
        .get_resource::<FocusPoints>()
        .map(|f| f.points.clone())
        .unwrap_or_default();
    let water = world.get_resource::<Water>().cloned();
    let (dt, real_dt) = {
        let t = world.resource::<Time>();
        (t.delta_secs(), t.real_delta_secs())
    };

    // ---------------------------------------------------------------- removals
    let mut gone: Vec<Entity> = std::mem::take(&mut world.resource_mut::<PendingRemovals>().0);
    {
        let Ok((mut rb, mut co, mut jo, mut ch)) = q.removed.get_mut(world) else {
            unreachable!("removal readers always validate")
        };
        gone.extend(rb.read());
        gone.extend(co.read());
        gone.extend(jo.read());
        gone.extend(ch.read());
    }
    let mut dirty_colliders: Vec<Entity> = Vec::new();
    for e in gone {
        if world.get::<RigidBody>(e).is_none() && sim.bodies.contains_key(&e) {
            dirty_colliders.extend(sim.remove_body(e));
        }
        if world.get::<Collider>(e).is_none() && sim.colliders.contains_key(&e) {
            sim.remove_collider(e, true);
        }
        if world.get::<Joint>(e).is_none() && sim.joints.contains_key(&e) {
            sim.remove_joint(e);
        }
        if world.get::<CharacterController>(e).is_none() && sim.characters.contains_key(&e) {
            sim.remove_character(e);
        }
    }

    // ---------------------------------------------------------------- bodies
    let changed: Vec<Entity> = q.changed_bodies.iter(world).collect();
    let mut inserts: Vec<Entity> = Vec::new();
    for e in changed {
        let desc = world.get::<RigidBody>(e).unwrap().clone();
        let existing = sim.bodies.get(&e).map(|b| b.kind);
        if desc.kind == BodyKind::Static {
            if existing.is_some() {
                dirty_colliders.extend(sim.remove_body(e));
            }
            colliders_of(world, e, &mut dirty_colliders);
            continue;
        }
        if existing == Some(desc.kind) {
            sim.update_body_params(e, &desc);
            continue;
        }
        let Some(pose) = world_pose(world, e) else {
            continue;
        };
        let velocity = world.get::<Velocity>(e).copied().unwrap_or_default();
        let parent = world.get::<ChildOf>(e).map(|p| p.parent());
        dirty_colliders.extend(sim.remove_body(e));
        sim.create_body(
            e,
            &desc,
            (pose.translation, pose.rotation),
            velocity,
            parent,
        );
        colliders_of(world, e, &mut dirty_colliders);
        inserts.push(e);
    }

    // ---------------------------------------------------------------- colliders
    dirty_colliders.extend(q.changed_colliders.iter(world));
    for e in q.moved_colliders.iter(world) {
        match sim.colliders.get(&e) {
            // Our own writes to a body's Transform move its colliders already.
            Some(c) if c.body == Some(e) => {}
            Some(c) if c.body.is_none() => {
                if let Some(p) = world_pose(world, e) {
                    sim.move_static(e, (p.translation, p.rotation));
                }
            }
            _ => dirty_colliders.push(e),
        }
    }
    dirty_colliders.sort();
    dirty_colliders.dedup();
    for e in dirty_colliders {
        let Some(desc) = world.get::<Collider>(e).cloned() else {
            continue;
        };
        let Some(pose) = world_pose(world, e) else {
            continue;
        };
        let Some(shape) = convert::shape(&desc.shape, pose.scale) else {
            engine_core::log::warn!(
                "physics: collider on {e} has a shape that can't be built ({:?}); skipped",
                std::mem::discriminant(&desc.shape)
            );
            continue;
        };
        let owner = owning_body(world, e)
            .filter(|(b, k)| *k != BodyKind::Static && sim.bodies.contains_key(b));
        match owner {
            Some((body, _)) => {
                let body_pose = world_pose(world, body).unwrap_or_default();
                let inv = body_pose.rotation.inverse();
                let local = Pose::from_parts(
                    inv * (pose.translation - body_pose.translation).as_vec3(),
                    inv * pose.rotation,
                );
                let (f, r) = sim.materials.get(desc.surface);
                sim.attach_collider(e, body, &desc, shape, local, f, r);
            }
            None => sim.add_static(e, &desc, shape, (pose.translation, pose.rotation)),
        }
    }

    // ---------------------------------------------------------------- joints
    for e in q.changed_joints.iter(world).collect::<Vec<_>>() {
        let desc = world.get::<Joint>(e).unwrap().clone();
        sim.remove_joint(e);
        sim.joints.insert(
            e,
            crate::sim::JointEntry {
                desc,
                created: None,
            },
        );
    }
    let waiting: Vec<Entity> = sim
        .joints
        .iter()
        .filter(|(_, j)| j.created.is_none())
        .map(|(e, _)| *e)
        .collect();
    for e in waiting {
        let lookup = |x: Entity| world_pose(world, x).map(|t| (t.translation, t.rotation));
        sim.create_joint(e, &lookup);
    }

    // ---------------------------------------------------------------- characters
    for e in q.changed_characters.iter(world).collect::<Vec<_>>() {
        let desc = *world.get::<CharacterController>(e).unwrap();
        let at = world_pose(world, e)
            .map(|t| t.translation)
            .unwrap_or_default();
        sim.create_character(e, &desc, at);
        if world.get::<CharacterState>(e).is_none() {
            inserts.push(e);
        }
    }
    for (e, t, mv, local) in q.character_inputs.iter(world) {
        let Some(c) = sim.characters.get_mut(&e) else {
            continue;
        };
        let teleported = c.written != Some(t.translation);
        c.desired = mv.map(|m| m.velocity).unwrap_or_default();
        c.world_dt = dt;
        c.dt = if let Some(l) = local {
            let t = world.resource::<Time>();
            t.delta_for(Some(l))
        } else {
            dt
        };
        if teleported {
            let (rid, body) = (c.region, c.body);
            let region = sim.region_mut(rid);
            let local_p = region.local(t.translation);
            if let Some(rb) = region.world.bodies.get_mut(body) {
                rb.set_translation(local_p, true);
                rb.set_next_kinematic_translation(local_p);
            }
        }
    }

    // ---------------------------------------------------------------- gameplay inputs
    let inputs: Vec<(Entity, Transform, Option<Velocity>, Option<ExternalForce>)> = q
        .body_inputs
        .iter(world)
        .map(|(e, t, v, f)| (e, *t, v.copied(), f.copied()))
        .collect();
    for (e, t, v, f) in inputs {
        let Some(entry) = sim.bodies.get(&e) else {
            continue;
        };
        let (rid, handle, kind) = (entry.region, entry.handle, entry.kind);
        let teleported = entry
            .written
            .is_some_and(|w| w != (t.translation, t.rotation));
        let new_velocity = v.filter(|v| entry.written_velocity.is_none_or(|w| w != *v));
        let new_force = f
            .map(|f| (f.force, f.torque, f.at))
            .filter(|f| *f != entry.external);
        if teleported {
            let pose = world_pose(world, e).unwrap_or(t);
            let region = sim.region_mut(rid);
            let p = Pose::from_parts(region.local(pose.translation), pose.rotation);
            if let Some(rb) = region.world.bodies.get_mut(handle) {
                if kind == BodyKind::Kinematic {
                    rb.set_next_kinematic_position(p);
                } else {
                    rb.set_position(p, true);
                }
            }
            sim.bodies.get_mut(&e).unwrap().written = Some((t.translation, t.rotation));
        }
        if let Some(v) = new_velocity {
            let entry = sim.bodies.get_mut(&e).unwrap();
            entry.written_velocity = Some(v);
            if kind == BodyKind::Kinematic {
                entry.kinematic_by_velocity = v.linear != Vec3::ZERO || v.angular != Vec3::ZERO;
            } else if let Some(rb) = sim.region_mut(rid).world.bodies.get_mut(handle) {
                rb.set_linvel(v.linear, true);
                rb.set_angvel(v.angular, true);
            }
        }
        if let Some(force) = new_force {
            let entry = sim.bodies.get_mut(&e).unwrap();
            entry.external = force;
            entry.forces_set = true;
            if let Some(rb) = sim.region_mut(rid).world.bodies.get_mut(handle) {
                rb.wake_up(true);
            }
        }
    }
    {
        let time = world.resource::<Time>().clone();
        for entry in sim.bodies.values_mut() {
            entry.time_factor = 1.0;
        }
        for (e, l) in q.time_scaled.iter(world) {
            if let Some(entry) = sim.bodies.get_mut(&e)
                && dt > 0.0
            {
                entry.time_factor = (time.delta_for(Some(l)) / dt).clamp(0.0, 20.0);
            }
        }
    }
    // Impulses and blasts.
    {
        let Ok((mut impulses, mut blasts)) = q.inbox.get_mut(world) else {
            unreachable!("message readers always validate")
        };
        let impulses: Vec<ApplyImpulse> = impulses.read().copied().collect();
        let blasts: Vec<WorldImpulse> = blasts.read().copied().collect();
        for i in impulses {
            apply_impulse(&mut sim, &i);
        }
        if settings.blast_response {
            for b in blasts {
                apply_blast(&mut sim, &b, settings.blast_scale);
            }
        }
    }

    // ---------------------------------------------------------------- regions
    let max_step = sim
        .live_regions()
        .flat_map(|(_, r)| r.world.bodies.iter().map(|(_, b)| b.linvel().length()))
        .fold(0.0f32, f32::max) as f64
        * dt as f64;
    sim.maintain_regions(&foci, max_step);

    // ---------------------------------------------------------------- step
    let substeps = if dt > 0.0 {
        ((dt / settings.max_substep_secs.max(1e-4)).ceil() as u32)
            .clamp(1, settings.max_substeps.max(1))
    } else {
        0
    };
    let sub_dt = if substeps > 0 {
        dt / substeps as f32
    } else {
        0.0
    };
    let region_ids: Vec<RegionId> = sim.live_regions().map(|(id, _)| id).collect();
    for rid in region_ids.iter().copied() {
        let (frozen, movers) = {
            let r = sim.region(rid);
            (r.frozen, r.movers)
        };
        if frozen || movers == 0 || substeps == 0 {
            let r = sim.region_mut(rid);
            if r.needs_refresh {
                r.world.detect_collisions(&(), &());
                r.needs_refresh = false;
                r.update_bounds();
            }
            continue;
        }
        let g = gravity.at(sim.region(rid).anchor);
        sim.region_mut(rid).world.gravity = g;
        prepare_forces(
            &mut sim,
            rid,
            &gravity,
            g,
            water.as_ref(),
            settings.buoyancy,
        );
        if sim.region(rid).needs_refresh && sim.characters.values().any(|c| c.region == rid) {
            sim.region_mut(rid).refresh_static_queries();
        }
        character::move_characters(&mut sim, rid, &gravity);
        for _ in 0..substeps {
            drive_kinematics(&mut sim, rid, sub_dt);
            let scaled = scale_time(&mut sim, rid, true);
            {
                let r = sim.region_mut(rid);
                r.world.integration_parameters.dt = sub_dt;
                let collector = std::mem::take(&mut r.collector);
                r.world.step_with_events(&(), &collector);
                r.collector = collector;
                r.needs_refresh = false;
            }
            if scaled {
                scale_time(&mut sim, rid, false);
            }
            if settings.contact_events {
                sim.process_contacts(rid, sub_dt);
            } else {
                sim.region(rid).collector.0.lock().clear();
            }
            break_joints(&mut sim, rid, sub_dt);
        }
        sim.region_mut(rid).update_bounds();
    }
    sim.recheck_contacts();
    let _ = real_dt;

    // ---------------------------------------------------------------- write back
    collect_writes(&mut sim);
    let writes = std::mem::take(&mut sim.writes);
    for w in &writes {
        let parent = sim.bodies.get(&w.entity).and_then(|b| b.parent);
        let local = match parent.and_then(|p| world_pose(world, p)) {
            Some(pp) => {
                let inv = pp.rotation.inverse();
                (
                    (inv * (w.translation - pp.translation).as_vec3() / pp.scale).as_dvec3()
                        + DVec3::ZERO,
                    inv * w.rotation,
                )
            }
            None => (w.translation, w.rotation),
        };
        if let Some(mut t) = world.get_mut::<Transform>(w.entity) {
            t.translation = local.0;
            t.rotation = local.1;
        }
        if let Some(mut v) = world.get_mut::<Velocity>(w.entity) {
            *v = w.velocity;
        }
        if let Some(entry) = sim.bodies.get_mut(&w.entity) {
            entry.written = Some(local);
            entry.written_velocity = Some(w.velocity);
        }
    }
    // Mass properties.
    let masses: Vec<(Entity, MassProperties)> = sim
        .bodies
        .iter_mut()
        .filter(|(_, b)| b.mass_dirty)
        .map(|(e, b)| {
            b.mass_dirty = false;
            (*e, (b.region, b.handle))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|(e, (rid, h))| {
            let rb = sim.region(rid).world.bodies.get(h)?;
            let m = &rb.mass_properties().local_mprops;
            Some((
                e,
                MassProperties {
                    mass_kg: m.mass(),
                    center_of_mass: m.local_com,
                    inertia: m.principal_inertia(),
                },
            ))
        })
        .collect();
    for (e, m) in masses {
        if let Ok(mut ent) = world.get_entity_mut(e) {
            ent.insert(m);
        }
    }
    // Characters.
    let char_writes: Vec<(Entity, DVec3, CharacterState)> = sim
        .characters
        .iter()
        .filter_map(|(e, c)| {
            let region = sim.region(c.region);
            let rb = region.world.bodies.get(c.body)?;
            let p = rb.next_position().translation;
            Some((*e, region.global(p), c.state))
        })
        .collect();
    for (e, p, state) in char_writes {
        if let Some(mut t) = world.get_mut::<Transform>(e) {
            t.translation = p;
        }
        if let Ok(mut ent) = world.get_entity_mut(e) {
            ent.insert(state);
        }
        if let Some(c) = sim.characters.get_mut(&e) {
            c.written = Some(p);
        }
    }
    // Missing companion components on new bodies.
    for e in inserts {
        let Ok(mut ent) = world.get_entity_mut(e) else {
            continue;
        };
        if ent.contains::<RigidBody>() {
            if !ent.contains::<Velocity>() {
                ent.insert(Velocity::default());
            }
            if !ent.contains::<TransformInterpolation>() && !ent.contains::<ChildOf>() {
                ent.insert(TransformInterpolation::default());
            }
        }
        if ent.contains::<CharacterController>() && !ent.contains::<CharacterState>() {
            ent.insert(CharacterState::default());
        }
    }
    // Broken joints.
    for e in std::mem::take(&mut sim.broken_joints) {
        sim.joints.remove(&e);
        let is_body = world.get::<RigidBody>(e).is_some();
        if let Ok(mut ent) = world.get_entity_mut(e) {
            if is_body {
                ent.remove::<Joint>();
            } else {
                ent.despawn();
            }
        }
    }

    // ---------------------------------------------------------------- messages
    let contacts: Vec<ContactEvent> = std::mem::take(&mut sim.contacts.out);
    let impacts = std::mem::take(&mut sim.contacts.impacts);
    let g = gravity
        .at(foci.first().copied().unwrap_or_default())
        .length()
        .max(0.1);
    for c in contacts {
        world.write_message(c);
    }
    if settings.impact_events {
        for i in impacts {
            let dv = i.event.impulse / i.mass.max(1e-3);
            if dv < settings.impact_min_speed {
                continue;
            }
            world.write_message(NoiseEvent {
                position: i.event.point,
                radius: (dv * 6.0).clamp(4.0, 80.0),
                source: Some(i.body),
            });
            if i.other_static {
                world.write_message(GroundContact {
                    position: i.event.point,
                    normal: i.normal,
                    size: i.size,
                    yaw: 0.0,
                    load_kg: i.mass * (1.0 + dv / g),
                    speed: i.tangent_speed,
                    kind: ContactKind::Body,
                    surface: Some(i.other_surface),
                    entity: Some(i.body),
                });
            }
        }
    }

    // ---------------------------------------------------------------- stats
    let stats = PhysicsStats::from_sim(&sim, started.elapsed().as_secs_f64() * 1000.0, substeps);
    drop(sim);
    if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
        d.set("physics.bodies", stats.bodies as f64);
        d.set("physics.awake", stats.awake as f64);
        d.set("physics.regions", stats.regions as f64);
        d.set("physics.contacts", stats.touching as f64);
        d.set("physics.step_ms", stats.step_ms);
    }
    world.insert_resource(stats);
}

fn apply_impulse(sim: &mut Simulation, i: &ApplyImpulse) {
    let Some(b) = sim.bodies.get(&i.entity) else {
        return;
    };
    let (rid, h) = (b.region, b.handle);
    let region = sim.region_mut(rid);
    let anchor = region.anchor;
    let Some(rb) = region.world.bodies.get_mut(h) else {
        return;
    };
    if !rb.is_dynamic() {
        return;
    }
    match i.at {
        Some(p) => rb.apply_impulse_at_point(i.impulse, (p - anchor).as_vec3(), true),
        None => rb.apply_impulse(i.impulse, true),
    }
    if i.angular != Vec3::ZERO {
        rb.apply_torque_impulse(i.angular, true);
    }
}

/// Push dynamic bodies away from an explosion: aerodynamic drag of the blast wind
/// (`½·ρ·A·v²` over the blast's duration), so light and broad things fly, heavy ones budge.
fn apply_blast(sim: &mut Simulation, blast: &WorldImpulse, scale: f32) {
    if blast.radius <= 0.0 || blast.strength <= 0.0 {
        return;
    }
    let duration = (blast.radius / blast.strength).clamp(0.02, 0.5);
    let ids: Vec<RegionId> = sim.live_regions().map(|(id, _)| id).collect();
    for rid in ids {
        let region = sim.region_mut(rid);
        if region.anchor.distance(blast.center) > region_reach(region) + blast.radius as f64 {
            continue;
        }
        let center = region.local(blast.center);
        let ball = SharedShape::ball(blast.radius);
        let mut hits: Vec<(rapier3d::prelude::RigidBodyHandle, Vec3, Vec3)> = Vec::new();
        for (_, c) in region.world.intersect_shape(
            Pose::from_translation(center),
            &*ball,
            RFilter::exclude_fixed().exclude_sensors(),
        ) {
            let Some(parent) = c.parent() else {
                continue;
            };
            let aabb = c.compute_aabb();
            let closest = center.clamp(aabb.mins, aabb.maxs);
            let ext = aabb.half_extents() * 2.0;
            let area = (ext.x * ext.y * ext.z).max(1e-6).powf(2.0 / 3.0);
            hits.push((parent, closest, Vec3::splat(area)));
        }
        for (h, point, area) in hits {
            let Some(rb) = region.world.bodies.get_mut(h) else {
                continue;
            };
            if !rb.is_dynamic() {
                continue;
            }
            let com = rb.center_of_mass();
            let to = com - center;
            let d = to.length();
            let falloff = (1.0 - d / blast.radius).clamp(0.0, 1.0);
            let wind = blast.strength * falloff * falloff;
            let dir = to.try_normalize().unwrap_or(Vec3::Y);
            let impulse = 0.5 * 1.2 * area.x * wind * wind * duration * scale;
            rb.apply_impulse_at_point(dir * impulse, point, true);
        }
    }
}

fn region_reach(region: &crate::sim::Region) -> f64 {
    region
        .world_bounds()
        .map(|(lo, hi)| (hi - lo).length() * 0.5 + region.anchor.distance((lo + hi) * 0.5))
        .unwrap_or(0.0)
}

/// Set this step's forces: gameplay `ExternalForce`, the difference between real gravity and
/// the region's gravity (planets), and buoyancy/drag in water.
fn prepare_forces(
    sim: &mut Simulation,
    rid: RegionId,
    gravity: &Gravity,
    region_g: Vec3,
    water: Option<&Water>,
    buoyancy: bool,
) {
    let varying = !gravity.bodies.is_empty();
    let entities: Vec<Entity> = sim
        .bodies
        .iter()
        .filter(|(_, b)| b.region == rid && b.kind == BodyKind::Dynamic)
        .map(|(e, _)| *e)
        .collect();
    for e in entities {
        let entry = sim.bodies.get_mut(&e).unwrap();
        let (h, external, gravity_scale, had_forces) = (
            entry.handle,
            entry.external,
            entry.desc.gravity_scale,
            entry.forces_set,
        );
        let region = sim.regions[rid as usize].as_mut().unwrap();
        let anchor = region.anchor;
        let Some(rb) = region.world.bodies.get_mut(h) else {
            continue;
        };
        if rb.is_sleeping() && !had_forces {
            continue;
        }
        let com = rb.center_of_mass();
        let mass = rb.mass();
        let mut force = external.0;
        let mut torque = external.1;
        if let Some(at) = external.2 {
            torque += ((at - anchor).as_vec3() - com).cross(external.0);
        }
        if varying {
            let dg = gravity.at(anchor + com.as_dvec3()) - region_g;
            if dg.length_squared() > 1.0e-8 {
                force += dg * mass * gravity_scale;
            }
        }
        let mut wet = false;
        if buoyancy
            && let Some(w) = water
            && !rb.is_sleeping()
        {
            let world_com = anchor + com.as_dvec3();
            if let Some(s) = w.sample(world_com) {
                let colliders: Vec<_> = rb.colliders().to_vec();
                let up = gravity.up_at(world_com);
                let g = gravity.at(world_com).length();
                let v = rb.linvel();
                let w_ang = rb.angvel();
                for ch in colliders {
                    let Some(c) = region.world.colliders.get(ch) else {
                        continue;
                    };
                    if c.is_sensor() {
                        continue;
                    }
                    let aabb = c.compute_aabb();
                    let bottom = anchor.y + aabb.mins.y as f64;
                    let height = (aabb.maxs.y - aabb.mins.y).max(1e-3);
                    let depth = (s.surface_height - bottom) as f32;
                    let frac = (depth / height).clamp(0.0, 1.0);
                    if frac <= 0.0 {
                        continue;
                    }
                    wet = true;
                    let volume = c.volume();
                    let lift = up * (1000.0 * g * volume * frac);
                    let center = aabb.center();
                    let point = Vec3::new(center.x, aabb.mins.y + height * frac * 0.5, center.z);
                    force += lift;
                    torque += (point - com).cross(lift);
                    // Water drag relative to the current, and rotational drag.
                    let rel = v - s.velocity;
                    force -= rel * (mass * 0.8 * frac);
                    torque -= w_ang * (mass * 0.3 * frac * height * height);
                    // Bobbing dies out within a few periods (waves carry the energy away):
                    // heave damping at about half the critical value.
                    let area = (aabb.maxs.x - aabb.mins.x) * (aabb.maxs.z - aabb.mins.z);
                    let critical = 2.0 * (1000.0 * g * area * mass).sqrt();
                    force -= up * (rel.dot(up) * critical * 0.5 * (frac * 2.0).min(1.0));
                }
            }
        }
        let rb = region.world.bodies.get_mut(h).unwrap();
        let needs = force != Vec3::ZERO || torque != Vec3::ZERO;
        if needs || had_forces {
            rb.reset_forces(false);
            rb.reset_torques(false);
            if needs {
                rb.add_force(force, false);
                rb.add_torque(torque, false);
            }
        }
        sim.bodies.get_mut(&e).unwrap().forces_set = needs || (wet && had_forces);
    }
}

/// Kinematic bodies moved by `Velocity`: advance their target pose by one substep.
fn drive_kinematics(sim: &mut Simulation, rid: RegionId, sub_dt: f32) {
    let moving: Vec<(rapier3d::prelude::RigidBodyHandle, Velocity)> = sim
        .bodies
        .values()
        .filter(|b| b.region == rid && b.kinematic_by_velocity)
        .map(|b| (b.handle, b.written_velocity.unwrap_or_default()))
        .collect();
    let region = sim.region_mut(rid);
    for (h, v) in moving {
        if let Some(rb) = region.world.bodies.get_mut(h) {
            let p = *rb.position();
            let angle = v.angular * sub_dt;
            let rot = Quat::from_scaled_axis(angle) * p.rotation;
            rb.set_next_kinematic_position(Pose::from_parts(
                p.translation + v.linear * sub_dt,
                rot.normalize(),
            ));
        }
    }
}

/// Bodies with their own time rate (`LocalTimeScale`): scale velocity and gravity so they move
/// `factor`× as fast as the world for one substep (`apply = true`), then undo it.
fn scale_time(sim: &mut Simulation, rid: RegionId, apply: bool) -> bool {
    let scaled: Vec<(rapier3d::prelude::RigidBodyHandle, f32, f32)> = sim
        .bodies
        .values()
        .filter(|b| {
            b.region == rid && b.kind == BodyKind::Dynamic && (b.time_factor - 1.0).abs() > 1e-3
        })
        .map(|b| (b.handle, b.time_factor, b.desc.gravity_scale))
        .collect();
    if scaled.is_empty() {
        return false;
    }
    let region = sim.region_mut(rid);
    for (h, k, gs) in scaled {
        let Some(rb) = region.world.bodies.get_mut(h) else {
            continue;
        };
        let k = k.max(1e-3);
        let (m, gscale) = if apply {
            (k, gs * k * k)
        } else {
            (1.0 / k, gs)
        };
        let (v, w) = (rb.linvel(), rb.angvel());
        rb.set_linvel(v * m, false);
        rb.set_angvel(w * m, false);
        rb.set_gravity_scale(gscale, false);
    }
    true
}

/// Remove joints whose constraint force went over their `break_force`.
fn break_joints(sim: &mut Simulation, rid: RegionId, sub_dt: f32) {
    if sub_dt <= 0.0 {
        return;
    }
    let candidates: Vec<(Entity, rapier3d::prelude::ImpulseJointHandle, f32)> = sim
        .joints
        .iter()
        .filter_map(|(e, j)| {
            let (r, h) = j.created?;
            (r == rid).then_some((*e, h, j.desc.break_force?))
        })
        .collect();
    let iterations = sim.settings.solver_iterations.clamp(1, 64) as f32;
    for (e, h, limit) in candidates {
        let region = sim.region_mut(rid);
        let Some(joint) = region.world.impulse_joints.get(h) else {
            continue;
        };
        // Joint impulses are per solver iteration (each iteration is a small substep).
        let i = joint.impulses;
        let force = Vec3::new(i[0], i[1], i[2]).length() * iterations / sub_dt;
        if force > limit {
            region.world.impulse_joints.remove(h, true);
            sim.joints.get_mut(&e).unwrap().created = None;
            sim.broken_joints.push(e);
        }
    }
}

/// Positions and velocities of bodies that moved this step.
fn collect_writes(sim: &mut Simulation) {
    let mut writes = std::mem::take(&mut sim.writes);
    writes.clear();
    for (e, b) in sim.bodies.iter_mut() {
        let Some(Some(region)) = sim.regions.get(b.region as usize) else {
            continue;
        };
        let Some(rb) = region.world.bodies.get(b.handle) else {
            continue;
        };
        if b.kind == BodyKind::Kinematic && !b.kinematic_by_velocity {
            continue;
        }
        let sleeping = rb.is_sleeping();
        if sleeping && b.was_sleeping && b.written.is_some() {
            continue;
        }
        b.was_sleeping = sleeping;
        let p = rb.position();
        let velocity = if b.kind == BodyKind::Kinematic {
            b.written_velocity.unwrap_or_default()
        } else if sleeping {
            Velocity::default()
        } else {
            Velocity {
                linear: rb.linvel(),
                angular: rb.angvel(),
            }
        };
        writes.push(BodyWrite {
            entity: *e,
            translation: region.global(p.translation),
            rotation: p.rotation,
            velocity,
        });
    }
    sim.writes = writes;
}
