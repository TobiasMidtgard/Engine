//! Interactive ripples: a wave-equation heightfield around the focus (the player or camera).
//!
//! Anything that touches the water pushes the surface here: swimmers and boats leave wakes,
//! footsteps and raindrops leave rings, bullets and explosions throw splashes. Ripples travel
//! at `WaterSettings::ripple_speed`, slow down in shallow water (√(g·h)) so they bend toward
//! the shore, reflect off it, and fade. Fast disturbances also leave foam.
//!
//! The grid follows the focus in whole cells and runs on the CPU in parallel row chunks: the
//! heights feed buoyancy and the `Water` query (boats rock on each other's wakes), and the
//! renderer uploads them as a texture each frame.

use engine_core::math::{DVec2, DVec3, Vec2};
use engine_world::WorldShift;

/// Cells at the grid edge where waves are absorbed instead of reflected.
const SPONGE: usize = 10;

/// A push on the surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Disturbance {
    /// World x/z.
    pub pos: DVec2,
    /// Gaussian radius, meters.
    pub radius: f32,
    /// How far the surface is pushed down at the center, meters (negative lifts it).
    pub depth: f32,
    /// Foam left behind, 0..1.
    pub foam: f32,
}

/// Ripple heights, foam and wave speeds on a square grid.
#[derive(Debug, Clone)]
pub struct RippleField {
    /// Cells per side.
    pub n: usize,
    /// Cell size, meters.
    pub cell: f32,
    /// World x/z of the corner of cell (0, 0).
    pub origin: DVec2,
    /// Height above the undisturbed surface, meters.
    pub h: Vec<f32>,
    prev: Vec<f32>,
    /// Foam, 0..1.
    pub foam: Vec<f32>,
    /// Squared wave speed per cell (m²/s²); 0 = dry land.
    pub speed2: Vec<f32>,
    /// Waiting disturbances, applied at the start of the next step.
    pub pending: Vec<Disturbance>,
    /// Bumped whenever heights change (for GPU uploads).
    pub version: u64,
    /// Any wet cell in the grid.
    pub has_water: bool,
}

impl RippleField {
    pub fn new(n: usize, cell: f32) -> Self {
        // The step runs on the compute pool; make sure it exists (no-op in a running app).
        engine_core::tasks::init_task_pools();
        let n = n.max(8);
        Self {
            n,
            cell: cell.max(0.01),
            origin: DVec2::new(
                -(n as f64) * cell as f64 * 0.5,
                -(n as f64) * cell as f64 * 0.5,
            ),
            h: vec![0.0; n * n],
            prev: vec![0.0; n * n],
            foam: vec![0.0; n * n],
            speed2: vec![0.0; n * n],
            pending: Vec::new(),
            version: 0,
            has_water: false,
        }
    }

    /// Side length, meters.
    pub fn size(&self) -> f64 {
        self.n as f64 * self.cell as f64
    }

    /// World x/z of a cell's center.
    pub fn cell_center(&self, i: usize, j: usize) -> DVec2 {
        self.origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * self.cell as f64
    }

    /// Follow the focus: shift the grid by whole cells when it drifts from the center.
    /// Returns true when the grid moved (wave speeds must then be rebuilt).
    pub fn recenter(&mut self, focus: DVec2) -> bool {
        let cell = self.cell as f64;
        let half = self.size() * 0.5;
        let center = self.origin + DVec2::splat(half);
        let d = focus - center;
        if d.x.abs() < half * 0.25 && d.y.abs() < half * 0.25 {
            return false;
        }
        let target = ((focus - DVec2::splat(half)) / cell).round() * cell;
        let shift = ((target - self.origin) / cell).round();
        let (sx, sz) = (shift.x as i64, shift.y as i64);
        self.origin = target;
        let n = self.n as i64;
        if sx.abs() >= n || sz.abs() >= n {
            self.h.fill(0.0);
            self.prev.fill(0.0);
            self.foam.fill(0.0);
        } else {
            for buf in [&mut self.h, &mut self.prev, &mut self.foam] {
                let old = buf.clone();
                for j in 0..n {
                    for i in 0..n {
                        let (oi, oj) = (i + sx, j + sz);
                        buf[(j * n + i) as usize] = if oi >= 0 && oi < n && oj >= 0 && oj < n {
                            old[(oj * n + oi) as usize]
                        } else {
                            0.0
                        };
                    }
                }
            }
        }
        self.version += 1;
        true
    }

    /// Set the wave speed of every cell from the water depth there (`None` = dry).
    pub fn rebuild_speeds(&mut self, speed: f32, depth_at: impl Fn(DVec2) -> Option<f32>) {
        let n = self.n;
        let mut any = false;
        for j in 0..n {
            for i in 0..n {
                let c2 = match depth_at(self.cell_center(i, j)) {
                    Some(d) if d > 0.02 => {
                        any = true;
                        let shallow = 9.81 * d;
                        (speed * speed).min(shallow)
                    }
                    _ => 0.0,
                };
                self.speed2[j * n + i] = c2;
            }
        }
        self.has_water = any;
        if !any {
            self.h.fill(0.0);
            self.prev.fill(0.0);
            self.foam.fill(0.0);
        }
    }

    /// Queue a push on the surface.
    pub fn disturb(&mut self, d: Disturbance) {
        if self.pending.len() < 4096 {
            self.pending.push(d);
        }
    }

    fn apply_pending(&mut self) {
        let n = self.n as i64;
        let cell = self.cell as f64;
        for d in std::mem::take(&mut self.pending) {
            let r = d.radius.max(self.cell * 0.75);
            let reach = (r * 2.0 / self.cell).ceil() as i64;
            let c = ((d.pos - self.origin) / cell).floor();
            let (ci, cj) = (c.x as i64, c.y as i64);
            for j in (cj - reach).max(0)..=(cj + reach).min(n - 1) {
                for i in (ci - reach).max(0)..=(ci + reach).min(n - 1) {
                    let idx = (j * n + i) as usize;
                    if self.speed2[idx] <= 0.0 {
                        continue;
                    }
                    let p = self.cell_center(i as usize, j as usize);
                    let dist = (p - d.pos).length() as f32;
                    let g = (-(dist / r) * (dist / r) * 2.0).exp();
                    if g < 1e-3 {
                        continue;
                    }
                    // A displacement at rest: the surface springs back and rings spread.
                    self.h[idx] -= d.depth * g;
                    self.prev[idx] -= d.depth * g;
                    self.foam[idx] = (self.foam[idx] + d.foam * g).min(1.0);
                }
            }
        }
    }

    /// Advance the ripples by `dt` seconds. `damping` is per second; foam fades to a third in
    /// `foam_life` seconds.
    pub fn step(&mut self, dt: f32, damping: f32, foam_life: f32) {
        if !self.has_water || dt <= 0.0 {
            self.pending.clear();
            return;
        }
        self.apply_pending();
        let max_c2 = self.speed2.iter().copied().fold(0.0f32, f32::max);
        let k_max = max_c2 * dt * dt / (self.cell * self.cell);
        let substeps = ((k_max / 0.4).sqrt().ceil() as u32).clamp(1, 8);
        let sdt = dt / substeps as f32;
        for _ in 0..substeps {
            self.substep(sdt, damping);
        }
        // Foam: fades, and grows where the surface moves fast (breaking ripples, splashes).
        let decay = (-sdt * substeps as f32 / foam_life.max(0.05)).exp();
        let gain = dt * 2.0;
        let n = self.n;
        let (h, prev) = (&self.h, &self.prev);
        engine_core::tasks::par_chunks_mut(&mut self.foam, n * 32, |chunk_index, chunk| {
            let start = chunk_index * n * 32;
            for (k, f) in chunk.iter_mut().enumerate() {
                let idx = start + k;
                let speed = (h[idx] - prev[idx]).abs() / sdt;
                *f = (*f * decay + gain * (speed - 0.35).max(0.0)).min(1.0);
            }
        });
        self.version += 1;
    }

    fn substep(&mut self, dt: f32, damping: f32) {
        let n = self.n;
        let inv_dx2 = 1.0 / (self.cell * self.cell);
        let keep = (1.0 - damping * dt).clamp(0.0, 1.0);
        let (h, speed2) = (&self.h, &self.speed2);
        // `prev` becomes `next` in place: each cell reads only its own old value from it.
        engine_core::tasks::par_chunks_mut(&mut self.prev, n * 16, |chunk_index, chunk| {
            let row0 = chunk_index * 16;
            for (k, out) in chunk.iter_mut().enumerate() {
                let idx = row0 * n + k;
                let (i, j) = (idx % n, idx / n);
                let c2 = speed2[idx];
                if c2 <= 0.0 {
                    *out = 0.0;
                    continue;
                }
                let hc = h[idx];
                // Dry neighbors act as walls (waves reflect off the shore); the grid edge
                // reads as calm water.
                let at = |ii: usize, jj: usize| {
                    let q = jj * n + ii;
                    if speed2[q] > 0.0 { h[q] } else { hc }
                };
                let l = if i > 0 { at(i - 1, j) } else { 0.0 };
                let r = if i + 1 < n { at(i + 1, j) } else { 0.0 };
                let b = if j > 0 { at(i, j - 1) } else { 0.0 };
                let t = if j + 1 < n { at(i, j + 1) } else { 0.0 };
                let lap = (l + r + b + t - 4.0 * hc) * inv_dx2;
                let mut next = hc + (hc - *out) * keep + c2 * dt * dt * lap;
                // Sponge layer: soak up waves near the edge of the moving grid.
                let edge = i.min(j).min(n - 1 - i).min(n - 1 - j);
                if edge < SPONGE {
                    let s = edge as f32 / SPONGE as f32;
                    next *= 0.9 + 0.1 * s;
                }
                *out = next;
            }
        });
        std::mem::swap(&mut self.h, &mut self.prev);
    }

    /// Height (m), gradient (dh/dx, dh/dz) and foam at world x/z, or `None` outside the grid.
    pub fn sample(&self, p: DVec2) -> Option<(f32, Vec2, f32)> {
        if !self.has_water {
            return None;
        }
        let g = (p - self.origin) / self.cell as f64 - DVec2::splat(0.5);
        let n = self.n as f64;
        if g.x < 0.0 || g.y < 0.0 || g.x >= n - 1.0 || g.y >= n - 1.0 {
            return None;
        }
        let (i, j) = (g.x as usize, g.y as usize);
        let (fx, fz) = ((g.x - i as f64) as f32, (g.y - j as f64) as f32);
        let n = self.n;
        let at = |buf: &[f32], ii: usize, jj: usize| buf[jj * n + ii];
        let bil = |buf: &[f32]| {
            let a = at(buf, i, j) + (at(buf, i + 1, j) - at(buf, i, j)) * fx;
            let b = at(buf, i, j + 1) + (at(buf, i + 1, j + 1) - at(buf, i, j + 1)) * fx;
            a + (b - a) * fz
        };
        let h = bil(&self.h);
        let foam = bil(&self.foam);
        let gx = ((at(&self.h, i + 1, j) - at(&self.h, i, j)) * (1.0 - fz)
            + (at(&self.h, i + 1, j + 1) - at(&self.h, i, j + 1)) * fz)
            / self.cell;
        let gz = ((at(&self.h, i, j + 1) - at(&self.h, i, j)) * (1.0 - fx)
            + (at(&self.h, i + 1, j + 1) - at(&self.h, i + 1, j)) * fx)
            / self.cell;
        Some((h, Vec2::new(gx, gz), foam))
    }

    /// Follow a world shift. A plain move keeps every cell and moves the corner. A turn can't
    /// turn the grid (it stays square to the world axes): it is resampled around the same
    /// place, so ripples already running carry on where they were.
    pub fn shift(&mut self, s: &WorldShift) {
        let to_xz = |p: DVec3| DVec2::new(p.x, p.z);
        let moved = |p: DVec2| to_xz(s.point(DVec3::new(p.x, 0.0, p.y)));
        for d in &mut self.pending {
            d.pos = moved(d.pos);
        }
        self.version += 1;
        if s.is_translation_only() {
            self.origin += to_xz(s.translation);
            return;
        }
        let old = self.clone();
        let half = DVec2::splat(self.size() * 0.5);
        self.origin = moved(old.origin + half) - half;
        let n = self.n;
        for j in 0..n {
            for i in 0..n {
                let c = self.cell_center(i, j);
                let back = s.inverse_point(DVec3::new(c.x, 0.0, c.y));
                let k = j * n + i;
                let at = DVec2::new(back.x, back.z);
                self.h[k] = old.bilinear(&old.h, at);
                self.prev[k] = old.bilinear(&old.prev, at);
                self.foam[k] = old.bilinear(&old.foam, at);
                // Speeds follow the depth there; rebuilt properly on the next step.
                let g = ((at - old.origin) / old.cell as f64).floor();
                self.speed2[k] = if g.x >= 0.0 && g.y >= 0.0 && g.x < n as f64 && g.y < n as f64 {
                    old.speed2[g.y as usize * n + g.x as usize]
                } else {
                    0.0
                };
            }
        }
    }

    /// `buf` (one of the grids) at world x/z, bilinear; 0 outside.
    fn bilinear(&self, buf: &[f32], p: DVec2) -> f32 {
        let g = (p - self.origin) / self.cell as f64 - DVec2::splat(0.5);
        let n = self.n as f64;
        if g.x < 0.0 || g.y < 0.0 || g.x >= n - 1.0 || g.y >= n - 1.0 {
            return 0.0;
        }
        let (i, j) = (g.x as usize, g.y as usize);
        let (fx, fz) = ((g.x - i as f64) as f32, (g.y - j as f64) as f32);
        let at = |ii: usize, jj: usize| buf[jj * self.n + ii];
        let a = at(i, j) + (at(i + 1, j) - at(i, j)) * fx;
        let b = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * fx;
        a + (b - a) * fz
    }

    /// Total wave energy (Σ h²), for tests and diagnostics.
    pub fn energy(&self) -> f32 {
        self.h.iter().map(|h| h * h).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pond() -> RippleField {
        let mut f = RippleField::new(128, 0.25);
        f.origin = DVec2::new(-16.0, -16.0);
        f.rebuild_speeds(1.3, |_| Some(5.0));
        f
    }

    #[test]
    fn a_drop_makes_a_ring_that_travels_at_the_ripple_speed() {
        let mut f = pond();
        f.disturb(Disturbance {
            pos: DVec2::ZERO,
            radius: 0.3,
            depth: 0.05,
            foam: 0.5,
        });
        let dt = 1.0 / 60.0;
        for _ in 0..120 {
            f.step(dt, 0.0, 4.0);
        }
        // After 2 s the wavefront is ~2.6 m out: find the strongest disturbance radius.
        let mut best = (0.0f32, 0.0f32);
        for k in 0..60 {
            let r = k as f64 * 0.1;
            let (h, _, _) = f.sample(DVec2::new(r, 0.0)).unwrap();
            if h.abs() > best.1 {
                best = (r as f32, h.abs());
            }
        }
        assert!(
            (best.0 - 2.6).abs() < 0.8,
            "front at {} m (amplitude {})",
            best.0,
            best.1
        );
        // Symmetric ring.
        let a = f.sample(DVec2::new(0.0, best.0 as f64)).unwrap().0;
        let b = f.sample(DVec2::new(best.0 as f64, 0.0)).unwrap().0;
        assert!((a - b).abs() < 0.3 * b.abs().max(1e-4), "{a} vs {b}");
    }

    #[test]
    fn ripples_fade_and_stay_stable() {
        let mut f = pond();
        for k in 0..20 {
            f.disturb(Disturbance {
                pos: DVec2::new(k as f64 * 0.5 - 5.0, 1.0),
                radius: 0.5,
                depth: 0.2,
                foam: 1.0,
            });
        }
        f.step(1.0 / 60.0, 0.35, 4.0);
        let e0 = f.energy();
        for _ in 0..600 {
            f.step(1.0 / 60.0, 0.35, 4.0);
        }
        let e1 = f.energy();
        assert!(e1.is_finite() && e1 < e0 * 0.2, "{e0} -> {e1}");
        assert!(f.foam.iter().all(|x| (0.0..=1.0).contains(x)));
    }

    #[test]
    fn dry_land_stays_flat_and_reflects() {
        let mut f = RippleField::new(64, 0.25);
        f.origin = DVec2::new(-8.0, -8.0);
        f.rebuild_speeds(1.3, |p| (p.x < 2.0).then_some(3.0));
        f.disturb(Disturbance {
            pos: DVec2::new(0.0, 0.0),
            radius: 0.4,
            depth: 0.1,
            foam: 0.0,
        });
        for _ in 0..200 {
            f.step(1.0 / 60.0, 0.1, 4.0);
            for j in 0..64 {
                for i in 0..64 {
                    if f.cell_center(i, j).x >= 2.0 {
                        assert_eq!(f.h[j * 64 + i], 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn recentering_keeps_waves_in_place() {
        let mut f = pond();
        f.disturb(Disturbance {
            pos: DVec2::new(3.0, 2.0),
            radius: 0.5,
            depth: 0.1,
            foam: 0.0,
        });
        f.step(1.0 / 60.0, 0.0, 4.0);
        let before = f.sample(DVec2::new(3.0, 2.0)).unwrap().0;
        assert!(f.recenter(DVec2::new(9.0, 7.0)));
        f.rebuild_speeds(1.3, |_| Some(5.0));
        let after = f.sample(DVec2::new(3.0, 2.0)).unwrap().0;
        assert!((before - after).abs() < 1e-6, "{before} vs {after}");
        assert!(!f.recenter(DVec2::new(9.5, 7.0)));
    }

    #[test]
    fn shallow_water_slows_waves() {
        let mut f = RippleField::new(32, 0.25);
        f.rebuild_speeds(1.3, |p| Some(if p.x < 0.0 { 0.05 } else { 3.0 }));
        let slow = f.speed2[16 * 32 + 2].sqrt();
        let fast = f.speed2[16 * 32 + 30].sqrt();
        assert!((slow - (9.81f32 * 0.05).sqrt()).abs() < 1e-3);
        assert!((fast - 1.3).abs() < 1e-3);
    }
}
