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

/// Metrics published by systems that run side by side: `Res<Metrics>` and [`Metrics::set`]
/// need only read access. Publishing through `ResMut<Diagnostics>` made every publisher
/// conflict with every other, so no two of them ever ran at once (the schedule audit found
/// six in the fixed step and seven after the update, among them the softbody step, smoke and
/// terrain colliders). What is set here lands in [`Diagnostics`] at the start of `Last`
/// ([`publish_metrics_system`]), before the bridge and the profiler read it.
#[derive(Resource, Debug, Default)]
pub struct Metrics(std::sync::Mutex<BTreeMap<String, (f64, bool)>>);

impl Metrics {
    pub fn set(&self, name: &str, value: f64) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match m.get_mut(name) {
            Some(v) => *v = (value, true),
            None => {
                m.insert(name.to_string(), (value, true));
            }
        }
    }
}

/// Engine system (`Last`, before anything else there): the [`Metrics`] set this frame go into
/// [`Diagnostics`].
pub fn publish_metrics_system(metrics: Res<Metrics>, mut diag: ResMut<Diagnostics>) {
    let mut m = metrics.0.lock().unwrap_or_else(|e| e.into_inner());
    for (name, (value, fresh)) in m.iter_mut() {
        if std::mem::take(fresh) {
            diag.set(name, *value);
        }
    }
}

/// Engine system (`First`): update frame timing.
pub fn frame_diagnostics_system(
    time: Res<Time>,
    mut diag: ResMut<Diagnostics>,
    archetypes: &bevy_ecs::archetype::Archetypes,
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
    diag.entities = spawned_in(archetypes);
    if diag.frame.is_multiple_of(15)
        && let Some(mb) = sampled_ram_mb()
    {
        diag.set(PROCESS_RAM_MB, mb);
    }
}

/// Entities spawned in `world`, summed over its archetypes (a few thousand of them) rather
/// than with `Entities::count_spawned`, which walks every entity slot ever allocated: 0.3-0.4
/// ms a frame on the PC's showcase (round 29b), every frame in `frame_diagnostics_system`.
pub fn spawned_entities(world: &bevy_ecs::world::World) -> u32 {
    spawned_in(world.archetypes())
}

fn spawned_in(archetypes: &bevy_ecs::archetype::Archetypes) -> u32 {
    archetypes.iter().map(|a| a.len()).sum()
}

/// The [`Diagnostics`] metric with the process's resident memory, MB (refreshed every 15
/// frames).
pub const PROCESS_RAM_MB: &str = "process.ram_mb";

/// The process's resident memory (RAM it holds now: the working set on Windows), MB.
pub fn process_ram_mb() -> Option<f64> {
    // memory-stats 1.2 on Linux decides between /proc/self/smaps and statm on its first call,
    // and a second thread calling in the middle of that reads statm with a page size of 0
    // (0 MB). The first call runs alone.
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        memory_stats::memory_stats();
    });
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
    fn metrics_reach_the_diagnostics_once_the_frame_ends() {
        use crate::app::App;
        use crate::prelude::*;
        fn publish(metrics: Res<super::Metrics>) {
            metrics.set("test.side_by_side", 3.0);
        }
        let mut app = App::new();
        app.add_systems(Update, publish);
        app.update();
        let d = app.world.resource::<Diagnostics>();
        assert_eq!(d.get("test.side_by_side"), Some(3.0));
        // A value set once stays, as with `Diagnostics::set`.
        app.world
            .resource_mut::<Diagnostics>()
            .set("test.side_by_side", 1.0);
        app.world
            .resource::<super::Metrics>()
            .set("test.other", 2.0);
        app.update();
        let d = app.world.resource::<Diagnostics>();
        assert_eq!(
            d.get("test.side_by_side"),
            Some(3.0),
            "set again this frame"
        );
        assert_eq!(d.get("test.other"), Some(2.0));
    }

    #[test]
    fn spawned_entities_count_what_count_spawned_does() {
        use bevy_ecs::prelude::*;
        #[derive(Component)]
        struct A;
        #[derive(Component)]
        struct B;
        let mut world = World::new();
        let fresh = super::spawned_entities(&world);
        let a: Vec<Entity> = (0..50).map(|_| world.spawn(A).id()).collect();
        for _ in 0..30 {
            world.spawn((A, B));
        }
        world.spawn_empty();
        for e in &a[..20] {
            world.despawn(*e);
        }
        assert_eq!(
            super::spawned_entities(&world),
            world.entities().count_spawned()
        );
        assert_eq!(super::spawned_entities(&world), fresh + 61);
    }

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
