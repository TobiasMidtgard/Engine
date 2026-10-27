//! Pipelines compiled on worker threads before a frame first needs them (after Unreal's PSO
//! precaching, which compiles the pipeline states a scene may use on background threads at
//! load, and Unity's `ShaderVariantCollection.WarmUp`): a feature queues every variant it may
//! draw with as soon as its shaders exist, and a frame that needs one takes it when done, waits
//! for it when it is compiling, or builds it itself when no worker has started it yet.
//!
//! wgpu devices are `Sync`, so pipelines can be created on any thread; under D3D12 and Vulkan
//! each one is a shader compile (seconds for a large shader with FXC), so several workers cut
//! the wall time of a scene's first frames and of the first sight of a new material kind.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, bounded};

/// A job: builds one pipeline (or a preset's set of them) on a warm-up thread.
pub(crate) type Build<T = wgpu::RenderPipeline> = Box<dyn FnOnce() -> Option<T> + Send>;

struct Job<K, T> {
    key: K,
    build: Build<T>,
    done: Sender<Option<T>>,
}

/// What [`Warmup::take`] found.
pub(crate) enum Taken<T = wgpu::RenderPipeline> {
    /// Compiled (`None` if the job could not build it).
    Ready(Option<T>),
    /// Never queued, or no worker had started it: build it now.
    Build,
}

pub(crate) struct Warmup<K, T = wgpu::RenderPipeline> {
    queue: Arc<Mutex<VecDeque<Job<K, T>>>>,
    results: HashMap<K, Receiver<Option<T>>>,
}

impl<K: Copy + Eq + Hash + Send + 'static, T: Send + 'static> Warmup<K, T> {
    /// Start compiling `jobs`, in order, on `threads` workers named `name`.
    pub fn start(name: &str, jobs: Vec<(K, Build<T>)>, threads: usize) -> Self {
        let mut results = HashMap::new();
        let queue: VecDeque<Job<K, T>> = jobs
            .into_iter()
            .map(|(key, build)| {
                let (done, rx) = bounded(1);
                results.insert(key, rx);
                Job { key, build, done }
            })
            .collect();
        let workers = threads.clamp(1, queue.len().max(1));
        let queue = Arc::new(Mutex::new(queue));
        for _ in 0..workers {
            let queue = queue.clone();
            let spawned = std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    loop {
                        let job = queue.lock().ok().and_then(|mut q| q.pop_front());
                        let Some(job) = job else { break };
                        // The frame may have stopped waiting (features changed): fine.
                        let _ = job.done.send((job.build)());
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!("could not start a pipeline warm-up thread: {e}");
            }
        }
        Self { queue, results }
    }

    /// The pipeline for `key`, waiting for it if a worker is compiling it.
    pub fn take(&mut self, key: K) -> Taken<T> {
        let Some(rx) = self.results.remove(&key) else {
            return Taken::Build;
        };
        if let Ok(mut queue) = self.queue.lock()
            && let Some(at) = queue.iter().position(|j| j.key == key)
        {
            // Not started: build it on the frame's thread rather than wait behind the others.
            queue.remove(at);
            return Taken::Build;
        }
        match rx.recv() {
            Ok(pipeline) => Taken::Ready(pipeline),
            Err(_) => Taken::Build,
        }
    }

    /// Jobs not finished yet.
    pub fn pending(&self) -> usize {
        self.results.values().filter(|rx| rx.is_empty()).count()
    }
}

impl<K, T> Drop for Warmup<K, T> {
    /// Unstarted jobs are dropped; ones being compiled finish and are thrown away.
    fn drop(&mut self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
    }
}
