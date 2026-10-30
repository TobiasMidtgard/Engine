//! Foam and bubbles far behind: what the ripple grid's wakes leave on the water after they have
//! passed out of it.
//!
//! The ripple grid ([`crate::ripples`]) is 64 m across and follows the focus; a motorboat's
//! pale trail of bubbles lasts tens of seconds and runs a hundred meters and more behind it,
//! and from the air the grid's foam stopping at the grid's edge reads as a disc of detailed
//! water (playtest 2026-10-29 item 7). So the grid hands its foam and bubbles down to this
//! coarser field, four times as wide (2 m cells, 256 m), which lets them fade where the grid
//! no longer reaches: Crest keeps all its water data in such viewer-centred cascades, each
//! level twice the last, blending each into the next over its outer part; Sea of Thieves
//! blurs its foam with feedback so it disperses instead of being cut off.
//!
//! The field holds no waves or currents: just what has been churned white or pale, fading
//! (the foam as residual lace, at [`crate::ripples::RESIDUAL_LIFE`] times the foam's life) and
//! spreading a little. Under the grid it keeps the most the grid's cells in each of its own
//! hold, so where the grid fades out (`water.wgsl`'s `trail_at`) the trail takes over with
//! what was just there.

use engine_core::math::DVec2;

/// Cells per side.
pub const N: usize = 128;
/// Cell size, m.
pub const CELL: f32 = 2.0;
/// Under this, nothing is left.
const STILL: f32 = 0.004;
/// How fast what the trail holds spreads sideways, m²/s (a wake's turbulence mixes its
/// bubbles out; at 2 m cells only the widening over tens of seconds shows).
const SPREAD: f32 = 0.04;

/// Foam and bubbles on a coarse grid round the ripple grid.
#[derive(Clone, Debug)]
pub struct Trail {
    /// World x/z of the corner of cell (0, 0).
    pub origin: DVec2,
    /// Foam, 0..1.
    pub foam: Vec<f32>,
    /// Bubbles under the water, 0..1.
    pub bubbles: Vec<f32>,
    /// Bumped whenever they change (for GPU uploads).
    pub version: u64,
    /// Anything left anywhere.
    pub live: bool,
    scratch: Vec<f32>,
}

impl Default for Trail {
    fn default() -> Self {
        Self {
            origin: DVec2::ZERO,
            foam: vec![0.0; N * N],
            bubbles: vec![0.0; N * N],
            version: 0,
            live: false,
            scratch: vec![0.0; N * N],
        }
    }
}

impl Trail {
    /// Side, m.
    pub fn size() -> f64 {
        (N as f32 * CELL) as f64
    }

    /// Keep centred on `center` (the ripple grid's middle), snapped to whole cells; what it
    /// holds stays where it is on the water.
    pub fn follow(&mut self, center: DVec2) {
        let cell = CELL as f64;
        let target = ((center - DVec2::splat(Self::size() * 0.5)) / cell).round() * cell;
        let shift = ((target - self.origin) / cell).round();
        if shift == DVec2::ZERO {
            return;
        }
        self.origin = target;
        self.version += 1;
        if !self.live {
            return;
        }
        let (sx, sz, n) = (shift.x as i64, shift.y as i64, N as i64);
        for buf in [&mut self.foam, &mut self.bubbles] {
            self.scratch.copy_from_slice(buf);
            for j in 0..n {
                for i in 0..n {
                    let (oi, oj) = (i + sx, j + sz);
                    buf[(j * n + i) as usize] = if oi >= 0 && oi < n && oj >= 0 && oj < n {
                        self.scratch[(oj * n + oi) as usize]
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    /// Move the corner with the world (a plain move of the floating origin).
    pub fn translate(&mut self, by: DVec2) {
        self.origin += by;
        self.version += 1;
    }

    pub fn clear(&mut self) {
        self.foam.fill(0.0);
        self.bubbles.fill(0.0);
        self.live = false;
        self.version += 1;
    }

    /// The cell holding world x/z `p`, if any.
    pub fn cell_at(&self, p: DVec2) -> Option<usize> {
        let g = ((p - self.origin) / CELL as f64).floor();
        (g.x >= 0.0 && g.y >= 0.0 && g.x < N as f64 && g.y < N as f64)
            .then(|| g.y as usize * N + g.x as usize)
    }

    /// Keep at least `foam` and `bubbles` in the cell holding `p`.
    pub fn deposit(&mut self, p: DVec2, foam: f32, bubbles: f32) {
        if let Some(k) = self.cell_at(p) {
            self.foam[k] = self.foam[k].max(foam);
            self.bubbles[k] = self.bubbles[k].max(bubbles);
            self.live = true;
        }
    }

    /// `dt` seconds on: the foam fades to a third in `foam_life` s, the bubbles in
    /// `bubble_life` s, both spreading a little.
    pub fn step(&mut self, dt: f32, foam_life: f32, bubble_life: f32) {
        if !self.live || dt <= 0.0 {
            return;
        }
        let k = (SPREAD * dt / (CELL * CELL)).min(0.2);
        let mut live = false;
        for (buf, life) in [
            (&mut self.foam, foam_life),
            (&mut self.bubbles, bubble_life),
        ] {
            let decay = (-dt / life.max(0.05)).exp();
            self.scratch.copy_from_slice(buf);
            let b = &self.scratch;
            for j in 0..N {
                for i in 0..N {
                    let idx = j * N + i;
                    let c = b[idx];
                    let side = |o: Option<usize>| o.map_or(0.0, |o| b[o] - c);
                    let spread = side((i > 0).then(|| idx - 1))
                        + side((i + 1 < N).then_some(idx + 1))
                        + side((j > 0).then(|| idx - N))
                        + side((j + 1 < N).then_some(idx + N));
                    let v = (c + k * spread) * decay;
                    buf[idx] = if v < STILL { 0.0 } else { v };
                    live |= v >= STILL;
                }
            }
        }
        self.live = live;
        self.version += 1;
    }

    /// Foam and bubbles at world x/z `p`, bilinear (0 outside).
    pub fn sample(&self, p: DVec2) -> (f32, f32) {
        let g = (p - self.origin) / CELL as f64 - DVec2::splat(0.5);
        if g.x < 0.0 || g.y < 0.0 || g.x >= (N - 1) as f64 || g.y >= (N - 1) as f64 {
            return (0.0, 0.0);
        }
        let (i, j) = (g.x as usize, g.y as usize);
        let (fx, fz) = ((g.x - i as f64) as f32, (g.y - j as f64) as f32);
        let at = |buf: &[f32]| {
            let k = j * N + i;
            let a = buf[k] + (buf[k + 1] - buf[k]) * fx;
            let b = buf[k + N] + (buf[k + N + 1] - buf[k + N]) * fx;
            a + (b - a) * fz
        };
        (at(&self.foam), at(&self.bubbles))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trail_stays_on_the_water_fades_and_spreads() {
        let mut t = Trail::default();
        t.follow(DVec2::new(10.0, 10.0));
        let p = DVec2::new(30.0, 10.0);
        t.deposit(p, 1.0, 1.0);
        let (f0, b0) = t.sample(p);
        assert!(f0 > 0.2 && b0 > 0.2, "{f0} {b0}");
        // The grid moves on: the trail stays where it was laid.
        t.follow(DVec2::new(60.0, 10.0));
        let (f1, _) = t.sample(p);
        assert!((f1 - f0).abs() < 1e-6, "{f0} -> {f1}");
        for _ in 0..30 {
            t.step(1.0 / 3.0, 12.0, 20.0);
        }
        let (f2, b2) = t.sample(p);
        // Ten seconds: foam (life 12 s) and bubbles (20 s) fade, the bubbles less.
        assert!(f2 < f1 * 0.6 && f2 > 0.0, "{f1} -> {f2}");
        assert!(b2 > f2, "{b2} {f2}");
        let (_, beside) = t.sample(p + DVec2::new(0.0, CELL as f64));
        assert!(beside > 0.0, "spreads sideways");
        for _ in 0..3000 {
            t.step(1.0 / 3.0, 12.0, 20.0);
        }
        assert!(!t.live);
        assert!(t.foam.iter().chain(&t.bubbles).all(|v| *v == 0.0));
    }

    #[test]
    fn moving_far_away_leaves_nothing_behind() {
        let mut t = Trail::default();
        t.follow(DVec2::ZERO);
        t.deposit(DVec2::new(5.0, 5.0), 0.5, 0.5);
        t.follow(DVec2::new(1000.0, 0.0));
        assert_eq!(t.sample(DVec2::new(5.0, 5.0)), (0.0, 0.0));
        assert!(t.foam.iter().all(|v| *v == 0.0));
    }
}
