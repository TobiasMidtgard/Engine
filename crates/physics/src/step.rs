//! The fixed-step system: read the ECS, step every region, write the results back.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use engine_core::ecs::system::SystemState;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{
    ApplyImpulse, BodyKind, CharacterController, CharacterMove, CharacterState, Collider,
    ContactEvent, ExternalForce, Joint, MassProperties, RigidBody, Velocity,
};
use engine_world::{
    ContactKind, FocusPoints, Gravity, GroundContact, NoiseEvent, SimulationRange,
    SurfaceMaterials, Water, WorldImpulse,
};
use rapier3d::prelude::{
    ActiveHooks, ColliderHandle, PhysicsWorld, Pose, QueryFilter as RFilter, RigidBodyHandle,
    SharedShape,
};

use crate::buoyancy::{Surface, WATER_DENSITY};
use crate::character;
use crate::convert;
use crate::settings::PhysicsSettings;
use crate::sim::{BodyWrite, ContactFilter, MaterialCache, RegionId, Simulation};
use crate::{FineSteps, PhysicsStats, SharedSimulation};

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
    fine: QueryState<(Entity, &'static FineSteps), With<RigidBody>>,
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
            fine: world.query_filtered(),
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
    // (Queries from other threads hold it for reading.)
    let lock = tracing::info_span!("simulation lock").entered();
    let mut sim = shared.write();
    drop(lock);
    let sync = tracing::info_span!("sync from world").entered();

    // ---------------------------------------------------------------- settings & environment
    let settings = world.resource::<PhysicsSettings>().clone();
    if settings != sim.settings {
        if settings.articulated_joints != sim.settings.articulated_joints {
            sim.unmake_joints();
        }
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
                .map(|m| (m.friction, m.restitution, crate::rolling::sinkage(m)))
                .collect(),
        );
        sim.refresh_materials();
    }
    sim.range = world.get_resource::<SimulationRange>().copied();
    sim.ground = world.get_resource::<engine_world::Ground>().cloned();
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
        if world.get::<RigidBody>(e).is_none() {
            dirty_colliders.extend(sim.forget_body(e));
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
                // A world shift moves the cached pose along with the entity: nothing to do.
                if let Some(p) = world_pose(world, e)
                    && (c.world_pose.0.distance(p.translation) > 1e-6
                        || !c.world_pose.1.abs_diff_eq(p.rotation, 1e-6))
                {
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
        let found = owning_body(world, e);
        let owner = found.filter(|(b, k)| *k != BodyKind::Static && sim.bodies.contains_key(b));
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
            None => sim.add_static(
                e,
                found.map(|(b, _)| b),
                &desc,
                shape,
                (pose.translation, pose.rotation),
            ),
        }
    }

    // ---------------------------------------------------------------- joints
    for e in q.changed_joints.iter(world).collect::<Vec<_>>() {
        let desc = world.get::<Joint>(e).unwrap().clone();
        // A world shift moves world anchors in the cache too.
        if sim.joints.get(&e).is_some_and(|j| j.desc == desc) {
            continue;
        }
        sim.remove_joint(e);
        sim.joints.insert(e, crate::sim::JointEntry::new(desc));
    }
    if !sim.regroup.is_empty() {
        let graph = sim.joint_graph();
        sim.rearticulate(&graph);
    }
    // The articulation budget looks every few steps, and whenever joints are being made (so a
    // new ragdoll past the budget starts on constraint joints).
    sim.budget_clock += 1;
    let mut graph = None;
    if sim.settings.articulated_joints
        && (sim.budget_clock >= crate::sim::BUDGET_EVERY
            || sim.joints.values().any(|j| j.created.is_none()))
    {
        sim.budget_clock = 0;
        let g = sim.joint_graph();
        let lookup = |x: Entity| world_pose(world, x).map(|t| (t.translation, t.rotation));
        sim.budget_articulations(&g, &foci, &lookup);
        graph = Some(g);
    }
    let waiting: Vec<Entity> = sim
        .joints
        .iter()
        .filter(|(_, j)| j.created.is_none())
        .map(|(e, _)| *e)
        .collect();
    let graph = match graph {
        Some(g) => g,
        None if waiting.is_empty() => Default::default(),
        None => sim.joint_graph(),
    };
    for e in waiting {
        let lookup = |x: Entity| world_pose(world, x).map(|t| (t.translation, t.rotation));
        sim.create_joint(e, &lookup, &graph);
    }
    if !sim.rehang.is_empty() {
        let graph = sim.joint_graph();
        sim.hang_loads(&graph);
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
        // A kinematic body moved by its `Transform` is never written back, so until gameplay
        // moves it there is nothing to compare with: take its pose as gameplay's.
        let teleported = match entry.written {
            Some(w) => w != (t.translation, t.rotation),
            None => kind == BodyKind::Kinematic,
        };
        let new_velocity = v.filter(|v| entry.written_velocity.is_none_or(|w| w != *v));
        let new_force = f
            .map(|f| (f.force, f.torque, f.at))
            .filter(|f| *f != entry.external);
        if teleported {
            let pose = world_pose(world, e).unwrap_or(t);
            let by_velocity = match new_velocity {
                Some(v) => v.linear != Vec3::ZERO || v.angular != Vec3::ZERO,
                None => entry.kinematic_by_velocity,
            };
            let region = sim.region_mut(rid);
            let p = Pose::from_parts(region.local(pose.translation), pose.rotation);
            // A kinematic body driven by its `Transform` moves there over the step, carrying
            // what it touches; one driven by `Velocity` jumps there and goes on from it.
            let glide = kind == BodyKind::Kinematic && !by_velocity;
            if !glide && let Some(rb) = region.world.bodies.get_mut(handle) {
                rb.set_position(p, true);
            }
            let entry = sim.bodies.get_mut(&e).unwrap();
            entry.written = Some((t.translation, t.rotation));
            entry.kinematic_goal = glide.then_some(p);
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
            sim.forced.insert(e);
            if let Some(rb) = sim.region_mut(rid).world.bodies.get_mut(handle) {
                rb.wake_up(true);
            }
        }
    }
    {
        let time = world.resource::<Time>().clone();
        for entry in sim.bodies.values_mut() {
            entry.time_factor = 1.0;
            entry.fine = None;
        }
        for (e, f) in q.fine.iter(world) {
            if let Some(entry) = sim.bodies.get_mut(&e) {
                entry.fine = Some(f.substep_secs.max(1e-4));
            }
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
    sim.water_clock += 1;
    if sim.water_clock >= WATER_LOOK_EVERY {
        sim.water_clock = 0;
        if settings.buoyancy
            && let Some(w) = water.as_ref()
        {
            wake_for_water(&mut sim, w);
        }
    }

    crate::grounding::hold_ungrounded(&mut sim, &foci);
    if !sim.resting.is_empty() || !sim.ungrounded.is_empty() {
        hold_resting(&mut sim);
    }
    drop(sync);
    // ---------------------------------------------------------------- regions
    let upkeep = tracing::info_span!("region upkeep").entered();
    // Only awake bodies move (sleeping ones are held still).
    let max_step = sim
        .live_regions()
        .flat_map(|(_, r)| {
            let w = &r.world;
            w.islands
                .active_bodies()
                .filter_map(|h| w.bodies.get(h))
                .map(|b| b.linvel().length())
        })
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
    if sub_dt > 0.0 {
        sim.last_sub_dt = sub_dt;
    }
    let region_ids: Vec<RegionId> = sim.live_regions().map(|(id, _)| id).collect();
    drop(upkeep);
    let report = crate::report::wanted();
    let planning = tracing::info_span!("region plans").entered();
    let busy = busy_regions(&sim);
    // Regions step side by side (Jolt and PhysX solve islands as parallel jobs; regions are
    // islands that share nothing): each substep's own work runs as one job per region on the
    // compute pool, between passes that touch the shared books (forces, kinematics, contact
    // events, joints) one region at a time. A region's substeps run in order as before, so it
    // steps exactly as it would alone.
    let mut plans: Vec<Plan> = Vec::new();
    for rid in region_ids.iter().copied() {
        let region_started = Instant::now();
        let (frozen, movers) = {
            let r = sim.region(rid);
            (
                r.frozen || r.tier >= crate::lod::REST || (!busy.contains(&rid) && idle(r)),
                r.movers,
            )
        };
        // Far regions step coarser, or sit a frame out (`crate::lod`).
        let plan = if frozen || movers == 0 || substeps == 0 {
            None
        } else {
            let r = sim.region_mut(rid);
            let plan = r.plan(dt, &settings);
            if plan.is_none() {
                continue;
            }
            plan
        };
        let Some((substeps, sub_dt)) = plan else {
            place_kinematics(&mut sim, rid);
            let s = &mut *sim;
            let filter = ContactFilter {
                no_contact: &s.no_contact,
                dt: 0.0,
            };
            let r = s.regions[rid as usize].as_mut().expect("live region");
            if r.needs_refresh && r.dormant {
                // A dormant region's contacts wait for its first real step (all of it asleep,
                // nothing here needs them): only the query tree learns of its new colliders.
                // Finding a yard's 10,000 pairs afresh, a split cost 30-85 ms in one step.
                if r.stale_all || !r.fresh.is_empty() {
                    r.refresh_queries();
                    r.update_bounds();
                }
            } else if r.needs_refresh {
                let _refresh = tracing::info_span!("refresh resting region").entered();
                // A zero-length step: Rapier applies the removals and updates the contacts and
                // its islands together (its collision-only pass leaves the islands a link
                // behind the contact graph, and a later contact gets mislinked).
                let t0 = Instant::now();
                let fresh = r.fresh.len();
                r.world.integration_parameters.dt = 0.0;
                r.world.step_with_events(&filter, &());
                r.needs_refresh = false;
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                if ms > 5.0 && report {
                    engine_core::log::info!(
                        "physics regions: refreshing resting region {rid} ({} bodies, {} \
                         colliders, {fresh} fresh, {} pairs) took {ms:.1} ms",
                        r.world.bodies.len(),
                        r.world.colliders.len(),
                        r.world.narrow_phase.contact_pairs().count(),
                    );
                }
                r.queries_refreshed();
                r.update_bounds();
            }
            continue;
        };
        let g = gravity.at(sim.region(rid).anchor);
        sim.region_mut(rid).world.gravity = g;
        refresh_contact_hooks(&mut sim.region_mut(rid).world);
        if sim.characters.values().any(|c| c.region == rid) {
            let r = sim.region_mut(rid);
            if r.stale_all || !r.fresh.is_empty() {
                r.refresh_queries();
            }
            character::find_platforms(&mut sim, rid, &gravity);
        }
        // The kinematic and time-scaled bodies here, found once for all substeps.
        let special: Vec<Entity> = sim
            .bodies
            .iter()
            .filter(|(_, b)| {
                b.region == rid
                    && (b.kind == BodyKind::Kinematic || (b.time_factor - 1.0).abs() > 1e-3)
            })
            .map(|(e, _)| *e)
            .collect();
        plans.push(Plan {
            rid,
            substeps,
            sub_dt,
            scaled: time_scaled(&sim, rid, &special),
            decks: decks(&sim, rid, &special),
            cones: crate::cones::cones(&sim, rid),
            special,
            ms: region_started.elapsed().as_secs_f64() * 1000.0,
        });
    }
    drop(planning);
    let rounds = plans.iter().map(|p| p.substeps).max().unwrap_or(0);
    let min_speed = settings.contact_events.then_some(settings.impact_min_speed);
    for sub in 0..rounds {
        let here: Vec<usize> = (0..plans.len())
            .filter(|&i| plans[i].substeps > sub)
            .collect();
        // Forces and kinematics, region by region (they read and write the body books).
        let mut marks = Vec::with_capacity(here.len());
        for &i in &here {
            let p = &plans[i];
            let ours = tracing::info_span!("substep upkeep").entered();
            let t0 = Instant::now();
            // Every substep: water pressure follows the pose (forces held over several
            // substeps lag behind it and pump energy into a floating body's rocking).
            let g = sim.region(p.rid).world.gravity;
            prepare_forces(
                &mut sim,
                p.rid,
                &gravity,
                g,
                water.as_ref(),
                settings.buoyancy,
            );
            let t0b = Instant::now();
            drive_kinematics(&mut sim, p.rid, &p.special, p.sub_dt, p.substeps - sub);
            drop(ours);
            marks.push((t0b - t0, t0b.elapsed()));
        }
        // Each region's substep on its own: cones, Rapier, rolling, the pairs to look at.
        let pending: Vec<HashSet<(ColliderHandle, ColliderHandle)>> = here
            .iter()
            .map(|&i| {
                if !settings.contact_events {
                    return HashSet::new();
                }
                // (The pending pairs' contacts are read with all the others.)
                sim.contacts
                    .pending
                    .values()
                    .filter(|v| v.0 == plans[i].rid)
                    .map(|v| crate::sim::ordered(v.1, v.2))
                    .collect()
            })
            .collect();
        let outs: Vec<SubstepOut> = {
            let s = &mut *sim;
            let shared = SubstepShared {
                no_contact: &s.no_contact,
                materials: &s.materials,
                rolling: s.settings.rolling_resistance.max(0.0),
                settings: &settings,
                min_speed,
            };
            let mut jobs: Vec<Job> = Vec::new();
            let mut wanted: Vec<(RegionId, usize)> = here
                .iter()
                .enumerate()
                .map(|(k, &i)| (plans[i].rid, k))
                .collect();
            wanted.sort_unstable();
            let mut want = wanted.iter().peekable();
            for (id, r) in s.regions.iter_mut().enumerate() {
                let Some(&&(rid, k)) = want.peek() else {
                    break;
                };
                if id as RegionId != rid {
                    continue;
                }
                want.next();
                let i = here[k];
                jobs.push((r.as_mut().expect("live region"), &plans[i], &pending[k]));
            }
            // Back in the order of `here`.
            let mut order: Vec<usize> = wanted.iter().map(|w| w.1).collect();
            let mut outs = run_substeps(jobs, &shared);
            let mut sorted: Vec<Option<SubstepOut>> = (0..outs.len()).map(|_| None).collect();
            for (out, k) in outs.drain(..).zip(order.drain(..)) {
                sorted[k] = Some(out);
            }
            sorted
                .into_iter()
                .map(|o| o.expect("every job ran"))
                .collect()
        };
        // Contact events and joints, region by region (the shared books again).
        let _events = tracing::info_span!("contacts and joints").entered();
        for ((&i, out), (forces, kinematics)) in here.iter().zip(outs).zip(marks) {
            let p = &mut plans[i];
            let t4 = Instant::now();
            if settings.contact_events {
                sim.process_contacts(p.rid, &out.hits, &out.touched);
            } else {
                sim.region(p.rid).collector.0.lock().clear();
            }
            let t5 = Instant::now();
            break_joints(&mut sim, p.rid, p.sub_dt);
            let (events, joints) = (t5 - t4, t5.elapsed());
            let [cones, rapier, touches, rolling] = out.parts;
            let parts = [
                forces,
                kinematics + cones,
                rapier,
                touches,
                rolling,
                events,
                joints,
            ];
            p.ms += parts.iter().map(|d| d.as_secs_f64()).sum::<f64>() * 1000.0;
            if report {
                crate::report::substep_parts(parts);
                crate::report::count("spin pairs", out.spins);
                crate::report::count("hits", out.hits.len());
            }
        }
    }
    let finish = tracing::info_span!("characters and bounds").entered();
    for p in &plans {
        let t = Instant::now();
        sim.region_mut(p.rid).queries_refreshed();
        character::move_characters(&mut sim, p.rid, &gravity);
        sim.region_mut(p.rid).update_bounds();
        if report {
            let ms = p.ms + t.elapsed().as_secs_f64() * 1000.0;
            crate::report::region_ms(p.rid, ms);
        }
    }
    drop(finish);
    let write_back = tracing::info_span!("write back").entered();
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
                    yaw: GroundContact::yaw_of(i.forward),
                    load_kg: i.mass * (1.0 + dv / g),
                    speed: i.tangent_speed,
                    kind: ContactKind::Body,
                    surface: Some(i.other_surface),
                    entity: Some(i.body),
                });
            }
        }
    }

    drop(write_back);
    let _stats = tracing::info_span!("stats").entered();
    if report {
        crate::report::step_done(world, &sim, &foci, started);
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
        d.set("physics.substeps", stats.substeps as f64);
        d.set("physics.articulated", stats.articulated as f64);
        d.set("physics.static_copies", stats.static_copies as f64);
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

/// Steps between looks at the water under bodies ([`wake_for_water`]).
const WATER_LOOK_EVERY: u32 = 6;
/// A sleeping body wakes when the water at it rose or fell this far (m) since it fell asleep...
const WATER_WAKE_LEVEL: f64 = 0.02;
/// ...or its current changed this much (m/s).
const WATER_WAKE_CURRENT: f32 = 0.1;
/// A body afloat in a current faster than this (m/s) drifts with it and never falls asleep.
const WATER_CARRY: f32 = 0.02;
/// Water this far (m) below a body's bottom still counts as reaching it.
const WATER_REACH: f64 = 0.05;

/// Wake bodies the water moves. Rapier wakes a sleeping body only when another body touches it,
/// and the water is no body, so:
/// - a sleeping body wakes when the water reaching it rises, falls or starts to flow. Each look
///   compares the water with what it was when the body was first seen asleep, so a slow tide
///   adds up. A body the water stays clear of, or stays well over, sleeps on through waves.
/// - a body afloat in a current is carried along and must not fall asleep, though it may drift
///   slower than Rapier's sleep threshold: it is kept awake while it rests on nothing fixed.
fn wake_for_water(sim: &mut Simulation, water: &Water) {
    let Simulation {
        bodies,
        regions,
        resting,
        ..
    } = sim;
    for (e, entry) in bodies.iter_mut() {
        // (Held asleep whatever the water does.)
        if entry.kind != BodyKind::Dynamic || resting.contains(e) {
            continue;
        }
        let Some(region) = regions[entry.region as usize].as_mut() else {
            continue;
        };
        if region.frozen {
            continue;
        }
        let world = &mut region.world;
        // `get`, not `get_mut`: Rapier re-checks every body taken mutably.
        let Some(rb) = world.bodies.get(entry.handle) else {
            continue;
        };
        let sleeping = rb.is_sleeping();
        if !sleeping {
            entry.water_mark = None;
        }
        let anchor = region.anchor;
        let now = water
            .sample(anchor + rb.center_of_mass().as_dvec3())
            .map_or((f64::NEG_INFINITY, Vec3::ZERO), |s| {
                (s.surface_height, s.velocity)
            });
        let flowing = now.1.length() > WATER_CARRY;
        if !sleeping && !flowing {
            continue;
        }
        // No water here, now or when it fell asleep: nothing can reach the body, so its
        // bounds aren't needed (they visit every point of a hull).
        let then = entry.water_mark.map_or(f64::NEG_INFINITY, |m| m.0);
        if now.0 == f64::NEG_INFINITY && then == f64::NEG_INFINITY {
            if sleeping && entry.water_mark.is_none() {
                entry.water_mark = Some(now);
            }
            continue;
        }
        let (mut bottom, mut top) = (f32::INFINITY, f32::NEG_INFINITY);
        for &ch in rb.colliders() {
            if let Some(c) = world.colliders.get(ch)
                && !c.is_sensor()
            {
                let aabb = c.compute_aabb();
                bottom = bottom.min(aabb.mins.y);
                top = top.max(aabb.maxs.y);
            }
        }
        if bottom > top {
            continue;
        }
        let (bottom, top) = (anchor.y + bottom as f64, anchor.y + top as f64);
        let reaches = |level: f64| level > bottom - WATER_REACH;
        let over = |level: f64| level > top + WATER_REACH;
        let changed = sleeping
            && match entry.water_mark {
                None => {
                    entry.water_mark = Some(now);
                    false
                }
                Some(then) => {
                    (reaches(now.0) || reaches(then.0))
                        && (((now.0 - then.0).abs() > WATER_WAKE_LEVEL
                            && !(over(now.0) && over(then.0)))
                            || (now.1 - then.1).length() > WATER_WAKE_CURRENT)
                }
            };
        let carried = flowing && reaches(now.0) && !resting_on_fixed(world, rb);
        if changed || carried {
            entry.water_mark = None;
            if let Some(rb) = world.bodies.get_mut(entry.handle) {
                rb.wake_up(true);
            }
        }
    }
}

/// Does the body touch a fixed body or a static collider (a riverbed, a quay)?
fn resting_on_fixed(world: &PhysicsWorld, rb: &rapier3d::prelude::RigidBody) -> bool {
    rb.colliders().iter().any(|&ch| {
        world.narrow_phase.contact_pairs_with(ch).any(|pair| {
            let other = if pair.collider1 == ch {
                pair.collider2
            } else {
                pair.collider1
            };
            pair.has_any_active_contact()
                && world.colliders.get(other).is_some_and(|c| {
                    c.parent()
                        .and_then(|b| world.bodies.get(b))
                        .is_none_or(|b| !b.is_dynamic())
                })
        })
    })
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
    // The awake bodies and those with forces set: a sleeping body without forces has nothing
    // to do here, and there may be thousands of them (settled debris).
    let mut entities: Vec<Entity> = {
        let world = &sim.region(rid).world;
        world
            .islands
            .active_bodies()
            .filter_map(|h| world.bodies.get(h))
            .map(|rb| convert::entity_of(rb.user_data))
            .chain(sim.forced.iter().copied())
            .collect()
    };
    entities.sort_unstable();
    entities.dedup();
    // Colliders whose water was looked at (each looks at up to 26 points).
    let mut wetted = 0;
    for e in entities {
        let Some(entry) = sim.bodies.get_mut(&e) else {
            sim.forced.remove(&e);
            continue;
        };
        if entry.region != rid || entry.kind != BodyKind::Dynamic {
            continue;
        }
        let (h, external, gravity_scale, had_forces, k) = (
            entry.handle,
            entry.external,
            entry.desc.gravity_scale,
            entry.forces_set,
            entry.time_factor,
        );
        let region = sim.regions[rid as usize].as_mut().unwrap();
        let hulls = &mut sim.hulls;
        let anchor = region.anchor;
        // Far away the water is looked at on a coarser grid.
        let cells = if region.tier >= 1 {
            crate::buoyancy::FAR_CELLS
        } else {
            crate::buoyancy::MAX_CELLS
        };
        // Read only: Rapier's `get_mut` flags the body as changed, which costs a sleeping one
        // its place in Rapier's bookkeeping every substep.
        let Some(rb) = region.world.bodies.get(h) else {
            continue;
        };
        // A sleeping body moves with no force: one only for gravity's change across the region
        // (planets), or left from before it slept, waits until it wakes (gameplay's own forces
        // wake it, and stay here). Taking it mutably every substep isn't free.
        let pushed = external.0 != Vec3::ZERO || external.1 != Vec3::ZERO;
        if rb.is_sleeping() && !(had_forces && pushed) {
            sim.forced.remove(&e);
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
                let rho_g = WATER_DENSITY * g;
                let v = rb.linvel();
                let w_ang = rb.angvel();
                for ch in colliders {
                    let Some(c) = region.world.colliders.get(ch) else {
                        continue;
                    };
                    if c.is_sensor() {
                        continue;
                    }
                    wetted += 1;
                    let aabb = c.compute_aabb();
                    let height = (aabb.maxs.y - aabb.mins.y).max(1e-3);
                    // Lift (and its torque) from the pressure on the real shape where there
                    // is a mesh for it, else from the fraction of the box under water.
                    let hull = hulls.get(c.shared_shape());
                    let meshed = hull.is_some();
                    let (frac, current) = if let Some(hull) = hull {
                        let Some(surface) = Surface::sample(w, anchor, aabb.mins, aabb.maxs, cells)
                        else {
                            continue;
                        };
                        if aabb.mins.y >= surface.top {
                            continue;
                        }
                        let wetted = hull.immerse(
                            c.position(),
                            &surface,
                            g,
                            com,
                            v - surface.current,
                            w_ang,
                        );
                        if wetted.volume <= 0.0 {
                            continue;
                        }
                        force += wetted.force;
                        torque += wetted.torque;
                        ((wetted.volume / hull.volume).min(1.0), surface.current)
                    } else {
                        let bottom = anchor.y + aabb.mins.y as f64;
                        let depth = (s.surface_height - bottom) as f32;
                        let frac = (depth / height).clamp(0.0, 1.0);
                        if frac <= 0.0 {
                            continue;
                        }
                        let lift = up * (rho_g * c.volume() * frac);
                        let center = aabb.center();
                        let point =
                            Vec3::new(center.x, aabb.mins.y + height * frac * 0.5, center.z);
                        force += lift;
                        torque += (point - com).cross(lift);
                        (frac, s.velocity)
                    };
                    wet = true;
                    // Water drag relative to the current, and rotational drag.
                    let rel = v - current;
                    force -= rel * (mass * 0.8 * frac);
                    torque -= spin_drag(c, w_ang) * (mass * 0.3 * frac);
                    if !meshed {
                        // Bobbing dies out within a few periods (waves carry the energy
                        // away): heave damping at about half the critical value. Meshes damp
                        // heave, roll and pitch over their wetted bottom instead.
                        let area = (aabb.maxs.x - aabb.mins.x) * (aabb.maxs.z - aabb.mins.z);
                        let critical = 2.0 * (rho_g * area * mass).sqrt();
                        force -= up * (rel.dot(up) * critical * 0.5 * (frac * 2.0).min(1.0));
                    }
                }
            }
        }
        // All of these are pushes in the body's own seconds (water drag from its own-time
        // velocity, before `scale_time`): in world seconds they are `k²` as strong, like its
        // gravity.
        if (k - 1.0).abs() > 1e-3 {
            force *= k * k;
            torque *= k * k;
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
        let forced = needs || (wet && had_forces);
        sim.bodies.get_mut(&e).unwrap().forces_set = forced;
        if !forced {
            sim.forced.remove(&e);
        }
    }
    if wetted > 0 && crate::report::wanted() {
        crate::report::count("buoyant colliders", wetted);
    }
}

/// Water's drag on a collider turning at `spin`, per unit of the body's mass and wetness: about
/// each of the shape's own axes, as far as the shape reaches across that axis (a log rolls
/// through the water easily, end over end it sweeps its whole length).
fn spin_drag(c: &rapier3d::geometry::Collider, spin: Vec3) -> Vec3 {
    let h = c.shape().compute_local_aabb().half_extents();
    let reach = Vec3::new(
        h.y * h.y + h.z * h.z,
        h.x * h.x + h.z * h.z,
        h.x * h.x + h.y * h.y,
    );
    let rot = c.position().rotation;
    rot * (reach * 2.0 * (rot.inverse() * spin))
}

/// A body whose far edge moves faster than this from turning alone, m/s, gets its contacts
/// recomputed every step; below [`STILL_EDGE_SPEED`] Rapier may reuse them again.
const TURNING_EDGE_SPEED: f32 = 0.02;
const STILL_EDGE_SPEED: f32 = 0.01;
/// Dynamic bodies reaching less than this from their center of mass, m, never count as decks.
const DECK_REACH_M: f32 = 1.2;
/// A round collider whose rim moves faster than this from spinning on its axis, m/s, gets the
/// rolling correction of `ContactFilter::modify_solver_contacts`; below [`SLOW_ROLL_SPEED`] it
/// goes back to plain contacts. Slower spin makes too little of a false gap to matter.
const FAST_ROLL_SPEED: f32 = 1.0;
pub(crate) const SLOW_ROLL_SPEED: f32 = 0.5;
/// Dynamic bodies reaching less than this from their center of mass, m, get fresh contacts
/// every step while awake: Rapier's 5 cm of reuse is more than their own size.
const SMALL_REACH_M: f32 = 0.05;
/// A body faster than this, m/s, gets the hard stop of `ContactFilter::modify_solver_contacts`
/// for its impacts; below [`SLOW_IMPACT_SPEED`] it goes back to plain contacts.
pub(crate) const HARD_IMPACT_SPEED: f32 = 50.0;
const SLOW_IMPACT_SPEED: f32 = 25.0;

/// Give the colliders that need it the contact hook (`ContactFilter::modify_solver_contacts`),
/// and take it off again when they settle. Pairs with a hook are never recycled, and recycling
/// is worth keeping wherever it does no harm (about a tenth of a 1000-body pile's step time).
///
/// - Turning decks. Rapier reuses ("recycles") a contact pair while its bodies barely move
///   relative to each other, with the solver's lever arms frozen in world space until either
///   body has turned about 11°. Cargo riding a turning body (a turntable, a ship coming about)
///   never moves relative to it, so its friction pulls at points up to 11° behind and the cargo
///   creeps outward at about a tenth of the deck's speed until it slides off. The error grows
///   with the deck's size, so only bodies big enough to carry something count
///   ([`DECK_REACH_M`]): debris and crates tumbling in a settling pile keep the speed-up.
/// - Fast-rolling balls, barrels and wheels, whose contacts the hook corrects.
/// - Small bodies ([`SMALL_REACH_M`]). A reused contact keeps its old plane until the pair has
///   moved 5 cm, and a ball sliding along that plane rides on it as if it went on forever: a
///   14 mm Galton ball slid 6 cm off a 4 mm pin on air, and balls stayed wedged between pins
///   by planes of pins they had left 3 cm behind. Sleeping ones keep reusing (nothing moves).
/// - Bullet-fast bodies ([`HARD_IMPACT_SPEED`]), whose impacts the hook makes hard.
fn refresh_contact_hooks(world: &mut PhysicsWorld) {
    let PhysicsWorld {
        bodies, colliders, ..
    } = world;
    for (_, rb) in bodies.iter() {
        if rb.is_fixed() {
            continue;
        }
        let awake = !rb.is_sleeping();
        let spin = if awake { rb.angvel().length() } else { 0.0 };
        let speed = if awake { rb.linvel().length() } else { 0.0 };
        // A ball's radius from its inertia: close enough for any shape (a disc reads 1.1× its
        // radius, a rod 0.9× its half-length) and cheap for a thousand bodies a step.
        let reach = if rb.is_kinematic() {
            f32::INFINITY
        } else {
            let mass = rb.mass();
            let inertia = rb
                .mass_properties()
                .local_mprops
                .principal_inertia()
                .max_element();
            if mass > 0.0 {
                (2.5 * inertia / mass).sqrt()
            } else {
                0.0
            }
        };
        let edge_speed = if rb.is_kinematic() {
            // Gameplay turns these on purpose, and steadily: any turning counts.
            if spin > 1.0e-4 { f32::INFINITY } else { 0.0 }
        } else if reach < DECK_REACH_M {
            0.0
        } else {
            spin * reach
        };
        let small = reach < SMALL_REACH_M;
        for &h in rb.colliders() {
            // Look before touching: `get_mut` marks the collider modified, which costs the
            // broad phase work every step.
            let Some(c) = colliders.get(h) else {
                continue;
            };
            let roll_speed = if awake {
                crate::rolling::symmetric_spin(c, rb).map_or(0.0, |(w, _, r)| w.length() * r)
            } else {
                0.0
            };
            let hooks = c.active_hooks();
            let hooked = hooks.contains(ActiveHooks::MODIFY_SOLVER_CONTACTS);
            let wants = edge_speed > TURNING_EDGE_SPEED
                || roll_speed > FAST_ROLL_SPEED
                || speed > HARD_IMPACT_SPEED
                || (small && awake);
            let settled = edge_speed < STILL_EDGE_SPEED
                && roll_speed < SLOW_ROLL_SPEED
                && speed < SLOW_IMPACT_SPEED
                && !(small && awake);
            let hooks = if !hooked && wants {
                hooks | ActiveHooks::MODIFY_SOLVER_CONTACTS
            } else if hooked && settled {
                hooks - ActiveHooks::MODIFY_SOLVER_CONTACTS
            } else {
                continue;
            };
            if let Some(col) = colliders.get_mut(h) {
                col.set_active_hooks(hooks);
            }
        }
    }
}

/// Keep cargo riding a turning deck (a turntable, a carousel) on the circle it rides. The
/// solver's friction carries it at the deck's speed, but along the tangent where the step began,
/// so each step leaves it `r·(√(1+(ωΔt)²) − 1)` farther out: with one step a frame, a crate 3 m
/// out on a deck turning 0.4 rad/s crept 8 cm in 20 s, and cargo slides off a deck that turns
/// for minutes. PhysX's patch friction holds static contacts to anchors for the same reason;
/// here a crate held by friction (moving with the deck) is put back on its circle.
/// The kinematic bodies among `special` in region `rid`, for [`carry_on_decks`].
fn decks(sim: &Simulation, rid: RegionId, special: &[Entity]) -> Vec<RigidBodyHandle> {
    special
        .iter()
        .filter_map(|e| sim.bodies.get(e))
        .filter(|b| b.region == rid && b.kind == BodyKind::Kinematic)
        .map(|b| b.handle)
        .collect()
}

fn carry_on_decks(world: &mut PhysicsWorld, decks: &[RigidBodyHandle], dt: f32) {
    if decks.is_empty() {
        return;
    }
    let mut moves = Vec::new();
    for &deck in decks {
        let Some(d) = world.bodies.get(deck) else {
            continue;
        };
        let w = d.angvel();
        let spin = w.length();
        if spin < 1e-3 {
            continue;
        }
        let axis = w / spin;
        let (center, v0) = (d.center_of_mass(), d.linvel());
        let out_per_m = (1.0 + (spin * dt).powi(2)).sqrt() - 1.0;
        for &c in d.colliders() {
            for pair in world.narrow_phase.contact_pairs_with(c) {
                if !pair.has_any_active_contact() {
                    continue;
                }
                let other = if pair.collider1 == c {
                    pair.collider2
                } else {
                    pair.collider1
                };
                let Some(h) = world.colliders.get(other).and_then(|o| o.parent()) else {
                    continue;
                };
                let Some(rb) = world
                    .bodies
                    .get(h)
                    .filter(|rb| rb.is_dynamic() && !rb.is_sleeping())
                else {
                    continue;
                };
                let p = rb.center_of_mass();
                let r = p - center;
                let flat = r - axis * r.dot(axis);
                // Held by friction: moving with the deck, not sliding across it.
                let carried = v0 + w.cross(r);
                if (rb.linvel() - carried).length() > 0.1 || flat.length() < 1e-3 {
                    continue;
                }
                moves.push((h, -flat * out_per_m));
            }
        }
    }
    moves.sort_by_key(|m| m.0.into_raw_parts());
    moves.dedup_by_key(|m| m.0);
    for (h, by) in moves {
        if let Some(rb) = world.bodies.get_mut(h) {
            let mut pose = *rb.position();
            pose.translation += by;
            rb.set_position(pose, false);
        }
    }
}

/// Kinematic bodies moved by gameplay: those driven by `Velocity` advance by one substep, and
/// those put somewhere through their `Transform` go 1/`left` of the rest of the way there
/// (`left` substeps of this step remain, this one included).
fn drive_kinematics(
    sim: &mut Simulation,
    rid: RegionId,
    special: &[Entity],
    sub_dt: f32,
    left: u32,
) {
    // `Velocity` is in the body's own seconds: `time_factor` of it per world second.
    let moving: Vec<(rapier3d::prelude::RigidBodyHandle, Velocity)> = special
        .iter()
        .filter_map(|e| Some((e, sim.bodies.get(e)?)))
        .filter(|(e, b)| b.region == rid && b.kinematic_by_velocity && !sim.resting.contains(*e))
        .map(|(_, b)| {
            let v = b.written_velocity.unwrap_or_default();
            let k = b.time_factor;
            (
                b.handle,
                Velocity {
                    linear: v.linear * k,
                    angular: v.angular * k,
                },
            )
        })
        .collect();
    let gliding: Vec<(rapier3d::prelude::RigidBodyHandle, Pose)> = special
        .iter()
        .filter_map(|e| {
            let b = sim.bodies.get_mut(e).filter(|b| b.region == rid)?;
            let goal = if left <= 1 {
                b.kinematic_goal.take()
            } else {
                b.kinematic_goal
            };
            goal.map(|g| (b.handle, g))
        })
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
    for (h, goal) in gliding {
        if let Some(rb) = region.world.bodies.get_mut(h) {
            let next = if left <= 1 {
                goal
            } else {
                let (p, k) = (*rb.position(), 1.0 / left as f32);
                Pose::from_parts(
                    p.translation.lerp(goal.translation, k),
                    p.rotation.slerp(goal.rotation, k).normalize(),
                )
            };
            rb.set_next_kinematic_position(next);
        }
    }
}

/// The regions with something to move though their bodies sleep: a character that isn't
/// resting, or a kinematic body gliding to a goal.
fn busy_regions(sim: &Simulation) -> HashSet<RegionId> {
    let characters = sim
        .characters
        .iter()
        .filter(|(e, _)| !sim.resting.contains(*e))
        .map(|(_, c)| c.region);
    let gliding = sim
        .bodies
        .values()
        .filter(|b| b.kinematic_goal.is_some())
        .map(|b| b.region);
    characters.chain(gliding).collect()
}

/// A region (not [busy](busy_regions)) has nothing to step: every body in it asleep, none woken
/// since the last step (a wake-up shows on the body at once, though Rapier's islands learn of it
/// only as they step), no collider added or moved. Rapier's step would only walk its sleeping
/// contacts and pairs (3 ms for a resting proving ground), so it sits the step out, as Box2D and
/// PhysX leave sleeping islands out of the solver and the narrow phase. Bodies told to rest
/// ([`Simulation::resting`]) were put back to sleep before this.
fn idle(r: &crate::sim::Region) -> bool {
    // A dormant one is all asleep or held (`wake_dormant` saw to it this step), though Rapier's
    // active set may not know yet, and its changes wait for its first real step.
    if r.dormant {
        return true;
    }
    r.world.islands.active_bodies().next().is_none()
        && !r.needs_refresh
        && r.fresh.is_empty()
        && !r.stale_all
        && r.world
            .bodies
            .iter()
            .all(|(_, rb)| rb.is_fixed() || rb.is_sleeping())
}

/// Hold the bodies told to rest asleep ([`Simulation::resting`]), and those waiting for their
/// ground to load ([`Simulation::ungrounded`]): wake-ups from their
/// neighbours, from gameplay writing to them, or bodies made since they were told, go back to
/// sleep, still. Every one is looked at, not only Rapier's active ones: a body woken since the
/// last step joins them only as the next one starts, and wakes all it touches.
fn hold_resting(sim: &mut Simulation) {
    let Simulation {
        resting,
        ungrounded,
        bodies,
        characters,
        regions,
        ..
    } = sim;
    let mut held = 0;
    for e in resting.iter().chain(ungrounded.iter()) {
        // (A character told to rest stands still: its body sleeps too.)
        let Some((region, handle)) = bodies
            .get(e)
            .map(|b| (b.region, b.handle))
            .or_else(|| characters.get(e).map(|c| (c.region, c.body)))
        else {
            continue;
        };
        let Some(r) = regions.get_mut(region as usize).and_then(|r| r.as_mut()) else {
            continue;
        };
        // `get`, not `get_mut`: Rapier re-checks every body taken mutably.
        if r.world.bodies.get(handle).is_none_or(|rb| rb.is_sleeping()) {
            continue;
        }
        if let Some(rb) = r.world.bodies.get_mut(handle) {
            rb.set_linvel(Vec3::ZERO, false);
            rb.set_angvel(Vec3::ZERO, false);
            rb.sleep();
            held += 1;
        }
    }
    if held > 0 && crate::report::wanted() {
        crate::report::count("resting bodies put back to sleep", held);
    }
}

/// Kinematic bodies put somewhere through their `Transform` in a region that doesn't step
/// (asleep, out of reach or paused): place them there now.
fn place_kinematics(sim: &mut Simulation, rid: RegionId) {
    let placed: Vec<(rapier3d::prelude::RigidBodyHandle, Pose)> = sim
        .bodies
        .values_mut()
        .filter(|b| b.region == rid)
        .filter_map(|b| b.kinematic_goal.take().map(|g| (b.handle, g)))
        .collect();
    if placed.is_empty() {
        return;
    }
    let region = sim.region_mut(rid);
    for (h, goal) in placed {
        if let Some(rb) = region.world.bodies.get_mut(h) {
            rb.set_position(goal, true);
        }
    }
    region.needs_refresh = true;
}

/// Bodies with their own time rate (`LocalTimeScale`): scale velocity and gravity so they move
/// `factor`× as fast as the world for one substep (`apply = true`), then undo it.
/// A body with its own time rate: its handle, rate, gravity scale and damping.
type Scaled = (RigidBodyHandle, f32, f32, [f32; 2]);

/// The bodies among `special` in region `rid` with their own time rate, for [`scale_time`].
fn time_scaled(sim: &Simulation, rid: RegionId, special: &[Entity]) -> Vec<Scaled> {
    special
        .iter()
        .filter_map(|e| sim.bodies.get(e))
        .filter(|b| {
            b.region == rid && b.kind == BodyKind::Dynamic && (b.time_factor - 1.0).abs() > 1e-3
        })
        .map(|b| {
            let damping = [
                convert::damping(b.desc.linear_damping),
                convert::damping(b.desc.angular_damping),
            ];
            (b.handle, b.time_factor, b.desc.gravity_scale, damping)
        })
        .collect()
}

fn scale_time(world: &mut PhysicsWorld, scaled: &[Scaled], apply: bool) {
    for &(h, k, gs, [lin, ang]) in scaled {
        let Some(rb) = world.bodies.get_mut(h) else {
            continue;
        };
        let k = k.max(1e-3);
        // Damping is per second of the body's own time: `k` times as fast per world second.
        let (m, gscale, d) = if apply {
            (k, gs * k * k, k)
        } else {
            (1.0 / k, gs, 1.0)
        };
        let (v, w) = (rb.linvel(), rb.angvel());
        rb.set_linvel(v * m, false);
        rb.set_angvel(w * m, false);
        rb.set_gravity_scale(gscale, false);
        rb.set_linear_damping(lin * d);
        rb.set_angular_damping(ang * d);
    }
}

/// A region stepping this frame: its substeps and what they need, found once.
struct Plan {
    rid: RegionId,
    substeps: u32,
    sub_dt: f32,
    /// Kinematic and time-scaled bodies.
    special: Vec<Entity>,
    scaled: Vec<Scaled>,
    decks: Vec<RigidBodyHandle>,
    cones: Vec<crate::cones::Cone>,
    /// Time its step took so far, ms.
    ms: f64,
}

/// What every region's substep job reads.
struct SubstepShared<'a> {
    no_contact: &'a HashMap<(Entity, Entity), u32>,
    materials: &'a crate::sim::MaterialCache,
    rolling: f32,
    settings: &'a crate::PhysicsSettings,
    min_speed: Option<f32>,
}

/// What a region's substep job leaves for the passes after it.
struct SubstepOut {
    hits: Vec<(ColliderHandle, ColliderHandle, f32)>,
    touched: Vec<crate::sim::Touched>,
    spins: usize,
    /// Cones and time rates, Rapier, touches, rolling (and cone friction).
    parts: [Duration; 4],
}

/// One substep of one region, touching nothing but the region.
fn substep(
    r: &mut crate::sim::Region,
    p: &Plan,
    pending: &HashSet<(ColliderHandle, ColliderHandle)>,
    shared: &SubstepShared,
) -> SubstepOut {
    let t0 = Instant::now();
    crate::cones::orient(&mut r.world, &p.cones);
    scale_time(&mut r.world, &p.scaled, true);
    let t1 = Instant::now();
    {
        let _rapier = tracing::info_span!("rapier step").entered();
        let filter = ContactFilter {
            no_contact: shared.no_contact,
            dt: p.sub_dt,
        };
        r.world.integration_parameters.dt = p.sub_dt;
        r.world.integration_parameters.num_solver_iterations =
            crate::fine::iterations(shared.settings, r.fine, p.sub_dt);
        let collector = std::mem::take(&mut r.collector);
        crate::articulation::before_step(&mut r.world, &r.links);
        if shared.settings.contact_events {
            r.note_velocities();
        }
        r.world.step_with_events(&filter, &collector);
        crate::articulation::after_step(&mut r.world, &mut r.links);
        r.collector = collector;
        r.needs_refresh = false;
    }
    scale_time(&mut r.world, &p.scaled, false);
    carry_on_decks(&mut r.world, &p.decks, p.sub_dt);
    let t2 = Instant::now();
    let touches = r.touches(
        p.sub_dt,
        shared.min_speed,
        (shared.rolling, shared.materials),
        pending,
    );
    let t3 = Instant::now();
    crate::rolling::resist_rolling(&mut r.world, touches.pushes);
    crate::cones::rub(&mut r.world, &p.cones);
    let t4 = Instant::now();
    SubstepOut {
        hits: touches.hits,
        touched: touches.touched,
        spins: touches.spins,
        parts: [t1 - t0, t2 - t1, t3 - t2, t4 - t3],
    }
}

/// A region's substep job: the region, its plan and its pairs waiting to be confirmed.
type Job<'a> = (
    &'a mut crate::sim::Region,
    &'a Plan,
    &'a HashSet<(ColliderHandle, ColliderHandle)>,
);

/// Run the regions' substep `jobs` side by side on the compute pool (in place when there is
/// one, or no pool); the results come back in the jobs' order.
fn run_substeps(jobs: Vec<Job>, shared: &SubstepShared) -> Vec<SubstepOut> {
    let pool = engine_core::tasks::ComputeTaskPool::try_get();
    match pool {
        Some(pool) if jobs.len() > 1 => pool.scope(|scope| {
            for (r, p, pending) in jobs {
                scope.spawn(async move { substep(r, p, pending, shared) });
            }
        }),
        _ => jobs
            .into_iter()
            .map(|(r, p, pending)| substep(r, p, pending, shared))
            .collect(),
    }
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
            (r == rid).then_some((*e, h.impulse()?, j.desc.break_force?))
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
        // A resting region didn't step: its bodies are where they were written.
        if region.tier >= crate::lod::REST && b.written.is_some() {
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
        // A half-rate region sitting this frame out: draw its bodies moving on.
        let ahead = region.owed_secs;
        let (translation, rotation) = if ahead > 0.0 && !sleeping {
            (
                p.translation + velocity.linear * ahead,
                (Quat::from_scaled_axis(velocity.angular * ahead) * p.rotation).normalize(),
            )
        } else {
            (p.translation, p.rotation)
        };
        writes.push(BodyWrite {
            entity: *e,
            translation: region.global(translation),
            rotation,
            velocity,
        });
    }
    sim.writes = writes;
}
