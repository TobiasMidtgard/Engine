//! Rivers: where the rain on a planet runs down to the sea.
//!
//! Drainage is solved once per shape, on coarse grids of its own heights: the whole planet in
//! cells of about 40 km (the big rivers, seen from orbit) and, around each pad, a square 240 km
//! across in cells of 600 m (the streams and rivers in view of a level). The sea rises from the
//! coasts (and, around a pad, from the square's edge) over the land, lowest ground first, and
//! each cell drains into the neighbor the water reached it from: pits fill up and spill over
//! their lowest rim, so every drop reaches the sea (priority flood, Barnes et al. 2014). The
//! area a cell drains, weighted by its rain, sets the width of the river through it.
//!
//! The rivers are smoothed polylines. As the shape is sampled they are carved into the ground
//! (a channel with a flat bed just below the water level, low banks, a flood plain, and valley
//! walls that steepen the higher the land stands above the river) and they meander by domain
//! warping. The terrain shader draws their water, and their banks are greener. The pads and
//! the hand-made ground around the home spot stay dry: while solving, a dome raised over them
//! turns the rivers around them.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use engine_core::math::DVec3;

use crate::cube::FACES;
use crate::noise;
use crate::shape::PlanetShape;

const NONE: u32 = u32::MAX;

/// Cell size of the planet-wide grid, meters (at most 256 cells along a cube face).
const GLOBAL_CELL_M: f64 = 40_000.0;
/// Cells across, and cell size in meters, of the square around each pad.
const REGION_CELLS: usize = 400;
const REGION_CELL_M: f64 = 600.0;

/// How one grid's rivers look.
struct Style {
    /// Drained area (km², weighted by rain) where a river begins.
    source_km2: f64,
    /// Half width of the channel: at the source, and growth with the square root of the
    /// drained area (km²), meters.
    base_half_m: f64,
    half_per_sqrt_km2: f64,
    /// How far beyond its flood plain a river's valley reaches, meters.
    valley_m: f64,
    /// Meanders: wavelength and amplitude of the warp bands, meters.
    warp: [(f64, f64); 3],
    /// A basin deeper than this (filled to its rim; plus 15 % of the rim's height, so high
    /// mountains hold few lakes) and at least `lake_cells` big holds a lake; the rivers cut
    /// through the rims of shallower ones. Meters.
    lake_depth_m: f64,
    lake_cells: usize,
    /// Unevenness added to the heights the water is routed over, meters: water on flats then
    /// wanders instead of running along the grid.
    route_noise_m: f64,
}

const PLANET: Style = Style {
    source_km2: 15_000.0,
    base_half_m: 20.0,
    half_per_sqrt_km2: 0.45,
    valley_m: 6_000.0,
    warp: [(120_000.0, 14_000.0), (16_000.0, 1_900.0), (3_000.0, 330.0)],
    lake_depth_m: 150.0,
    lake_cells: 2,
    route_noise_m: 60.0,
};

const REGION: Style = Style {
    source_km2: 6.0,
    base_half_m: 1.5,
    half_per_sqrt_km2: 1.0,
    valley_m: 1_500.0,
    warp: [(2_800.0, 330.0), (560.0, 66.0), (120.0, 12.0)],
    lake_depth_m: 30.0,
    lake_cells: 6,
    route_noise_m: 3.0,
};

/// One straight piece of a river.
#[derive(Debug, Clone, Copy)]
struct Segment {
    /// Ends, planet-local at the base radius, meters.
    a: DVec3,
    b: DVec3,
    /// Water level at the ends, meters above sea level.
    level: [f64; 2],
    /// Half width of the channel at the ends, meters.
    half: [f64; 2],
    /// How far from the river its valley reaches, meters.
    reach: f64,
    /// Across the segment (along the surface), and how far the river bows out that way
    /// between the ends (a meander), meters.
    side: DVec3,
    bow: f64,
}

/// A cell of a lake: the basin it lies in is filled to `level` (meters above sea level).
#[derive(Debug, Clone, Copy)]
struct LakeCell {
    /// Planet-local at the base radius, meters.
    center: DVec3,
    level: f64,
}

/// The rivers of one grid: the planet's, or a pad's.
#[derive(Debug)]
struct Network {
    segments: Vec<Segment>,
    /// Segments near each cube of space `bucket_m` across.
    buckets: HashMap<[i32; 3], Vec<u32>>,
    bucket_m: f64,
    lakes: Vec<LakeCell>,
    /// Lake cells near each bucket (same cubes as the segments').
    lake_buckets: HashMap<[i32; 3], Vec<u32>>,
    cell_m: f64,
    warp: [(f64, f64); 3],
    /// A pad's rivers fade out toward the edge of their square (center, from, to meters).
    region: Option<(DVec3, f64, f64)>,
    /// The planet's rivers give way to the pads' squares (center, from, to meters).
    holes: Vec<(DVec3, f64, f64)>,
}

/// The rivers of one planet shape (see the module docs).
#[derive(Debug, Default)]
pub struct Rivers {
    networks: Vec<Network>,
}

/// A straight piece of a river (see [`Rivers::segments`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RiverSegment {
    /// Upstream and downstream end, planet-local at the base radius, meters.
    pub a: DVec3,
    pub b: DVec3,
    /// Water level at the ends, meters above sea level.
    pub level_m: [f64; 2],
    /// Half width of the channel at the ends, meters.
    pub half_width_m: [f64; 2],
}

/// Rivers at one spot.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RiverHere {
    /// How far to lower the ground, meters.
    pub carve: f64,
    /// River water, 0..1 (a share of it where the channel is narrower than the detail asked
    /// for).
    pub water: f64,
    /// Nearness to a river, 0..1: wetter, greener ground.
    pub banks: f64,
    /// How far to raise the ground to the level of a lake's surface, meters.
    pub fill: f64,
    /// Lake water, 0..1 (the rest of [`Self::water`] is river).
    pub lake: f64,
}

impl Rivers {
    /// Solve the drainage of `shape` (sampled without rivers).
    pub(crate) fn build(shape: &PlanetShape) -> Rivers {
        let r = shape.radius_m;
        let domes = domes(shape);
        let half_square = REGION_CELLS as f64 * REGION_CELL_M * 0.5;
        let cells = ((r * FRAC_PI_2 / GLOBAL_CELL_M).round() as usize).clamp(16, 256);
        let holes = shape
            .pads
            .iter()
            .map(|p| (p.direction(), half_square * 0.6, half_square * 0.85))
            .collect();
        let mut networks = vec![solve(
            shape,
            &Grid::cube(cells, r),
            &PLANET,
            &domes,
            None,
            holes,
        )];
        for pad in &shape.pads {
            let c = pad.direction();
            networks.push(solve(
                shape,
                &Grid::square(c, r, REGION_CELLS, REGION_CELL_M),
                &REGION,
                &domes,
                Some((c, half_square * 0.7, half_square * 0.95)),
                Vec::new(),
            ));
        }
        Rivers { networks }
    }

    /// Every straight piece of every river (the planet's and the pads'), before the meanders
    /// (which move a river by up to a few percent of the piece's length).
    pub fn segments(&self) -> impl Iterator<Item = RiverSegment> + '_ {
        self.networks.iter().flat_map(|n| {
            n.segments.iter().map(|s| RiverSegment {
                a: s.a,
                b: s.b,
                level_m: s.level,
                half_width_m: s.half,
            })
        })
    }

    /// The rivers at planet-local point `p` (at the base radius `r`, direction `dir`) on ground
    /// `h` meters above sea level, with detail down to wavelength `finest`.
    pub(crate) fn at(
        &self,
        p: DVec3,
        dir: DVec3,
        h: f64,
        finest: f64,
        r: f64,
        seed: u64,
    ) -> RiverHere {
        let mut out = RiverHere::default();
        for net in &self.networks {
            let key = bucket(p, net.bucket_m);
            let weight = net.weight(dir, r);
            if weight <= 0.0 {
                continue;
            }
            // Lakes: the surface is flat at the basin's rim level, out to a ragged outline
            // around the basin's cells; within it the ground draws the shore where it rises
            // above the water.
            if let Some(list) = net.lake_buckets.get(&key) {
                let sigma = net.cell_m * 0.75;
                let (mut field, mut level, mut near) = (0.0, 0.0, f64::MAX);
                for &i in list {
                    let c = &net.lakes[i as usize];
                    let d = (p - c.center).length();
                    field += (-(d / sigma).powi(2)).exp();
                    if d < near {
                        (near, level) = (d, c.level);
                    }
                }
                if field > 0.2 {
                    let ragged = noise::fbm(p / (net.cell_m * 1.3), seed ^ 0x1A4E, 3, 0.5);
                    let within = smooth(0.4, 0.6, field + 0.35 * ragged) * weight;
                    if within > 0.0 && h < level {
                        let lake = within * smooth(0.0, 0.3, level - h);
                        out.fill = out.fill.max((level - h) * within);
                        out.lake = out.lake.max(lake);
                        out.water = out.water.max(lake);
                    }
                }
            }
            let Some(list) = net.buckets.get(&key) else {
                continue;
            };
            let q = p + net.warp(p, dir, finest, seed);
            for &i in list {
                let s = &net.segments[i as usize];
                let ab = s.b - s.a;
                let t = ((q - s.a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                let off = q - (s.a + ab * t);
                let across = off.dot(s.side);
                let rest = (off.length_squared() - across * across).max(0.0);
                let d = ((across - s.bow * (PI * t).sin()).powi(2) + rest).sqrt();
                if d >= s.reach {
                    continue;
                }
                let half = lerp(s.half[0], s.half[1], t) * weight;
                if half < 0.05 {
                    continue;
                }
                let level = lerp(s.level[0], s.level[1], t);
                // A channel narrower than the detail asked for is drawn wider and shallower,
                // and only partly as water, so it neither flickers nor vanishes from afar.
                let wide = half.max(finest * 0.6);
                let cover = half / wide;
                let depth = (0.3 + half * 0.01).min(2.5) * cover;
                let bank_w = 1.5 + half * 0.4;
                let bank_h = (0.5 + half * 0.012).min(4.0);
                let plain = wide + bank_w + half * 2.0 + 5.0;
                // Gentle valley sides in the lowlands, steep ones where the river cuts through
                // high ground.
                let wall = 0.1 + 0.45 * smooth(50.0, 1_200.0, h - level);
                let profile = level - depth
                    + (depth + bank_h) * smooth(wide * 0.8, wide + bank_w, d)
                    + wall * (d - plain).max(0.0);
                let fade = 1.0 - smooth(s.reach * 0.55, s.reach, d);
                out.carve = out.carve.max((h - profile).max(0.0) * fade * weight);
                out.water = out
                    .water
                    .max(cover * (1.0 - smooth(wide * 0.75, wide * 1.02, d)));
                // Trees line the banks: a strip a hundred meters or so wide along a stream, a
                // kilometer or two along a great river (thinner, not wider, where the vertices
                // are too far apart for it).
                let corridor = 30.0 + 2.0 * half + 25.0 * half.sqrt();
                let corridor_wide = corridor.max(finest * 0.6);
                let gauss = (-(d / corridor_wide).powi(2)).exp() * corridor / corridor_wide;
                out.banks = out.banks.max(smooth(0.5, 12.0, half) * gauss * weight);
            }
        }
        out
    }
}

impl Network {
    /// How much of this network holds at `dir` (0..1).
    fn weight(&self, dir: DVec3, r: f64) -> f64 {
        let mut w = 1.0;
        if let Some((c, from, to)) = self.region {
            w *= 1.0 - smooth(from, to, (dir - c).length() * r);
        }
        for &(c, from, to) in &self.holes {
            w *= smooth(from, to, (dir - c).length() * r);
        }
        w
    }

    /// Meanders: the point is moved along the surface by a few bands of noise (each fades
    /// out once the detail asked for is too coarse to show it).
    fn warp(&self, p: DVec3, dir: DVec3, finest: f64, seed: u64) -> DVec3 {
        let mut w = DVec3::ZERO;
        for (k, &(wavelength, amplitude)) in self.warp.iter().enumerate() {
            let fade = 1.0 - smooth(wavelength * 0.1, wavelength * 0.35, finest);
            if fade <= 0.0 {
                continue;
            }
            let q = p / wavelength;
            let s = seed ^ (0x7E1 + k as u64 * 0x3C5);
            let v = DVec3::new(
                noise::fbm(q, s, 2, 0.5),
                noise::fbm(q + 17.1, s ^ 0x1, 2, 0.5),
                noise::fbm(q - 23.7, s ^ 0x2, 2, 0.5),
            );
            w += v * (amplitude * fade);
        }
        w - dir * w.dot(dir)
    }
}

/// The bucket holding a point.
fn bucket(p: DVec3, size: f64) -> [i32; 3] {
    let q = (p / size).floor();
    [q.x as i32, q.y as i32, q.z as i32]
}

/// Ground kept dry, as (center, radius): the pads with their blends, and the hand-made ground
/// around the home spot.
fn domes(shape: &PlanetShape) -> Vec<(DVec3, f64)> {
    let mut out: Vec<(DVec3, f64)> = shape
        .pads
        .iter()
        .map(|p| (p.direction(), p.radius_m + p.blend_m * 3.0 + 3_000.0))
        .collect();
    if let Some(o) = &shape.overlay {
        // The overlay's frame puts the home spot straight above the planet's center.
        let c = (o.rotation.inverse() * DVec3::Y).normalize();
        out.push((c, o.radius_m + o.blend_m + 4_000.0));
    }
    out
}

/// Cells of a drainage grid on the sphere.
struct Grid {
    dirs: Vec<DVec3>,
    neighbors: Vec<[u32; 8]>,
    /// Cells on the edge of a grid that doesn't cover the planet: water drains off it there.
    edge: Vec<bool>,
    cell_m: f64,
}

impl Grid {
    /// The whole planet: `n` × `n` cells on each cube face (equal angles).
    fn cube(n: usize, r: f64) -> Grid {
        let angle = |k: f64| ((k + 0.5) / n as f64 * 2.0 - 1.0) * FRAC_PI_4;
        let dir_of = |f: usize, i: f64, j: f64| {
            let (c, u, v) = FACES[f];
            (c + u * angle(i).tan() + v * angle(j).tan()).normalize()
        };
        let cell_of = |d: DVec3| {
            let (f, &(c, u, v)) = FACES
                .iter()
                .enumerate()
                .max_by(|a, b| d.dot(a.1.0).total_cmp(&d.dot(b.1.0)))
                .expect("six faces");
            let k = d.dot(c);
            let to = |t: f64| {
                let a = t.atan() / FRAC_PI_4;
                (((a + 1.0) * 0.5 * n as f64).floor() as i64).clamp(0, n as i64 - 1) as usize
            };
            (f * n * n + to(d.dot(v) / k) * n + to(d.dot(u) / k)) as u32
        };
        let mut dirs = Vec::with_capacity(6 * n * n);
        let mut neighbors = Vec::with_capacity(6 * n * n);
        for f in 0..6 {
            for j in 0..n {
                for i in 0..n {
                    dirs.push(dir_of(f, i as f64, j as f64));
                    let me = (f * n * n + j * n + i) as u32;
                    let mut around = [NONE; 8];
                    let mut count = 0;
                    for (di, dj) in AROUND {
                        // Past the face's edge the extended face plane still points at the
                        // right spot on the neighboring face.
                        let k = cell_of(dir_of(f, i as f64 + di as f64, j as f64 + dj as f64));
                        if k != me && !around[..count].contains(&k) {
                            around[count] = k;
                            count += 1;
                        }
                    }
                    neighbors.push(around);
                }
            }
        }
        Grid {
            edge: vec![false; dirs.len()],
            dirs,
            neighbors,
            cell_m: r * FRAC_PI_2 / n as f64,
        }
    }

    /// A square `m` cells of `cell_m` across, centered on direction `c`.
    fn square(c: DVec3, r: f64, m: usize, cell_m: f64) -> Grid {
        let east = DVec3::Y.cross(c).try_normalize().unwrap_or(DVec3::X);
        let north = c.cross(east);
        let mid = (m as f64 - 1.0) * 0.5;
        let mut dirs = Vec::with_capacity(m * m);
        let mut neighbors = Vec::with_capacity(m * m);
        let mut edge = Vec::with_capacity(m * m);
        for j in 0..m {
            for i in 0..m {
                let (x, y) = ((i as f64 - mid) * cell_m, (j as f64 - mid) * cell_m);
                dirs.push((c * r + east * x + north * y).normalize());
                let mut around = [NONE; 8];
                for (k, (di, dj)) in AROUND.iter().enumerate() {
                    let (a, b) = (i as i64 + di, j as i64 + dj);
                    if (0..m as i64).contains(&a) && (0..m as i64).contains(&b) {
                        around[k] = (b as usize * m + a as usize) as u32;
                    }
                }
                neighbors.push(around);
                edge.push(i == 0 || j == 0 || i == m - 1 || j == m - 1);
            }
        }
        Grid {
            dirs,
            neighbors,
            edge,
            cell_m,
        }
    }
}

const AROUND: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (-1, 1),
    (1, -1),
    (-1, -1),
];

/// A cell waiting in the flood, lowest first (ties: lower index first).
#[derive(PartialEq)]
struct Flood(f64, u32);

impl Eq for Flood {}

impl Ord for Flood {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}

impl PartialOrd for Flood {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Share of the rain that runs off, by moisture (deserts give almost none).
fn runoff(moisture: f32) -> f64 {
    (moisture as f64 / 0.55).powi(2).clamp(0.03, 2.5)
}

/// Solve one grid's drainage and lay out its rivers.
fn solve(
    shape: &PlanetShape,
    grid: &Grid,
    style: &Style,
    domes: &[(DVec3, f64)],
    region: Option<(DVec3, f64, f64)>,
    holes: Vec<(DVec3, f64, f64)>,
) -> Network {
    let r = shape.radius_m;
    let n = grid.dirs.len();
    let sampled = par_map(n, |k| {
        let s = shape.sample_with(grid.dirs[k], grid.cell_m * 2.0, None);
        (s.height, s.moisture)
    });
    let mut height: Vec<f64> = sampled.iter().map(|s| s.0).collect();
    for (k, h) in height.iter_mut().enumerate() {
        if *h > 0.0 {
            let q = grid.dirs[k] * (r / (grid.cell_m * 5.0));
            *h = (*h + style.route_noise_m * noise::fbm(q, shape.seed ^ 0x99, 2, 0.5)).max(0.1);
        }
    }
    let mut dry = vec![false; n];
    for (k, h) in height.iter_mut().enumerate() {
        for &(c, radius) in domes {
            let d = (grid.dirs[k] - c).length() * r;
            if d < radius {
                *h = h.max(0.0) + 4_000.0 * (1.0 - d / radius);
                dry[k] |= d < radius * 0.85;
            }
        }
    }
    let sea: Vec<bool> = height.iter().map(|&h| shape.ocean && h < 0.0).collect();

    // The flood, from the sea and the grid's edge.
    let mut parent = vec![NONE; n];
    let mut filled = height.clone();
    let mut seen = vec![false; n];
    let mut heap = BinaryHeap::new();
    for k in 0..n {
        if sea[k] || grid.edge[k] {
            seen[k] = true;
            heap.push(Flood(filled[k], k as u32));
        }
    }
    // Land cells, each after the cell it drains into.
    let mut order = Vec::with_capacity(n);
    while let Some(Flood(level, k)) = heap.pop() {
        if !sea[k as usize] {
            order.push(k);
        }
        for &m in &grid.neighbors[k as usize] {
            if m == NONE || seen[m as usize] {
                continue;
            }
            let m = m as usize;
            seen[m] = true;
            filled[m] = height[m].max(level + 0.01);
            parent[m] = k;
            heap.push(Flood(filled[m], m as u32));
        }
    }

    // Drained area, and the biggest stream flowing into each cell.
    let cell_km2 = (grid.cell_m * 0.001).powi(2);
    let mut flow: Vec<f64> = (0..n)
        .map(|k| {
            if sea[k] {
                0.0
            } else {
                cell_km2 * runoff(sampled[k].1)
            }
        })
        .collect();
    let mut main = vec![NONE; n];
    for &k in order.iter().rev() {
        let p = parent[k as usize];
        if p == NONE || sea[p as usize] {
            continue;
        }
        let p = p as usize;
        flow[p] += flow[k as usize];
        if main[p] == NONE || flow[k as usize] > flow[main[p] as usize] {
            main[p] = k;
        }
    }

    // Basins: cells the flood filled above their ground. A big, deep one holds a lake (unless
    // the land is too dry): the rivers end at its shore and start again where it spills over
    // its rim.
    let inundated: Vec<bool> = (0..n)
        .map(|k| !sea[k] && filled[k] - height[k] > 0.5)
        .collect();
    let mut lake = vec![false; n];
    let mut basin = vec![NONE; n];
    for start in 0..n {
        if !inundated[start] || basin[start] != NONE {
            continue;
        }
        let mut cells = vec![start];
        basin[start] = start as u32;
        let (mut deepest, mut i, mut wet) = (0.0f64, 0, 0.0f64);
        let rim = filled[start].max(0.0);
        while i < cells.len() {
            let k = cells[i];
            i += 1;
            deepest = deepest.max(filled[k] - height[k]);
            wet += sampled[k].1 as f64;
            for &m in &grid.neighbors[k] {
                if m != NONE && inundated[m as usize] && basin[m as usize] == NONE {
                    basin[m as usize] = start as u32;
                    cells.push(m as usize);
                }
            }
        }
        if deepest > style.lake_depth_m + 0.15 * rim
            && cells.len() >= style.lake_cells
            && wet / cells.len() as f64 > 0.2
        {
            for k in cells {
                lake[k] = !dry[k];
            }
        }
    }
    let lake_cells: Vec<LakeCell> = (0..n)
        .filter(|&k| lake[k])
        .map(|k| LakeCell {
            center: grid.dirs[k] * r,
            level: filled[k],
        })
        .collect();

    // Water levels: a river cuts down through the rim of a shallower basin, to the lowest
    // ground upstream but no deeper than `lake_depth_m` below the flood's level (both fall
    // downstream, so the river does too); a lake stays at its rim's level.
    let mut level: Vec<f64> = (0..n)
        .map(|k| if lake[k] { filled[k] } else { height[k] })
        .collect();
    for &k in order.iter().rev() {
        let p = parent[k as usize];
        if p != NONE && !sea[p as usize] {
            level[p as usize] = level[p as usize].min(level[k as usize] - 0.01);
        }
    }
    for k in 0..n {
        level[k] = level[k].max(filled[k] - style.lake_depth_m);
    }

    let river = |k: u32| k != NONE && !sea[k as usize] && flow[k as usize] >= style.source_km2;
    let half = |k: usize| style.base_half_m + style.half_per_sqrt_km2 * flow[k].sqrt();

    // Nodes: cell centers moved about within their cells, then smoothed along the river.
    let mut pos = grid.dirs.clone();
    let jitter = 0.6 * grid.cell_m / r;
    for &k in &order {
        if river(k) {
            let k = k as usize;
            let d = grid.dirs[k];
            let t1 = d.any_orthonormal_vector();
            let t2 = d.cross(t1);
            let rnd = |salt| noise::cell_random(k as i64, 0, 0, shape.seed, salt) - 0.5;
            pos[k] = d + (t1 * rnd(0x51) + t2 * rnd(0x52)) * jitter;
        }
    }
    for _ in 0..2 {
        let prev = pos.clone();
        for &k in &order {
            let p = parent[k as usize];
            if !river(k) || !river(p) {
                continue;
            }
            let (k, p, m) = (k as usize, p as usize, main[k as usize]);
            pos[k] = if river(m) {
                prev[k] * 0.5 + prev[p] * 0.25 + prev[m as usize] * 0.25
            } else {
                prev[k] * 0.7 + prev[p] * 0.3
            };
        }
    }

    // Steps from the sea along the river: meanders swing left and right in turn, so the bends
    // run smoothly into each other.
    let mut steps = vec![0u32; n];
    for &k in &order {
        let p = parent[k as usize];
        if p != NONE && !sea[p as usize] {
            steps[k as usize] = steps[p as usize] + 1;
        }
    }

    // Segments from each river cell to the cell it drains into; at the coast, on out to sea.
    let mut segments = Vec::new();
    for &k in &order {
        if !river(k) || dry[k as usize] || lake[k as usize] {
            continue;
        }
        let k = k as usize;
        let p = parent[k];
        let at = |v: DVec3| v.normalize() * r;
        let start = if river(main[k]) {
            half(k)
        } else {
            half(k) * 0.3
        };
        let seg = if p != NONE && sea[p as usize] {
            let mouth = pos[k] + (grid.dirs[p as usize] - pos[k]) * 1.1;
            Segment {
                a: at(pos[k]),
                b: at(mouth),
                level: [level[k], -12.0],
                half: [start, half(k) * 1.3],
                reach: 0.0,
                side: DVec3::ZERO,
                bow: 0.0,
            }
        } else if river(p) && !dry[p as usize] {
            // (Into a lake too: the river ends one cell in.)
            let p = p as usize;
            // Rivers meander on flat ground, and run straighter down steep slopes.
            let len = (pos[p] - pos[k]).length() * r;
            let slope = (level[k] - level[p]).max(0.0) / len.max(1.0);
            let swing = if steps[k].is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let amount = 0.12 + 0.18 * noise::cell_random(k as i64, 1, 0, shape.seed, 0x53);
            Segment {
                a: at(pos[k]),
                b: at(pos[p]),
                level: [level[k], level[p]],
                half: [
                    start,
                    if main[p] == k as u32 {
                        half(p)
                    } else {
                        half(k)
                    },
                ],
                reach: 0.0,
                side: DVec3::ZERO,
                bow: swing * len * amount * (1.0 - smooth(0.004, 0.04, slope)),
            }
        } else {
            continue;
        };
        let widest = seg.half[0].max(seg.half[1]);
        segments.push(Segment {
            reach: widest * 4.0 + 30.0 + style.valley_m,
            side: (seg.a + seg.b)
                .normalize()
                .cross(seg.b - seg.a)
                .normalize_or_zero(),
            ..seg
        });
    }

    // Buckets: each segment goes into every one its valley (and the meanders) may reach.
    let warp_sum: f64 = style.warp.iter().map(|w| w.1).sum();
    let bucket_m = grid.cell_m * 1.5 + style.valley_m;
    let mut buckets: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    for (i, s) in segments.iter().enumerate() {
        let margin = DVec3::splat(s.reach + warp_sum + s.bow.abs());
        let lo = bucket(s.a.min(s.b) - margin, bucket_m);
        let hi = bucket(s.a.max(s.b) + margin, bucket_m);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    buckets.entry([x, y, z]).or_default().push(i as u32);
                }
            }
        }
    }
    let mut lake_buckets: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    for (i, c) in lake_cells.iter().enumerate() {
        let margin = DVec3::splat(grid.cell_m * 1.6);
        let lo = bucket(c.center - margin, bucket_m);
        let hi = bucket(c.center + margin, bucket_m);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    lake_buckets.entry([x, y, z]).or_default().push(i as u32);
                }
            }
        }
    }
    Network {
        segments,
        buckets,
        bucket_m,
        lakes: lake_cells,
        lake_buckets,
        cell_m: grid.cell_m,
        warp: style.warp,
        region,
        holes,
    }
}

/// `f(0..n)` on all cores, in order.
fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |t| t.get())
        .clamp(1, 8);
    let chunk = n.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let f = &f;
        let parts: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|start| {
                scope.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        parts
            .into_iter()
            .flat_map(|h| h.join().expect("river sampling panicked"))
            .collect()
    })
}

/// The rivers solved for a shape, shared by its clones. Solved rivers are also kept for the
/// whole program (a few shapes' worth), so an identical shape built again doesn't solve them
/// again; a shape changed after it was sampled finds its own by [`PlanetShape::river_key`].
#[derive(Clone, Default)]
pub struct RiverCache(Arc<OnceLock<(u64, Arc<Rivers>)>>);

static SOLVED: RwLock<Vec<(u64, Arc<Rivers>)>> = RwLock::new(Vec::new());
static SOLVING: Mutex<()> = Mutex::new(());

impl RiverCache {
    /// The rivers for `key`, solving them with `build` the first time.
    pub(crate) fn get(&self, key: u64, build: impl FnOnce() -> Rivers) -> Arc<Rivers> {
        if let Some((k, r)) = self.0.get()
            && *k == key
        {
            return r.clone();
        }
        let rivers = solved(key).unwrap_or_else(|| {
            let _one_at_a_time = SOLVING.lock().unwrap_or_else(|e| e.into_inner());
            solved(key).unwrap_or_else(|| {
                let rivers = Arc::new(build());
                let mut all = SOLVED.write().unwrap_or_else(|e| e.into_inner());
                if all.len() >= 6 {
                    all.remove(0);
                }
                all.push((key, rivers.clone()));
                rivers
            })
        });
        let _ = self.0.set((key, rivers.clone()));
        rivers
    }
}

fn solved(key: u64) -> Option<Arc<Rivers>> {
    let all = SOLVED.read().unwrap_or_else(|e| e.into_inner());
    all.iter().find(|(k, _)| *k == key).map(|(_, r)| r.clone())
}

impl std::fmt::Debug for RiverCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RiverCache(..)")
    }
}

/// A cache is not part of the shape's value.
impl PartialEq for RiverCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::FlatPad;

    fn earth() -> PlanetShape {
        PlanetShape::earthlike(7).with_pad(FlatPad::default())
    }

    #[test]
    fn rivers_run_downhill_to_the_sea() {
        let shape = earth();
        let rivers = shape
            .river_network()
            .expect("an Earth-like world has rivers");
        for net in &rivers.networks {
            assert!(net.segments.len() > 100, "{} segments", net.segments.len());
            for s in &net.segments {
                assert!(s.level[1] <= s.level[0], "uphill: {:?}", s.level);
                assert!(s.half[0] > 0.0 && s.half[1] > 0.0);
            }
        }
        let mouths = rivers.segments().filter(|s| s.level_m[1] < 0.0).count();
        assert!(mouths > 20, "{mouths} river mouths");
        // The moon and Mars have none.
        assert!(PlanetShape::moon(7).river_network().is_none());
        assert!(PlanetShape::mars(7).river_network().is_none());
    }

    #[test]
    fn a_river_is_water_in_a_channel_below_its_banks() {
        let shape = earth();
        let rivers = shape.river_network().unwrap();
        let pad = shape.pads[0].direction();
        let r = shape.radius_m;
        // Rivers of a fair size on land near the pad.
        let mut checked = 0;
        for s in rivers.segments() {
            let mid = (s.a + s.b) * 0.5;
            let near = (mid.normalize() - pad).length() * r;
            if !(9_000.0..40_000.0).contains(&near)
                || s.half_width_m[0] < 8.0
                || s.level_m[1] < 20.0
            {
                continue;
            }
            let along = (s.b - s.a).normalize();
            let across = mid.normalize().cross(along);
            let transect: Vec<_> = (-700..=700)
                .step_by(2)
                .map(|k| shape.sample((mid + across * k as f64).normalize(), 0.0))
                .collect();
            let wet: Vec<_> = transect.iter().filter(|t| t.water > 0.9).collect();
            if wet.is_empty() {
                continue;
            }
            let water_level = wet.iter().map(|t| t.height).fold(f64::MIN, f64::max);
            let ground = transect
                .iter()
                .filter(|t| t.water < 0.01)
                .map(|t| t.height)
                .fold(f64::MIN, f64::max);
            assert!(
                ground > water_level + 0.5,
                "banks {ground:.1} m over water at {water_level:.1} m"
            );
            assert!(wet.iter().all(|t| t.height <= s.level_m[0] + 0.5));
            checked += 1;
            if checked >= 5 {
                break;
            }
        }
        assert!(checked >= 3, "only {checked} rivers checked");
    }

    #[test]
    fn the_home_ground_stays_dry() {
        let shape = earth();
        let pad = shape.pads[0];
        let c = pad.direction();
        let east = DVec3::Y.cross(c).normalize();
        let north = c.cross(east);
        let keep = pad.radius_m + pad.blend_m * 3.0;
        for i in 0..64 {
            let a = i as f64 * std::f64::consts::TAU / 64.0;
            for k in 0..20 {
                let d = keep * k as f64 / 19.0;
                let p = c * shape.radius_m + (east * a.cos() + north * a.sin()) * d;
                let s = shape.sample(p.normalize(), 0.0);
                assert_eq!(s.water, 0.0, "water {d:.0} m from the pad");
            }
        }
    }

    #[test]
    fn rivers_follow_the_shape_when_it_changes() {
        let a = earth();
        let first = a.river_network().unwrap();
        let mut b = a.clone();
        b.pads[0].sea_azimuth_deg = 20.0;
        let second = b.river_network().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        // Clones and rebuilt identical shapes share the solve.
        assert!(Arc::ptr_eq(&first, &earth().river_network().unwrap()));
    }
}
