//! Water wets the ground it reaches: waves run up a lake's shore and leave a band of dark, wet
//! sand above the still level (wider when the waves are bigger), streams soak their banks, and
//! the spray where a waterfall lands wets the ground and rock around its foot (walls read the
//! ground in front of them, so the cliff behind the falls darkens too).
//!
//! Writes `SurfaceLayers::wetness` around the focus. It only ever raises it: the weather dries
//! the ground as it dries rain, so a shore the water leaves dries out by itself. Ground under a
//! lake is left alone, and so is ground under a [`ShoreCover`].
//!
//! Each 1 m cell is judged over its whole footprint, not just its center: its height spread
//! (from the ground's slope) decides whether any of it reaches the water, so a narrow band on a
//! steep beach still wets every cell it touches.

use engine_core::math::{DVec2, DVec3, Vec2, smoothstep};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{FocusPoints, Ground, SurfaceLayers, TerrainChanged};

use crate::body::{WaterBed, WaterSettings};
use crate::flow::FlowSim;
use crate::state::SharedWater;

/// How often shores update, seconds.
const PERIOD: f32 = 0.25;
/// Each update visits every `STRIDE`th row of the grid, so the reach is refreshed once a
/// second.
const STRIDE: usize = 4;
/// Shores are wetted within this distance of the focus, meters.
const REACH_M: f64 = 128.0;
/// Ground heights sampled per update at most: after the grid moves, the first pass is spread
/// over a few updates.
const SAMPLES_PER_UPDATE: usize = 6000;
/// Height of the wet band above a lake's still level in calm water, meters: sand wicks water
/// up a few decimeters, and even ripples lap. Waves add 1.5 times their significant height.
const SWASH_M: f32 = 0.4;
/// Waves wet the shore only this close to the water, meters: low land far inside a sea's
/// extent stays dry.
const RUNUP_M: f64 = 24.0;
/// Cells per side of the blocks that remember where the lake bed is (for [`RUNUP_M`]).
const BLOCK: usize = 8;
/// How far beyond a stream's edge its bank is wet, meters.
const BANK_M: f32 = 1.5;
/// Falls at least this high throw spray (as for their splashes).
const SPRAY_DROP_M: f32 = 0.5;
/// Spray wets the ground at most this far from a fall's foot, meters.
const SPRAY_MAX_M: f32 = 14.0;
/// A cell whose highest point is this far under the water is under it, meters.
const UNDER_M: f32 = 0.05;
/// Steepest slope (rise over run) a cell's height spread allows for: a cliff beside the water
/// doesn't make the cells on top of it reach down to it.
const MAX_SLOPE: f32 = 3.0;
/// Cells without ground are sampled again this often, seconds (a terrain that arrives late).
const RETRY_S: f32 = 30.0;

/// Keeps the ground under it dry of lake, stream and spray water: a jetty's deck, a boathouse,
/// a quay. A rectangle in the entity's own x/z (meters, turned with it). Rain still falls on
/// the roof; this only stops the shore's wetness from showing on the floor above it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct ShoreCover {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Default for ShoreCover {
    fn default() -> Self {
        Self {
            min: [-1.0, -1.0],
            max: [1.0, 1.0],
        }
    }
}

/// The grid's ground, sampled once per cell while the grid stays put.
#[derive(Default)]
pub struct ShoreCache {
    origin: DVec2,
    size: (u32, u32),
    shifts: u64,
    /// Ground height per cell: NaN until sampled, infinite where there is no ground.
    heights: Vec<f64>,
    /// Half the height spread across each cell (from its slope), meters.
    spread: Vec<f32>,
    /// Blocks of [`BLOCK`]² cells holding ground under a lake.
    lakebed: Vec<bool>,
    /// Rows visited next.
    phase: usize,
    /// The ground provider changed since the last update.
    ground_changed: bool,
    /// Seconds since cells without ground were last retried.
    since_retry: f32,
}

impl ShoreCache {
    fn reset(&mut self, origin: DVec2, size: (u32, u32), shifts: u64) {
        let n = size.0 as usize * size.1 as usize;
        let blocks =
            size.0.div_ceil(BLOCK as u32) as usize * size.1.div_ceil(BLOCK as u32) as usize;
        *self = Self {
            origin,
            size,
            shifts,
            heights: vec![f64::NAN; n],
            spread: vec![0.0; n],
            lakebed: vec![false; blocks],
            // Keep turning through the rows: a cache reset every update must not visit only
            // one row in four.
            phase: self.phase,
            ground_changed: false,
            since_retry: 0.0,
        };
    }

    fn block(&self, i: usize, j: usize) -> usize {
        (j / BLOCK) * self.size.0.div_ceil(BLOCK as u32) as usize + i / BLOCK
    }

    /// Is there lake bed within about `cells` cells of cell (i, j)?
    fn lakebed_near(&self, i: usize, j: usize, cells: usize) -> bool {
        let bw = self.size.0.div_ceil(BLOCK as u32) as i64;
        let bh = self.size.1.div_ceil(BLOCK as u32) as i64;
        let r = (cells / BLOCK) as i64 + 1;
        let (bi, bj) = ((i / BLOCK) as i64, (j / BLOCK) as i64);
        (bj - r..=bj + r).filter(|y| (0..bh).contains(y)).any(|y| {
            (bi - r..=bi + r)
                .filter(|x| (0..bw).contains(x))
                .any(|x| self.lakebed[(y * bw + x) as usize])
        })
    }
}

/// How wet spray leaves the ground `d` meters from the foot of a fall of `power` watts: soaked
/// near the plunge and round its pool, drying out over a radius that grows with flow times
/// drop (out to about 10 m round a 1 m³/s fall over a 10 m cliff).
pub fn spray_wetness(d: f32, power: f32) -> f32 {
    let size = (power / 9810.0).max(0.0).sqrt();
    let reach = (2.0 + size * 2.5).min(SPRAY_MAX_M);
    1.0 - smoothstep(0.5 * reach, reach, d)
}

/// Wetness of a bank `d` meters from the edge of a stream (0.9 at the water, dry at
/// [`BANK_M`]).
pub fn bank_wetness(d: f32) -> f32 {
    0.9 * (1.0 - smoothstep(0.4 * BANK_M, BANK_M, d))
}

/// Wetness of lake shore whose lowest point is `low` meters above the still level, with waves
/// of significant height `hs`: soaked at the waterline, a band of wet sand, then dry.
pub fn swash_wetness(low: f32, hs: f32) -> f32 {
    let band = SWASH_M + 1.5 * hs.clamp(0.0, 2.0);
    0.95 * (1.0 - smoothstep(0.5 * band, band, low))
}

#[allow(clippy::too_many_arguments)]
pub fn shore_wetness_system(
    time: Res<Time>,
    settings: Res<WaterSettings>,
    ground: Res<Ground>,
    focus: Res<FocusPoints>,
    shared: Res<SharedWater>,
    cameras: Query<(&GlobalTransform, &Camera)>,
    covers: Query<(&GlobalTransform, &ShoreCover)>,
    mut changes: MessageReader<TerrainChanged>,
    mut surface: ResMut<SurfaceLayers>,
    mut accumulated: Local<f32>,
    mut cache: Local<ShoreCache>,
) {
    // Changes to the ground land between updates, so note them on every step.
    if ground.is_changed() {
        cache.ground_changed = true;
    }
    for c in changes.read() {
        forget(&mut cache, c.center, c.radius as f64);
    }
    if !settings.wet_shores {
        return;
    }
    *accumulated += time.delta_secs();
    if *accumulated < PERIOD {
        return;
    }
    let dt = std::mem::take(&mut *accumulated);
    let state = shared.0.read();
    let lakes: Vec<_> = state
        .bodies
        .iter()
        .filter(|b| matches!(b.body.bed, WaterBed::Ground))
        .map(|b| (b, b.waves.significant_height()))
        .collect();
    if lakes.is_empty() && state.flows.is_empty() {
        return;
    }
    let plunges: Vec<(DVec3, f32)> = state
        .flows
        .iter()
        .flat_map(|f| f.plunges(SPRAY_DROP_M))
        .map(|(foot, _, power)| (foot, power))
        .collect();
    let covers: Vec<(DVec3, Quat, ShoreCover)> = covers
        .iter()
        .map(|(g, c)| (g.translation(), g.rotation(), *c))
        .collect();
    let focus = crate::sim::focus_point(&focus, &cameras);

    let grid = &surface.grid;
    let (width, height) = (grid.width as usize, grid.height as usize);
    let (origin, cell) = (grid.origin, grid.cell as f64);
    if cache.origin != origin
        || cache.size != (grid.width, grid.height)
        || cache.shifts != state.shifts
    {
        cache.reset(origin, (grid.width, grid.height), state.shifts);
    }
    // A new ground provider usually describes the same ground: a terrain moved somewhere else,
    // or was edited, which `TerrainChanged` covers. Only cells that had no ground are asked
    // again, then and every so often.
    cache.since_retry += dt;
    if std::mem::take(&mut cache.ground_changed) || cache.since_retry > RETRY_S {
        cache.since_retry = 0.0;
        for h in cache.heights.iter_mut().filter(|h| h.is_infinite()) {
            *h = f64::NAN;
        }
    }
    let phase = cache.phase;
    cache.phase = (phase + 1) % STRIDE;

    let focus_xz = DVec2::new(focus.x, focus.z);
    let c0 = ((focus_xz - origin) / cell).floor();
    let reach = (REACH_M / cell).ceil() as i64;
    let rows = (c0.y as i64 - reach).max(0)..=(c0.y as i64 + reach).min(height as i64 - 1);
    let cols = (c0.x as i64 - reach).max(0)..=(c0.x as i64 + reach).min(width as i64 - 1);
    let runup_cells = (RUNUP_M / cell).ceil() as usize;
    let mut budget = SAMPLES_PER_UPDATE;
    let mut writes: Vec<(usize, f32)> = Vec::new();
    for j in rows.filter(|j| *j as usize % STRIDE == phase) {
        let j = j as usize;
        for i in cols.clone() {
            let i = i as usize;
            let xz = origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * cell;
            if (xz - focus_xz).length_squared() > REACH_M * REACH_M {
                continue;
            }
            let p = DVec3::new(xz.x, focus.y + 50.0, xz.y);
            // Only cells water could reach: inside a lake, beside a stream, near a fall.
            let near_lake = lakes.iter().any(|(b, _)| b.contains(p));
            let near_flow = state
                .flows
                .iter()
                .any(|f| beside(f, p, BANK_M as f64 + 1.0));
            let near_fall = plunges
                .iter()
                .any(|(foot, _)| (DVec2::new(foot.x, foot.z) - xz).length() < SPRAY_MAX_M as f64);
            if !(near_lake || near_flow || near_fall) {
                continue;
            }
            let idx = j * width + i;
            if cache.heights[idx].is_nan() {
                if budget == 0 {
                    continue;
                }
                budget -= 1;
                let (h, spread) = match ground.sample(p) {
                    Some(g) => {
                        let n = g.normal.normalize_or(Vec3::Y);
                        let slope = (1.0 - n.y * n.y).max(0.0).sqrt() / n.y.max(1e-3);
                        // Half the cell's diagonal up and down from its center.
                        (g.height, 0.71 * cell as f32 * slope.min(MAX_SLOPE))
                    }
                    None => (f64::INFINITY, 0.0),
                };
                cache.heights[idx] = h;
                cache.spread[idx] = spread;
            }
            let h = cache.heights[idx];
            if h.is_infinite() {
                continue;
            }
            let spread = cache.spread[idx];
            let ground_p = DVec3::new(xz.x, h, xz.y);

            let mut target = 0.0f32;
            let mut under_lake = false;
            for (b, hs) in lakes.iter().filter(|(b, _)| b.contains(ground_p)) {
                let above = (h - b.center.y) as f32;
                if above + spread < -UNDER_M {
                    under_lake = true;
                } else if cache.lakebed_near(i, j, runup_cells) {
                    target = target.max(swash_wetness((above - spread).max(0.0), *hs));
                }
            }
            if under_lake {
                let block = cache.block(i, j);
                cache.lakebed[block] = true;
                continue;
            }
            for (foot, power) in &plunges {
                // Spray hangs low: ground well above the foot (the top of the cliff) stays dry.
                let rise = (h - spread as f64 - foot.y).max(0.0) * 1.5;
                let d = (DVec2::new(foot.x, foot.z) - xz).length().hypot(rise) as f32;
                target = target.max(spray_wetness(d, *power));
            }
            for f in state
                .flows
                .iter()
                .filter(|f| beside(f, ground_p, BANK_M as f64 + 1.0))
            {
                if let Some(d) = bank_distance(f, ground_p, spread) {
                    target = target.max(bank_wetness(d));
                }
            }
            if target > surface.grid.data[idx].wetness + 0.02 && !covered(&covers, xz, cell) {
                writes.push((idx, target));
            }
        }
    }
    if !writes.is_empty() {
        let grid = &mut surface.grid;
        for (idx, w) in writes {
            grid.data[idx].wetness = w.min(1.0);
        }
    }
}

/// Sample the ground within `radius` of `center` again (it was edited).
fn forget(cache: &mut ShoreCache, center: DVec3, radius: f64) {
    if cache.heights.is_empty() {
        return;
    }
    let (w, h) = (cache.size.0 as i64, cache.size.1 as i64);
    let c = DVec2::new(center.x, center.z) - cache.origin;
    let lo = (c - (radius + 1.0)).floor().as_i64vec2();
    let hi = (c + (radius + 1.0)).ceil().as_i64vec2();
    for j in lo.y.max(0)..hi.y.min(h) {
        for i in lo.x.max(0)..hi.x.min(w) {
            cache.heights[(j * w + i) as usize] = f64::NAN;
        }
    }
}

/// Is the 1 m cell at `xz` under a [`ShoreCover`]? Covers keep a cell round their edge dry too:
/// the renderer blends neighboring cells, and a wet cell beside a deck would show on its edge.
fn covered(covers: &[(DVec3, Quat, ShoreCover)], xz: DVec2, cell: f64) -> bool {
    covers.iter().any(|(at, rotation, c)| {
        let d = (xz - DVec2::new(at.x, at.z)).as_vec2();
        let local = rotation.inverse() * Vec3::new(d.x, 0.0, d.y);
        let l = Vec2::new(local.x, local.z);
        let margin = cell as f32;
        l.x >= c.min[0] - margin
            && l.y >= c.min[1] - margin
            && l.x <= c.max[0] + margin
            && l.y <= c.max[1] + margin
    })
}

/// Is `p` over `f`'s grid or within `margin` meters of it?
fn beside(f: &FlowSim, p: DVec3, margin: f64) -> bool {
    let l = f.local(p);
    let size = DVec2::new(f.nx as f64, f.nz as f64) * f.cell as f64;
    l.x > -margin && l.y > -margin && l.x < size.x + margin && l.y < size.y + margin
}

/// How far the ground at `p` (a cell whose height spreads `spread` meters either way) is from
/// the edge of `f`'s water, meters, if it is a bank. A cell that rises out of the water at its
/// edge is right at it; one wholly under the stream isn't a bank. Ground higher above the water
/// than spray climbs (more from fast water) counts as that much further away.
fn bank_distance(f: &FlowSim, p: DVec3, spread: f32) -> Option<f32> {
    let l = f.local(p);
    let c = f.cell as f64;
    let (gi, gj) = ((l.x / c).floor() as i64, (l.y / c).floor() as i64);
    let at = |i: i64, j: i64| {
        (i >= 0 && j >= 0 && i < f.nx as i64 && j < f.nz as i64)
            .then(|| j as usize * f.nx + i as usize)
    };
    let surface = |idx: usize| f.origin.y + (f.bed[idx] + f.depth[idx]) as f64;
    let top = p.y + spread as f64;
    let low = p.y - spread as f64;
    if let Some(idx) = at(gi, gj).filter(|&idx| f.depth[idx] > 0.02) {
        return (top > surface(idx) + UNDER_M as f64).then_some(0.0);
    }
    let k = (BANK_M as f64 / c).ceil() as i64 + 1;
    let mut best: Option<f32> = None;
    for j in gj - k..=gj + k {
        for i in gi - k..=gi + k {
            let Some(idx) = at(i, j) else { continue };
            if f.depth[idx] <= 0.01 {
                continue;
            }
            let climb = 0.25 + 0.15 * f.vel[idx].length();
            let over = (low - surface(idx) - climb as f64).max(0.0) as f32;
            if over >= BANK_M {
                continue;
            }
            // From the edge of the wet cell, not its center.
            let center = DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * c;
            let d = ((center - l).length() - c * 0.5).max(0.0) as f32 + over;
            best = Some(best.map_or(d, |b| b.min(d)));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shores_are_wet_by_the_water_and_dry_above() {
        assert!(swash_wetness(0.0, 0.0) > 0.9);
        assert!(swash_wetness(0.15, 0.0) > 0.9);
        assert_eq!(swash_wetness(1.0, 0.0), 0.0);
        // Bigger waves run further up.
        assert!(swash_wetness(0.6, 0.6) > 0.8);
        assert!(bank_wetness(0.2) > 0.85 && bank_wetness(BANK_M) == 0.0);
        // A 1 m³/s fall over 10 m soaks its pool and wets the ground several meters out.
        let power = 1000.0 * 9.81 * 10.0;
        assert!(spray_wetness(4.0, power) > 0.99);
        assert!(spray_wetness(7.0, power) > 0.5);
        assert_eq!(spray_wetness(20.0, power), 0.0);
    }

    #[test]
    fn covers_keep_their_floor_and_a_cell_round_it_dry() {
        // A deck 18 m long and 2 m wide from (10, 0, 0), turned a quarter so that its +x
        // runs along world -z.
        let deck = ShoreCover {
            min: [0.0, -1.0],
            max: [18.0, 1.0],
        };
        let turned = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let covers = [(DVec3::new(10.0, 0.0, 0.0), turned, deck)];
        assert!(covered(&covers, DVec2::new(10.0, -9.0), 1.0));
        assert!(covered(&covers, DVec2::new(11.5, -9.0), 1.0));
        assert!(!covered(&covers, DVec2::new(13.0, -9.0), 1.0));
        assert!(!covered(&covers, DVec2::new(10.0, 9.0), 1.0));
    }
}
