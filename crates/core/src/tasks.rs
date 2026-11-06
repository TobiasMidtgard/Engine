//! Thread pools for parallel work.
//!
//! ECS systems already run in parallel, and `Query::par_iter_mut` splits work across the
//! compute pool. For custom data-parallel loops in simulations (grids, particles), use
//! [`par_chunks_mut`]. To run a stage of the frame one frame behind on its own thread (the
//! renderer, like Unreal's render thread), use [`Pipelined`]. To build costly art on a worker
//! and spawn it on the frame in budgeted, ordered slices, use [`off_frame`](crate::off_frame).

pub use bevy_tasks::{
    AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool, TaskPool, TaskPoolBuilder,
};

pub use crate::off_frame::{OffFrameCommands, off_frame};

/// Create the compute / async / IO pools once per process.
///
/// Sized so the frame's own threads always have a core, as Unreal does (task graph workers =
/// logical cores − 2, leaving the game and render threads theirs): the compute pool gets
/// cores − 2, because the main thread works through every scope it opens and the render
/// thread ([`Pipelined`]) needs a core while the next frame simulates. Async and IO get a
/// quarter of the cores each, 1 to 4 threads (Bevy's `TaskPoolOptions` defaults); they sleep
/// unless loading. A compute pool of every core beside them oversubscribed the cores ~1.5×,
/// leaving the render thread to compete for one in busy frames.
pub fn init_task_pools() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let (compute, side) = pool_sizes(threads);
    ComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(compute)
            .thread_name("compute".into())
            .build()
    });
    AsyncComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(side)
            .thread_name("async".into())
            .build()
    });
    IoTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(side)
            .thread_name("io".into())
            .build()
    });
}

/// Threads for the compute pool and for each of the async and IO pools, given the logical
/// cores: see [`init_task_pools`].
fn pool_sizes(cores: usize) -> (usize, usize) {
    (cores.saturating_sub(2).max(1), (cores / 4).clamp(1, 4))
}

/// Threads that background work (art, bakes, nav builds) may split into across the whole
/// process: cores − 2, like the compute pool, so the frame's own threads keep their cores.
fn background_threads() -> usize {
    static TOTAL: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *TOTAL.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .saturating_sub(2)
            .max(1)
    })
}

/// Background threads leased now, by every [`BackgroundLease`] alive.
static LEASED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A share of the background threads, for a job that splits its work over threads of its own
/// (`std::thread::scope` over chunks). Several such jobs at once used to each take every core:
/// on the 4-core cloud VM 8 art threads ran beside the frame while a zone streamed in, and the
/// physics threads waited 130 ms to be scheduled for 0.04 ms of work (16 × 16 on a 32-core
/// PC). Leases together stay within [`background_threads`]; a job always gets at least one
/// thread. Unreal's task graph and Unity's job system share one worker count the same way.
#[derive(Debug)]
pub struct BackgroundLease {
    taken: usize,
    threads: usize,
}

impl BackgroundLease {
    /// Threads the job may run on (at least 1).
    pub fn threads(&self) -> usize {
        self.threads
    }
}

impl Drop for BackgroundLease {
    fn drop(&mut self) {
        LEASED.fetch_sub(self.taken, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Lease up to `want` background threads: what's free of [`background_threads`], at least 1.
/// Hold it while the threads run; they return when it drops.
pub fn background_lease(want: usize) -> BackgroundLease {
    use std::sync::atomic::Ordering;
    let total = background_threads();
    let want = want.max(1);
    let mut leased = LEASED.load(Ordering::Acquire);
    loop {
        let taken = want.min(total.saturating_sub(leased));
        match LEASED.compare_exchange_weak(
            leased,
            leased + taken,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                return BackgroundLease {
                    taken,
                    threads: taken.max(1),
                };
            }
            Err(now) => leased = now,
        }
    }
}

/// Run `f(chunk_index, chunk)` over `data` split into `chunk_size` pieces on the compute pool.
///
/// Called from an async worker (an [`off_frame`] build, a cell build) or from a thread with no
/// name (a module's own background thread), the chunks run on the async pool instead, so a
/// background build never takes the compute threads the frame's systems need. A thread
/// waiting in a compute scope also runs the frame's own system tasks: on the session VM's
/// route an unnamed background thread held `engine_vehicles::prepare_frames` for 0.5 s that
/// way while the frame waited.
/// With no pool set up (a plain unit test) the chunks run in order on the calling thread.
pub fn par_chunks_mut<T: Send>(
    data: &mut [T],
    chunk_size: usize,
    f: impl Fn(usize, &mut [T]) + Send + Sync,
) {
    let chunk_size = chunk_size.max(1);
    if data.len() <= chunk_size {
        f(0, data);
        return;
    }
    let f = &f;
    let on_async = std::thread::current()
        .name()
        .is_none_or(|n| n.starts_with("async"));
    // From the frame's threads the chunks go to the leaf pool, not the compute pool: a thread
    // waiting in a compute-pool scope runs other queued tasks meanwhile, whole systems among
    // them, so a loop of a millisecond's work held its system up for another system's run
    // (softbody's 24 resting bodies took 7-10 ms, fire's kernels 2.8 ms a frame at the
    // fortress, VM). Waiting on the leaf pool runs only other loops' chunks, which never wait
    // on anything but their own chunks.
    let pool: Option<&TaskPool> = match on_async.then(AsyncComputeTaskPool::try_get).flatten() {
        Some(pool) => Some(pool),
        None => ComputeTaskPool::try_get().map(|_| leaf_pool()),
    };
    // Without pools (a plain unit test, a tool), the chunks run one after another.
    let Some(pool) = pool else {
        for (i, chunk) in data.chunks_mut(chunk_size).enumerate() {
            f(i, chunk);
        }
        return;
    };
    pool.scope(|scope| {
        for (i, chunk) in data.chunks_mut(chunk_size).enumerate() {
            scope.spawn(async move { f(i, chunk) });
        }
    });
}

/// Threads for data-parallel loops called from the frame's threads ([`par_chunks_mut`]): as
/// many as the compute pool. Their tasks are pieces of a loop that never wait on anything
/// but nested pieces, so a thread waiting in a scope here only helps other loops along.
/// Unreal's task graph keeps short "leaf" work apart from tasks that wait the same way.
pub fn leaf_pool() -> &'static TaskPool {
    static POOL: std::sync::OnceLock<TaskPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        TaskPoolBuilder::new()
            .num_threads(pool_sizes(cores).0)
            .thread_name("leaf".into())
            .build()
    })
}

/// Run the calling thread above normal priority, so a pipeline thread (the renderer) isn't
/// starved by the worker pools during a busy frame, as Unreal runs its render thread. Only on
/// Windows: elsewhere raising a priority needs privileges, and the pool sizing leaves it a core.
#[cfg(windows)]
fn raise_priority() {
    const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
    }
    // SAFETY: GetCurrentThread returns a pseudo handle for this thread that needs no closing,
    // and SetThreadPriority only changes this thread's scheduling.
    if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) } == 0 {
        tracing::debug!("could not raise the pipeline thread's priority");
    }
}
#[cfg(not(windows))]
fn raise_priority() {}

type Job<T> = Box<dyn FnOnce(&mut T) + Send>;
type Done<T> = Result<T, Box<dyn std::any::Any + Send>>;
const LOST: &str = "the pipelined value was lost to an earlier panic in its job";

/// A value that can be sent to its own worker thread to run a job while the caller goes on,
/// then waited for: frame pipelining with one sync point and at most one job in flight.
///
/// The pattern is Bevy's pipelined rendering and Unreal's game/render thread split
/// (`r.OneFrameThreadLag`): the main thread hands the renderer frame N's extracted data and
/// [`dispatch`](Self::dispatch)es "record and submit"; it then simulates frame N+1 and calls
/// [`get`](Self::get) at the next extract, which waits only if the worker is still busy. The
/// value is never in two places, so no locking is needed around it. A panic in a job is
/// re-raised on the caller's thread at the next [`get`](Self::get).
pub struct Pipelined<T: Send + 'static> {
    home: Option<T>,
    /// A job is running (or finished but not yet waited for).
    out: bool,
    jobs: Option<crossbeam_channel::Sender<(T, Job<T>)>>,
    done: crossbeam_channel::Receiver<Done<T>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl<T: Send + 'static> Pipelined<T> {
    /// Start the worker thread (named `name`) holding nothing; `value` stays home.
    pub fn new(name: &str, value: T) -> Self {
        let (jobs, job_rx) = crossbeam_channel::bounded::<(T, Job<T>)>(1);
        let (done_tx, done) = crossbeam_channel::bounded::<Done<T>>(1);
        let worker = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                raise_priority();
                for (mut value, job) in job_rx {
                    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        job(&mut value);
                    }));
                    if done_tx.send(ran.map(|()| value)).is_err() {
                        return;
                    }
                }
            })
            .expect("could not start a pipeline thread");
        Self {
            home: Some(value),
            out: false,
            jobs: Some(jobs),
            done,
            worker: Some(worker),
        }
    }

    /// Whether a job is running (the value is on the worker).
    pub fn is_out(&self) -> bool {
        self.out
    }

    /// The value, waiting for the job in flight to finish first (the sync point).
    pub fn get(&mut self) -> &mut T {
        self.flush();
        self.home.as_mut().expect(LOST)
    }

    /// Wait for the job in flight, if any. Use before anything that needs the frame finished
    /// (a screenshot, a test, shutdown), like Unreal's `FlushRenderingCommands`.
    pub fn flush(&mut self) {
        if !self.out {
            return;
        }
        self.out = false;
        let back = {
            let _span = tracing::info_span!(target: "engine_core", "pipeline wait").entered();
            self.done.recv()
        };
        match back {
            Ok(Ok(value)) => self.home = Some(value),
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            Err(_) => panic!("the pipeline thread stopped"),
        }
    }

    /// Send the value to the worker to run `job` and return at once. Waits first if a job is
    /// still in flight, so at most one runs at a time.
    pub fn dispatch(&mut self, job: impl FnOnce(&mut T) + Send + 'static) {
        self.flush();
        let value = self.home.take().expect(LOST);
        let jobs = self.jobs.as_ref().expect("the pipeline is running");
        if let Err(crossbeam_channel::SendError((value, job))) = jobs.send((value, Box::new(job))) {
            // The worker is gone (it can only stop when dropped): run inline instead.
            let mut value = value;
            job(&mut value);
            self.home = Some(value);
        } else {
            self.out = true;
        }
    }

    /// Wait for the job in flight and take the value back, stopping the worker.
    pub fn into_inner(mut self) -> T {
        self.flush();
        self.home.take().expect(LOST)
    }
}

impl<T: Send + 'static> Drop for Pipelined<T> {
    fn drop(&mut self) {
        if self.out && !std::thread::panicking() {
            // Let the job finish before its value is dropped (it may own GPU objects).
            let _ = self.done.recv();
        }
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn background_leases_share_the_cores_and_give_them_back() {
        let total = background_threads();
        let first = background_lease(usize::MAX);
        assert!(first.threads() >= 1 && first.threads() <= total);
        // Another job while the first holds them all: one thread of its own, no more.
        let second = background_lease(16);
        assert_eq!(
            second.threads(),
            1.max(total.saturating_sub(first.threads())).min(16)
        );
        drop(first);
        drop(second);
        let again = background_lease(total);
        assert_eq!(again.threads(), total, "returned when the leases drop");
    }
    use super::*;
    use std::sync::Arc;

    #[test]
    fn pools_leave_the_main_and_render_threads_a_core() {
        assert_eq!(pool_sizes(1), (1, 1));
        assert_eq!(pool_sizes(2), (1, 1));
        assert_eq!(pool_sizes(4), (2, 1));
        assert_eq!(pool_sizes(8), (6, 2));
        assert_eq!(pool_sizes(32), (30, 4));
    }

    #[test]
    fn par_chunks_from_an_async_worker_stay_off_the_compute_pool() {
        init_task_pools();
        let names = AsyncComputeTaskPool::get().spawn(async {
            let caller = std::thread::current().name().map(str::to_owned);
            let mut seen: Vec<Option<String>> = vec![None; 16];
            par_chunks_mut(&mut seen, 1, |_, s| {
                std::thread::sleep(std::time::Duration::from_millis(2));
                s[0] = std::thread::current().name().map(str::to_owned);
            });
            (caller, seen)
        });
        let (caller, seen) = bevy_tasks::block_on(names);
        if caller.is_some_and(|n| n.starts_with("async")) {
            for name in seen.into_iter().flatten() {
                assert!(!name.starts_with("compute"), "chunk ran on {name}");
            }
        }
    }
    #[test]
    fn a_loop_on_a_system_thread_waits_without_running_other_queued_tasks() {
        init_task_pools();
        let pool = ComputeTaskPool::get();
        let loop_ms = std::sync::Mutex::new(None);
        pool.scope(|s| {
            // The loop starts first; long "systems" queue behind it on the compute pool.
            s.spawn(async {
                let t0 = std::time::Instant::now();
                let mut data = vec![0u32; 64];
                par_chunks_mut(&mut data, 2, |i, c| c.fill(i as u32));
                assert_eq!(data[63], 31);
                *loop_ms.lock().unwrap() = Some(t0.elapsed().as_millis());
            });
            for _ in 0..pool.thread_num() * 2 + 2 {
                s.spawn(async { std::thread::sleep(std::time::Duration::from_millis(300)) });
            }
        });
        let ms = loop_ms.into_inner().unwrap().expect("the loop ran");
        assert!(ms < 150, "the loop waited {ms} ms behind other tasks");
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn jobs_run_off_thread_one_at_a_time_and_come_home() {
        let mut p = Pipelined::new("test pipeline", Vec::<usize>::new());
        let main = std::thread::current().id();
        for i in 0..5 {
            p.get().push(100 + i);
            p.dispatch(move |v| {
                assert_ne!(std::thread::current().id(), main);
                std::thread::sleep(std::time::Duration::from_millis(2));
                v.push(i);
            });
            assert!(p.is_out());
        }
        assert_eq!(p.into_inner(), [100, 0, 101, 1, 102, 2, 103, 3, 104, 4]);
    }

    #[test]
    fn the_caller_overlaps_the_job() {
        let started = Arc::new(AtomicUsize::new(0));
        let mut p = Pipelined::new("test overlap", ());
        let s = started.clone();
        let t0 = std::time::Instant::now();
        p.dispatch(move |_| {
            s.store(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(50));
        });
        // dispatch returned before the job finished.
        assert!(t0.elapsed() < std::time::Duration::from_millis(40));
        p.flush();
        assert!(t0.elapsed() >= std::time::Duration::from_millis(50));
        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert!(!p.is_out());
    }

    #[test]
    fn a_panicking_job_panics_the_caller_at_the_next_get() {
        let mut p = Pipelined::new("test panic", 0u32);
        p.dispatch(|_| panic!("job failed"));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            p.get();
        }));
        assert!(caught.is_err());
    }

    #[test]
    fn dropping_waits_for_the_job() {
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        let mut p = Pipelined::new("test drop", ());
        p.dispatch(move |_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            r.store(1, Ordering::SeqCst);
        });
        drop(p);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pipelined_values_can_be_resources() {
        fn is_resource_ready<R: Send + Sync + 'static>() {}
        is_resource_ready::<Pipelined<Vec<u8>>>();
    }
}
