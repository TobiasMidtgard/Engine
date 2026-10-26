//! Thread pools for parallel work.
//!
//! ECS systems already run in parallel, and `Query::par_iter_mut` splits work across the
//! compute pool. For custom data-parallel loops in simulations (grids, particles), use
//! [`par_chunks_mut`]. To run a stage of the frame one frame behind on its own thread (the
//! renderer, like Unreal's render thread), use [`Pipelined`].

pub use bevy_tasks::{
    AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool, TaskPool, TaskPoolBuilder,
};

/// Create the compute / async / IO pools once per process.
pub fn init_task_pools() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    ComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(threads)
            .thread_name("compute".into())
            .build()
    });
    AsyncComputeTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads((threads / 4).max(1))
            .thread_name("async".into())
            .build()
    });
    IoTaskPool::get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads((threads / 4).max(1))
            .thread_name("io".into())
            .build()
    });
}

/// Run `f(chunk_index, chunk)` over `data` split into `chunk_size` pieces on the compute pool.
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
    ComputeTaskPool::get().scope(|scope| {
        for (i, chunk) in data.chunks_mut(chunk_size).enumerate() {
            scope.spawn(async move { f(i, chunk) });
        }
    });
}

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
    use super::*;
    use std::sync::Arc;
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
