//! The valley's woods for the world's [`ForestCover`]. The planet terrain paints its own woods
//! everywhere, but out to the overlay the landscape is the ground, so foliage's near and far
//! woods should grow where the landscape is wooded: never on the zones' pads, the runway, the
//! roads, the fields, the lake or the footpath, and only scattered trees on open grass. Beyond
//! the overlay's blend the terrain's own woods take over.

use crate::landscape::{Cover, Landscape};
use crate::planet::{OVERLAY_BLEND, OVERLAY_RADIUS};
use engine::prelude::*;
use engine::world::{ForestCover, ForestQuery, ForestSample};
use std::sync::Arc;

/// The landscape's woods in front of the terrain's.
struct ValleyForest {
    land: Arc<Landscape>,
    terrain: Arc<dyn ForestQuery>,
}

impl ValleyForest {
    /// Canopy cover the landscape gives (x, z): 0 where something else is built or grown.
    fn density(&self, x: f64, z: f64) -> f32 {
        let l = &self.land;
        if l.pad_weight(x, z) > 0.1
            || l.road_weight(x, z) > 0.0
            || crate::places::near_lake_trail(x, z, 4.0)
        {
            return 0.0;
        }
        let h = l.height(x, z);
        match l.cover(x, z, h, l.normal(x, z, 2.0)) {
            Cover::Forest => l.woodland(x, z) as f32,
            // Open grass carries a tree here and there (as the valley's own scatter does);
            // fields' grass carries none.
            Cover::Grass if l.field_at(x, z).is_none() => (l.woodland(x, z).powi(2) * 0.6) as f32,
            _ => 0.0,
        }
    }
}

impl ForestQuery for ValleyForest {
    fn sample(&self, p: DVec3) -> Option<ForestSample> {
        let terrain = self.terrain.sample(p);
        let r = (p.x * p.x + p.z * p.z).sqrt();
        // Far out (or far above, in space) the terrain's woods are the only ones.
        if r > OVERLAY_RADIUS + OVERLAY_BLEND || (p.y - self.land.height(p.x, p.z)).abs() > 3000.0 {
            return terrain;
        }
        let t = ((r - OVERLAY_RADIUS) / OVERLAY_BLEND).clamp(0.0, 1.0) as f32;
        let valley = self.density(p.x, p.z);
        let density = valley + (terrain.map_or(0.0, |s| s.density) - valley) * t;
        (density > 0.0).then(|| ForestSample {
            density,
            conifer: terrain.map_or(0.35, |s| s.conifer),
        })
    }
}

/// The provider this module installed last, so a new one from the terrain gets wrapped again.
#[derive(Default)]
pub struct Installed(Option<Arc<dyn ForestQuery>>);

/// Put the valley's woods in front of whatever forest provider the terrain installs.
pub fn install(mut cover: ResMut<ForestCover>, mut installed: Local<Installed>) {
    if !cover.is_changed()
        || installed
            .0
            .as_ref()
            .is_some_and(|a| Arc::ptr_eq(a, &cover.0))
    {
        return;
    }
    let ours: Arc<dyn ForestQuery> = Arc::new(ValleyForest {
        land: crate::landscape::shared().clone(),
        terrain: cover.0.clone(),
    });
    cover.0 = ours.clone();
    installed.0 = Some(ours);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::{RUNWAY, zone};

    struct Everywhere;
    impl ForestQuery for Everywhere {
        fn sample(&self, _p: DVec3) -> Option<ForestSample> {
            Some(ForestSample {
                density: 1.0,
                conifer: 0.5,
            })
        }
    }

    #[test]
    fn no_woods_on_the_runway_or_the_zones() {
        let land = crate::landscape::shared().clone();
        let forest = ValleyForest {
            land: land.clone(),
            terrain: Arc::new(Everywhere),
        };
        let at = |x: f64, z: f64| forest.sample(DVec3::new(x, land.height(x, z), z));
        let (a, b) = (DVec2::from_array(RUNWAY.0), DVec2::from_array(RUNWAY.1));
        for k in 0..=20 {
            let p = a.lerp(b, k as f64 / 20.0);
            assert_eq!(at(p.x, p.y), None, "runway at {p}");
        }
        for key in ["airfield", "spaceport", "track", "plaza", "physics"] {
            let c = zone(key).center;
            assert_eq!(at(c.x, c.z), None, "{key}");
        }
        // The hills' woods stand, and far out the terrain's.
        let wooded = (-20..=20)
            .flat_map(|i| (-20..=20).map(move |j| (i as f64 * 50.0, j as f64 * 50.0)))
            .filter(|&(x, z)| at(x, z).is_some_and(|s| s.density > 0.5))
            .count();
        assert!(wooded > 20, "{wooded} wooded spots");
        assert!(at(40_000.0, 0.0).is_some());
    }
}
