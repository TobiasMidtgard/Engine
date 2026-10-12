//! Flowing water: streams, rivers, waterfalls, floods and draining basins.
//!
//! A [`FlowRegion`] is a shallow-water simulation on a grid laid over a riverbed (the "virtual
//! pipes" method: each cell exchanges water with its four neighbors through pipes whose flow
//! accelerates with the difference in water level). Water runs downhill, pools in hollows,
//! spills over rims, speeds up on steep drops (waterfalls and rapids) and drains away through
//! open edges. Where a region overlaps a still [`crate::WaterBody`] the lake acts as a reservoir:
//! a notch in its rim becomes an outlet stream, and streams that reach it are absorbed.
//!
//! It is cheap (a few float operations per cell per step, no pressure solve), stable for any
//! bed shape, and conserves water exactly apart from sources, drains and losses.

use engine_core::math::{DVec2, DVec3, Vec2};
use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::body::WaterOptics;

/// Where a [`FlowRegion`]'s bed comes from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum FlowBed {
    /// Sample the terrain (`engine_world::Ground`) when the region starts.
    #[default]
    Ground,
    /// Explicit bed heights, meters above the region's origin, row-major (`cells[0]` per row,
    /// `cells[1]` rows, rows along +z).
    Heights(Vec<f32>),
}

/// Water added to a [`FlowRegion`]: a spring, a pipe, a tipped bucket, a magic fountain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowSource {
    /// Position in meters from the region's origin corner (x, z).
    pub at: [f32; 2],
    pub radius_m: f32,
    /// Volume added per second, m³/s (a garden hose ≈ 0.0003, a stream ≈ 0.2, a river ≈ 50).
    /// Negative is a drain (a culvert, a sinkhole, a pump): it takes up to that much of
    /// whatever water reaches it.
    pub rate_m3_s: f32,
}

impl Default for FlowSource {
    fn default() -> Self {
        Self {
            at: [0.0, 0.0],
            radius_m: 1.0,
            rate_m3_s: 0.1,
        }
    }
}

/// Standing water a [`FlowRegion`] starts with: the basin containing `at` (meters from the
/// region's origin corner, x and z) filled up to `level` (meters above the origin), or with
/// `to_spill`, filled to the brim: up to where it would overflow (a stream's pools and hollows
/// as the flowing stream leaves them). Spilling over the region's edge counts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowFill {
    pub at: [f32; 2],
    pub level: f32,
    pub to_spill: bool,
}

impl FlowFill {
    /// The hollow around `at` (if it is in one) filled to the brim.
    pub fn to_spill(at: [f32; 2]) -> Self {
        Self {
            at,
            level: 0.0,
            to_spill: true,
        }
    }
}

impl Default for FlowFill {
    fn default() -> Self {
        Self {
            at: [0.0, 0.0],
            level: 1.0,
            to_spill: false,
        }
    }
}

/// The level the hollow around cell `seed` holds before it spills: climb out of it lowest
/// cell first, until the next cell is lower than the water (the way out) or on the edge.
fn spill_level(bed: &[f32], nx: usize, nz: usize, seed: usize) -> f32 {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    #[derive(PartialEq)]
    struct Cell(f32, usize);
    impl Eq for Cell {}
    impl PartialOrd for Cell {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Cell {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&o.0).then(self.1.cmp(&o.1))
        }
    }

    let mut level = bed[seed];
    let mut seen = vec![false; nx * nz];
    let mut heap = BinaryHeap::new();
    heap.push(Reverse(Cell(bed[seed], seed)));
    seen[seed] = true;
    while let Some(Reverse(Cell(b, idx))) = heap.pop() {
        if b < level - 1e-4 {
            break;
        }
        level = level.max(b);
        let (x, z) = (idx % nx, idx / nx);
        if x == 0 || z == 0 || x + 1 == nx || z + 1 == nz {
            break;
        }
        for n in [idx - 1, idx + 1, idx - nx, idx + nx] {
            if !seen[n] {
                seen[n] = true;
                heap.push(Reverse(Cell(bed[n], n)));
            }
        }
    }
    level
}

/// Depth per cell after flooding from each fill's seed cell to its level (4-connected: water
/// stops at any bed higher than the level, so only the basin around the seed fills).
pub fn flood(bed: &[f32], nx: usize, nz: usize, cell: f32, fills: &[FlowFill]) -> Vec<f32> {
    let mut depth = vec![0.0f32; nx * nz];
    for f in fills {
        let (i, j) = ((f.at[0] / cell).floor(), (f.at[1] / cell).floor());
        if i < 0.0 || j < 0.0 || i as usize >= nx || j as usize >= nz {
            continue;
        }
        let seed = j as usize * nx + i as usize;
        let level = if f.to_spill {
            spill_level(bed, nx, nz, seed)
        } else {
            f.level
        };
        let mut seen = vec![false; nx * nz];
        let mut stack = vec![seed];
        while let Some(idx) = stack.pop() {
            if seen[idx] || bed[idx] >= level {
                continue;
            }
            seen[idx] = true;
            depth[idx] = depth[idx].max(level - bed[idx]);
            let (x, z) = (idx % nx, idx / nx);
            if x > 0 {
                stack.push(idx - 1);
            }
            if x + 1 < nx {
                stack.push(idx + 1);
            }
            if z > 0 {
                stack.push(idx - nx);
            }
            if z + 1 < nz {
                stack.push(idx + nx);
            }
        }
    }
    depth
}

/// A patch of ground where water flows (see the [module docs](self)).
///
/// The entity's position is the grid's corner (cell (0, 0)); cells run along the entity's +x
/// and +z (the grid turns with the entity's yaw).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct FlowRegion {
    /// Cells along x and z.
    pub cells: [u32; 2],
    /// Cell size, meters.
    pub cell_m: f32,
    pub bed: FlowBed,
    pub sources: Vec<FlowSource>,
    /// Water leaves through the region's edges (a river continuing out of view). Off = walls.
    pub open_edges: bool,
    /// Water lost to soil and evaporation, meters of depth per second.
    pub loss_m_s: f32,
    /// Bed friction (0 ice-smooth .. 1 boulder-strewn): higher is slower and calmer.
    pub friction: f32,
    /// Water already standing at the start: a full pond, a reservoir behind a dam ready to
    /// break. Each fill floods the basin around its point up to its level.
    pub fills: Vec<FlowFill>,
    /// Seconds of flow to simulate before the first frame, so a stream already runs when the
    /// player arrives.
    pub prewarm_s: f32,
    pub optics: WaterOptics,
    pub visible: bool,
}

impl Default for FlowRegion {
    fn default() -> Self {
        Self {
            cells: [64, 64],
            cell_m: 0.5,
            bed: FlowBed::Ground,
            sources: Vec::new(),
            open_edges: true,
            loss_m_s: 0.0,
            friction: 0.3,
            fills: Vec::new(),
            prewarm_s: 0.0,
            optics: WaterOptics::stream(),
            visible: true,
        }
    }
}

/// Simulation state of one [`FlowRegion`].
#[derive(Debug, Clone)]
pub struct FlowSim {
    pub entity: Entity,
    /// World position of the corner of cell (0, 0); heights are relative to its y.
    pub origin: DVec3,
    /// Turn of the grid about +Y, radians (the entity's yaw): cells run along the turned x/z.
    pub yaw: f32,
    pub nx: usize,
    pub nz: usize,
    pub cell: f32,
    pub bed: Vec<f32>,
    pub depth: Vec<f32>,
    /// Outflow to −x, +x, −z, +z, m³/s.
    pub flux: Vec<[f32; 4]>,
    /// `flux` averaged over the last [`STEADY_TIME`] or so: the steady part of the flow,
    /// which damping leaves alone.
    pub steady: Vec<[f32; 4]>,
    /// Depth-averaged velocity (x, z), m/s.
    pub vel: Vec<Vec2>,
    /// Fixed water level per cell from overlapping still water (lakes), or NaN.
    pub reservoir: Vec<f32>,
    pub open_edges: bool,
    pub friction: f32,
    pub loss: f32,
    pub sources: Vec<FlowSource>,
    pub optics: WaterOptics,
    pub visible: bool,
    /// Bumped every step (GPU upload).
    pub version: u64,
    /// Water that left through edges and into reservoirs, m³ (diagnostics).
    pub drained_m3: f64,
}

impl FlowSim {
    /// A region with `bed` heights (relative to `origin.y`).
    pub fn new(entity: Entity, origin: DVec3, region: &FlowRegion, bed: Vec<f32>) -> Self {
        let nx = region.cells[0].max(2) as usize;
        let nz = region.cells[1].max(2) as usize;
        let mut bed = bed;
        bed.resize(nx * nz, 0.0);
        let depth = flood(&bed, nx, nz, region.cell_m.max(0.05), &region.fills);
        Self {
            entity,
            origin,
            yaw: 0.0,
            nx,
            nz,
            cell: region.cell_m.max(0.05),
            bed,
            depth,
            flux: vec![[0.0; 4]; nx * nz],
            steady: vec![[0.0; 4]; nx * nz],
            vel: vec![Vec2::ZERO; nx * nz],
            reservoir: vec![f32::NAN; nx * nz],
            open_edges: region.open_edges,
            friction: region.friction,
            loss: region.loss_m_s,
            sources: region.sources.clone(),
            optics: region.optics,
            visible: region.visible,
            version: 0,
            drained_m3: 0.0,
        }
    }

    /// Update the tunable parts from the component (sources, friction, look).
    pub fn sync(&mut self, region: &FlowRegion) {
        self.open_edges = region.open_edges;
        self.friction = region.friction;
        self.loss = region.loss_m_s;
        self.sources.clone_from(&region.sources);
        self.optics = region.optics;
        self.visible = region.visible;
    }

    /// World x/z of a cell center.
    pub fn cell_center(&self, i: usize, j: usize) -> DVec3 {
        let c = self.cell as f64;
        self.world(DVec2::new((i as f64 + 0.5) * c, (j as f64 + 0.5) * c), 0.0)
    }

    /// Offset of world `p` from the corner along the grid's axes (x, z), meters.
    pub fn local(&self, p: DVec3) -> DVec2 {
        grid_local(self.origin, self.yaw, p)
    }

    /// The point `local` meters along the grid's axes from the corner, `y` above it.
    pub fn world(&self, local: DVec2, y: f64) -> DVec3 {
        grid_point(self.origin, self.yaw, local, y)
    }

    /// A horizontal vector along the grid's axes (a velocity) in world x/z.
    pub fn world_dir(&self, v: Vec2) -> Vec2 {
        let (s, c) = self.yaw.sin_cos();
        Vec2::new(c * v.x + s * v.y, -s * v.x + c * v.y)
    }

    /// Follow a world shift. The water stays where it was on the ground: the grid's corner
    /// moves, and it turns with the world about +Y. Its cells hold heights above the corner and
    /// velocities along its own axes, so nothing in them changes. (A shift that tilts +Y can't
    /// be followed by a level grid; the corner still lands in the right place.)
    pub fn shift(&mut self, s: &WorldShift) {
        self.origin = s.point(self.origin);
        self.yaw = turned_yaw(s, self.yaw);
        self.version += 1;
    }

    /// Where water falls over a drop: (world position at the foot, horizontal velocity of the
    /// falling water m/s, power ρgQΔh in watts), one entry per cluster of nearby falling
    /// cells. Drops under `min_drop` meters are ignored.
    pub fn plunges(&self, min_drop: f32) -> Vec<(DVec3, Vec2, f32)> {
        // Cells cluster into buckets of 8 × 8, so a wide fall is one plunge, two falls are two.
        const BUCKET: usize = 8;
        let mut buckets: Vec<(usize, DVec3, Vec2, f32)> = Vec::new();
        let level = |idx: usize| self.bed[idx] + self.depth[idx];
        for j in 0..self.nz {
            for i in 0..self.nx {
                let idx = j * self.nx + i;
                // Water racing over a lip is thin: any water moving counts.
                if self.depth[idx] < 0.0005 || self.reservoir[idx].is_finite() {
                    continue;
                }
                let out = self.flux[idx];
                for (k, (di, dj)) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)]
                    .into_iter()
                    .enumerate()
                {
                    let (ni, nj) = (i as i64 + di, j as i64 + dj);
                    if ni < 0
                        || nj < 0
                        || ni >= self.nx as i64
                        || nj >= self.nz as i64
                        || out[k] <= 1e-4
                    {
                        continue;
                    }
                    let n = nj as usize * self.nx + ni as usize;
                    let drop = level(idx) - level(n);
                    if drop < min_drop {
                        continue;
                    }
                    let power = 1000.0 * 9.81 * out[k] * drop;
                    let foot =
                        self.cell_center(ni as usize, nj as usize) + DVec3::Y * level(n) as f64;
                    let key =
                        (nj as usize / BUCKET) * self.nx.div_ceil(BUCKET) + ni as usize / BUCKET;
                    let dir = Vec2::new(di as f32, dj as f32);
                    let v = dir * (self.vel[idx].dot(dir)).max(0.5);
                    match buckets.iter_mut().find(|b| b.0 == key) {
                        Some(b) => {
                            b.1 += foot * power as f64;
                            b.2 += v * power;
                            b.3 += power;
                        }
                        None => buckets.push((key, foot * power as f64, v * power, power)),
                    }
                }
            }
        }
        buckets
            .into_iter()
            .map(|(_, p, v, w)| (p / w as f64, self.world_dir(v / w), w))
            .collect()
    }

    /// Total water in the region, m³.
    pub fn volume(&self) -> f64 {
        let a = (self.cell * self.cell) as f64;
        self.depth.iter().map(|d| *d as f64 * a).sum()
    }

    /// Advance `dt` seconds (sub-stepped for stability).
    pub fn step(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let max_d = self.depth.iter().copied().fold(0.05f32, f32::max);
        let max_dt = 0.3 * self.cell / (9.81 * max_d).sqrt();
        let n = ((dt / max_dt).ceil() as u32).clamp(1, 16);
        let sdt = dt / n as f32;
        for _ in 0..n {
            self.substep(sdt);
        }
        self.version += 1;
    }

    fn substep(&mut self, dt: f32) {
        let (nx, nz) = (self.nx, self.nz);
        let l = self.cell;
        let area = l * l;
        let g = 9.81;
        // Reservoirs pin the level (lakes feed outlets and swallow inflow).
        for idx in 0..nx * nz {
            let r = self.reservoir[idx];
            if r.is_finite() {
                let want = (r - self.bed[idx]).max(0.0);
                self.drained_m3 += ((self.depth[idx] - want) * area) as f64;
                self.depth[idx] = want;
            }
        }
        // Sources: spread each one's volume evenly over the cells it covers.
        for s in &self.sources {
            let r = s.radius_m.max(l * 0.5);
            let (ci, cj) = ((s.at[0] / l) as i64, (s.at[1] / l) as i64);
            let reach = (r / l).ceil() as i64;
            let covered = |i: i64, j: i64| {
                let dx = (i as f32 + 0.5) * l - s.at[0];
                let dz = (j as f32 + 0.5) * l - s.at[1];
                dx * dx + dz * dz <= r * r
            };
            let range_j = (cj - reach).max(0)..=(cj + reach).min(nz as i64 - 1);
            let range_i = (ci - reach).max(0)..=(ci + reach).min(nx as i64 - 1);
            let mut count = 0;
            for j in range_j.clone() {
                for i in range_i.clone() {
                    count += covered(i, j) as u32;
                }
            }
            if count == 0 {
                continue;
            }
            let add = s.rate_m3_s * dt / (count as f32 * area);
            for j in range_j {
                for i in range_i.clone() {
                    if covered(i, j) {
                        let d = &mut self.depth[j as usize * nx + i as usize];
                        *d = (*d + add).max(0.0);
                    }
                }
            }
        }
        // Pipe flow: each pipe's cross-section is the water in the cell it drains (so a thin
        // film carries little), accelerated by the level difference and slowed by bed friction
        // (Manning: a steady stream settles at v = d^(2/3) S^(1/2) / n). A light damping calms
        // sloshing: it acts on how the flow differs from its recent average, so it settles
        // pools without holding back a steady stream.
        let keep = (1.0 - STILL_DAMPING * dt).clamp(0.0, 1.0);
        let follow = (dt / STEADY_TIME).min(1.0);
        let n2 = manning(self.friction).powi(2);
        let surface = |idx: usize, s: &Self| s.bed[idx] + s.depth[idx];
        for j in 0..nz {
            for i in 0..nx {
                let idx = j * nx + i;
                let h = surface(idx, self);
                let d = self.depth[idx];
                let neighbor = |ii: Option<usize>, jj: Option<usize>| match (ii, jj) {
                    (Some(ii), Some(jj)) if ii < nx && jj < nz => Some(surface(jj * nx + ii, self)),
                    _ => None,
                };
                let ns = [
                    neighbor(i.checked_sub(1), Some(j)),
                    neighbor(Some(i + 1), Some(j)),
                    neighbor(Some(i), j.checked_sub(1)),
                    neighbor(Some(i), Some(j + 1)),
                ];
                let mut f = self.flux[idx];
                // Bed drag on what flows out of here, semi-implicit so thin films stay stable.
                let wet = d.max(1e-4);
                let speed = (f[0] + f[1] + f[2] + f[3]) / (l * wet);
                let drag = 1.0 + dt * g * n2 * speed / wet.powf(4.0 / 3.0);
                let steady = self.steady[idx];
                let k = dt * g * d;
                for (dir, other) in ns.iter().enumerate() {
                    let target = match other {
                        Some(o) => *o,
                        // Open edge: the outside is the same bed, dry.
                        None if self.open_edges => self.bed[idx],
                        None => f32::INFINITY,
                    };
                    f[dir] = if target.is_finite() {
                        let kept = steady[dir] + (f[dir] - steady[dir]) * keep;
                        ((kept + k * (h - target)) / drag).max(0.0)
                    } else {
                        0.0
                    };
                }
                let total = (f[0] + f[1] + f[2] + f[3]) * dt;
                let have = d * area;
                if total > have && total > 0.0 {
                    let s = have / total;
                    for v in &mut f {
                        *v *= s;
                    }
                }
                self.flux[idx] = f;
                for (m, v) in self.steady[idx].iter_mut().zip(f) {
                    *m += (v - *m) * follow;
                }
            }
        }
        // Move water and derive velocities.
        for j in 0..nz {
            for i in 0..nx {
                let idx = j * nx + i;
                let f = self.flux[idx];
                let from_l = if i > 0 { self.flux[idx - 1][1] } else { 0.0 };
                let from_r = if i + 1 < nx {
                    self.flux[idx + 1][0]
                } else {
                    0.0
                };
                let from_b = if j > 0 { self.flux[idx - nx][3] } else { 0.0 };
                let from_t = if j + 1 < nz {
                    self.flux[idx + nx][2]
                } else {
                    0.0
                };
                let inflow = from_l + from_r + from_b + from_t;
                let outflow = f[0] + f[1] + f[2] + f[3];
                let before = self.depth[idx];
                let after = (before + dt * (inflow - outflow) / area).max(0.0);
                // Water leaving the grid through open edges.
                let lost = (if i == 0 { f[0] } else { 0.0 })
                    + (if i + 1 == nx { f[1] } else { 0.0 })
                    + (if j == 0 { f[2] } else { 0.0 })
                    + (if j + 1 == nz { f[3] } else { 0.0 });
                self.drained_m3 += (lost * dt) as f64;
                let avg = ((before + after) * 0.5).max(1e-3);
                let qx = (from_l - f[0] + f[1] - from_r) * 0.5;
                let qz = (from_b - f[2] + f[3] - from_t) * 0.5;
                let v = Vec2::new(qx, qz) / (l * avg);
                // Thin films can't carry absurd speeds.
                self.vel[idx] = v.clamp_length_max(12.0);
                self.depth[idx] = (after - self.loss * dt).max(0.0);
            }
        }
    }

    /// Water at world `p` (x/z): (surface height world y, depth, world x/z velocity), if wet.
    pub fn sample(&self, p: DVec3) -> Option<(f64, f32, Vec2)> {
        let local = self.local(p);
        let gx = local.x / self.cell as f64 - 0.5;
        let gz = local.y / self.cell as f64 - 0.5;
        if gx < 0.0 || gz < 0.0 || gx >= (self.nx - 1) as f64 || gz >= (self.nz - 1) as f64 {
            return None;
        }
        let (i, j) = (gx as usize, gz as usize);
        let (fx, fz) = ((gx - i as f64) as f32, (gz - j as f64) as f32);
        let idx = |ii: usize, jj: usize| jj * self.nx + ii;
        let corners = [idx(i, j), idx(i + 1, j), idx(i, j + 1), idx(i + 1, j + 1)];
        let w = [
            (1.0 - fx) * (1.0 - fz),
            fx * (1.0 - fz),
            (1.0 - fx) * fz,
            fx * fz,
        ];
        let mut depth = 0.0;
        let mut wet_w = 0.0;
        let mut surf = 0.0;
        let mut vel = Vec2::ZERO;
        for (c, w) in corners.iter().zip(w) {
            depth += self.depth[*c] * w;
            if self.depth[*c] > 0.002 {
                wet_w += w;
                surf += (self.bed[*c] + self.depth[*c]) * w;
                vel += self.vel[*c] * w;
            }
        }
        if depth < 0.005 || wet_w <= 0.0 {
            return None;
        }
        Some((
            self.origin.y + (surf / wet_w) as f64,
            depth,
            self.world_dir(vel / wet_w),
        ))
    }
}

/// Damping of flow away from its steady average, 1/s: sloshing settles, while bed friction
/// alone slows a steady stream.
const STILL_DAMPING: f32 = 0.3;
/// Seconds over which [`FlowSim::steady`] follows the flow.
const STEADY_TIME: f32 = 30.0;

/// Manning's roughness for a region's `friction` (0 smooth concrete 0.011, 0.35 a natural
/// stream 0.035, 1 a boulder-strewn torrent 0.08).
fn manning(friction: f32) -> f32 {
    0.011 + 0.069 * friction.clamp(0.0, 1.0)
}

/// Offset of world `p` from a grid corner at `origin` turned by `yaw`, along the grid's axes.
pub fn grid_local(origin: DVec3, yaw: f32, p: DVec3) -> DVec2 {
    let d = DVec2::new(p.x - origin.x, p.z - origin.z);
    let (s, c) = (yaw as f64).sin_cos();
    DVec2::new(c * d.x - s * d.y, s * d.x + c * d.y)
}

/// The world point `local` meters along the axes of a grid at `origin` turned by `yaw`, `y`
/// above the corner.
pub fn grid_point(origin: DVec3, yaw: f32, local: DVec2, y: f64) -> DVec3 {
    let (s, c) = (yaw as f64).sin_cos();
    origin + DVec3::new(c * local.x + s * local.y, y, -s * local.x + c * local.y)
}

/// The yaw (turn about +Y, as `EulerRot::YXZ` reads it) of something at `yaw` after `s`.
pub fn turned_yaw(s: &WorldShift, yaw: f32) -> f32 {
    use engine_core::math::{EulerRot, Quat};
    s.orientation(Quat::from_rotation_y(yaw))
        .to_euler(EulerRot::YXZ)
        .0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(nx: u32, nz: u32) -> FlowRegion {
        FlowRegion {
            cells: [nx, nz],
            cell_m: 0.5,
            open_edges: false,
            friction: 0.3,
            ..Default::default()
        }
    }

    #[test]
    fn water_runs_downhill_and_pools() {
        let r = region(40, 8);
        // A ramp from x = 0 (3 m) down to a basin at the far end.
        let mut bed = vec![0.0; 40 * 8];
        for j in 0..8 {
            for i in 0..40 {
                bed[j * 40 + i] = if i < 30 { 3.0 - i as f32 * 0.1 } else { 0.0 };
            }
        }
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, bed);
        sim.sources.push(FlowSource {
            at: [1.0, 2.0],
            radius_m: 0.8,
            rate_m3_s: 0.2,
        });
        for _ in 0..60 * 20 {
            sim.step(1.0 / 60.0);
        }
        // Water on the ramp flows toward +x at the speed bed friction allows (Manning: about
        // 1.5 m/s for 5 cm of water on a 20% slope), a few centimeters deep.
        let v = sim.vel[4 * 40 + 15];
        assert!(v.x > 0.8 && v.x < 3.0, "{v:?}");
        let ramp = sim.depth[4 * 40 + 15];
        assert!(ramp > 0.01 && ramp < 0.1, "ramp depth {ramp}");
        // When the source stops, the ramp drains into the basin.
        sim.sources.clear();
        for _ in 0..60 * 20 {
            sim.step(1.0 / 60.0);
        }
        let basin: f32 = (30..40).map(|i| sim.depth[4 * 40 + i]).sum::<f32>() / 10.0;
        let ramp_top = sim.depth[4 * 40 + 5];
        assert!(basin > 0.15, "basin depth {basin}");
        assert!(ramp_top < 0.01, "ramp {ramp_top} basin {basin}");
        // Closed walls, no losses: everything the source added is still here.
        let added = 0.2 * 20.0;
        assert!(
            (sim.volume() - added).abs() < added * 0.02,
            "{} vs {added}",
            sim.volume()
        );
    }

    #[test]
    fn a_fall_is_found_where_water_drops() {
        // A shelf at 3 m ending in a 2 m step at x = 20, fed at the top.
        let r = region(40, 8);
        let bed: Vec<f32> = (0..40 * 8)
            .map(|k| {
                if k % 40 < 20 {
                    3.0 - (k % 40) as f32 * 0.01
                } else {
                    1.0 - (k % 40) as f32 * 0.01
                }
            })
            .collect();
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, bed);
        sim.sources.push(FlowSource {
            at: [1.0, 2.0],
            radius_m: 0.8,
            rate_m3_s: 0.2,
        });
        for _ in 0..60 * 40 {
            sim.step(1.0 / 60.0);
        }
        let falls = sim.plunges(0.5);
        assert_eq!(falls.len(), 1, "{falls:?}");
        let (foot, v, power) = falls[0];
        // At the foot of the step (cells are 0.5 m: x = 10 m), going +x, about ρgQΔh.
        assert!((foot.x - 10.25).abs() < 0.6, "{foot:?}");
        assert!(v.x > 0.3, "{v:?}");
        assert!(power > 1000.0 * 9.81 * 0.1 * 1.5, "{power}");
        // Gentle slopes don't count.
        assert!(sim.plunges(2.5).is_empty());
    }

    #[test]
    fn a_dam_break_levels_out() {
        let r = FlowRegion {
            fills: Vec::new(),
            ..region(60, 4)
        };
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, vec![0.0; 240]);
        for j in 0..4 {
            for i in 0..20 {
                sim.depth[j * 60 + i] = 2.0;
            }
        }
        let v0 = sim.volume();
        for _ in 0..60 * 60 {
            sim.step(1.0 / 60.0);
        }
        let min = sim.depth.iter().copied().fold(f32::MAX, f32::min);
        let max = sim.depth.iter().copied().fold(0.0f32, f32::max);
        assert!(max - min < 0.1, "still sloshing: {min}..{max}");
        assert!((sim.volume() - v0).abs() < v0 * 1e-3);
        assert!(sim.depth.iter().all(|d| d.is_finite()));
    }

    #[test]
    fn open_edges_drain_and_reservoirs_hold_level() {
        let r = FlowRegion {
            open_edges: true,
            ..region(20, 20)
        };
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, vec![0.0; 400]);
        sim.depth.fill(0.5);
        for _ in 0..60 * 30 {
            sim.step(1.0 / 60.0);
        }
        assert!(sim.volume() < 0.5 * 100.0 * 0.2, "{}", sim.volume());
        assert!(sim.drained_m3 > 30.0);

        let mut sim = FlowSim::new(
            Entity::PLACEHOLDER,
            DVec3::ZERO,
            &region(20, 4),
            vec![0.0; 80],
        );
        for j in 0..4 {
            sim.reservoir[j * 20] = 1.0;
        }
        for _ in 0..60 * 20 {
            sim.step(1.0 / 60.0);
        }
        // The reservoir filled the closed channel to its level.
        assert!(
            (sim.depth[2 * 20 + 19] - 1.0).abs() < 0.05,
            "{}",
            sim.depth[2 * 20 + 19]
        );
    }

    #[test]
    fn sampling_reports_surface_and_flow() {
        let r = region(10, 10);
        let mut sim = FlowSim::new(
            Entity::PLACEHOLDER,
            DVec3::new(100.0, 5.0, 0.0),
            &r,
            vec![1.0; 100],
        );
        sim.depth.fill(0.4);
        let (h, d, _) = sim.sample(DVec3::new(102.0, 0.0, 2.0)).unwrap();
        assert!((h - 6.4).abs() < 1e-4 && (d - 0.4).abs() < 1e-4);
        assert!(sim.sample(DVec3::new(90.0, 0.0, 2.0)).is_none());
    }

    #[test]
    fn flood_fills_only_the_basin_around_the_seed() {
        // Two hollows (bed 0) split by a wall (bed 2) in a 5 × 1 strip.
        let bed = [0.0, 0.0, 2.0, 0.0, 0.0];
        let fill = FlowFill {
            at: [0.5, 0.5],
            level: 1.0,
            ..Default::default()
        };
        let d = flood(&bed, 5, 1, 1.0, &[fill]);
        assert_eq!(d, vec![1.0, 1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_hollow_fills_to_its_brim() {
        // A pool (bed 0) in a 7 × 5 patch of ground at 3, whose rim dips to 2 on its east side
        // where a channel (bed 1) leads away to the edge.
        let mut bed = vec![3.0f32; 35];
        for j in 1..4 {
            for i in 1..4 {
                bed[j * 7 + i] = 0.0;
            }
        }
        bed[2 * 7 + 4] = 2.0;
        bed[2 * 7 + 5] = 1.0;
        bed[2 * 7 + 6] = 1.0;
        let d = flood(&bed, 7, 5, 1.0, &[FlowFill::to_spill([2.5, 2.5])]);
        assert_eq!(d[2 * 7 + 2], 2.0, "the pool fills to the dip in its rim");
        assert_eq!(d[2 * 7 + 4], 0.0);
        assert_eq!(d[2 * 7 + 5], 0.0, "nothing runs on down the channel");
        // A seed on a slope or in the channel (no hollow) fills nothing.
        let d = flood(&bed, 7, 5, 1.0, &[FlowFill::to_spill([5.5, 2.5])]);
        assert!(d.iter().all(|d| *d == 0.0));
    }
}
