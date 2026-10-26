//! The cloud map: a 2D picture of the weather around the player (coverage, cloud type, rain,
//! density) that the volumetric clouds are carved from. Built on the CPU from the same
//! function that decides whether it rains at the player, so what you see is what falls.

use engine_core::math::{DVec2, Vec4};
use engine_core::prelude::*;

use crate::sim::WeatherSim;

/// Cells per side.
pub const MAP_SIZE: u32 = 128;
/// Cell size, meters (the map spans 128 km).
pub const MAP_CELL_M: f32 = 1000.0;
/// Precipitation stored in the map is divided by this (mm/h) to fit 0..1.
pub const MAP_PRECIP_SCALE: f32 = 50.0;

/// The weather map the renderer draws clouds from. Texels: coverage, cloud type,
/// precipitation / [`MAP_PRECIP_SCALE`], density.
#[derive(Resource, Debug, Clone)]
pub struct CloudMap {
    /// World X/Z of the map's minimum corner.
    pub origin: DVec2,
    pub size: u32,
    pub cell_m: f32,
    /// How far the clouds had drifted when the map was built (see [`WeatherSim::cloud_drift`],
    /// pattern X/Z).
    pub drift_at_build: DVec2,
    /// Weather time when built.
    pub built_at_s: f64,
    pub texels: Vec<[u8; 4]>,
    /// Weather far beyond the map (coverage, type, precipitation, density).
    pub background: Vec4,
    /// Increases every rebuild (the renderer re-uploads when it changes).
    pub revision: u64,
}

impl Default for CloudMap {
    fn default() -> Self {
        Self {
            origin: DVec2::splat(-(MAP_SIZE as f64) * MAP_CELL_M as f64 * 0.5),
            size: MAP_SIZE,
            cell_m: MAP_CELL_M,
            drift_at_build: DVec2::ZERO,
            built_at_s: f64::NEG_INFINITY,
            texels: vec![[0; 4]; (MAP_SIZE * MAP_SIZE) as usize],
            background: Vec4::new(0.3, 0.5, 0.0, 0.7),
            revision: 0,
        }
    }
}

fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Rebuild the map centered on the player.
pub fn build(map: &mut CloudMap, sim: &WeatherSim) {
    let mut next = Rebuild::start(map, sim);
    next.rows(sim, map.size as usize);
    next.finish(map, sim);
}

/// A rebuild in progress: the new texels fill a few rows per tick and replace the map's once
/// complete. Built whole, the map cost several milliseconds twice a second, a hitch each time.
pub(crate) struct Rebuild {
    origin: DVec2,
    drift: DVec2,
    time_s: f64,
    texels: Vec<[u8; 4]>,
    next_row: usize,
}

/// Rows of the map built per tick: the whole map in 8 ticks, well inside the half second
/// between rebuilds.
const ROWS_PER_TICK: usize = 16;

impl Rebuild {
    /// Start a rebuild centered on the player, at the weather as it is now.
    fn start(map: &CloudMap, sim: &WeatherSim) -> Self {
        let span = map.size as f64 * map.cell_m as f64;
        // Snap to whole cells so the pattern doesn't swim as the player moves.
        let cell = map.cell_m as f64;
        let size = map.size as usize;
        Self {
            origin: ((sim.focus - DVec2::splat(span * 0.5)) / cell).floor() * cell,
            drift: sim.cloud_drift,
            time_s: sim.time_s,
            texels: vec![[0; 4]; size * size],
            next_row: 0,
        }
    }

    fn done(&self, size: usize) -> bool {
        self.next_row >= size
    }

    /// Fill the next `rows` rows.
    fn rows(&mut self, sim: &WeatherSim, rows: usize) {
        let size = (self.texels.len() as f64).sqrt() as usize;
        let first = self.next_row.min(size);
        let last = (first + rows).min(size);
        self.next_row = last;
        let (origin, cell) = (self.origin, MAP_CELL_M as f64);
        let texels = &mut self.texels[first * size..last * size];
        engine_core::tasks::par_chunks_mut(texels, size * 4, |chunk, texels| {
            for (i, t) in texels.iter_mut().enumerate() {
                let index = first * size + chunk * size * 4 + i;
                let (x, y) = ((index % size) as f64, (index / size) as f64);
                let p = origin + DVec2::new(x + 0.5, y + 0.5) * cell;
                let c = sim.cell_at(p);
                *t = [
                    unorm(c.coverage),
                    unorm(c.cloud_type),
                    unorm(c.precipitation_mm_h / MAP_PRECIP_SCALE),
                    unorm(c.density),
                ];
            }
        });
    }

    /// Hand the finished texels to the map.
    fn finish(self, map: &mut CloudMap, sim: &WeatherSim) {
        map.origin = self.origin;
        map.drift_at_build = self.drift;
        map.built_at_s = self.time_s;
        map.texels = self.texels;
        let b = sim.base;
        map.background = Vec4::new(
            b.cloud_cover,
            b.cloud_type,
            b.precipitation_mm_h / MAP_PRECIP_SCALE,
            b.cloud_density,
        );
        map.revision += 1;
    }
}

/// FixedUpdate: rebuild the map twice per second of weather time, or when the player has
/// moved far from its center. A rebuild is spread over a few ticks; the first map, a jump
/// in weather time and a move past the map's edge are built at once.
pub(crate) fn cloud_map_system(
    sim: Res<WeatherSim>,
    mut map: ResMut<CloudMap>,
    mut pending: Local<Option<Rebuild>>,
) {
    let size = map.size as usize;
    let span = size as f64 * map.cell_m as f64;
    let center = map.origin + DVec2::splat(span * 0.5);
    let off = (center - sim.focus).abs().max_element();
    let since = (sim.time_s - map.built_at_s).abs();
    if map.revision == 0 || off > span * 0.4 || since > 5.0 {
        *pending = None;
        build(&mut map, &sim);
        return;
    }
    if pending.is_none() && (since >= 0.5 || off > span * 0.1) {
        *pending = Some(Rebuild::start(&map, &sim));
    }
    if let Some(next) = pending.as_mut() {
        next.rows(&sim, ROWS_PER_TICK);
        if next.done(size)
            && let Some(next) = pending.take()
        {
            next.finish(&mut map, &sim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::WeatherPreset;
    use crate::sim::Weather;

    #[test]
    fn map_matches_the_weather() {
        engine_core::tasks::init_task_pools();
        let mut map = CloudMap::default();
        for (preset, lo, hi) in [
            (WeatherPreset::Clear, 0.0, 0.25),
            (WeatherPreset::Overcast, 0.8, 1.0),
        ] {
            let sim = WeatherSim::new(&Weather {
                preset,
                ..Default::default()
            });
            build(&mut map, &sim);
            let mean = map.texels.iter().map(|t| t[0] as f32 / 255.0).sum::<f32>()
                / map.texels.len() as f32;
            assert!(mean > lo && mean < hi, "{preset:?} coverage {mean}");
        }
        assert!(map.revision >= 2);
    }
}
