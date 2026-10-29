//! Interactive water: ripples, currents, swirls and foam around the focus (the player or
//! camera).
//!
//! Anything that touches the water pushes it here: swimmers and boats leave wakes and drag the
//! water along, so their wash curls into eddies behind them; footsteps and raindrops leave
//! rings; bullets and explosions throw splashes. Foam rides the currents, so a boat's trail
//! winds through the swirls it left.
//!
//! The surface is a shallow-water simulation on a staggered grid: a height per cell and a
//! current through each cell face. Ripples travel at `WaterSettings::ripple_speed`, slow down
//! in shallow water (√(g·h)) so they bend toward the shore, reflect off it, and fade. The
//! current carries itself and the foam along (semi-Lagrangian advection) and keeps its swirls
//! tight (vorticity confinement), like a 2D fluid solver. The height does the fluid solver's
//! pressure solve for free: water pushed together rises and runs off as waves, and whatever
//! stirring is left swirls.
//!
//! The grid follows the focus in whole cells and runs on the CPU in parallel rows of square
//! tiles, stepping only the tiles where something moves (and those next to them): calm water
//! costs nothing. The heights and currents feed buoyancy and the `Water` query (boats rock on
//! each other's wakes, floating things drift in them), and the renderer uploads them as
//! textures each frame.

use engine_core::math::{DVec2, DVec3, Vec2, Vec3};
use engine_core::tasks::ComputeTaskPool;
use engine_world::WorldShift;
use std::sync::Arc;

use crate::surface::{CoarseBed, SurfaceLink};

/// Cells at the grid edge where waves are absorbed instead of reflected.
const SPONGE: usize = 10;
/// Cells per side of a tile: the unit the grid wakes and sleeps in.
pub const TILE: usize = 16;
/// A tile whose heights (m) and currents (m/s) all stay under this is still.
const STILL: f32 = 2e-4;
/// A tile with currents faster than this (m/s) has more than waves moving its water (a ripple
/// 1 cm high already moves its water at ~0.08 m/s): the current is carried along there.
const CURRENT: f32 = 0.15;
/// A tile whose foam all stays under this has none.
const FOAM_STILL: f32 = 2e-3;
/// Fastest current the grid carries, m/s.
pub const MAX_CURRENT: f32 = 6.0;
/// Swirls turning slower than this (1/s) aren't kept tight: they fade (confinement would feed
/// the faint numerical curl everywhere and keep the water moving forever).
const SWIRL_FLOOR: f32 = 0.3;
/// Water turning faster than this (1/s) churns up foam, `CHURN_FOAM` per second for each 1/s
/// more (halved by the foam step's gain).
const CHURN_TURN: f32 = 1.5;
const CHURN_FOAM: f32 = 0.08;
/// How far a stirring body pulls the water at its middle toward its own velocity per push.
const STIR: f32 = 0.35;
const G: f32 = 9.81;

/// A push on the surface.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Disturbance {
    /// World x/z.
    pub pos: DVec2,
    /// Gaussian radius, meters.
    pub radius: f32,
    /// How far the surface is pushed down at the center, meters (negative lifts it).
    pub depth: f32,
    /// Foam left behind, 0..1.
    pub foam: f32,
    /// Drag the water inside the radius toward this velocity (world x/z, m/s): a hull or a
    /// swimmer carrying water along, a propeller's wash. `None` leaves the current alone.
    pub stir: Option<Vec2>,
    /// Set the water inside the radius turning about the center, rad/s (positive turns from
    /// +x toward +z): an eddy shed off a moving body, a paddle's twist, a whirlpool.
    pub spin: f32,
    /// Pressure head times the seconds it acts, m·s: a hull holding the water down as it
    /// passes (its draft over its step). The dispersive surface ([`crate::surface`]) feels it
    /// as a pressure on the water, so a hull at rest sits in a dip that deep and one under
    /// way draws its wake from it (Havelock's moving pressure patch, 1908), the lake neither
    /// rising nor sinking; it then ignores `depth`, which only the grid takes.
    pub press: f32,
}

/// The interactive surface at a point (see [`RippleField::sample`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RippleSample {
    /// Height above the undisturbed surface, meters.
    pub height: f32,
    /// (dh/dx, dh/dz).
    pub gradient: Vec2,
    /// Foam, 0..1.
    pub foam: f32,
    /// Current (world x/z), m/s.
    pub current: Vec2,
}

/// Ripple heights, currents and foam on a square grid.
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
    /// Current through each cell's +x face (`[0]`) and +z face (`[1]`), m/s.
    pub vel: Vec<[f32; 2]>,
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
    /// How fast currents (and the swirls in them) die out, 1/s. Waves also lose what
    /// `damping` in [`step`](Self::step) asks for beyond this.
    pub current_damping: f32,
    /// How hard swirls are kept tight against the blur of advection (vorticity confinement).
    pub swirl: f32,
    /// Tiles where something moved at the end of the last step (or a push landed since).
    moving: Vec<bool>,
    /// Tiles stepped in the last step.
    active: Vec<bool>,
    /// Tiles with foam on them.
    foamy: Vec<bool>,
    /// Tiles with currents faster than waves alone make, at the end of the last step.
    currents: Vec<bool>,
    /// Tiles where the current is carried along and curled (active tiles with currents in
    /// or next to them).
    stirred: Vec<bool>,
    /// Fastest current anywhere at the end of the last step, m/s (sets the substeps).
    max_current: f32,
    /// Largest of `speed2`.
    max_speed2: f32,
    /// Substeps run (every other one carries the currents along).
    substeps: u64,
    // Scratch, the same size as the grid. Outside active tiles every buffer holds zeros, so
    // a step only ever touches the tiles where something moves.
    h2: Vec<f32>,
    vel2: Vec<[f32; 2]>,
    foam2: Vec<f32>,
    /// The current carried along (stirred tiles, carrying substeps).
    adv: Vec<[f32; 2]>,
    /// How fast the water turns at each cell's +x+z corner, 1/s (stirred tiles).
    curl: Vec<f32>,
    curl2: Vec<f32>,
    /// Vorticity confinement per cell (x, z), m/s².
    force: Vec<[f32; 2]>,
    /// Depth that carries waves through each cell's +x and +z face (c²/g between the two
    /// cells, m), 0 where the face is shut (dry land, the grid's edge).
    faces: Vec<[f32; 2]>,
    /// How much of each step's change a cell keeps: 0 on dry land, under 1 in the sponge.
    wet: Vec<f32>,
    /// Per tile, from the last step: the largest height or current, the largest current,
    /// the most foam.
    stats: Vec<[f32; 3]>,
    /// Per tile, the most foam a step ago (what `foam2` holds).
    foam_was: Vec<f32>,
    /// The flow the ripples ride on per cell (a stream's current, world x/z, m/s): rings and
    /// foam drift downstream with it, so a push held in a stream trails a wake.
    drift: Vec<Vec2>,
    /// Tiles where the water flows (some cell drifts), and the ones drifted this step.
    drifting: Vec<bool>,
    drifted: Vec<bool>,
    /// Scratch for the drift (heights, foam), only ever read where just written.
    drift_h: Vec<f32>,
    drift_foam: Vec<f32>,
    /// The GPU's dispersive surface over this grid ([`crate::surface`]): it takes every push
    /// and, while it runs, its heights are the ones [`sample`](Self::sample) answers with.
    pub surface: Arc<SurfaceLink>,
    /// The water under the surface's coarse level, centered on this grid ([`CoarseBed`]);
    /// `None` until read, and for grids the surface doesn't run on.
    pub coarse: Option<Arc<CoarseBed>>,
}

/// Bilinear sample of one velocity component, `g` in that component's own grid coordinates
/// (clamped to the grid).
fn sample_component(vel: &[[f32; 2]], c: usize, n: usize, gx: f32, gz: f32) -> f32 {
    let m = (n - 1) as f32;
    let (gx, gz) = (gx.clamp(0.0, m), gz.clamp(0.0, m));
    let (i0, j0) = (gx as usize, gz as usize);
    let (i1, j1) = ((i0 + 1).min(n - 1), (j0 + 1).min(n - 1));
    let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
    let a = vel[j0 * n + i0][c] + (vel[j0 * n + i1][c] - vel[j0 * n + i0][c]) * fx;
    let b = vel[j1 * n + i0][c] + (vel[j1 * n + i1][c] - vel[j1 * n + i0][c]) * fx;
    a + (b - a) * fz
}

/// Bilinear sample of a cell-centered grid at grid coordinates (cell (i, j)'s center is at
/// (i, j)), clamped to the grid.
fn sample_cells(buf: &[f32], n: usize, gx: f32, gz: f32) -> f32 {
    let m = (n - 1) as f32;
    let (gx, gz) = (gx.clamp(0.0, m), gz.clamp(0.0, m));
    let (i0, j0) = (gx as usize, gz as usize);
    let (i1, j1) = ((i0 + 1).min(n - 1), (j0 + 1).min(n - 1));
    let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
    let a = buf[j0 * n + i0] + (buf[j0 * n + i1] - buf[j0 * n + i0]) * fx;
    let b = buf[j1 * n + i0] + (buf[j1 * n + i1] - buf[j1 * n + i0]) * fx;
    a + (b - a) * fz
}

/// Velocity component `c` of face (i, j), 0 off the grid.
fn face(vel: &[[f32; 2]], n: usize, c: usize, i: isize, j: isize) -> f32 {
    if i < 0 || j < 0 || i >= n as isize || j >= n as isize {
        0.0
    } else {
        vel[j as usize * n + i as usize][c]
    }
}

/// Current at the center of cell (i, j): the mean of its two faces each way.
fn center_velocity(vel: &[[f32; 2]], n: usize, i: isize, j: isize) -> Vec2 {
    Vec2::new(
        0.5 * (face(vel, n, 0, i - 1, j) + face(vel, n, 0, i, j)),
        0.5 * (face(vel, n, 1, i, j - 1) + face(vel, n, 1, i, j)),
    )
}

/// How fast the water turns at the center of cell (i, j), 1/s (positive turns from +x toward
/// +z); a centered difference that falls back on the cell itself at the grid's edge.
fn vorticity_at(vel: &[[f32; 2]], n: usize, cell: f32, i: isize, j: isize) -> f32 {
    let c = |di: isize, dj: isize| {
        let (x, z) = (i + di, j + dj);
        if x < 0 || z < 0 || x >= n as isize || z >= n as isize {
            center_velocity(vel, n, i, j)
        } else {
            center_velocity(vel, n, x, z)
        }
    };
    ((c(1, 0).y - c(-1, 0).y) - (c(0, 1).x - c(0, -1).x)) * 0.5 / cell
}

/// How much of a step's change survives near the grid's edge (the sponge).
fn sponge(n: usize, i: usize, j: usize) -> f32 {
    let edge = i.min(j).min(n - 1 - i).min(n - 1 - j);
    if edge < SPONGE {
        0.9 + 0.1 * edge as f32 / SPONGE as f32
    } else {
        1.0
    }
}

/// How a step's foam fades and grows (see [`RippleField::step`]).
#[derive(Clone, Copy)]
struct Foam {
    /// What's left after the step.
    decay: f32,
    /// Foam per (m/s of surface speed over the breaking speed) per step.
    gain: f32,
    /// The step in cells per (m/s): how far the current carries the foam.
    k: f32,
}

/// What one substep's march reads (see [`RippleField::march`]).
struct March<'a> {
    n: usize,
    vel: &'a [[f32; 2]],
    adv: &'a [[f32; 2]],
    force: &'a [[f32; 2]],
    h: &'a [f32],
    faces: &'a [[f32; 2]],
    wet: &'a [f32],
    foam: &'a [f32],
    curl: &'a [f32],
    active: &'a [bool],
    stirred: &'a [bool],
    /// `adv` holds the carried current in the stirred tiles.
    carried: bool,
    dt: f32,
    /// What the currents and the heights keep of themselves through the substep (drag).
    keep: f32,
    keep_h: f32,
    g_dx: f32,
    k: f32,
    inv_dt: f32,
}

/// A cell's new currents through its +x and +z faces: `v` pushed by the confinement `f` and
/// down the slope of the surface (`h` here, `east` and `north` next door), kept `keep` of
/// (drag, the sponge), none through shut faces.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn face_currents(
    v: [f32; 2],
    f: [f32; 2],
    h: f32,
    east: f32,
    north: f32,
    open: [f32; 2],
    keep: f32,
    dt: f32,
    g_dx: f32,
) -> [f32; 2] {
    let u = ((v[0] + dt * (f[0] - g_dx * (east - h))) * keep).clamp(-MAX_CURRENT, MAX_CURRENT);
    let w = ((v[1] + dt * (f[1] - g_dx * (north - h))) * keep).clamp(-MAX_CURRENT, MAX_CURRENT);
    [
        if open[0] > 0.0 { u } else { 0.0 },
        if open[1] > 0.0 { w } else { 0.0 },
    ]
}

impl March<'_> {
    /// New currents through the faces of row `j`'s cells from `i0` on (`out`), in `tile`.
    fn currents(&self, tile: usize, j: usize, i0: usize, out: &mut [[f32; 2]]) {
        let n = self.n;
        let from = if self.carried && self.stirred[tile] {
            self.adv
        } else {
            self.vel
        };
        // The confinement sits on the corners: each face takes the mean of its two ends.
        let swirls = self.stirred[tile];
        let (force, h, last) = (self.force, self.h, n * n - 1);
        let (dt, g_dx, keep) = (self.dt, self.g_dx, self.keep);
        let len = out.len();
        let base = j * n + i0;
        if j > 0 && j + 1 < n && i0 > 0 {
            // Inside the grid: plain runs of each grid the compiler can vectorize.
            let r = base..base + len;
            let (v, hc, he, hn) = (
                &from[r.clone()],
                &h[r.clone()],
                &h[base + 1..],
                &h[base + n..],
            );
            let (wet, open) = (&self.wet[r.clone()], &self.faces[r.clone()]);
            let (he, hn) = (&he[..len], &hn[..len]);
            if swirls {
                let (fc, fb, fl) = (&force[r], &force[base - n..], &force[base - 1..]);
                let (fb, fl) = (&fb[..len], &fl[..len]);
                for o in 0..len {
                    let f = [0.5 * (fc[o][0] + fb[o][0]), 0.5 * (fc[o][1] + fl[o][1])];
                    out[o] = face_currents(
                        v[o],
                        f,
                        hc[o],
                        he[o],
                        hn[o],
                        open[o],
                        wet[o] * keep,
                        dt,
                        g_dx,
                    );
                }
            } else {
                for o in 0..len {
                    out[o] = face_currents(
                        v[o],
                        [0.0; 2],
                        hc[o],
                        he[o],
                        hn[o],
                        open[o],
                        wet[o] * keep,
                        dt,
                        g_dx,
                    );
                }
            }
            return;
        }
        for (o, out) in out.iter_mut().enumerate() {
            let idx = base + o;
            let (east, north) = ((idx + 1).min(last), (idx + n).min(last));
            let f = if swirls {
                let below = if j > 0 { force[idx - n][0] } else { 0.0 };
                let left = if i0 + o > 0 { force[idx - 1][1] } else { 0.0 };
                [0.5 * (force[idx][0] + below), 0.5 * (force[idx][1] + left)]
            } else {
                [0.0; 2]
            };
            let s = self.wet[idx] * keep;
            *out = face_currents(
                from[idx],
                f,
                h[idx],
                h[east],
                h[north],
                self.faces[idx],
                s,
                dt,
                g_dx,
            );
        }
    }

    /// New heights of row `j`'s cells from `i0` to `i1` at the grid's edge (`out` is the
    /// whole row): `row` and `prev` are the new currents of this row and the one below.
    fn heights_edge(
        &self,
        j: usize,
        i0: usize,
        i1: usize,
        row: &[[f32; 2]],
        prev: &[[f32; 2]],
        out: &mut [f32],
    ) {
        let n = self.n;
        for i in i0..i1 {
            let idx = j * n + i;
            let (f, v) = (self.faces[idx], row[i]);
            let mut div = f[0] * v[0] + f[1] * v[1];
            if i > 0 {
                div -= self.faces[idx - 1][0] * row[i - 1][0];
            }
            if j > 0 {
                div -= self.faces[idx - n][1] * prev[i][1];
            }
            out[i] = ((self.h[idx] - self.k * div) * self.keep_h * self.wet[idx]).clamp(-2.0, 2.0);
        }
    }

    /// March the tiles of row `ty` set in `mask` (see [`RippleField::march`]). The output
    /// slices are this row of tiles' part of each grid.
    #[allow(clippy::too_many_arguments)]
    fn tile_row(
        &self,
        ty: usize,
        mask: &[bool],
        foam: Option<Foam>,
        vel2: &mut [[f32; 2]],
        h2: &mut [f32],
        foam2: &mut [f32],
        stats: &mut [[f32; 3]],
    ) {
        let n = self.n;
        let t = n.div_ceil(TILE);
        let j0 = ty * TILE;
        // The new currents through the +z faces of the row below this row of tiles: the
        // heights of its first row need them (another task marches that row: work them out
        // again here).
        let mut below = vec![[0.0f32; 2]; n];
        if j0 > 0 {
            for (tile, i0, i1) in spans(n, self.active, ty - 1) {
                if self.active[tile + t] {
                    self.currents(tile, j0 - 1, i0, &mut below[i0..i1]);
                }
            }
        }
        for j in j0..(j0 + TILE).min(n) {
            let r = (j - j0) * n;
            for (tile, i0, i1) in spans(n, self.active, ty) {
                self.currents(tile, j, i0, &mut vel2[r + i0..r + i1]);
            }
            let (done, _) = vel2.split_at(r + n);
            let (prev, row) = if j == j0 {
                (&below[..], &done[r..])
            } else {
                done[r - n..].split_at(n)
            };
            for (tx, stat) in stats.iter_mut().enumerate().take(t) {
                let tile = ty * t + tx;
                if !mask[tile] {
                    continue;
                }
                let (i0, i1) = (tx * TILE, ((tx + 1) * TILE).min(n));
                if self.active[tile] {
                    // Heights: water piles up where currents meet and drops where they part,
                    // carried by each face's depth (c²/g: ripples run at the ripple speed,
                    // slower in the shallows). `wet` is 0 on dry land (which stays flat) and
                    // fades in the sponge.
                    if j > 0 && i0 > 0 {
                        let (b, len) = (j * n + i0, i1 - i0);
                        let (fc, fl, fb) = (
                            &self.faces[b..b + len],
                            &self.faces[b - 1..b - 1 + len],
                            &self.faces[b - n..b - n + len],
                        );
                        let (vc, vl, vb) = (&row[i0..i1], &row[i0 - 1..i1 - 1], &prev[i0..i1]);
                        let (hc, wet) = (&self.h[b..b + len], &self.wet[b..b + len]);
                        let out = &mut h2[r + i0..r + i1];
                        let (k, keep) = (self.k, self.keep_h);
                        for o in 0..len {
                            let div = fc[o][0] * vc[o][0] + fc[o][1] * vc[o][1]
                                - fl[o][0] * vl[o][0]
                                - fb[o][1] * vb[o][1];
                            out[o] = ((hc[o] - k * div) * keep * wet[o]).clamp(-2.0, 2.0);
                        }
                    } else {
                        self.heights_edge(j, i0, i1, row, prev, &mut h2[r..r + n]);
                    }
                }
                let Some(fm) = foam else {
                    continue;
                };
                // Foam rides the current, fades, and grows where the surface moves fast
                // (breaking ripples, splashes) and where the water churns (tight swirls).
                let carried = self.stirred[tile];
                let b = j * n + i0;
                let len = i1 - i0;
                if !carried {
                    let (f, wet, hc) = (
                        &self.foam[b..b + len],
                        &self.wet[b..b + len],
                        &self.h[b..b + len],
                    );
                    let out = &mut foam2[r + i0..r + i1];
                    let moved = &h2[r + i0..r + i1];
                    let inv = if self.active[tile] { self.inv_dt } else { 0.0 };
                    let mut most = stat[2];
                    for o in 0..len {
                        let speed = (moved[o] - hc[o]).abs() * inv;
                        let v = (f[o] * fm.decay + fm.gain * (speed - 0.35).max(0.0)).min(1.0);
                        let v = if wet[o] > 0.0 { v } else { 0.0 };
                        out[o] = v;
                        most = most.max(v);
                    }
                    stat[2] = most;
                }
                if carried {
                    for i in i0..i1 {
                        let idx = j * n + i;
                        if self.wet[idx] <= 0.0 {
                            foam2[r + i] = 0.0;
                            continue;
                        }
                        let mut f = self.foam[idx];
                        let mut churn = 0.0;
                        if carried {
                            let u = 0.5 * (row[i][0] + if i > 0 { row[i - 1][0] } else { 0.0 });
                            let w = 0.5 * (row[i][1] + prev[i][1]);
                            f = sample_cells(
                                self.foam,
                                n,
                                i as f32 - u * fm.k,
                                j as f32 - w * fm.k,
                            );
                            churn = CHURN_FOAM * (self.curl[idx].abs() - CHURN_TURN).max(0.0);
                        }
                        let speed = if self.active[tile] {
                            (h2[r + i] - self.h[idx]).abs() * self.inv_dt
                        } else {
                            0.0
                        };
                        let out =
                            (f * fm.decay + fm.gain * ((speed - 0.35).max(0.0) + churn)).min(1.0);
                        foam2[r + i] = out;
                        stat[2] = stat[2].max(out);
                    }
                }
                if self.active[tile] {
                    let (mut most, mut fastest) = (stat[0], stat[1]);
                    for (v, h) in row[i0..i1].iter().zip(&h2[r + i0..r + i1]) {
                        let v = v[0].abs().max(v[1].abs());
                        most = most.max(h.abs());
                        fastest = fastest.max(v);
                    }
                    stat[0] = most.max(fastest);
                    stat[1] = fastest;
                }
            }
        }
    }
}

/// The tiles set in `mask` along row of tiles `ty`: (tile, first column, end column).
fn spans(n: usize, mask: &[bool], ty: usize) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let t = n.div_ceil(TILE);
    (0..t)
        .map(move |tx| (ty * t + tx, tx * TILE, ((tx + 1) * TILE).min(n)))
        .filter(move |(tile, _, _)| mask[*tile])
}

/// Below this many cells to work on a pass runs on the calling thread: waking the pool
/// costs more than the work.
const SERIAL_CELLS: usize = 8 * TILE * TILE;

/// Run `f(ty, row)` for each row of tiles with any tile set in `mask` (`rows` yields every
/// row of tiles' part of the output grids, in order), shared out evenly over the compute pool:
/// one task per thread, rows of about the same work each.
fn par_tile_rows<C: Send>(
    n: usize,
    mask: &[bool],
    rows: impl Iterator<Item = C>,
    f: impl Fn(usize, C) + Send + Sync,
) {
    let t = n.div_ceil(TILE);
    let work: Vec<usize> = mask
        .chunks(t)
        .map(|r| r.iter().filter(|a| **a).count())
        .collect();
    let total: usize = work.iter().sum();
    if total == 0 {
        return;
    }
    let rows = rows.enumerate().filter(|(ty, _)| work[*ty] > 0);
    let threads = ComputeTaskPool::get().thread_num().max(1);
    if total * TILE * TILE < SERIAL_CELLS || threads == 1 {
        for (ty, row) in rows {
            f(ty, row);
        }
        return;
    }
    let share = total.div_ceil(threads);
    let mut groups: Vec<Vec<(usize, C)>> = vec![Vec::new()];
    let mut filled = 0;
    for (ty, row) in rows {
        if filled >= share {
            groups.push(Vec::new());
            filled = 0;
        }
        filled += work[ty];
        groups.last_mut().unwrap().push((ty, row));
    }
    let f = &f;
    ComputeTaskPool::get().scope(|scope| {
        for group in groups {
            scope.spawn(async move {
                for (ty, row) in group {
                    f(ty, row);
                }
            });
        }
    });
}

/// Is any of the tiles round (tx, ty) (itself included) set in `flags`?
fn near(flags: &[bool], t: usize, tx: usize, ty: usize) -> bool {
    (ty.saturating_sub(1)..=(ty + 1).min(t - 1))
        .any(|y| (tx.saturating_sub(1)..=(tx + 1).min(t - 1)).any(|x| flags[y * t + x]))
}

impl RippleField {
    pub fn new(n: usize, cell: f32) -> Self {
        // The step runs on the compute pool; make sure it exists (no-op in a running app).
        engine_core::tasks::init_task_pools();
        let n = n.max(8);
        let tiles = n.div_ceil(TILE).pow(2);
        Self {
            n,
            cell: cell.max(0.01),
            origin: DVec2::new(
                -(n as f64) * cell as f64 * 0.5,
                -(n as f64) * cell as f64 * 0.5,
            ),
            h: vec![0.0; n * n],
            vel: vec![[0.0; 2]; n * n],
            foam: vec![0.0; n * n],
            speed2: vec![0.0; n * n],
            pending: Vec::new(),
            version: 0,
            has_water: false,
            current_damping: 0.12,
            swirl: 0.5,
            moving: vec![false; tiles],
            active: vec![false; tiles],
            foamy: vec![false; tiles],
            currents: vec![false; tiles],
            stirred: vec![false; tiles],
            max_current: 0.0,
            max_speed2: 0.0,
            substeps: 0,
            h2: vec![0.0; n * n],
            vel2: vec![[0.0; 2]; n * n],
            foam2: vec![0.0; n * n],
            adv: vec![[0.0; 2]; n * n],
            curl: vec![0.0; n * n],
            curl2: vec![0.0; n * n],
            force: vec![[0.0; 2]; n * n],
            faces: vec![[0.0; 2]; n * n],
            wet: vec![0.0; n * n],
            stats: vec![[0.0; 3]; tiles],
            foam_was: vec![0.0; tiles],
            drift: vec![Vec2::ZERO; n * n],
            drifting: vec![false; tiles],
            drifted: vec![false; tiles],
            drift_h: vec![0.0; n * n],
            drift_foam: vec![0.0; n * n],
            surface: Arc::default(),
            coarse: None,
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

    fn tiles(&self) -> usize {
        self.n.div_ceil(TILE)
    }

    /// Tiles stepped in the last step (diagnostics: what the water costs).
    pub fn active_tiles(&self) -> usize {
        self.active.iter().filter(|a| **a).count()
    }

    /// Tiles where currents were carried along and curled in the last step (diagnostics).
    pub fn stirred_tiles(&self) -> usize {
        self.stirred.iter().filter(|s| **s).count()
    }

    /// Tiles in the grid.
    pub fn tile_count(&self) -> usize {
        self.active.len()
    }

    /// Step every tile next time: call after writing to `h`, `vel` or `foam` directly.
    pub fn wake_all(&mut self) {
        self.moving.fill(true);
        self.foamy.fill(true);
        self.currents.fill(true);
        self.max_current = MAX_CURRENT;
        self.h2.fill(0.0);
        self.vel2.fill([0.0; 2]);
        self.foam2.fill(0.0);
        self.curl.fill(0.0);
        self.curl2.fill(0.0);
        self.force.fill([0.0; 2]);
        self.version += 1;
    }

    fn clear(&mut self) {
        self.h.fill(0.0);
        self.vel.fill([0.0; 2]);
        self.foam.fill(0.0);
        self.h2.fill(0.0);
        self.vel2.fill([0.0; 2]);
        self.foam2.fill(0.0);
        self.curl.fill(0.0);
        self.curl2.fill(0.0);
        self.force.fill([0.0; 2]);
        self.moving.fill(false);
        self.active.fill(false);
        self.foamy.fill(false);
        self.currents.fill(false);
        self.stirred.fill(false);
        self.max_current = 0.0;
    }

    /// Whether [`RippleField::recenter`] on `focus` would move the grid.
    pub fn would_recenter(&self, focus: DVec2) -> bool {
        let half = self.size() * 0.5;
        let d = focus - (self.origin + DVec2::splat(half));
        d.x.abs() >= half * 0.25 || d.y.abs() >= half * 0.25
    }

    /// Follow the focus: shift the grid by whole cells when it drifts from the center.
    /// Returns true when the grid moved (wave speeds must then be rebuilt).
    pub fn recenter(&mut self, focus: DVec2) -> bool {
        if !self.would_recenter(focus) {
            return false;
        }
        let cell = self.cell as f64;
        let half = self.size() * 0.5;
        let target = ((focus - DVec2::splat(half)) / cell).round() * cell;
        let shift = ((target - self.origin) / cell).round();
        let (sx, sz) = (shift.x as i64, shift.y as i64);
        self.origin = target;
        let n = self.n as i64;
        if sx.abs() >= n || sz.abs() >= n {
            self.clear();
        } else {
            fn moved<T: Copy + Default>(buf: &mut [T], n: i64, sx: i64, sz: i64) {
                let old = buf.to_vec();
                for j in 0..n {
                    for i in 0..n {
                        let (oi, oj) = (i + sx, j + sz);
                        buf[(j * n + i) as usize] = if oi >= 0 && oi < n && oj >= 0 && oj < n {
                            old[(oj * n + oi) as usize]
                        } else {
                            T::default()
                        };
                    }
                }
            }
            moved(&mut self.h, n, sx, sz);
            moved(&mut self.vel, n, sx, sz);
            moved(&mut self.foam, n, sx, sz);
            self.wake_all();
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
                        let shallow = G * d;
                        (speed * speed).min(shallow)
                    }
                    _ => 0.0,
                };
                self.speed2[j * n + i] = c2;
            }
        }
        self.has_water = any;
        self.speeds_changed();
        if !any {
            self.clear();
        }
    }

    /// Set the flow the ripples ride on at every cell (`None` = still water): a stream's
    /// current carries rings and foam downstream.
    pub fn rebuild_drift(&mut self, drift_at: impl Fn(DVec2) -> Option<Vec2>) {
        let (n, t) = (self.n, self.tiles());
        self.drifting.fill(false);
        for j in 0..n {
            for i in 0..n {
                let idx = j * n + i;
                let v = if self.speed2[idx] > 0.0 {
                    drift_at(self.cell_center(i, j)).unwrap_or(Vec2::ZERO)
                } else {
                    Vec2::ZERO
                };
                // (Clamped: a torrent's ripples are lost in its own churn anyway.)
                self.drift[idx] = v.clamp_length_max(6.0);
                if v.length_squared() > 0.05 * 0.05 {
                    self.drifting[(j / TILE) * t + i / TILE] = true;
                }
            }
        }
    }

    /// Make this a copy of what readers of `other` see (heights, currents, foam and where
    /// the grid is), without its working buffers: cheap enough to publish every step.
    pub fn copy_view_from(&mut self, other: &RippleField) {
        self.n = other.n;
        self.cell = other.cell;
        self.origin = other.origin;
        self.version = other.version;
        self.has_water = other.has_water;
        self.h.clone_from(&other.h);
        self.vel.clone_from(&other.vel);
        self.foam.clone_from(&other.foam);
        self.surface = other.surface.clone();
        self.coarse.clone_from(&other.coarse);
    }

    /// Derive what the step needs from `speed2`: the open faces' depths, the wet mask and the
    /// fastest wave.
    fn speeds_changed(&mut self) {
        let n = self.n;
        self.max_speed2 = self.speed2.iter().copied().fold(0.0, f32::max);
        for j in 0..n {
            for i in 0..n {
                let idx = j * n + i;
                let c2 = self.speed2[idx];
                let depth = |other: Option<usize>| match other {
                    Some(o) if c2 > 0.0 && self.speed2[o] > 0.0 => 0.5 * (c2 + self.speed2[o]) / G,
                    _ => 0.0,
                };
                self.faces[idx] = [
                    depth((i + 1 < n).then_some(idx + 1)),
                    depth((j + 1 < n).then_some(idx + n)),
                ];
                self.wet[idx] = if c2 > 0.0 { sponge(n, i, j) } else { 0.0 };
            }
        }
    }

    /// Is there anything for [`step`](Self::step) to do: pushes waiting, or water still
    /// moving or foaming somewhere?
    pub fn needs_step(&self) -> bool {
        self.has_water
            && (!self.pending.is_empty()
                || self.moving.iter().any(|m| *m)
                || self.foamy.iter().any(|f| *f))
    }

    /// Queue a push on the surface.
    pub fn disturb(&mut self, d: Disturbance) {
        if self.pending.len() < 4096 {
            self.pending.push(d);
        }
    }

    fn apply_pending(&mut self) {
        let n = self.n as i64;
        let tiles = self.tiles();
        let cell = self.cell as f64;
        for d in std::mem::take(&mut self.pending) {
            self.surface.record(&d);
            let r = d.radius.max(self.cell * 0.75);
            let reach = (r * 2.0 / self.cell).ceil() as i64;
            let c = ((d.pos - self.origin) / cell).floor();
            let (ci, cj) = (c.x as i64, c.y as i64);
            let (i0, i1) = ((ci - reach).max(0), (ci + reach).min(n - 1));
            let (j0, j1) = ((cj - reach).max(0), (cj + reach).min(n - 1));
            if i0 > i1 || j0 > j1 {
                continue;
            }
            let swirls = d.stir.is_some() || d.spin != 0.0;
            for ty in j0 as usize / TILE..=j1 as usize / TILE {
                for tx in i0 as usize / TILE..=i1 as usize / TILE {
                    self.moving[ty * tiles + tx] = true;
                    self.foamy[ty * tiles + tx] |= d.foam > 0.0;
                    self.currents[ty * tiles + tx] |= swirls;
                }
            }
            let fastest = d.stir.map_or(0.0, |v| v.length()) + (d.spin * r).abs() * 0.5;
            self.max_current = self.max_current.max(fastest.min(MAX_CURRENT));
            for j in j0..=j1 {
                for i in i0..=i1 {
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
                    self.foam[idx] = (self.foam[idx] + d.foam * g).min(1.0);
                    if let Some(s) = d.stir {
                        let k = STIR * g;
                        let v = &mut self.vel[idx];
                        v[0] += (s.x - v[0]) * k;
                        v[1] += (s.y - v[1]) * k;
                    }
                    if d.spin != 0.0 {
                        // Each face turns with the eddy at its own place (half a cell past
                        // the center along its axis).
                        let half = 0.5 * self.cell as f64;
                        let ex = p + DVec2::new(half, 0.0) - d.pos;
                        let ez = p + DVec2::new(0.0, half) - d.pos;
                        let v = &mut self.vel[idx];
                        v[0] += -d.spin * ex.y as f32 * g;
                        v[1] += d.spin * ez.x as f32 * g;
                    }
                }
            }
        }
    }

    /// Advance by `dt` seconds. Waves die out at `damping` per second (currents at
    /// [`current_damping`](Self::current_damping)); foam fades to a third in `foam_life` seconds.
    pub fn step(&mut self, dt: f32, damping: f32, foam_life: f32) {
        if !self.has_water || dt <= 0.0 {
            self.pending.clear();
            return;
        }
        self.apply_pending();
        // Step the tiles where something moves and their neighbors (waves run into them);
        // carry currents along and curl them only where there are some.
        let t = self.tiles();
        for ty in 0..t {
            for tx in 0..t {
                let k = ty * t + tx;
                self.active[k] = near(&self.moving, t, tx, ty);
                let stirred = self.active[k] && near(&self.currents, t, tx, ty);
                if self.stirred[k] && !stirred {
                    // Leaving: the swirl buffers hold zeros outside stirred tiles.
                    for j in ty * TILE..((ty + 1) * TILE).min(self.n) {
                        let row =
                            j * self.n + tx * TILE..j * self.n + ((tx + 1) * TILE).min(self.n);
                        self.curl[row.clone()].fill(0.0);
                        self.curl2[row.clone()].fill(0.0);
                        self.force[row].fill([0.0; 2]);
                    }
                }
                self.stirred[k] = stirred;
            }
        }
        let any_active = self.active.iter().any(|a| *a);
        if !any_active && !self.foamy.iter().any(|f| *f) {
            return;
        }
        if any_active {
            self.drift_along(dt);
        }
        let foam = Foam {
            decay: (-dt / foam_life.max(0.05)).exp(),
            gain: dt * 2.0,
            k: dt / self.cell,
        };
        self.stats.fill([0.0; 3]);
        if any_active {
            let courant = (self.max_speed2.sqrt() + self.max_current) * dt / self.cell;
            let substeps = ((courant / 0.5).ceil() as u32).clamp(1, 8);
            let sdt = dt / substeps as f32;
            let k_u = self
                .current_damping
                .clamp(0.0, damping.max(self.current_damping));
            let k_h = (damping - k_u).max(0.0);
            for s in 1..=substeps {
                // Currents turn over far slower than waves run: carry and curl them every
                // other substep, two substeps' worth at a time (the confinement pushes on in
                // between).
                self.substeps += 1;
                let carry = self.substeps.is_multiple_of(2) && self.stirred.iter().any(|s| *s);
                if carry {
                    self.carry(2.0 * sdt);
                }
                self.march(sdt, k_u, k_h, carry, (s == substeps).then_some(foam));
            }
        } else {
            self.march(dt, 0.0, 0.0, false, Some(foam));
        }
        self.settle();
        self.version += 1;
    }

    /// Carry the heights and foam of the active tiles where the water flows downstream with
    /// it (tracing each cell back along the flow), so rings drift and a push held in the
    /// flow trails a wake. Dry cells stay flat.
    fn drift_along(&mut self, dt: f32) {
        let n = self.n;
        for (d, (a, f)) in self
            .drifted
            .iter_mut()
            .zip(self.active.iter().zip(&self.drifting))
        {
            *d = *a && *f;
        }
        if !self.drifted.iter().any(|d| *d) {
            return;
        }
        let k = dt / self.cell;
        let (h, foam, drift, speed2, mask) = (
            &self.h,
            &self.foam,
            &self.drift,
            &self.speed2,
            &self.drifted,
        );
        let bilinear = |buf: &[f32], x: f32, z: f32| {
            let m = (n - 1) as f32;
            let (x, z) = (x.clamp(0.0, m), z.clamp(0.0, m));
            let (i0, j0) = (x as usize, z as usize);
            let (i1, j1) = ((i0 + 1).min(n - 1), (j0 + 1).min(n - 1));
            let (fx, fz) = (x - i0 as f32, z - j0 as f32);
            let a = buf[j0 * n + i0] + (buf[j0 * n + i1] - buf[j0 * n + i0]) * fx;
            let b = buf[j1 * n + i0] + (buf[j1 * n + i1] - buf[j1 * n + i0]) * fx;
            a + (b - a) * fz
        };
        let rows = self
            .drift_h
            .chunks_mut(TILE * n)
            .zip(self.drift_foam.chunks_mut(TILE * n));
        par_tile_rows(n, mask, rows, |ty, (out_h, out_foam)| {
            for (_, i0, i1) in spans(n, mask, ty) {
                for j in ty * TILE..((ty + 1) * TILE).min(n) {
                    let r = (j - ty * TILE) * n;
                    for i in i0..i1 {
                        let idx = j * n + i;
                        let v = drift[idx];
                        if speed2[idx] <= 0.0 || v == Vec2::ZERO {
                            out_h[r + i] = h[idx];
                            out_foam[r + i] = foam[idx];
                            continue;
                        }
                        let (x, z) = (i as f32 - v.x * k, j as f32 - v.y * k);
                        out_h[r + i] = bilinear(h, x, z);
                        out_foam[r + i] = bilinear(foam, x, z);
                    }
                }
            }
        });
        let t = self.tiles();
        for ty in 0..t {
            for (_, i0, i1) in spans(n, &self.drifted, ty) {
                for j in ty * TILE..((ty + 1) * TILE).min(n) {
                    let row = j * n + i0..j * n + i1;
                    self.h[row.clone()].copy_from_slice(&self.drift_h[row.clone()]);
                    self.foam[row.clone()].copy_from_slice(&self.drift_foam[row]);
                }
            }
        }
    }

    /// Carry the current along in the stirred tiles (into `adv`, for the next march), and
    /// measure how it turns (`curl`, at each cell's +x+z corner) and the confinement that keeps
    /// its swirls tight (`force`, from the last measure: a substep or two behind, which the
    /// swirls don't notice).
    fn carry(&mut self, dt: f32) {
        let n = self.n;
        let k = dt / self.cell;
        let inv = 1.0 / self.cell;
        let swirl = self.swirl * self.cell;
        let (vel, curl, faces, stirred) = (&self.vel, &self.curl, &self.faces, &self.stirred);
        let at = |i: isize, j: isize| {
            if i < 0 || j < 0 || i >= n as isize || j >= n as isize {
                0.0
            } else {
                curl[j as usize * n + i as usize]
            }
        };
        let rows = self
            .adv
            .chunks_mut(TILE * n)
            .zip(self.curl2.chunks_mut(TILE * n))
            .zip(self.force.chunks_mut(TILE * n));
        par_tile_rows(n, stirred, rows, |ty, ((adv, curl2), force)| {
            for (_, i0, i1) in spans(n, stirred, ty) {
                for j in ty * TILE..((ty + 1) * TILE).min(n) {
                    let (jj, r) = (j as isize, (j - ty * TILE) * n);
                    for i in i0..i1 {
                        let (idx, ii) = (j * n + i, i as isize);
                        // The current carries itself along: trace each face back along the
                        // flow.
                        let mut out = [0.0; 2];
                        let inside = i > 0 && j > 0 && i + 1 < n && j + 1 < n;
                        if faces[idx][0] > 0.0 {
                            let u = vel[idx][0];
                            let w = 0.25
                                * if inside {
                                    vel[idx][1]
                                        + vel[idx + 1][1]
                                        + vel[idx - n][1]
                                        + vel[idx + 1 - n][1]
                                } else {
                                    face(vel, n, 1, ii, jj)
                                        + face(vel, n, 1, ii + 1, jj)
                                        + face(vel, n, 1, ii, jj - 1)
                                        + face(vel, n, 1, ii + 1, jj - 1)
                                };
                            let (x, z) = (i as f32 + 1.0 - u * k, j as f32 + 0.5 - w * k);
                            out[0] = sample_component(vel, 0, n, x - 1.0, z - 0.5);
                        }
                        if faces[idx][1] > 0.0 {
                            let w = vel[idx][1];
                            let u = 0.25
                                * if inside {
                                    vel[idx][0]
                                        + vel[idx - 1][0]
                                        + vel[idx + n][0]
                                        + vel[idx - 1 + n][0]
                                } else {
                                    face(vel, n, 0, ii, jj)
                                        + face(vel, n, 0, ii - 1, jj)
                                        + face(vel, n, 0, ii, jj + 1)
                                        + face(vel, n, 0, ii - 1, jj + 1)
                                };
                            let (x, z) = (i as f32 + 0.5 - u * k, j as f32 + 1.0 - w * k);
                            out[1] = sample_component(vel, 1, n, x - 0.5, z - 1.0);
                        }
                        adv[r + i] = out;
                        // How fast it turns at the +x+z corner, from the four faces round it.
                        curl2[r + i] = if i + 1 < n && j + 1 < n {
                            ((vel[idx + 1][1] - vel[idx][1]) - (vel[idx + n][0] - vel[idx][0]))
                                * inv
                        } else {
                            0.0
                        };
                        // Vorticity confinement: push along the swirl, toward where it turns
                        // hardest, so advection's blur doesn't smear it out.
                        let w = curl[idx];
                        let strong = ((w.abs() - SWIRL_FLOOR) / SWIRL_FLOOR).clamp(0.0, 1.0);
                        force[r + i] = [0.0; 2];
                        if swirl > 0.0 && strong > 0.0 {
                            let grad = if inside {
                                Vec2::new(
                                    curl[idx + 1].abs() - curl[idx - 1].abs(),
                                    curl[idx + n].abs() - curl[idx - n].abs(),
                                )
                            } else {
                                Vec2::new(
                                    at(ii + 1, jj).abs() - at(ii - 1, jj).abs(),
                                    at(ii, jj + 1).abs() - at(ii, jj - 1).abs(),
                                )
                            };
                            let len = grad.length();
                            if len > 1e-6 {
                                let f = Vec2::new(grad.y, -grad.x) * (w * swirl * strong / len);
                                force[r + i] = [f.x, f.y];
                            }
                        }
                    }
                }
            }
        });
        std::mem::swap(&mut self.curl, &mut self.curl2);
    }

    /// One substep of the active tiles, in one pass over each row of tiles: the currents
    /// (water runs down the slope of the surface, the swirls are kept tight, drag), then the
    /// heights they move, then (with `foam`, on a step's last substep) the foam and the
    /// tiles' stats for [`settle`](Self::settle). Tiles that only foam just fade.
    fn march(&mut self, dt: f32, k_u: f32, k_h: f32, carried: bool, foam: Option<Foam>) {
        let n = self.n;
        let t = self.tiles();
        let mask: Vec<bool> = match foam {
            Some(_) => self
                .active
                .iter()
                .zip(&self.foamy)
                .map(|(a, f)| *a || *f)
                .collect(),
            None => self.active.clone(),
        };
        let m = March {
            n,
            vel: &self.vel,
            adv: &self.adv,
            force: &self.force,
            h: &self.h,
            faces: &self.faces,
            wet: &self.wet,
            foam: &self.foam,
            curl: &self.curl,
            active: &self.active,
            stirred: &self.stirred,
            carried,
            dt,
            keep: (1.0 - k_u * dt).clamp(0.0, 1.0),
            keep_h: (1.0 - k_h * dt).clamp(0.0, 1.0),
            g_dx: G / self.cell,
            k: dt / self.cell,
            inv_dt: 1.0 / dt,
        };
        let rows = self
            .vel2
            .chunks_mut(TILE * n)
            .zip(self.h2.chunks_mut(TILE * n))
            .zip(self.foam2.chunks_mut(TILE * n))
            .zip(self.stats.chunks_mut(t));
        par_tile_rows(n, &mask, rows, |ty, (((vel2, h2), foam2), stats)| {
            m.tile_row(ty, &mask, foam, vel2, h2, foam2, stats);
        });
        if self.active.iter().any(|a| *a) {
            std::mem::swap(&mut self.vel, &mut self.vel2);
            std::mem::swap(&mut self.h, &mut self.h2);
        }
        if foam.is_some() {
            std::mem::swap(&mut self.foam, &mut self.foam2);
            self.foamy = mask;
        }
    }

    /// Put the tiles where nothing moves or foams any more to sleep (exact zeros), from the
    /// stats the last march took.
    fn settle(&mut self) {
        let n = self.n;
        let t = self.tiles();
        self.max_current = 0.0;
        for (k, s) in self.stats.iter().enumerate() {
            self.moving[k] = s[0] > STILL;
            self.currents[k] = s[1] > CURRENT;
            self.max_current = self.max_current.max(s[1]);
        }
        // Tiles that won't be stepped next time hold exact zeros, so nothing drifts there.
        for ty in 0..t {
            for tx in 0..t {
                let k = ty * t + tx;
                let calm = self.active[k] && !near(&self.moving, t, tx, ty);
                let dry = self.foamy[k] && self.stats[k][2] <= FOAM_STILL;
                if !calm && !dry {
                    continue;
                }
                for j in ty * TILE..((ty + 1) * TILE).min(n) {
                    let row = j * n + tx * TILE..j * n + ((tx + 1) * TILE).min(n);
                    if calm {
                        self.h[row.clone()].fill(0.0);
                        self.h2[row.clone()].fill(0.0);
                        self.vel[row.clone()].fill([0.0; 2]);
                        self.vel2[row.clone()].fill([0.0; 2]);
                        self.curl[row.clone()].fill(0.0);
                        self.curl2[row.clone()].fill(0.0);
                        self.force[row.clone()].fill([0.0; 2]);
                    }
                    // (Only what holds any: most foam fades from tiles that never had much.)
                    if dry && self.stats[k][2] > 0.0 {
                        self.foam[row.clone()].fill(0.0);
                    }
                    if dry && self.foam_was[k] > 0.0 {
                        self.foam2[row].fill(0.0);
                    }
                }
                if calm {
                    self.stirred[k] = false;
                }
                if dry {
                    self.foamy[k] = false;
                    self.stats[k][2] = 0.0;
                }
            }
        }
        // What `foam2` holds after the next march's swap.
        for (was, s) in self.foam_was.iter_mut().zip(&self.stats) {
            *was = s[2];
        }
    }

    /// The surface at world x/z, or `None` outside the grid.
    pub fn sample(&self, p: DVec2) -> Option<RippleSample> {
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
        let gx = ((at(&self.h, i + 1, j) - at(&self.h, i, j)) * (1.0 - fz)
            + (at(&self.h, i + 1, j + 1) - at(&self.h, i, j + 1)) * fz)
            / self.cell;
        let gz = ((at(&self.h, i, j + 1) - at(&self.h, i, j)) * (1.0 - fx)
            + (at(&self.h, i + 1, j + 1) - at(&self.h, i + 1, j)) * fx)
            / self.cell;
        // The GPU's dispersive surface, when it runs, is the one drawn: ride that.
        let (height, gradient) = self.surface.sample(p).unwrap_or((
            sample_cells(&self.h, n, g.x as f32, g.y as f32),
            Vec2::new(gx, gz),
        ));
        // Cell centers sit at whole grid coordinates + 0.5; faces half a cell further along.
        let (x, z) = (g.x as f32 + 0.5, g.y as f32 + 0.5);
        Some(RippleSample {
            height,
            gradient,
            foam: sample_cells(&self.foam, n, g.x as f32, g.y as f32),
            current: Vec2::new(
                sample_component(&self.vel, 0, n, x - 1.0, z - 0.5),
                sample_component(&self.vel, 1, n, x - 0.5, z - 1.0),
            ),
        })
    }

    /// How much of a step's change cell `k` (row-major) keeps: 0 on dry land, under 1 in the
    /// sponge at the grid's edge.
    pub fn cell_wet(&self, k: usize) -> f32 {
        self.wet[k]
    }

    /// The flow cell `k` (row-major) rides on: a stream's current, world x/z, m/s.
    pub fn cell_drift(&self, k: usize) -> Vec2 {
        self.drift[k]
    }

    /// Current at the center of cell (i, j), world x/z, m/s.
    pub fn cell_current(&self, i: usize, j: usize) -> Vec2 {
        center_velocity(&self.vel, self.n, i as isize, j as isize)
    }

    /// How fast the water turns at the center of cell (i, j), 1/s (positive turns from +x
    /// toward +z).
    pub fn vorticity(&self, i: usize, j: usize) -> f32 {
        vorticity_at(&self.vel, self.n, self.cell, i as isize, j as isize)
    }

    /// Follow a world shift. A plain move keeps every cell and moves the corner. A turn can't
    /// turn the grid (it stays square to the world axes): it is resampled around the same
    /// place, so ripples and currents already running carry on where they were.
    pub fn shift(&mut self, s: &WorldShift) {
        let to_xz = |p: DVec3| DVec2::new(p.x, p.z);
        let moved = |p: DVec2| to_xz(s.point(DVec3::new(p.x, 0.0, p.y)));
        let turned = |v: Vec2| {
            let r = s.direction(Vec3::new(v.x, 0.0, v.y));
            Vec2::new(r.x, r.z)
        };
        for d in &mut self.pending {
            d.pos = moved(d.pos);
            d.stir = d.stir.map(turned);
        }
        self.version += 1;
        // The coarse level's water moves with the world (a turn reads it again).
        if s.is_translation_only() {
            if let Some(bed) = &mut self.coarse {
                Arc::make_mut(bed).origin += to_xz(s.translation);
            }
        } else {
            self.coarse = None;
        }
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
                self.foam[k] = old.bilinear(&old.foam, at);
                // Faces take the turned current at their cell's center: close enough for
                // the rare turn of the world.
                let v = turned(old.sample(at).map_or(Vec2::ZERO, |s| s.current));
                self.vel[k] = [v.x, v.y];
                // Speeds follow the depth there; rebuilt properly on the next step.
                let g = ((at - old.origin) / old.cell as f64).floor();
                self.speed2[k] = if g.x >= 0.0 && g.y >= 0.0 && g.x < n as f64 && g.y < n as f64 {
                    old.speed2[g.y as usize * n + g.x as usize]
                } else {
                    0.0
                };
            }
        }
        self.speeds_changed();
        self.wake_all();
    }

    /// `buf` (one of the grids) at world x/z, bilinear; 0 outside.
    fn bilinear(&self, buf: &[f32], p: DVec2) -> f32 {
        let g = (p - self.origin) / self.cell as f64 - DVec2::splat(0.5);
        let n = self.n as f64;
        if g.x < 0.0 || g.y < 0.0 || g.x >= n - 1.0 || g.y >= n - 1.0 {
            return 0.0;
        }
        sample_cells(buf, self.n, g.x as f32, g.y as f32)
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

    fn drop_at(x: f64, z: f64, radius: f32, depth: f32, foam: f32) -> Disturbance {
        Disturbance {
            pos: DVec2::new(x, z),
            radius,
            depth,
            foam,
            stir: None,
            spin: 0.0,
            press: 0.0,
        }
    }

    #[test]
    fn a_drop_makes_a_ring_that_travels_at_the_ripple_speed() {
        let mut f = pond();
        f.disturb(drop_at(0.0, 0.0, 0.3, 0.05, 0.5));
        let dt = 1.0 / 60.0;
        for _ in 0..120 {
            f.step(dt, 0.0, 4.0);
        }
        // After 2 s the wavefront is ~2.6 m out: find the strongest disturbance radius.
        let mut best = (0.0f32, 0.0f32);
        for k in 0..60 {
            let r = k as f64 * 0.1;
            let h = f.sample(DVec2::new(r, 0.0)).unwrap().height;
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
        let a = f.sample(DVec2::new(0.0, best.0 as f64)).unwrap().height;
        let b = f.sample(DVec2::new(best.0 as f64, 0.0)).unwrap().height;
        assert!((a - b).abs() < 0.3 * b.abs().max(1e-4), "{a} vs {b}");
    }

    #[test]
    fn rings_and_foam_drift_down_a_stream() {
        // Where the foam's middle and the rings' strongest point lie along x after a second.
        let run = |flow: f32| {
            let mut f = pond();
            f.rebuild_drift(|_| Some(Vec2::new(flow, 0.0)));
            f.disturb(drop_at(0.0, 0.0, 0.3, 0.05, 0.8));
            for _ in 0..60 {
                f.step(1.0 / 60.0, 0.0, 6.0);
            }
            let (mut foam_x, mut foam_sum, mut up, mut down) = (0.0, 0.0, 0.0f32, 0.0f32);
            for k in -60..=60 {
                let x = k as f64 * 0.1;
                let s = f.sample(DVec2::new(x, 0.0)).unwrap();
                foam_x += x as f32 * s.foam;
                foam_sum += s.foam;
                if k < 0 {
                    up = up.max(s.height.abs());
                } else {
                    down = down.max(s.height.abs());
                }
            }
            (foam_x / foam_sum.max(1e-6), up, down)
        };
        let (still, ..) = run(0.0);
        let (carried, up, down) = run(1.5);
        assert!(still.abs() < 0.1, "still foam moved to {still} m");
        assert!(
            (carried - 1.5).abs() < 0.4,
            "a 1.5 m/s stream carried the foam {carried} m in a second"
        );
        assert!(
            up > 0.0 && down > up,
            "rings upstream {up}, downstream {down}"
        );
    }

    #[test]
    fn ripples_fade_and_stay_stable() {
        let mut f = pond();
        for k in 0..20 {
            f.disturb(drop_at(k as f64 * 0.5 - 5.0, 1.0, 0.5, 0.2, 1.0));
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
            stir: Some(Vec2::new(2.0, 0.0)),
            spin: 0.0,
            ..drop_at(0.0, 0.0, 0.4, 0.1, 0.0)
        });
        for _ in 0..200 {
            f.step(1.0 / 60.0, 0.1, 4.0);
            for j in 0..64 {
                for i in 0..64 {
                    if f.cell_center(i, j).x >= 2.0 {
                        assert_eq!(f.h[j * 64 + i], 0.0);
                        assert_eq!(f.vel[j * 64 + i], [0.0; 2]);
                    }
                }
            }
        }
    }

    #[test]
    fn recentering_keeps_waves_in_place() {
        let mut f = pond();
        f.disturb(Disturbance {
            stir: Some(Vec2::new(1.0, 0.5)),
            spin: 0.0,
            ..drop_at(3.0, 2.0, 0.5, 0.1, 0.0)
        });
        f.step(1.0 / 60.0, 0.0, 4.0);
        let before = f.sample(DVec2::new(3.0, 2.0)).unwrap();
        assert!(f.recenter(DVec2::new(9.0, 7.0)));
        f.rebuild_speeds(1.3, |_| Some(5.0));
        let after = f.sample(DVec2::new(3.0, 2.0)).unwrap();
        assert!(
            (before.height - after.height).abs() < 1e-6,
            "{before:?} vs {after:?}"
        );
        assert!((before.current - after.current).length() < 1e-6);
        assert!(!f.recenter(DVec2::new(9.5, 7.0)));
        // The moved waves keep running.
        f.step(1.0 / 60.0, 0.0, 4.0);
        assert!(f.active_tiles() > 0);
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

    /// Drag a stirring body along +x from x = -8 m at `speed` m/s for `secs`, then let the
    /// water run on for `after` seconds.
    fn drag(f: &mut RippleField, speed: f32, secs: f32, after: f32) {
        let dt = 1.0 / 60.0;
        let steps = (secs / dt) as usize;
        for s in 0..steps {
            let x = -8.0 + (s as f32 * dt * speed) as f64;
            f.disturb(Disturbance {
                stir: Some(Vec2::new(speed, 0.0)),
                spin: 0.0,
                ..drop_at(x, 0.0, 0.6, 0.02, 0.3)
            });
            f.step(dt, 0.35, 4.0);
        }
        for _ in 0..(after / dt) as usize {
            f.step(dt, 0.35, 4.0);
        }
    }

    /// Cell (i, j) holding world x/z.
    fn cell_at(f: &RippleField, x: f64, z: f64) -> (usize, usize) {
        let cell = f.cell as f64;
        (
            ((x - f.origin.x) / cell) as usize,
            ((z - f.origin.y) / cell) as usize,
        )
    }

    #[test]
    fn a_stirring_body_leaves_a_current_that_curls_into_eddies() {
        let mut f = pond();
        drag(&mut f, 2.0, 6.0, 3.0);
        // Three seconds after it stopped, the wash it dragged along still runs after it, and
        // the edges of the wash turn: eddies of opposite sense either side of its track.
        let along = f.sample(DVec2::new(0.0, 0.0)).unwrap().current;
        assert!(along.x > 0.05, "wash {along:?}");
        let turn = |z: f64| {
            (0..24)
                .map(|k| {
                    let (i, j) = cell_at(&f, -6.0 + k as f64 * 0.5, z);
                    f.vorticity(i, j)
                })
                .sum::<f32>()
        };
        // The wash runs fastest on the track: it turns from +x toward +z on its +z side.
        let (plus_z, minus_z) = (turn(0.6), turn(-0.6));
        assert!(
            plus_z > 0.5 && minus_z < -0.5,
            "eddies {plus_z} (+z side) {minus_z} (-z side)"
        );
        assert!(f.h.iter().chain(&f.foam).all(|v| v.is_finite()));
    }

    #[test]
    fn swirls_stay_tight_with_confinement() {
        let mut spun = pond();
        let mut blurred = pond();
        blurred.swirl = 0.0;
        drag(&mut spun, 2.0, 6.0, 4.0);
        drag(&mut blurred, 2.0, 6.0, 4.0);
        let peak = |f: &RippleField| {
            let mut m = 0.0f32;
            for j in 4..f.n - 4 {
                for i in 4..f.n - 4 {
                    m = m.max(f.vorticity(i, j).abs());
                }
            }
            m
        };
        let (a, b) = (peak(&spun), peak(&blurred));
        assert!(a > b * 1.15, "confined {a}, unconfined {b}");
    }

    #[test]
    fn foam_rides_the_current() {
        let mut f = pond();
        // A broad steady current toward +x, and a dab of foam in it.
        for v in f.vel.iter_mut() {
            v[0] = 1.0;
        }
        f.wake_all();
        f.current_damping = 0.0;
        f.disturb(drop_at(-4.0, 0.0, 0.5, 0.0, 1.0));
        for _ in 0..120 {
            f.step(1.0 / 60.0, 0.0, 100.0);
        }
        // (Round the dab only: water leaving the box's upwind wall foams there.)
        let (mut sum, mut w) = (0.0f64, 0.0f64);
        for j in 0..f.n {
            for i in 0..f.n {
                let c = f.cell_center(i, j);
                if c.x.abs() > 6.0 || c.y.abs() > 3.0 {
                    continue;
                }
                let m = f.foam[j * f.n + i] as f64;
                sum += c.x * m;
                w += m;
            }
        }
        // Two seconds at about 1 m/s.
        let centroid = sum / w;
        assert!((centroid + 2.0).abs() < 0.6, "foam at x = {centroid}");
    }

    #[test]
    fn calm_water_costs_nothing() {
        let mut f = pond();
        f.disturb(drop_at(0.0, 0.0, 0.4, 0.05, 0.5));
        f.step(1.0 / 60.0, 0.35, 1.0);
        let busy = f.active_tiles();
        assert!(busy > 0 && busy < f.tile_count() / 2, "{busy}");
        for _ in 0..60 * 60 {
            f.step(1.0 / 60.0, 0.35, 1.0);
        }
        assert_eq!(f.active_tiles(), 0);
        assert!(f.h.iter().all(|h| *h == 0.0));
        assert!(f.foam.iter().all(|x| *x == 0.0));
        let v = f.version;
        f.step(1.0 / 60.0, 0.35, 1.0);
        assert_eq!(f.version, v, "a still pond needs no upload");
    }
}
