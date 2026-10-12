//! Navigation: a walkable-height grid built lazily in chunks from the [`TacticalWorld`], the
//! `Ground` query and the `Water` query, plus A* pathfinding with string-pulled (smooth) paths.
//!
//! * Each cell stores the height someone standing there would be at: the ground, or the top of
//!   whatever they stand on (a platform, a ramp). Walls make tall cells that can't be stepped
//!   onto, so paths go around walls, up ramps and onto platforms with no extra setup.
//! * Cells next to walls are shrunk away by the agent radius; deep water is not walkable.
//! * Callers add their own per-cell cost (fire heat, enemy sight lines) when they search, so
//!   NPCs route around burning grass or through cover.
//! * Chunks are rebuilt when the world model changes (colliders appear or move, terrain is
//!   edited), and only where someone asks for a path, so it scales to planet-sized maps.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

use engine_core::prelude::*;
use engine_world::{Ground, Water};

use crate::world_model::TacticalWorld;

/// Cells per chunk side.
const CHUNK: i32 = 32;

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
}

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
            self.chunks.clear();
            self.generation = world.generation;
        }
        NavCtx {
            grid: self,
            world,
            ground,
            water,
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

impl NavCtx<'_> {
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
        let s = self.grid.settings;
        let margin = (s.agent_radius / s.cell).ceil() as i32;
        let side = CHUNK + 2 * margin;
        let (cx0, cz0) = (key.0 * CHUNK - margin, key.1 * CHUNK - margin);
        let mut heights = vec![0.0f64; (side * side) as usize];
        let mut wet = vec![false; (side * side) as usize];
        let mut scratch = std::mem::take(&mut self.grid.scratch);
        for lz in 0..side {
            for lx in 0..side {
                let (x, z) = self.cell_center(cx0 + lx, cz0 + lz);
                let (h, _) = self.world.stacked_height_scratch(
                    x,
                    z,
                    s.agent_height,
                    self.ground,
                    &mut scratch,
                );
                let i = (lz * side + lx) as usize;
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
                let o = (lz * CHUNK + lx) as usize;
                chunk.h[o] = (h - base) as f32;
                chunk.walk[o] = ok;
            }
        }
        self.grid.chunks.insert(key, chunk);
        self.grid.chunks_built += 1;
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
        out.clear();
        let s = self.grid.settings;
        let start = self.cell_of(from);
        let goal = self.cell_of(to);
        // Most trips in open ground are a straight walk: skip the search for those.
        if let Some(h) = self.straight_walk(from, to, extra_cost) {
            out.push(DVec3::new(to.x, h, to.z));
            return PathStatus::Complete;
        }
        let win = self.window(start, goal, 24);
        let Some(mut si) = win.idx(start.0, start.1) else {
            return PathStatus::Failed;
        };
        if !win.walk[si] || (win.h[si] - from.y).abs() > 1.2 {
            // Standing next to a wall (inside the clearance margin): start from the nearest
            // walkable cell on the same level.
            let mut best: Option<(i32, usize)> = None;
            for dz in -4..=4 {
                for dx in -4..=4 {
                    if let Some(i) = win.idx(start.0 + dx, start.1 + dz)
                        && win.walk[i]
                        && (win.h[i] - from.y).abs() <= 1.2
                    {
                        let d2 = dx * dx + dz * dz;
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
        let (gx, gz) = ((gi as i32) % win.w, (gi as i32) / win.w);
        let heuristic = |i: usize| {
            let (x, z) = ((i as i32) % win.w, (i as i32) / win.w);
            let (dx, dz) = ((x - gx).abs() as f32, (z - gz).abs() as f32);
            (dx.max(dz) + 0.4142 * dx.min(dz)) * s.cell
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
                let cost = g[i] + (step + climb) * (1.0 + extra[j]);
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
            PathStatus::Complete
        } else {
            PathStatus::Partial
        }
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
        let g = ground.height(DVec3::new(x, 1.0e6, z)).unwrap_or(0.0);
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
        (h, g)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_model::{Obstacle, ObstacleShape};
    use engine_world::{WaterQuery, WaterSample};
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
}
