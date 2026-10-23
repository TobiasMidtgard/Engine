//! Rain and snow change the ground: wetness, puddles and snow depth in the shared
//! [`SurfaceLayers`] grid around the player.
//!
//! * Rain wets exposed ground within a millimeter or so; what the ground can't soak up
//!   (concrete, rock, packed or already soaked soil) collects in the low spots as puddles.
//! * Snow piles up when the air at the ground is below freezing, and melts by degree-days when
//!   it warms (fires melt it too, through the heat field).
//! * Sun, wind, warmth and dry air evaporate it all again; puddles also drain into soil.
//! * Covered ground stays dry: roofs with physics colliders are found with upward raycasts
//!   (a few thousand per update, nearest first), and [`WeatherShelter`] marks covers without
//!   colliders.
//!
//! The renderer shades wet, puddled and snowy ground from the same grid; footsteps, fire and
//! vehicle grip read it too.

use engine_core::math::noise::perlin2;
use engine_core::math::{DVec2, DVec3, IVec2, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{
    Grid2, Ground, HeatField, PrecipitationKind, SurfaceId, SurfaceLayers, SurfaceMaterials,
    WeatherState,
};

use crate::climate;
use crate::sim::WeatherSim;

/// Tuning for how rain and snow change the ground.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundWeather {
    /// Updates per second of game time.
    pub rate_hz: f32,
    /// Rain that makes dry ground fully wet, mm.
    pub wetting_mm: f32,
    /// Deepest puddle, meters.
    pub max_puddle_m: f32,
    /// Share of flat ground where water collects (little dips), 0..1.
    pub puddle_share: f32,
    /// Evaporation on a warm, sunny, breezy day, mm/h.
    pub evaporation_mm_h: f32,
    /// How much water fully absorbent soil soaks up, mm/h.
    pub infiltration_mm_h: f32,
    /// Snow depth per depth of water (10 = 1 cm of snow per mm of water).
    pub snow_ratio: f32,
    /// Snow melt per °C above freezing per day, mm of water (degree-day factor).
    pub melt_mm_per_c_day: f32,
    /// Upward raycasts per update that look for roofs over ground not seen yet; known ground
    /// is re-checked with a 32nd of that.
    pub shelter_rays: u32,
    /// Speed up drying and wetting (1 = physical; time-lapse demos use more).
    pub time_scale: f32,
}

impl Default for GroundWeather {
    fn default() -> Self {
        Self {
            rate_hz: 4.0,
            wetting_mm: 0.8,
            max_puddle_m: 0.04,
            puddle_share: 0.3,
            evaporation_mm_h: 0.6,
            infiltration_mm_h: 10.0,
            snow_ratio: 10.0,
            melt_mm_per_c_day: 4.0,
            shelter_rays: 2048,
            time_scale: 1.0,
        }
    }
}

/// Keeps rain and snow off the ground under it for the ground simulation: a roof, a tent, a
/// bridge. Physics colliders do this automatically; use this for covers without one. The
/// footprint is a box of `half_extents` (X, Z) around the entity, and it covers everything
/// below the entity's height.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherShelter {
    /// Half size of the covered footprint in X and Z, meters.
    pub half_extents: [f32; 2],
}

impl Default for WeatherShelter {
    fn default() -> Self {
        Self {
            half_extents: [2.0, 2.0],
        }
    }
}

/// Overrides what the ground is made of under a footprint, for ground the terrain doesn't
/// describe (a paved yard, a sand pit, a rooftop): how much water it soaks up and how much of
/// it collects puddles.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundPatch {
    /// Half size of the footprint in X and Z, meters.
    pub half_extents: [f32; 2],
    /// 0 (asphalt, rock) .. 1 (loose soil, sand).
    pub absorbency: f32,
    /// Share of the patch where water collects, 0..1.
    pub puddles: f32,
}

impl Default for GroundPatch {
    fn default() -> Self {
        Self {
            half_extents: [2.0, 2.0],
            absorbency: 0.1,
            puddles: 0.4,
        }
    }
}

/// What the ground simulation knows about one cell that doesn't change with the weather.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellInfo {
    /// 1 open sky .. 0 under a roof.
    pub exposure: f32,
    /// Ground height, meters.
    pub height: f32,
    /// How much the surface soaks up, 0 rock .. 1 soil.
    pub absorbency: f32,
    /// Where water collects, 0 (high spot) .. 1 (dip): the larger of `spots` and a real
    /// hollow in the ground around the cell.
    pub dip: f32,
    /// Where small unevenness the terrain doesn't model collects water (hard ground), 0..1.
    pub spots: f32,
    /// Raycast done.
    pub known: bool,
}

impl Default for CellInfo {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            height: 0.0,
            absorbency: 0.5,
            dip: 0.0,
            spots: 0.0,
            known: false,
        }
    }
}

/// Per-cell ground facts, laid out like [`SurfaceLayers`].
#[derive(Resource, Debug, Clone)]
pub struct GroundCache {
    pub cells: Grid2<CellInfo>,
    /// Cell indices nearest the grid center first.
    order: Vec<u32>,
    /// Where the search for cells not looked at yet continues.
    cursor: usize,
    /// Every cell has been looked at since the grid last moved.
    complete: bool,
    /// Where the slow re-check of known cells continues.
    sweep: usize,
    /// Seconds since each band of rows was last updated.
    band_age: [f32; GROUND_BANDS],
    /// The band updated next.
    next_band: usize,
    /// Bands and raycasts owed to the update rate, carried between ticks.
    band_credit: f32,
    ray_credit: f32,
    /// Cells covered by a [`WeatherShelter`] this update.
    sheltered: Vec<bool>,
}

impl Default for GroundCache {
    fn default() -> Self {
        Self {
            cells: Grid2::new(DVec2::ZERO, 1.0, 0, 0, CellInfo::default()),
            order: Vec::new(),
            cursor: 0,
            complete: false,
            sweep: 0,
            band_age: [0.0; GROUND_BANDS],
            next_band: 0,
            band_credit: 0.0,
            ray_credit: 0.0,
            sheltered: Vec::new(),
        }
    }
}

impl GroundCache {
    /// 1 open sky .. 0 covered (a roof or a [`WeatherShelter`]) for cell `index`; ground not
    /// looked at yet counts as open.
    pub fn exposure(&self, index: usize) -> f32 {
        if self.sheltered.get(index).copied().unwrap_or(false) {
            0.0
        } else {
            self.cells.data.get(index).map_or(1.0, |c| c.exposure)
        }
    }

    /// Follow the surface grid: same layout, keeping what overlaps when it moves. Returns the
    /// grid's previous origin when it moved.
    fn align(&mut self, surface: &Grid2<engine_world::SurfaceCell>) -> Option<DVec2> {
        let same_dims = self.cells.width == surface.width
            && self.cells.height == surface.height
            && self.cells.cell == surface.cell;
        if !same_dims {
            self.cells = Grid2::new(
                surface.origin,
                surface.cell,
                surface.width,
                surface.height,
                CellInfo::default(),
            );
            self.order = near_first_order(surface.width, surface.height);
            self.cursor = 0;
            self.complete = false;
        } else if self.cells.origin != surface.origin {
            let old = self.cells.origin;
            let half =
                DVec2::new(surface.width as f64, surface.height as f64) * surface.cell as f64 * 0.5;
            self.cells.recenter(surface.origin + half);
            self.cells.origin = surface.origin;
            self.cursor = 0;
            self.complete = false;
            self.sheltered.resize(self.cells.len(), false);
            return Some(old);
        }
        self.sheltered.resize(self.cells.len(), false);
        None
    }
}

/// The cells a moving grid uncovers start out bare. Give them the wetness and snow of the
/// open ground the grid kept (its upper quartile), so walking out of a snowfield doesn't leave
/// a bare band along the far edge of the grid. A jump past the whole grid keeps nothing to go
/// by, and the ground starts bare there.
fn seed_uncovered(
    surface: &mut Grid2<engine_world::SurfaceCell>,
    cache: &GroundCache,
    old_origin: DVec2,
) {
    let (origin, cell, width) = (surface.origin, surface.cell as f64, surface.width as usize);
    let size = DVec2::new(width as f64, surface.height as f64) * cell;
    let kept = |i: usize| {
        let xz = origin + (DVec2::new((i % width) as f64, (i / width) as f64) + 0.5) * cell;
        let rel = xz - old_origin;
        rel.x >= 0.0 && rel.y >= 0.0 && rel.x < size.x && rel.y < size.y
    };
    let (mut wet, mut snow) = (Vec::new(), Vec::new());
    for i in (0..surface.data.len()).step_by(7) {
        if kept(i) && cache.exposure(i) >= 0.5 {
            wet.push(surface.data[i].wetness);
            snow.push(surface.data[i].snow_m);
        }
    }
    if wet.is_empty() {
        return;
    }
    let upper_quartile = |v: &mut Vec<f32>| {
        let k = v.len() * 3 / 4;
        *v.select_nth_unstable_by(k, f32::total_cmp).1
    };
    let (wetness, snow_m) = (upper_quartile(&mut wet), upper_quartile(&mut snow));
    if wetness <= 0.0 && snow_m <= 0.0 {
        return;
    }
    for i in 0..surface.data.len() {
        if !kept(i) {
            let c = &mut surface.data[i];
            c.wetness = c.wetness.max(wetness);
            c.snow_m = c.snow_m.max(snow_m);
        }
    }
}

fn near_first_order(w: u32, h: u32) -> Vec<u32> {
    let mut order: Vec<u32> = (0..w * h).collect();
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    order.sort_by_key(|&i| {
        let (x, y) = ((i % w) as f32 + 0.5 - cx, (i / w) as f32 + 0.5 - cy);
        ((x * x + y * y) * 16.0) as u32
    });
    order
}

/// Little dips where puddles form: smooth noise over a few meters, thresholded so about
/// `share` of flat ground collects water.
pub fn puddle_dip(p: DVec2, share: f32, seed: u64) -> f32 {
    let n =
        perlin2(p.x / 4.5, p.y / 4.5, seed) * 0.7 + perlin2(p.x / 1.7, p.y / 1.7, seed ^ 9) * 0.3;
    // n is close to normally distributed with sigma ~0.24: keep the lowest `share` of it
    // (logistic approximation of the normal quantile).
    let s = share.clamp(0.001, 0.999) as f64;
    let t = 0.24 * (s / (1.0 - s)).ln() / 1.702;
    smoothstep(t as f32 + 0.04, t as f32 - 0.1, n)
}

/// Inputs to one ground update, gathered from the world.
pub struct GroundInputs<'a> {
    pub weather: &'a WeatherState,
    pub heat: Option<&'a HeatField>,
    pub dt: f32,
}

/// Update every cell of `surface` for `dt` seconds of weather.
pub fn update_cells(
    surface: &mut SurfaceLayers,
    cache: &GroundCache,
    params: &GroundWeather,
    input: &GroundInputs,
) {
    let rows = surface.grid.height as usize;
    update_rows(surface, cache, params, input, 0..rows);
}

/// Update rows `rows` of `surface` for `dt` seconds of weather.
pub fn update_rows(
    surface: &mut SurfaceLayers,
    cache: &GroundCache,
    params: &GroundWeather,
    input: &GroundInputs,
    rows: std::ops::Range<usize>,
) {
    let w = input.weather;
    let dt_h = input.dt * params.time_scale / 3600.0;
    if dt_h <= 0.0 {
        return;
    }
    let rate = w.precipitation_mm_h.max(0.0);
    let falling_snow = w.precipitation_kind == PrecipitationKind::Snow;
    let sleet = w.precipitation_kind == PrecipitationKind::Sleet;
    let sun = w.daylight() * (1.0 - 0.8 * w.cloud_cover);
    let wind = w.wind.length();
    // Evaporation potential, mm/h: warmer, sunnier, windier and drier air dries faster.
    let warmth = climate::saturation_vapor_pressure_hpa(w.temperature_c)
        / climate::saturation_vapor_pressure_hpa(20.0);
    let evap = params.evaporation_mm_h
        * warmth
        * (0.3 + 0.7 * sun)
        * (0.5 + 0.5 * (wind / 5.0).min(2.0))
        * ((1.0 - w.humidity) * 2.0).clamp(0.0, 1.5);
    let grid = &mut surface.grid;
    let width = grid.width as usize;
    let origin = grid.origin;
    let cell_size = grid.cell as f64;
    let infos = &cache.cells.data;
    let sheltered = &cache.sheltered;
    let heat = input.heat;
    let melt_per_c_h = params.melt_mm_per_c_day / 24.0;
    let max_puddle = params.max_puddle_m;
    let wetting = params.wetting_mm.max(0.05);
    let snow_ratio = params.snow_ratio.max(1.0);
    let infiltration = params.infiltration_mm_h;
    let share = params.puddle_share.max(0.05);

    let first = rows.start * width;
    let end = (rows.end * width).min(grid.data.len());
    if first >= end {
        return;
    }
    let chunk = width * 4;
    engine_core::tasks::par_chunks_mut(&mut grid.data[first..end], chunk, |chunk_index, cells| {
        for (i, cell) in cells.iter_mut().enumerate() {
            let index = first + chunk_index * chunk + i;
            let info = infos.get(index).copied().unwrap_or_default();
            let exposure = if sheltered.get(index).copied().unwrap_or(false) {
                0.0
            } else {
                info.exposure
            };
            let (x, y) = ((index % width) as f64, (index / width) as f64);
            let p = DVec3::new(
                origin.x + (x + 0.5) * cell_size,
                info.height as f64,
                origin.y + (y + 0.5) * cell_size,
            );
            let heat_c = heat.map(|h| h.grid.sample_nearest(p)).unwrap_or(0.0);
            let t = w.temperature_at_altitude(p.y) + heat_c;
            let input_mm = rate * exposure * dt_h;

            // Precipitation: snow piles up where it's freezing, rain wets and pools.
            let mut water_mm = 0.0;
            if falling_snow || (sleet && t < 1.0) {
                if t <= 1.0 {
                    cell.snow_m += input_mm * snow_ratio / 1000.0;
                } else {
                    water_mm += input_mm;
                }
            } else {
                water_mm += input_mm;
            }

            // Melt: degree-days, more in sun and in rain; melt water joins the rain.
            if cell.snow_m > 0.0 && t > 0.0 {
                let melt_mm =
                    (melt_per_c_h * t * (1.0 + sun) + 0.0125 * rate * t * exposure) * dt_h;
                let snow_water_mm = cell.snow_m * 1000.0 / snow_ratio;
                let melted = melt_mm.min(snow_water_mm);
                cell.snow_m = ((snow_water_mm - melted) * snow_ratio / 1000.0).max(0.0);
                water_mm += melted;
            }

            // Wetting, soaking in, pooling.
            let soak_mm = infiltration * info.absorbency * dt_h;
            cell.wetness = (cell.wetness + water_mm / wetting).min(1.0);
            let excess_mm = (water_mm - soak_mm).max(0.0);
            // Water runs off high spots into dips (volume roughly conserved over the area).
            let pooled_mm = excess_mm * info.dip / share;
            cell.puddle_m = (cell.puddle_m + pooled_mm / 1000.0).min(max_puddle * info.dip);

            // Drying: puddles evaporate and drain; damp ground dries once puddles are gone.
            let dry_mm = evap * dt_h * exposure.max(0.25);
            let drain_mm = infiltration * 0.25 * info.absorbency * dt_h;
            if cell.puddle_m > 0.0 {
                cell.puddle_m = (cell.puddle_m - (dry_mm + drain_mm) / 1000.0).max(0.0);
            } else if water_mm <= 0.0 && cell.snow_m <= 0.0 {
                cell.wetness = (cell.wetness - dry_mm / wetting).max(0.0);
            }
            // Snow on the ground keeps the surface under it wet.
            if cell.snow_m > 0.005 {
                cell.wetness = cell.wetness.max(0.6);
            }
        }
    });
}

/// Bands of rows the ground update takes in turn, one or a few per tick.
const GROUND_BANDS: usize = 32;

/// Roofs higher than this above the ground don't keep rain off it (m).
const ROOF_SEARCH_M: f32 = 120.0;
/// Colliders this close above the ground's height are the ground itself (m): the physics
/// terrain is built separately and can sit a little off it.
const GROUND_SLACK_M: f32 = 1.0;

/// How far around the player API calls look at the ground before changing it (m).
pub(crate) const PREPARE_RADIUS_M: f64 = 64.0;

/// Ring sizes that find hollows in the ground, small and large (m).
const HOLLOW_RADII_M: [f64; 2] = [2.0, 4.5];
/// How far below the ground around it a spot starts holding water, and where it is a full
/// puddle (m).
const HOLLOW_M: [f32; 2] = [0.02, 0.07];

/// Look up roofs and ground facts for up to `rays` cells: first the cells not looked at yet
/// (nearest the player first), then a slow sweep over known cells that notices roofs built
/// or torn down since. Each look is one upward raycast, so once the area is known this costs
/// a few dozen rays per update rather than `rays`.
pub fn refresh_cache(
    cache: &mut GroundCache,
    rays: u32,
    ground: &Ground,
    physics: Option<&PhysicsQueries>,
    materials: &SurfaceMaterials,
    share: f32,
    sim: &WeatherSim,
) {
    let n = cache.order.len();
    if n == 0 {
        return;
    }
    let look = LookUp {
        ground,
        physics,
        materials,
        share,
        sim,
        filter: QueryFilter::default(),
    };
    let mut budget = rays.min(n as u32);
    let mut looked = Vec::new();
    if !cache.complete {
        let mut scanned = 0;
        while budget > 0 && scanned < n {
            let index = cache.order[cache.cursor] as usize;
            cache.cursor = (cache.cursor + 1) % n;
            scanned += 1;
            if !cache.cells.data[index].known {
                looked.push(index);
                budget -= 1;
            }
        }
        cache.complete = scanned == n;
    }
    for _ in 0..(rays / 32).max(1).min(budget) {
        looked.push(cache.order[cache.sweep] as usize);
        cache.sweep = (cache.sweep + 1) % n;
    }
    looked.sort_unstable();
    looked.dedup();
    look_at(cache, &look, &looked);
}

/// Look at cells `indices` on all cores (each is a raycast and a ground sample), then
/// recompute the dips their new heights touch. Done one cell at a time, a ground update
/// cost a quarter second of the frame while the player moved, mostly in recomputing the
/// same neighbors' dips dozens of times over.
fn look_at(cache: &mut GroundCache, look: &LookUp, indices: &[usize]) {
    let mut found: Vec<(usize, CellInfo)> =
        indices.iter().map(|&i| (i, CellInfo::default())).collect();
    let cells = &cache.cells;
    engine_core::tasks::par_chunks_mut(&mut found, 128, |_, chunk| {
        for (index, info) in chunk {
            *info = look.facts(cells, *index);
        }
    });
    for &(index, info) in &found {
        cache.cells.data[index] = info;
    }
    update_dips(cache, indices, look.sim);
}

/// Look at the ground within `radius_m` of the player now, for changes that apply at once
/// (`weather.ground`, `weather.advance`) right after the grid moved: the cache follows the
/// surface grid, every cell in reach is known, and shelters and patches are stamped in.
pub(crate) fn prepare(world: &mut World, radius_m: f64) {
    let shelters: Vec<(DVec3, WeatherShelter)> = world
        .query::<(&GlobalTransform, &WeatherShelter)>()
        .iter(world)
        .map(|(gt, s)| (gt.translation(), *s))
        .collect();
    let patches: Vec<(DVec3, GroundPatch)> = world
        .query::<(&GlobalTransform, &GroundPatch)>()
        .iter(world)
        .map(|(gt, p)| (gt.translation(), *p))
        .collect();
    world.resource_scope(|world, mut cache: Mut<GroundCache>| {
        let cache = &mut *cache;
        if let Some(old) = cache.align(&world.resource::<SurfaceLayers>().grid) {
            seed_uncovered(&mut world.resource_mut::<SurfaceLayers>().grid, cache, old);
        }
        let sim = world.resource::<WeatherSim>();
        let look = LookUp {
            ground: world.resource::<Ground>(),
            physics: world.get_resource::<PhysicsQueries>(),
            materials: world.resource::<SurfaceMaterials>(),
            share: world.resource::<GroundWeather>().puddle_share,
            sim,
            filter: QueryFilter::default(),
        };
        let (w, h) = (cache.cells.width, cache.cells.height);
        let reach = radius_m / cache.cells.cell as f64;
        let mut found: Vec<(usize, CellInfo)> = Vec::new();
        for &index in &cache.order {
            let x = (index % w) as f64 + 0.5 - w as f64 * 0.5;
            let y = (index / w) as f64 + 0.5 - h as f64 * 0.5;
            if x * x + y * y > reach * reach {
                // Nearest first: everything after this is farther.
                break;
            }
            if !cache.cells.data[index as usize].known {
                found.push((index as usize, CellInfo::default()));
            }
        }
        // Tens of thousands of cells after a teleport: look at them on all cores.
        let indices: Vec<usize> = found.iter().map(|&(index, _)| index).collect();
        look_at(cache, &look, &indices);
        stamp(
            cache,
            shelters.iter().map(|(p, s)| (*p, s)),
            patches.iter().map(|(p, g)| (*p, g)),
            sim,
        );
    });
}

struct LookUp<'a> {
    ground: &'a Ground,
    physics: Option<&'a PhysicsQueries>,
    materials: &'a SurfaceMaterials,
    share: f32,
    sim: &'a WeatherSim,
    filter: QueryFilter,
}

impl LookUp<'_> {
    /// What the ground at cell `index` of `cells` is like.
    fn facts(&self, cells: &Grid2<CellInfo>, index: usize) -> CellInfo {
        let w = cells.width;
        let c = IVec2::new((index as u32 % w) as i32, (index as u32 / w) as i32);
        let xz = cells.cell_center(c);
        let sample = self.ground.sample(DVec3::new(xz.x, 0.0, xz.y));
        let height = sample.map(|s| s.height).unwrap_or(0.0);
        // Unknown ground soaks like the world's default material.
        let absorbency = self
            .materials
            .get(sample.map(|s| s.surface).unwrap_or(SurfaceId::DEFAULT))
            .absorbency;
        let flat = sample.map(|s| s.normal.y).unwrap_or(1.0);
        // Look down from the sky for the first thing above the ground. Looking up from the
        // ground instead started under the physics terrain wherever it sits a little above
        // the height the ground reports, and took the terrain for a roof.
        let exposure = match self.physics {
            Some(q) => {
                let from = DVec3::new(xz.x, height + ROOF_SEARCH_M as f64, xz.y);
                let reach = ROOF_SEARCH_M - GROUND_SLACK_M;
                if q.raycast(from, -Vec3::Y, reach, &self.filter).is_some() {
                    0.0
                } else {
                    1.0
                }
            }
            None => 1.0,
        };
        // Hard ground pools in small unevenness the terrain doesn't model (a spot pattern that
        // keeps its place when the world shifts). Soft ground (a grass meadow, soil) drains:
        // water stands only where the ground itself dips (see `hollow_dip`).
        let share_here = self.share * (1.0 - smoothstep(0.2, 0.65, absorbency));
        let spots = puddle_dip(self.sim.frame.to_pattern(xz), share_here, self.sim.seed)
            * smoothstep(0.88, 0.97, flat);
        CellInfo {
            exposure,
            height: height as f32,
            absorbency,
            dip: spots,
            spots,
            known: true,
        }
    }
}

/// Recompute the dips of the known cells whose hollow rings reach any of cells `changed`
/// (their heights are new), each once, on all cores.
fn update_dips(cache: &mut GroundCache, changed: &[usize], sim: &WeatherSim) {
    let (axis, diagonal) = hollow_rings(cache.cells.cell);
    let reach = axis.max(diagonal);
    let cells = &cache.cells;
    let w = cells.width;
    let mut touched = vec![false; cells.data.len()];
    let mut targets: Vec<(usize, f32)> = Vec::new();
    for &index in changed {
        let c = IVec2::new((index as u32 % w) as i32, (index as u32 / w) as i32);
        for y in -reach..=reach {
            for x in -reach..=reach {
                if let Some(i) = cells.index(c + IVec2::new(x, y))
                    && !touched[i]
                    && cells.data[i].known
                {
                    touched[i] = true;
                    targets.push((i, 0.0));
                }
            }
        }
    }
    engine_core::tasks::par_chunks_mut(&mut targets, 256, |_, chunk| {
        for (i, dip) in chunk {
            let c = IVec2::new((*i as u32 % w) as i32, (*i as u32 / w) as i32);
            *dip = hollow_dip(cells, c, sim);
        }
    });
    for (i, dip) in targets {
        cache.cells.data[i].dip = dip;
    }
}

/// Cell offsets of the two rings that find hollows: along the axes and along the diagonals.
fn hollow_rings(cell: f32) -> (i32, i32) {
    let cells = |m: f64| ((m / cell as f64).round() as i32).max(1);
    (
        cells(HOLLOW_RADII_M[0]),
        cells(HOLLOW_RADII_M[1] * std::f64::consts::FRAC_1_SQRT_2),
    )
}

/// Where water collects in cell `c`: its spots, or a real hollow, where the ground lies lower
/// than the ground around it (ruts, dips in a meadow, a dry stream bed). A steady slope
/// cancels out; ground not looked at yet counts as level with the cell.
fn hollow_dip(cells: &Grid2<CellInfo>, c: IVec2, sim: &WeatherSim) -> f32 {
    let Some(info) = cells.index(c).map(|i| cells.data[i]) else {
        return 0.0;
    };
    let h = info.height;
    let at = |x: i32, y: i32| {
        cells
            .index(c + IVec2::new(x, y))
            .map(|i| cells.data[i])
            .filter(|n| n.known)
            .map_or(h, |n| n.height)
    };
    let (a, d) = hollow_rings(cells.cell);
    let along = (at(a, 0) + at(-a, 0) + at(0, a) + at(0, -a)) * 0.25 - h;
    let across = (at(d, d) + at(-d, d) + at(d, -d) + at(-d, -d)) * 0.25 - h;
    // Water doesn't stand on a hillside, even in a gully.
    let slope = (at(1, 0) - at(-1, 0)).hypot(at(0, 1) - at(0, -1)) / (2.0 * cells.cell);
    let level = 1.0 - smoothstep(0.25, 0.54, slope);
    // Ragged shores: the water doesn't fill a hollow's rim evenly.
    let pattern = sim.frame.to_pattern(cells.cell_center(c));
    let shore = 0.75 + 0.25 * perlin2(pattern.x / 1.3, pattern.y / 1.3, sim.seed ^ 5);
    let hollow = smoothstep(HOLLOW_M[0], HOLLOW_M[1], along.max(across)) * shore * level;
    info.spots.max(hollow.min(1.0))
}

/// FixedUpdate: rain, snow, melting and drying on the ground around the player.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ground_system(
    time: Res<Time>,
    params: Res<GroundWeather>,
    weather: Res<WeatherState>,
    ground: Res<Ground>,
    materials: Res<SurfaceMaterials>,
    physics: Option<Res<PhysicsQueries>>,
    heat: Option<Res<HeatField>>,
    sim: Res<crate::sim::WeatherSim>,
    shelters: Query<(&GlobalTransform, &WeatherShelter)>,
    patches: Query<(&GlobalTransform, &GroundPatch)>,
    mut cache: ResMut<GroundCache>,
    mut surface: ResMut<SurfaceLayers>,
) {
    // The work is spread over the ticks: each updates a band of rows (every band
    // `rate_hz` times a second) and casts its share of the roof rays. Updating the whole
    // grid at once took tens of milliseconds and, on a busy PC, a quarter second: a hitch
    // every update, and a slide show once frames were slow enough for it to run every frame.
    let tick = time.delta_secs();
    let rate = params.rate_hz.max(0.1);
    for age in &mut cache.band_age {
        *age += tick;
    }
    cache.band_credit =
        (cache.band_credit + GROUND_BANDS as f32 * rate * tick).min(GROUND_BANDS as f32);
    cache.ray_credit = (cache.ray_credit + params.shelter_rays as f32 * rate * tick)
        .min(params.shelter_rays as f32);
    let bands = cache.band_credit.floor() as usize;
    if bands == 0 {
        return;
    }
    cache.band_credit -= bands as f32;
    let rays = cache.ray_credit.floor() as u32;
    cache.ray_credit -= rays as f32;

    if let Some(old) = cache.align(&surface.grid) {
        seed_uncovered(&mut surface.grid, &cache, old);
    }
    refresh_cache(
        &mut cache,
        rays,
        &ground,
        physics.as_deref(),
        &materials,
        params.puddle_share,
        &sim,
    );
    stamp(
        &mut cache,
        shelters.iter().map(|(gt, s)| (gt.translation(), s)),
        patches.iter().map(|(gt, p)| (gt.translation(), p)),
        &sim,
    );
    let height = surface.grid.height as usize;
    let band_rows = height.div_ceil(GROUND_BANDS);
    for _ in 0..bands {
        let band = cache.next_band;
        cache.next_band = (band + 1) % GROUND_BANDS;
        let dt = std::mem::take(&mut cache.band_age[band]);
        update_rows(
            &mut surface,
            &cache,
            &params,
            &GroundInputs {
                weather: &weather,
                heat: heat.as_deref(),
                dt,
            },
            band * band_rows..((band + 1) * band_rows).min(height),
        );
    }
}

/// Mark the ground under shelters and stamp the ground patches into the cache.
fn stamp<'a>(
    cache: &mut GroundCache,
    shelters: impl Iterator<Item = (DVec3, &'a WeatherShelter)>,
    patches: impl Iterator<Item = (DVec3, &'a GroundPatch)>,
    sim: &WeatherSim,
) {
    cache.sheltered.fill(false);
    for (p, shelter) in shelters {
        let grid = &cache.cells;
        let h = DVec2::new(
            shelter.half_extents[0] as f64,
            shelter.half_extents[1] as f64,
        );
        let lo = grid.cell_at(DVec3::new(p.x - h.x, 0.0, p.z - h.y));
        let hi = grid.cell_at(DVec3::new(p.x + h.x, 0.0, p.z + h.y));
        for y in lo.y.max(0)..=hi.y.min(grid.height as i32 - 1) {
            for x in lo.x.max(0)..=hi.x.min(grid.width as i32 - 1) {
                if let Some(i) = grid.index(IVec2::new(x, y))
                    && (grid.data[i].height as f64) < p.y
                {
                    cache.sheltered[i] = true;
                }
            }
        }
    }
    for (p, patch) in patches {
        apply_patch(cache, p, patch, sim);
    }
}

/// Stamp a [`GroundPatch`] centered on `at` into the cache.
pub fn apply_patch(cache: &mut GroundCache, at: DVec3, patch: &GroundPatch, sim: &WeatherSim) {
    let grid = &mut cache.cells;
    let h = DVec2::new(patch.half_extents[0] as f64, patch.half_extents[1] as f64);
    let lo = grid.cell_at(DVec3::new(at.x - h.x, 0.0, at.z - h.y));
    let hi = grid.cell_at(DVec3::new(at.x + h.x, 0.0, at.z + h.y));
    for y in lo.y.max(0)..=hi.y.min(grid.height as i32 - 1) {
        for x in lo.x.max(0)..=hi.x.min(grid.width as i32 - 1) {
            let c = IVec2::new(x, y);
            let xz = grid.cell_center(c);
            // Only cells whose middle is on the patch: the grass around a yard stays grass.
            let off = (xz - DVec2::new(at.x, at.z)).abs();
            if off.x > h.x || off.y > h.y {
                continue;
            }
            if let Some(i) = grid.index(c) {
                let info = &mut grid.data[i];
                info.absorbency = patch.absorbency.clamp(0.0, 1.0);
                info.dip = puddle_dip(sim.frame.to_pattern(xz), patch.puddles, sim.seed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (SurfaceLayers, GroundCache, GroundWeather) {
        engine_core::tasks::init_task_pools();
        let surface = SurfaceLayers {
            grid: Grid2::centered(DVec2::ZERO, 64.0, 1.0, Default::default()),
        };
        let mut cache = GroundCache::default();
        cache.align(&surface.grid);
        let ground = Ground::default();
        refresh_cache(
            &mut cache,
            100_000,
            &ground,
            None,
            &SurfaceMaterials::default(),
            0.3,
            &WeatherSim::default(),
        );
        (surface, cache, GroundWeather::default())
    }

    /// Makes the (flat) test ground soak like `absorbency` (it is grass by default).
    fn ground_soaking(cache: &mut GroundCache, absorbency: f32) {
        let ground = Ground::default();
        let mut materials = SurfaceMaterials::default();
        let id = ground.sample(DVec3::ZERO).unwrap().surface;
        materials.materials[id.0 as usize].absorbency = absorbency;
        // Look at every cell again.
        for c in &mut cache.cells.data {
            c.known = false;
        }
        cache.complete = false;
        refresh_cache(
            cache,
            100_000,
            &ground,
            None,
            &materials,
            0.3,
            &WeatherSim::default(),
        );
    }

    fn rain(mm_h: f32, t: f32) -> WeatherState {
        WeatherState {
            precipitation_mm_h: mm_h,
            precipitation_kind: if mm_h > 0.0 {
                crate::sim::kind_for(t)
            } else {
                PrecipitationKind::None
            },
            temperature_c: t,
            humidity: 0.9,
            cloud_cover: 1.0,
            ..Default::default()
        }
    }

    fn run(
        surface: &mut SurfaceLayers,
        cache: &GroundCache,
        p: &GroundWeather,
        w: &WeatherState,
        seconds: f32,
    ) {
        let steps = (seconds / 10.0).ceil() as usize;
        for _ in 0..steps {
            update_cells(
                surface,
                cache,
                p,
                &GroundInputs {
                    weather: w,
                    heat: None,
                    dt: 10.0,
                },
            );
        }
    }

    fn stats(surface: &SurfaceLayers) -> (f32, f32, f32, f32) {
        let n = surface.grid.data.len() as f32;
        let wet = surface.grid.data.iter().map(|c| c.wetness).sum::<f32>() / n;
        let puddled = surface
            .grid
            .data
            .iter()
            .filter(|c| c.puddle_m > 0.002)
            .count() as f32
            / n;
        let deepest = surface
            .grid
            .data
            .iter()
            .map(|c| c.puddle_m)
            .fold(0.0, f32::max);
        let snow = surface.grid.data.iter().map(|c| c.snow_m).sum::<f32>() / n;
        (wet, puddled, deepest, snow)
    }

    #[test]
    fn rain_wets_the_ground_and_fills_puddles_then_it_dries() {
        let (mut surface, mut cache, params) = setup();
        // Hard-packed ground so water pools quickly.
        ground_soaking(&mut cache, 0.15);
        run(&mut surface, &cache, &params, &rain(10.0, 12.0), 600.0);
        let (wet, puddled, deepest, _) = stats(&surface);
        assert!(wet > 0.95, "wet {wet}");
        assert!(puddled > 0.1 && puddled < 0.6, "puddled share {puddled}");
        assert!(deepest > 0.003, "deepest {deepest}");

        let mut sunny = rain(0.0, 24.0);
        sunny.humidity = 0.4;
        sunny.cloud_cover = 0.0;
        sunny.wind = Vec3::new(6.0, 0.0, 0.0);
        run(&mut surface, &cache, &params, &sunny, 6.0 * 3600.0);
        let (wet, puddled, _, _) = stats(&surface);
        assert!(wet < 0.2, "still wet {wet}");
        assert!(puddled < 0.02, "puddles left {puddled}");
    }

    #[test]
    fn known_ground_is_only_rechecked_slowly() {
        // Each look is an upward raycast; a full budget every update stalled worlds whose
        // physics backend marches rays through the terrain.
        let (_, mut cache, _) = setup();
        assert!(cache.complete);
        let (cursor, sweep) = (cache.cursor, cache.sweep);
        refresh_cache(
            &mut cache,
            2048,
            &Ground::default(),
            None,
            &SurfaceMaterials::default(),
            0.3,
            &WeatherSim::default(),
        );
        assert_eq!(cache.cursor, cursor);
        assert_eq!(cache.sweep, sweep + 64);
    }

    #[test]
    fn a_meadow_holds_far_fewer_puddles_than_hard_ground() {
        let downpour = rain(40.0, 12.0);
        let mut puddled = Vec::new();
        for absorbency in [0.15, 0.6] {
            let (mut surface, mut cache, params) = setup();
            ground_soaking(&mut cache, absorbency);
            run(&mut surface, &cache, &params, &downpour, 1800.0);
            puddled.push(stats(&surface).1);
        }
        assert!(puddled[0] > 0.15, "hard ground {}", puddled[0]);
        assert!(
            puddled[1] < puddled[0] * 0.4,
            "a meadow pools {} vs {} on hard ground",
            puddled[1],
            puddled[0]
        );
    }

    /// A grass meadow, flat but for a bowl 20 cm deep at (10, -8) and a hump at (-10, 10).
    struct Meadow;
    impl engine_world::GroundQuery for Meadow {
        fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
            let shape = |c: DVec2, r: f64, h: f64| {
                let d = (DVec2::new(p.x, p.z) - c).length() / r;
                h * (1.0 - d * d).max(0.0)
            };
            let height =
                shape(DVec2::new(10.0, -8.0), 3.0, -0.2) + shape(DVec2::new(-10.0, 10.0), 3.0, 0.3);
            Some(engine_world::GroundSample {
                height,
                normal: Vec3::Y,
                surface: Ground::default().sample(p)?.surface,
            })
        }
    }

    #[test]
    fn water_stands_in_a_meadows_real_hollows() {
        let (mut surface, _, params) = setup();
        let mut cache = GroundCache::default();
        cache.align(&surface.grid);
        refresh_cache(
            &mut cache,
            100_000,
            &Ground(std::sync::Arc::new(Meadow)),
            None,
            &SurfaceMaterials::default(),
            0.3,
            &WeatherSim::default(),
        );
        run(&mut surface, &cache, &params, &rain(40.0, 12.0), 1800.0);
        let puddle = |x: f64, z: f64| surface.sample(DVec3::new(x, 0.0, z)).puddle_m;
        assert!(puddle(10.0, -8.0) > 0.01, "bowl {}", puddle(10.0, -8.0));
        assert_eq!(puddle(-10.0, 10.0), 0.0, "hump");
        let open_meadow = surface
            .grid
            .data
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let xz = surface.grid.cell_center(IVec2::new(
                    (*i as u32 % surface.grid.width) as i32,
                    (*i as u32 / surface.grid.width) as i32,
                ));
                (xz - DVec2::new(10.0, -8.0)).length() > 5.0
            })
            .map(|(_, c)| c)
            .collect::<Vec<_>>();
        let puddled = open_meadow.iter().filter(|c| c.puddle_m > 0.002).count();
        assert!(
            (puddled as f32) < open_meadow.len() as f32 * 0.03,
            "{puddled} of {} cells of open meadow hold water",
            open_meadow.len()
        );
    }

    /// Physics with a terrain 40 cm above the height the ground reports (they are built
    /// separately) and a roof 2.5 m up over 6 × 6 m around the origin. Rays hit both from
    /// either side, like a heightfield does from below.
    struct RoofOverTerrain;
    impl engine_world::physics::PhysicsQuery for RoofOverTerrain {
        fn raycast(
            &self,
            origin: DVec3,
            dir: Vec3,
            max_distance: f32,
            _: &QueryFilter,
        ) -> Option<engine_world::physics::Hit> {
            let under_roof = origin.x.abs() < 3.0 && origin.z.abs() < 3.0;
            let planes: &[f64] = if under_roof { &[0.4, 2.5, 2.7] } else { &[0.4] };
            planes
                .iter()
                .map(|&y| ((y - origin.y) / dir.y as f64) as f32)
                .filter(|t| (0.0..=max_distance).contains(t))
                .min_by(f32::total_cmp)
                .map(|t| engine_world::physics::Hit {
                    entity: Entity::PLACEHOLDER,
                    point: origin + dir.as_dvec3() * t as f64,
                    normal: -dir,
                    distance: t,
                    surface: SurfaceId::DEFAULT,
                })
        }
        fn shapecast(
            &self,
            _: &engine_world::physics::ColliderShape,
            _: DVec3,
            _: engine_core::math::Quat,
            _: Vec3,
            _: f32,
            _: &QueryFilter,
        ) -> Option<engine_world::physics::Hit> {
            None
        }
        fn overlap(
            &self,
            _: &engine_world::physics::ColliderShape,
            _: DVec3,
            _: engine_core::math::Quat,
            _: &QueryFilter,
            _: &mut Vec<Entity>,
        ) {
        }
        fn closest_point(
            &self,
            _: DVec3,
            _: f32,
            _: &QueryFilter,
        ) -> Option<engine_world::physics::Hit> {
            None
        }
    }

    #[test]
    fn roofs_keep_rain_off_but_the_terrain_is_no_roof() {
        let (surface, _, _) = setup();
        let mut cache = GroundCache::default();
        cache.align(&surface.grid);
        refresh_cache(
            &mut cache,
            100_000,
            &Ground::default(),
            Some(&PhysicsQueries(std::sync::Arc::new(RoofOverTerrain))),
            &SurfaceMaterials::default(),
            0.3,
            &WeatherSim::default(),
        );
        let exposure = |x: f64, z: f64| {
            let c = cache.cells.cell_at(DVec3::new(x, 0.0, z));
            cache.cells.data[cache.cells.index(c).unwrap()].exposure
        };
        assert_eq!(exposure(0.5, -1.5), 0.0, "under the roof");
        assert_eq!(exposure(10.5, -6.5), 1.0, "open meadow");
    }

    #[test]
    fn snow_piles_up_in_the_cold_and_melts_in_the_warm() {
        let (mut surface, cache, params) = setup();
        run(
            &mut surface,
            &cache,
            &params,
            &rain(3.0, -5.0),
            2.0 * 3600.0,
        );
        let (_, _, _, snow) = stats(&surface);
        // 6 mm of water ≈ 6 cm of snow.
        assert!((snow - 0.06).abs() < 0.01, "snow {snow}");
        let mut thaw = rain(0.0, 8.0);
        thaw.humidity = 0.6;
        run(&mut surface, &cache, &params, &thaw, 6.0 * 3600.0);
        let (wet, _, _, snow) = stats(&surface);
        assert!(snow < 0.005, "snow left {snow}");
        // The melt water leaves the ground wet for a while.
        assert!(wet > 0.2, "wet {wet}");
    }

    #[test]
    fn ground_a_moving_grid_uncovers_starts_like_the_open_ground_it_kept() {
        let (mut surface, mut cache, params) = setup();
        run(&mut surface, &cache, &params, &rain(3.0, -5.0), 3600.0);
        let lying = surface.sample(DVec3::ZERO).snow_m;
        assert!(lying > 0.02, "snow {lying}");
        // The player walks 16 m east: the grid drops its west strip and uncovers one east.
        surface.grid.recenter(DVec2::new(16.0, 0.0));
        let old = cache.align(&surface.grid).expect("the grid moved");
        seed_uncovered(&mut surface.grid, &cache, old);
        let strip = surface.sample(DVec3::new(44.0, 0.0, 5.0));
        assert!(
            (strip.snow_m - lying).abs() < 0.002,
            "{} vs {lying}",
            strip.snow_m
        );
        assert!(strip.wetness > 0.5, "wetness {}", strip.wetness);

        // A jump past the whole grid has nothing to go by.
        surface.grid.recenter(DVec2::new(5000.0, 0.0));
        let old = cache.align(&surface.grid).expect("the grid moved");
        seed_uncovered(&mut surface.grid, &cache, old);
        assert_eq!(surface.sample(DVec3::new(5000.0, 0.0, 0.0)).snow_m, 0.0);
    }

    #[test]
    fn sheltered_ground_stays_dry() {
        let (mut surface, mut cache, params) = setup();
        let center = surface.grid.cell_at(DVec3::ZERO);
        let i = surface.grid.index(center).unwrap();
        cache.sheltered[i] = true;
        run(&mut surface, &cache, &params, &rain(10.0, 12.0), 600.0);
        assert_eq!(surface.grid.data[i].wetness, 0.0);
        assert!(surface.grid.data[i + 5].wetness > 0.9);
    }
}
