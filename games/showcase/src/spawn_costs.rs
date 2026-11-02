//! What each slice of a zone's spawn cost on the main thread ([`crate::zones::cell_parts`]):
//! the frame a streamed zone loads in, its slice runs inside that frame, so a slow slice is a
//! hitch the player sees. Slices over [`LOG_MS`] are logged with their owner, and the census
//! ([`crate::census`]) lists them all, so each owner sees its own row (the lead's budget: no
//! frame over 33 ms while loading).

use std::time::Duration;

use engine::prelude::*;

/// Slices that took longer than this are logged, ms (half a 60 fps frame).
pub const LOG_MS: f32 = 8.0;

/// One slice run: the cell, who owns the slice, and how long it took (ms).
#[derive(Debug, Clone)]
pub struct Slice {
    pub cell: String,
    pub part: &'static str,
    pub ms: f32,
}

/// Every zone spawn slice run so far, in order.
#[derive(Resource, Default)]
pub struct SpawnCosts(pub Vec<Slice>);

/// Record that slice `part` of `cell` took `took`.
pub fn note(world: &mut World, cell: &str, part: &'static str, took: Duration) {
    let ms = took.as_secs_f32() * 1000.0;
    if ms > LOG_MS {
        engine::core::log::info!("zone spawn slice: {cell} ({part}) took {ms:.1} ms");
    }
    world.get_resource_or_init::<SpawnCosts>().0.push(Slice {
        cell: cell.to_string(),
        part,
        ms,
    });
}

/// A markdown table of the slowest slices (each cell's latest load), slowest first.
pub fn table(world: &World) -> Option<String> {
    let costs = world.get_resource::<SpawnCosts>()?;
    let mut latest: Vec<&Slice> = Vec::new();
    for s in costs.0.iter().rev() {
        if !latest.iter().any(|l| l.cell == s.cell && l.part == s.part) {
            latest.push(s);
        }
    }
    latest.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    let mut out = String::from("| zone | slice (owner) | main thread, ms |\n|---|---|---|\n");
    for s in latest {
        out.push_str(&format!("| {} | {} | {:.1} |\n", s.cell, s.part, s.ms));
    }
    Some(out)
}
