//! The swash band: waves running up a beach, thinning out and draining back.
//!
//! The dispersive surface ([`crate::surface`]) and the ripple grid see dry land as a wall: the
//! waterline never moves, and a wake that reaches a beach only darkens the sand
//! ([`crate::shore`]). Here a narrow strip along the shore runs the full shallow-water
//! equations, which keep their nonlinear terms and let water flood dry ground and leave it
//! again. A wave shoals into a bore, the bore collapses on the sand and climbs it like a
//! thrown ball, slows, thins out and slides back down until the next one meets it.
//!
//! - **Scheme.** Finite volumes on a cell-centred grid. Each face's flux is HLL on the
//!   hydrostatic reconstruction of Audusse et al. 2004 ("A fast and stable well-balanced
//!   scheme with hydrostatic reconstruction for shallow water flows"): depths at a face are
//!   cut to the higher of the two beds, so depth never goes negative and a lake at rest stays
//!   at rest over any bed, its waterline included. Second order (minmod slopes, SSP-RK2) where
//!   a cell and its neighbours are wet, first order at the wet/dry front. Velocities in thin
//!   water are desingularized as Kurganov & Petrova 2007 do. Friction is Manning's,
//!   semi-implicit (sand, n 0.025): it is what makes the backwash thin out and stop.
//! - **Forcing.** In a strip where the still water is [`RELAX_FROM_M`]–[`RELAX_TO_M`] deep,
//!   the state is blended toward the waves of the surface and the ripple grid (relaxation
//!   zones of numerical wave tanks, Jacobsen, Fuhrman & Fredsøe 2012): the strip lets their
//!   waves in and soaks up what the beach sends back. Deeper than that the swash is the
//!   surface's waves; shallower, it runs free.
//! - **Sand.** Water soaks into sand above the still level ([`INFILTRATION_MS`]), so the
//!   backwash carries less than the uprush (Turner & Masselink 1998). Each cell remembers
//!   being wet: the glassy sheen goes in seconds, the darkening in a minute or two (wet sand
//!   is darker because light is trapped in the water between the grains, Lekner & Dorf 1988).
//! - **Foam** rides the flow as a tracer, is made where a bore front steepens and at the tip
//!   of the uprush, and is stranded where the water drains: the swash mark at the top of
//!   each run.
//!
//! The renderer runs it on the GPU (`render/swash.rs`, `swash.wgsl`) in step with this
//! reference, which the tests use.

use std::sync::Arc;

use engine_core::math::{DVec2, DVec3, Vec2};
use engine_core::prelude::*;
use engine_world::Ground;

use crate::state::BodyState;

/// Cells per side.
pub const N: usize = 512;
/// Cell size, meters (two to a ripple cell's 0.25 m).
pub const CELL_M: f32 = 0.125;
/// Cells per side of a tile: what is stepped or left asleep together.
pub const TILE: usize = 16;
/// Water this deep or less counts as dry (for drawing, the sand and the foam), meters.
pub const DRY_M: f32 = 5e-4;
/// Depth below which velocities are desingularized (Kurganov & Petrova), meters.
const H_EPS: f32 = 1e-3;
/// Still water this deep or less runs free, meters.
pub const RELAX_FROM_M: f32 = 0.45;
/// Still water this deep or more is the surface's own waves, meters.
pub const RELAX_TO_M: f32 = 0.9;
/// Ground higher than this over the still level never takes part, meters (twice the
/// highest run-up a wake sends up a steep shore).
pub const BAND_RISE_M: f32 = 0.6;
/// Manning's roughness of sand, s/m^(1/3).
pub const MANNING: f32 = 0.025;
/// How fast water soaks into sand above the still level, m/s (it soaks in at the water table
/// too: the swash film there drains into the beach).
pub const INFILTRATION_MS: f32 = 6e-4;
/// Fastest water anywhere, m/s.
pub const MAX_SPEED: f32 = 5.0;
/// Courant number of a step.
pub const CFL: f32 = 0.25;
/// Most steps a frame takes.
pub const MAX_STEPS: u32 = 8;
/// How long wet sand keeps its glassy sheen, seconds (the film soaks in).
pub const SHEEN_S: f32 = 4.0;
/// How long sand stays dark once the water has gone, seconds.
pub const DAMP_S: f32 = 90.0;
/// How long the foam stranded at the top of a run stays, seconds.
pub const MARK_S: f32 = 20.0;
/// Sand this little above the still level stays wet (the water table), meters.
pub const TABLE_M: f32 = 0.05;
/// A bore front steeper than this foams.
pub const BORE_SLOPE: f32 = 0.3;
/// Foam a bore front makes per second.
const BORE_FOAM: f32 = 4.0;
/// The tip of the uprush foams when this thin and this fast (m, m/s).
const TIP_DEPTH_M: f32 = 0.02;
const TIP_SPEED_MS: f32 = 0.4;
const TIP_FOAM: f32 = 2.0;
const G: f32 = 9.81;

/// A cell's water: depth (m) and discharge (m²/s) along x and z, and foam carried along
/// (concentration × depth, m).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cell {
    pub h: f32,
    pub hu: f32,
    pub hv: f32,
    pub hc: f32,
}

impl Cell {
    fn axpy(self, a: f32, o: Cell) -> Cell {
        Cell {
            h: self.h + a * o.h,
            hu: self.hu + a * o.hu,
            hv: self.hv + a * o.hv,
            hc: self.hc + a * o.hc,
        }
    }

    fn lerp(self, o: Cell, t: f32) -> Cell {
        self.axpy(t, o.axpy(-1.0, self))
    }
}

/// What the surface and the ripple grid want the water to do over a cell: height over the
/// still level (m), velocity (world x/z, m/s), foam (0..1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Target {
    pub eta: f32,
    pub vel: Vec2,
    pub foam: f32,
}

/// What a cell's sand remembers of the water (each 0..1, `reach` in meters).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sand {
    /// The film's sheen: 1 while covered, gone in about [`SHEEN_S`].
    pub sheen: f32,
    /// Darkening: 1 while covered, gone in about [`DAMP_S`].
    pub damp: f32,
    /// Foam stranded where the water drained.
    pub mark: f32,
    /// Foam on the water while covered.
    pub foam: f32,
}

/// Desingularized velocity of discharge `q` in water `h` deep (Kurganov & Petrova 2007, eq.
/// 2.17): q/h where the water is deep, going to 0 as it thins out.
#[inline]
pub fn velocity(h: f32, q: f32) -> f32 {
    let h4 = h * h * h * h;
    let v = std::f32::consts::SQRT_2 * h * q / (h4 + h4.max(H_EPS * H_EPS * H_EPS * H_EPS)).sqrt();
    v.clamp(-MAX_SPEED, MAX_SPEED)
}

/// Minmod of three: 0 where they disagree in sign, else the smallest.
#[inline]
fn minmod(a: f32, b: f32, c: f32) -> f32 {
    if a > 0.0 && b > 0.0 && c > 0.0 {
        a.min(b).min(c)
    } else if a < 0.0 && b < 0.0 && c < 0.0 {
        a.max(b).max(c)
    } else {
        0.0
    }
}

/// Limited slope (per cell) of `l`, `c`, `r`, with θ 1.3 (generalized minmod).
#[inline]
pub fn slope(l: f32, c: f32, r: f32) -> f32 {
    const THETA: f32 = 1.3;
    minmod(THETA * (c - l), 0.5 * (r - l), THETA * (r - c))
}

/// One side of a face: depth, bed, velocity across and along it, foam concentration.
#[derive(Debug, Clone, Copy, Default)]
pub struct Side {
    pub h: f32,
    pub b: f32,
    pub un: f32,
    pub ut: f32,
    pub c: f32,
}

/// The flux through a face between `l` and `r` (normal along +x or +z): (depth, normal
/// discharge, tangential discharge, foam), plus the hydrostatic corrections each side adds
/// to its normal momentum (Audusse et al. 2004). Zero where both sides are dry.
#[inline]
pub fn face_flux(l: Side, r: Side) -> ([f32; 4], f32, f32) {
    let b = l.b.max(r.b);
    let hl = (l.h + l.b - b).max(0.0);
    let hr = (r.h + r.b - b).max(0.0);
    let corr_l = 0.5 * G * (l.h * l.h - hl * hl);
    let corr_r = 0.5 * G * (r.h * r.h - hr * hr);
    if hl <= 0.0 && hr <= 0.0 {
        return ([0.0; 4], corr_l, corr_r);
    }
    let (cl, cr) = ((G * hl).sqrt(), (G * hr).sqrt());
    let sl = (l.un - cl).min(r.un - cr);
    let sr = (l.un + cl).max(r.un + cr);
    let flux = |h: f32, s: Side| [h * s.un, h * s.un * s.un + 0.5 * G * h * h, h * s.un * s.ut];
    let (fl, fr) = (flux(hl, l), flux(hr, r));
    let f = if sl >= 0.0 {
        fl
    } else if sr <= 0.0 {
        fr
    } else {
        let ul = [hl, hl * l.un, hl * l.ut];
        let ur = [hr, hr * r.un, hr * r.ut];
        let k = 1.0 / (sr - sl);
        [0, 1, 2].map(|m| (sr * fl[m] - sl * fr[m] + sl * sr * (ur[m] - ul[m])) * k)
    };
    // Foam goes with the water, from upwind.
    let c = if f[0] >= 0.0 { l.c } else { r.c };
    ([f[0], f[1], f[2], f[0] * c], corr_l, corr_r)
}

/// The swash over a square grid.
#[derive(Debug, Clone)]
pub struct SwashField {
    /// Cells per side.
    pub n: usize,
    /// Cell size, meters.
    pub cell: f32,
    /// World x/z of the corner of cell (0, 0).
    pub origin: DVec2,
    /// World height the heights here are measured from.
    pub datum: f64,
    /// Ground per cell, over `datum`, meters.
    pub bed: Vec<f32>,
    /// Still water level per cell over `datum` (`NAN` where no water lies).
    pub still: Vec<f32>,
    /// How much of the surface's waves a cell takes each step: 1 in deep water (it is the
    /// surface), 0 where it runs free.
    pub relax: Vec<f32>,
    /// The water.
    pub water: Vec<Cell>,
    /// What the sand remembers.
    pub sand: Vec<Sand>,
    /// Highest water level reached per cell since it was last taken (`take_reach`), over
    /// `datum` (`NEG_INFINITY` where none).
    pub reach: Vec<f32>,
    /// The surface's waves per cell (set before each step).
    pub target: Vec<Target>,
    /// Tiles that take part (the shore runs through them).
    pub band: Vec<bool>,
    /// Steps taken in the last frame.
    pub steps: u32,
    /// Whether the bed drags on the water (off for the analytic tests).
    pub friction: bool,
    // Scratch: the state at the start of the step, the change, and limited slopes per cell
    // ((depth, level, u, v) along x, then along z).
    start: Vec<Cell>,
    change: Vec<Cell>,
    slopes: Vec<[[f32; 4]; 2]>,
    vel: Vec<Vec2>,
    /// Tiles the step touches: the band and its neighbours.
    reach_tiles: Vec<bool>,
}

impl SwashField {
    /// An empty field (all dry, nothing in the band).
    pub fn new(n: usize, cell: f32) -> Self {
        assert!(n.is_multiple_of(TILE), "the grid is whole tiles");
        let cells = n * n;
        let tiles = (n / TILE) * (n / TILE);
        Self {
            n,
            cell,
            origin: DVec2::ZERO,
            datum: 0.0,
            bed: vec![0.0; cells],
            still: vec![f32::NAN; cells],
            relax: vec![0.0; cells],
            water: vec![Cell::default(); cells],
            sand: vec![Sand::default(); cells],
            reach: vec![f32::NEG_INFINITY; cells],
            target: vec![Target::default(); cells],
            band: vec![false; tiles],
            steps: 0,
            friction: true,
            start: vec![Cell::default(); cells],
            change: vec![Cell::default(); cells],
            slopes: vec![[[0.0; 4]; 2]; cells],
            vel: vec![Vec2::ZERO; cells],
            reach_tiles: vec![false; tiles],
        }
    }

    /// World x/z of cell (i, j)'s center.
    pub fn cell_center(&self, i: usize, j: usize) -> DVec2 {
        self.origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * self.cell as f64
    }

    /// How much of the surface's waves water `depth` deep (still) takes: Jacobsen's
    /// exponential ramp, 0 at [`RELAX_FROM_M`] and shallower, 1 at [`RELAX_TO_M`].
    pub fn relax_weight(depth: f32) -> f32 {
        let s = ((depth - RELAX_FROM_M) / (RELAX_TO_M - RELAX_FROM_M)).clamp(0.0, 1.0);
        ((s.powf(3.5)).exp() - 1.0) / (std::f32::consts::E - 1.0)
    }

    /// Lay the field over the ground: `ground` and `still` per cell (world heights at world
    /// x/z; `None` still where no water lies), heights measured from `datum`. The water starts
    /// at rest, the sand dry.
    pub fn lay(
        &mut self,
        origin: DVec2,
        datum: f64,
        ground: &dyn Fn(DVec2) -> f64,
        still: &dyn Fn(DVec2) -> Option<f64>,
    ) {
        self.origin = origin;
        self.datum = datum;
        let n = self.n;
        for j in 0..n {
            for i in 0..n {
                let k = j * n + i;
                let p = self.cell_center(i, j);
                self.bed[k] = (ground(p) - datum) as f32;
                self.still[k] = still(p).map_or(f32::NAN, |s| (s - datum) as f32);
            }
        }
        self.settle();
    }

    /// After the bed or the still levels changed: the strip's weights, the band, and the
    /// water at rest.
    pub fn settle(&mut self) {
        let n = self.n;
        let t = n / TILE;
        self.band.fill(false);
        for k in 0..n * n {
            let (b, s) = (self.bed[k], self.still[k]);
            let depth = if s.is_nan() { -1.0 } else { s - b };
            self.relax[k] = if s.is_nan() {
                0.0
            } else {
                Self::relax_weight(depth)
            };
            self.water[k] = Cell {
                h: depth.max(0.0),
                ..Cell::default()
            };
            if !s.is_nan() && depth < RELAX_TO_M && depth > -BAND_RISE_M {
                let (i, j) = (k % n, k / n);
                self.band[(j / TILE) * t + i / TILE] = true;
            }
        }
        self.sand.fill(Sand::default());
        self.reach.fill(f32::NEG_INFINITY);
        self.target.fill(Target::default());
        self.mark_reach_tiles();
    }

    fn mark_reach_tiles(&mut self) {
        let t = self.n / TILE;
        self.reach_tiles.fill(false);
        for tj in 0..t {
            for ti in 0..t {
                if !self.band[tj * t + ti] {
                    continue;
                }
                for dj in -1i32..=1 {
                    for di in -1i32..=1 {
                        let (x, z) = (ti as i32 + di, tj as i32 + dj);
                        if x >= 0 && z >= 0 && (x as usize) < t && (z as usize) < t {
                            self.reach_tiles[z as usize * t + x as usize] = true;
                        }
                    }
                }
            }
        }
    }

    /// Every tile takes part (the tests' small basins).
    pub fn band_everywhere(&mut self) {
        self.band.fill(true);
        self.mark_reach_tiles();
    }

    /// Water level over `datum` at cell `k`.
    pub fn level(&self, k: usize) -> f32 {
        self.bed[k] + self.water[k].h
    }

    /// Water held in the band, m³.
    pub fn volume(&self) -> f64 {
        let a = (self.cell * self.cell) as f64;
        self.water.iter().map(|c| c.h as f64 * a).sum()
    }

    /// Cells of tiles in `tiles`, row by row.
    fn cells_of(&self, tiles: &[bool]) -> Vec<usize> {
        let (n, t) = (self.n, self.n / TILE);
        let mut out = Vec::new();
        for j in 0..n {
            let tj = j / TILE;
            for ti in 0..t {
                if tiles[tj * t + ti] {
                    out.extend((ti * TILE..(ti + 1) * TILE).map(|i| j * n + i));
                }
            }
        }
        out
    }

    /// Run `dt` seconds: as many steps as the fastest wave allows (at most [`MAX_STEPS`];
    /// past that the frame is cut short), each blended toward `target` in the strip; then
    /// the sand and the foam.
    pub fn step(&mut self, dt: f32, foam_life_s: f32) {
        let cells = self.cells_of(&self.reach_tiles.clone());
        let band = self.cells_of(&self.band.clone());
        let mut left = dt;
        self.steps = 0;
        while left > 1e-6 && self.steps < MAX_STEPS {
            let mut fastest = 0.0f32;
            for &k in &band {
                let c = self.water[k];
                if c.h > DRY_M {
                    let v = Vec2::new(velocity(c.h, c.hu), velocity(c.h, c.hv));
                    fastest = fastest.max(v.x.abs().max(v.y.abs()) + (G * c.h).sqrt());
                }
            }
            let mut sub = if fastest > 0.0 {
                CFL * self.cell / fastest
            } else {
                left
            };
            // Even steps over what is left, so the last isn't a sliver.
            sub = left / (left / sub).ceil();
            self.rk2(sub, &cells, &band);
            self.relax_toward_target(&band);
            left -= sub;
            self.steps += 1;
        }
        self.after_frame(dt, foam_life_s, &band);
    }

    /// One SSP-RK2 step (Shu & Osher) of `dt`.
    fn rk2(&mut self, dt: f32, cells: &[usize], band: &[usize]) {
        for &k in cells {
            self.start[k] = self.water[k];
        }
        self.rates(cells, band);
        for &k in band {
            self.water[k] = self.water[k].axpy(dt, self.change[k]);
            clamp_cell(&mut self.water[k]);
        }
        self.rates(cells, band);
        for &k in band {
            let euler = self.water[k].axpy(dt, self.change[k]);
            let mut c = self.start[k].lerp(euler, 0.5);
            clamp_cell(&mut c);
            self.water[k] = c;
        }
        // Friction and the sand soaking water up.
        let n_dt = if self.friction {
            G * MANNING * MANNING * dt
        } else {
            0.0
        };
        for &k in band {
            let c = &mut self.water[k];
            if c.h <= DRY_M {
                c.hu = 0.0;
                c.hv = 0.0;
                continue;
            }
            let speed = (velocity(c.h, c.hu).powi(2) + velocity(c.h, c.hv).powi(2)).sqrt();
            let keep = 1.0 / (1.0 + n_dt * speed / c.h.powf(4.0 / 3.0));
            c.hu *= keep;
            c.hv *= keep;
            let s = self.still[k];
            if !s.is_nan() && self.bed[k] > s {
                let was = c.h;
                c.h = (c.h - INFILTRATION_MS * dt).max(0.0);
                let kept = if was > 0.0 { c.h / was } else { 0.0 };
                c.hu *= kept;
                c.hv *= kept;
                c.hc *= kept;
            }
        }
    }

    /// d(water)/dt over `band` into `change` (`cells` are the band and a tile around it).
    fn rates(&mut self, cells: &[usize], band: &[usize]) {
        let n = self.n;
        let inv_dx = 1.0 / self.cell;
        // Velocities, then slopes (first order at the wet/dry front).
        for &k in cells {
            let c = &mut self.water[k];
            let v = Vec2::new(velocity(c.h, c.hu), velocity(c.h, c.hv));
            c.hu = c.h * v.x;
            c.hv = c.h * v.y;
            self.vel[k] = v;
        }
        let at = |i: isize, j: isize| -> usize {
            let m = n as isize - 1;
            (j.clamp(0, m) * n as isize + i.clamp(0, m)) as usize
        };
        for &k in cells {
            let (i, j) = ((k % n) as isize, (k / n) as isize);
            let wet = |k: usize| self.water[k].h > DRY_M;
            let nb = [at(i - 1, j), at(i + 1, j), at(i, j - 1), at(i, j + 1)];
            if !wet(k) || nb.iter().any(|&o| !wet(o)) {
                self.slopes[k] = [[0.0; 4]; 2];
                continue;
            }
            let q = |o: usize| {
                let c = self.water[o];
                [c.h, c.h + self.bed[o], self.vel[o].x, self.vel[o].y]
            };
            let (c, w, e, s, nn) = (q(k), q(nb[0]), q(nb[1]), q(nb[2]), q(nb[3]));
            self.slopes[k] = [
                [0, 1, 2, 3].map(|m| slope(w[m], c[m], e[m])),
                [0, 1, 2, 3].map(|m| slope(s[m], c[m], nn[m])),
            ];
        }
        // One side of cell `k`'s face along `axis` (0 x, 1 z), `sign` +1 the far face.
        let side = |k: usize, axis: usize, sign: f32| -> Side {
            let c = self.water[k];
            let d = self.slopes[k][axis];
            let h = (c.h + 0.5 * sign * d[0]).max(0.0);
            let level = c.h + self.bed[k] + 0.5 * sign * d[1];
            let (u, v) = (
                self.vel[k].x + 0.5 * sign * d[2],
                self.vel[k].y + 0.5 * sign * d[3],
            );
            let (un, ut) = if axis == 0 { (u, v) } else { (v, u) };
            Side {
                h,
                b: level - h,
                un,
                ut,
                c: if c.h > DRY_M {
                    (c.hc / c.h).clamp(0.0, 1.0)
                } else {
                    0.0
                },
            }
        };
        for &k in band {
            let (i, j) = ((k % n) as isize, (k / n) as isize);
            let mut d = Cell::default();
            for axis in 0..2 {
                let (lo, hi) = if axis == 0 {
                    (at(i - 1, j), at(i + 1, j))
                } else {
                    (at(i, j - 1), at(i, j + 1))
                };
                let near = side(k, axis, -1.0);
                let far = side(k, axis, 1.0);
                // The grid's edge is a wall: a mirrored cell beyond it.
                let mirror = |s: Side| Side { un: -s.un, ..s };
                let (first, last) = if axis == 0 {
                    (i == 0, i == n as isize - 1)
                } else {
                    (j == 0, j == n as isize - 1)
                };
                let beyond_hi = if last {
                    mirror(far)
                } else {
                    side(hi, axis, -1.0)
                };
                let beyond_lo = if first {
                    mirror(near)
                } else {
                    side(lo, axis, 1.0)
                };
                // Through the far face (this cell on the left) and the near one (on the right).
                let (f_hi, corr_l, _) = face_flux(far, beyond_hi);
                let (f_lo, _, corr_r) = face_flux(beyond_lo, near);
                // The bed's slope inside the cell (zero at first order).
                let source = -G * 0.5 * (near.h + far.h) * (far.b - near.b);
                let dn = -(f_hi[1] + corr_l - f_lo[1] - corr_r) + source;
                let dt_ = -(f_hi[2] - f_lo[2]);
                d.h -= (f_hi[0] - f_lo[0]) * inv_dx;
                d.hc -= (f_hi[3] - f_lo[3]) * inv_dx;
                if axis == 0 {
                    d.hu += dn * inv_dx;
                    d.hv += dt_ * inv_dx;
                } else {
                    d.hv += dn * inv_dx;
                    d.hu += dt_ * inv_dx;
                }
            }
            self.change[k] = d;
        }
    }

    /// Blend the strip toward the surface's waves.
    fn relax_toward_target(&mut self, band: &[usize]) {
        for &k in band {
            let r = self.relax[k];
            if r <= 0.0 {
                continue;
            }
            let t = self.target[k];
            let h = (self.still[k] + t.eta - self.bed[k]).max(0.0);
            let want = Cell {
                h,
                hu: h * t.vel.x,
                hv: h * t.vel.y,
                hc: h * t.foam,
            };
            self.water[k] = self.water[k].lerp(want, r);
        }
    }

    /// Once a frame: foam made and fading, what the sand remembers, the highest reach.
    fn after_frame(&mut self, dt: f32, foam_life_s: f32, band: &[usize]) {
        let n = self.n;
        let fade = (-dt / foam_life_s.max(1e-3)).exp();
        let (sheen, damp, mark) = (
            (-dt / SHEEN_S).exp(),
            (-dt / DAMP_S).exp(),
            (-dt / MARK_S).exp(),
        );
        let inv_2dx = 0.5 / self.cell;
        for &k in band {
            let (i, j) = (k % n, k / n);
            let c = self.water[k];
            let mut sd = self.sand[k];
            let wet = c.h > DRY_M;
            if wet {
                let conc = (c.hc / c.h).clamp(0.0, 1.0);
                let v = Vec2::new(velocity(c.h, c.hu), velocity(c.h, c.hv));
                // A bore front: the surface steep and the water running into the slope.
                let lvl = |ii: usize, jj: usize| {
                    let o = jj * n + ii;
                    if self.water[o].h > DRY_M {
                        self.level(o)
                    } else {
                        self.level(k)
                    }
                };
                let (im, ip) = (i.saturating_sub(1), (i + 1).min(n - 1));
                let (jm, jp) = (j.saturating_sub(1), (j + 1).min(n - 1));
                let grad = Vec2::new(lvl(ip, j) - lvl(im, j), lvl(i, jp) - lvl(i, jm)) * inv_2dx;
                let mut make = 0.0;
                if grad.length() > BORE_SLOPE && grad.dot(v) < 0.0 {
                    make += BORE_FOAM * dt;
                }
                // The tip of the uprush: thin, fast, toward dry sand.
                let dry_ahead = {
                    let ahead = Vec2::new(v.x.signum(), v.y.signum());
                    let (ai, aj) = (
                        (i as i32 + ahead.x as i32).clamp(0, n as i32 - 1) as usize,
                        (j as i32 + ahead.y as i32).clamp(0, n as i32 - 1) as usize,
                    );
                    self.water[aj * n + ai].h <= DRY_M
                };
                if c.h < TIP_DEPTH_M && v.length() > TIP_SPEED_MS && dry_ahead {
                    make += TIP_FOAM * dt;
                }
                let conc = ((conc + make) * fade).min(1.0);
                self.water[k].hc = conc * c.h;
                sd.sheen = 1.0;
                sd.damp = 1.0;
                sd.foam = conc;
                let lv = self.bed[k] + c.h;
                self.reach[k] = self.reach[k].max(lv);
            } else {
                // Just drained: the foam it carried stays on the sand.
                if sd.sheen >= 1.0 {
                    sd.mark = sd.mark.max(sd.foam);
                }
                sd.foam = 0.0;
                self.water[k].hc = 0.0;
                let table = !self.still[k].is_nan() && self.bed[k] <= self.still[k] + TABLE_M;
                sd.sheen *= sheen;
                if !table {
                    sd.damp *= damp;
                }
                sd.mark *= mark;
            }
            self.sand[k] = sd;
        }
    }

    /// The highest level reached per cell since the last call (over `datum`), and forget it.
    pub fn take_reach(&mut self) -> Vec<f32> {
        std::mem::replace(&mut self.reach, vec![f32::NEG_INFINITY; self.n * self.n])
    }
}

/// Ground the world's height query doesn't know: a zone drawn as a mesh of its own (the
/// demo's tarn and its beach) tells the swash its shape. On an entity: `height` takes x/z
/// meters from the entity and answers meters above it, within `half_size` each way.
#[derive(Component, Clone)]
pub struct BeachGround {
    pub half_size: f32,
    pub height: Arc<dyn Fn(f32, f32) -> f32 + Send + Sync>,
}

/// The ground and the still water under the swash grid, laid over the ripple grid's window
/// ([`crate::ripples::RippleField::swash`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SwashBed {
    /// World x/z of the corner of cell (0, 0).
    pub origin: DVec2,
    /// Cells per side.
    pub n: usize,
    /// Cell size, meters.
    pub cell: f32,
    /// World height the heights are measured from.
    pub datum: f64,
    /// Ground per cell over `datum`, meters.
    pub bed: Vec<f32>,
    /// Still water level per cell over `datum` (`NAN` where none, or a stream runs).
    pub still: Vec<f32>,
    /// Tiles the shore runs through ([`TILE`]² cells each, row-major).
    pub band: Vec<bool>,
    /// Counts up whenever any of it changes.
    pub version: u64,
}

impl SwashBed {
    /// Whether tile `t`, by its cells, lies in the band: still water under [`RELAX_TO_M`]
    /// or ground less than [`BAND_RISE_M`] over it.
    fn tile_in_band(&self, t: usize) -> bool {
        let tiles = self.n / TILE;
        let (ti, tj) = (t % tiles, t / tiles);
        (tj * TILE..(tj + 1) * TILE).any(|j| {
            (ti * TILE..(ti + 1) * TILE).any(|i| {
                let k = j * self.n + i;
                let depth = self.still[k] - self.bed[k];
                !depth.is_nan() && depth < RELAX_TO_M && depth > -BAND_RISE_M
            })
        })
    }

    /// Any tile in the band.
    pub fn any(&self) -> bool {
        self.band.iter().any(|b| *b)
    }
}

/// How far past a lake's edge its beach is laid, meters.
pub const BEACH_M: f32 = 4.0;
/// The ground is read this far apart and smoothed in between, meters (the drawn waterline
/// comes from the real ground, so this only moves where the water flows).
pub const SAMPLE_M: f32 = 0.5;
/// Ground samples read per step after a jump.
const SAMPLE_READS: usize = 2048;

/// Reads the ground and the still water under the swash grid, like the ripple grid's still
/// depths ([`crate::sim`]): only the samples the window newly covers, nearest the focus first.
#[derive(Default)]
pub struct SwashReader {
    /// (window corner, samples per side, layout) the samples were read for.
    at: Option<(DVec2, usize, u64)>,
    /// Per sample: (ground, still) world heights (`NAN` none); `INFINITY` ground unread.
    samples: Vec<(f64, f64)>,
    unread: usize,
    /// What was last handed out.
    built: Option<(DVec2, u64, usize)>,
    version: u64,
}

impl SwashReader {
    /// Samples still to read.
    pub fn unread(&self) -> usize {
        self.unread
    }

    /// Read the ground under a window `size` m wide from `origin` (the ripple grid's), and
    /// return a new bed when it changed. `stream` says where a stream runs (no swash there).
    #[allow(clippy::too_many_arguments)]
    pub fn read(
        &mut self,
        origin: DVec2,
        size: f64,
        layout: u64,
        bodies: &[BodyState],
        beaches: &[(DVec3, BeachGround)],
        ground: &Ground,
        focus: DVec3,
        stream: &dyn Fn(DVec2) -> bool,
    ) -> Option<Arc<SwashBed>> {
        let m = (size / SAMPLE_M as f64).round() as usize + 1;
        let at = (origin, m, layout);
        let mut budget = SAMPLE_READS;
        if self.at != Some(at) {
            let mut samples = vec![(f64::INFINITY, f64::NAN); m * m];
            match self.at {
                Some((old, old_m, old_layout)) if old_m == m && old_layout == layout => {
                    let step = ((origin - old) / SAMPLE_M as f64).round();
                    let (di, dj) = (step.x as i64, step.y as i64);
                    if di.abs() < m as i64 && dj.abs() < m as i64 {
                        for (idx, s) in samples.iter_mut().enumerate() {
                            let (i, j) = ((idx % m) as i64 + di, (idx / m) as i64 + dj);
                            if i >= 0 && j >= 0 && i < m as i64 && j < m as i64 {
                                *s = self.samples[j as usize * m + i as usize];
                            }
                        }
                    }
                }
                _ => budget = usize::MAX,
            }
            self.unread = samples.iter().filter(|s| s.0 == f64::INFINITY).count();
            self.samples = samples;
            self.at = Some(at);
        }
        if self.unread > 0 {
            let at_sample = |idx: usize| {
                origin + DVec2::new((idx % m) as f64, (idx / m) as f64) * SAMPLE_M as f64
            };
            let f = DVec2::new(focus.x, focus.z);
            let mut todo: Vec<(f64, usize)> = (0..m * m)
                .filter(|&idx| self.samples[idx].0 == f64::INFINITY)
                .map(|idx| ((at_sample(idx) - f).length_squared(), idx))
                .collect();
            if todo.len() > budget {
                todo.select_nth_unstable_by(budget, |a, b| a.0.total_cmp(&b.0));
                todo.truncate(budget);
            }
            for (_, idx) in todo {
                self.samples[idx] = sample(at_sample(idx), bodies, beaches, ground, focus.y);
                self.unread -= 1;
            }
        }
        // Hand out a bed once the samples near the focus are in, and again when all are.
        let key = (origin, layout, self.unread);
        if self
            .built
            .is_some_and(|b| b.0 == origin && b.1 == layout && (b.2 == 0 || self.unread > 0))
        {
            return None;
        }
        self.built = Some(key);
        self.version += 1;
        Some(Arc::new(self.build(origin, size, m, focus.y, stream)))
    }

    fn build(
        &self,
        origin: DVec2,
        size: f64,
        m: usize,
        level_y: f64,
        stream: &dyn Fn(DVec2) -> bool,
    ) -> SwashBed {
        let n = ((size / CELL_M as f64).round() as usize).max(TILE) / TILE * TILE;
        let cell = (size / n as f64) as f32;
        let datum = level_y.round();
        let mut bed = vec![0.0f32; n * n];
        let mut still = vec![f32::NAN; n * n];
        let ratio = cell as f64 / SAMPLE_M as f64;
        for j in 0..n {
            for i in 0..n {
                let g = DVec2::new((i as f64 + 0.5) * ratio, (j as f64 + 0.5) * ratio);
                let (i0, j0) = (
                    (g.x.floor() as usize).min(m - 2),
                    (g.y.floor() as usize).min(m - 2),
                );
                let (fx, fz) = (g.x - i0 as f64, g.y - j0 as f64);
                let s = |a: usize, b: usize| self.samples[b * m + a];
                let corners = [s(i0, j0), s(i0 + 1, j0), s(i0, j0 + 1), s(i0 + 1, j0 + 1)];
                if corners.iter().any(|c| !c.0.is_finite()) {
                    continue;
                }
                let w = [
                    (1.0 - fx) * (1.0 - fz),
                    fx * (1.0 - fz),
                    (1.0 - fx) * fz,
                    fx * fz,
                ];
                let ground: f64 = (0..4).map(|c| corners[c].0 * w[c]).sum();
                let k = j * n + i;
                bed[k] = (ground - datum) as f32;
                // The still level is the lake's: the same at all four, or none.
                let levels = corners.iter().filter(|c| !c.1.is_nan());
                let level = levels.clone().map(|c| c.1).fold(f64::NAN, f64::max);
                let p = origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * cell as f64;
                if levels.count() == 4 && !stream(p) {
                    still[k] = (level - datum) as f32;
                }
            }
        }
        let tiles = n / TILE;
        let mut out = SwashBed {
            origin,
            n,
            cell,
            datum,
            bed,
            still,
            band: vec![false; tiles * tiles],
            version: self.version,
        };
        out.band = (0..tiles * tiles).map(|t| out.tile_in_band(t)).collect();
        out
    }
}

/// The ground and the still water level at world x/z (world heights; `INFINITY`-free).
fn sample(
    p: DVec2,
    bodies: &[BodyState],
    beaches: &[(DVec3, BeachGround)],
    ground: &Ground,
    level_y: f64,
) -> (f64, f64) {
    let mut still = f64::NAN;
    let mut under = None;
    for b in bodies.iter().filter(|b| b.body.interactive) {
        let p3 = DVec3::new(p.x, b.center.y, p.y);
        let inset = b.body.extent.inset(b.local(p3));
        if inset > -BEACH_M {
            still = still.max(b.center.y);
            if let Some(d) = b.depth_at(p3, ground)
                && d > 0.0
            {
                under = Some(b.center.y - d as f64);
            }
        }
    }
    let beach = beaches.iter().find_map(|(c, g)| {
        let (x, z) = ((p.x - c.x) as f32, (p.y - c.z) as f32);
        (x.abs() <= g.half_size && z.abs() <= g.half_size).then(|| c.y + (g.height)(x, z) as f64)
    });
    let probe = DVec3::new(p.x, level_y + 50.0, p.y);
    let g = beach
        .or(under)
        .or_else(|| ground.height(probe))
        .unwrap_or(f64::NAN);
    if g.is_nan() {
        (f64::NAN, f64::NAN)
    } else {
        (g, still)
    }
}

/// Depth never below 0 (rounding), and dry cells hold no momentum or foam.
fn clamp_cell(c: &mut Cell) {
    if c.h <= 0.0 {
        *c = Cell::default();
    } else {
        c.hc = c.hc.clamp(0.0, c.h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field `n` cells of `cell` m over the given ground, the still water at 0, all of it
    /// in the band and running free.
    fn basin(n: usize, cell: f32, ground: &dyn Fn(f32, f32) -> f32) -> SwashField {
        let mut s = SwashField::new(n, cell);
        s.lay(
            DVec2::ZERO,
            0.0,
            &|p| ground(p.x as f32, p.y as f32) as f64,
            &|_| Some(0.0),
        );
        s.band_everywhere();
        s.relax.fill(0.0);
        s
    }

    fn noise(x: f32, z: f32) -> f32 {
        let h = ((x * 12.9898 + z * 78.233).sin() * 43758.547).fract();
        (h - 0.5) * 0.04
    }

    #[test]
    fn a_still_lake_stays_off_the_sand() {
        // A 1:10 beach rising out of 0.6 m of water, with 4 cm of bumps.
        let mut s = basin(64, 0.125, &|x, z| -0.6 + x * 0.1 + noise(x, z));
        let v0 = s.volume();
        for _ in 0..600 {
            s.step(1.0 / 60.0, 6.0);
        }
        let fastest = s
            .water
            .iter()
            .map(|c| velocity(c.h, c.hu).abs().max(velocity(c.h, c.hv).abs()))
            .fold(0.0f32, f32::max);
        assert!(fastest < 1e-4, "the lake moves: {fastest} m/s");
        for (k, c) in s.water.iter().enumerate() {
            if s.bed[k] > TABLE_M {
                assert!(c.h <= DRY_M, "water climbed to {} at cell {k}", s.bed[k]);
            }
        }
        assert!(
            (s.volume() - v0).abs() < 1e-4 * v0,
            "{} -> {}",
            v0,
            s.volume()
        );
    }

    #[test]
    fn depth_never_goes_negative_and_water_is_kept() {
        // A closed bumpy box, half full, stirred hard and often.
        let mut s = basin(48, 0.125, &|x, z| {
            -0.3 + 0.25 * (x * 1.3).sin() * (z * 0.9).cos()
        });
        // No infiltration: the still level is above every bed.
        s.still.fill(1.0);
        let v0 = s.volume();
        let mut seed = 1u32;
        for step in 0..1500 {
            if step % 20 == 0 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let k = (seed as usize >> 8) % s.water.len();
                let c = &mut s.water[k];
                c.hu += 0.5 * ((seed >> 4) % 7) as f32 - 1.5;
                c.hv -= 0.5 * ((seed >> 7) % 5) as f32 - 1.0;
            }
            s.step(1.0 / 60.0, 6.0);
            assert!(s.water.iter().all(|c| c.h >= 0.0 && c.h.is_finite()));
        }
        let drift = (s.volume() - v0).abs() / v0;
        assert!(drift < 1e-4, "volume drifted by {drift}");
    }

    #[test]
    fn water_sloshes_in_a_bowl_on_time() {
        // Thacker's planar surface in a paraboloid (SWASHES §4.2.2), frictionless.
        let (a, h0, eta) = (8.0f32, 0.5f32, 0.5f32);
        let (n, cell) = (160usize, 0.125f32);
        let half = n as f32 * cell * 0.5;
        let omega = (2.0 * G * h0).sqrt() / a;
        let bed = |x: f32, z: f32| {
            let (x, z) = (x - half, z - half);
            -h0 * (1.0 - (x * x + z * z) / (a * a))
        };
        let exact = |x: f32, z: f32, t: f32| {
            let (xc, zc) = (x - half, z - half);
            let level = eta * h0 / (a * a)
                * (2.0 * xc * (omega * t).cos() + 2.0 * zc * (omega * t).sin() - eta);
            (level - bed(x, z)).max(0.0)
        };
        let mut s = basin(n, cell, &bed);
        s.still.fill(10.0); // no infiltration, no water table
        for j in 0..n {
            for i in 0..n {
                let p = s.cell_center(i, j);
                let h = exact(p.x as f32, p.y as f32, 0.0);
                s.water[j * n + i] = Cell {
                    h,
                    hu: 0.0,
                    hv: h * eta * omega,
                    hc: 0.0,
                };
            }
        }
        s.friction = false;
        let period = std::f32::consts::TAU / omega;
        let frames = (period * 60.0).round() as usize;
        let mut t = 0.0;
        let mut worst_shore = 0.0f32;
        for f in 0..frames {
            s.step(1.0 / 60.0, 6.0);
            t += 1.0 / 60.0;
            if f % 60 == 59 {
                // Shoreline along the x axis through the middle: furthest wet cell each way.
                let row = n / 2;
                let wet_x = |h: &dyn Fn(usize) -> f32| {
                    let xs: Vec<usize> = (0..n).filter(|&i| h(i) > 1e-3).collect();
                    (xs[0] as f32 * cell, *xs.last().unwrap() as f32 * cell)
                };
                let sim = wet_x(&|i| s.water[row * n + i].h);
                let ex = wet_x(&|i| {
                    let p = s.cell_center(i, row);
                    exact(p.x as f32, p.y as f32, t)
                });
                worst_shore = worst_shore
                    .max((sim.0 - ex.0).abs())
                    .max((sim.1 - ex.1).abs());
            }
        }
        assert!(
            worst_shore <= 3.0 * cell,
            "shoreline off by {worst_shore} m"
        );
    }

    #[test]
    fn a_bore_runs_up_like_a_thrown_ball() {
        // A thin sheet thrown up a frictionless 1:10 slope at 1.5 m/s climbs about u²/2g.
        let (n, cell) = (128usize, 0.05f32);
        let mut s = basin(n, cell, &|x, _| (x - 1.0) * 0.1);
        s.still.fill(10.0);
        s.friction = false;
        for k in 0..s.water.len() {
            let x = s.cell_center(k % n, k / n).x as f32;
            let h = if (0.5..1.2).contains(&x) { 0.02 } else { 0.0 };
            s.water[k] = Cell {
                h,
                hu: h * 1.5,
                hv: 0.0,
                hc: 0.0,
            };
        }
        let top0 = (1.2 - 1.0) * 0.1;
        let mut top = f32::NEG_INFINITY;
        // Up for u/(g·tanβ), about 1.5 s, and as long back down.
        for _ in 0..210 {
            s.step(1.0 / 60.0, 6.0);
            for (k, c) in s.water.iter().enumerate() {
                if c.h > 2e-3 {
                    top = top.max(s.bed[k]);
                }
            }
        }
        let rise = top - top0;
        // At least a ball thrown at the sheet's speed; at most one thrown at the speed of a
        // dam break's tip into dry land (Ritter: u + 2√(gh)).
        let ball = 1.5 * 1.5 / (2.0 * G);
        let tip = 1.5 + 2.0 * (G * 0.02f32).sqrt();
        let ritter = tip * tip / (2.0 * G);
        assert!(
            (0.9 * ball..ritter).contains(&rise),
            "climbed {rise} m, a ball {ball} m, a dam-break tip {ritter} m"
        );
        // And slid back down.
        let now = s
            .water
            .iter()
            .enumerate()
            .filter(|(_, c)| c.h > 2e-3)
            .map(|(k, _)| s.bed[k])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(now < top - 0.03, "still up at {now} (top {top})");
    }

    #[test]
    fn waves_from_the_strip_run_up_the_beach_and_drain() {
        // A 1:12 beach; the surface's waves (0.08 m, 2 s) arrive through the strip for 6 s.
        let (n, cell) = (128usize, 0.125f32);
        let mut s = SwashField::new(n, cell);
        s.lay(DVec2::ZERO, 0.0, &|p| -1.0 + p.x / 12.0, &|_| Some(0.0));
        let still_front = (0..n)
            .find(|&i| s.water[(n / 2) * n + i].h <= DRY_M)
            .unwrap();
        let mut highest = 0.0f32;
        let mut top = still_front;
        let mut t = 0.0f32;
        let k_wave = std::f32::consts::TAU / (2.0 * (G * 0.7).sqrt());
        for f in 0..1200 {
            let on = t < 6.0;
            for k in 0..s.water.len() {
                let x = s.cell_center(k % n, k / n).x as f32;
                let depth = (s.still[k] - s.bed[k]).max(0.05);
                let phase = k_wave * x - std::f32::consts::PI * t;
                let eta = if on { 0.08 * phase.cos() } else { 0.0 };
                s.target[k] = Target {
                    eta,
                    vel: Vec2::new(eta * (G / depth).sqrt(), 0.0),
                    foam: 0.0,
                };
            }
            s.step(1.0 / 60.0, 6.0);
            t += 1.0 / 60.0;
            let row = n / 2;
            let front = (0..n)
                .rev()
                .find(|&i| s.water[row * n + i].h > DRY_M)
                .unwrap_or(0);
            if front > still_front {
                highest = highest.max((front - still_front) as f32 * cell);
                top = top.max(front);
            }
            if f == 1199 {
                assert!(
                    front <= still_front + 2,
                    "the beach didn't drain: front {front}, still {still_front}"
                );
            }
        }
        assert!(highest >= 0.3, "waves ran up only {highest} m");
        // The sand remembers: dark just under the highest reach, the sheen gone.
        let row = n / 2;
        let k = row * n + top - 1;
        assert!(s.sand[k].damp > 0.5, "{:?}", s.sand[k]);
        assert!(s.sand[k].sheen < 0.2, "{:?}", s.sand[k]);
    }
}
