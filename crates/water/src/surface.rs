//! Dispersive ripples over the ripple grid ([`crate::ripples`]).
//!
//! The ripple grid is a shallow-water solver: every wave on it runs at one speed, so a boat
//! leaves a plain V and a stone a single ring. Real ripples disperse: long waves outrun short
//! ones, a dropped stone sends out a train of rings that spreads as it goes, and a moving hull
//! draws a Kelvin wake (the 19.5° wedge of feathered crests with the transverse waves inside).
//! This surface evolves exactly for every wavelength at once in Fourier space (Tessendorf's
//! "eWave", 2014; the hybrid with a shallow-water solver for the currents is Jeschke & Wojtan,
//! "Generalizing Shallow Water Simulations with Dispersive Surface Waves", SIGGRAPH 2023):
//! the grid keeps the currents, swirls and foam and hands the surface every push it takes;
//! the surface is carried along the grid's currents and is what is drawn and floated on.
//!
//! The renderer runs it on the GPU (`render/surface.rs`, `surface.wgsl`) and reads its heights
//! back for floating things a frame or two later. Without a GPU it can run here instead
//! ([`SurfaceSim`], `WaterSettings::cpu_surface`): headless servers that want the same waves,
//! galleries that simulate without drawing, and the tests, for which it is the reference.
//! The two step the same way, and the GPU picks up where this one left off.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use engine_core::math::{DVec2, Vec2};
use engine_core::prelude::*;
use parking_lot::{Mutex, RwLock};

use crate::body::WaterSettings;
use crate::ripples::{Disturbance, RippleField};
use crate::state::SharedWater;

/// Cells per side the surface runs on (the default ripple grid; other sizes keep the grid's
/// own waves).
pub const N: usize = 256;
/// Water depth the waves feel, meters: past a wavelength or so it hardly matters, and the
/// ripple grid's waves are a few meters long at most.
pub const DEPTH_M: f32 = 2.5;
/// Damps the shortest waves the grid holds (two cells), m²/s: they'd ring on as noise.
pub const VISCOSITY: f32 = 0.008;
/// Surface tension over density, m³/s².
pub const TENSION: f32 = 7.4e-5;
/// Longest step the surface takes at once, seconds (the evolution is exact for any step; the
/// currents' carry isn't).
pub const MAX_DT: f32 = 0.05;
/// How long the surface keeps running after the last push, seconds: by then the waves have
/// died out or left the grid.
pub const RING_S: f32 = 25.0;
/// Cells at the grid's edge where the waves are soaked up (the transform wraps around).
pub const SPONGE: f32 = 20.0;
const G: f32 = 9.81;
/// Most pushes kept waiting (one step's worth is a few dozen).
const MAX_STAMPS: usize = 1024;

/// Who runs the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Runner {
    Nobody = 0,
    Gpu = 1,
    Cpu = 2,
}

/// The surface's latest state.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceHeights {
    /// World x/z of the corner of cell (0, 0).
    pub origin: DVec2,
    /// Cells per side.
    pub n: usize,
    /// Cell size, meters.
    pub cell: f32,
    /// Height above the undisturbed surface per cell, meters.
    pub h: Vec<f32>,
    /// Velocity potential per cell (m²/s) when the CPU ran it (the GPU picks up from it);
    /// empty from the GPU.
    pub phi: Vec<f32>,
}

impl SurfaceHeights {
    /// (height, (dh/dx, dh/dz)) at world x/z, or `None` outside the grid.
    pub fn sample(&self, p: DVec2) -> Option<(f32, Vec2)> {
        let g = (p - self.origin) / self.cell as f64 - DVec2::splat(0.5);
        let m = self.n as f64 - 1.0;
        if self.h.len() != self.n * self.n || g.x < 0.0 || g.y < 0.0 || g.x >= m || g.y >= m {
            return None;
        }
        let (i, j) = (g.x as usize, g.y as usize);
        let (fx, fz) = ((g.x - i as f64) as f32, (g.y - j as f64) as f32);
        let n = self.n;
        let at = |ii: usize, jj: usize| self.h[jj * n + ii];
        let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
        let height = (a + (b - a) * fx) * (1.0 - fz) + (c + (d - c) * fx) * fz;
        let gx = ((b - a) * (1.0 - fz) + (d - c) * fz) / self.cell;
        let gz = ((c - a) * (1.0 - fx) + (d - b) * fx) / self.cell;
        Some((height, Vec2::new(gx, gz)))
    }
}

/// Shared by every copy of one ripple grid (the stepped field and the readers' copy) and
/// whoever runs the surface.
#[derive(Debug, Default)]
pub struct SurfaceLink {
    runner: AtomicU8,
    /// Set by the GPU each frame it steps the surface: a renderer that stops drawing (a
    /// gallery simulating without pictures) lets the CPU take over.
    beat: AtomicBool,
    stamps: Mutex<Vec<Disturbance>>,
    heights: RwLock<Option<Arc<SurfaceHeights>>>,
}

impl SurfaceLink {
    pub fn runner(&self) -> Runner {
        match self.runner.load(Ordering::Relaxed) {
            1 => Runner::Gpu,
            2 => Runner::Cpu,
            _ => Runner::Nobody,
        }
    }

    /// Whether the surface is running (its heights are the ones to read).
    pub fn running(&self) -> bool {
        self.runner() != Runner::Nobody
    }

    /// `runner` takes the surface on. The GPU takes it from the CPU (which then stands
    /// aside), never the other way round while the GPU has it.
    pub fn claim(&self, runner: Runner) -> bool {
        let now = self.runner();
        if now == runner || (now == Runner::Gpu && runner == Runner::Cpu) {
            return now == runner;
        }
        self.runner.store(runner as u8, Ordering::Relaxed);
        true
    }

    /// `runner` stops running the surface (if it has it). Its heights are forgotten.
    pub fn release(&self, runner: Runner) {
        if self.runner() != runner {
            return;
        }
        self.runner.store(Runner::Nobody as u8, Ordering::Relaxed);
        self.stamps.lock().clear();
        *self.heights.write() = None;
    }

    /// The GPU stepped the surface this frame.
    pub fn beat(&self) {
        self.beat.store(true, Ordering::Relaxed);
    }

    /// Whether the GPU stepped the surface since the last call.
    pub fn take_beat(&self) -> bool {
        self.beat.swap(false, Ordering::Relaxed)
    }

    /// The CPU takes the surface from a GPU that has stopped stepping it.
    fn take_from_idle_gpu(&self) {
        if self.runner() == Runner::Gpu {
            self.runner.store(Runner::Cpu as u8, Ordering::Relaxed);
        }
    }

    /// Keep a push the grid just took for the surface.
    pub fn record(&self, d: &Disturbance) {
        if !self.running() || d.depth == 0.0 {
            return;
        }
        let mut stamps = self.stamps.lock();
        if stamps.len() < MAX_STAMPS {
            stamps.push(*d);
        }
    }

    /// The pushes taken since the last call.
    pub fn take_stamps(&self) -> Vec<Disturbance> {
        std::mem::take(&mut *self.stamps.lock())
    }

    /// Hand out the surface's latest state.
    pub fn publish(&self, heights: SurfaceHeights) {
        *self.heights.write() = Some(Arc::new(heights));
    }

    /// The surface's latest state, while it runs.
    pub fn heights(&self) -> Option<Arc<SurfaceHeights>> {
        if !self.running() {
            return None;
        }
        self.heights.read().clone()
    }
}

/// Rasterize a push into `push`, for an `n`-cell grid with its corner at `origin`; returns the
/// rows touched. The dip is the one [`RippleField`] applies (a Gaussian), less a ring twice as
/// wide that holds the water it pushed aside: no water comes or goes. (A hull pushes every
/// step; were each push to take water away, the lake would sink round its course in
/// basin-wide swells, which the dispersive surface carries far faster than the grid.)
pub fn rasterize(
    push: &mut [f32],
    n: usize,
    cell: f32,
    origin: DVec2,
    d: &Disturbance,
) -> Option<(usize, usize)> {
    let r = d.radius.max(cell * 0.75);
    let reach = (r * 4.0 / cell).ceil() as i64;
    let c = ((d.pos - origin) / cell as f64).floor();
    let (ci, cj) = (c.x as i64, c.y as i64);
    let n_i = n as i64;
    let (i0, i1) = ((ci - reach).max(0), (ci + reach).min(n_i - 1));
    let (j0, j1) = ((cj - reach).max(0), (cj + reach).min(n_i - 1));
    if i0 > i1 || j0 > j1 {
        return None;
    }
    for j in j0..=j1 {
        for i in i0..=i1 {
            let p = origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * cell as f64;
            let dist = (p - d.pos).length() as f32;
            let x = dist / r;
            let g = (-x * x * 2.0).exp() - 0.25 * (-x * x * 0.5).exp();
            push[j as usize * n + i as usize] -= d.depth * g;
        }
    }
    Some((j0 as usize, j1 as usize))
}

/// How much of a step's waves a cell keeps near the grid's edge (the sponge), for a step of
/// `dt` seconds.
pub fn sponge(n: usize, i: usize, j: usize, dt: f32) -> f32 {
    let edge = i.min(j).min(n - 1 - i).min(n - 1 - j) as f32;
    let t = (1.0 - edge / SPONGE).clamp(0.0, 1.0);
    (-6.0 * t * t * dt).exp()
}

/// The signed wavenumber index of bin `i` of `n` (0..n/2−1, then −n/2..−1).
fn wave_index(i: usize, n: usize) -> f32 {
    if i >= n / 2 {
        i as f32 - n as f32
    } else {
        i as f32
    }
}

/// Turn one mode: the spectra of height `h` and potential `phi` at wavenumber `k` (1/m) after
/// `dt` seconds, damped by `damping` (1/s) and the viscosity.
pub fn evolve_mode(
    h: [f32; 2],
    phi: [f32; 2],
    k: f32,
    dt: f32,
    damping: f32,
) -> ([f32; 2], [f32; 2]) {
    if k <= 1e-6 {
        return ([0.0; 2], [0.0; 2]);
    }
    let a = k * (k * DEPTH_M).tanh();
    let b = G + TENSION * k * k;
    let w = (a * b).sqrt();
    let (s, c) = (w * dt).sin_cos();
    let fade = (-(damping + VISCOSITY * k * k) * dt).exp();
    let h2 = [
        (h[0] * c + phi[0] * a / w * s) * fade,
        (h[1] * c + phi[1] * a / w * s) * fade,
    ];
    let phi2 = [
        (phi[0] * c - h[0] * b / w * s) * fade,
        (phi[1] * c - h[1] * b / w * s) * fade,
    ];
    (h2, phi2)
}

/// In-place radix-2 FFT of `line` (length a power of two); `dir` −1 forward, +1 inverse
/// (unnormalized).
fn fft(line: &mut [[f32; 2]], dir: f32) {
    let n = line.len();
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            line.swap(i, j);
        }
    }
    let mut half = 1;
    while half < n {
        let step = dir * std::f32::consts::PI / half as f32;
        for start in (0..n).step_by(half * 2) {
            for pos in 0..half {
                let (s, c) = (step * pos as f32).sin_cos();
                let b = line[start + pos + half];
                let bw = [b[0] * c - b[1] * s, b[0] * s + b[1] * c];
                let a = line[start + pos];
                line[start + pos] = [a[0] + bw[0], a[1] + bw[1]];
                line[start + pos + half] = [a[0] - bw[0], a[1] - bw[1]];
            }
        }
        half *= 2;
    }
}

/// 2D FFT of an `n`×`n` row-major grid.
fn fft2(grid: &mut [[f32; 2]], n: usize, dir: f32, column: &mut Vec<[f32; 2]>) {
    for row in grid.chunks_mut(n) {
        fft(row, dir);
    }
    column.resize(n, [0.0; 2]);
    for i in 0..n {
        for j in 0..n {
            column[j] = grid[j * n + i];
        }
        fft(column, dir);
        for j in 0..n {
            grid[j * n + i] = column[j];
        }
    }
}

/// The surface stepped on the CPU, the same way `surface.wgsl` steps it on the GPU.
#[derive(Debug, Clone)]
pub struct SurfaceSim {
    pub n: usize,
    pub cell: f32,
    pub origin: DVec2,
    /// (h, φ) per cell.
    pub state: Vec<[f32; 2]>,
    /// Pushes this step, rasterized.
    push: Vec<f32>,
    work: Vec<[f32; 2]>,
    spectrum: Vec<[f32; 2]>,
    column: Vec<[f32; 2]>,
    /// Seconds since the last push.
    pub quiet_s: f32,
}

impl SurfaceSim {
    pub fn new(n: usize, cell: f32, origin: DVec2) -> Self {
        Self {
            n,
            cell,
            origin,
            state: vec![[0.0; 2]; n * n],
            push: vec![0.0; n * n],
            work: vec![[0.0; 2]; n * n],
            spectrum: vec![[0.0; 2]; n * n],
            column: Vec::new(),
            quiet_s: f32::INFINITY,
        }
    }

    /// Follow the grid's corner to `origin` (whole cells; anything else starts over).
    pub fn follow(&mut self, origin: DVec2) {
        let d = (origin - self.origin) / self.cell as f64;
        let s = d.round();
        if (d - s).length() > 1e-3 || s.x.abs() >= self.n as f64 || s.y.abs() >= self.n as f64 {
            self.state.fill([0.0; 2]);
        } else if s != DVec2::ZERO {
            let (sx, sz, n) = (s.x as i64, s.y as i64, self.n as i64);
            let old = self.state.clone();
            for j in 0..n {
                for i in 0..n {
                    let (oi, oj) = (i + sx, j + sz);
                    self.state[(j * n + i) as usize] = if oi >= 0 && oi < n && oj >= 0 && oj < n {
                        old[(oj * n + oi) as usize]
                    } else {
                        [0.0; 2]
                    };
                }
            }
        }
        self.origin = origin;
    }

    /// Advance by `dt` seconds over `field` (its wet cells, currents and streams) with the
    /// pushes `stamps`, the waves dying out at `damping` per second.
    pub fn step(&mut self, field: &RippleField, stamps: &[Disturbance], dt: f32, damping: f32) {
        let n = self.n;
        let dt = dt.clamp(0.0, MAX_DT);
        self.push.fill(0.0);
        let mut pushed = false;
        for d in stamps {
            pushed |= rasterize(&mut self.push, n, self.cell, self.origin, d).is_some();
        }
        if pushed {
            self.quiet_s = 0.0;
        } else {
            self.quiet_s += dt;
        }
        if self.quiet_s > RING_S {
            return;
        }
        // Carried along the currents, pushed, shut out of dry land, soaked up at the edge.
        let at = |state: &[[f32; 2]], x: f32, z: f32| {
            let (bx, bz) = (x.floor(), z.floor());
            let (fx, fz) = (x - bx, z - bz);
            let mut s = [0.0f32; 2];
            for (dx, dz, w) in [
                (0, 0, (1.0 - fx) * (1.0 - fz)),
                (1, 0, fx * (1.0 - fz)),
                (0, 1, (1.0 - fx) * fz),
                (1, 1, fx * fz),
            ] {
                let (ci, cj) = (bx as i64 + dx, bz as i64 + dz);
                if ci < 0 || cj < 0 || ci >= n as i64 || cj >= n as i64 {
                    continue;
                }
                let v = state[cj as usize * n + ci as usize];
                s[0] += v[0] * w;
                s[1] += v[1] * w;
            }
            s
        };
        for j in 0..n {
            for i in 0..n {
                let k = j * n + i;
                if field.cell_wet(k) <= 0.0 {
                    self.work[k] = [0.0; 2];
                    continue;
                }
                let v = field.cell_current(i, j) + field.cell_drift(k);
                let back = (Vec2::new(i as f32, j as f32) - v * (dt / self.cell)).to_array();
                let mut s = at(&self.state, back[0], back[1]);
                s[0] += self.push[k];
                let keep = sponge(n, i, j, dt);
                self.work[k] = [s[0] * keep, s[1] * keep];
            }
        }
        fft2(&mut self.work, n, -1.0, &mut self.column);
        let dk = std::f32::consts::TAU / (n as f32 * self.cell);
        for j in 0..n {
            for i in 0..n {
                let z = self.work[j * n + i];
                let m = self.work[((n - j) % n) * n + (n - i) % n];
                let zm = [m[0], -m[1]];
                let h = [0.5 * (z[0] + zm[0]), 0.5 * (z[1] + zm[1])];
                let d = [0.5 * (z[0] - zm[0]), 0.5 * (z[1] - zm[1])];
                let phi = [d[1], -d[0]];
                let k = Vec2::new(wave_index(i, n), wave_index(j, n)).length() * dk;
                let (h2, phi2) = evolve_mode(h, phi, k, dt, damping);
                self.spectrum[j * n + i] = [h2[0] - phi2[1], h2[1] + phi2[0]];
            }
        }
        fft2(&mut self.spectrum, n, 1.0, &mut self.column);
        let norm = 1.0 / (n * n) as f32;
        for (k, (s, z)) in self.state.iter_mut().zip(&self.spectrum).enumerate() {
            *s = if field.cell_wet(k) > 0.0 {
                [z[0] * norm, z[1] * norm]
            } else {
                [0.0; 2]
            };
        }
    }

    /// The state to hand out.
    pub fn heights(&self) -> SurfaceHeights {
        SurfaceHeights {
            origin: self.origin,
            n: self.n,
            cell: self.cell,
            h: self.state.iter().map(|s| s[0]).collect(),
            phi: self.state.iter().map(|s| s[1]).collect(),
        }
    }
}

/// Steps without a GPU beat before the CPU takes the surface over.
const IDLE_GPU_STEPS: u32 = 4;

/// The surface on the CPU, when `WaterSettings::cpu_surface` asks for it and no GPU runs it;
/// and how many steps the GPU has gone without stepping it.
#[derive(Resource, Default)]
pub struct CpuSurface(pub Option<SurfaceSim>, u32);

/// Step the surface on the CPU (see [`CpuSurface`]), after the ripple grid is put back.
pub fn cpu_surface_system(
    time: Res<Time>,
    settings: Res<WaterSettings>,
    shared: Res<SharedWater>,
    mut sim: ResMut<CpuSurface>,
) {
    let state = shared.0.read();
    let r = &state.ripples;
    let wanted = settings.cpu_surface && r.n == N && r.has_water && !cpu_ripples_forced();
    if wanted && r.surface.runner() == Runner::Gpu {
        sim.1 = if r.surface.take_beat() { 0 } else { sim.1 + 1 };
        if sim.1 > IDLE_GPU_STEPS {
            r.surface.take_from_idle_gpu();
        }
    }
    if !wanted || !r.surface.claim(Runner::Cpu) {
        if !wanted {
            r.surface.release(Runner::Cpu);
        }
        sim.0 = None;
        return;
    }
    let link = r.surface.clone();
    let s = sim
        .0
        .get_or_insert_with(|| SurfaceSim::new(N, r.cell, r.origin));
    if s.cell != r.cell {
        *s = SurfaceSim::new(N, r.cell, r.origin);
    }
    s.follow(r.origin);
    let stamps = link.take_stamps();
    s.step(r, &stamps, time.delta_secs(), settings.ripple_damping);
    drop(state);
    link.publish(s.heights());
}

/// `ENGINE_WATER_CPU_RIPPLES` set: neither the GPU nor the CPU runs the dispersive surface,
/// the ripple grid's own waves are drawn and floated on (to compare against).
pub fn cpu_ripples_forced() -> bool {
    static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| std::env::var_os("ENGINE_WATER_CPU_RIPPLES").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushes_are_kept_only_while_someone_runs_the_surface() {
        let link = SurfaceLink::default();
        let d = Disturbance {
            depth: 0.05,
            radius: 0.5,
            ..Default::default()
        };
        link.record(&d);
        assert!(link.take_stamps().is_empty());
        assert!(link.claim(Runner::Cpu));
        link.record(&d);
        link.record(&Disturbance { depth: 0.0, ..d });
        assert_eq!(link.take_stamps(), vec![d]);
        assert!(link.take_stamps().is_empty());
        // The GPU takes over; the CPU can't take it back.
        assert!(link.claim(Runner::Gpu));
        assert!(!link.claim(Runner::Cpu));
        link.release(Runner::Cpu);
        assert_eq!(link.runner(), Runner::Gpu);
    }

    #[test]
    fn heights_sample_bilinearly_and_vanish_when_released() {
        let link = SurfaceLink::default();
        link.claim(Runner::Gpu);
        let n = 8;
        // A ramp along x: h = 0.01 · i.
        let h = (0..n * n).map(|k| 0.01 * (k % n) as f32).collect();
        link.publish(SurfaceHeights {
            origin: DVec2::ZERO,
            n,
            cell: 0.5,
            h,
            phi: Vec::new(),
        });
        let s = link.heights().expect("published");
        // Cell 2's center is at x = 1.25; halfway to cell 3.
        let (height, g) = s.sample(DVec2::new(1.5, 2.0)).expect("inside");
        assert!((height - 0.025).abs() < 1e-5, "{height}");
        assert!((g.x - 0.02).abs() < 1e-5 && g.y.abs() < 1e-5, "{g:?}");
        assert!(s.sample(DVec2::new(-1.0, 2.0)).is_none());
        link.release(Runner::Gpu);
        assert!(link.heights().is_none());
    }

    #[test]
    fn the_transform_round_trips() {
        let n = 16;
        let mut grid: Vec<[f32; 2]> = (0..n * n)
            .map(|k| [(k as f32 * 0.37).sin(), (k as f32 * 0.11).cos()])
            .collect();
        let before = grid.clone();
        let mut column = Vec::new();
        fft2(&mut grid, n, -1.0, &mut column);
        fft2(&mut grid, n, 1.0, &mut column);
        for (a, b) in grid.iter().zip(&before) {
            assert!((a[0] / (n * n) as f32 - b[0]).abs() < 1e-4);
            assert!((a[1] / (n * n) as f32 - b[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn each_mode_keeps_its_energy_and_turns_at_the_dispersion_rate() {
        // Deep water at k = 2 rad/m: ω = √(g·k·tanh(kD)).
        let k = 2.0f32;
        let w = (G * k * (k * DEPTH_M).tanh()).sqrt();
        let period = std::f32::consts::TAU / w;
        let (mut h, mut phi) = ([1.0, 0.0], [0.0, 0.0]);
        let steps = 200;
        for _ in 0..steps {
            (h, phi) = evolve_mode(h, phi, k, period / steps as f32, 0.0);
        }
        // Back where it started after one period (viscosity takes a little).
        assert!(
            (h[0] - (-VISCOSITY * k * k * period).exp()).abs() < 1e-3,
            "{h:?}"
        );
        assert!(phi[0].abs() < 1e-2, "{phi:?}");
    }

    #[test]
    fn pushes_rasterize_like_the_grid_applies_them() {
        let n = 32;
        let mut push = vec![0.0; n * n];
        let d = Disturbance {
            pos: DVec2::new(4.125, 4.125),
            radius: 0.5,
            depth: 0.1,
            ..Default::default()
        };
        let rows = rasterize(&mut push, n, 0.25, DVec2::ZERO, &d).expect("on the grid");
        assert!(rows.0 <= 16 && rows.1 >= 16);
        // Cell (16, 16)'s center is the push's center: the dip less its ring's share.
        assert!((push[16 * n + 16] + 0.075).abs() < 1e-6);
        // No water comes or goes.
        let net: f32 = push.iter().sum();
        assert!(
            net.abs() < 1e-3 * push.iter().map(|p| p.abs()).sum::<f32>(),
            "{net}"
        );
        let mut field = RippleField::new(n, 0.25);
        field.origin = DVec2::ZERO;
        field.rebuild_speeds(1.3, |_| Some(1.0));
        field.disturb(d);
        field.step(1e-4, 0.0, 6.0);
        // The grid's own dip after a tiny step is as deep (it lets the water go).
        assert!((field.h[16 * n + 16] - push[16 * n + 16] / 0.75).abs() < 2e-3);
        // Off the grid: nothing.
        let far = Disturbance {
            pos: DVec2::new(-50.0, 0.0),
            ..d
        };
        assert!(rasterize(&mut push, n, 0.25, DVec2::ZERO, &far).is_none());
    }

    /// A still pond the size of the surface's grid.
    fn pond() -> RippleField {
        let mut f = RippleField::new(N, 0.25);
        f.rebuild_speeds(1.3, |_| Some(3.0));
        f
    }

    #[test]
    fn a_dropped_stone_sends_out_a_spreading_train_of_rings() {
        let field = pond();
        let mut sim = SurfaceSim::new(N, field.cell, field.origin);
        let stone = Disturbance {
            pos: DVec2::ZERO,
            radius: 0.6,
            depth: 0.05,
            ..Default::default()
        };
        sim.step(&field, &[stone], 1.0 / 30.0, 0.0);
        for _ in 0..90 {
            sim.step(&field, &[], 1.0 / 30.0, 0.0);
        }
        // Along +x from the middle (cell 128): the crests, and the gaps between them.
        let h = sim.heights().h;
        let row: Vec<f32> = (128..N - 24).map(|i| h[128 * N + i]).collect();
        let peak = row.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let crests: Vec<usize> = (1..row.len() - 1)
            .filter(|&i| row[i] > row[i - 1] && row[i] >= row[i + 1] && row[i] > 0.03 * peak)
            .collect();
        assert!(crests.len() >= 3, "a train, not one ring: {crests:?}");
        // Long waves run ahead: the gaps between crests grow outward.
        let gaps: Vec<usize> = crests.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.last() > gaps.first(), "{gaps:?}");
    }

    #[test]
    fn a_moving_hull_draws_a_kelvin_wedge() {
        let field = pond();
        let mut sim = SurfaceSim::new(N, field.cell, field.origin);
        // 2.5 m/s along +x for 10 s, from x = −18 m.
        let dt = 1.0 / 30.0;
        let speed = 2.5;
        let steps = 300;
        for s in 0..steps {
            let x = -18.0 + speed * dt as f64 * s as f64;
            let hull = Disturbance {
                pos: DVec2::new(x, 0.0),
                radius: 0.6,
                depth: 0.01,
                ..Default::default()
            };
            sim.step(&field, &[hull], dt, 0.0);
        }
        let h = sim.heights().h;
        let bow = -18.0 + speed * dt as f64 * steps as f64;
        // The cusp waves along the wedge's edge are the wake's highest: where each cut across
        // it behind the hull peaks (clear of the hull's own trough on the track).
        let mut angles = Vec::new();
        for behind in [6.0, 9.0, 12.0] {
            let x = bow - behind;
            let i = ((x - field.origin.x) / 0.25) as usize;
            let across = |dz: usize| {
                (i - 2..=i + 2)
                    .map(|ii| h[(128 + dz) * N + ii].abs())
                    .fold(0.0f32, f32::max)
            };
            let cusp = (6..N / 2 - 24)
                .max_by(|a, b| across(*a).total_cmp(&across(*b)))
                .unwrap() as f64
                * 0.25;
            angles.push((cusp / behind).atan().to_degrees());
        }
        // Kelvin's wedge: 19.5° whatever the speed (deep water); the cusps of a short,
        // fast hull sit a little inside it (14-17° here; Rabaud & Moisy 2013), where a
        // non-dispersive grid would draw a Mach cone of asin(1.3 / 2.5) = 31°.
        for a in &angles {
            assert!(*a > 11.0 && *a < 23.0, "wedge half-angles {angles:?}");
        }
    }
}
