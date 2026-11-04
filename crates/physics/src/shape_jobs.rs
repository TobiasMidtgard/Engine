//! Static mesh shapes built off the frame.
//!
//! A static triangle mesh costs about 1 ms per 1,000 vertices to build (parry's BVH, plus
//! merging duplicate vertices and fixing internal edges): the scenery streaming in around the
//! fortress took 5-36 ms of a step, several times a second (round 6 on the VM). Nothing touches
//! a static collider until something moving comes near it, so a big static mesh is built on
//! threads of its own and placed when it is ready, as Unreal cooks collision off the game thread and
//! Jolt builds shapes on job threads ahead of `AddBody`.
//!
//! The guard: a job is finished on the spot (waited for, or built here if no worker has taken
//! it yet) the step an awake body or a character comes near it, or after [`MAX_WAIT_STEPS`]
//! steps whatever is near. So no body ever meets a missing static, and a query for one waits
//! half a second at most. A collider whose shape changes (a voxel cliff's chunk remeshed where
//! it was dug) keeps its old shape while the new one builds, even with something moving near,
//! for [`MAX_REPLACE_STEPS`] steps at most.

use std::sync::Arc;

use engine_core::ecs::entity::EntityHashMap;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_world::physics::{Collider, ColliderShape};
use parking_lot::{Condvar, Mutex};
use rapier3d::prelude::SharedShape;

/// Vertices a static mesh (or hull) needs before it is built off the frame: smaller ones cost
/// less than a hand-off.
pub(crate) const OFF_FRAME_POINTS: usize = 512;
/// Steps a job may wait before it is finished on the spot, so a ray or overlap (an NPC's sight
/// line through a building just streamed in) misses it half a second at most. On their own
/// threads jobs take milliseconds, so this is only reached when those threads fall behind.
pub(crate) const MAX_WAIT_STEPS: u32 = 30;
/// Steps the old shape of a collider changed near something moving stands in for the new one
/// (a voxel cliff's chunk remeshed as debris falls off it) before the new one is finished on
/// the spot.
pub(crate) const MAX_REPLACE_STEPS: u32 = 3;
/// A job is finished when an awake body comes within this distance (m) of its bounds, plus
/// how far the body travels in [`NEAR_SECS`]. It is looked at every step, before any region
/// steps, so the margin needs to cover only a few steps of travel.
pub(crate) const NEAR_M: f64 = 2.0;
pub(crate) const NEAR_SECS: f64 = 0.15;

/// The threads static shapes are built on: two of their own, so a zone's art queued on the
/// async pool (seconds of it while loading) doesn't hold them up.
fn pool() -> &'static engine_core::tasks::TaskPool {
    static POOL: std::sync::OnceLock<engine_core::tasks::TaskPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        engine_core::tasks::TaskPoolBuilder::new()
            .num_threads(2)
            .thread_name("physics shapes".into())
            .build()
    })
}

enum State {
    Queued(ColliderShape, Vec3),
    Building,
    Done(Option<SharedShape>),
    Taken,
}

/// A shape being built on a worker.
pub(crate) struct Job {
    state: Mutex<State>,
    done: Condvar,
}

impl Job {
    /// Queue `shape` at `scale` on the shape threads, or `None` on a machine of one thread
    /// (built on the spot).
    pub(crate) fn spawn(shape: ColliderShape, scale: Vec3) -> Option<Arc<Self>> {
        if std::thread::available_parallelism().map_or(1, |n| n.get()) < 2 {
            return None;
        }
        let pool = pool();
        let job = Arc::new(Self {
            state: Mutex::new(State::Queued(shape, scale)),
            done: Condvar::new(),
        });
        let worker = job.clone();
        pool.spawn(async move {
            let (shape, scale) = {
                let mut s = worker.state.lock();
                match std::mem::replace(&mut *s, State::Building) {
                    State::Queued(shape, scale) => (shape, scale),
                    // Finished on the spot or dropped before a worker got to it.
                    other => {
                        *s = other;
                        return;
                    }
                }
            };
            let built = crate::convert::shape_cached(&shape, scale);
            let mut s = worker.state.lock();
            if matches!(*s, State::Building) {
                *s = State::Done(built);
            }
            worker.done.notify_all();
        })
        .detach();
        Some(job)
    }

    /// The shape if it is built (`Some(None)`: it can't be built).
    pub(crate) fn poll(&self) -> Option<Option<SharedShape>> {
        let mut s = self.state.lock();
        if matches!(*s, State::Done(_)) {
            let State::Done(shape) = std::mem::replace(&mut *s, State::Taken) else {
                unreachable!()
            };
            return Some(shape);
        }
        None
    }

    /// The shape, now: waited for if a worker is building it, else built here.
    pub(crate) fn finish(&self) -> Option<SharedShape> {
        let mut s = self.state.lock();
        loop {
            match std::mem::replace(&mut *s, State::Taken) {
                State::Queued(shape, scale) => {
                    drop(s);
                    return crate::convert::shape_cached(&shape, scale);
                }
                State::Building => {
                    *s = State::Building;
                    self.done.wait(&mut s);
                }
                State::Done(shape) => return shape,
                State::Taken => return None,
            }
        }
    }

    /// Forget it (a worker that hasn't started skips it).
    pub(crate) fn cancel(&self) {
        let mut s = self.state.lock();
        if !matches!(*s, State::Building) {
            *s = State::Taken;
        }
    }
}

/// A static collider whose shape is being built ([`crate::sim::Simulation::pending_statics`]).
pub(crate) struct Pending {
    pub job: Arc<Job>,
    pub desc: Collider,
    pub scale: Vec3,
    /// World bounds of the shape at the pose it was queued at.
    pub lo: DVec3,
    pub hi: DVec3,
    pub waited: u32,
    /// Queued though something moving is near, because an older shape of the collider stands
    /// in for it: finished after [`MAX_REPLACE_STEPS`] at most.
    pub replacing: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.job.cancel();
    }
}

/// Points a shape is built from, if it is one built off the frame (a mesh or a hull).
pub(crate) fn heavy_points(shape: &ColliderShape) -> Option<&[Vec3]> {
    match shape {
        ColliderShape::TriMesh { vertices, .. } if vertices.len() >= OFF_FRAME_POINTS => {
            Some(vertices)
        }
        ColliderShape::ConvexHull { points } if points.len() >= OFF_FRAME_POINTS => Some(points),
        _ => None,
    }
}

/// World bounds of `points` scaled, turned and placed by a pose.
pub(crate) fn world_bounds(
    points: &[Vec3],
    translation: DVec3,
    rotation: Quat,
    scale: Vec3,
) -> (DVec3, DVec3) {
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for p in points {
        let q = rotation * (*p * scale);
        lo = lo.min(q);
        hi = hi.max(q);
    }
    (translation + lo.as_dvec3(), translation + hi.as_dvec3())
}

/// Do any of `movers` (world position, reach in m) come near the box `lo..hi`?
pub(crate) fn near(movers: &[(DVec3, f64)], lo: DVec3, hi: DVec3) -> bool {
    movers.iter().any(|(p, r)| {
        let pad = DVec3::splat(*r + NEAR_M);
        p.cmpge(lo - pad).all() && p.cmple(hi + pad).all()
    })
}

/// Awake dynamic bodies and characters: where they are and how far they reach in
/// [`NEAR_SECS`].
pub(crate) fn movers(sim: &crate::sim::Simulation) -> Vec<(DVec3, f64)> {
    let mut out = Vec::new();
    for b in sim.bodies.values() {
        let Some(r) = sim.regions.get(b.region as usize).and_then(|r| r.as_ref()) else {
            continue;
        };
        let Some(rb) = r.world.bodies.get(b.handle) else {
            continue;
        };
        if !rb.is_dynamic() || rb.is_sleeping() {
            continue;
        }
        let travel = rb.linvel().length() as f64 * NEAR_SECS;
        out.push((r.global(rb.translation()), b.reach as f64 + travel));
    }
    for c in sim.characters.values() {
        let Some(r) = sim.regions.get(c.region as usize).and_then(|r| r.as_ref()) else {
            continue;
        };
        if let Some(rb) = r.world.bodies.get(c.body) {
            let travel = rb.linvel().length() as f64 * NEAR_SECS;
            out.push((r.global(rb.translation()), 2.0 + travel));
        }
    }
    out
}

/// Entities whose static shape is still being built.
pub(crate) type PendingStatics = EntityHashMap<Pending>;
