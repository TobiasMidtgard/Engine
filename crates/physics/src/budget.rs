//! The physics step's time budget: when steps keep running over
//! [`PhysicsSettings::step_budget_ms`], big simulations degrade a level at a time instead of
//! dragging the frame rate down (and, with more fixed steps owed each slow frame, spiralling).
//!
//! What games do (`engine-design/physics-perf-research.md`): Havok's speed in Bethesda's games
//! is mostly deactivation, bodies sleeping as soon as they settle; Horizon Forbidden West only
//! simulates near the player; Skyrim drops to one 30 Hz step a frame under load
//! (`fMaxTimeComplex`). Each level here takes the next of those steps, cheapest to see first:
//!
//! | level | settle and sleep  | coarse / half rate from |
//! |-------|-------------------|-------------------------|
//! | 0     | as set            | as set (150 m / 600 m)  |
//! | 1     | 2×, after 0.35 s  | as set                  |
//! | 2     | 4×, after 0.25 s  | 1/2 (75 m / 300 m)      |
//! | 3     | 8×, after 0.15 s  | 1/4 (38 m / 150 m)      |
//!
//! "Settle and sleep" raises the speed under which a body counts as at rest (Rapier's 5 cm/s
//! and 0.5 rad/s as set; 8× is the 0.4 m/s older Rapier used) and shortens how long it must
//! stay so (0.5 s), so a collapsed heap goes to sleep while its last pieces still rock: that is
//! where a heap's time goes, its many slow contacts (the yard after every station replayed:
//! 9.8 ms a step on average without the budget, 8.0 with it, on 4 cores). Fewer solver
//! iterations, which Jolt and PhysX offer per island, were tried and dropped: at 3 of 8 the yard
//! jittered awake and stepped twice as slowly, and launched a body at 100 m/s. A level is taken
//! after the step's smoothed time stays over budget, and given back after it stays well under
//! for two seconds, so a level doesn't flicker; `PhysicsStats::load_level` shows it.
//!
//! Off by default (round 21c, RTX 5080 PC): the yard's busiest step is near 4 ms there, and
//! runs that sat at levels 1-3 spent 10-12 ms a frame in physics against 2 ms at level 0. Those
//! levels were taken on the step's span, which held the frame's other systems running beside
//! it; the budget now reads the step's own work (`PhysicsStats::step_ms`).

/// Highest level.
pub const MAX_LEVEL: u8 = 3;

/// Steps a level is kept at least before the next one is taken.
const UP_AFTER: u32 = 20;
/// Steps the time must stay under [`DOWN_BELOW`] of the budget before a level is given back.
const DOWN_AFTER: u32 = 120;
/// Fraction of the budget the smoothed time must stay under to give a level back.
const DOWN_BELOW: f64 = 0.65;
/// Weight of the newest step in the smoothed time (about the last ten steps count).
const SMOOTH: f64 = 0.15;

/// Rapier's resting speed for bodies that may sleep (normalized: m/s per length unit).
pub(crate) const SLEEP_SPEED: f32 = 0.05;
/// Rapier's default time a body must rest before it sleeps, seconds.
pub(crate) const SLEEP_AFTER: f32 = 0.5;
/// Rapier's default resting spin, rad/s.
const SLEEP_SPIN: f32 = 0.5;

/// How loaded the physics step is.
#[derive(Debug, Default, Clone)]
pub(crate) struct Load {
    /// Current level, 0 to [`MAX_LEVEL`].
    pub level: u8,
    /// Smoothed step time, ms.
    pub smoothed_ms: f64,
    /// Steps since the level last changed.
    held: u32,
    /// Steps in a row well under budget.
    under: u32,
}

impl Load {
    /// A step took `ms`. Returns whether the level changed.
    pub(crate) fn observe(&mut self, ms: f64, budget_ms: Option<f32>) -> bool {
        let Some(budget) = budget_ms.map(|b| b.max(0.1) as f64) else {
            let changed = self.level != 0;
            *self = Self::default();
            return changed;
        };
        self.smoothed_ms = if self.held == 0 && self.smoothed_ms == 0.0 {
            ms
        } else {
            self.smoothed_ms + (ms - self.smoothed_ms) * SMOOTH
        };
        self.held = self.held.saturating_add(1);
        self.under = if self.smoothed_ms < budget * DOWN_BELOW {
            self.under + 1
        } else {
            0
        };
        let before = self.level;
        if self.smoothed_ms > budget && self.held >= UP_AFTER && self.level < MAX_LEVEL {
            self.level += 1;
        } else if self.under >= DOWN_AFTER && self.level > 0 {
            self.level -= 1;
            self.under = 0;
        }
        if self.level != before {
            self.held = 0;
            tracing::info!(
                "physics budget: level {} (steps took {:.1} ms, budget {budget:.1})",
                self.level,
                self.smoothed_ms
            );
        }
        self.level != before
    }

    /// Time to make sure bodies made since the level changed settle as it asks (every
    /// [`crate::lod::LOD_EVERY`] steps).
    pub(crate) fn due(&self) -> bool {
        self.held.is_multiple_of(crate::lod::LOD_EVERY)
    }

    /// The resting speed's multiple and the time to rest before sleeping, seconds.
    pub(crate) fn sleep(&self) -> (f32, f32) {
        [(1.0, SLEEP_AFTER), (2.0, 0.35), (4.0, 0.25), (8.0, 0.15)]
            [self.level.min(MAX_LEVEL) as usize]
    }

    /// The level-of-detail distances at this level, as a share of the settings'.
    pub(crate) fn lod_scale(&self) -> f64 {
        [1.0, 1.0, 0.5, 0.25][self.level.min(MAX_LEVEL) as usize]
    }
}

/// Make the bodies of `world` that may sleep settle as soon as `load` asks (and back).
pub(crate) fn apply_sleep(world: &mut rapier3d::prelude::PhysicsWorld, load: &Load) {
    let (scale, after) = load.sleep();
    let speed = SLEEP_SPEED * scale;
    // Look before touching: a body taken mutably counts as changed by the user, which Rapier
    // then re-checks at the next step.
    let due: Vec<_> = world
        .bodies
        .iter()
        .filter(|(_, rb)| {
            let a = rb.activation();
            // (A body that may not sleep has a negative threshold.)
            !rb.is_fixed()
                && a.normalized_linear_threshold >= 0.0
                && a.normalized_linear_threshold != speed
        })
        .map(|(h, _)| h)
        .collect();
    for h in due {
        if let Some(rb) = world.bodies.get_mut(h) {
            let a = rb.activation_mut();
            a.normalized_linear_threshold = speed;
            a.angular_threshold = SLEEP_SPIN * scale;
            a.time_until_sleep = after;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_is_taken_after_steps_stay_over_budget_and_given_back_well_under() {
        let mut l = Load::default();
        for _ in 0..UP_AFTER - 1 {
            l.observe(20.0, Some(8.0));
        }
        assert_eq!(l.level, 0, "not before {UP_AFTER} steps");
        l.observe(20.0, Some(8.0));
        assert_eq!(l.level, 1);
        for _ in 0..200 {
            l.observe(20.0, Some(8.0));
        }
        assert_eq!(l.level, MAX_LEVEL);
        // A step or two under budget gives nothing back.
        for _ in 0..30 {
            l.observe(1.0, Some(8.0));
        }
        assert_eq!(l.level, MAX_LEVEL);
        for _ in 0..DOWN_AFTER * 4 {
            l.observe(1.0, Some(8.0));
        }
        assert_eq!(l.level, 0);
        // No budget, no degrading.
        for _ in 0..200 {
            l.observe(50.0, None);
        }
        assert_eq!(l.level, 0);
    }
}
