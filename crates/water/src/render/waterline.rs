//! The camera at the waterline (`water.wgsl`'s "the waterline" and "the lens"): how near the
//! water's surface the camera has to be for the screen to split, and how wet its lens is after
//! it comes up out of the water.

/// Radius of the lens's dome, m: a pixel sees from under the water when the dome's point along
/// its ray is under it. Over-under shots are taken through domes about 20 cm across, which
/// hold the waterline steady across the frame while the water laps at it; the camera's 5 cm
/// near plane would flip the whole screen with every ripple.
pub(crate) const DOME_M: f32 = 0.1;
/// What the ripples and the swash can add to the waves' height, m: how much farther than the
/// waves reach the screen may still split.
pub(crate) const SLACK_M: f64 = 0.15;
/// How fast the lens dries after it surfaces: its wetness falls as exp(−t/τ), each drop drying
/// off at its own threshold.
const WET_TAU_S: f32 = 5.0;
/// Past this the lens is dry.
const DRY_S: f32 = 20.0;

/// Water on the main view's lens.
#[derive(Default)]
pub(crate) struct Lens {
    /// The camera was under the water at the last step.
    under: bool,
    /// Seconds since it came out, while the lens is still wet.
    since: Option<f32>,
    /// Changes every time it comes out, so each time leaves its own drops.
    seed: f32,
}

impl Lens {
    /// One frame of the main view, `dt` seconds on, the camera `depth` meters under the water
    /// (negative above it). The lens is wet once it was all under (its dome too) and the
    /// camera has come back up: a lens lapped at the waterline stays as it is. After a camera
    /// `cut` (a teleport, another camera) it starts dry: it never came up out of anything.
    pub fn step(&mut self, depth: f32, dt: f32, cut: bool) {
        let all_under = depth > DOME_M;
        if cut {
            self.under = all_under;
            self.since = None;
        } else if all_under {
            self.under = true;
            self.since = None;
        } else if depth < 0.0 && std::mem::take(&mut self.under) {
            self.since = Some(0.0);
            self.seed = (self.seed + 1.618).rem_euclid(97.0);
        } else if let Some(t) = &mut self.since {
            *t += dt.max(0.0);
            if *t > DRY_S {
                self.since = None;
            }
        }
    }

    /// 1 just out of the water, falling to 0 as it dries.
    pub fn wetness(&self) -> f32 {
        self.since.map_or(0.0, |t| (-t / WET_TAU_S).exp())
    }

    /// Seconds since the camera came out (0 while dry).
    pub fn since(&self) -> f32 {
        self.since.unwrap_or(0.0)
    }

    pub fn seed(&self) -> f32 {
        self.seed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coming_up_wets_the_lens_and_it_dries() {
        let mut lens = Lens::default();
        for _ in 0..30 {
            lens.step(-1.0, 1.0 / 60.0, false);
        }
        assert_eq!(lens.wetness(), 0.0, "never in the water");
        for _ in 0..30 {
            lens.step(1.0, 1.0 / 60.0, false);
        }
        assert_eq!(lens.wetness(), 0.0, "under the water nothing clings to it");
        let seed = lens.seed();
        lens.step(-1.0, 1.0 / 60.0, false);
        assert_eq!(lens.wetness(), 1.0, "just out");
        assert_ne!(lens.seed(), seed, "new drops each time");
        for _ in 0..180 {
            lens.step(-1.0, 1.0 / 60.0, false);
        }
        assert!((lens.since() - 3.0).abs() < 1e-3);
        assert!((lens.wetness() - (-0.6f32).exp()).abs() < 1e-3);
        for _ in 0..(DRY_S * 60.0) as usize {
            lens.step(-1.0, 1.0 / 60.0, false);
        }
        assert_eq!(lens.wetness(), 0.0, "dry again");
        lens.step(1.0, 1.0 / 60.0, false);
        lens.step(-1.0, 1.0 / 60.0, false);
        assert_eq!(lens.wetness(), 1.0, "and wet again after the next dip");
        lens.step(1.0, 1.0 / 60.0, false);
        lens.step(-1.0, 1.0 / 60.0, true);
        assert_eq!(
            lens.wetness(),
            0.0,
            "a cut out of the water leaves the lens dry"
        );
        lens.step(-1.0, 1.0 / 60.0, false);
        assert_eq!(lens.wetness(), 0.0);
        for depth in [0.05, -0.02, 0.08, -0.05] {
            lens.step(depth, 1.0 / 60.0, false);
        }
        assert_eq!(
            lens.wetness(),
            0.0,
            "lapped at the waterline, never all under"
        );
    }
}
