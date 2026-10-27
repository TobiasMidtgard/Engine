//! Navigation: a walkable-height grid built lazily in chunks from the [`TacticalWorld`], the
//! `Ground` query and the `Water` query, plus A* pathfinding with string-pulled (smooth) paths.
//!
//! * Each cell stores the height someone standing there would be at: the ground, or the top of
//!   whatever they stand on (a platform, a ramp). Walls make tall cells that can't be stepped
//!   onto, so paths go around walls, up ramps and onto platforms with no extra setup.
//! * Cells next to walls are shrunk away by the agent radius; deep water is not walkable.
//! * Callers add their own per-cell cost (fire heat, enemy sight lines) when they search, so
//!   NPCs route around burning grass or through cover.
//! * Given the world's [`GroundWear`] ([`NavCtx::worn`]), walking over worn ground costs less
//!   (down to half on a well-worn path), so routes take the paths feet have worn and the
//!   paths wear deeper the more they're used.
//! * Chunks are rebuilt when the world model changes (colliders appear or move, terrain is
//!   edited), and only where someone asks for a path, so it scales to planet-sized maps.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, Mutex};

use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::{Ground, GroundWear, Water};

use crate::world_model::TacticalWorld;

/// Cells per chunk side.
const CHUNK: i32 = 32;

/// How much cheaper a well-worn path (wear 1) is to walk than untouched ground.
pub const WORN_DISCOUNT: f32 = 0.5;

/// Wear below this anywhere near a trip is treated as none (a few stray footprints).
const WORN_MIN: f32 = 0.05;

/// Hashes chunk coordinates with one multiply-xorshift per value (path searches look chunks up
/// thousands of times per step, and the default hasher is built for untrusted keys).
#[derive(Default)]
struct ChunkHasher(u64);

impl Hasher for ChunkHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    fn write_i32(&mut self, v: i32) {
        self.write_u64(v as u32 as u64);
    }

    fn write_u64(&mut self, v: u64) {
        let x = (self.0.rotate_left(26) ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = x ^ (x >> 29);
    }
}

type ChunkMap = HashMap<(i32, i32), Chunk, BuildHasherDefault<ChunkHasher>>;

/// Navigation settings (one grid for every agent of roughly human size).
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct NavSettings {
    /// Cell size, meters.
    pub cell: f32,
    /// Clearance kept from walls, meters.
    pub agent_radius: f32,
    /// Obstacles whose bottom is above this (lintels, branches) are walked under, meters.
    pub agent_height: f32,
    /// Highest step between neighboring cells, meters (stairs, curbs, ramps up to ~40°).
    pub max_step: f32,
    /// Water deeper than this is not walkable, meters.
    pub max_water_depth: f32,
    /// Largest search window side, cells (longer trips return a partial path to replan from).
    pub max_window: i32,
}

impl Default for NavSettings {
    fn default() -> Self {
        Self {
            cell: 0.5,
            agent_radius: 0.4,
            agent_height: 1.8,
            max_step: 0.4,
            max_water_depth: 0.9,
            max_window: 200,
        }
    }
}

struct Chunk {
    base: f64,
    /// Height above `base` per cell.
    h: Vec<f32>,
    walk: Vec<bool>,
}

/// The lazily built navigation grid. Use [`NavGrid::ctx`] to query it.
#[derive(Resource, Default)]
pub struct NavGrid {
    pub settings: NavSettings,
    chunks: ChunkMap,
    generation: u64,
    /// Chunks built since startup (diagnostics).
    pub chunks_built: u64,
    scratch: Vec<u32>,
    /// Ground heights of chunks being worked out off the main thread (`warm_nav`).
    ground_jobs: HashMap<(i32, i32), GroundJob>,
    /// Ground heights worked out off the main thread, waiting for their chunk to be built.
    ground_ready: HashMap<(i32, i32), Vec<f64>>,
}

/// A chunk's ground heights (its cells and margin, row by row), once worked out.
type GroundJob = Arc<Mutex<Option<Vec<f64>>>>;

impl std::fmt::Debug for NavGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavGrid")
            .field("settings", &self.settings)
            .field("chunks", &self.chunks.len())
            .finish()
    }
}

/// Outcome of a path search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStatus {
    /// The path reaches the goal.
    Complete,
    /// The goal is unreachable or too far: the path gets as close as possible.
    Partial,
    /// Not even the start is walkable.
    Failed,
}

/// A grid query bound to the world it reads.
pub struct NavCtx<'a> {
    pub grid: &'a mut NavGrid,
    pub world: &'a TacticalWorld,
    pub ground: &'a Ground,
    pub water: &'a Water,
    /// How worn the ground is: worn cells cost less to cross (see [`NavCtx::worn`]).
    pub wear: Option<&'a GroundWear>,
}

impl NavGrid {
    /// Bind the grid to the world for queries (drops stale chunks if the world changed).
    pub fn ctx<'a>(
        &'a mut self,
        world: &'a TacticalWorld,
        ground: &'a Ground,
        water: &'a Water,
    ) -> NavCtx<'a> {
        if self.generation != world.generation {
            // Drop only the chunks near what changed, when the world model knows.
            match world.changes_since(self.generation) {
                Some(changes) => {
                    let s = self.settings;
                    let pad = (s.agent_radius + 2.0 * s.cell) as f64;
                    let side = CHUNK as f64 * s.cell as f64;
                    for (min, max) in changes {
                        let (min, max) = (min - DVec2::splat(pad), max + DVec2::splat(pad));
                        self.chunks.retain(|&(kx, kz), _| {
                            let (x0, z0) = (kx as f64 * side, kz as f64 * side);
                            x0 > max.x || x0 + side < min.x || z0 > max.y || z0 + side < min.y
                        });
                    }
                }
                None => {
                    // Everything may have moved (a world shift): ground worked out ahead too.
                    self.chunks.clear();
                    self.ground_jobs.clear();
                    self.ground_ready.clear();
                }
            }
            self.generation = world.generation;
        }
        NavCtx {
            grid: self,
            world,
            ground,
            water,
            wear: None,
        }
    }

    /// Forget every chunk (call after big world edits the model doesn't see).
    pub fn clear(&mut self) {
        self.chunks.clear();
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Open {
    f: f32,
    idx: u32,
}
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .partial_cmp(&self.f)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.idx.cmp(&self.idx))
    }
}

/// A dense copy of part of the grid used by one search.
struct Window {
    x0: i32,
    z0: i32,
    w: i32,
    d: i32,
    h: Vec<f64>,
    walk: Vec<bool>,
}

impl Window {
    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        let (lx, lz) = (x - self.x0, z - self.z0);
        (lx >= 0 && lz >= 0 && lx < self.w && lz < self.d).then(|| (lz * self.w + lx) as usize)
    }

    fn connected(&self, a: usize, b: usize, step: f32, diagonal: bool) -> bool {
        let limit = if diagonal { step * 1.42 } else { step } as f64;
        self.walk[a] && self.walk[b] && (self.h[a] - self.h[b]).abs() <= limit
    }
}

impl<'a> NavCtx<'a> {
    /// Searches prefer worn ground: a step onto a cell costs `1 - WORN_DISCOUNT * wear` of
    /// its usual length (its extra cost is unchanged, so a worn path through fire or an
    /// enemy's sight line is no safer). Without wear (`None`, or none anywhere near the
    /// trip) paths are exactly as before.
    pub fn worn(mut self, wear: Option<&'a GroundWear>) -> Self {
        self.wear = wear;
        self
    }

    /// The highest wear on a coarse lattice (every other cell) over the cells a search from
    /// `a` to `b` with `margin` would cover; 0 without a wear source.
    fn wear_peak(&self, a: (i32, i32), b: (i32, i32), margin: i32, y: f64) -> f32 {
        let Some(wear) = self.wear else {
            return 0.0;
        };
        let max = self.grid.settings.max_window;
        let (x0, z0) = (a.0.min(b.0) - margin, a.1.min(b.1) - margin);
        let x1 = (a.0.max(b.0) + margin).min(x0 + max);
        let z1 = (a.1.max(b.1) + margin).min(z0 + max);
        let mut peak = 0.0f32;
        for z in (z0..=z1).step_by(2) {
            for x in (x0..=x1).step_by(2) {
                let (cx, cz) = self.cell_center(x, z);
                peak = peak.max(wear.wear(DVec3::new(cx, y, cz)));
                if peak >= 1.0 {
                    return 1.0;
                }
            }
        }
        peak
    }
    fn cell_of(&self, p: DVec3) -> (i32, i32) {
        let c = self.grid.settings.cell as f64;
        ((p.x / c).floor() as i32, (p.z / c).floor() as i32)
    }

    fn cell_center(&self, x: i32, z: i32) -> (f64, f64) {
        let c = self.grid.settings.cell as f64;
        ((x as f64 + 0.5) * c, (z as f64 + 0.5) * c)
    }

    fn ensure_chunk(&mut self, key: (i32, i32)) {
        if self.grid.chunks.contains_key(&key) {
            return;
        }
        let _span = tracing::info_span!("nav chunk").entered();
        let s = self.grid.settings;
        let margin = (s.agent_radius / s.cell).ceil() as i32;
        let side = CHUNK + 2 * margin;
        let (cx0, cz0) = (key.0 * CHUNK - margin, key.1 * CHUNK - margin);
        let mut heights = vec![0.0f64; (side * side) as usize];
        let mut wet = vec![false; (side * side) as usize];
        let mut scratch = std::mem::take(&mut self.grid.scratch);
        let below = self.grid.ground_ready.remove(&key);
        for lz in 0..side {
            for lx in 0..side {
                let (x, z) = self.cell_center(cx0 + lx, cz0 + lz);
                let i = (lz * side + lx) as usize;
                let g = match &below {
                    Some(g) => g[i],
                    None => ground_height(self.ground, x, z),
                };
                let h = self
                    .world
                    .stacked_height_over(x, z, g, s.agent_height, &mut scratch);
                heights[i] = h;
                if let Some(w) = self.water.sample(DVec3::new(x, h, z)) {
                    wet[i] = w.surface_height - h > s.max_water_depth as f64;
                }
            }
        }
        self.grid.scratch = scratch;
        let base = heights[(margin * side + margin) as usize];
        let mut chunk = Chunk {
            base,
            h: vec![0.0; (CHUNK * CHUNK) as usize],
            walk: vec![true; (CHUNK * CHUNK) as usize],
        };
        let r2 = (s.agent_radius / s.cell).powi(2) + 0.01;
        let mut near = Vec::new();
        for lz in 0..CHUNK {
            for lx in 0..CHUNK {
                let (sx, sz) = (lx + margin, lz + margin);
                let i = (sz * side + sx) as usize;
                let h = heights[i];
                let mut ok = !wet[i];
                if ok {
                    'n: for dz in -margin..=margin {
                        for dx in -margin..=margin {
                            if (dx * dx + dz * dz) as f32 > r2 || (dx == 0 && dz == 0) {
                                continue;
                            }
                            let j = ((sz + dz) * side + sx + dx) as usize;
                            if heights[j] > h + s.max_step as f64 {
                                ok = false;
                                break 'n;
                            }
                        }
                    }
                }
                // Clearance: no wall, post or block side within the agent radius (the heights
                // above are sampled a cell apart, so they miss a wall's end or a thin wall).
                if ok {
                    let (x, z) = self.cell_center(cx0 + sx, cz0 + sz);
                    ok = !self.crowded(DVec3::new(x, h, z), &mut near);
                }
                let o = (lz * CHUNK + lx) as usize;
                chunk.h[o] = (h - base) as f32;
                chunk.walk[o] = ok;
            }
        }
        self.grid.chunks.insert(key, chunk);
        self.grid.chunks_built += 1;
    }

    /// Whether something stands in the way of a body at `p` (its feet): an obstacle side within
    /// the agent radius that rises more than a step above its feet and whose bounds start below
    /// its head (not one it stands on, not a lintel above it). A tilted ramp counts from its
    /// lowest corner: the way under its high end is not taken, as bodies don't fit there.
    fn crowded(&self, p: DVec3, near: &mut Vec<u32>) -> bool {
        let s = self.grid.settings;
        let r = s.agent_radius as f64;
        let (step, head) = (p.y + s.max_step as f64, p.y + s.agent_height as f64);
        self.world.obstacles_near(
            DVec2::new(p.x - r, p.z - r),
            DVec2::new(p.x + r, p.z + r),
            near,
        );
        near.iter().any(|&i| {
            let o = &self.world.obstacles[i as usize];
            if o.max.y <= step || o.min.y >= head {
                return false;
            }
            let (d, out) = o.footprint_distance(p);
            if d as f64 >= r {
                return false;
            }
            // How high it rises where it is nearest, just inside its edge.
            let q = p - out.as_dvec3() * (d as f64 + 0.02);
            o.vertical_span(q.x, q.z).is_none_or(|(top, _)| top > step)
        })
    }

    fn cell(&mut self, x: i32, z: i32) -> (f64, bool) {
        let key = (x.div_euclid(CHUNK), z.div_euclid(CHUNK));
        let i = (z.rem_euclid(CHUNK) * CHUNK + x.rem_euclid(CHUNK)) as usize;
        if let Some(chunk) = self.grid.chunks.get(&key) {
            return (chunk.base + chunk.h[i] as f64, chunk.walk[i]);
        }
        self.ensure_chunk(key);
        let chunk = &self.grid.chunks[&key];
        (chunk.base + chunk.h[i] as f64, chunk.walk[i])
    }

    /// Height of the walkable surface at `p` and whether it is walkable.
    /// Like `sample`, but only from what is already built: `None` where the grid hasn't been
    /// worked out yet (so a wide scan doesn't stall a step building it all at once).
    pub fn peek(&self, p: DVec3) -> Option<(f64, bool)> {
        let (x, z) = self.cell_of(p);
        let key = (x.div_euclid(CHUNK), z.div_euclid(CHUNK));
        let i = (z.rem_euclid(CHUNK) * CHUNK + x.rem_euclid(CHUNK)) as usize;
        let chunk = self.grid.chunks.get(&key)?;
        Some((chunk.base + chunk.h[i] as f64, chunk.walk[i]))
    }

    pub fn sample(&mut self, p: DVec3) -> (f64, bool) {
        let (x, z) = self.cell_of(p);
        self.cell(x, z)
    }

    /// Is `p` on a walkable cell (with its height within a step of `p.y`)?
    pub fn walkable(&mut self, p: DVec3) -> bool {
        let (h, ok) = self.sample(p);
        ok && (h - p.y).abs() <= 1.0
    }

    /// The walkable point nearest to `p` within `radius` meters (same level), if any.
    pub fn nearest_walkable(&mut self, p: DVec3, radius: f32) -> Option<DVec3> {
        let (x, z) = self.cell_of(p);
        let r = (radius / self.grid.settings.cell).ceil() as i32;
        let mut best: Option<(i32, DVec3)> = None;
        for dz in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dz * dz;
                if d2 > r * r || best.is_some_and(|b| b.0 <= d2) {
                    continue;
                }
                let (h, ok) = self.cell(x + dx, z + dz);
                if ok && (h - p.y).abs() <= 1.2 {
                    let (cx, cz) = self.cell_center(x + dx, z + dz);
                    best = Some((d2, DVec3::new(cx, h, cz)));
                }
            }
        }
        best.map(|b| b.1)
    }

    fn window(&mut self, a: (i32, i32), b: (i32, i32), margin: i32) -> Window {
        let max = self.grid.settings.max_window;
        let x0 = a.0.min(b.0) - margin;
        let z0 = a.1.min(b.1) - margin;
        let w = (a.0.max(b.0) + margin - x0 + 1).min(max);
        let d = (a.1.max(b.1) + margin - z0 + 1).min(max);
        // Keep the start inside when clamped.
        let x0 = x0
            .max(a.0 - w + margin.min(w - 1) + 1)
            .min(a.0 - margin.min(w - 1));
        let z0 = z0
            .max(a.1 - d + margin.min(d - 1) + 1)
            .min(a.1 - margin.min(d - 1));
        let mut h = vec![0.0; (w * d) as usize];
        let mut walk = vec![false; (w * d) as usize];
        // Copy chunk by chunk: one map lookup per chunk, not per cell.
        for kz in z0.div_euclid(CHUNK)..=(z0 + d - 1).div_euclid(CHUNK) {
            for kx in x0.div_euclid(CHUNK)..=(x0 + w - 1).div_euclid(CHUNK) {
                self.ensure_chunk((kx, kz));
                let chunk = &self.grid.chunks[&(kx, kz)];
                let (cx, cz) = (kx * CHUNK, kz * CHUNK);
                for z in cz.max(z0)..(cz + CHUNK).min(z0 + d) {
                    for x in cx.max(x0)..(cx + CHUNK).min(x0 + w) {
                        let ci = ((z - cz) * CHUNK + (x - cx)) as usize;
                        let wi = ((z - z0) * w + (x - x0)) as usize;
                        h[wi] = chunk.base + chunk.h[ci] as f64;
                        walk[wi] = chunk.walk[ci];
                    }
                }
            }
        }
        Window {
            x0,
            z0,
            w,
            d,
            h,
            walk,
        }
    }

    /// Find a path from `from` to `to`. `extra_cost` returns an additional cost multiplier
    /// (0 = normal, 1 = twice as expensive, `INFINITY` = avoid) for a world point; use it for
    /// fire, danger or enemy sight lines. Waypoints (including the goal) are written to `out`.
    pub fn find_path(
        &mut self,
        from: DVec3,
        to: DVec3,
        extra_cost: &mut dyn FnMut(DVec3) -> f32,
        out: &mut Vec<DVec3>,
    ) -> PathStatus {
        let _span = tracing::info_span!("find path").entered();
        out.clear();
        let s = self.grid.settings;
        let start = self.cell_of(from);
        let goal = self.cell_of(to);
        // Worn ground anywhere the search could go: it may be worth a detour, so search.
        let peak = self.wear_peak(start, goal, 24, from.y);
        let worn = peak > WORN_MIN;
        // Most trips in open ground are a straight walk: skip the search for those.
        if !worn && let Some(h) = self.straight_walk(from, to, extra_cost) {
            out.push(DVec3::new(to.x, h, to.z));
            return PathStatus::Complete;
        }
        let win = self.window(start, goal, 24);
        let Some(mut si) = win.idx(start.0, start.1) else {
            return PathStatus::Failed;
        };
        if !win.walk[si] || (win.h[si] - from.y).abs() > 1.2 {
            // Standing next to a wall (inside the clearance margin): start from the nearest
            // walkable cell on the same level, one a step from its feet before one on top of
            // the barrel or crate it stands beside (it can't get up there).
            let mut best: Option<(i32, usize)> = None;
            for dz in -4..=4 {
                for dx in -4..=4 {
                    if let Some(i) = win.idx(start.0 + dx, start.1 + dz)
                        && win.walk[i]
                        && (win.h[i] - from.y).abs() <= 1.2
                    {
                        let off_level = (win.h[i] - from.y).abs() > s.max_step as f64;
                        let d2 = dx * dx + dz * dz + if off_level { 100 } else { 0 };
                        if best.is_none_or(|b| d2 < b.0) {
                            best = Some((d2, i));
                        }
                    }
                }
            }
            match best {
                Some((_, i)) => si = i,
                None => return PathStatus::Failed,
            }
        }
        let goal_in = (
            goal.0.clamp(win.x0, win.x0 + win.w - 1),
            goal.1.clamp(win.z0, win.z0 + win.d - 1),
        );
        let clamped = goal_in != goal;
        let gi = win.idx(goal_in.0, goal_in.1).unwrap();
        let n = (win.w * win.d) as usize;
        let mut g = vec![f32::INFINITY; n];
        let mut parent = vec![u32::MAX; n];
        let mut closed = vec![false; n];
        let mut extra = vec![f32::NAN; n];
        let mut open = BinaryHeap::new();
        // What a step onto each cell costs per meter before its extra cost: 1, or less on
        // worn ground.
        let mut tread = if worn { vec![f32::NAN; n] } else { Vec::new() };
        let wear = self.wear.filter(|_| worn);
        let mut tread_at = |ctx: &Self, i: usize| -> f32 {
            let Some(wear) = wear else {
                return 1.0;
            };
            if tread[i].is_nan() {
                let (x, z) = ((i as i32) % win.w + win.x0, (i as i32) / win.w + win.z0);
                let (cx, cz) = ctx.cell_center(x, z);
                tread[i] = 1.0 - WORN_DISCOUNT * wear.wear(DVec3::new(cx, win.h[i], cz));
            }
            tread[i]
        };
        // The cheapest a meter can be (the coarse peak can miss a cell's true top, so allow
        // a little more): keeps the estimate from overshooting, so worn detours are found.
        let floor = if worn {
            1.0 - WORN_DISCOUNT * (peak * 1.25).min(1.0)
        } else {
            1.0
        };
        let (gx, gz) = ((gi as i32) % win.w, (gi as i32) / win.w);
        let heuristic = |i: usize| {
            let (x, z) = ((i as i32) % win.w, (i as i32) / win.w);
            let (dx, dz) = ((x - gx).abs() as f32, (z - gz).abs() as f32);
            (dx.max(dz) + 0.4142 * dx.min(dz)) * s.cell * floor
        };
        g[si] = 0.0;
        open.push(Open {
            f: heuristic(si),
            idx: si as u32,
        });
        let mut best = (heuristic(si), si);
        let mut reached = false;
        const N8: [(i32, i32); 8] = [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ];
        while let Some(Open { idx, .. }) = open.pop() {
            let i = idx as usize;
            if closed[i] {
                continue;
            }
            closed[i] = true;
            let hcur = heuristic(i);
            if hcur < best.0 {
                best = (hcur, i);
            }
            if i == gi {
                reached = true;
                break;
            }
            let (x, z) = ((i as i32) % win.w, (i as i32) / win.w);
            for (dx, dz) in N8 {
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= win.w || nz >= win.d {
                    continue;
                }
                let j = (nz * win.w + nx) as usize;
                if closed[j] {
                    continue;
                }
                let diagonal = dx != 0 && dz != 0;
                if !win.connected(i, j, s.max_step, diagonal) {
                    continue;
                }
                if diagonal {
                    let a = (z * win.w + nx) as usize;
                    let b = (nz * win.w + x) as usize;
                    if !win.connected(i, a, s.max_step, false)
                        || !win.connected(i, b, s.max_step, false)
                    {
                        continue;
                    }
                }
                if extra[j].is_nan() {
                    let (cx, cz) = self.cell_center(win.x0 + nx, win.z0 + nz);
                    extra[j] = extra_cost(DVec3::new(cx, win.h[j], cz)).max(0.0);
                }
                if !extra[j].is_finite() {
                    continue;
                }
                let step = if diagonal {
                    std::f32::consts::SQRT_2
                } else {
                    1.0
                } * s.cell;
                let climb = (win.h[j] - win.h[i]).abs() as f32;
                let cost = g[i] + (step + climb) * (tread_at(self, j) + extra[j]);
                if cost < g[j] {
                    g[j] = cost;
                    parent[j] = i as u32;
                    open.push(Open {
                        f: cost + heuristic(j),
                        idx: j as u32,
                    });
                }
            }
        }
        let end = if reached { gi } else { best.1 };
        // Walk back.
        let mut cells = Vec::new();
        let mut c = end;
        while c != si {
            cells.push(c);
            c = parent[c] as usize;
            if c == u32::MAX as usize {
                return PathStatus::Failed;
            }
        }
        cells.push(si);
        cells.reverse();
        // String pulling: skip waypoints while a straight walk stays on connected cells.
        let to_world = |ctx: &Self, i: usize| {
            let (x, z) = ((i as i32) % win.w + win.x0, (i as i32) / win.w + win.z0);
            let (cx, cz) = ctx.cell_center(x, z);
            DVec3::new(cx, win.h[i], cz)
        };
        // Shortcuts may not cross cells that cost more than the cells the path used.
        let mut path_cost = 0.0f32;
        for &c in &cells {
            if extra[c].is_finite() {
                path_cost = path_cost.max(extra[c]);
            }
        }
        let mut cost_at = |ctx: &Self, i: usize| -> f32 {
            if extra[i].is_nan() {
                let (x, z) = ((i as i32) % win.w + win.x0, (i as i32) / win.w + win.z0);
                let (cx, cz) = ctx.cell_center(x, z);
                extra[i] = extra_cost(DVec3::new(cx, win.h[i], cz)).max(0.0);
            }
            extra[i]
        };
        let mut anchor = 0;
        let mut k = 1;
        while k < cells.len() {
            if k + 1 < cells.len()
                && self.straight(
                    &win,
                    cells[anchor],
                    cells[k + 1],
                    s.max_step,
                    &mut |ctx, i| cost_at(ctx, i) <= path_cost + 0.05,
                )
                // On worn ground a shortcut must also cost no more than the path it skips
                // (or string pulling would cut the worn path's bends off).
                && (!worn || {
                    let (a, b) = (cells[anchor], cells[k + 1]);
                    let line = self.line_cost(&win, a, b, &mut |ctx, i| {
                        tread_at(ctx, i) + cost_at(ctx, i)
                    });
                    line <= (g[b] - g[a]) * 1.02 + 0.05
                })
            {
                k += 1;
                continue;
            }
            out.push(to_world(self, cells[k]));
            anchor = k;
            k += 1;
        }
        if reached && !clamped {
            // Finish at the exact goal when it's on the goal cell's level.
            let last = to_world(self, gi);
            if (to.y - last.y).abs() < 1.0 {
                if let Some(l) = out.last_mut() {
                    *l = DVec3::new(to.x, last.y, to.z);
                } else {
                    out.push(DVec3::new(to.x, last.y, to.z));
                }
            }
        }
        if reached && !clamped {
            return PathStatus::Complete;
        }
        // A goal just inside a wall's clearance (a cover spot, a mate lying by a wall): the
        // path ends on the nearest free cell, and the last step goes on to the goal itself.
        let last = to_world(self, end);
        let short = DVec2::new(to.x - last.x, to.z - last.z).length();
        if !clamped && short <= 1.0 && (to.y - last.y).abs() < 0.5 {
            out.push(DVec3::new(to.x, last.y, to.z));
            return PathStatus::Complete;
        }
        PathStatus::Partial
    }

    /// If one can walk straight from `from` to `to` over free cells (walkable, connected, no
    /// extra cost, with clearance either side), the height of the goal cell.
    fn straight_walk(
        &mut self,
        from: DVec3,
        to: DVec3,
        extra_cost: &mut dyn FnMut(DVec3) -> f32,
    ) -> Option<f64> {
        let s = self.grid.settings;
        let (a, b) = (self.cell_of(from), self.cell_of(to));
        let (dx, dz) = (b.0 - a.0, b.1 - a.1);
        let n = dx.abs().max(dz.abs()) * 2;
        if n > s.max_window {
            return None;
        }
        let (h0, ok) = self.cell(a.0, a.1);
        if !ok || (h0 - from.y).abs() > 1.2 {
            return None;
        }
        let len = ((dx * dx + dz * dz) as f32).sqrt().max(1e-3);
        let (px, pz) = (-(dz as f32) / len, dx as f32 / len);
        let mut prev = (a, h0);
        for k in 1..=n.max(1) {
            let t = k as f32 / n.max(1) as f32;
            for off in [-0.35f32, 0.35, 0.0] {
                let x = (a.0 as f32 + 0.5 + dx as f32 * t + px * off).floor() as i32;
                let z = (a.1 as f32 + 0.5 + dz as f32 * t + pz * off).floor() as i32;
                let (h, ok) = self.cell(x, z);
                if !ok {
                    return None;
                }
                if off == 0.0 && (x, z) != prev.0 {
                    let diagonal = x != prev.0.0 && z != prev.0.1;
                    let limit = if diagonal {
                        s.max_step * 1.42
                    } else {
                        s.max_step
                    } as f64;
                    if (h - prev.1).abs() > limit {
                        return None;
                    }
                    let (cx, cz) = self.cell_center(x, z);
                    if extra_cost(DVec3::new(cx, h, cz)) != 0.0 {
                        return None;
                    }
                    prev = ((x, z), h);
                }
            }
        }
        (prev.0 == b && (to.y - prev.1).abs() < 1.0).then_some(prev.1)
    }

    /// What walking straight between window cells `a` and `b` costs, with `per_m` the cost
    /// per meter of a cell (sampled along the center line, climbs left out).
    fn line_cost(
        &self,
        win: &Window,
        a: usize,
        b: usize,
        per_m: &mut dyn FnMut(&Self, usize) -> f32,
    ) -> f32 {
        let (ax, az) = ((a as i32) % win.w, (a as i32) / win.w);
        let (bx, bz) = ((b as i32) % win.w, (b as i32) / win.w);
        let (dx, dz) = (bx - ax, bz - az);
        let n = dx.abs().max(dz.abs()) * 2;
        if n == 0 {
            return 0.0;
        }
        let len = ((dx * dx + dz * dz) as f32).sqrt() * self.grid.settings.cell;
        let mut sum = 0.0;
        for k in 1..=n {
            let t = (k as f32 - 0.5) / n as f32;
            let x = (ax as f32 + 0.5 + dx as f32 * t).floor() as i32;
            let z = (az as f32 + 0.5 + dz as f32 * t).floor() as i32;
            sum += per_m(self, (z * win.w + x) as usize);
        }
        len * sum / n as f32
    }

    /// Can one walk straight between window cells `a` and `b` (every crossed cell walkable
    /// and connected to the previous one)?
    fn straight(
        &self,
        win: &Window,
        a: usize,
        b: usize,
        step: f32,
        allowed: &mut dyn FnMut(&Self, usize) -> bool,
    ) -> bool {
        let (ax, az) = ((a as i32) % win.w, (a as i32) / win.w);
        let (bx, bz) = ((b as i32) % win.w, (b as i32) / win.w);
        let (dx, dz) = (bx - ax, bz - az);
        let n = dx.abs().max(dz.abs()) * 2;
        if n == 0 {
            return true;
        }
        let mut prev = a;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            // Sample at the center line and half a cell to either side (clearance).
            for off in [-0.35f32, 0.0, 0.35] {
                let len = ((dx * dx + dz * dz) as f32).sqrt();
                let (px, pz) = (-(dz as f32) / len * off, dx as f32 / len * off);
                let x = (ax as f32 + 0.5 + dx as f32 * t + px).floor() as i32;
                let z = (az as f32 + 0.5 + dz as f32 * t + pz).floor() as i32;
                if x < 0 || z < 0 || x >= win.w || z >= win.d {
                    return false;
                }
                let i = (z * win.w + x) as usize;
                if !win.walk[i] || !allowed(self, i) {
                    return false;
                }
                if off == 0.0 {
                    if i != prev && !win.connected(prev, i, step, true) {
                        return false;
                    }
                    prev = i;
                }
            }
        }
        true
    }
}

impl TacticalWorld {
    /// [`TacticalWorld::stacked_height`] with a caller-owned scratch buffer.
    pub fn stacked_height_scratch(
        &self,
        x: f64,
        z: f64,
        body_height: f32,
        ground: &Ground,
        scratch: &mut Vec<u32>,
    ) -> (f64, f64) {
        let g = ground_height(ground, x, z);
        (self.stacked_height_over(x, z, g, body_height, scratch), g)
    }

    /// [`Self::stacked_height_scratch`] over a ground height already sampled.
    pub fn stacked_height_over(
        &self,
        x: f64,
        z: f64,
        g: f64,
        body_height: f32,
        scratch: &mut Vec<u32>,
    ) -> f64 {
        let mut h = g;
        self.obstacles_near(DVec2::new(x, z), DVec2::new(x, z), scratch);
        for _ in 0..4 {
            let mut raised = false;
            for &i in scratch.iter() {
                if let Some((top, bottom)) = self.obstacles[i as usize].vertical_span(x, z)
                    && top > h + 0.01
                    && bottom < h + body_height as f64
                {
                    h = top;
                    raised = true;
                }
            }
            if !raised {
                break;
            }
        }
        h
    }
}

/// The ground's height at X/Z, from above (0 where there is no ground).
fn ground_height(ground: &Ground, x: f64, z: f64) -> f64 {
    ground.height(DVec3::new(x, 1.0e6, z)).unwrap_or(0.0)
}

/// Path following state for one agent (managed by the NPC systems).
#[derive(Component, Debug, Clone, Default)]
pub struct NavAgent {
    /// Waypoints still ahead.
    pub path: Vec<DVec3>,
    /// The goal the path was planned for.
    pub goal: Option<DVec3>,
    pub status: Option<PathStatus>,
    /// Local time (NPC clock) of the last plan.
    pub planned_at: f64,
}

impl NavAgent {
    /// The next waypoint, dropping ones already reached.
    pub fn next_waypoint(&mut self, at: DVec3, reach: f64) -> Option<DVec3> {
        while let Some(&p) = self.path.first() {
            let flat = DVec2::new(p.x - at.x, p.z - at.z).length();
            if flat <= reach && self.path.len() > 1 {
                self.path.remove(0);
            } else {
                return Some(p);
            }
        }
        None
    }

    /// Forget the current path.
    pub fn clear(&mut self) {
        self.path.clear();
        self.goal = None;
        self.status = None;
    }
}

/// How far round an awake NPC the grid is worked out ahead of need, in chunks (a chunk is
/// 16 m at the default cell): its own search for spots and the ring round its enemy.
const WARM_CHUNKS: i32 = 3;
/// Chunks whose ground is worked out off the main thread at once.
const WARM_JOBS: usize = 4;

/// Works out the walkable grid round awake NPCs ahead of need, nearest first, so their first
/// look round a new place (spots, high ground, a route) doesn't build a dozen chunks in a
/// single step. The ground under a chunk (the slow part on real terrain) is sampled on the
/// async pool and the chunk finished here a step or more later; without a pool, one chunk is
/// built a step.
pub(crate) fn warm_nav(
    mut grid: ResMut<NavGrid>,
    world: Res<TacticalWorld>,
    ground: Res<Ground>,
    water: Res<Water>,
    agents: Query<&Transform, (With<crate::agent::Npc>, Without<crate::rest::Resting>)>,
) {
    if ground.is_changed() {
        grid.ground_jobs.clear();
        grid.ground_ready.clear();
    }
    // Collect what the pool has finished.
    let done: Vec<(i32, i32)> = grid
        .ground_jobs
        .iter()
        .filter(|(_, job)| job.lock().is_ok_and(|j| j.is_some()))
        .map(|(&k, _)| k)
        .collect();
    for key in done {
        if let Some(job) = grid.ground_jobs.remove(&key)
            && let Some(g) = job.lock().ok().and_then(|mut j| j.take())
        {
            grid.ground_ready.insert(key, g);
        }
    }
    let pool = AsyncComputeTaskPool::try_get();
    let mut ctx = grid.ctx(&world, &ground, &water);
    // Finish one chunk whose ground is ready (its obstacles and clearance are quick).
    if let Some(&key) = ctx.grid.ground_ready.keys().next() {
        if ctx.grid.chunks.contains_key(&key) {
            ctx.grid.ground_ready.remove(&key);
        } else {
            ctx.ensure_chunk(key);
        }
    }
    let room = if pool.is_some() {
        WARM_JOBS.saturating_sub(ctx.grid.ground_jobs.len())
    } else {
        1
    };
    if room == 0 {
        return;
    }
    let mut wanted: Vec<(i32, (i32, i32))> = Vec::new();
    for t in &agents {
        let (x, z) = ctx.cell_of(t.translation);
        let (kx, kz) = (x.div_euclid(CHUNK), z.div_euclid(CHUNK));
        for dz in -WARM_CHUNKS..=WARM_CHUNKS {
            for dx in -WARM_CHUNKS..=WARM_CHUNKS {
                let d2 = dx * dx + dz * dz;
                let key = (kx + dx, kz + dz);
                if d2 > WARM_CHUNKS * WARM_CHUNKS
                    || ctx.grid.chunks.contains_key(&key)
                    || ctx.grid.ground_jobs.contains_key(&key)
                    || ctx.grid.ground_ready.contains_key(&key)
                {
                    continue;
                }
                match wanted.iter_mut().find(|w| w.1 == key) {
                    Some(w) => w.0 = w.0.min(d2),
                    None => wanted.push((d2, key)),
                }
            }
        }
    }
    wanted.sort_unstable();
    let Some(pool) = pool else {
        if let Some(&(_, key)) = wanted.first() {
            ctx.ensure_chunk(key);
        }
        return;
    };
    let s = ctx.grid.settings;
    let margin = (s.agent_radius / s.cell).ceil() as i32;
    let side = CHUNK + 2 * margin;
    for &(_, key) in wanted.iter().take(room) {
        let job: GroundJob = Arc::new(Mutex::new(None));
        let out = job.clone();
        let ground = ground.clone();
        let (cx0, cz0) = (key.0 * CHUNK - margin, key.1 * CHUNK - margin);
        let cell = s.cell as f64;
        pool.spawn(async move {
            let mut g = Vec::with_capacity((side * side) as usize);
            for lz in 0..side {
                for lx in 0..side {
                    let x = ((cx0 + lx) as f64 + 0.5) * cell;
                    let z = ((cz0 + lz) as f64 + 0.5) * cell;
                    g.push(ground_height(&ground, x, z));
                }
            }
            if let Ok(mut out) = out.lock() {
                *out = Some(g);
            }
        })
        .detach();
        ctx.grid.ground_jobs.insert(key, job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_model::{Obstacle, ObstacleShape};
    use engine_world::{WaterQuery, WaterSample, WearQuery};
    use std::sync::Arc;

    fn wall(i: u32, center: DVec3, half: Vec3, rot: Quat) -> Obstacle {
        Obstacle::new(
            Entity::from_raw_u32(i).unwrap(),
            center,
            rot,
            ObstacleShape::Box { half },
        )
    }

    fn path_len(from: DVec3, path: &[DVec3]) -> f64 {
        let mut p = from;
        let mut len = 0.0;
        for &q in path {
            len += p.distance(q);
            p = q;
        }
        len
    }

    #[test]
    fn paths_go_around_a_wall() {
        let world = TacticalWorld::from_obstacles(
            vec![wall(
                1,
                DVec3::new(0.0, 2.0, 0.0),
                Vec3::new(6.0, 2.0, 0.3),
                Quat::IDENTITY,
            )],
            &Ground::default(),
        );
        let mut grid = NavGrid::default();
        let (ground, water) = (Ground::default(), Water::default());
        let mut ctx = grid.ctx(&world, &ground, &water);
        let mut path = Vec::new();
        let from = DVec3::new(0.0, 0.0, -5.0);
        let to = DVec3::new(0.0, 0.0, 5.0);
        let status = ctx.find_path(from, to, &mut |_| 0.0, &mut path);
        assert_eq!(status, PathStatus::Complete);
        let len = path_len(from, &path);
        assert!(len > 13.0 && len < 18.0, "{len} {path:?}");
        assert!(path.last().unwrap().distance(to) < 0.01);
        // Every waypoint is off the wall.
        for p in &path {
            assert!(p.x.abs() > 6.0 || p.z.abs() > 0.5, "{p}");
        }
    }

    #[test]
    fn paths_climb_a_ramp_onto_a_platform() {
        let angle = 0.35f32;
        let ramp_len = 1.5 / angle.sin();
        let world = TacticalWorld::from_obstacles(
            vec![
                wall(
                    1,
                    DVec3::new(0.0, 0.75, 0.0),
                    Vec3::new(3.0, 0.75, 3.0),
                    Quat::IDENTITY,
                ),
                wall(
                    2,
                    DVec3::new(
                        0.0,
                        0.75 - 0.1,
                        3.0 + ramp_len as f64 * 0.5 * angle.cos() as f64,
                    ),
                    Vec3::new(1.0, 0.1, ramp_len * 0.5),
                    Quat::from_rotation_x(angle),
                ),
            ],
            &Ground::default(),
        );
        let mut grid = NavGrid::default();
        let (ground, water) = (Ground::default(), Water::default());
        let mut ctx = grid.ctx(&world, &ground, &water);
        let mut path = Vec::new();
        // From behind the platform to its top: must go around and up the ramp (+Z side).
        let from = DVec3::new(0.0, 0.0, -6.0);
        let to = DVec3::new(0.0, 1.5, 0.0);
        let status = ctx.find_path(from, to, &mut |_| 0.0, &mut path);
        assert_eq!(status, PathStatus::Complete, "{path:?}");
        assert!((path.last().unwrap().y - 1.5).abs() < 0.05);
        assert!(
            path.iter().any(|p| p.z > 3.5 && p.y < 1.4),
            "path should use the ramp: {path:?}"
        );
    }

    #[test]
    fn a_path_starts_on_the_ground_beside_a_barrel() {
        // Pressed against a barrel by a wall's end (inside the clearance margin): the nearest
        // free cell is the barrel's top, a meter up, but the way out is along the ground.
        let barrel = Obstacle::new(
            Entity::from_raw_u32(2).unwrap(),
            DVec3::new(1.8, 0.5, 11.2),
            Quat::IDENTITY,
            ObstacleShape::Cylinder {
                radius: 0.32,
                half_height: 0.5,
            },
        );
        let world = TacticalWorld::from_obstacles(
            vec![
                wall(
                    1,
                    DVec3::new(0.0, 0.55, 10.5),
                    Vec3::new(1.6, 0.55, 0.25),
                    Quat::IDENTITY,
                ),
                barrel,
            ],
            &Ground::default(),
        );
        let mut grid = NavGrid::default();
        let (ground, water) = (Ground::default(), Water::default());
        let mut ctx = grid.ctx(&world, &ground, &water);
        let mut path = Vec::new();
        let from = DVec3::new(1.67, 0.0, 11.99);
        let to = DVec3::new(-10.0, 0.0, -3.8);
        let status = ctx.find_path(from, to, &mut |_| 0.0, &mut path);
        assert_eq!(status, PathStatus::Complete, "{path:?}");
        assert!(path.iter().all(|p| p.y < 0.2), "{path:?}");
    }

    #[test]
    fn costs_steer_paths_around_fire() {
        let world = TacticalWorld::from_obstacles(Vec::new(), &Ground::default());
        let mut grid = NavGrid::default();
        let (ground, water) = (Ground::default(), Water::default());
        let mut ctx = grid.ctx(&world, &ground, &water);
        let mut path = Vec::new();
        let from = DVec3::new(-8.0, 0.0, 0.0);
        let to = DVec3::new(8.0, 0.0, 0.0);
        let fire = |p: DVec3| {
            if p.x.abs() < 3.0 && p.z.abs() < 3.0 {
                f32::INFINITY
            } else {
                0.0
            }
        };
        let status = ctx.find_path(from, to, &mut { fire }, &mut path);
        assert_eq!(status, PathStatus::Complete);
        let mut prev = from;
        for &p in &path {
            for k in 0..=10 {
                let q = prev.lerp(p, k as f64 / 10.0);
                assert!(
                    !(q.x.abs() < 2.9 && q.z.abs() < 2.9),
                    "walks through fire at {q}"
                );
            }
            prev = p;
        }
    }

    struct Lake;
    impl WaterQuery for Lake {
        fn sample(&self, p: DVec3) -> Option<WaterSample> {
            (p.x.abs() < 4.0 && p.z.abs() < 20.0).then_some(WaterSample {
                surface_height: 2.0,
                depth: 2.0,
                velocity: Vec3::ZERO,
                normal: Vec3::Y,
            })
        }
    }

    #[test]
    fn deep_water_is_not_walkable_and_partial_paths_get_close() {
        let world = TacticalWorld::from_obstacles(Vec::new(), &Ground::default());
        let mut grid = NavGrid::default();
        let ground = Ground::default();
        let water = Water(Arc::new(Lake));
        let mut ctx = grid.ctx(&world, &ground, &water);
        assert!(!ctx.walkable(DVec3::new(0.0, 0.0, 0.0)));
        let mut path = Vec::new();
        let status = ctx.find_path(
            DVec3::new(-10.0, 0.0, 0.0),
            DVec3::new(0.0, 0.0, 0.0),
            &mut |_| 0.0,
            &mut path,
        );
        assert_eq!(status, PathStatus::Partial);
        let end = *path.last().unwrap();
        assert!(end.x > -5.0 && end.x < -4.0, "{end}");
    }

    /// Wear 1 within half a meter of a polyline, fading out by 1.2 m.
    struct Trail(Vec<DVec3>);
    impl WearQuery for Trail {
        fn wear(&self, p: DVec3) -> f32 {
            let q = DVec2::new(p.x, p.z);
            let mut d = f64::INFINITY;
            for s in self.0.windows(2) {
                let (a, b) = (DVec2::new(s[0].x, s[0].z), DVec2::new(s[1].x, s[1].z));
                let t = ((q - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
                d = d.min(q.distance(a + (b - a) * t));
            }
            (1.0 - (d as f32 - 0.5) / 0.7).clamp(0.0, 1.0)
        }
    }

    /// The share of a path's length on ground worn at least `at`.
    fn share_worn(from: DVec3, path: &[DVec3], wear: &GroundWear, at: f32) -> f64 {
        let (mut on, mut all) = (0.0, 0.0);
        let mut prev = from;
        for &p in path {
            let n = (prev.distance(p) / 0.25).ceil().max(1.0) as usize;
            for k in 0..n {
                let q = prev.lerp(p, (k as f64 + 0.5) / n as f64);
                let l = prev.distance(p) / n as f64;
                all += l;
                if wear.wear(q) >= at {
                    on += l;
                }
            }
            prev = p;
        }
        on / all.max(1e-9)
    }

    #[test]
    fn a_worn_path_bends_a_route_onto_it() {
        let world = TacticalWorld::from_obstacles(Vec::new(), &Ground::default());
        let mut grid = NavGrid::default();
        let (ground, water) = (Ground::default(), Water::default());
        let from = DVec3::new(-15.0, 0.0, 0.0);
        let to = DVec3::new(15.0, 0.0, 0.0);
        // A path worn out to the side and back: 1.24x and 1.41x the straight walk.
        for out in [7.0, 10.0] {
            let trail = vec![
                from,
                DVec3::new(-10.0, 0.0, out),
                DVec3::new(10.0, 0.0, out),
                to,
            ];
            let trail_len = path_len(from, &trail[1..]);
            let ratio = trail_len / from.distance(to);
            let wear = GroundWear(Arc::new(Trail(trail)));
            let mut ctx = grid.ctx(&world, &ground, &water).worn(Some(&wear));
            let mut path = Vec::new();
            let status = ctx.find_path(from, to, &mut |_| 0.0, &mut path);
            assert_eq!(status, PathStatus::Complete);
            let len = path_len(from, &path);
            let on = share_worn(from, &path, &wear, 0.5);
            assert!(
                path.iter().any(|p| p.z > out - 1.0),
                "{ratio:.2}x: the route keeps off the worn path: {path:?}"
            );
            assert!(
                on > 0.9 && (len / trail_len - 1.0).abs() < 0.06,
                "{ratio:.2}x: {:.0}% of the {len:.1} m route is on the {trail_len:.1} m path: \
                 {path:?}",
                on * 100.0
            );
        }
        // Worn ground straight across the way changes nothing: still a straight walk.
        let across = GroundWear(Arc::new(Trail(vec![
            DVec3::new(0.0, 0.0, -10.0),
            DVec3::new(0.0, 0.0, 10.0),
        ])));
        let mut ctx = grid.ctx(&world, &ground, &water).worn(Some(&across));
        let mut path = Vec::new();
        ctx.find_path(from, to, &mut |_| 0.0, &mut path);
        assert!(path_len(from, &path) < 30.2, "{path:?}");
    }

    #[test]
    fn without_wear_routes_are_unchanged() {
        struct Untouched;
        impl WearQuery for Untouched {
            fn wear(&self, _p: DVec3) -> f32 {
                0.0
            }
        }
        let world = TacticalWorld::from_obstacles(
            vec![wall(
                1,
                DVec3::new(0.0, 2.0, 0.0),
                Vec3::new(6.0, 2.0, 0.3),
                Quat::IDENTITY,
            )],
            &Ground::default(),
        );
        let (ground, water) = (Ground::default(), Water::default());
        let default = GroundWear::default();
        let untouched = GroundWear(Arc::new(Untouched));
        let trips = [
            (DVec3::new(0.0, 0.0, -5.0), DVec3::new(0.0, 0.0, 5.0)),
            (DVec3::new(-9.0, 0.0, -3.0), DVec3::new(8.0, 0.0, 4.0)),
            (DVec3::new(-20.0, 0.0, 2.0), DVec3::new(20.0, 0.0, 9.0)),
            (DVec3::new(3.0, 0.0, 1.0), DVec3::new(-3.0, 0.0, -1.5)),
        ];
        let fire = |p: DVec3| {
            if (p.x - 8.0).abs() < 2.0 && p.z.abs() < 3.0 {
                f32::INFINITY
            } else if p.z > 4.0 {
                0.7
            } else {
                0.0
            }
        };
        for (from, to) in trips {
            let mut runs = Vec::new();
            for wear in [None, Some(&default), Some(&untouched)] {
                let mut grid = NavGrid::default();
                let mut ctx = grid.ctx(&world, &ground, &water).worn(wear);
                let mut path = Vec::new();
                let status = ctx.find_path(from, to, &mut { fire }, &mut path);
                runs.push((status, path));
            }
            assert_eq!(runs[0], runs[1], "{from} -> {to}");
            assert_eq!(runs[0], runs[2], "{from} -> {to}");
        }
    }

    /// A small wear map that walkers tread: each walk adds wear along its line and the rest
    /// heals a little (a stand-in for terrain's, which this crate can't use).
    #[derive(Clone)]
    struct Field {
        wear: Vec<f32>,
    }
    const FIELD_W: i32 = 100;
    const FIELD_D: i32 = 80;
    impl Field {
        fn index(p: DVec3) -> Option<usize> {
            let (x, z) = (
                ((p.x + 25.0) / 0.5).floor() as i32,
                ((p.z + 20.0) / 0.5).floor() as i32,
            );
            (x >= 0 && z >= 0 && x < FIELD_W && z < FIELD_D).then_some((z * FIELD_W + x) as usize)
        }
        fn tread(&mut self, from: DVec3, path: &[DVec3]) {
            for w in &mut self.wear {
                *w *= 0.99;
            }
            let mut prev = from;
            let mut hit = std::collections::HashSet::new();
            for &p in path {
                let n = (prev.distance(p) / 0.2).ceil().max(1.0) as usize;
                for k in 0..=n {
                    if let Some(i) = Self::index(prev.lerp(p, k as f64 / n as f64)) {
                        hit.insert(i);
                    }
                }
                prev = p;
            }
            for i in hit {
                self.wear[i] = (self.wear[i] + 0.12).min(1.0);
            }
        }
    }
    impl WearQuery for Field {
        fn wear(&self, p: DVec3) -> f32 {
            Self::index(p).map_or(0.0, |i| self.wear[i])
        }
    }

    /// Walkers cross a field between two loose crowds of start and end points; returns how
    /// wide the band of worn ground (wear > 0.5) is across the middle, and its peak.
    fn walk_the_field(prefer_worn: bool) -> (f64, f32) {
        let world = TacticalWorld::from_obstacles(Vec::new(), &Ground::default());
        let (ground, water) = (Ground::default(), Water::default());
        let mut grid = NavGrid::default();
        let mut field = Field {
            wear: vec![0.0; (FIELD_W * FIELD_D) as usize],
        };
        let mut seed = 0x2545_F491_u32;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f64 / u32::MAX as f64
        };
        // A walk every few seconds for a quarter of an hour, alternating ways.
        for k in 0..240 {
            let mut end = |x: f64| {
                let (a, r) = (rand() * std::f64::consts::TAU, 5.0 * rand().sqrt());
                DVec3::new(x + a.cos() * r, 0.0, a.sin() * r)
            };
            let (mut from, mut to) = (end(-16.0), end(16.0));
            if k % 2 == 1 {
                std::mem::swap(&mut from, &mut to);
            }
            let wear = GroundWear(Arc::new(field.clone()));
            let mut ctx = grid
                .ctx(&world, &ground, &water)
                .worn(prefer_worn.then_some(&wear));
            let mut path = Vec::new();
            ctx.find_path(from, to, &mut |_| 0.0, &mut path);
            field.tread(from, &path);
        }
        // Across the middle of the field.
        let across: Vec<f32> = (0..FIELD_D)
            .map(|z| {
                let zm = z as f64 * 0.5 - 20.0 + 0.25;
                (-4..=4)
                    .map(|dx| field.wear(DVec3::new(dx as f64 * 0.5, 0.0, zm)))
                    .sum::<f32>()
                    / 9.0
            })
            .collect();
        let worn = across.iter().filter(|w| **w > 0.5).count() as f64 * 0.5;
        (worn, across.iter().cloned().fold(0.0, f32::max))
    }

    #[test]
    fn walkers_wear_a_path_that_draws_them_onto_it() {
        let (band, peak) = walk_the_field(true);
        let (spread, flat) = walk_the_field(false);
        // Taking the worn line, they wear one narrow path deep; walking straight, they wear
        // the field over a wide strip and nowhere deeply.
        assert!(
            peak > 0.8 && band > 0.0 && band <= 3.0,
            "preferring worn ground: band {band} m wide, peak {peak}"
        );
        assert!(
            flat < peak - 0.2 || spread > band * 2.0,
            "walking straight: band {spread} m, peak {flat} (vs {band} m, {peak})"
        );
    }
}
