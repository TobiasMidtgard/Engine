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
//! It runs on two levels, both [`N`]² cells: the fine one over the ripple grid (0.25 m cells,
//! 64 m) and a coarse one four times larger every way (1 m cells, 256 m), so a boat's long
//! transverse waves, 20 m and more, have room to spread into its whole wake. Each wavelength
//! belongs to one level, waves shorter than [`BAND_CELLS`] fine cells to the fine one and
//! longer ones to the coarse one, with a crossfade an octave wide: every push goes into
//! both, each keeping only its own band, and since the evolution never mixes wavelengths a
//! wave never counts twice and never has to pass from one level to the other (camera-centred
//! levels summed for drawing, as Crest's dynamic waves are; the band split as Jeschke &
//! Wojtan split theirs). Short waves die out at the fine level's edge, where the eye no
//! longer resolves them.
//!
//! The renderer runs it on the GPU (`render/surface.rs`, `surface.wgsl`) and reads its heights
//! back for floating things a frame or two later. Without a GPU it can run here instead
//! ([`SurfaceSim`], `WaterSettings::cpu_surface`): headless servers that want the same waves,
//! galleries that simulate without drawing, and the tests, for which it is the reference.
//! The two step the same way, and the GPU picks up where this one left off.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use engine_core::math::{DVec2, Vec2, smoothstep};
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
/// Damps the shortest waves the grid holds (two cells), m²/s: they'd ring on as noise. (Fades
/// them as ν·k²: half a meter long in about 2 s, the meter-long waves of a wake in about 8 s.
/// Water's own 10⁻⁶ would keep everything; 0.008 took a wake's short waves in 3 s.)
pub const VISCOSITY: f32 = 0.003;
/// Surface tension over density, m³/s².
pub const TENSION: f32 = 7.4e-5;
/// Longest step the surface takes at once, seconds (the evolution is exact for any step; the
/// currents' carry isn't).
pub const MAX_DT: f32 = 0.05;
/// How long the surface keeps running after the last push, seconds: by then the waves have
/// died out or left the grid.
pub const RING_S: f32 = 25.0;
/// Cells at the fine level's edge where the waves are soaked up (the transform wraps around).
pub const SPONGE: f32 = 20.0;
/// The coarse level's cells, in fine cells.
pub const COARSE: usize = 4;
/// Cells at the coarse level's edge where its waves are soaked up: a boat's transverse waves
/// are longer than the fine level's whole sponge and need more room to die without echoing.
pub const COARSE_SPONGE: f32 = 40.0;
/// Waves this many fine cells long (4 m) are shared half and half between the levels.
pub const BAND_CELLS: f32 = 16.0;
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

/// Which wavelengths a level holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Band {
    /// All of them: the fine level alone.
    All,
    /// The short waves, below the crossover wavenumber (1/m).
    Fine(f32),
    /// The long waves.
    Coarse(f32),
}

impl Band {
    /// The crossover wavenumber over a fine level of `fine_cell` m cells.
    pub fn crossover(fine_cell: f32) -> f32 {
        std::f32::consts::TAU / (BAND_CELLS * fine_cell)
    }

    /// The share of a wave of wavenumber `k` (1/m) this level holds; the two levels' shares
    /// sum to one (a cos² fade over the octave about the crossover).
    pub fn share(&self, k: f32) -> f32 {
        let (k0, coarse) = match *self {
            Band::All => return 1.0,
            Band::Fine(k0) => (k0, false),
            Band::Coarse(k0) => (k0, true),
        };
        let s = ((k.max(1e-6) / k0).log2() + 0.5).clamp(0.0, 1.0);
        let long = (std::f32::consts::FRAC_PI_2 * s).cos().powi(2);
        if coarse { long } else { 1.0 - long }
    }

    /// (crossover, 0 all / 1 fine / 2 coarse) for the shader.
    pub fn gpu(&self) -> [f32; 2] {
        match *self {
            Band::All => [0.0, 0.0],
            Band::Fine(k) => [k, 1.0],
            Band::Coarse(k) => [k, 2.0],
        }
    }
}

/// What a level's waves run over: where the water is, and the currents carrying them.
pub trait Medium {
    /// How much of a step cell `k` (row-major) keeps: 0 on dry land.
    fn wet(&self, k: usize) -> f32;
    /// The water carrying the waves at cell (i, j) = `k`, world x/z, m/s.
    fn carry(&self, i: usize, j: usize, k: usize) -> Vec2;
    /// How deep the water the waves feel is, meters.
    fn depth(&self) -> f32 {
        DEPTH_M
    }
    /// How fast the waves die out over cell `k`, 1/s ([`shore_absorb`]).
    fn absorb(&self, _k: usize) -> f32 {
        0.0
    }
}

/// Water shallower than this (m) soaks up the waves running into it.
pub const SHORE_ABSORB_DEPTH_M: f32 = 0.8;
/// How fast the shallowest water soaks them up, 1/s.
pub const SHORE_ABSORB_MAX: f32 = 3.0;

/// How fast waves die out over water `depth` m deep, 1/s. A wave running up a shelving beach
/// steepens, breaks once it is about as high as the water is deep (McCowan's H/h ≈ 0.78) and
/// spends itself in the surf and the swash: sand gives back about a tenth of a wave or less
/// (reflection ~0.1·ξ², Battjes 1974), a wall nearly all of it. The linear surface can't
/// break, so the last stretch of shallows takes the energy out instead, over many cells so
/// the soaking edge doesn't reflect in its turn (an absorbing layer, as the grid's sponge).
/// Steep banks go from deep to dry within a cell or two and still bounce most of it back.
pub fn shore_absorb(depth: f32) -> f32 {
    let t = ((depth - 0.05) / (SHORE_ABSORB_DEPTH_M - 0.05)).clamp(0.0, 1.0);
    let open = t * t * (3.0 - 2.0 * t);
    SHORE_ABSORB_MAX * (1.0 - open) * (1.0 - open)
}

impl Medium for RippleField {
    fn wet(&self, k: usize) -> f32 {
        self.cell_wet(k)
    }

    /// The stream under the waves only. The grid's stirred currents (a hull's wash, a
    /// swimmer's eddies) are a few meters across and gone in seconds: the waves a hull makes
    /// run out of its wash at once, and tracing the surface back through it every frame
    /// would only blur its wake away (the grid's foam swirls with them instead).
    fn carry(&self, _: usize, _: usize, k: usize) -> Vec2 {
        self.cell_drift(k)
    }

    fn absorb(&self, k: usize) -> f32 {
        self.absorb.get(k).copied().unwrap_or(0.0)
    }
}

/// The water under the coarse level: which of its cells are lake (read from the bodies like
/// the ripple grid's), with no currents (the grid's swirls are smaller than its cells).
#[derive(Debug, Clone, PartialEq)]
pub struct CoarseBed {
    /// World x/z of the corner of cell (0, 0).
    pub origin: DVec2,
    /// Cell size, meters.
    pub cell: f32,
    /// Per cell (row-major, [`N`]²): 1 over water, 0 on land.
    pub wet: Vec<f32>,
    /// The water's mean depth over the level, meters: long waves feel the bottom (a boat
    /// going faster than √(g·depth) leaves no transverse waves at all).
    pub depth: f32,
    /// Per cell: how fast the waves die out there, 1/s ([`shore_absorb`] of its depth).
    pub absorb: Vec<f32>,
    /// Counts up whenever `wet` changes.
    pub version: u64,
}

impl Medium for CoarseBed {
    fn wet(&self, k: usize) -> f32 {
        self.wet[k]
    }

    fn carry(&self, _: usize, _: usize, _: usize) -> Vec2 {
        Vec2::ZERO
    }

    fn depth(&self) -> f32 {
        self.depth
    }

    fn absorb(&self, k: usize) -> f32 {
        self.absorb.get(k).copied().unwrap_or(0.0)
    }
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
    coarse: RwLock<Option<Arc<SurfaceHeights>>>,
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
        *self.coarse.write() = None;
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
        if !self.running() || (d.depth == 0.0 && d.press == 0.0) {
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

    /// Hand out the coarse level's latest state (`None`: it stopped).
    pub fn publish_coarse(&self, heights: Option<SurfaceHeights>) {
        *self.coarse.write() = heights.map(Arc::new);
    }

    /// The coarse level's latest state, while it runs.
    pub fn coarse(&self) -> Option<Arc<SurfaceHeights>> {
        if !self.running() {
            return None;
        }
        self.coarse.read().clone()
    }

    /// (height, (dh/dx, dh/dz)) of both levels together at world x/z, while the surface
    /// runs and the fine level covers it.
    pub fn sample(&self, p: DVec2) -> Option<(f32, Vec2)> {
        let (h, g) = self.heights()?.sample(p)?;
        Some(match self.coarse().and_then(|c| c.sample(p)) {
            Some((hc, gc)) => (h + hc, g + gc),
            None => (h, g),
        })
    }
}

/// Rasterize a push into `push` (height pushed, pressure impulse per cell), for an `n`-cell
/// grid of `cell` m with its corner at `origin`; returns the rows touched. The dip is the one
/// [`RippleField`] applies (a Gaussian), less a ring twice as wide that holds the water it
/// pushed aside: no water comes or goes. (Pushes come every step; were each to take water
/// away, the lake would sink round a hull's course in basin-wide swells, which the
/// dispersive surface carries far faster than the grid.) A push with a pressure
/// ([`Disturbance::press`]) is drawn as that alone, a plain Gaussian. A push narrower than a
/// coarse level's cells is drawn a cell wide with the same volume as on the finest level,
/// whose cells are `fine_cell` m, so the long waves it makes are the same.
pub fn rasterize(
    push: &mut [[f32; 2]],
    n: usize,
    cell: f32,
    origin: DVec2,
    fine_cell: f32,
    d: &Disturbance,
) -> Option<(usize, usize)> {
    if d.press != 0.0 && d.keel != Vec2::ZERO {
        return rasterize_hull(push, n, cell, origin, d);
    }
    let base = d.radius.max(fine_cell * 0.75);
    let r = base.max(cell * 0.75);
    let keep = (base / r).powi(2);
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
            let main = (-x * x * 2.0).exp();
            let cell_push = &mut push[j as usize * n + i as usize];
            if d.press != 0.0 {
                cell_push[1] += d.press * keep * main;
            } else {
                cell_push[0] -= d.depth * keep * (main - 0.25 * (-x * x * 0.5).exp());
            }
        }
    }
    Some((j0 as usize, j1 as usize))
}

/// A long hull's pressure (`d.keel`, `d.radius` half its beam): its waterline, round aft of
/// midships and drawn to a point at the bow, full `press` inside, its edge smoothed over about
/// a cell (sharper than that the grid would alias it).
fn rasterize_hull(
    push: &mut [[f32; 2]],
    n: usize,
    cell: f32,
    origin: DVec2,
    d: &Disturbance,
) -> Option<(usize, usize)> {
    let len = d.keel.length();
    let along = d.keel / len;
    let across = Vec2::new(-along.y, along.x);
    let beam = d.radius.max(1e-3);
    let soft = 0.6 * cell;
    let reach = ((len.max(beam) + 2.0 * soft) / cell).ceil() as i64 + 1;
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
            let q = (p - d.pos).as_vec2();
            let u = q.dot(along) / len;
            let v = q.dot(across).abs();
            let w = if u.abs() >= 1.0 {
                0.0
            } else if u > 0.0 {
                beam * (1.0 - u * u)
            } else {
                beam * (1.0 - u * u).sqrt()
            };
            // (Roughly how far inside the waterline, meters.)
            let inside = (len * (1.0 - u.abs())).min(w - v);
            let cover = smoothstep(-soft, soft, inside);
            if cover > 0.0 {
                push[j as usize * n + i as usize][1] += d.press * cover;
            }
        }
    }
    Some((j0 as usize, j1 as usize))
}

/// How much of a step's waves a cell keeps within `width` cells of the grid's edge (the
/// sponge), for a step of `dt` seconds.
pub fn sponge(n: usize, width: f32, i: usize, j: usize, dt: f32) -> f32 {
    let edge = i.min(j).min(n - 1 - i).min(n - 1 - j) as f32;
    let t = (1.0 - edge / width).clamp(0.0, 1.0);
    (-6.0 * t * t * dt).exp()
}

/// Catmull-Rom weights of the four samples around a point `t` (0..1) past the second.
pub fn catmull_rom(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// The signed wavenumber index of bin `i` of `n` (0..n/2−1, then −n/2..−1).
fn wave_index(i: usize, n: usize) -> f32 {
    if i >= n / 2 {
        i as f32 - n as f32
    } else {
        i as f32
    }
}

/// Turn one mode: the spectra of height `h` and potential `phi` at wavenumber `k` (1/m) over
/// water `depth` m deep after `dt` seconds, damped by `damping` (1/s) and the viscosity.
pub fn evolve_mode(
    h: [f32; 2],
    phi: [f32; 2],
    k: f32,
    depth: f32,
    dt: f32,
    damping: f32,
) -> ([f32; 2], [f32; 2]) {
    if k <= 1e-6 {
        return ([0.0; 2], [0.0; 2]);
    }
    let a = k * (k * depth).tanh();
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

/// One level of the surface stepped on the CPU, the same way `surface.wgsl` steps it on the
/// GPU.
#[derive(Debug, Clone)]
pub struct SurfaceSim {
    pub n: usize,
    pub cell: f32,
    pub origin: DVec2,
    /// The wavelengths it holds.
    pub band: Band,
    /// Cells at its edge where the waves are soaked up.
    pub sponge: f32,
    /// (h, φ) per cell.
    pub state: Vec<[f32; 2]>,
    /// Pushes this step, rasterized: (height, pressure impulse) per cell.
    push: Vec<[f32; 2]>,
    work: Vec<[f32; 2]>,
    pushed: Vec<[f32; 2]>,
    spectrum: Vec<[f32; 2]>,
    column: Vec<[f32; 2]>,
    /// Seconds since the last push.
    pub quiet_s: f32,
}

/// Split the transform of two real fields packed as `a + i·b` at bin `k` (its mirror at `m`)
/// into theirs.
fn unpack(z: [f32; 2], m: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    let zm = [m[0], -m[1]];
    let a = [0.5 * (z[0] + zm[0]), 0.5 * (z[1] + zm[1])];
    let d = [0.5 * (z[0] - zm[0]), 0.5 * (z[1] - zm[1])];
    (a, [d[1], -d[0]])
}

impl SurfaceSim {
    /// The fine level alone (all wavelengths, the fine sponge).
    pub fn new(n: usize, cell: f32, origin: DVec2) -> Self {
        Self::level(n, cell, origin, Band::All, SPONGE)
    }

    pub fn level(n: usize, cell: f32, origin: DVec2, band: Band, sponge: f32) -> Self {
        Self {
            n,
            cell,
            origin,
            band,
            sponge,
            state: vec![[0.0; 2]; n * n],
            push: vec![[0.0; 2]; n * n],
            work: vec![[0.0; 2]; n * n],
            pushed: vec![[0.0; 2]; n * n],
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

    /// Advance by `dt` seconds over `medium` (its wet cells and currents) with the pushes
    /// `stamps` (drawn as on a finest level of `fine_cell` m cells), the waves dying out at
    /// `damping` per second.
    pub fn step(
        &mut self,
        medium: &impl Medium,
        stamps: &[Disturbance],
        fine_cell: f32,
        dt: f32,
        damping: f32,
    ) {
        let n = self.n;
        let dt = dt.clamp(0.0, MAX_DT);
        self.push.fill([0.0; 2]);
        let mut pushed = false;
        for d in stamps {
            pushed |= rasterize(&mut self.push, n, self.cell, self.origin, fine_cell, d).is_some();
        }
        if pushed {
            self.quiet_s = 0.0;
        } else {
            self.quiet_s += dt;
        }
        if self.quiet_s > RING_S {
            return;
        }
        // Carried along the currents, shut out of dry land, soaked up at the edge. Traced back
        // through a cubic (Catmull-Rom): a bilinear lookup each step would blur away the
        // short waves wherever the water moves (a boat's own wash drowned its wake).
        let at = |state: &[[f32; 2]], x: f32, z: f32| {
            let (bx, bz) = (x.floor(), z.floor());
            let (wx, wz) = (catmull_rom(x - bx), catmull_rom(z - bz));
            let mut s = [0.0f32; 2];
            for (dz, wz) in wz.iter().enumerate() {
                let cj = bz as i64 + dz as i64 - 1;
                if cj < 0 || cj >= n as i64 {
                    continue;
                }
                for (dx, wx) in wx.iter().enumerate() {
                    let ci = bx as i64 + dx as i64 - 1;
                    if ci < 0 || ci >= n as i64 {
                        continue;
                    }
                    let v = state[cj as usize * n + ci as usize];
                    s[0] += v[0] * wx * wz;
                    s[1] += v[1] * wx * wz;
                }
            }
            s
        };
        for j in 0..n {
            for i in 0..n {
                let k = j * n + i;
                if medium.wet(k) <= 0.0 {
                    self.work[k] = [0.0; 2];
                    continue;
                }
                let v = medium.carry(i, j, k);
                let s = if v == Vec2::ZERO {
                    self.state[k]
                } else {
                    let back = Vec2::new(i as f32, j as f32) - v * (dt / self.cell);
                    at(&self.state, back.x, back.y)
                };
                let keep = sponge(n, self.sponge, i, j, dt) * (-medium.absorb(k) * dt).exp();
                self.work[k] = [s[0] * keep, s[1] * keep];
            }
        }
        fft2(&mut self.work, n, -1.0, &mut self.column);
        // The pushes go in through their own transform, each level taking its band of them:
        // height pushed, and the potential a pressure sets going (dφ/dt = −g·h − p/ρ).
        if pushed {
            for (dst, p) in self.pushed.iter_mut().zip(&self.push) {
                *dst = [p[0], -G * p[1]];
            }
            fft2(&mut self.pushed, n, -1.0, &mut self.column);
        }
        let dk = std::f32::consts::TAU / (n as f32 * self.cell);
        let depth = medium.depth();
        for j in 0..n {
            for i in 0..n {
                let mirror = ((n - j) % n) * n + (n - i) % n;
                let (mut h, mut phi) = unpack(self.work[j * n + i], self.work[mirror]);
                let k = Vec2::new(wave_index(i, n), wave_index(j, n)).length() * dk;
                if pushed {
                    let (ph, pphi) = unpack(self.pushed[j * n + i], self.pushed[mirror]);
                    let w = self.band.share(k);
                    for c in 0..2 {
                        h[c] += w * ph[c];
                        phi[c] += w * pphi[c];
                    }
                }
                let (h2, phi2) = evolve_mode(h, phi, k, depth, dt, damping);
                self.spectrum[j * n + i] = [h2[0] - phi2[1], h2[1] + phi2[0]];
            }
        }
        fft2(&mut self.spectrum, n, 1.0, &mut self.column);
        let norm = 1.0 / (n * n) as f32;
        for (k, (s, z)) in self.state.iter_mut().zip(&self.spectrum).enumerate() {
            *s = if medium.wet(k) > 0.0 {
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

/// The surface on the CPU, when `WaterSettings::cpu_surface` asks for it and no GPU runs it
/// (its fine and coarse levels); and how many steps the GPU has gone without stepping it.
#[derive(Resource, Default)]
pub struct CpuSurface {
    pub fine: Option<SurfaceSim>,
    pub coarse: Option<SurfaceSim>,
    idle: u32,
}

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
        sim.idle = if r.surface.take_beat() {
            0
        } else {
            sim.idle + 1
        };
        if sim.idle > IDLE_GPU_STEPS {
            r.surface.take_from_idle_gpu();
        }
    }
    if !wanted || !r.surface.claim(Runner::Cpu) {
        if !wanted {
            r.surface.release(Runner::Cpu);
        }
        sim.fine = None;
        sim.coarse = None;
        return;
    }
    let link = r.surface.clone();
    let stamps = link.take_stamps();
    let (dt, damping) = (time.delta_secs(), settings.surface_damping);
    let k0 = Band::crossover(r.cell);
    let band = if r.coarse.is_some() {
        Band::Fine(k0)
    } else {
        Band::All
    };
    let fine = sim
        .fine
        .get_or_insert_with(|| SurfaceSim::level(N, r.cell, r.origin, band, SPONGE));
    if fine.cell != r.cell || fine.band != band {
        *fine = SurfaceSim::level(N, r.cell, r.origin, band, SPONGE);
    }
    fine.follow(r.origin);
    fine.step(r, &stamps, r.cell, dt, damping);
    let fine_heights = fine.heights();
    let coarse_heights = match &r.coarse {
        Some(bed) => {
            let cell = r.cell * COARSE as f32;
            let coarse = sim.coarse.get_or_insert_with(|| {
                SurfaceSim::level(N, cell, bed.origin, Band::Coarse(k0), COARSE_SPONGE)
            });
            if coarse.cell != cell {
                *coarse = SurfaceSim::level(N, cell, bed.origin, Band::Coarse(k0), COARSE_SPONGE);
            }
            coarse.follow(bed.origin);
            coarse.step(&**bed, &stamps, r.cell, dt, damping);
            Some(coarse.heights())
        }
        None => {
            sim.coarse = None;
            None
        }
    };
    drop(state);
    link.publish(fine_heights);
    link.publish_coarse(coarse_heights);
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
            (h, phi) = evolve_mode(h, phi, k, DEPTH_M, period / steps as f32, 0.0);
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
        let mut push = vec![[0.0f32; 2]; n * n];
        let d = Disturbance {
            pos: DVec2::new(4.125, 4.125),
            radius: 0.5,
            depth: 0.1,
            ..Default::default()
        };
        let rows = rasterize(&mut push, n, 0.25, DVec2::ZERO, 0.25, &d).expect("on the grid");
        assert!(rows.0 <= 16 && rows.1 >= 16);
        // Cell (16, 16)'s center is the push's center: the dip less its ring's share.
        assert!((push[16 * n + 16][0] + 0.075).abs() < 1e-6);
        // No water comes or goes.
        let net: f32 = push.iter().map(|p| p[0]).sum();
        assert!(
            net.abs() < 1e-3 * push.iter().map(|p| p[0].abs()).sum::<f32>(),
            "{net}"
        );
        let mut field = RippleField::new(n, 0.25);
        field.origin = DVec2::ZERO;
        field.rebuild_speeds(1.3, |_| Some(1.0));
        field.disturb(d);
        field.step(1e-4, 0.0, 6.0);
        // The grid's own dip after a tiny step is as deep (it lets the water go).
        assert!((field.h[16 * n + 16] - push[16 * n + 16][0] / 0.75).abs() < 2e-3);
        // Off the grid: nothing.
        let far = Disturbance {
            pos: DVec2::new(-50.0, 0.0),
            ..d
        };
        assert!(rasterize(&mut push, n, 0.25, DVec2::ZERO, 0.25, &far).is_none());
    }

    /// A still pond the size of the surface's grid.
    fn pond() -> RippleField {
        let mut f = RippleField::new(N, 0.25);
        f.rebuild_speeds(1.3, |_| Some(3.0));
        f
    }

    #[test]
    fn a_shelving_beach_soaks_up_the_waves_a_wall_sends_back() {
        // Dry from x = 10 m. The beach shelves from 3 m deep at x = 2 m up to the waterline;
        // the wall drops straight to 3 m.
        let energy = |beach: bool| {
            let mut field = RippleField::new(N, 0.25);
            field.rebuild_speeds(1.3, |p| {
                let d = if beach { (10.0 - p.x) * 3.0 / 8.0 } else { 3.0 };
                (p.x < 10.0).then_some(d.min(3.0) as f32)
            });
            let mut sim = SurfaceSim::new(N, field.cell, field.origin);
            let stone = Disturbance {
                pos: DVec2::new(-2.0, 0.0),
                radius: 0.6,
                depth: 0.05,
                ..Default::default()
            };
            sim.step(&field, &[stone], 0.25, 1.0 / 30.0, 0.0);
            for _ in 0..480 {
                sim.step(&field, &[], 0.25, 1.0 / 30.0, 0.0);
            }
            // What is left in front of the shore: the rings running in, and what came back.
            let h = sim.heights().h;
            let mut e = 0.0f64;
            for j in 0..N {
                for i in 0..N {
                    let x = field.cell_center(i, j).x;
                    if x > 0.0 {
                        e += (h[j * N + i] as f64).powi(2);
                    }
                }
            }
            e
        };
        let (beach, wall) = (energy(true), energy(false));
        // About 0.54 of the wall's.
        assert!(beach < wall * 0.7, "beach {beach:.3e}, wall {wall:.3e}");
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
        sim.step(&field, &[stone], 0.25, 1.0 / 30.0, 0.0);
        for _ in 0..90 {
            sim.step(&field, &[], 0.25, 1.0 / 30.0, 0.0);
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
            sim.step(&field, &[hull], 0.25, dt, 0.0);
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

    /// Still water everywhere under a coarse level centered on `field`.
    fn coarse_pond(field: &RippleField) -> CoarseBed {
        let cell = field.cell * COARSE as f32;
        let span = cell as f64 * N as f64;
        CoarseBed {
            origin: field.origin + DVec2::splat((field.size() - span) * 0.5),
            cell,
            wet: vec![1.0; N * N],
            depth: 10.0,
            absorb: vec![0.0; N * N],
            version: 0,
        }
    }

    #[test]
    fn the_two_bands_share_every_wave_once() {
        let k0 = Band::crossover(0.25);
        for k in [0.05, 0.5, k0 / 1.5, k0, k0 * 1.3, 4.0, 12.0] {
            let sum = Band::Fine(k0).share(k) + Band::Coarse(k0).share(k);
            assert!((sum - 1.0).abs() < 1e-5, "{k}: {sum}");
        }
        assert!(Band::Coarse(k0).share(k0 / 2.0) > 0.99);
        assert!(Band::Fine(k0).share(k0 * 2.0) > 0.99);
        assert_eq!(Band::All.share(3.0), 1.0);
    }

    #[test]
    fn the_levels_together_carry_what_one_level_alone_does() {
        // A stone in the middle: over its first seconds, before the waves reach either
        // level's edge, the fine and coarse levels' heights sum to one fine level holding
        // every wave.
        let field = pond();
        let bed = coarse_pond(&field);
        let k0 = Band::crossover(field.cell);
        let mut all = SurfaceSim::new(N, field.cell, field.origin);
        let mut fine = SurfaceSim::level(N, field.cell, field.origin, Band::Fine(k0), SPONGE);
        let mut coarse =
            SurfaceSim::level(N, bed.cell, bed.origin, Band::Coarse(k0), COARSE_SPONGE);
        let stone = Disturbance {
            pos: DVec2::new(0.1, -0.2),
            radius: 1.2,
            depth: 0.05,
            ..Default::default()
        };
        let dt = 1.0 / 30.0;
        for s in 0..60 {
            let stamps: &[Disturbance] = if s == 0 { &[stone] } else { &[] };
            all.step(&field, stamps, field.cell, dt, 0.0);
            fine.step(&field, stamps, field.cell, dt, 0.0);
            coarse.step(&bed, stamps, field.cell, dt, 0.0);
        }
        let (all, fine, coarse) = (all.heights(), fine.heights(), coarse.heights());
        let mut err = 0.0f32;
        let mut size = 0.0f32;
        for x in -40..=40 {
            let p = DVec2::new(x as f64 * 0.25, 0.3);
            let a = all.sample(p).unwrap().0;
            let b = fine.sample(p).unwrap().0 + coarse.sample(p).unwrap().0;
            err += (a - b).powi(2);
            size += a * a;
        }
        assert!(size > 0.0);
        assert!(
            err < 0.05 * size,
            "off by {:.1}%",
            100.0 * (err / size).sqrt()
        );
    }

    #[test]
    fn a_pressure_holds_a_dip_and_moves_no_water() {
        // A hull at rest pressing 10 cm for 12 s settles into a dip about that deep; the
        // water it holds down rises round it, none lost.
        let field = pond();
        let mut sim = SurfaceSim::new(N, field.cell, field.origin);
        let dt = 1.0 / 30.0;
        for _ in 0..360 {
            let hull = Disturbance {
                pos: DVec2::ZERO,
                radius: 2.0,
                press: 0.1 * dt,
                ..Default::default()
            };
            sim.step(&field, &[hull], 0.25, dt, 0.5);
        }
        let h = sim.heights();
        let middle = h.sample(DVec2::ZERO).unwrap().0;
        assert!(middle < -0.07 && middle > -0.13, "{middle}");
        let net: f32 = h.h.iter().sum::<f32>() * 0.25 * 0.25;
        assert!(net.abs() < 0.01, "{net} m³");
    }

    #[test]
    fn a_hull_pressing_its_way_spreads_its_long_waves_on_the_coarse_level() {
        // 5 m/s for 12 s over 10 m of water: the transverse waves behind it are about
        // 2πv²/g = 16 m long, too long for the fine level's grid to hold to the wedge's far
        // end; the coarse one carries them.
        let field = pond();
        let bed = coarse_pond(&field);
        let k0 = Band::crossover(field.cell);
        let mut fine = SurfaceSim::level(N, field.cell, field.origin, Band::Fine(k0), SPONGE);
        let mut coarse =
            SurfaceSim::level(N, bed.cell, bed.origin, Band::Coarse(k0), COARSE_SPONGE);
        let dt = 1.0 / 30.0;
        let speed = 5.0;
        for s in 0..360 {
            let x = -70.0 + speed * dt as f64 * s as f64;
            let hull = Disturbance {
                pos: DVec2::new(x, 0.0),
                radius: 1.0,
                press: 0.15 * dt,
                ..Default::default()
            };
            fine.step(&field, &[hull], field.cell, dt, 0.0);
            coarse.step(&bed, &[hull], field.cell, dt, 0.0);
        }
        let c = coarse.heights();
        // Along the track behind the hull (now at x = −10 m): crests about 16 m apart.
        let track: Vec<f32> = (0..50)
            .map(|k| c.sample(DVec2::new(-14.0 - k as f64, 0.0)).unwrap().0)
            .collect();
        let crests: Vec<usize> = (1..track.len() - 1)
            .filter(|&i| track[i] > track[i - 1] && track[i] >= track[i + 1] && track[i] > 0.0)
            .collect();
        assert!(crests.len() >= 2, "{track:?}");
        let gap = (crests[1] - crests[0]) as f32;
        assert!(gap > 12.0 && gap < 20.0, "crests {crests:?}");
        // Out past the fine level's grid, 30-45 m behind the hull, the wake is still there.
        let peak = |x0: f64, x1: f64| {
            let n = ((x1 - x0) / 0.25) as usize;
            (0..n)
                .flat_map(|k| (0..40).map(move |z| (k, z)))
                .map(|(k, z)| {
                    let p = DVec2::new(x0 + k as f64 * 0.25, z as f64 * 0.5);
                    c.sample(p).map_or(0.0, |v| v.0.abs())
                })
                .fold(0.0f32, f32::max)
        };
        let (near, far) = (peak(-25.0, -12.0), peak(-55.0, -40.0));
        assert!(far > 0.2 * near, "near {near} far {far}");
        assert!(fine.heights().sample(DVec2::new(-50.0, 0.0)).is_none());
    }

    #[test]
    fn a_hull_presses_its_waterline_pointed_at_the_bow() {
        let n = 64;
        let mut push = vec![[0.0f32; 2]; n * n];
        let (len, beam) = (2.3f32, 0.9f32);
        let hull = Disturbance {
            pos: DVec2::new(8.0, 8.0),
            radius: beam,
            press: 1.0,
            keel: Vec2::new(len, 0.0),
            ..Default::default()
        };
        rasterize(&mut push, n, 0.25, DVec2::ZERO, 0.25, &hull).expect("on the grid");
        // Its area: half an ellipse aft, a parabola's point forward.
        let area: f32 = push.iter().map(|p| p[1]).sum::<f32>() * 0.25 * 0.25;
        let want = beam * len * (std::f32::consts::FRAC_PI_2 + 4.0 / 3.0);
        assert!((area - want).abs() < 0.05 * want, "{area} m², want {want}");
        assert!(push.iter().all(|p| p[0] == 0.0));
        // As wide as its beam amidships, narrower toward the bow than the stern.
        let width = |x: f64| {
            let i = (x / 0.25) as usize;
            (0..n).map(|j| push[j * n + i][1]).sum::<f32>() * 0.25
        };
        assert!((width(8.0) - 2.0 * beam).abs() < 0.15, "{}", width(8.0));
        assert!(
            width(9.6) < 0.8 * width(6.4),
            "{} {}",
            width(9.6),
            width(6.4)
        );
    }

    #[test]
    fn a_sharp_hull_draws_short_waves_a_smooth_patch_does_not() {
        // The same pressure volume at 5 m/s, as the hull's waterline or as one round patch:
        // the waterline's edges make the short divergent waves whose steep faces show the V.
        let field = pond();
        let dt = 1.0 / 30.0;
        let run = |shape: Disturbance| {
            let mut sim = SurfaceSim::new(N, field.cell, field.origin);
            for s in 0..180 {
                let d = Disturbance {
                    pos: DVec2::new(-25.0 + 5.0 * dt as f64 * s as f64, 0.0),
                    ..shape
                };
                sim.step(&field, &[d], field.cell, dt, 0.0);
            }
            // Mean square slope over the wake behind it.
            let h = sim.heights();
            let mut sum = 0.0f32;
            let mut count = 0;
            for x in 0..60 {
                for z in -32..=32 {
                    let p = DVec2::new(-15.0 + x as f64 * 0.25, z as f64 * 0.25);
                    let g = h.sample(p).unwrap().1;
                    sum += g.length_squared();
                    count += 1;
                }
            }
            sum / count as f32
        };
        let head = 0.25 * dt;
        let sharp = run(Disturbance {
            radius: 0.9,
            press: head,
            keel: Vec2::new(2.3, 0.0),
            ..Default::default()
        });
        // exp(−2(d/r)²) holds πr²/2 under it.
        let area = 0.9 * 2.3 * (std::f32::consts::FRAC_PI_2 + 4.0 / 3.0);
        let r = 2.0f32;
        let smooth = run(Disturbance {
            radius: r,
            press: head * area / (std::f32::consts::PI * r * r * 0.5),
            ..Default::default()
        });
        assert!(sharp > 2.0 * smooth, "sharp {sharp}, smooth {smooth}");
    }

    #[test]
    fn a_current_carries_short_waves_without_wearing_them_down() {
        // A 2 m wave train riding a 1 m/s stream for two seconds keeps its height as it does
        // in still water (a bilinear trace back lost half of it).
        struct Flowing(f32);
        impl Medium for Flowing {
            fn wet(&self, _: usize) -> f32 {
                1.0
            }
            fn carry(&self, _: usize, _: usize, _: usize) -> Vec2 {
                Vec2::new(self.0, 0.0)
            }
        }
        let run = |v: f32| {
            let mut sim = SurfaceSim::level(N, 0.25, DVec2::ZERO, Band::All, 2.0);
            let k = std::f32::consts::TAU / 2.0;
            for j in 0..N {
                for i in 0..N {
                    sim.state[j * N + i][0] = 0.02 * (k * i as f32 * 0.25).cos();
                }
            }
            sim.quiet_s = 0.0;
            for _ in 0..120 {
                sim.step(&Flowing(v), &[], 0.25, 1.0 / 60.0, 0.0);
            }
            let h = sim.heights();
            h.h.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        let (still, carried) = (run(0.0), run(1.0));
        assert!(still > 0.008, "{still}");
        assert!(carried > 0.6 * still, "still {still}, carried {carried}");
    }
}
