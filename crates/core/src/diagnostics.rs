//! Frame timing and named counters. Any module can publish a metric with
//! [`Diagnostics::set`]; the AI bridge exposes them via `engine.stats` and the debug UI shows
//! them.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use serde::Serialize;

use crate::time::Time;

#[derive(Resource, Debug, Clone, Default, Serialize)]
pub struct Diagnostics {
    /// Smoothed real frame time in milliseconds.
    pub frame_ms: f64,
    /// Smoothed frames per second.
    pub fps: f64,
    pub frame: u64,
    pub entities: u32,
    /// Named metrics published by modules (`"physics.bodies"`, `"render.draw_calls"`, …).
    pub metrics: BTreeMap<String, f64>,
}

impl Diagnostics {
    pub fn set(&mut self, name: &str, value: f64) {
        match self.metrics.get_mut(name) {
            Some(v) => *v = value,
            None => {
                self.metrics.insert(name.to_string(), value);
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.metrics.get(name).copied()
    }
}

/// Engine system (`First`): update frame timing.
pub fn frame_diagnostics_system(
    time: Res<Time>,
    mut diag: ResMut<Diagnostics>,
    entities: &bevy_ecs::entity::Entities,
) {
    let dt = time.real_delta_secs() as f64;
    if dt > 0.0 {
        let ms = dt * 1000.0;
        diag.frame_ms = if diag.frame_ms == 0.0 {
            ms
        } else {
            diag.frame_ms * 0.95 + ms * 0.05
        };
        diag.fps = 1000.0 / diag.frame_ms.max(1e-6);
    }
    diag.frame = time.frame_count();
    diag.entities = entities.count_spawned();
    if diag.frame.is_multiple_of(15)
        && let Some(mb) = sampled_ram_mb()
    {
        diag.set(PROCESS_RAM_MB, mb);
    }
}

/// The [`Diagnostics`] metric with the process's resident memory, MB (refreshed every 15
/// frames).
pub const PROCESS_RAM_MB: &str = "process.ram_mb";

/// The process's resident memory (RAM it holds now: the working set on Windows), MB.
pub fn process_ram_mb() -> Option<f64> {
    memory_stats::memory_stats().map(|m| m.physical_mem as f64 / (1024.0 * 1024.0))
}

/// The process's resident memory as a background thread last read it (every 100 ms), MB;
/// `None` until its first reading. Never blocks the caller: on Linux reading it takes the
/// address space's lock, which waits behind any thread mapping memory (a worker building
/// art), for 100 ms or more on a busy load.
pub fn sampled_ram_mb() -> Option<f64> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static MB: AtomicU64 = AtomicU64::new(u64::MAX);
    static SAMPLER: std::sync::Once = std::sync::Once::new();
    SAMPLER.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("ram sampler".into())
            .spawn(|| {
                loop {
                    if let Some(mb) = process_ram_mb() {
                        MB.store(mb.to_bits(), Ordering::Relaxed);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
        if spawned.is_err()
            && let Some(mb) = process_ram_mb()
        {
            MB.store(mb.to_bits(), Ordering::Relaxed);
        }
    });
    match MB.load(Ordering::Relaxed) {
        u64::MAX => None,
        bits => Some(f64::from_bits(bits)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_process_ram_reads() {
        let mb = super::process_ram_mb().expect("supported on every target we ship");
        assert!(mb > 1.0 && mb < 1.0e6, "{mb} MB");
    }

    #[test]
    fn the_sampled_ram_arrives_without_blocking() {
        let start = std::time::Instant::now();
        let mb = loop {
            if let Some(mb) = super::sampled_ram_mb() {
                break mb;
            }
            assert!(start.elapsed().as_secs() < 10, "no sample");
            std::thread::yield_now();
        };
        assert!(mb > 1.0 && mb < 1.0e6, "{mb} MB");
    }
}
