//! The hand-made ground's paint as a map, so far patches show it per pixel.
//!
//! An overlay's paint (roads, fields, a plaza: [`PlanetTerrain::with_overlay_paint`]) is
//! carried per vertex. Up close vertices are under two meters apart and that is sharp enough,
//! but from a few hundred meters up they are ten or twenty meters apart: a 5 m road averaged
//! over each vertex's cell shows only where a vertex lands near it, as dashes that crawl as
//! patches split. Engines keep painted masks in textures sampled per pixel, apart from the
//! geometry's level of detail (Frostbite: Andersson 2007, "Terrain Rendering in Frostbite
//! Using Procedural Shader Splatting"; Unreal's landscape weightmaps). So does this: the paint
//! is baked into a map around the home spot ([`PAINT_MAP_HALF_M`] each way, a texel every
//! [`PAINT_MAP_TEXEL_M`]), on worker threads, and the shader reads it beyond the near patches.
//!
//! [`PlanetTerrain::with_overlay_paint`]: crate::PlanetTerrain::with_overlay_paint

use std::cell::Cell;

use crate::shape::{HeightOverlay, OverlayPaintFn};

/// The map reaches this far from the home spot along both axes, meters.
pub const PAINT_MAP_HALF_M: f64 = 2_048.0;
/// Size of a texel, meters.
pub const PAINT_MAP_TEXEL_M: f64 = 2.0;

thread_local! {
    static FOOTPRINT: Cell<f64> = const { Cell::new(f64::NAN) };
}

/// How much ground the overlay paint asked for on this thread stands for, meters, when the
/// terrain says: the paint map's bake asks at its texel size. A paint function that answers
/// from coarser grids away from the camera (to keep patch builds cheap) should answer these
/// asks at this size, or the map holds its coarse grid's aliasing. `None` elsewhere.
pub fn paint_footprint() -> Option<f64> {
    let m = FOOTPRINT.with(Cell::get);
    (!m.is_nan()).then_some(m)
}

/// The paint of a square of hand-made ground in the overlay's frame (x east, z south).
#[derive(Debug, Clone, PartialEq)]
pub struct PaintMap {
    /// Overlay-frame `(x, z)` of texel (0, 0)'s center, meters.
    pub min: [f64; 2],
    pub texel_m: f64,
    /// Texels per side.
    pub size: u32,
    /// Per texel, row by row (rows along +z): layer, weight, tint amount and 255 where
    /// painted (0 elsewhere).
    pub paint: Vec<[u8; 4]>,
    /// Per texel: the tint's linear color, square-rooted (as the vertices carry it).
    pub tint: Vec<[u8; 4]>,
}

impl PaintMap {
    /// Bake the overlay's paint over `half_m` each way of the home spot, a texel every
    /// `texel_m`, on all cores. `None` without paint.
    pub fn bake(overlay: &HeightOverlay, half_m: f64, texel_m: f64) -> Option<Self> {
        let paint = overlay.paint.as_ref()?;
        Some(Self::bake_paint(
            paint,
            overlay.radius_m,
            overlay.blend_m,
            half_m,
            texel_m,
        ))
    }

    /// Bake `paint`, at full strength within `radius_m` of the home spot and fading out over
    /// `blend_m` beyond.
    pub fn bake_paint(
        paint: &OverlayPaintFn,
        radius_m: f64,
        blend_m: f64,
        half_m: f64,
        texel_m: f64,
    ) -> Self {
        let half_m = half_m.min(radius_m + blend_m);
        let size = ((2.0 * half_m / texel_m).round() as u32).max(1);
        let min = -half_m + texel_m * 0.5;
        let (radius, blend) = (radius_m, blend_m);
        // Half the cores: patch builds run beside it.
        let threads = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1));
        let rows_each = (size as usize).div_ceil(threads);
        let n = size as usize;
        let mut texels = vec![([0u8; 4], [0u8; 4]); n * n];
        std::thread::scope(|scope| {
            for (chunk_i, chunk) in texels.chunks_mut(rows_each * n).enumerate() {
                let paint = &paint;
                scope.spawn(move || {
                    FOOTPRINT.with(|f| f.set(texel_m));
                    for (k, out) in chunk.iter_mut().enumerate() {
                        let (row, col) = (chunk_i * rows_each + k / n, k % n);
                        let x = min + col as f64 * texel_m;
                        let z = min + row as f64 * texel_m;
                        // Faded out with the overlay's blend, as `HeightOverlay::paint_at`.
                        let d = x.hypot(z);
                        let t = 1.0 - smooth(radius, radius + blend, d) as f32;
                        if t <= 0.0 {
                            continue;
                        }
                        let Some(p) = paint(x, z) else {
                            continue;
                        };
                        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                        out.0 = [p.layer, byte(p.weight * t), byte(p.tint_amount * t), 255];
                        out.1 = [
                            byte(p.tint[0].max(0.0).sqrt()),
                            byte(p.tint[1].max(0.0).sqrt()),
                            byte(p.tint[2].max(0.0).sqrt()),
                            0,
                        ];
                    }
                });
            }
        });
        let (paint, tint) = texels.into_iter().unzip();
        Self {
            min: [min, min],
            texel_m,
            size,
            paint,
            tint,
        }
    }

    /// The baked paint at overlay-frame `(x, z)` (the nearest texel; for tests and tools):
    /// layer, weight, tint amount.
    pub fn at(&self, x: f64, z: f64) -> Option<(u8, f32, f32)> {
        let col = ((x - self.min[0]) / self.texel_m).round();
        let row = ((z - self.min[1]) / self.texel_m).round();
        let n = self.size as f64;
        if col < 0.0 || row < 0.0 || col >= n || row >= n {
            return None;
        }
        let p = self.paint[row as usize * self.size as usize + col as usize];
        (p[3] > 0).then(|| (p[0], p[1] as f32 / 255.0, p[2] as f32 / 255.0))
    }
}

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OverlayPaint, PlanetTerrain};

    #[test]
    fn a_road_bakes_into_the_map_where_it_runs() {
        let t = PlanetTerrain::earthlike(3)
            .with_overlay(3_000.0, 1_000.0, |_, _| 0.0)
            .with_overlay_paint(|x, z| {
                // Asked at the map's texel size.
                assert_eq!(paint_footprint(), Some(PAINT_MAP_TEXEL_M));
                // A 5 m dirt road along z = 100, a wheat field tinted gold.
                if (z - 100.0).abs() < 2.5 {
                    Some(OverlayPaint::layer("dirt", 1.0))
                } else if (300.0..400.0).contains(&x) && (300.0..400.0).contains(&z) {
                    Some(OverlayPaint::tint([0.36, 0.27, 0.09], 0.8))
                } else {
                    None
                }
            });
        let overlay = t.shape.overlay.as_ref().unwrap();
        let start = std::time::Instant::now();
        let map = PaintMap::bake(overlay, 600.0, PAINT_MAP_TEXEL_M).expect("painted");
        assert!(start.elapsed().as_secs_f64() < 5.0);
        assert_eq!(map.size, 600);
        let dirt = crate::edits::paint_index("dirt");
        for x in [-500.0, -3.0, 0.0, 250.0, 599.0] {
            assert_eq!(map.at(x, 101.0).map(|p| p.0), Some(dirt), "road at {x}");
            assert_eq!(map.at(x, 110.0), None, "beside the road at {x}");
        }
        let (layer, weight, tint) = map.at(350.0, 350.0).expect("field");
        assert_eq!((layer, weight), (0, 0.0));
        assert!((tint - 0.8).abs() < 0.01);
        assert_eq!(map.at(700.0, 0.0), None, "outside the map");
        assert_eq!(paint_footprint(), None);
    }
}
