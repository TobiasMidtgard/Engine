//! Foliage's loading work kept off the frame: species visuals (meshes and painted textures)
//! and the regions' soil and history are worked out on worker threads, then put into the
//! world a slice at a time, as Unreal's async loading (worker threads, then game-thread
//! `PostLoad` within `s.AsyncLoadingTimeLimit`) and Unity's `backgroundLoadingPriority` (a few
//! ms a frame integrating what the loading threads made) do. A loading screen waits on
//! [`FoliageLoading::is_ready`].

use engine_core::prelude::*;

/// How foliage loads, and how far along it is.
#[derive(Resource, Debug, Clone)]
pub struct FoliageLoading {
    /// Build on worker threads and put the results in place over frames (true), or build
    /// everything in the frame that needs it (tests, tools that want it at once).
    pub background: bool,
    /// Most milliseconds a frame spends putting finished builds into the world.
    pub budget_ms: f32,
    /// The species visuals have been looked at once (before that, nothing is known to build).
    pub(crate) species_checked: bool,
    pub(crate) species_building: usize,
    pub(crate) regions_building: usize,
    /// New plants or scenery trees left without their parts last frame (out of time).
    pub(crate) plants_waiting: bool,
    pub(crate) trees_waiting: bool,
}

impl Default for FoliageLoading {
    fn default() -> Self {
        Self {
            background: true,
            budget_ms: 6.0,
            species_checked: false,
            species_building: 0,
            regions_building: 0,
            plants_waiting: false,
            trees_waiting: false,
        }
    }
}

impl FoliageLoading {
    /// Everything in the frame at once, as before background loading.
    pub fn in_frame() -> Self {
        Self {
            background: false,
            ..Default::default()
        }
    }

    /// No species visuals or regions still building, and every plant and scenery tree has
    /// its parts. Regions are seen the frame after they are spawned (in `PreUpdate`), so wait
    /// a frame after spawning them before asking.
    pub fn is_ready(&self) -> bool {
        self.species_checked
            && self.species_building == 0
            && self.regions_building == 0
            && !self.plants_waiting
            && !self.trees_waiting
    }

    /// How long a frame spends giving new plants and trees their parts: a third of the
    /// budget (their spawns apply as commands after), unlimited when loading in the frame.
    pub(crate) fn fresh_budget(&self) -> std::time::Duration {
        if self.background {
            std::time::Duration::from_secs_f32(self.budget_ms.max(0.0) / 3000.0)
        } else {
            std::time::Duration::MAX
        }
    }

    /// Species whose visuals are missing or out of date.
    pub fn species_building(&self) -> usize {
        self.species_building
    }

    /// Regions still working out their history or planting it.
    pub fn regions_building(&self) -> usize {
        self.regions_building
    }
}
