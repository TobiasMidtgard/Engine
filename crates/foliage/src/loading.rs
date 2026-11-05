//! Foliage's loading work kept off the frame: species visuals (meshes and painted textures)
//! and the regions' soil and history are worked out on worker threads, then put into the
//! world a slice at a time, as Unreal's async loading (worker threads, then game-thread
//! `PostLoad` within `s.AsyncLoadingTimeLimit`) and Unity's `backgroundLoadingPriority` (a few
//! ms a frame integrating what the loading threads made) do. A loading screen waits on
//! [`FoliageLoading::is_ready`].

use engine_core::prelude::*;
use engine_world::LoadingBudget;

/// How foliage loads, and how far along it is.
#[derive(Resource, Debug, Clone)]
pub struct FoliageLoading {
    /// Build on worker threads and put the results in place over frames (true), or build
    /// everything in the frame that needs it (tests, tools that want it at once). Also where
    /// the regions' live ecology steps run: on a worker, or at once in the tick (both land at
    /// the next fixed tick, so the forest grows the same either way).
    pub background: bool,
    /// Most milliseconds a frame spends putting finished builds into the world during play.
    /// While a loading screen is up (`engine_world::LoadingBudget` present) its budget
    /// applies instead, as Unity's High background loading priority does.
    pub budget_ms: f32,
    /// The species visuals have been looked at once (before that, nothing is known to build).
    pub(crate) species_checked: bool,
    pub(crate) species_building: usize,
    pub(crate) regions_building: usize,
    /// New plants or scenery trees left without their parts last frame (out of time).
    pub(crate) plants_waiting: bool,
    pub(crate) trees_waiting: bool,
    /// A region's grass and forest floor fields are still being mapped for the first time.
    pub(crate) floors_waiting: bool,
    /// Ground covers being rebuilt or waiting for their regions.
    pub(crate) scenery_building: usize,
    /// Far woods cells building on workers or waiting to go in place.
    pub(crate) far_building: usize,
    /// How long the last frame took on the wall clock (ms), measured by [`measure_frames`].
    pub(crate) last_frame_ms: f32,
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
            floors_waiting: false,
            scenery_building: 0,
            far_building: 0,
            last_frame_ms: 0.0,
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

    /// No species visuals or regions still building, every plant and scenery tree has its
    /// parts, and every region its grass and floor fields. Regions are seen the frame after they are spawned (in `PreUpdate`), so wait
    /// a frame after spawning them before asking.
    pub fn is_ready(&self) -> bool {
        self.species_checked
            && self.species_building == 0
            && self.regions_building == 0
            && !self.plants_waiting
            && !self.trees_waiting
            && !self.floors_waiting
            && self.scenery_building == 0
            && self.far_building == 0
    }

    /// Milliseconds this frame may spend putting finished builds in place: the play budget,
    /// the loading screen's while one is up, unlimited when loading in the frame.
    pub(crate) fn frame_budget_ms(&self, screen: Option<&LoadingBudget>) -> f32 {
        match (self.background, screen) {
            (false, _) => f32::INFINITY,
            (true, Some(screen)) => (screen.budget_ms as f32)
                .max(self.budget_ms)
                .max(self.frame_share_ms()),
            (true, None) => self.budget_ms.max(self.frame_share_ms()),
        }
    }

    /// A tenth of the last frame: when frames are slow anyway (a headless render takes
    /// seconds), loading a few ms of each would take most of an hour and change nothing a
    /// player feels.
    fn frame_share_ms(&self) -> f32 {
        0.1 * self.last_frame_ms.min(5000.0)
    }

    /// How many new plants, or new scenery trees, get their parts in one frame. In play the
    /// count is capped as well as the time: the time budget grows with slow frames, and each
    /// one's parts cost several times the measured work again when their spawns apply and
    /// the renderer resolves them, so a slow frame would make the next one slower still.
    /// Behind a loading screen, loading in the frame, or in a headless render (frames of
    /// seconds), only time counts.
    pub(crate) fn fresh_cap(&self, screen: Option<&LoadingBudget>) -> u32 {
        if self.background && screen.is_none() && self.last_frame_ms < HEADLESS_FRAME_MS {
            FRESH_PER_FRAME
        } else {
            u32::MAX
        }
    }

    /// How long a frame spends giving new plants and trees their parts: a third of the
    /// budget (their spawns apply as commands after).
    pub(crate) fn fresh_budget(&self, screen: Option<&LoadingBudget>) -> std::time::Duration {
        let ms = self.frame_budget_ms(screen);
        if ms.is_finite() {
            std::time::Duration::from_secs_f32(ms.max(0.0) / 3000.0)
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

/// New plants (or scenery trees) given their parts per frame in play: 48 trees spawn about 150
/// part entities.
pub(crate) const FRESH_PER_FRAME: u32 = 48;

/// Frames slower than this are a headless render's, not a player's.
const HEADLESS_FRAME_MS: f32 = 3000.0;

/// Notes how long each frame takes on the wall clock (for [`FoliageLoading`]'s budgets).
pub(crate) fn measure_frames(
    mut loading: ResMut<FoliageLoading>,
    mut last: Local<Option<std::time::Instant>>,
) {
    let now = std::time::Instant::now();
    if let Some(before) = last.replace(now) {
        loading.bypass_change_detection().last_frame_ms =
            now.duration_since(before).as_secs_f32() * 1000.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_frames_get_a_bigger_loading_budget() {
        let mut l = FoliageLoading {
            last_frame_ms: 16.0,
            ..Default::default()
        };
        assert_eq!(l.frame_budget_ms(None), l.budget_ms);
        // A headless render frame of 20 s may load for 0.5 s (a tenth, capped at 5 s).
        l.last_frame_ms = 20_000.0;
        assert_eq!(l.frame_budget_ms(None), 500.0);
        assert!(
            FoliageLoading::in_frame()
                .frame_budget_ms(None)
                .is_infinite()
        );
    }

    #[test]
    fn in_play_a_slow_frame_does_not_let_the_next_spawn_more_trees() {
        let mut l = FoliageLoading {
            last_frame_ms: 16.0,
            ..Default::default()
        };
        assert_eq!(l.fresh_cap(None), FRESH_PER_FRAME);
        // A hitch of a second: the time budget grows, the count does not.
        l.last_frame_ms = 1000.0;
        assert_eq!(l.fresh_cap(None), FRESH_PER_FRAME);
        // A headless render, a loading screen, or loading in the frame: time only.
        l.last_frame_ms = 20_000.0;
        assert_eq!(l.fresh_cap(None), u32::MAX);
        l.last_frame_ms = 16.0;
        let screen = LoadingBudget { budget_ms: 50.0 };
        assert_eq!(l.fresh_cap(Some(&screen)), u32::MAX);
        assert_eq!(FoliageLoading::in_frame().fresh_cap(None), u32::MAX);
    }
}
