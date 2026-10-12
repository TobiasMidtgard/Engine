//! Terrain edits: craters, trenches, mounds, smoothing and painted materials.
//!
//! [`TerrainEdit`](engine_world::TerrainEdit) messages arrive in world space; the plugin
//! converts them into the terrain's local frame and stores them here. Edits are kept as
//! analytic shapes (not baked into a grid), so they are exact at every level of detail and
//! cost nothing until something samples near them. A spatial hash finds the edits near a
//! point, in the order they were made.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::math::{DVec3, Quat};
use engine_world::{EditOp, EditShape};

/// Painted surface layers the terrain shader knows (index stored per vertex).
pub const PAINT_NAMES: [&str; 9] = [
    "", "mud", "snow", "sand", "dirt", "grass", "rock", "concrete", "ash",
];

/// Paint index for a surface material name (unknown names paint as dirt; empty = none).
pub fn paint_index(name: &str) -> u8 {
    match name {
        "mud" => 1,
        "snow" | "ice" => 2,
        "sand" | "regolith" => 3,
        "dirt" | "default" => 4,
        "grass" | "leaves" => 5,
        "rock" | "metal" | "glass" => 6,
        "concrete" | "asphalt" => 7,
        "ash" | "scorched" => 8,
        _ => 4,
    }
}

/// One edit in terrain-local space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalEdit {
    pub center: DVec3,
    pub shape: EditShape,
    pub op: EditOp,
    /// Paint layer for `Add`/`Paint` (see [`PAINT_NAMES`]), 0 = keep.
    pub paint: u8,
}

impl LocalEdit {
    /// Radius of a sphere around `center` that contains the whole edit.
    pub fn reach(&self) -> f64 {
        match self.shape {
            EditShape::Sphere { radius } => radius as f64,
            EditShape::Box { half_extents, .. } => half_extents.length() as f64,
            EditShape::Capsule { to, radius } => (to.length() + radius) as f64,
        }
    }

    /// Heights (along `up`) where the column through `base` enters and leaves the shape.
    fn column_span(&self, base: DVec3, up: DVec3) -> Option<(f64, f64, f64)> {
        match self.shape {
            EditShape::Sphere { radius } => sphere_span(self.center, radius as f64, base, up),
            EditShape::Capsule { to, radius } => {
                // Closest point of the segment to the column, then a sphere there.
                let a = self.center;
                let b = self.center + to.as_dvec3();
                let ab = b - a;
                let len2 = ab.length_squared().max(1e-12);
                // Project the column onto the segment by its horizontal distance.
                let rel = base - a;
                let rel_h = rel - up * rel.dot(up);
                let ab_h = ab - up * ab.dot(up);
                let t = if ab_h.length_squared() > 1e-9 {
                    (rel_h.dot(ab_h) / ab_h.length_squared()).clamp(0.0, 1.0)
                } else {
                    (rel.dot(ab) / len2).clamp(0.0, 1.0)
                };
                sphere_span(a + ab * t, radius as f64, base, up)
            }
            EditShape::Box {
                half_extents,
                rotation,
            } => box_span(self.center, half_extents.as_dvec3(), rotation, base, up),
        }
    }

    /// Apply to a height at the column through `base` (the surface point at height 0, `up`
    /// the local vertical). `coarse` returns the height with less detail (for smoothing).
    pub fn apply(
        &self,
        base: DVec3,
        up: DVec3,
        height: f64,
        coarse: &dyn Fn(f64) -> f64,
        paint: &mut (u8, f32),
    ) -> f64 {
        match self.op {
            EditOp::Remove => {
                let Some((bottom, top, edge)) = self.column_span(base, up) else {
                    return height;
                };
                // Only cut where the shape reaches the surface (heightfields can't hold caves).
                if top < height - 0.05 && edge > 0.0 {
                    return height;
                }
                smin(height, bottom, 0.15 + 0.1 * edge.min(4.0))
            }
            EditOp::Add => {
                let Some((bottom, top, _)) = self.column_span(base, up) else {
                    return height;
                };
                if bottom > height + 0.05 {
                    return height;
                }
                if self.paint != 0 {
                    *paint = (self.paint, 1.0);
                }
                smax(height, top, 0.2)
            }
            EditOp::Smooth { strength } => {
                let r = self.reach();
                let d = horizontal_distance(self.center, base, up);
                if d >= r {
                    return height;
                }
                let w = (1.0 - smoothstep(0.5 * r, r, d)) * strength.clamp(0.0, 1.0) as f64;
                height + (coarse(r) - height) * w
            }
            EditOp::Paint => {
                let r = self.reach();
                let d = horizontal_distance(self.center, base, up);
                if d < r && self.paint != 0 {
                    let w = (1.0 - smoothstep(0.7 * r, r, d)) as f32;
                    if paint.0 == self.paint {
                        paint.1 = paint.1.max(w);
                    } else if w >= paint.1 * 0.5 {
                        *paint = (self.paint, w);
                    }
                }
                height
            }
        }
    }
}

fn horizontal_distance(center: DVec3, base: DVec3, up: DVec3) -> f64 {
    let rel = center - base;
    (rel - up * rel.dot(up)).length()
}

/// (bottom, top, depth of the chord) for a column through a sphere.
fn sphere_span(center: DVec3, r: f64, base: DVec3, up: DVec3) -> Option<(f64, f64, f64)> {
    let rel = center - base;
    let t = rel.dot(up);
    let d2 = (rel - up * t).length_squared();
    let r2 = r * r;
    if d2 >= r2 {
        return None;
    }
    let half = (r2 - d2).sqrt();
    Some((t - half, t + half, half))
}

/// (bottom, top, chord length) for a column through an oriented box (slab test).
fn box_span(
    center: DVec3,
    half: DVec3,
    rotation: Quat,
    base: DVec3,
    up: DVec3,
) -> Option<(f64, f64, f64)> {
    let inv = rotation.inverse().as_dquat();
    let o = inv * (base - center);
    let d = inv * up;
    let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
    for i in 0..3 {
        let (oi, di, hi) = (o[i], d[i], half[i]);
        if di.abs() < 1e-12 {
            if oi.abs() > hi {
                return None;
            }
        } else {
            let a = (-hi - oi) / di;
            let b = (hi - oi) / di;
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
    }
    (t1 > t0).then_some((t0, t1, t1 - t0))
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth minimum (polynomial, blend width `k`).
fn smin(a: f64, b: f64, k: f64) -> f64 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

fn smax(a: f64, b: f64, k: f64) -> f64 {
    -smin(-a, -b, k)
}

type Cell = (i64, i64, i64);

/// Every edit on one terrain, bucketed for fast lookup. Cheap to clone (shared buckets);
/// chunk builders and ground queries hold an `Arc` snapshot.
#[derive(Debug, Clone, Default)]
pub struct EditLayer {
    buckets: HashMap<Cell, Arc<Vec<LocalEdit>>>,
    /// Edits too large to bucket (checked everywhere).
    large: Arc<Vec<LocalEdit>>,
    count: usize,
}

const BUCKET: f64 = 32.0;
const MAX_BUCKETS_PER_EDIT: i64 = 4096;

impl EditLayer {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn add(&mut self, edit: LocalEdit) {
        let reach = edit.reach() + 1.0;
        let lo = ((edit.center - reach) / BUCKET).floor();
        let hi = ((edit.center + reach) / BUCKET).floor();
        let n = (hi - lo + 1.0).max(DVec3::ONE);
        self.count += 1;
        if (n.x * n.y * n.z) as i64 > MAX_BUCKETS_PER_EDIT {
            Arc::make_mut(&mut self.large).push(edit);
            return;
        }
        for z in lo.z as i64..=hi.z as i64 {
            for y in lo.y as i64..=hi.y as i64 {
                for x in lo.x as i64..=hi.x as i64 {
                    Arc::make_mut(self.buckets.entry((x, y, z)).or_default()).push(edit);
                }
            }
        }
    }

    /// Apply every edit near the surface column at `base` to `height`. Returns the new
    /// height and the painted layer (index, weight).
    pub fn apply(
        &self,
        base: DVec3,
        up: DVec3,
        height: f64,
        coarse: &dyn Fn(f64) -> f64,
    ) -> (f64, (u8, f32)) {
        let mut paint = (0u8, 0.0f32);
        if self.count == 0 {
            return (height, paint);
        }
        let mut h = height;
        let p = base + up * height;
        let cell = (
            (p.x / BUCKET).floor() as i64,
            (p.y / BUCKET).floor() as i64,
            (p.z / BUCKET).floor() as i64,
        );
        for e in self.large.iter() {
            h = e.apply(base, up, h, coarse, &mut paint);
        }
        if let Some(list) = self.buckets.get(&cell) {
            for e in list.iter() {
                h = e.apply(base, up, h, coarse, &mut paint);
            }
        }
        (h, paint)
    }

    /// Whether any edit could touch a sphere (terrain-local center and radius).
    pub fn touches(&self, center: DVec3, radius: f64) -> bool {
        if self.count == 0 {
            return false;
        }
        if self
            .large
            .iter()
            .any(|e| (e.center - center).length() < e.reach() + radius)
        {
            return true;
        }
        let lo = ((center - radius) / BUCKET).floor();
        let hi = ((center + radius) / BUCKET).floor();
        let n = hi - lo + 1.0;
        if n.x * n.y * n.z > (self.buckets.len() as f64) {
            return self
                .buckets
                .values()
                .flat_map(|b| b.iter())
                .any(|e| (e.center - center).length() < e.reach() + radius);
        }
        for z in lo.z as i64..=hi.z as i64 {
            for y in lo.y as i64..=hi.y as i64 {
                for x in lo.x as i64..=hi.x as i64 {
                    if self.buckets.contains_key(&(x, y, z)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// All edits in the order they were made (for saving).
    pub fn all(&self) -> Vec<LocalEdit> {
        let mut seen = Vec::new();
        for e in self.large.iter() {
            seen.push(*e);
        }
        for list in self.buckets.values() {
            for e in list.iter() {
                if !seen.contains(e) {
                    seen.push(*e);
                }
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_: f64) -> f64 {
        0.0
    }

    #[test]
    fn crater_lowers_the_ground_and_mound_raises_it() {
        let mut layer = EditLayer::default();
        layer.add(LocalEdit {
            center: DVec3::new(0.0, 0.0, 0.0),
            shape: EditShape::Sphere { radius: 3.0 },
            op: EditOp::Remove,
            paint: 0,
        });
        layer.add(LocalEdit {
            center: DVec3::new(20.0, 0.0, 0.0),
            shape: EditShape::Sphere { radius: 2.0 },
            op: EditOp::Add,
            paint: paint_index("sand"),
        });
        let (h, _) = layer.apply(DVec3::ZERO, DVec3::Y, 0.0, &flat);
        assert!((h + 3.0).abs() < 0.3, "{h}");
        let (h, paint) = layer.apply(DVec3::new(20.0, 0.0, 0.0), DVec3::Y, 0.0, &flat);
        assert!((h - 2.0).abs() < 0.3, "{h}");
        assert_eq!(paint.0, paint_index("sand"));
        let (h, _) = layer.apply(DVec3::new(10.0, 0.0, 0.0), DVec3::Y, 0.0, &flat);
        assert_eq!(h, 0.0);
    }

    #[test]
    fn buried_sphere_does_not_dig_a_pit() {
        let mut layer = EditLayer::default();
        layer.add(LocalEdit {
            center: DVec3::new(0.0, -20.0, 0.0),
            shape: EditShape::Sphere { radius: 3.0 },
            op: EditOp::Remove,
            paint: 0,
        });
        let (h, _) = layer.apply(DVec3::ZERO, DVec3::Y, 0.0, &flat);
        assert_eq!(h, 0.0);
    }

    #[test]
    fn trench_follows_the_capsule() {
        let mut layer = EditLayer::default();
        layer.add(LocalEdit {
            center: DVec3::new(0.0, 0.0, 0.0),
            shape: EditShape::Capsule {
                to: engine_core::math::Vec3::new(30.0, 0.0, 0.0),
                radius: 1.5,
            },
            op: EditOp::Remove,
            paint: 0,
        });
        for x in [0.0, 10.0, 25.0] {
            let (h, _) = layer.apply(DVec3::new(x, 0.0, 0.0), DVec3::Y, 0.0, &flat);
            assert!(h < -1.0, "x {x}: {h}");
        }
        let (h, _) = layer.apply(DVec3::new(10.0, 0.0, 5.0), DVec3::Y, 0.0, &flat);
        assert_eq!(h, 0.0);
    }

    #[test]
    fn paint_fades_at_the_edge() {
        let mut layer = EditLayer::default();
        layer.add(LocalEdit {
            center: DVec3::ZERO,
            shape: EditShape::Sphere { radius: 10.0 },
            op: EditOp::Paint,
            paint: paint_index("mud"),
        });
        let (_, inner) = layer.apply(DVec3::ZERO, DVec3::Y, 0.0, &flat);
        let (_, edge) = layer.apply(DVec3::new(9.0, 0.0, 0.0), DVec3::Y, 0.0, &flat);
        assert_eq!(inner, (1, 1.0));
        assert!(edge.1 < 1.0 && edge.1 > 0.0);
        assert!(layer.touches(DVec3::new(5.0, 0.0, 0.0), 1.0));
        assert!(!layer.touches(DVec3::new(500.0, 0.0, 0.0), 1.0));
    }
}
