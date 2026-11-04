//! Static mesh shapes built off the frame.
//!
//! A static triangle mesh costs about 1 ms per 1,000 vertices to build (parry's BVH, plus
//! merging duplicate vertices and fixing internal edges): the scenery streaming in around the
//! fortress took 5-36 ms of a step, several times a second (round 6 on the VM). Nothing touches
//! a static collider until something moving comes near it, so a big static mesh is built on
//! threads of its own and placed when it is ready, as Unreal cooks collision off the game thread and
//! Jolt builds shapes on job threads ahead of `AddBody`.
//!
//! The guard: an awake body or a character near a static still being built waits for it,
//! held where it is (asleep and weightless, as bodies waiting for their ground are, or a
//! character standing still), and its job goes to the front of the queue. A job is finished on
//! the spot (waited for, or built here if no worker has taken it yet) once it has held
//! something [`MAX_HOLD_STEPS`] steps, or after [`MAX_WAIT_STEPS`] steps whatever is near. So
//! no body ever meets a missing static, a query for one waits half a second at most, and a
//! mesh streaming in under the player (a teleport, a respawn, a zone loading around a camera
//! stop) costs a few steps of standing still instead of 40-200 ms of one frame: rounds 7-8
//! on the VM built a 50,000 vertex mesh on the spot under the loops' player, 44-208 ms each
//! time. Streaming engines hold the player the same way while what is under them loads (Unreal
//! waits for streaming before a teleported pawn moves on; Jolt adds a body only once its shape
//! is built). A collider whose shape changes (a voxel cliff's chunk remeshed where it was dug)
//! keeps its old shape while the new one builds, even with something moving near, for
//! [`MAX_REPLACE_STEPS`] steps at most.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine_core::ecs::entity::{Entity, EntityHashMap};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_world::physics::{Collider, ColliderShape};
use parking_lot::{Condvar, Mutex};
use rapier3d::prelude::SharedShape;

/// Vertices a static mesh (or hull) needs before it is built off the frame: smaller ones cost
/// less than a hand-off. A hull costs about 1 µs a point, and zones streaming in bring dozens
/// of 150-500 point hulls at once (14,000-25,000 points, 17-21 ms of one sync on the VM loops,
/// round 10), so from 128.
pub(crate) const OFF_FRAME_POINTS: usize = 128;
/// Steps a job may wait before it is finished on the spot, so a ray or overlap (an NPC's sight
/// line through a building just streamed in) misses it half a second at most. On their own
/// threads jobs take milliseconds, so this is only reached when those threads fall behind.
pub(crate) const MAX_WAIT_STEPS: u32 = 30;
/// Steps the old shape of a collider changed near something moving stands in for the new one
/// (a voxel cliff's chunk remeshed as debris falls off it) before the new one is finished on
/// the spot.
pub(crate) const MAX_REPLACE_STEPS: u32 = 3;
/// Steps a job may hold what is near it (bodies and characters waiting for it) before it is
/// finished on the spot: a tenth of a second at 60 Hz, as long as a 50,000 vertex mesh takes
/// on a shape thread (about 1 ms per 1,000 vertices).
pub(crate) const MAX_HOLD_STEPS: u32 = 6;
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
    /// Something waits for it: the shape threads take it before the others.
    urgent: AtomicBool,
}

/// Jobs not yet taken by a shape thread, oldest first. Each queued job spawns one task, and a
/// task takes the first urgent job, else the oldest, so the counts always match.
fn queue() -> &'static Mutex<VecDeque<Arc<Job>>> {
    static QUEUE: std::sync::OnceLock<Mutex<VecDeque<Arc<Job>>>> = std::sync::OnceLock::new();
    QUEUE.get_or_init(Default::default)
}

/// Can shapes be built off the frame here? (Not on a machine of one thread.)
fn threaded() -> bool {
    static THREADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *THREADED.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()) >= 2)
}

impl Job {
    /// Queue `shape` at `scale` on the shape threads (`urgent`: ahead of the others), or `None`
    /// on a machine of one thread (built on the spot).
    pub(crate) fn spawn(shape: ColliderShape, scale: Vec3, urgent: bool) -> Option<Arc<Self>> {
        if !threaded() {
            return None;
        }
        let job = Arc::new(Self {
            state: Mutex::new(State::Queued(shape, scale)),
            done: Condvar::new(),
            urgent: AtomicBool::new(urgent),
        });
        queue().lock().push_back(job.clone());
        pool()
            .spawn(async move {
                let next = {
                    let mut q = queue().lock();
                    let i = q
                        .iter()
                        .position(|j| j.urgent.load(Ordering::Relaxed))
                        .unwrap_or(0);
                    q.remove(i)
                };
                if let Some(job) = next {
                    job.run();
                }
            })
            .detach();
        Some(job)
    }

    /// Build it here, on a shape thread, unless it was finished on the spot or dropped.
    fn run(&self) {
        let (shape, scale) = {
            let mut s = self.state.lock();
            match std::mem::replace(&mut *s, State::Building) {
                State::Queued(shape, scale) => (shape, scale),
                other => {
                    *s = other;
                    return;
                }
            }
        };
        let built = crate::convert::shape_cached(&shape, scale);
        let mut s = self.state.lock();
        if matches!(*s, State::Building) {
            *s = State::Done(built);
        }
        self.done.notify_all();
    }

    /// Something waits for it: build it before the jobs nothing waits for.
    pub(crate) fn hurry(&self) {
        self.urgent.store(true, Ordering::Relaxed);
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
    /// Steps it has held something near it waiting ([`MAX_HOLD_STEPS`]).
    pub held: u32,
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

/// A dynamic body or a character that may come near a static being built.
pub(crate) struct Mover {
    pub entity: Entity,
    pub at: DVec3,
    /// How far it reaches (m), with how far it travels in [`NEAR_SECS`].
    pub reach: f64,
}

impl Mover {
    /// Does it come near the box `lo..hi`?
    pub(crate) fn near(&self, lo: DVec3, hi: DVec3) -> bool {
        let pad = DVec3::splat(self.reach + NEAR_M);
        self.at.cmpge(lo - pad).all() && self.at.cmple(hi + pad).all()
    }
}

/// Do any of `movers` come near the box `lo..hi`?
pub(crate) fn near(movers: &[Mover], lo: DVec3, hi: DVec3) -> bool {
    movers.iter().any(|m| m.near(lo, hi))
}

/// Awake dynamic bodies, those held for a shape ([`crate::sim::Simulation::held_for_shapes`],
/// asleep only while held), and characters.
pub(crate) fn movers(sim: &crate::sim::Simulation) -> Vec<Mover> {
    let mut out = Vec::new();
    for (e, b) in sim.bodies.iter() {
        let Some(r) = sim.regions.get(b.region as usize).and_then(|r| r.as_ref()) else {
            continue;
        };
        let Some(rb) = r.world.bodies.get(b.handle) else {
            continue;
        };
        if !rb.is_dynamic() || (rb.is_sleeping() && !sim.held_for_shapes.contains_key(e)) {
            continue;
        }
        let travel = rb.linvel().length() as f64 * NEAR_SECS;
        out.push(Mover {
            entity: *e,
            at: r.global(rb.translation()),
            reach: b.reach as f64 + travel,
        });
    }
    for (e, c) in sim.characters.iter() {
        let Some(r) = sim.regions.get(c.region as usize).and_then(|r| r.as_ref()) else {
            continue;
        };
        if let Some(rb) = r.world.bodies.get(c.body) {
            let travel = rb.linvel().length() as f64 * NEAR_SECS;
            out.push(Mover {
                entity: *e,
                at: r.global(rb.translation()),
                reach: 2.0 + travel,
            });
        }
    }
    out
}

/// Entities whose static shape is still being built.
pub(crate) type PendingStatics = EntityHashMap<Pending>;
