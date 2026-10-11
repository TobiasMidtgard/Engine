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
}
