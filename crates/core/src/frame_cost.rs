//! What the main thread spends each frame on, with the profiler off: its own work through the
//! frame's schedules ([`crate::app::run_frame`]) apart from the time it only waits for the
//! render thread to hand the renderer back ([`crate::tasks::Pipelined::flush`], the "pipeline
//! wait"). So a frame held up by the graphics card (long wait, the render thread waiting for
//! the window image) can be told from one held up by the game's own systems (busy near the
//! frame time) on a player's machine, where the profiler's per-system tables cost too much to
//! leave on.
//!
//! The renderer's perf line reads the sums with [`take`] every few seconds; the latest frame's
//! values also go to `Diagnostics` as `frame.main_ms`, `frame.main_fixed_ms` and
//! `frame.main_wait_ms`.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Sums over frames, taken as a window.
struct Sums {
    busy_ns: AtomicU64,
    fixed_ns: AtomicU64,
    wait_ns: AtomicU64,
    steps: AtomicU64,
    frames: AtomicU64,
}

static SUMS: Sums = Sums::new();

thread_local! {
    /// Time this thread spent waiting in `Pipelined::flush`, ever.
    static WAITED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// Time the calling thread waited in `Pipelined::flush`, ever.
pub(crate) fn waited() -> Duration {
    WAITED.with(Cell::get)
}

pub(crate) fn note_wait(d: Duration) {
    WAITED.with(|w| w.set(w.get() + d));
}

/// One frame of the main thread.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Frame {
    /// Its own work through the schedules, the fixed steps included, in milliseconds.
    pub busy_ms: f64,
    /// Of `busy_ms`, the fixed steps'.
    pub fixed_ms: f64,
    /// Waiting for the render thread.
    pub wait_ms: f64,
    /// Fixed steps run.
    pub steps: f64,
}

pub(crate) fn note_frame(f: Frame) {
    SUMS.note(f);
}

/// The average frame since the last call (`None` without one), and start a new window. Any
/// thread may call it.
pub fn take() -> Option<Frame> {
    SUMS.take()
}

impl Sums {
    const fn new() -> Self {
        Self {
            busy_ns: AtomicU64::new(0),
            fixed_ns: AtomicU64::new(0),
            wait_ns: AtomicU64::new(0),
            steps: AtomicU64::new(0),
            frames: AtomicU64::new(0),
        }
    }

    fn note(&self, f: Frame) {
        let ns = |ms: f64| (ms * 1e6) as u64;
        self.busy_ns.fetch_add(ns(f.busy_ms), Ordering::Relaxed);
        self.fixed_ns.fetch_add(ns(f.fixed_ms), Ordering::Relaxed);
        self.wait_ns.fetch_add(ns(f.wait_ms), Ordering::Relaxed);
        self.steps.fetch_add(f.steps as u64, Ordering::Relaxed);
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    fn take(&self) -> Option<Frame> {
        let frames = self.frames.swap(0, Ordering::Relaxed);
        let busy = self.busy_ns.swap(0, Ordering::Relaxed);
        let fixed = self.fixed_ns.swap(0, Ordering::Relaxed);
        let wait = self.wait_ns.swap(0, Ordering::Relaxed);
        let steps = self.steps.swap(0, Ordering::Relaxed);
        (frames > 0).then(|| {
            let n = frames as f64;
            let ms = |ns: u64| ns as f64 / 1e6 / n;
            Frame {
                busy_ms: ms(busy),
                fixed_ms: ms(fixed),
                wait_ms: ms(wait),
                steps: steps as f64 / n,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_averages_its_frames_and_starts_again() {
        let sums = Sums::new();
        sums.note(Frame {
            busy_ms: 4.0,
            fixed_ms: 2.0,
            wait_ms: 10.0,
            steps: 1.0,
        });
        sums.note(Frame {
            busy_ms: 6.0,
            fixed_ms: 0.0,
            wait_ms: 0.0,
            steps: 0.0,
        });
        let f = sums.take().unwrap();
        assert!((f.busy_ms - 5.0).abs() < 1e-6, "{f:?}");
        assert!((f.fixed_ms - 1.0).abs() < 1e-6, "{f:?}");
        assert!((f.wait_ms - 5.0).abs() < 1e-6, "{f:?}");
        assert!((f.steps - 0.5).abs() < 1e-6, "{f:?}");
        assert_eq!(sums.take(), None);
    }
}
