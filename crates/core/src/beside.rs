//! Fixed-step jobs that run beside the frame (engine-design/fixed-step-beside-frame.md).
//!
//! The PC's frame is the main thread's chain of schedules, and the fixed step is about half of
//! it at the busiest stops. A fixed-step system that only needs to hand its readers last step's
//! answer splits into: take the answer of the job started last step, apply it, gather owned
//! inputs, start the next job. The job runs on [`crate::tasks::beside_pool`] through the rest of the frame
//! (Update, PostUpdate, Extract, Render, First) and the next step takes it, the way physics
//! already steps Rapier beside the frame and Unreal's async physics ticks on its own thread.
//!
//! Inline (headless runs and tests, `beside` false), [`Beside::start`] runs the job at once
//! and [`Beside::take`] hands it back in the same step, so a system that starts then takes
//! behaves exactly as it did before it had a job.

use bevy_tasks::Task;

type Done<T> = std::thread::Result<T>;

/// One fixed-step job, beside the frame or inline. See the [module docs](self).
pub struct Beside<T: Send + 'static> {
    name: &'static str,
    state: State<T>,
}

enum State<T> {
    Idle,
    Ready(T),
    Running(Task<Done<T>>),
}

impl<T: Send + 'static> Default for Beside<T> {
    fn default() -> Self {
        Self {
            name: "beside job",
            state: State::Idle,
        }
    }
}

impl<T: Send + 'static> Beside<T> {
    /// The answer of the job started last (this step's, inline). Waits under a
    /// `wait for beside: <name>` span if it is still running: it has had a whole frame, so a
    /// wait in a trace means the job is longer than the frame. A panic in the job is raised
    /// here.
    pub fn take(&mut self) -> Option<T> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => None,
            State::Ready(v) => Some(v),
            State::Running(task) => {
                let done = if task.is_finished() {
                    bevy_tasks::block_on(task)
                } else {
                    let _span = tracing::info_span!(
                        target: "engine_core",
                        "wait for beside",
                        name = self.name
                    )
                    .entered();
                    bevy_tasks::block_on(task)
                };
                match done {
                    Ok(v) => Some(v),
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }
        }
    }

    /// Whether a job is running or an answer waits to be taken.
    pub fn is_pending(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Start the next job, beside the frame on [`crate::tasks::beside_pool`] when `beside` (and
    /// the pools are set up), else now on this thread. An answer not taken yet is dropped first (as is a job
    /// still running: it finishes on its own and its answer is lost). The job owns its inputs;
    /// it never touches the World.
    pub fn start(
        &mut self,
        name: &'static str,
        beside: bool,
        job: impl FnOnce() -> T + Send + 'static,
    ) {
        self.name = name;
        self.cancel();
        let pool = crate::tasks::beside_pool().filter(|_| beside);
        self.state = match pool {
            Some(pool) => State::Running(pool.spawn(async move {
                let _span = tracing::info_span!(target: "engine_core", "beside", name).entered();
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
            })),
            None => State::Ready(job()),
        };
    }

    /// Drop the job and its answer (its inputs no longer hold: a world shift, a zone unload).
    pub fn cancel(&mut self) {
        if let State::Running(task) = std::mem::replace(&mut self.state, State::Idle) {
            // Detached, it runs to its end on the pool; dropping a running task would only
            // cancel it at its next await, which a plain closure never reaches.
            task.detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_the_answer_is_there_in_the_same_step() {
        let mut job = Beside::default();
        assert!(job.take().is_none());
        job.start("sum", false, || (1..=4).sum::<i32>());
        assert_eq!(job.take(), Some(10));
        assert!(job.take().is_none());
    }

    #[test]
    fn beside_the_frame_the_next_step_takes_the_same_answer() {
        crate::tasks::init_task_pools();
        let run = |beside: bool| {
            let mut job = Beside::default();
            let mut seen = Vec::new();
            for state in 1u64..=5 {
                if beside {
                    // The answer of the job started last step.
                    if let Some(v) = job.take() {
                        seen.push(v);
                    }
                }
                let input = state;
                job.start("square", beside, move || input * input + 1);
                if !beside {
                    seen.push(job.take().unwrap());
                }
            }
            if beside {
                seen.push(job.take().unwrap());
            }
            seen
        };
        assert_eq!(run(true), run(false));
    }

    #[test]
    fn a_panic_in_the_job_comes_back_where_it_is_taken() {
        crate::tasks::init_task_pools();
        let mut job: Beside<i32> = Beside::default();
        job.start("boom", true, || panic!("boom"));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.take()));
        assert!(caught.is_err());
    }

    #[test]
    fn a_job_beside_the_frame_does_not_queue_behind_long_async_work() {
        crate::tasks::init_task_pools();
        // Every async thread busy with something long, as with zone builds while streaming.
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pool = bevy_tasks::AsyncComputeTaskPool::get();
        for _ in 0..pool.thread_num() {
            let release = release.clone();
            pool.spawn(async move {
                let t = std::time::Instant::now();
                while !release.load(std::sync::atomic::Ordering::Relaxed)
                    && t.elapsed().as_secs() < 20
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
            .detach();
        }
        let mut job = Beside::default();
        let t = std::time::Instant::now();
        job.start("quick", true, || 7);
        let got = job.take();
        let waited = t.elapsed();
        release.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(got, Some(7));
        assert!(
            waited.as_secs_f64() < 2.0,
            "waited {waited:?} behind async work"
        );
    }
}
