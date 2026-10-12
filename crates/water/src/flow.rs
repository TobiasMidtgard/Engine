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

use engine_core::math::{DVec3, Vec2};
use engine_core::prelude::*;

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
/// region's origin corner, x and z) filled up to `level` (meters above the origin).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowFill {
    pub at: [f32; 2],
    pub level: f32,
}

impl Default for FlowFill {
    fn default() -> Self {
        Self {
            at: [0.0, 0.0],
            level: 1.0,
        }
    }
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
        let mut seen = vec![false; nx * nz];
        let mut stack = vec![j as usize * nx + i as usize];
        while let Some(idx) = stack.pop() {
            if seen[idx] || bed[idx] >= f.level {
                continue;
            }
            seen[idx] = true;
            depth[idx] = depth[idx].max(f.level - bed[idx]);
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
/// The entity's position is the grid's corner (cell (0, 0)); cells run along world +x and +z.
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
    pub nx: usize,
    pub nz: usize,
    pub cell: f32,
    pub bed: Vec<f32>,
    pub depth: Vec<f32>,
    /// Outflow to −x, +x, −z, +z, m³/s.
    pub flux: Vec<[f32; 4]>,
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
            nx,
            nz,
            cell: region.cell_m.max(0.05),
            bed,
            depth,
            flux: vec![[0.0; 4]; nx * nz],
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
        self.origin
            + DVec3::new(
                (i as f64 + 0.5) * self.cell as f64,
                0.0,
                (j as f64 + 0.5) * self.cell as f64,
            )
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
        // Pipe flow: accelerate by the level difference, damp by friction.
        let keep = (1.0 - self.friction.clamp(0.0, 1.0) * 2.0 * dt).clamp(0.0, 1.0);
        let k = dt * g * l;
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
                for (dir, other) in ns.iter().enumerate() {
                    let target = match other {
                        Some(o) => *o,
                        // Open edge: the outside is the same bed, dry.
                        None if self.open_edges => self.bed[idx],
                        None => f32::INFINITY,
                    };
                    f[dir] = if target.is_finite() {
                        (f[dir] * keep + k * (h - target)).max(0.0)
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

    /// Water at world `p` (x/z): (surface height world y, depth, velocity), if wet.
    pub fn sample(&self, p: DVec3) -> Option<(f64, f32, Vec2)> {
        let local = p - self.origin;
        let gx = local.x / self.cell as f64 - 0.5;
        let gz = local.z / self.cell as f64 - 0.5;
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
        Some((self.origin.y + (surf / wet_w) as f64, depth, vel / wet_w))
    }
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
        let basin: f32 = (30..40).map(|i| sim.depth[4 * 40 + i]).sum::<f32>() / 10.0;
        let ramp_top = sim.depth[4 * 40 + 5];
        assert!(basin > 0.15, "basin depth {basin}");
        assert!(ramp_top < basin, "ramp {ramp_top} basin {basin}");
        // Water on the ramp flows toward +x.
        assert!(sim.vel[4 * 40 + 15].x > 0.1, "{:?}", sim.vel[4 * 40 + 15]);
        // Closed walls, no losses: everything the source added is still here.
        let added = 0.2 * 20.0;
        assert!(
            (sim.volume() - added).abs() < added * 0.02,
            "{} vs {added}",
            sim.volume()
        );
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
        };
        let d = flood(&bed, 5, 1, 1.0, &[fill]);
        assert_eq!(d, vec![1.0, 1.0, 0.0, 0.0, 0.0]);
    }
}
