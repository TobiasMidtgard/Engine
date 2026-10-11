//! Thread pools for parallel work.
//!
//! ECS systems already run in parallel, and `Query::par_iter_mut` splits work across the
//! compute pool. For custom data-parallel loops in simulations (grids, particles), use
//! [`par_chunks_mut`].

pub use bevy_tasks::{AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool, TaskPool, TaskPoolBuilder};

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
