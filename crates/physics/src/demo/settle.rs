//! Pre-settled stations. A station nobody has come near rests at once, held asleep as built
//! ([`super::replay_stations`]), but some are built mid-air: the pile's crates hang in a grid
//! above their bin, which reads as a bug from across the valley. Like the pre-settled props
//! games bake, a resting station's loose bodies are dropped once in a private Rapier world on
//! a worker thread, and held asleep where they came to rest.
//!
//! Everything else in the yard (scenery, jointed and kinematic bodies, other stations) stands
//! still in that world. Angular damping stands in for rolling resistance, so drums and balls
//! come to rest instead of rolling to the end of the settle.

use std::thread::JoinHandle;

use engine_core::ecs::entity::EntityHashSet;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::Gravity;
use engine_world::physics::{BodyKind, Collider, Joint, RigidBody};
use rapier3d::prelude::{ColliderBuilder, PhysicsWorld, Pose, RigidBodyBuilder};

use super::YARD_SIZE;

/// Longest settle, seconds of simulated time (the pile is down in about three).
const SETTLE_SECS: f32 = 6.0;

/// A settled body: its entity, how far it moved and its new rotation.
type Settled = Vec<(Entity, DVec3, Quat)>;

/// Settles running on workers.
#[derive(Resource, Default)]
pub(crate) struct Settling(Vec<JoinHandle<Settled>>);

impl Settling {
    /// Has a settle finished, for [`finish`] to apply?
    pub(crate) fn any_done(&self) -> bool {
        self.0.iter().any(|h| h.is_finished())
    }
}

/// The nearest entity at or above `e` (including `e`) with a `RigidBody`.
fn owner(world: &World, e: Entity) -> Option<Entity> {
    let mut cur = e;
    loop {
        if world.get::<RigidBody>(cur).is_some() {
            return Some(cur);
        }
        cur = world.get::<ChildOf>(cur)?.parent();
    }
}

/// Drop `bodies` (a resting station's) of the proving ground at `origin` on a worker; their
/// loose dynamic ones move to where they come to rest once it is done ([`finish`]).
pub(crate) fn start(world: &mut World, origin: DVec3, bodies: &[Entity]) {
    let jointed: EntityHashSet = world
        .query::<&Joint>()
        .iter(world)
        .flat_map(|j| [Some(j.body_a), j.body_b])
        .flatten()
        .collect();
    let loose: EntityHashSet = bodies
        .iter()
        .copied()
        .filter(|e| {
            world
                .get::<RigidBody>(*e)
                .is_some_and(|rb| rb.kind == BodyKind::Dynamic)
                && world.get::<ChildOf>(*e).is_none()
                && !jointed.contains(e)
        })
        .collect();
    if loose.is_empty() {
        return;
    }
    let reach = DVec3::new(YARD_SIZE[0] * 0.5 + 10.0, 40.0, YARD_SIZE[1] * 0.5 + 10.0);
    let near = |p: DVec3| ((p - origin).abs() - reach).max_element() <= 0.0;
    // Every collider in the yard, with its pose and the loose body it moves with (if any).
    let mut parts = Vec::new();
    let colliders: Vec<(Entity, Collider)> = world
        .query::<(Entity, &Collider)>()
        .iter(world)
        .filter(|(_, c)| !c.sensor)
        .map(|(e, c)| (e, c.clone()))
        .collect();
    for (e, c) in colliders {
        let Some(pose) = crate::step::world_pose(world, e) else {
            continue;
        };
        if !near(pose.translation) {
            continue;
        }
        let Some(shape) = crate::convert::shape(&c.shape, pose.scale) else {
            continue;
        };
        let body = owner(world, e).filter(|b| loose.contains(b));
        parts.push((body, pose, shape, c.density));
    }
    let mut movers = Vec::new();
    for &e in &loose {
        if let Some(pose) = crate::step::world_pose(world, e) {
            movers.push((e, pose.translation, pose.rotation));
        }
    }
    let g = world
        .get_resource::<Gravity>()
        .map_or(Vec3::new(0.0, -9.81, 0.0), |g| g.at(origin));
    let job = move || settle(origin, g, movers, parts);
    let spawned = std::thread::Builder::new()
        .name("station settle".into())
        .spawn(job.clone());
    match spawned {
        Ok(handle) => world
            .get_resource_or_insert_with(Settling::default)
            .0
            .push(handle),
        // No threads (or none left): settle here, behind the loading screen.
        Err(_) => apply(world, job()),
    }
}

type Part = (
    Option<Entity>,
    Transform,
    rapier3d::prelude::SharedShape,
    f32,
);

fn settle(
    origin: DVec3,
    gravity: Vec3,
    movers: Vec<(Entity, DVec3, Quat)>,
    parts: Vec<Part>,
) -> Settled {
    let started = std::time::Instant::now();
    let local = |p: DVec3| (p - origin).as_vec3();
    let mut w = PhysicsWorld::new();
    w.gravity = gravity;
    w.integration_parameters.dt = 1.0 / 60.0;
    w.integration_parameters.num_solver_iterations = 8;
    let ground = w.insert_body(RigidBodyBuilder::fixed());
    let mut handles = std::collections::HashMap::new();
    // (Masses come from the colliders' densities: how heavy each crate is hardly changes
    // where it lands.)
    for (e, at, rot) in movers {
        let b = RigidBodyBuilder::dynamic()
            .pose(Pose::from_parts(local(at), rot))
            .linear_damping(0.1)
            .angular_damping(1.0);
        handles.insert(e, (w.insert_body(b), at, rot));
    }
    for (body, pose, shape, density) in parts {
        let (parent, at) = match body.and_then(|b| handles.get(&b)) {
            Some((h, at, rot)) => {
                let rel = Pose::from_parts(
                    rot.inverse() * (pose.translation - *at).as_vec3(),
                    rot.inverse() * pose.rotation,
                );
                (*h, rel)
            }
            None => (
                ground,
                Pose::from_parts(local(pose.translation), pose.rotation),
            ),
        };
        let c = ColliderBuilder::new(shape)
            .position(at)
            .friction(0.7)
            .density(density.max(1.0));
        w.insert_collider(c, Some(parent));
    }
    let mut steps = 0;
    while steps < (SETTLE_SECS * 60.0) as usize {
        w.step();
        steps += 1;
        if steps % 30 == 0 && w.islands.active_bodies().next().is_none() {
            break;
        }
    }
    if crate::report::wanted() {
        engine_core::log::info!(
            "proving ground: settled {} bodies off the frame in {:.0} ms ({:.1} s of drop)",
            handles.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            steps as f32 / 60.0
        );
    }
    handles
        .into_iter()
        .filter_map(|(e, (h, at, _))| {
            let rb = w.bodies.get(h)?;
            let p = rb.position();
            let moved = origin + p.translation.as_dvec3() - at;
            Some((e, moved, p.rotation))
        })
        .collect()
}

/// Move the bodies of finished settles to where they came to rest.
pub(crate) fn finish(world: &mut World) {
    let Some(mut s) = world.get_resource_mut::<Settling>() else {
        return;
    };
    if s.0.is_empty() {
        return;
    }
    let (done, running): (Vec<_>, Vec<_>) = std::mem::take(&mut s.0)
        .into_iter()
        .partition(|h| h.is_finished());
    s.0 = running;
    for h in done {
        if let Ok(settled) = h.join() {
            apply(world, settled);
        }
    }
}

fn apply(world: &mut World, settled: Settled) {
    // (A station rebuilt meanwhile despawned these.)
    for (e, moved, rotation) in settled {
        if let Some(mut t) = world.get_mut::<Transform>(e) {
            t.translation += moved;
            t.rotation = rotation;
        }
    }
}
