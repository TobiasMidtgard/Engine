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

use engine_core::math::{DVec2, DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::ComputeTaskPool;
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

/// A stone standing in a [`FlowRegion`]'s bed: the bed rises over it in a dome and the water
/// parts round it, piling up in front, racing past its sides and leaving slack water behind,
/// as round a boulder in a stream. Fixed scenery; moving things are found by the physics scan
/// instead ([`FlowSim::set_standing`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowRock {
    /// Its middle, meters from the region's origin corner (x, z).
    pub at: [f32; 2],
    pub radius_m: f32,
    /// Its top over the ground under its middle, meters.
    pub height_m: f32,
}

impl Default for FlowRock {
    fn default() -> Self {
        Self {
            at: [0.0, 0.0],
            radius_m: 0.5,
            height_m: 0.4,
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
/// Millimeters don't count: a flat floor that tilts a little (the planet's curve) is still
/// the floor, not the way out.
fn spill_level(bed: &[f32], nx: usize, nz: usize, seed: usize) -> f32 {
    const SPILL_TOLERANCE: f32 = 0.02;
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
        if b < level - SPILL_TOLERANCE {
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

/// Top of the stones standing in each cell (`NEG_INFINITY` where none does), over `ground`:
/// each a dome over the ground under its middle, sampled at the cell centers it covers.
fn rock_tops(rocks: &[FlowRock], ground: &[f32], nx: usize, nz: usize, cell: f32) -> Vec<f32> {
    let mut tops = vec![f32::NEG_INFINITY; nx * nz];
    for r in rocks {
        let radius = r.radius_m.max(1e-3);
        let cell_of = |x: f32| (x / cell).floor() as i64;
        let (ci, cj) = (cell_of(r.at[0]), cell_of(r.at[1]));
        if ci < 0 || cj < 0 || ci >= nx as i64 || cj >= nz as i64 {
            continue;
        }
        let base = ground[cj as usize * nx + ci as usize];
        let reach = (radius / cell).ceil() as i64 + 1;
        for j in (cj - reach).max(0)..=(cj + reach).min(nz as i64 - 1) {
            for i in (ci - reach).max(0)..=(ci + reach).min(nx as i64 - 1) {
                let c = [(i as f32 + 0.5) * cell, (j as f32 + 0.5) * cell];
                let d = ((c[0] - r.at[0]).powi(2) + (c[1] - r.at[1]).powi(2)).sqrt() / radius;
                // A rock narrower than a cell still stands in the cell it is in.
                let d = if (i, j) == (ci, cj) { d.min(0.5) } else { d };
                if d < 1.0 {
                    let k = j as usize * nx + i as usize;
                    tops[k] = tops[k].max(base + r.height_m * (1.0 - d * d).sqrt());
                }
            }
        }
    }
    tops
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
    /// Extra springs that run only through the first half of the prewarm: placed along a long
    /// course, they fill it everywhere at once instead of from one end, without flooding the
    /// head of the stream.
    pub priming: Vec<FlowSource>,
    /// Stones standing in the bed.
    pub rocks: Vec<FlowRock>,
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
            priming: Vec::new(),
            rocks: Vec::new(),
            optics: WaterOptics::stream(),
            visible: true,
        }
    }
}

/// An edge water pours over into a drop (see [`FlowSim::lips`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lip {
    /// Middle of the edge, at the water's surface (world).
    pub top: DVec3,
    /// Horizontal direction the water leaves in (world, unit).
    pub dir: Vec2,
    /// Speed it leaves at, m/s.
    pub speed: f32,
    /// Water over the edge per meter of edge, m²/s.
    pub q: f32,
    /// Height it falls to the water or ground below, meters.
    pub drop: f32,
    /// Length of the edge, meters.
    pub width: f32,
}

/// A point down a falling curtain (see [`FlowSim::fall_path`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FallPoint {
    /// World position.
    pub pos: DVec3,
    /// Velocity (world), m/s.
    pub vel: Vec3,
    /// Seconds since the water left the lip.
    pub t: f32,
    /// Outward normal (world) of the rock the water slides down here, zero where it falls
    /// free.
    pub rock: Vec3,
}

/// Water sliding down rock rides this far off it, meters (a sheet's thickness, and clear of
/// the terrain's triangles).
const SHEET: f32 = 0.06;
/// Rock flatter than this (rise over run) catches falling water: it lands and runs on in
/// the grid. Steeper faces it slides down.
const LANDING_SLOPE: f32 = 0.7;

/// For each of `lips`, whether another carries its sheet on past its left and right ends
/// (-across, +across): a neighbor about a lip's width along the brink at about its height.
fn lip_sides(lips: &[Lip]) -> Vec<[bool; 2]> {
    lips.iter()
        .enumerate()
        .map(|(k, lip)| {
            let across = Vec2::new(-lip.dir.y, lip.dir.x);
            let mut sides = [false; 2];
            for (m, other) in lips.iter().enumerate() {
                let d = other.top - lip.top;
                if m == k || d.y.abs() > lip.width.max(0.5) as f64 {
                    continue;
                }
                let d = Vec2::new(d.x as f32, d.z as f32);
                let (a, b) = (d.dot(across), d.dot(lip.dir));
                let reach = 1.6 * lip.width.max(other.width);
                if b.abs() < lip.width && a.abs() > 0.3 * lip.width && a.abs() < reach {
                    sides[(a > 0.0) as usize] = true;
                }
            }
            sides
        })
        .collect()
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
    /// The bed the water runs on: the ground, raised where something stands in the water
    /// ([`FlowSim::set_standing`]).
    pub bed: Vec<f32>,
    /// The ground under each cell, without what stands on it.
    pub ground: Vec<f32>,
    /// Top of whatever stands on the ground in each cell (a rock, a crate, a leg), relative to
    /// `origin.y`; `NEG_INFINITY` where nothing does.
    pub standing: Vec<f32>,
    /// Bumped whenever the bed changes (copies and the renderer follow it).
    pub bed_version: u64,
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
    /// Out of the `SimulationRange` of every focus: it holds still, as it last ran, until one
    /// comes near.
    pub resting: bool,
    /// Lowest and highest bed, meters above `origin.y` (only ever grows).
    bed_span: [f32; 2],
    /// The falls as last worked out ([`FlowSim::falls`]), and seconds of flow since.
    falls: std::sync::Arc<Falls>,
    since_falls: f32,
}

/// Where a region's water falls, worked out from its flow every [`FALLS_PERIOD`] of flow
/// (the lips move slowly once a stream has settled): the renderer draws the curtains, the
/// spray and the boil from it each frame, and the shores wet round the feet, without each
/// tracing every fall again.
#[derive(Debug, Clone, Default)]
pub struct Falls {
    /// The lips with a drop of at least [`FlowSim::fall_drop`].
    pub lips: Vec<Lip>,
    /// Each lip's path down ([`FlowSim::fall_path`], [`FALL_POINTS`] segments).
    pub paths: Vec<Vec<FallPoint>>,
    /// Where they land ([`FlowSim::plunges`]).
    pub plunges: Vec<(DVec3, Vec2, f32)>,
    /// Per lip, whether the sheet carries on past its left and right ends (-across, +across:
    /// another lip along the same brink), so only a fall's outer edges fray.
    pub sides: Vec<[bool; 2]>,
}

/// Seconds of flow between working out a region's [`Falls`] again.
pub const FALLS_PERIOD: f32 = 0.2;
/// Segments of each cached fall path.
pub const FALL_POINTS: usize = 16;

impl FlowSim {
    /// A region with `bed` heights (relative to `origin.y`).
    pub fn new(entity: Entity, origin: DVec3, region: &FlowRegion, bed: Vec<f32>) -> Self {
        // The step runs on the compute pool; make sure it exists (no-op in a running app).
        engine_core::tasks::init_task_pools();
        let nx = region.cells[0].max(2) as usize;
        let nz = region.cells[1].max(2) as usize;
        let cell = region.cell_m.max(0.05);
        let ground = {
            let mut g = bed;
            g.resize(nx * nz, 0.0);
            g
        };
        let standing = rock_tops(&region.rocks, &ground, nx, nz, cell);
        let bed: Vec<f32> = ground
            .iter()
            .zip(&standing)
            .map(|(g, s)| g.max(*s))
            .collect();
        let depth = flood(&bed, nx, nz, cell, &region.fills);
        let bed_span = bed
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], &h| {
                [lo.min(h), hi.max(h)]
            });
        Self {
            entity,
            origin,
            yaw: 0.0,
            nx,
            nz,
            cell,
            ground,
            standing,
            bed_version: 0,
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
            resting: false,
            bed_span,
            falls: Default::default(),
            since_falls: f32::INFINITY,
        }
    }

    /// The least drop that makes a fall (one the renderer draws a curtain over): at least a
    /// cell's width, so a steep bank isn't a waterfall, and half a meter.
    pub fn fall_drop(&self) -> f32 {
        self.cell.max(0.5)
    }

    /// The falls as last worked out (see [`Falls`]); [`FlowSim::refresh_falls`] works them out
    /// now.
    pub fn falls(&self) -> &Falls {
        &self.falls
    }

    /// Work the falls out again from the flow as it is.
    pub fn refresh_falls(&mut self) {
        let lips = self.lips(self.fall_drop());
        let paths = lips
            .iter()
            .map(|lip| self.fall_path(lip, FALL_POINTS))
            .collect::<Vec<_>>();
        let plunges = self.plunges_of(&lips, &paths);
        let sides = lip_sides(&lips);
        self.falls = std::sync::Arc::new(Falls {
            lips,
            paths,
            plunges,
            sides,
        });
        self.since_falls = 0.0;
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
    /// Become a copy of `other`, reusing this one's buffers: no allocation once the shapes
    /// match. The bed rarely changes, so a copy of the same region keeps its own until then.
    pub fn copy_from(&mut self, other: &FlowSim) {
        let FlowSim {
            entity,
            origin,
            yaw,
            nx,
            nz,
            cell,
            bed,
            ground,
            standing,
            bed_version,
            depth,
            flux,
            steady,
            vel,
            reservoir,
            open_edges,
            friction,
            loss,
            sources,
            optics,
            visible,
            version,
            drained_m3,
            resting,
            bed_span,
            falls,
            since_falls,
        } = other;
        if self.entity != *entity || self.bed.len() != bed.len() || self.bed_version != *bed_version
        {
            self.bed.clone_from(bed);
            self.ground.clone_from(ground);
            self.standing.clone_from(standing);
            self.bed_version = *bed_version;
        }
        self.entity = *entity;
        self.origin = *origin;
        self.yaw = *yaw;
        self.nx = *nx;
        self.nz = *nz;
        self.cell = *cell;
        self.depth.clone_from(depth);
        self.flux.clone_from(flux);
        self.steady.clone_from(steady);
        self.vel.clone_from(vel);
        self.reservoir.clone_from(reservoir);
        self.open_edges = *open_edges;
        self.friction = *friction;
        self.loss = *loss;
        self.sources.clone_from(sources);
        self.optics = *optics;
        self.visible = *visible;
        self.version = *version;
        self.drained_m3 = *drained_m3;
        self.resting = *resting;
        self.bed_span = *bed_span;
        self.falls = falls.clone();
        self.since_falls = *since_falls;
    }

    /// Something stands in cell `idx` up to `top` meters above `origin.y` (`NEG_INFINITY`:
    /// nothing does any more): the bed rises to it and the water parts round it, as round a
    /// rock in a stream. Returns whether the bed changed.
    pub fn set_standing(&mut self, idx: usize, top: f32) -> bool {
        self.standing[idx] = top;
        self.rebed(idx)
    }

    /// The ground under cell `idx` is now `ground` meters above `origin.y` (a terrain edit).
    /// Returns whether the bed changed.
    pub fn set_ground(&mut self, idx: usize, ground: f32) -> bool {
        self.ground[idx] = ground;
        self.rebed(idx)
    }

    /// Bring cell `idx`'s bed to its ground and what stands on it. Water a rising bed lifts
    /// out spills onto the lower cells round it (none is lost while there is somewhere to go).
    fn rebed(&mut self, idx: usize) -> bool {
        let new = self.ground[idx].max(self.standing[idx]);
        let old = self.bed[idx];
        // Changes under 2 cm don't change the flow (and keep a scan's noise out).
        if (new - old).abs() < 0.02 {
            return false;
        }
        if new > old {
            let lifted = self.depth[idx].min(new - old);
            self.depth[idx] -= lifted;
            let (i, j) = (idx % self.nx, idx / self.nx);
            let mut lower = Vec::with_capacity(4);
            for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (x, z) = (i as i64 + di, j as i64 + dj);
                if x >= 0 && z >= 0 && (x as usize) < self.nx && (z as usize) < self.nz {
                    let n = z as usize * self.nx + x as usize;
                    if self.bed[n] < new {
                        lower.push(n);
                    }
                }
            }
            if lower.is_empty() {
                self.depth[idx] += lifted;
            } else {
                let share = lifted / lower.len() as f32;
                for n in lower {
                    self.depth[n] += share;
                }
            }
        }
        self.bed[idx] = new;
        self.flux[idx] = [0.0; 4];
        self.steady[idx] = [0.0; 4];
        self.vel[idx] = Vec2::ZERO;
        self.bed_span = [self.bed_span[0].min(new), self.bed_span[1].max(new)];
        self.bed_version += 1;
        self.version += 1;
        // The falls move with the bed.
        self.since_falls = f32::INFINITY;
        true
    }

    /// The middle of the grid and the distance from it to a corner, meters: a sphere round
    /// the whole region.
    pub fn bounds(&self) -> (DVec3, f64) {
        let c = self.cell as f64;
        let (w, d) = (self.nx as f64 * c, self.nz as f64 * c);
        let [lo, hi] = self.bed_span.map(|h| h as f64);
        let center = self.world(DVec2::new(w * 0.5, d * 0.5), (lo + hi) * 0.5);
        (center, 0.5 * (w * w + d * d + (hi - lo).powi(2)).sqrt())
    }

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
        self.refresh_falls();
    }

    /// Where water falls over a drop: (world position at the foot, horizontal velocity of the
    /// falling water m/s, power in watts), one entry per cluster of nearby
    /// [`lips`](Self::lips). Drops under `min_drop` meters are ignored. The power is ρgQΔh for a
    /// free fall; a sheet sliding down rock spends most of it on the way, so less of it
    /// reaches the foot (less spray, mist and boil).
    pub fn plunges(&self, min_drop: f32) -> Vec<(DVec3, Vec2, f32)> {
        let lips = self.lips(min_drop);
        let paths: Vec<_> = lips.iter().map(|lip| self.fall_path(lip, 6)).collect();
        self.plunges_of(&lips, &paths)
    }

    /// [`FlowSim::plunges`] of `lips` falling down `paths`.
    fn plunges_of(&self, lips: &[Lip], paths: &[Vec<FallPoint>]) -> Vec<(DVec3, Vec2, f32)> {
        // Feet cluster into squares 8 cells wide, so a wide fall is one plunge, two falls two.
        let size = 8.0 * self.cell as f64;
        let mut buckets: Vec<((i64, i64), DVec3, Vec2, f32)> = Vec::new();
        for (lip, path) in lips.iter().zip(paths) {
            // Where it lands in the water or on ground below, down its arc and any rock it
            // slides over.
            let end = *path.last().unwrap();
            let foot = end.pos;
            let free =
                path.iter().filter(|p| p.rock == Vec3::ZERO).count() as f32 / path.len() as f32;
            let power = 1000.0 * 9.81 * lip.q * lip.width * lip.drop * (0.25 + 0.75 * free);
            let v = Vec2::new(end.vel.x, end.vel.z);
            let key = (
                (foot.x / size).floor() as i64,
                (foot.z / size).floor() as i64,
            );
            match buckets.iter_mut().find(|b| b.0 == key) {
                Some(b) => {
                    b.1 += foot * power as f64;
                    b.2 += v * power;
                    b.3 += power;
                }
                None => buckets.push((key, foot * power as f64, v * power, power)),
            }
        }
        buckets
            .into_iter()
            .map(|(_, p, v, w)| (p / w as f64, v / w, w))
            .collect()
    }

    /// Where water pours over an edge into a drop of at least `min_drop` meters within one
    /// cell, and so leaves the ground and falls free: one [`Lip`] per wet cell at the brink,
    /// facing the way its water goes over (diagonal where the brink runs across the grid), with
    /// where it lands.
    pub fn lips(&self, min_drop: f32) -> Vec<Lip> {
        let level = |idx: usize| self.bed[idx] + self.depth[idx];
        let steps: [(i64, i64); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
        let at = |i: i64, j: i64| {
            (i >= 0 && j >= 0 && i < self.nx as i64 && j < self.nz as i64)
                .then(|| j as usize * self.nx + i as usize)
        };
        let mut out = Vec::new();
        for j in 0..self.nz {
            for i in 0..self.nx {
                let idx = j * self.nx + i;
                // Water thins out as it speeds up over a brink: any film counts.
                if self.depth[idx] < 0.0005 || self.reservoir[idx].is_finite() {
                    continue;
                }
                let top = level(idx);
                let (mut flux, mut way, mut drop) = (0.0f32, Vec2::ZERO, 0.0f32);
                for (k, (di, dj)) in steps.into_iter().enumerate() {
                    let f = self.flux[idx][k];
                    let Some(n) = at(i as i64 + di, j as i64 + dj) else {
                        continue;
                    };
                    if f < 2e-4 * self.cell || top - level(n) < min_drop {
                        continue;
                    }
                    // Water already falling through this cell belongs to the lip above it.
                    if at(i as i64 - di, j as i64 - dj).is_some_and(|b| level(b) - top >= min_drop)
                    {
                        continue;
                    }
                    // Follow the drop down to where it stops falling steeply: the foot.
                    let (mut m, mut mi, mut mj) = (n, i as i64 + di, j as i64 + dj);
                    for _ in 0..64 {
                        match at(mi + di, mj + dj) {
                            Some(next) if level(next) < level(m) - self.cell * 0.5 => {
                                (m, mi, mj) = (next, mi + di, mj + dj);
                            }
                            _ => break,
                        }
                    }
                    flux += f;
                    way += Vec2::new(di as f32, dj as f32) * f;
                    drop += (top - level(m)) * f;
                }
                if flux <= 0.0 {
                    continue;
                }
                let dir = way.normalize_or(Vec2::X);
                // The brink is as long as the cell is wide across the way the water goes.
                let width = self.cell * (dir.x.abs() + dir.y.abs());
                let q = flux / width;
                let edge = (Vec2::new(i as f32, j as f32) + 0.5)
                    + dir * (0.5 / dir.x.abs().max(dir.y.abs()));
                // Water leaves a brink at about 1.4 times the critical speed (g q)^⅓, or as
                // fast as it came if faster. (The grid's own velocity in the brink cell is
                // pulled by the whole height of the drop.)
                let back = if dir.x.abs() >= dir.y.abs() {
                    (i as i64 - dir.x.signum() as i64, j as i64)
                } else {
                    (i as i64, j as i64 - dir.y.signum() as i64)
                };
                let approach = at(back.0, back.1).map_or(0.0, |b| self.vel[b].dot(dir));
                let speed = (1.4 * (9.81 * q).cbrt()).max(approach).clamp(0.3, 12.0);
                out.push(Lip {
                    top: self.world((edge * self.cell).as_dvec2(), top as f64),
                    dir: self.world_dir(dir),
                    speed,
                    q,
                    drop: drop / flux,
                    width,
                });
            }
        }
        out
    }

    /// Bed height (above the corner) at `local` meters along the grid's axes, bilinear, and
    /// its slope (d/dx, d/dz); `None` off the grid.
    fn bed_at(&self, local: Vec2) -> Option<(f32, Vec2, f32)> {
        let size = Vec2::new(self.nx as f32, self.nz as f32);
        let g = local / self.cell;
        if g.x < 0.0 || g.y < 0.0 || g.x > size.x || g.y > size.y {
            return None;
        }
        // The outer half cells take the edge cells' heights.
        let g = (g - 0.5).clamp(Vec2::ZERO, size - 1.001);
        let (i, j) = (g.x as usize, g.y as usize);
        let (fx, fz) = (g.x - i as f32, g.y - j as f32);
        let idx = j * self.nx + i;
        let corner = |o: usize| (self.bed[o], self.depth[o]);
        let ((b00, d00), (b10, d10)) = (corner(idx), corner(idx + 1));
        let ((b01, d01), (b11, d11)) = (corner(idx + self.nx), corner(idx + self.nx + 1));
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let bed = lerp(lerp(b00, b10, fx), lerp(b01, b11, fx), fz);
        let depth = lerp(lerp(d00, d10, fx), lerp(d01, d11, fx), fz);
        let slope = Vec2::new(
            lerp(b10 - b00, b11 - b01, fz),
            lerp(b01 - b00, b11 - b10, fx),
        ) / self.cell;
        Some((bed, slope, depth))
    }

    /// The path the water over `lip` takes down: `count + 1` points evenly spaced along it,
    /// from the lip to where it lands in the water or on ground flat enough to catch it.
    /// It falls free on its arc; where the arc meets a rock face too steep to land on, it
    /// clings and slides down it (sped up by the slope, slowed by the rock), and leaves it
    /// again where the face drops away under it: a cascade down a crevice, not a curtain cut
    /// through the rock.
    pub fn fall_path(&self, lip: &Lip, count: usize) -> Vec<FallPoint> {
        const G: f32 = 9.81;
        const H: f32 = 1.0 / 120.0;
        let to_local = |v: Vec2| {
            let (s, c) = self.yaw.sin_cos();
            Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
        };
        let start = self.local(lip.top).as_vec2();
        let top_y = (lip.top.y - self.origin.y) as f32;
        let flat = to_local(lip.dir) * lip.speed;
        let (mut p, mut v) = (
            Vec3::new(start.x, top_y, start.y),
            Vec3::new(flat.x, 0.0, flat.y),
        );
        let mut rock = Vec3::ZERO;
        let mut t = 0.0;
        let mut path = vec![(p, v, t, rock)];
        // Somewhat past the drop the lip saw (it may slide further than it fell).
        let floor = top_y - lip.drop - 1.0;
        while t < 4.0 && p.y > floor {
            t += H;
            if rock == Vec3::ZERO {
                v.y -= G * H;
            } else {
                // Gravity along the face, and the rock's drag on a thin sheet.
                let down = Vec3::new(0.0, -G, 0.0);
                v += (down - rock * down.dot(rock)) * H;
                v *= 1.0 - 1.2 * H;
            }
            p += v * H;
            let Some((bed, slope, depth)) = self.bed_at(Vec2::new(p.x, p.z)) else {
                break;
            };
            let level = bed + depth;
            let normal = Vec3::new(-slope.x, 1.0, -slope.y).normalize();
            if rock != Vec3::ZERO && p.y > bed + SHEET + 0.15 {
                // The face drops away under it: it leaves the rock.
                rock = Vec3::ZERO;
            } else if p.y <= level.max(bed + SHEET) {
                if depth > 0.08 || slope.length() < LANDING_SLOPE {
                    // Into a pool, or onto a ledge: the grid carries it on from here.
                    p.y = level;
                    path.push((p, v, t, rock));
                    break;
                }
                if rock == Vec3::ZERO {
                    // Hitting the face throws off some of its speed.
                    v *= 0.7;
                }
                rock = normal;
                p.y = bed + SHEET;
                let into = v.dot(rock);
                if into < 0.0 {
                    v -= rock * into;
                }
            }
            path.push((p, v, t, rock));
        }
        // Even spacing along the path.
        let mut along = vec![0.0f32; path.len()];
        for k in 1..path.len() {
            along[k] = along[k - 1] + path[k].0.distance(path[k - 1].0);
        }
        let total = along.last().copied().unwrap_or(0.0).max(1e-4);
        let mut out = Vec::with_capacity(count + 1);
        let mut k = 0;
        for s in 0..=count {
            let want = total * s as f32 / count.max(1) as f32;
            while k + 2 < path.len() && along[k + 1] < want {
                k += 1;
            }
            let (a, b) = (path[k], path[(k + 1).min(path.len() - 1)]);
            let span = (along[(k + 1).min(path.len() - 1)] - along[k]).max(1e-6);
            let f = ((want - along[k]) / span).clamp(0.0, 1.0);
            let pos = a.0.lerp(b.0, f);
            let world_v = self.world_dir(Vec2::new(a.1.x, a.1.z).lerp(Vec2::new(b.1.x, b.1.z), f));
            let r = if f < 0.5 { a.3 } else { b.3 };
            let world_r = self.world_dir(Vec2::new(r.x, r.z));
            out.push(FallPoint {
                pos: self.world(DVec2::new(pos.x as f64, pos.z as f64), pos.y as f64),
                vel: Vec3::new(world_v.x, a.1.y + (b.1.y - a.1.y) * f, world_v.y),
                t: a.2 + (b.2 - a.2) * f,
                rock: Vec3::new(world_r.x, r.y, world_r.y),
            });
        }
        out
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
        self.since_falls += dt;
        if self.since_falls >= FALLS_PERIOD {
            self.refresh_falls();
        }
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
        let open = self.open_edges;
        let (bed, depth) = (&self.bed, &self.depth);
        let rows = nz.div_ceil(bands(nx, nz));
        let flux_rows = self
            .flux
            .chunks_mut(rows * nx)
            .zip(self.steady.chunks_mut(rows * nx));
        par_bands(flux_rows, |band, (flux, steady)| {
            for (o, (f_out, steady)) in flux.iter_mut().zip(steady.iter_mut()).enumerate() {
                let idx = band * rows * nx + o;
                let (i, j) = (idx % nx, idx / nx);
                let d = depth[idx];
                if d <= 0.0 {
                    // Nothing to send: a dry cell's pipes are empty.
                    *f_out = [0.0; 4];
                    for m in steady.iter_mut() {
                        *m -= *m * follow;
                    }
                    continue;
                }
                let h = bed[idx] + d;
                let level = |o: usize| bed[o] + depth[o];
                // Open edge: the outside is the same bed, dry. Shut: a wall.
                let edge = if open { bed[idx] } else { f32::INFINITY };
                let ns = [
                    if i > 0 { level(idx - 1) } else { edge },
                    if i + 1 < nx { level(idx + 1) } else { edge },
                    if j > 0 { level(idx - nx) } else { edge },
                    if j + 1 < nz { level(idx + nx) } else { edge },
                ];
                let mut f = *f_out;
                // Bed drag on what flows out of here, semi-implicit so thin films stay stable.
                let wet = d.max(1e-4);
                let speed = (f[0] + f[1] + f[2] + f[3]) / (l * wet);
                let drag = 1.0 + dt * g * n2 * speed / (wet * wet.cbrt());
                let k = dt * g * d;
                for (dir, target) in ns.into_iter().enumerate() {
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
                *f_out = f;
                for (m, v) in steady.iter_mut().zip(f) {
                    *m += (v - *m) * follow;
                }
            }
        });
        // Move water and derive velocities.
        let (flux, loss) = (&self.flux, self.loss);
        let mut drained = vec![0.0f64; nz.div_ceil(rows)];
        let move_rows = self
            .depth
            .chunks_mut(rows * nx)
            .zip(self.vel.chunks_mut(rows * nx))
            .zip(drained.iter_mut());
        par_bands(move_rows, |band, ((depth, vel), drained)| {
            for (o, (depth, vel)) in depth.iter_mut().zip(vel.iter_mut()).enumerate() {
                let idx = band * rows * nx + o;
                let (i, j) = (idx % nx, idx / nx);
                let f = flux[idx];
                let from_l = if i > 0 { flux[idx - 1][1] } else { 0.0 };
                let from_r = if i + 1 < nx { flux[idx + 1][0] } else { 0.0 };
                let from_b = if j > 0 { flux[idx - nx][3] } else { 0.0 };
                let from_t = if j + 1 < nz { flux[idx + nx][2] } else { 0.0 };
                let inflow = from_l + from_r + from_b + from_t;
                let outflow = f[0] + f[1] + f[2] + f[3];
                let before = *depth;
                if before <= 0.0 && inflow <= 0.0 {
                    *vel = Vec2::ZERO;
                    continue;
                }
                let after = (before + dt * (inflow - outflow) / area).max(0.0);
                // Water leaving the grid through open edges.
                let lost = (if i == 0 { f[0] } else { 0.0 })
                    + (if i + 1 == nx { f[1] } else { 0.0 })
                    + (if j == 0 { f[2] } else { 0.0 })
                    + (if j + 1 == nz { f[3] } else { 0.0 });
                *drained += (lost * dt) as f64;
                let avg = ((before + after) * 0.5).max(1e-3);
                let qx = (from_l - f[0] + f[1] - from_r) * 0.5;
                let qz = (from_b - f[2] + f[3] - from_t) * 0.5;
                let v = Vec2::new(qx, qz) / (l * avg);
                // Thin films can't carry absurd speeds.
                *vel = v.clamp_length_max(12.0);
                *depth = (after - loss * dt).max(0.0);
            }
        });
        self.drained_m3 += drained.iter().sum::<f64>();
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

/// Bands of rows a region's step is split into: one per thread, or one for a small region
/// (waking the pool costs more than its work).
fn bands(nx: usize, nz: usize) -> usize {
    if nx * nz < 4096 {
        1
    } else {
        ComputeTaskPool::get().thread_num().clamp(1, nz)
    }
}

/// Run `f(band, item)` for each band of rows, one task each.
fn par_bands<C: Send>(items: impl Iterator<Item = C>, f: impl Fn(usize, C) + Send + Sync) {
    let items: Vec<C> = items.collect();
    if items.len() <= 1 {
        for (band, item) in items.into_iter().enumerate() {
            f(band, item);
        }
        return;
    }
    let f = &f;
    ComputeTaskPool::get().scope(|scope| {
        for (band, item) in items.into_iter().enumerate() {
            scope.spawn(async move { f(band, item) });
        }
    });
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
    fn water_parts_round_a_rock_in_the_bed() {
        let mut r = region(40, 9);
        // A boulder 1.5 m across in the middle of the channel, a meter tall.
        r.rocks.push(FlowRock {
            at: [10.25, 2.25],
            radius_m: 0.75,
            height_m: 1.0,
        });
        let bed: Vec<f32> = (0..40 * 9).map(|k| 2.0 - (k % 40) as f32 * 0.04).collect();
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, bed.clone());
        let middle = 4 * 40 + 20;
        assert!(
            (sim.bed[middle] - (bed[middle] + 1.0)).abs() < 0.05,
            "{}",
            sim.bed[middle]
        );
        assert_eq!(sim.ground, bed, "the ground stays the ground's");
        assert!(sim.standing[middle].is_finite() && sim.standing[0] == f32::NEG_INFINITY);
        sim.sources.push(FlowSource {
            at: [1.0, 2.25],
            radius_m: 1.5,
            rate_m3_s: 0.3,
        });
        for _ in 0..60 * 20 {
            sim.step(1.0 / 60.0);
        }
        assert!(
            sim.depth[middle] < 0.005,
            "over the rock: {}",
            sim.depth[middle]
        );
        // The current runs past its sides, and the water stays in the channel round it.
        assert!(sim.vel[2 * 40 + 20].length() > 0.05 && sim.vel[6 * 40 + 20].length() > 0.05);
        assert!(sim.depth[4 * 40 + 18] > 0.01, "in front of it");
    }

    #[test]
    fn water_parts_round_something_standing_in_it() {
        let r = region(40, 9);
        // A channel sloping down along +x, fed at its top end.
        let bed: Vec<f32> = (0..40 * 9).map(|k| 2.0 - (k % 40) as f32 * 0.04).collect();
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, bed);
        sim.sources.push(FlowSource {
            at: [1.0, 2.25],
            radius_m: 1.5,
            rate_m3_s: 0.3,
        });
        for _ in 0..60 * 15 {
            sim.step(1.0 / 60.0);
        }
        // A crate 1 m tall drops into the middle of the channel (2 × 2 cells).
        let crate_cells = [4 * 40 + 20, 4 * 40 + 21, 5 * 40 + 20, 5 * 40 + 21];
        let before = sim.volume();
        let (version, beside) = (sim.bed_version, sim.depth[4 * 40 + 19]);
        for &k in &crate_cells {
            let top = sim.ground[k] + 1.0;
            assert!(sim.set_standing(k, top));
        }
        assert!(sim.bed_version > version);
        // The water it stood in went to the cells round it, not away.
        assert!(
            (sim.volume() - before).abs() < 1e-3,
            "{} vs {before}",
            sim.volume()
        );
        for _ in 0..60 * 10 {
            sim.step(1.0 / 60.0);
        }
        // Dry on top, the water piled up in front of it and running past its sides.
        for &k in &crate_cells {
            assert!(sim.depth[k] < 0.005, "on the crate: {}", sim.depth[k]);
        }
        assert!(
            sim.depth[4 * 40 + 19] > beside * 1.2,
            "in front: {beside} -> {}",
            sim.depth[4 * 40 + 19]
        );
        assert!(sim.vel[3 * 40 + 20].length() > 0.05 && sim.vel[6 * 40 + 20].length() > 0.05);
        // Lifted away again, the bed is the ground's.
        for &k in &crate_cells {
            sim.set_standing(k, f32::NEG_INFINITY);
            assert_eq!(sim.bed[k], sim.ground[k]);
        }
    }

    #[test]
    fn a_terrain_edit_regrades_the_bed() {
        let r = region(8, 8);
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, vec![1.0; 64]);
        assert!(sim.set_ground(27, 0.4));
        assert_eq!(sim.bed[27], 0.4);
        // Something standing there still holds the bed up.
        sim.set_standing(27, 1.5);
        assert!(!sim.set_ground(27, 0.2));
        assert_eq!(sim.bed[27], 1.5);
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
        // Where the water lands past the step (its edge is at x = 10 m), going +x, about ρgQΔh.
        assert!(foot.x > 10.2 && foot.x < 11.5, "{foot:?}");
        assert!((foot.y - 1.0 + 0.1).abs() < 0.25, "{foot:?}");
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

    /// A spring on a shelf 6 m up (x < 5 m), a 1.4 m sheer drop at its edge, then (with
    /// `face`) a rock face at 60° down to the floor, or (without) a sheer cliff; cells are
    /// 0.5 m. The water's been running for 20 s.
    fn cascade(face: bool) -> FlowSim {
        let (nx, nz) = (40, 6);
        let mut bed = vec![0.0f32; nx * nz];
        for j in 0..nz {
            for i in 0..nx {
                let x = (i as f32 + 0.5) * 0.5;
                bed[j * nx + i] = if x < 5.0 {
                    6.0
                } else if face {
                    (4.6 - (x - 5.0) * 3f32.sqrt()).max(0.0)
                } else {
                    0.0
                };
            }
        }
        let r = FlowRegion {
            cells: [nx as u32, nz as u32],
            cell_m: 0.5,
            sources: vec![FlowSource {
                at: [1.0, 1.5],
                radius_m: 0.6,
                rate_m3_s: 0.05,
            }],
            open_edges: false,
            ..Default::default()
        };
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::new(10.0, 2.0, -3.0), &r, bed);
        sim.yaw = 0.4;
        for _ in 0..30 * 20 {
            sim.step(1.0 / 30.0);
        }
        sim
    }

    #[test]
    fn a_fall_slides_down_the_rock_face_it_meets() {
        let sim = cascade(true);
        let lips = sim.lips(1.0);
        assert!(!lips.is_empty(), "no lip");
        for lip in &lips {
            let path = sim.fall_path(lip, 16);
            assert_eq!(path.len(), 17);
            assert!(
                path[0].pos.distance(lip.top) < 1e-3,
                "{:?} vs {lip:?}",
                path[0]
            );
            let sliding = path.iter().filter(|p| p.rock != Vec3::ZERO).count();
            assert!(sliding >= 4, "only {sliding} of 17 points on the rock");
            for p in &path {
                let (bed, _, _) = sim.bed_at(sim.local(p.pos).as_vec2()).unwrap();
                let above = (p.pos.y - sim.origin.y) as f32 - bed;
                assert!(above > -0.02, "{above} m inside the rock at {p:?}");
                if p.rock != Vec3::ZERO {
                    assert!(above < 0.3, "{above} m off the rock it slides on");
                    assert!(p.rock.y > 0.4 && p.rock.y < 0.6, "a 60° face: {:?}", p.rock);
                }
            }
            // It runs out at the foot of the face, on the floor.
            let end = path.last().unwrap();
            let x = sim.local(end.pos).x;
            assert!((7.0..10.0).contains(&x), "ends {x} m along");
            assert!(end.pos.y - sim.origin.y < 0.3, "ends {} m up", end.pos.y);
            // Times run on down the path.
            assert!(path.windows(2).all(|w| w[1].t >= w[0].t));
        }
    }

    #[test]
    fn a_fall_over_a_sheer_cliff_falls_free() {
        let sim = cascade(false);
        let lips = sim.lips(1.0);
        assert!(!lips.is_empty(), "no lip");
        for lip in &lips {
            let path = sim.fall_path(lip, 12);
            assert!(path.iter().all(|p| p.rock == Vec3::ZERO), "{path:?}");
            // Down its arc to the floor.
            let end = path.last().unwrap();
            assert!(end.pos.y - sim.origin.y < 0.2, "ends {} m up", end.pos.y);
            let reach = sim.local(end.pos).x - sim.local(lip.top).x;
            let t = (2.0 * lip.drop / 9.81).sqrt();
            assert!((reach as f32 - lip.speed * t).abs() < 0.3, "{reach} m out");
        }
    }

    #[test]
    fn water_leaves_a_cliff_edge_as_a_lip() {
        // A 4 m high shelf (x < 5 m) with a spring on it, ending in a cliff over a flat
        // floor; cells are 0.5 m.
        let mut bed = vec![0.0f32; 40 * 6];
        for j in 0..6 {
            for i in 0..10 {
                bed[j * 40 + i] = 4.0;
            }
        }
        let r = FlowRegion {
            cells: [40, 6],
            cell_m: 0.5,
            sources: vec![FlowSource {
                at: [1.0, 1.5],
                radius_m: 0.6,
                rate_m3_s: 0.05,
            }],
            open_edges: false,
            ..Default::default()
        };
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, bed);
        for _ in 0..30 * 20 {
            sim.step(1.0 / 30.0);
        }
        let lips = sim.lips(1.0);
        assert!(!lips.is_empty(), "no lip");
        for lip in &lips {
            // On the cliff's edge (x = 5 m), pouring toward +x, about 4 m down.
            assert!((lip.top.x - 5.0).abs() < 1e-3, "{lip:?}");
            assert!(lip.dir.x > 0.99 && lip.speed > 0.3, "{lip:?}");
            assert!(lip.drop > 3.8 && lip.drop < 4.2, "{lip:?}");
        }
        let poured: f32 = lips.iter().map(|l| l.q * l.width).sum();
        // Once the shelf is wet, what the spring gives goes over the edge.
        assert!(
            poured > 0.03 && poured < 0.08,
            "{poured} m³/s over the edge"
        );
        // Gentle slopes have none.
        assert!(sim.lips(4.5).is_empty());
    }

    #[test]
    fn a_brink_across_the_grid_pours_across_it() {
        // A 3 m shelf whose edge runs diagonally across the grid (cells are 0.5 m).
        let n = 30;
        let bed: Vec<f32> = (0..n * n)
            .map(|k| if k % n + k / n < 24 { 3.0 } else { 0.0 })
            .collect();
        let r = FlowRegion {
            cells: [n as u32, n as u32],
            cell_m: 0.5,
            sources: vec![FlowSource {
                at: [3.0, 3.0],
                radius_m: 1.0,
                rate_m3_s: 0.1,
            }],
            open_edges: false,
            ..Default::default()
        };
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, bed);
        for _ in 0..30 * 20 {
            sim.step(1.0 / 30.0);
        }
        let lips = sim.lips(1.0);
        assert!(lips.len() > 5, "{} lips", lips.len());
        // One per brink cell, facing down the diagonal, not a zigzag of x and z strips.
        let diagonal = Vec2::ONE.normalize();
        let along = lips.iter().filter(|l| l.dir.dot(diagonal) > 0.95).count();
        assert!(
            along * 5 >= lips.len() * 4,
            "{along} of {} diagonal",
            lips.len()
        );
    }

    #[test]
    fn a_stepped_face_has_one_lip_at_its_top() {
        // The same shelf, with its face spread over two cells (4 m, 2 m, then the floor).
        let mut bed = vec![0.0f32; 40 * 6];
        for j in 0..6 {
            for i in 0..11 {
                bed[j * 40 + i] = if i < 10 { 4.0 } else { 2.0 };
            }
        }
        let r = FlowRegion {
            cells: [40, 6],
            cell_m: 0.5,
            sources: vec![FlowSource {
                at: [1.0, 1.5],
                radius_m: 0.6,
                rate_m3_s: 0.05,
            }],
            open_edges: false,
            ..Default::default()
        };
        let mut sim = FlowSim::new(Entity::PLACEHOLDER, DVec3::ZERO, &r, bed);
        for _ in 0..30 * 20 {
            sim.step(1.0 / 30.0);
        }
        let lips = sim.lips(1.0);
        assert!(!lips.is_empty(), "no lip");
        for lip in &lips {
            // Water on the face's middle step is already falling: no second curtain there.
            assert!((lip.top.x - 5.0).abs() < 1e-3, "{lip:?}");
            assert!(lip.drop > 3.8 && lip.drop < 4.2, "{lip:?}");
        }
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
        // A floor that tilts by a millimeter a cell is still a floor.
        let mut tilted = bed.clone();
        for j in 1..4 {
            for i in 1..4 {
                tilted[j * 7 + i] = 0.003 - 0.001 * i as f32;
            }
        }
        let d = flood(&tilted, 7, 5, 1.0, &[FlowFill::to_spill([1.5, 2.5])]);
        assert!((d[2 * 7 + 3] - 2.0).abs() < 1e-4, "{}", d[2 * 7 + 3]);
    }
}
