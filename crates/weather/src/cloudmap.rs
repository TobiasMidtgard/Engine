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
    let span = map.size as f64 * map.cell_m as f64;
    // Snap to whole cells so the pattern doesn't swim as the player moves.
    let cell = map.cell_m as f64;
    let origin = ((sim.focus - DVec2::splat(span * 0.5)) / cell).floor() * cell;
    map.origin = origin;
    map.drift_at_build = sim.cloud_drift;
    map.built_at_s = sim.time_s;
    let size = map.size as usize;
    map.texels.resize(size * size, [0; 4]);
    engine_core::tasks::par_chunks_mut(&mut map.texels, size * 8, |chunk, texels| {
        for (i, t) in texels.iter_mut().enumerate() {
            let index = chunk * size * 8 + i;
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
    let b = sim.base;
    map.background = Vec4::new(
        b.cloud_cover,
        b.cloud_type,
        b.precipitation_mm_h / MAP_PRECIP_SCALE,
        b.cloud_density,
    );
    map.revision += 1;
}

/// FixedUpdate: rebuild the map twice per second of weather time, or when the player has
/// moved far from its center.
pub(crate) fn cloud_map_system(sim: Res<WeatherSim>, mut map: ResMut<CloudMap>) {
    let span = map.size as f64 * map.cell_m as f64;
    let center = map.origin + DVec2::splat(span * 0.5);
    let moved = (center - sim.focus).abs().max_element() > span * 0.1;
    if (sim.time_s - map.built_at_s).abs() >= 0.5 || moved || map.revision == 0 {
        build(&mut map, &sim);
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
