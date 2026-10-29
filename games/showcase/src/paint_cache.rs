//! The valley's ground paint for the terrain's patch builds, baked on grids. The terrain asks
//! the overlay's paint at every vertex, and nine more times per vertex on far patches (to
//! average the cell), and working out the landscape's noise and covers costs about 15 µs a
//! time: over 100 ms for one far patch. So away from the camera the paint comes from grids of
//! baked samples, finer near and coarser far (4 m to 256 m), each baked in tiles the first
//! time a patch asks there and looked up bilinearly after. Near the camera, where the paint is
//! seen up close, it is worked out exactly.

use crate::landscape::Landscape;
use engine::prelude::*;
use engine_terrain::OverlayPaint;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Samples per tile side, less one (the last row and column repeat the next tile's first, so a
/// lookup reads one tile).
const TILE: usize = 32;
/// The finest grid's spacing (m); each level doubles it.
const FINEST_M: f64 = 4.0;
const LEVELS: u32 = 7;
/// Ground this far from the camera per meter of distance is what a patch's tap stands for (the
/// terrain's vertex spacing grows with distance), until the terrain says so itself.
const FOOTPRINT_PER_M: f64 = 0.01;
/// Tiles kept before the least recently used half goes (about 26 KB each).
const MAX_TILES: usize = 2000;

/// The camera, for the footprint (NaN until known: then every lookup is exact).
static EYE: [AtomicU64; 3] = [
    AtomicU64::new(f64::NAN.to_bits()),
    AtomicU64::new(f64::NAN.to_bits()),
    AtomicU64::new(f64::NAN.to_bits()),
];

/// Keep the paint's idea of where the camera is up to date.
pub fn track_eye(cameras: Query<(&Camera, &GlobalTransform)>) {
    if let Some(eye) = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
    {
        for (slot, v) in EYE.iter().zip(eye.to_array()) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
    }
}

fn eye() -> DVec3 {
    let [x, y, z] = EYE
        .each_ref()
        .map(|v| f64::from_bits(v.load(Ordering::Relaxed)));
    DVec3::new(x, y, z)
}

type Tile = Arc<OnceLock<Box<[OverlayPaint]>>>;

/// The landscape's paint, exact near the camera and from baked grids farther off.
pub struct PaintCache {
    land: Arc<Landscape>,
    exact: fn(&Landscape, f64, f64) -> OverlayPaint,
    tiles: Mutex<Tiles>,
}

/// The baked tiles by level and tile coordinates, each with when it was last asked for.
#[derive(Default)]
struct Tiles {
    map: HashMap<(u32, i64, i64), (Tile, u64)>,
    clock: u64,
}

impl PaintCache {
    pub fn new(land: Arc<Landscape>, exact: fn(&Landscape, f64, f64) -> OverlayPaint) -> Self {
        Self {
            land,
            exact,
            tiles: Mutex::default(),
        }
    }

    /// The paint at (x, z), worked out as finely as the terrain asks (its paint map) or else as
    /// the camera's distance asks.
    pub fn paint(&self, x: f64, z: f64) -> OverlayPaint {
        let footprint = engine_terrain::paint_footprint().unwrap_or_else(|| {
            let eye = eye();
            (x - eye.x).hypot(z - eye.z) * FOOTPRINT_PER_M
        });
        self.paint_for(x, z, footprint)
    }

    /// The paint at (x, z) for a tap standing for `footprint` meters of ground.
    pub fn paint_for(&self, x: f64, z: f64, footprint: f64) -> OverlayPaint {
        // (NaN before the camera is known: exact.)
        if footprint.is_nan() || footprint < FINEST_M {
            return (self.exact)(&self.land, x, z);
        }
        let level = ((footprint / FINEST_M).log2() as u32).min(LEVELS - 1);
        let spacing = FINEST_M * (1u32 << level) as f64;
        let (gx, gz) = (x / spacing, z / spacing);
        let (ix, iz) = (gx.floor() as i64, gz.floor() as i64);
        let (fx, fz) = ((gx - ix as f64) as f32, (gz - iz as f64) as f32);
        let t = TILE as i64;
        let (lx, lz) = (ix.rem_euclid(t) as usize, iz.rem_euclid(t) as usize);
        let tile = self.tile(level, ix.div_euclid(t), iz.div_euclid(t), spacing);
        let samples = tile.get().expect("baked by `tile`");
        let at = |i: usize, j: usize| samples[(lz + j) * (TILE + 1) + lx + i];
        let corners = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
        let weights = [
            (1.0 - fx) * (1.0 - fz),
            fx * (1.0 - fz),
            (1.0 - fx) * fz,
            fx * fz,
        ];
        // The nearest sample's layer (the terrain blends a patch's taps), the rest bilinear.
        let mut out = corners[(fx >= 0.5) as usize + 2 * (fz >= 0.5) as usize];
        out.weight = 0.0;
        out.tint = [0.0; 3];
        out.tint_amount = 0.0;
        for (c, w) in corners.iter().zip(weights) {
            out.weight += c.weight * w;
            out.tint_amount += c.tint_amount * w;
            for (t, v) in out.tint.iter_mut().zip(c.tint) {
                *t += v * w;
            }
        }
        out
    }

    fn tile(&self, level: u32, tx: i64, tz: i64, spacing: f64) -> Tile {
        let tile = {
            let mut tiles = self.tiles.lock().unwrap();
            if tiles.map.len() >= MAX_TILES {
                // Forget the half asked for least recently.
                let mut used: Vec<u64> = tiles.map.values().map(|t| t.1).collect();
                let (_, &mut median, _) = used.select_nth_unstable(MAX_TILES / 2);
                tiles.map.retain(|_, t| t.1 > median);
            }
            tiles.clock += 1;
            let now = tiles.clock;
            let entry = tiles.map.entry((level, tx, tz)).or_default();
            entry.1 = now;
            entry.0.clone()
        };
        // Bake outside the lock; another thread asking for the same tile waits for this one.
        tile.get_or_init(|| {
            let origin = (tx * TILE as i64, tz * TILE as i64);
            let mut samples = Vec::with_capacity((TILE + 1) * (TILE + 1));
            for j in 0..=TILE {
                for i in 0..=TILE {
                    let x = (origin.0 + i as i64) as f64 * spacing;
                    let z = (origin.1 + j as i64) as f64 * spacing;
                    samples.push((self.exact)(&self.land, x, z));
                }
            }
            samples.into_boxed_slice()
        });
        tile
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> PaintCache {
        PaintCache::new(crate::landscape::shared().clone(), crate::planet::paint)
    }

    #[test]
    fn baked_paint_matches_the_landscape_on_its_grid_and_blends_between() {
        let cache = cache();
        let land = crate::landscape::shared();
        // Up close: exact.
        assert_eq!(
            cache.paint_for(123.4, -56.7, 1.0),
            crate::planet::paint(land, 123.4, -56.7)
        );
        for footprint in [5.0, 40.0, 3000.0] {
            let level = ((footprint / FINEST_M).log2() as u32).min(LEVELS - 1);
            let spacing = FINEST_M * (1u32 << level) as f64;
            for (i, j) in [(0, 0), (31, -3), (-70, 45), (500, 12)] {
                let (x, z) = (i as f64 * spacing, j as f64 * spacing);
                assert_eq!(
                    cache.paint_for(x, z, footprint),
                    crate::planet::paint(land, x, z)
                );
                // Halfway to the next sample: the mean of the two tints.
                let mid = cache.paint_for(x + spacing * 0.5, z, footprint);
                let (a, b) = (
                    crate::planet::paint(land, x, z),
                    crate::planet::paint(land, x + spacing, z),
                );
                for k in 0..3 {
                    assert!((mid.tint[k] - (a.tint[k] + b.tint[k]) * 0.5).abs() < 1e-5);
                }
            }
        }
    }
}
