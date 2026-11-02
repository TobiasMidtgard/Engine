//! The rivers' and lakes' water for gameplay: the world's [`Water`] answers from the terrains'
//! rivers and lakes wherever no water body does, so people wade and swim in them, boats float
//! and splashes splash. The surface and the bed come from the same shape the renderer draws
//! (the river's level over its carved channel, a lake's level over its basin), the current
//! from the river's fall (`rivers::current_mps`).
//!
//! The terrain wraps whatever query the water module installed: its bodies answer first.

use std::sync::Arc;

use engine_core::math::{DVec3, Vec3};
use engine_world::{Water, WaterQuery, WaterSample};

use crate::ground::GroundTerrain;

/// Water of a river or lake shallower than this reaching a point counts as none (the bank's
/// last wet centimeters).
const MIN_DEPTH_M: f64 = 0.02;

/// [`Water`]'s query while terrains answer the ground: the inner query's water bodies, then the
/// terrains' rivers and lakes.
pub struct TerrainWater {
    pub inner: Arc<dyn WaterQuery>,
    pub terrains: Vec<GroundTerrain>,
}

impl WaterQuery for TerrainWater {
    fn sample(&self, p: DVec3) -> Option<WaterSample> {
        if let Some(s) = self.inner.sample(p) {
            return Some(s);
        }
        self.terrains.iter().find_map(|t| t.water(p))
    }
}

impl GroundTerrain {
    /// The river or lake water over the ground under (or around) world point `p`.
    pub fn water(&self, p: DVec3) -> Option<WaterSample> {
        let rivers = self.source.shape.river_network_solved()??;
        let local = self.rotation.inverse() * (p - self.position);
        let (dir, up, base) = self.source.map.locate(local)?;
        // Most of the world has no river or lake near (and the hand-made ground none at all):
        // answered without sampling the shape.
        let shape = &self.source.shape;
        if shape.inside_hand_made(dir) || !rivers.may_hold_water(dir, shape.radius_m) {
            return None;
        }
        let s = self.source.sample(base, up, dir, 1.0);
        let depth = s.water_level_m - s.height;
        if s.water_near < 0.5 || depth < MIN_DEPTH_M {
            return None;
        }
        let surface = self.position + self.rotation * (base + up * s.water_level_m);
        let current = self.rotation * s.current;
        Some(WaterSample {
            surface_height: surface.y,
            depth: depth as f32,
            velocity: current.as_vec3(),
            normal: (self.rotation * up).as_vec3().normalize_or(Vec3::Y),
        })
    }
}

/// What [`install`] put in [`Water`], and the query it wraps.
#[derive(Default)]
pub(crate) struct Installed {
    ours: Option<Arc<TerrainWater>>,
    inner: Option<Arc<dyn WaterQuery>>,
}

fn same(a: &Arc<TerrainWater>, b: &Arc<dyn WaterQuery>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

/// Wrap the installed water query with the terrains' rivers and lakes (`terrains` empty: put
/// back what was there). `changed`: the terrains answering the ground changed.
pub(crate) fn install(
    water: &mut Water,
    state: &mut Installed,
    changed: bool,
    terrains: impl FnOnce() -> Vec<GroundTerrain>,
) {
    let ours_now = state.ours.as_ref().is_some_and(|o| same(o, &water.0));
    if !ours_now {
        // Another module installed its query (or none was wrapped yet): wrap that one.
        state.inner = Some(water.0.clone());
        state.ours = None;
    }
    if ours_now && !changed {
        return;
    }
    let list = terrains();
    let inner = state.inner.clone().unwrap_or_else(|| Water::default().0);
    if list.is_empty() {
        if state.ours.take().is_some() {
            *water = Water(inner);
        }
        return;
    }
    let ours = Arc::new(TerrainWater {
        inner,
        terrains: list,
    });
    *water = Water(ours.clone());
    state.ours = Some(ours);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkSource;
    use crate::cube::{SurfaceMap, TerrainDomain};
    use crate::edits::EditLayer;
    use crate::{FlatPad, PlanetTerrain, dir_from_lat_lon};
    use engine_core::math::DQuat;

    /// The earthlike test world (seed 7) with its pad at 40° N 0° E, rivers solved.
    fn earth() -> GroundTerrain {
        let planet = PlanetTerrain::earthlike(7).with_pad(FlatPad::default());
        let shape = Arc::new(planet.shape.clone());
        assert!(
            shape.river_network().is_some(),
            "the earthlike world has rivers"
        );
        GroundTerrain {
            source: ChunkSource {
                map: SurfaceMap::new(TerrainDomain::Sphere, shape.radius_m),
                shape,
                edits: Arc::new(EditLayer::default()),
                ocean_surface: true,
            },
            palette: planet.palette.clone(),
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            surface_override: None,
        }
    }

    #[test]
    fn a_lake_is_deep_enough_to_swim_in_and_the_pad_is_dry() {
        let t = earth();
        let r = t.source.shape.radius_m;
        // A lake 9 km west of the pad (its floor some 30 m under its surface at 67 m).
        let dir = dir_from_lat_lon(40.01, -0.104);
        let w = t.water(dir * (r + 40.0)).expect("the lake's water");
        assert!(w.depth > 2.0, "deep enough to swim: {} m", w.depth);
        let s = t.source.shape.sample(dir, 1.0);
        let surface = dir * (r + s.water_level_m);
        assert!((w.surface_height - surface.y).abs() < 0.01);
        assert!(s.height < s.water_level_m, "the basin keeps its floor");
        assert!(w.velocity.length() < 1e-6, "lakes are still");
        assert!(t.water(dir_from_lat_lon(40.0, 0.0) * (r + 10.0)).is_none());
    }

    #[test]
    fn the_wrapped_query_answers_first() {
        struct Everywhere;
        impl WaterQuery for Everywhere {
            fn sample(&self, p: DVec3) -> Option<WaterSample> {
                Some(WaterSample {
                    surface_height: p.y + 1.0,
                    depth: 9.0,
                    velocity: Vec3::ZERO,
                    normal: Vec3::Y,
                })
            }
        }
        let mut water = Water(Arc::new(Everywhere));
        let mut state = Installed::default();
        install(&mut water, &mut state, true, || vec![earth()]);
        let s = water
            .sample(DVec3::new(0.0, 5.0, 0.0))
            .expect("the body's water");
        assert_eq!(s.depth, 9.0);
        // Installed once: nothing changes, nothing is rebuilt.
        let before = water.0.clone();
        install(&mut water, &mut state, false, || unreachable!());
        assert!(Arc::ptr_eq(&before, &water.0));
        // The terrains go: the module's own query is back.
        install(&mut water, &mut state, true, Vec::new);
        assert!(!state.ours.is_some());
        assert_eq!(water.sample(DVec3::ZERO).map(|s| s.depth), Some(9.0));
    }
}
