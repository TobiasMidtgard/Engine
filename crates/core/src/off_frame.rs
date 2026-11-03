//! Off-frame builds: make art on a worker, put it in the world on the frame in small,
//! ordered slices.
//!
//! A module that spawns something costly (a zone's meshes, a baked texture set, a layout)
//! almost always has the same shape: `build()` makes data and touches no world state, then
//! `apply(world, data)` adds assets and spawns. Run inside a command, the build holds the main
//! thread for its whole length. [`OffFrameCommands::off_frame`] (or [`off_frame`] from code
//! that already has the world) splits the two:
//!
//! ```ignore
//! commands.off_frame("vfx prop court.floor", move || build(), |world, prop| place(world, prop));
//! ```
//!
//! The build runs on [`AsyncComputeTaskPool`]. The apply runs on the main thread from
//! [`apply_off_frame_system`] (in `First`), in queue order (only the finished prefix is
//! applied, so spawn order never depends on thread timing) and within
//! [`OffFrameSettings::apply_ms`] a frame. This is the shape Unity's staging worlds
//! (`ExclusiveEntityTransaction` then `MoveEntitiesFrom`) and Unreal's async loading
//! (`s.AsyncLoadingTimeLimit` for the game-thread finish) share: build where the world isn't,
//! integrate on the frame within a budget.
//!
//! **Scopes.** A job remembers the scope current when it was queued ([`set_scope`]). The
//! frame system applies only unscoped jobs; a scope's jobs are applied by whoever set it,
//! with [`apply_finished`]. The streamer scopes each cell's spawn so the cell's art lands
//! inside its load (its members and assets are counted, and it unloads with the cell), and
//! [`drop_scope`] forgets a cell's jobs when it goes before they are in.
//!
//! **Inline.** With no [`OffFrameJobs`] resource (a bare `World`), no async pool, or
//! [`OffFrameSettings::inline`], `build` and `apply` run at once where they are queued, so
//! behavior is exactly a plain command's. [`finish`] waits for a scope's jobs and applies
//! them, for tests and tools that need everything in now.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use bevy_tasks::AsyncComputeTaskPool;
use crossbeam_channel::{Receiver, TryRecvError};

use crate::diagnostics::Diagnostics;

/// An apply that takes longer than this on the main thread is logged with its job's name.
pub const SLOW_APPLY_MS: f64 = 8.0;

/// How off-frame jobs run.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct OffFrameSettings {
    /// Main-thread milliseconds a frame [`apply_off_frame_system`] spends applying finished
    /// jobs. It always applies at least one, so a frame goes over by at most one job.
    pub apply_ms: f64,
    /// Build and apply at once where queued, as a plain command would.
    pub inline: bool,
}

impl Default for OffFrameSettings {
    fn default() -> Self {
        Self {
            apply_ms: 4.0,
            inline: false,
        }
    }
}

/// A scope jobs can be grouped under (the streamer uses one per cell).
pub type Scope = u64;

type Apply = Box<dyn FnOnce(&mut World) + Send>;
type Panic = Box<dyn std::any::Any + Send>;
/// What a worker sends back: the apply with the built value in it, and the build's time.
type Done = Result<(Apply, Duration), Panic>;

struct Job {
    name: String,
    scope: Option<Scope>,
    done: Receiver<Done>,
    /// Set when the job is dropped, so a build not yet started is skipped.
    cancelled: Arc<AtomicBool>,
}

/// Jobs building or built and not yet applied, in queue order.
#[derive(Resource, Default)]
pub struct OffFrameJobs {
    jobs: Vec<Job>,
    scope: Option<Scope>,
    /// Since the frame system last reported: main-thread ms in applies, worker ms in builds.
    apply_ms: f64,
    build_ms: f64,
    /// The frame system's last report wasn't all zeros (so it runs once more to zero it).
    reported: bool,
}

impl OffFrameJobs {
    /// Jobs not yet applied, of every scope.
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Jobs of `scope` not yet applied.
    pub fn pending(&self, scope: Option<Scope>) -> usize {
        self.jobs.iter().filter(|j| j.scope == scope).count()
    }

    /// The scope jobs queued now are put under.
    pub fn scope(&self) -> Option<Scope> {
        self.scope
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// Queue `build` to run on a worker and `apply` to run with its value on the main thread
/// later (see the module docs); inline when off-frame building is off.
pub fn off_frame<T: Send + 'static>(
    world: &mut World,
    name: impl Into<String>,
    build: impl FnOnce() -> T + Send + 'static,
    apply: impl FnOnce(&mut World, T) + Send + 'static,
) {
    let pool = AsyncComputeTaskPool::try_get();
    let inline = world
        .get_resource::<OffFrameSettings>()
        .is_none_or(|s| s.inline);
    let (Some(pool), false, Some(mut jobs)) =
        (pool, inline, world.get_resource_mut::<OffFrameJobs>())
    else {
        let value = build();
        apply(world, value);
        return;
    };
    let name = name.into();
    let (tx, done) = crossbeam_channel::bounded::<Done>(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let skip = cancelled.clone();
    let scope = jobs.scope;
    jobs.jobs.push(Job {
        name,
        scope,
        done,
        cancelled,
    });
    pool.spawn(async move {
        if skip.load(Ordering::Relaxed) {
            return;
        }
        let t = Instant::now();
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build));
        let out = built.map(|value| {
            let apply: Apply = Box::new(move |world: &mut World| apply(world, value));
            (apply, t.elapsed())
        });
        // The job may have been dropped meanwhile; its value then goes with the channel.
        let _ = tx.send(out);
    })
    .detach();
}

/// [`off_frame`] from a system.
pub trait OffFrameCommands {
    /// Queue `build` on a worker and `apply` on the main thread after it (see [`off_frame`]).
    fn off_frame<T: Send + 'static>(
        &mut self,
        name: impl Into<String>,
        build: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut World, T) + Send + 'static,
    );
}

impl OffFrameCommands for Commands<'_, '_> {
    fn off_frame<T: Send + 'static>(
        &mut self,
        name: impl Into<String>,
        build: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut World, T) + Send + 'static,
    ) {
        let name = name.into();
        self.queue(move |world: &mut World| off_frame(world, name, build, apply));
    }
}

/// Put jobs queued from now on under `scope`; returns the scope that was current.
pub fn set_scope(world: &mut World, scope: Option<Scope>) -> Option<Scope> {
    match world.get_resource_mut::<OffFrameJobs>() {
        Some(mut jobs) => std::mem::replace(&mut jobs.scope, scope),
        None => None,
    }
}

/// Jobs of `scope` not yet applied (0 without the resource).
pub fn pending(world: &World, scope: Option<Scope>) -> usize {
    world
        .get_resource::<OffFrameJobs>()
        .map_or(0, |j| j.pending(scope))
}

/// Whether `scope`'s next job has finished building, so [`apply_finished`] would apply it.
pub fn ready(world: &World, scope: Option<Scope>) -> bool {
    world.get_resource::<OffFrameJobs>().is_some_and(|jobs| {
        jobs.jobs
            .iter()
            .find(|j| j.scope == scope)
            .is_some_and(|j| !j.done.is_empty())
    })
}

/// Forget `scope`'s jobs: builds not started are skipped, finished ones are dropped unapplied.
pub fn drop_scope(world: &mut World, scope: Scope) {
    if let Some(mut jobs) = world.get_resource_mut::<OffFrameJobs>() {
        jobs.jobs.retain(|j| j.scope != Some(scope));
    }
}

/// Apply `scope`'s finished jobs in queue order, stopping at the first still building or once
/// `budget` is spent (at least one is applied when ready). Returns how many were applied.
pub fn apply_finished(world: &mut World, scope: Option<Scope>, budget: Option<Duration>) -> usize {
    run(world, scope, budget, false)
}

/// Wait for every job of `scope` (and any they queue) and apply them all now.
pub fn finish(world: &mut World, scope: Option<Scope>) {
    run(world, scope, None, true);
}

fn run(world: &mut World, scope: Option<Scope>, budget: Option<Duration>, wait: bool) -> usize {
    let start = Instant::now();
    let mut applied = 0;
    loop {
        if applied > 0 && budget.is_some_and(|b| start.elapsed() >= b) {
            break;
        }
        let Some(mut jobs) = world.get_resource_mut::<OffFrameJobs>() else {
            break;
        };
        let Some(at) = jobs.jobs.iter().position(|j| j.scope == scope) else {
            break;
        };
        let got = if wait {
            jobs.jobs[at].done.recv().map_err(|_| ())
        } else {
            match jobs.jobs[at].done.try_recv() {
                Ok(done) => Ok(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => Err(()),
            }
        };
        let job = jobs.jobs.remove(at);
        let (apply, built) = match got {
            Ok(Ok(done)) => done,
            Ok(Err(panic)) => {
                tracing::error!("off-frame build \"{}\" panicked", job.name);
                std::panic::resume_unwind(panic);
            }
            Err(()) => {
                // The worker went without answering (its pool shut down): nothing to apply.
                tracing::warn!("off-frame build \"{}\" never finished", job.name);
                continue;
            }
        };
        // Jobs the apply queues belong where this one did.
        let outer = std::mem::replace(&mut jobs.scope, job.scope);
        let t = Instant::now();
        apply(world);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms > SLOW_APPLY_MS {
            tracing::warn!(
                "off-frame apply \"{}\" took {ms:.1} ms on the main thread",
                job.name
            );
        }
        if let Some(mut jobs) = world.get_resource_mut::<OffFrameJobs>() {
            jobs.scope = outer;
            jobs.apply_ms += ms;
            jobs.build_ms += built.as_secs_f64() * 1000.0;
        }
        applied += 1;
    }
    applied
}

/// The [`Diagnostics`] metric with the jobs not yet applied, every scope.
pub const PENDING: &str = "offframe.pending";
/// The [`Diagnostics`] metric with the worker milliseconds of the builds applied last frame.
pub const BUILDING_MS: &str = "offframe.building_ms";
/// The [`Diagnostics`] metric with the main-thread milliseconds spent in applies last frame
/// (the streamer's included).
pub const APPLY_MS: &str = "offframe.apply_ms";

/// Engine system (`First`): apply finished unscoped jobs within the budget, and report.
/// Run condition for [`apply_off_frame_system`]: a job is queued, or the last report wasn't
/// zero yet. It needs the whole world, so it runs only then (AGENTS.md, "Let systems run side
/// by side").
pub fn off_frame_due(jobs: Option<Res<OffFrameJobs>>) -> bool {
    jobs.is_some_and(|j| !j.jobs.is_empty() || j.reported || j.apply_ms > 0.0 || j.build_ms > 0.0)
}

pub fn apply_off_frame_system(world: &mut World) {
    let budget = world
        .get_resource::<OffFrameSettings>()
        .copied()
        .unwrap_or_default()
        .apply_ms;
    apply_finished(world, None, Some(Duration::from_secs_f64(budget / 1000.0)));
    let Some(mut jobs) = world.get_resource_mut::<OffFrameJobs>() else {
        return;
    };
    let report = (
        jobs.jobs.len() as f64,
        std::mem::take(&mut jobs.build_ms),
        std::mem::take(&mut jobs.apply_ms),
    );
    jobs.reported = report != (0.0, 0.0, 0.0);
    if let Some(mut diag) = world.get_resource_mut::<Diagnostics>() {
        diag.set(PENDING, report.0);
        diag.set(BUILDING_MS, report.1);
        diag.set(APPLY_MS, report.2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Resource, Default)]
    struct Log(Vec<u32>);

    fn world() -> World {
        crate::tasks::init_task_pools();
        let mut world = World::new();
        world.init_resource::<OffFrameJobs>();
        world.init_resource::<OffFrameSettings>();
        world.init_resource::<Log>();
        world
    }

    fn push(world: &mut World, n: u32, sleep_ms: u64) {
        off_frame(
            world,
            format!("job {n}"),
            move || {
                std::thread::sleep(Duration::from_millis(sleep_ms));
                n
            },
            |world, n| world.resource_mut::<Log>().0.push(n),
        );
    }

    /// Run the frame system until nothing is left (or give up).
    fn drain(world: &mut World) -> usize {
        let mut frames = 0;
        while !world.resource::<OffFrameJobs>().is_empty() {
            apply_off_frame_system(world);
            frames += 1;
            assert!(frames < 10_000, "jobs never finished");
            std::thread::sleep(Duration::from_millis(1));
        }
        frames
    }

    #[test]
    fn a_job_applies_after_its_build_and_off_the_calling_thread() {
        let mut world = world();
        let main = std::thread::current().id();
        let built_on = Arc::new(Mutex::new(None));
        let b = built_on.clone();
        off_frame(
            &mut world,
            "probe",
            move || {
                *b.lock().unwrap() = Some(std::thread::current().id());
                std::thread::sleep(Duration::from_millis(20));
                7u32
            },
            |world, n| world.resource_mut::<Log>().0.push(n),
        );
        // Queued, not applied: the caller went on.
        assert!(world.resource::<Log>().0.is_empty());
        assert_eq!(pending(&world, None), 1);
        drain(&mut world);
        assert_eq!(world.resource::<Log>().0, [7]);
        assert_ne!(built_on.lock().unwrap().unwrap(), main);
    }

    #[test]
    fn inline_mode_and_a_bare_world_apply_at_once() {
        let mut world = world();
        world.resource_mut::<OffFrameSettings>().inline = true;
        push(&mut world, 1, 0);
        assert_eq!(world.resource::<Log>().0, [1]);

        let mut bare = World::new();
        bare.init_resource::<Log>();
        push(&mut bare, 2, 0);
        assert_eq!(bare.resource::<Log>().0, [2]);
    }

    #[test]
    fn jobs_apply_in_queue_order_whatever_finishes_first() {
        let mut world = world();
        push(&mut world, 0, 50);
        for n in 1..6 {
            push(&mut world, n, 0);
        }
        // The quick ones are done long before the first; none may jump it.
        std::thread::sleep(Duration::from_millis(10));
        apply_off_frame_system(&mut world);
        assert!(world.resource::<Log>().0.is_empty());
        drain(&mut world);
        assert_eq!(world.resource::<Log>().0, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn slow_applies_spread_over_frames_within_the_budget() {
        let mut world = world();
        world.resource_mut::<OffFrameSettings>().apply_ms = 4.0;
        for _ in 0..20 {
            off_frame(
                &mut world,
                "slow apply",
                || (),
                |_, ()| std::thread::sleep(Duration::from_millis(2)),
            );
        }
        finish_builds(&world);
        let mut frames = 0;
        while !world.resource::<OffFrameJobs>().is_empty() {
            let before = pending(&world, None);
            let t = Instant::now();
            apply_off_frame_system(&mut world);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let applied = before - pending(&world, None);
            // Budget plus one job; 2 ms sleeps make at most 3 fit in 4 ms (with slack).
            assert!(
                (1..=3).contains(&applied),
                "{applied} applied in {ms:.1} ms"
            );
            frames += 1;
        }
        assert!(frames >= 7, "20 applies of 2 ms took only {frames} frames");
        let diag_ms = world.resource::<OffFrameJobs>().apply_ms;
        assert_eq!(diag_ms, 0.0, "the system reports and resets its sums");
    }

    /// Wait until every queued build has finished (applies nothing).
    fn finish_builds(world: &World) {
        let jobs = world.resource::<OffFrameJobs>();
        let t = Instant::now();
        while jobs.jobs.iter().any(|j| j.done.is_empty()) {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn scoped_jobs_wait_for_their_owner_and_drop_with_the_scope() {
        let mut world = world();
        set_scope(&mut world, Some(3));
        push(&mut world, 30, 0);
        push(&mut world, 31, 0);
        off_frame(
            &mut world,
            "dropped",
            || (),
            |_, ()| panic!("a dropped job applied"),
        );
        assert_eq!(set_scope(&mut world, None), Some(3));
        push(&mut world, 1, 0);
        drain_scope_free(&mut world);
        // The frame system took only the unscoped job.
        assert_eq!(world.resource::<Log>().0, [1]);
        assert_eq!(pending(&world, Some(3)), 3);
        // The owner applies two, one at a time, then drops the scope before the third.
        let t = Instant::now();
        while world.resource::<Log>().0.len() < 3 {
            apply_finished(&mut world, Some(3), Some(Duration::ZERO));
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(1));
        }
        drop_scope(&mut world, 3);
        assert_eq!(pending(&world, Some(3)), 0);
        assert_eq!(world.resource::<Log>().0, [1, 30, 31]);
    }

    fn drain_scope_free(world: &mut World) {
        let t = Instant::now();
        while pending(world, None) > 0 {
            apply_off_frame_system(world);
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn an_apply_that_queues_more_keeps_its_scope() {
        let mut world = world();
        set_scope(&mut world, Some(9));
        off_frame(
            &mut world,
            "outer",
            || 1u32,
            |world, n| {
                world.resource_mut::<Log>().0.push(n);
                push(world, 2, 0);
            },
        );
        set_scope(&mut world, None);
        finish(&mut world, Some(9));
        assert_eq!(world.resource::<Log>().0, [1, 2]);
        assert_eq!(world.resource::<OffFrameJobs>().scope(), None);
    }

    #[test]
    fn a_panicking_build_panics_at_apply() {
        let mut world = world();
        off_frame(
            &mut world,
            "boom",
            || -> u32 { panic!("build failed") },
            |_, _| {},
        );
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finish(&mut world, None);
        }));
        assert!(caught.is_err());
    }

    #[test]
    fn commands_queue_off_frame_jobs() {
        let mut world = world();
        let mut queue = bevy_ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        commands.off_frame(
            "from commands",
            || 5u32,
            |world, n| world.resource_mut::<Log>().0.push(n),
        );
        queue.apply(&mut world);
        finish(&mut world, None);
        assert_eq!(world.resource::<Log>().0, [5]);
    }
}
